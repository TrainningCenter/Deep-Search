
use crate::content::extract_text;
use crate::database::{Database, SearchExpression, SearchResult, SearchSortDirection, SearchSortField};
use crate::indexer::scan_paths_batched;
use rusqlite::Result;
use std::path::Path;


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
}

impl SearchEngine {
    pub fn open<P: AsRef<Path>>(database_path: P) -> Result<Self> {
        let database = Database::open(database_path)?;
        Ok(Self { database })
    }

    pub fn index_paths<P: AsRef<Path>>(
        &self,
        paths: &[P],
    ) -> IndexingReport {
        const INDEX_BATCH_SIZE: usize = 128;
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

        let scan = scan_paths_batched(paths, INDEX_BATCH_SIZE, |batch| {
            if self.database.mark_scan_seen(scan_id, batch).is_err() {
                indexing_failed = true;
                return false;
            }

            let mut pending_batch = Vec::with_capacity(batch.len());
            for file in batch {
                match self.database.is_file_unchanged(file) {
                    Ok(true) => {
                        report.files_unchanged += 1;
                    }
                    Ok(false) => {
                        let content = extract_text(&file.path, file.size_bytes);
                        pending_batch.push((file.clone(), content));
                    }
                    Err(_) => {
                        report.errors += 1;
                        indexing_failed = true;
                    }
                }
            }

            if !pending_batch.is_empty() {
                let saved = pending_batch.len() as u64;
                match self.database.replace_files_with_content_batch(&pending_batch) {
                    Ok(()) => report.files_saved += saved,
                    Err(_) => {
                        report.errors += saved;
                        indexing_failed = true;
                    }
                }
            }
            true
        });

        report.files_discovered = scan.files_discovered;
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
}
