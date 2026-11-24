use crate::database::{Database, SearchResult};
use crate::indexer::scan_paths;
use rusqlite::Result;
use std::path::Path;

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
            match self.database.upsert_file(file) {
                Ok(()) => report.files_saved += 1,
                Err(_) => report.errors += 1,
            }
        }

        report
    }

    pub fn indexed_file_count(&self) -> Result<u64> {
        self.database.file_count()
    }

    pub fn search_by_name(&self, term: &str) -> Result<Vec<SearchResult>> {
        self.database.search_by_name(term)
    }
}
