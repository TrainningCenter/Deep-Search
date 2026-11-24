
use crate::{FileRecord, IndexingStatus};
use rusqlite::{params, params_from_iter, types::Value, Connection, Result};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub path: std::path::PathBuf,
    pub name: String,
    pub extension: Option<String>,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchExpression {
    NameContains(String),
    ExtensionEquals(String),
    SizeAtLeast(u64),
    SizeAtMost(u64),
    And(Vec<SearchExpression>),
    Or(Vec<SearchExpression>),
    Not(Box<SearchExpression>),
}

fn compile_expression(expression: &SearchExpression, values: &mut Vec<Value>) -> String {
    match expression {
        SearchExpression::NameContains(term) => {
            let escaped = term.trim()
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            values.push(Value::Text(format!("%{escaped}%")));
            format!("name LIKE ?{} ESCAPE '\\' COLLATE NOCASE", values.len())
        }
        SearchExpression::ExtensionEquals(extension) => {
            values.push(Value::Text(extension.trim().trim_start_matches('.').to_lowercase()));
            format!("extension = ?{} COLLATE NOCASE", values.len())
        }
        SearchExpression::SizeAtLeast(size) => {
            values.push(Value::Integer((*size).min(i64::MAX as u64) as i64));
            format!("size_bytes >= ?{}", values.len())
        }
        SearchExpression::SizeAtMost(size) => {
            values.push(Value::Integer((*size).min(i64::MAX as u64) as i64));
            format!("size_bytes <= ?{}", values.len())
        }
        SearchExpression::And(expressions) => {
            if expressions.is_empty() { return "1 = 1".to_string(); }
            let parts: Vec<String> = expressions.iter()
                .map(|item| compile_expression(item, values)).collect();
            format!("({})", parts.join(" AND "))
        }
        SearchExpression::Or(expressions) => {
            if expressions.is_empty() { return "0 = 1".to_string(); }
            let parts: Vec<String> = expressions.iter()
                .map(|item| compile_expression(item, values)).collect();
            format!("({})", parts.join(" OR "))
        }
        SearchExpression::Not(inner) => {
            format!("NOT ({})", compile_expression(inner, values))
        }
    }
}

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
            ",
        )?;

        Ok(())
    }

    pub fn upsert_file(&self, file: &FileRecord) -> Result<()> {
        let path = file.path.to_string_lossy().to_string();
        let size = file.size_bytes.min(i64::MAX as u64) as i64;

        let modified = file
            .modified_unix
            .map(|value| value.min(i64::MAX as u64) as i64);

        let status = match &file.status {
            IndexingStatus::Pending => "pending",
            IndexingStatus::Indexing => "indexing",
            IndexingStatus::Indexed => "indexed",
            IndexingStatus::Skipped => "skipped",
            IndexingStatus::Failed => "failed",
        };

        self.connection.execute(
            "
            INSERT INTO files
                (path, name, extension, size_bytes, modified_unix, status)
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
                size,
                modified,
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

        Ok(count.max(0) as u64)
    }

    pub fn search(&self, expression: &SearchExpression) -> Result<Vec<SearchResult>> {
        let mut values = Vec::new();
        let condition = compile_expression(expression, &mut values);
        let sql = format!(
            "SELECT path, name, extension, size_bytes FROM files WHERE {condition} ORDER BY name COLLATE NOCASE"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            let path: String = row.get(0)?;
            let size: i64 = row.get(3)?;
            Ok(SearchResult {
                path: path.into(),
                name: row.get(1)?,
                extension: row.get(2)?,
                size_bytes: size.max(0) as u64,
            })
        })?;
        rows.collect()
    }

    pub fn search_by_name(&self, term: &str) -> Result<Vec<SearchResult>> {
        let term = term.trim();
        if term.is_empty() {
            return Ok(Vec::new());
        }

        let escaped = term
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{}%", escaped);

        let mut statement = self.connection.prepare(
            "
            SELECT path, name, extension, size_bytes
            FROM files
            WHERE name LIKE ?1 ESCAPE '\\' COLLATE NOCASE
            ORDER BY name COLLATE NOCASE
            ",
        )?;

        let rows = statement.query_map([pattern], |row| {
            let path: String = row.get(0)?;
            let size: i64 = row.get(3)?;

            Ok(SearchResult {
                path: path.into(),
                name: row.get(1)?,
                extension: row.get(2)?,
                size_bytes: size.max(0) as u64,
            })
        })?;

        rows.collect()
    }

    pub fn search_by_extension(
        &self,
        extension: &str,
    ) -> Result<Vec<SearchResult>> {
        let extension = extension
            .trim_start_matches('.')
            .to_lowercase();

        let mut statement = self.connection.prepare(
            "
            SELECT path, name, extension, size_bytes
            FROM files
            WHERE extension = ?1 COLLATE NOCASE
            ORDER BY name COLLATE NOCASE
            ",
        )?;

        let rows = statement.query_map([extension], |row| {
            let path: String = row.get(0)?;
            let size: i64 = row.get(3)?;

            Ok(SearchResult {
                path: path.into(),
                name: row.get(1)?,
                extension: row.get(2)?,
                size_bytes: size.max(0) as u64,
            })
        })?;

        rows.collect()
    }

    pub fn search_by_size_range(
        &self,
        min_bytes: Option<u64>,
        max_bytes: Option<u64>,
    ) -> Result<Vec<SearchResult>> {
        let min_size = min_bytes.map(|value| value.min(i64::MAX as u64) as i64);
        let max_size = max_bytes.map(|value| value.min(i64::MAX as u64) as i64);

        let mut statement = self.connection.prepare(
            "
            SELECT path, name, extension, size_bytes
            FROM files
            WHERE (?1 IS NULL OR size_bytes >= ?1)
              AND (?2 IS NULL OR size_bytes <= ?2)
            ORDER BY size_bytes
            ",
        )?;

        let rows = statement.query_map(
            params![min_size, max_size],
            |row| {
                let path: String = row.get(0)?;
                let size: i64 = row.get(3)?;

                Ok(SearchResult {
                    path: path.into(),
                    name: row.get(1)?,
                    extension: row.get(2)?,
                    size_bytes: size.max(0) as u64,
                })
            },
        )?;

        rows.collect()
    }
}

