
use crate::content::extract_text;
use crate::database::{Database, SearchExpression, SearchResult};
use crate::indexer::scan_paths;
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
        let scan = scan_paths(paths);

        let mut report = IndexingReport {
            files_discovered: scan.files.len() as u64,
            directories_scanned: scan.directories_scanned,
            errors: scan.errors,
            files_saved: 0,
            files_unchanged: 0,
            files_removed: 0,
        };

        let mut indexing_failed = false;
        for file in &scan.files {
            match self.database.is_file_unchanged(file) {
                Ok(true) => {
                    report.files_unchanged += 1;
                    continue;
                }
                Ok(false) => {}
                Err(_) => {
                    report.errors += 1;
                    indexing_failed = true;
                    continue;
                }
            }

            let content = extract_text(&file.path, file.size_bytes);
            match self.database.replace_file_with_content(file, content.as_deref()) {
                Ok(()) => report.files_saved += 1,
                Err(_) => {
                    report.errors += 1;
                    indexing_failed = true;
                }
            }
        }

        if !indexing_failed && !scan.completed_roots.is_empty() {
            let present_paths: Vec<_> = scan.files.iter().map(|file| file.path.clone()).collect();
            match self.database.prune_missing_files(&scan.completed_roots, &present_paths) {
                Ok(removed) => report.files_removed = removed,
                Err(_) => report.errors += 1,
            }
        }

        report
    }

    pub fn indexed_file_count(&self) -> Result<u64> {
        self.database.file_count()
    }

    pub fn search(&self, expression: &SearchExpression) -> Result<Vec<SearchResult>> {
        self.database.search(expression)
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
