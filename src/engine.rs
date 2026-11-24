
use crate::database::{Database, SearchExpression, SearchResult};
use crate::indexer::scan_paths;
use rusqlite::Result;
use std::fs;
use std::path::Path;

const MAX_TEXT_FILE_SIZE: u64 = 1024 * 1024;

fn read_indexable_content(path: &Path, size_bytes: u64) -> Option<String> {
    if size_bytes > MAX_TEXT_FILE_SIZE {
        return None;
    }

    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let supported = matches!(
        extension.as_str(),
        "txt" | "md" | "markdown" | "log" | "csv" | "tsv" |
        "json" | "jsonl" | "xml" | "html" | "htm" | "css" |
        "js" | "jsx" | "ts" | "tsx" | "rs" | "toml" |
        "yaml" | "yml" | "ini" | "conf" | "cfg" | "sql" |
        "py" | "sh" | "bash" | "c" | "h" | "cpp" | "hpp" |
        "java" | "go" | "php" | "rb" | "swift" | "kt" |
        "env" | "rst" | "tex"
    );
    if !supported {
        return None;
    }

    let content = fs::read_to_string(path).ok()?;
    if content.contains('\0') {
        return None;
    }
    Some(content)
}

#[derive(Debug, Default)]
pub struct IndexingReport {
    pub files_discovered: u64,
    pub directories_scanned: u64,
    pub errors: u64,
    pub files_saved: u64,
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
        };

        for file in &scan.files {
            let content = read_indexable_content(&file.path, file.size_bytes);
            match self.database.upsert_file_with_content(file, content.as_deref()) {
                Ok(()) => report.files_saved += 1,
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
        let content = read_indexable_content(&path, 24);
        assert_eq!(content.as_deref(), Some("searchable document text"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn skips_unsupported_binary_and_oversized_files() {
        let binary = temporary_file("txt", b"hello\0world");
        assert!(read_indexable_content(&binary, 11).is_none());
        fs::remove_file(binary).unwrap();

        let unsupported = temporary_file("pdf", b"%PDF-1.7");
        assert!(read_indexable_content(&unsupported, 8).is_none());
        fs::remove_file(unsupported).unwrap();

        let large = temporary_file("txt", b"small");
        assert!(read_indexable_content(&large, MAX_TEXT_FILE_SIZE + 1).is_none());
        fs::remove_file(large).unwrap();
    }
}