#[cfg(test)]
mod database_tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_file(name: &str, size: u64) -> FileRecord {
        FileRecord {
            path: PathBuf::from(format!("/test/{name}")),
            name: name.to_string(),
            extension: name.rsplit('.').next().map(str::to_lowercase),
            size_bytes: size,
            modified_unix: Some(1_700_000_000),
            status: IndexingStatus::Pending,
        }
    }

    #[test]
    fn saves_and_counts_files() {
        let db = Database::open(":memory:").unwrap();

        db.upsert_file(&sample_file("video.mp4", 5000))
            .unwrap();

        assert_eq!(db.file_count().unwrap(), 1);
    }

    #[test]
    fn combines_search_conditions_with_and() {
        let db = Database::open(":memory:").unwrap();
        db.upsert_file(&sample_file("movie.mkv", 5_000)).unwrap();
        db.upsert_file(&sample_file("movie.mp4", 50_000)).unwrap();
        db.upsert_file(&sample_file("notes.mkv", 5_000)).unwrap();

        let query = SearchExpression::And(vec![
            SearchExpression::NameContains("movie".into()),
            SearchExpression::ExtensionEquals("mkv".into()),
            SearchExpression::SizeAtLeast(1_000),
            SearchExpression::SizeAtMost(10_000),
        ]);
        let results = db.search(&query).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "movie.mkv");
    }

    #[test]
    fn supports_or_and_not_search_conditions() {
        let db = Database::open(":memory:").unwrap();
        db.upsert_file(&sample_file("movie.mkv", 5_000)).unwrap();
        db.upsert_file(&sample_file("movie.mp4", 50_000)).unwrap();
        db.upsert_file(&sample_file("notes.txt", 500)).unwrap();

        let query = SearchExpression::And(vec![
            SearchExpression::Or(vec![
                SearchExpression::ExtensionEquals("mkv".into()),
                SearchExpression::ExtensionEquals("mp4".into()),
            ]),
            SearchExpression::Not(Box::new(SearchExpression::SizeAtLeast(20_000))),
        ]);
        let results = db.search(&query).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "movie.mkv");
    }

    #[test]
    fn empty_or_expression_matches_nothing() {
        let db = Database::open(":memory:").unwrap();
        db.upsert_file(&sample_file("report.pdf", 1_000)).unwrap();
        assert!(db.search(&SearchExpression::Or(vec![])).unwrap().is_empty());
    }

    #[test]
    fn searches_files_by_name() {
        let db = Database::open(":memory:").unwrap();

        db.upsert_file(&sample_file("report.pdf", 1000))
            .unwrap();

        let results = db.search_by_name("report").unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "report.pdf");
    }


    #[test]
    fn searches_percent_and_underscore_as_literal_characters() {
        let db = Database::open(":memory:").unwrap();

        db.upsert_file(&sample_file("100%_complete.txt", 100)).unwrap();
        db.upsert_file(&sample_file("100X_complete.txt", 100)).unwrap();

        let results = db.search_by_name("100%_").unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "100%_complete.txt");
    }

    #[test]
    fn empty_name_search_returns_no_results() {
        let db = Database::open(":memory:").unwrap();
        db.upsert_file(&sample_file("report.pdf", 1000)).unwrap();

        assert!(db.search_by_name("   ").unwrap().is_empty());
    }

    #[test]
    fn searches_files_by_extension() {
        let db = Database::open(":memory:").unwrap();

        db.upsert_file(&sample_file("movie.mkv", 5000))
            .unwrap();

        let results = db.search_by_extension(".mkv").unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].extension.as_deref(), Some("mkv"));
    }

    #[test]
    fn searches_files_by_size() {
        let db = Database::open(":memory:").unwrap();

        db.upsert_file(&sample_file("small.txt", 100))
            .unwrap();

        db.upsert_file(&sample_file("large.txt", 5000))
            .unwrap();

        let results = db
            .search_by_size_range(Some(1000), None)
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].size_bytes, 5000);
    }
}
