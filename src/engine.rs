
use crate::content::extract_text;
use crate::database::{Database, SearchExpression, SearchResult, SearchSortDirection, SearchSortField};
use crate::indexer::scan_paths_batched;
use crate::media::{extract_image_metadata, extract_media_metadata, is_supported_image_extension};
use rusqlite::Result;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::time::Duration;


#[derive(Debug, Default)]
pub struct IndexingReport {
    pub files_discovered: u64,
    pub directories_scanned: u64,
    pub errors: u64,
    pub files_saved: u64,
    pub files_unchanged: u64,
    pub files_removed: u64,
}

pub struct SearchEngine {
    database: Database,
    database_path: PathBuf,
    indexing_paused: Arc<AtomicBool>,
}

impl SearchEngine {
    pub fn open<P: AsRef<Path>>(database_path: P) -> Result<Self> {
        let requested_path = database_path.as_ref().to_path_buf();
        let database = Database::open(&requested_path)?;
        let database_path = std::fs::canonicalize(&requested_path).unwrap_or(requested_path);
        Ok(Self { database, database_path, indexing_paused: Arc::new(AtomicBool::new(false)) })
    }

    /// Connect to an initialized database and share indexing pause state with
    /// other service workers and the filesystem watcher.
    pub fn connect_existing_with_indexing_state<P: AsRef<Path>>(
        database_path: P,
        indexing_paused: Arc<AtomicBool>,
    ) -> Result<Self> {
        let requested_path = database_path.as_ref().to_path_buf();
        let database = Database::connect_existing(&requested_path)?;
        let database_path = std::fs::canonicalize(&requested_path).unwrap_or(requested_path);
        Ok(Self { database, database_path, indexing_paused })
    }

    /// Connect to an initialized database without sharing pause state.
    pub fn connect_existing<P: AsRef<Path>>(database_path: P) -> Result<Self> {
        Self::connect_existing_with_indexing_state(
            database_path,
            Arc::new(AtomicBool::new(false)),
        )
    }

    pub fn set_indexing_paused(&self, paused: bool) {
        self.indexing_paused.store(paused, Ordering::Release);
    }

    pub fn is_indexing_paused(&self) -> bool {
        self.indexing_paused.load(Ordering::Acquire)
    }

    pub fn index_roots(&self) -> Result<Vec<PathBuf>> {
        self.database.index_roots()
    }

    pub fn stop_on_logout(&self) -> Result<bool> {
        self.database.stop_on_logout()
    }

    pub fn set_stop_on_logout(&self, enabled: bool) -> Result<()> {
        self.database.set_stop_on_logout(enabled)
    }

    pub fn index_paths<P: AsRef<Path>>(
        &self,
        paths: &[P],
    ) -> IndexingReport {
        self.index_paths_internal(paths, true)
    }

