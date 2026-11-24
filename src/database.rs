use crate::{FileRecord, IndexingStatus};
use rusqlite::{params, Connection, Result};
use std::path::Path;

pub struct Database {
    connection: Connection,
}

impl Database {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let connection = Connection::open(path)?;

        let database = Self { connection };
        database.initialize()?;

        Ok(database)
    }

    pub fn initialize(&self) -> Result<()> {
        self.connection.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS files (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                extension TEXT,
                size_bytes INTEGER NOT NULL,
                modified_unix INTEGER,
                status TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_files_name
            ON files(name);

            CREATE INDEX IF NOT EXISTS idx_files_extension
            ON files(extension);

            CREATE INDEX IF NOT EXISTS idx_files_size
            ON files(size_bytes);
            "
        )?;

        Ok(())
    }

    pub fn upsert_file(&self, file: &FileRecord) -> Result<()> {
        let path = file.path.to_string_lossy().to_string();

        let size_bytes = file.size_bytes.min(i64::MAX as u64) as i64;

        let modified_unix = file
            .modified_unix
            .map(|value| value.min(i64::MAX as u64) as i64);

        let status = match file.status {
            IndexingStatus::Pending => "pending",
            IndexingStatus::Indexing => "indexing",
            IndexingStatus::Indexed => "indexed",
            IndexingStatus::Skipped => "skipped",
            IndexingStatus::Failed => "failed",
        };

        self.connection.execute(
            "
            INSERT INTO files (
                path,
                name,
                extension,
                size_bytes,
                modified_unix,
                status
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(path) DO UPDATE SET
                name = excluded.name,
                extension = excluded.extension,
                size_bytes = excluded.size_bytes,
                modified_unix = excluded.modified_unix,
                status = excluded.status
            ",
            params![
                path,
                file.name,
                file.extension,
                size_bytes,
                modified_unix,
                status
            ],
        )?;

        Ok(())
    }

    pub fn file_count(&self) -> Result<u64> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM files",
            [],
            |row| row.get(0),
        )?;

        Ok(count as u64)
    }
}