    fn index_paths_internal<P: AsRef<Path>>(
        &self,
        paths: &[P],
        persist_roots: bool,
    ) -> IndexingReport {
        const INDEX_BATCH_SIZE: usize = 128;
        if persist_roots && self.database.add_index_roots(paths).is_err() {
            return IndexingReport { errors: 1, ..IndexingReport::default() };
        }
        let scan_id = match self.database.begin_scan() {
            Ok(id) => id,
            Err(_) => {
                return IndexingReport {
                    errors: 1,
                    ..IndexingReport::default()
                };
            }
        };

        let mut report = IndexingReport::default();
        let mut indexing_failed = false;
        let mut internal_files_skipped = 0_u64;

        let scan = scan_paths_batched(paths, INDEX_BATCH_SIZE, |batch| {
            // Pause at batch boundaries so file traversal and metadata extraction
            // stop promptly without abandoning the active database transaction.
            while self.indexing_paused.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(100));
            }
            let filtered_batch = batch.iter().filter(|file| {
                if is_database_sidecar(&file.path, &self.database_path) {
                    internal_files_skipped += 1;
                    false
                } else {
                    true
                }
            }).cloned().collect::<Vec<_>>();
            if filtered_batch.is_empty() {
                return true;
            }
            if self.database.mark_scan_seen(scan_id, &filtered_batch).is_err() {
                indexing_failed = true;
                return false;
            }

            // SQLite stays on the indexing thread, while independent file reads
            // and document extraction run concurrently across available CPU cores.
            let mut changed_files = Vec::with_capacity(filtered_batch.len());
            for file in &filtered_batch {
                match self.database.is_file_unchanged(file) {
                    Ok(true) => {
                        report.files_unchanged += 1;
                    }
                    Ok(false) => changed_files.push(file.clone()),
                    Err(_) => {
                        report.errors += 1;
                        indexing_failed = true;
                    }
                }
            }

            let extracted_batch: Vec<_> = changed_files.into_par_iter()
                .map(|file| {
                    let content = extract_text(&file.path, file.size_bytes);
                    let media = if is_supported_image_extension(file.extension.as_deref()) {
                        extract_image_metadata(&file.path, file.extension.as_deref())
                            .or_else(|| extract_media_metadata(&file.path, file.extension.as_deref()))
                    } else {
                        extract_media_metadata(&file.path, file.extension.as_deref())
                    };
                    (file, content, media)
                })
                .collect();

            if !extracted_batch.is_empty() {
                let saved = extracted_batch.len() as u64;
                let mut content_batch = Vec::with_capacity(extracted_batch.len());
                let mut media_batch = Vec::with_capacity(extracted_batch.len());
                for (file, content, media) in extracted_batch {
                    media_batch.push((file.path.clone(), media));
                    content_batch.push((file, content));
                }
                match self.database.replace_files_with_content_batch(&content_batch) {
                    Ok(()) => match self.database.replace_media_metadata_batch(&media_batch) {
                        Ok(()) => report.files_saved += saved,
                        Err(_) => {
                            report.errors += 1;
                            indexing_failed = true;
                        }
                    },
                    Err(_) => {
                        report.errors += saved;
                        indexing_failed = true;
                    }
                }
            }
            true
        });

        report.files_discovered = scan.files_discovered.saturating_sub(internal_files_skipped);
        report.directories_scanned = scan.directories_scanned;
        report.errors += scan.errors;

        if !indexing_failed && scan.errors == 0 && !scan.completed_roots.is_empty() {
            match self.database.finish_scan(scan_id, &scan.completed_roots) {
                Ok(removed) => report.files_removed = removed,
                Err(_) => {
                    report.errors += 1;
                    let _ = self.database.discard_scan(scan_id);
                }
            }
        } else {
            let _ = self.database.discard_scan(scan_id);
        }

        report
    }

    /// Refresh only paths reported by the filesystem watcher. Removed paths are
    /// deleted directly; existing files or directories are scanned incrementally.
    pub fn refresh_changed_paths(&self, paths: &[PathBuf]) -> IndexingReport {
        let mut total = IndexingReport::default();
        for path in paths {
            let report = if path.exists() {
                self.index_paths_internal(std::slice::from_ref(path), false)
            } else {
                match self.database.remove_path_tree(path) {
                    Ok(removed) => IndexingReport { files_removed: removed, ..IndexingReport::default() },
                    Err(_) => IndexingReport { errors: 1, ..IndexingReport::default() },
                }
            };
            total.files_discovered += report.files_discovered;
            total.directories_scanned += report.directories_scanned;
            total.errors += report.errors;
            total.files_saved += report.files_saved;
            total.files_unchanged += report.files_unchanged;
            total.files_removed += report.files_removed;
        }
        total
    }

    pub fn indexed_file_count(&self) -> Result<u64> {
        self.database.file_count()
    }

    pub fn search(&self, expression: &SearchExpression) -> Result<Vec<SearchResult>> {
        self.database.search(expression)
    }

    /// Search a bounded result page. The database enforces a maximum page size
    /// of 1000 records.
    pub fn search_page(
        &self,
        expression: &SearchExpression,
        limit: u32,
        offset: u64,
    ) -> Result<Vec<SearchResult>> {
        self.database.search_page(expression, limit, offset)
    }

    pub fn search_page_sorted(
        &self,
        expression: &SearchExpression,
        limit: u32,
        offset: u64,
        sort_field: SearchSortField,
        sort_direction: SearchSortDirection,
    ) -> Result<Vec<SearchResult>> {
        self.database.search_page_sorted(
            expression, limit, offset, sort_field, sort_direction,
        )
    }

    pub fn count_search_results(&self, expression: &SearchExpression) -> Result<u64> {
        self.database.count_search_results(expression)
    }

    pub fn search_content(&self, term: &str) -> Result<Vec<SearchResult>> {
        self.database.search_content(term)
    }

    pub fn search_by_name(&self, term: &str) -> Result<Vec<SearchResult>> {
        self.database.search_by_name(term)
    }

    pub fn search_by_extension(
        &self,
        extension: &str,
    ) -> Result<Vec<SearchResult>> {
        self.database.search_by_extension(extension)
    }

    pub fn search_by_size_range(
        &self,
        min_bytes: Option<u64>,
        max_bytes: Option<u64>,
    ) -> Result<Vec<SearchResult>> {
        self.database.search_by_size_range(min_bytes, max_bytes)
    }
}

fn is_database_sidecar(candidate: &Path, database_path: &Path) -> bool {
    let database = database_path.to_string_lossy().into_owned();
    let candidates = [
        database_path.to_path_buf(),
        PathBuf::from(format!("{database}-wal")),
        PathBuf::from(format!("{database}-shm")),
        PathBuf::from(format!("{database}-journal")),
    ];
    candidates.iter().any(|path| {
        candidate == path
            || (std::fs::canonicalize(candidate).ok().is_some()
                && std::fs::canonicalize(candidate).ok() == std::fs::canonicalize(path).ok())
    })
}

#[cfg(test)]
mod content_indexing_tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_file(extension: &str, contents: &[u8]) -> std::path::PathBuf {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!(
            "deep-search-content-{}-{unique}.{extension}",
            std::process::id()
        ));
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn indexing_waits_at_batch_boundaries_while_paused() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        use std::thread;
        use std::time::Duration;

        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let base = std::env::temp_dir().join(format!(
            "deep-search-pause-{}-{unique}", std::process::id()
        ));
        let root = base.join("files");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("paused.txt"), b"resume me").unwrap();

        let database_path = base.join("index.sqlite");
        drop(SearchEngine::open(&database_path).unwrap());
        let paused = Arc::new(AtomicBool::new(true));
        let engine = SearchEngine::connect_existing_with_indexing_state(
            &database_path, Arc::clone(&paused),
        ).unwrap();
        let worker = thread::spawn(move || engine.index_paths(&[root.as_path()]));

        thread::sleep(Duration::from_millis(250));
        assert!(!worker.is_finished(), "indexing must remain paused until resumed");
        paused.store(false, Ordering::Release);
        let report = worker.join().unwrap();
        assert_eq!(report.errors, 0);
        assert_eq!(report.files_saved, 1);

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn reads_supported_utf8_text_files() {
        let path = temporary_file("txt", "searchable document text".as_bytes());
        let content = extract_text(&path, 24);
        assert_eq!(content.as_deref(), Some("searchable document text"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn removes_deleted_files_after_a_complete_rescan() {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let base = std::env::temp_dir().join(format!(
            "deep-search-reconcile-{}-{unique}",
            std::process::id()
        ));
        let scan_root = base.join("data");
        fs::create_dir_all(&scan_root).unwrap();
        let file_path = scan_root.join("temporary.txt");
        fs::write(&file_path, b"indexed once").unwrap();

        let engine = SearchEngine::open(base.join("index.sqlite")).unwrap();
        let first = engine.index_paths(&[scan_root.as_path()]);
        assert_eq!(first.errors, 0);
        assert_eq!(first.files_saved, 1);
        assert_eq!(engine.indexed_file_count().unwrap(), 1);

        fs::remove_file(&file_path).unwrap();
        let second = engine.index_paths(&[scan_root.as_path()]);
        assert_eq!(second.errors, 0);
        assert_eq!(second.files_removed, 1);
        assert_eq!(engine.indexed_file_count().unwrap(), 0);

        drop(engine);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn preserves_existing_records_when_a_root_becomes_unavailable() {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let base = std::env::temp_dir().join(format!(
            "deep-search-unavailable-root-{}-{unique}",
            std::process::id()
        ));
        let scan_root = base.join("data");
        fs::create_dir_all(&scan_root).unwrap();
        fs::write(scan_root.join("kept.txt"), b"still indexed").unwrap();

        let engine = SearchEngine::open(base.join("index.sqlite")).unwrap();
        let first = engine.index_paths(&[scan_root.as_path()]);
        assert_eq!(first.errors, 0);
        assert_eq!(engine.indexed_file_count().unwrap(), 1);

        fs::remove_dir_all(&scan_root).unwrap();
        let second = engine.index_paths(&[scan_root.as_path()]);
        assert!(second.errors > 0);
        assert_eq!(second.files_removed, 0);
        assert_eq!(engine.indexed_file_count().unwrap(), 1);

        drop(engine);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn skips_unsupported_binary_and_oversized_files() {
        let binary = temporary_file("txt", b"hello\0world");
        assert!(extract_text(&binary, 11).is_none());
        fs::remove_file(binary).unwrap();

        let unsupported = temporary_file("pdf", b"%PDF-1.7");
        assert!(extract_text(&unsupported, 8).is_none());
        fs::remove_file(unsupported).unwrap();

        let large = temporary_file("txt", b"small");
        assert!(extract_text(&large, 1024 * 1024 + 1).is_none());
        fs::remove_file(large).unwrap();
    }
    #[test]
    fn persists_index_roots_and_never_indexes_its_own_database_files() {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let base = std::env::temp_dir().join(format!(
            "deep-search-self-index-{}-{unique}", std::process::id()
        ));
        let root = base.join("data");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("document.txt"), b"searchable").unwrap();

        let database_path = root.join("index.sqlite");
        let engine = SearchEngine::open(&database_path).unwrap();
        let report = engine.index_paths(&[root.as_path()]);

        assert_eq!(report.errors, 0);
        assert_eq!(report.files_discovered, 1);
        assert_eq!(report.files_saved, 1);
        assert_eq!(engine.index_roots().unwrap(), vec![root.clone()]);
        assert!(engine.search_by_name("index.sqlite").unwrap().is_empty());
        assert_eq!(engine.indexed_file_count().unwrap(), 1);

        drop(engine);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn opens_independent_connections_for_concurrent_service_clients() {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let base = std::env::temp_dir().join(format!(
            "deep-search-multi-client-{}-{unique}", std::process::id()
        ));
        let root = base.join("files");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("shared.txt"), b"concurrent readers").unwrap();
        let database_path = base.join("index.sqlite");

        let engine = SearchEngine::open(&database_path).unwrap();
        let indexed = engine.index_paths(&[root.as_path()]);
        assert_eq!(indexed.errors, 0);
        assert_eq!(indexed.files_saved, 1);
        drop(engine);

        let mut readers = Vec::new();
        for _ in 0..4 {
            let path = database_path.clone();
            readers.push(std::thread::spawn(move || {
                let client = SearchEngine::connect_existing(path).unwrap();
                assert_eq!(client.indexed_file_count().unwrap(), 1);
                assert_eq!(client.search_by_name("shared").unwrap().len(), 1);
            }));
        }
        for reader in readers { reader.join().unwrap(); }

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn refreshes_a_changed_file_and_removes_deleted_paths() {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let base = std::env::temp_dir().join(format!(
            "deep-search-watch-refresh-{}-{unique}", std::process::id()
        ));
        fs::create_dir_all(&base).unwrap();
        let file = base.join("watch-me.txt");
        fs::write(&file, b"first version").unwrap();

        let engine = SearchEngine::open(base.join("index.sqlite")).unwrap();
        let first = engine.index_paths(&[base.as_path()]);
        assert_eq!(first.files_saved, 1);

        fs::write(&file, b"second version with more bytes").unwrap();
        let changed = engine.refresh_changed_paths(std::slice::from_ref(&file));
        assert_eq!(changed.files_saved, 1);
        assert!(engine.search_content("second version").unwrap().len() == 1);

        fs::remove_file(&file).unwrap();
        let removed = engine.refresh_changed_paths(std::slice::from_ref(&file));
        assert_eq!(removed.files_removed, 1);
        assert!(engine.search_by_name("watch-me").unwrap().is_empty());

        drop(engine);
        fs::remove_dir_all(base).unwrap();
    }

}
