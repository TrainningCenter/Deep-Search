
use crate::{FileRecord, IndexingStatus};
use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension, Result};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub path: std::path::PathBuf,
    pub name: String,
    pub extension: Option<String>,
    pub size_bytes: u64,
    pub modified_timestamp_ns: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchSortField {
    Relevance,
    Name,
    Path,
    Extension,
    Size,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchSortDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchExpression {
    NameContains(String),
    PathContains(String),
    ContentContains(String),
    ExtensionEquals(String),
    FileType(String),
    SizeAtLeast(u64),
    SizeAtMost(u64),
    And(Vec<SearchExpression>),
    Or(Vec<SearchExpression>),
    Not(Box<SearchExpression>),
}

fn collect_name_terms<'a>(expression: &'a SearchExpression, terms: &mut Vec<&'a str>) {
    match expression {
        SearchExpression::NameContains(term) => terms.push(term.trim()),
        SearchExpression::And(items) | SearchExpression::Or(items) => {
            for item in items {
                collect_name_terms(item, terms);
            }
        }
        SearchExpression::Not(inner) => collect_name_terms(inner, terms),
        _ => {}
    }
}

fn name_relevance_order(expression: &SearchExpression, values: &mut Vec<Value>) -> String {
    let mut terms = Vec::new();
    collect_name_terms(expression, &mut terms);
    if terms.is_empty() {
        return "name COLLATE NOCASE".to_string();
    }

    let mut scores = Vec::new();
    for term in terms {
        let exact_index = values.len() + 1;
        values.push(Value::Text(term.to_string()));
        let prefix_index = values.len() + 1;
        let escaped = term
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        values.push(Value::Text(format!("{escaped}%")));
        scores.push(format!(
            "CASE WHEN name = ?{exact_index} COLLATE NOCASE THEN 0
                  WHEN name LIKE ?{prefix_index} ESCAPE '\\' COLLATE NOCASE THEN 1
                  ELSE 2 END"
        ));
    }

    if scores.len() == 1 {
        scores.remove(0)
    } else {
        format!("MIN({})", scores.join(", "))
    }
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
        SearchExpression::PathContains(term) => {
            let escaped = term.trim()
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            values.push(Value::Text(format!("%{escaped}%")));
            format!("path LIKE ?{} ESCAPE '\\' COLLATE NOCASE", values.len())
        }
        SearchExpression::ContentContains(term) => {
            let term = term.trim();
            if term.chars().count() >= 3 {
                let phrase = format!("\"{}\"", term.replace('"', "\"\""));
                values.push(Value::Text(phrase));
                format!("id IN (SELECT rowid FROM files_fts WHERE files_fts MATCH ?{})", values.len())
            } else {
                let escaped = term
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_");
                values.push(Value::Text(format!("%{escaped}%")));
                format!("content LIKE ?{} ESCAPE '\\' COLLATE NOCASE", values.len())
            }
        }
        SearchExpression::ExtensionEquals(extension) => {
            values.push(Value::Text(extension.trim().trim_start_matches('.').to_lowercase()));
            format!("extension = ?{} COLLATE NOCASE", values.len())
        }
        SearchExpression::FileType(file_type) => {
            let extensions: &[&str] = match file_type.trim().to_ascii_lowercase().as_str() {
                "video" => &["3gp", "avi", "flv", "m2ts", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "mts", "ogv", "ts", "vob", "webm", "wmv"],
                "audio" => &["aac", "aif", "aiff", "alac", "flac", "m4a", "mp3", "ogg", "opus", "wav", "wma"],
                "image" => &["avif", "bmp", "gif", "heic", "heif", "jpeg", "jpg", "png", "svg", "tif", "tiff", "webp"],
                "document" => &["csv", "doc", "docx", "md", "odt", "pdf", "ppt", "pptx", "rtf", "txt", "xls", "xlsx"],
                "archive" => &["7z", "bz2", "gz", "rar", "tar", "xz", "zip"],
                _ => return "0 = 1".to_string(),
            };
            let placeholders: Vec<String> = extensions.iter().map(|extension| {
                values.push(Value::Text((*extension).to_string()));
                format!("?{}", values.len())
            }).collect();
            format!("extension IN ({}) COLLATE NOCASE", placeholders.join(", "))
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
                status TEXT NOT NULL,
                content TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_files_name
            ON files(name);

            CREATE INDEX IF NOT EXISTS idx_files_extension
            ON files(extension);

            CREATE INDEX IF NOT EXISTS idx_files_size
            ON files(size_bytes);

            CREATE TABLE IF NOT EXISTS scan_sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT
            );

            CREATE TABLE IF NOT EXISTS scan_seen_paths (
                session_id INTEGER NOT NULL REFERENCES scan_sessions(id) ON DELETE CASCADE,
                path TEXT NOT NULL,
                PRIMARY KEY(session_id, path)
            );

            CREATE TABLE IF NOT EXISTS index_roots (
                path TEXT PRIMARY KEY
            );

            ",
        )?;

        let has_content_column: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('files') WHERE name = 'content')",
            [],
            |row| row.get(0),
        )?;
        if !has_content_column {
            self.connection.execute("ALTER TABLE files ADD COLUMN content TEXT", [])?;
        }

        self.connection.execute_batch(
            "
            CREATE VIRTUAL TABLE IF NOT EXISTS files_fts
            USING fts5(content, content='files', content_rowid='id', tokenize='trigram');

            CREATE TRIGGER IF NOT EXISTS files_fts_insert AFTER INSERT ON files BEGIN
                INSERT INTO files_fts(rowid, content) VALUES (new.id, new.content);
            END;

            CREATE TRIGGER IF NOT EXISTS files_fts_delete AFTER DELETE ON files BEGIN
                INSERT INTO files_fts(files_fts, rowid, content)
                VALUES ('delete', old.id, old.content);
            END;

            CREATE TRIGGER IF NOT EXISTS files_fts_update AFTER UPDATE OF content ON files BEGIN
                INSERT INTO files_fts(files_fts, rowid, content)
                VALUES ('delete', old.id, old.content);
                INSERT INTO files_fts(rowid, content) VALUES (new.id, new.content);
            END;
            "
        )?;

        let schema_version: i64 = self.connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if schema_version < 1 {
            self.connection.execute("INSERT INTO files_fts(files_fts) VALUES ('rebuild')", [])?;
            self.connection.pragma_update(None, "user_version", 1)?;
        }

        Ok(())
    }

    pub fn upsert_file(&self, file: &FileRecord) -> Result<()> {
        self.upsert_file_with_content(file, None)
    }

    pub fn upsert_file_with_content(
        &self,
        file: &FileRecord,
        content: Option<&str>,
    ) -> Result<()> {
        self.upsert_file_internal(file, content, true)
    }

    /// Replace stored content even when extraction returns None.
    /// This prevents stale searchable text after a file changes format.
    pub fn replace_file_with_content(
        &self,
        file: &FileRecord,
        content: Option<&str>,
    ) -> Result<()> {
        self.upsert_file_internal(file, content, false)
    }

    fn upsert_file_internal(
        &self,
        file: &FileRecord,
        content: Option<&str>,
        preserve_missing_content: bool,
    ) -> Result<()> {
        let path = file.path.to_string_lossy().to_string();
        let size = file.size_bytes.min(i64::MAX as u64) as i64;
        let modified = file.modified_timestamp_ns.map(|value| value.min(i64::MAX as u64) as i64);
        let status = match &file.status {
            IndexingStatus::Pending => "pending",
            IndexingStatus::Indexing => "indexing",
            IndexingStatus::Indexed => "indexed",
            IndexingStatus::Skipped => "skipped",
            IndexingStatus::Failed => "failed",
        };
        let content_update = if preserve_missing_content {
            "COALESCE(excluded.content, files.content)"
        } else {
            "excluded.content"
        };
        let sql = format!(
            "INSERT INTO files
                (path, name, extension, size_bytes, modified_unix, status, content)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(path) DO UPDATE SET
                name = excluded.name,
                extension = excluded.extension,
                size_bytes = excluded.size_bytes,
                modified_unix = excluded.modified_unix,
                status = excluded.status,
                content = {content_update}"
        );

        self.connection.execute(
            &sql,
            params![path, file.name, file.extension, size, modified, status, content],
        )?;
        Ok(())
    }

    /// Persist a group of extracted records in one SQLite transaction.
    pub fn replace_files_with_content_batch(
        &self,
        files: &[(FileRecord, Option<String>)],
    ) -> Result<()> {
        if files.is_empty() {
            return Ok(());
        }

        let transaction = self.connection.unchecked_transaction()?;
        {
            let mut statement = transaction.prepare(
                "INSERT INTO files
                    (path, name, extension, size_bytes, modified_unix, status, content)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(path) DO UPDATE SET
                    name = excluded.name,
                    extension = excluded.extension,
                    size_bytes = excluded.size_bytes,
                    modified_unix = excluded.modified_unix,
                    status = excluded.status,
                    content = excluded.content"
            )?;

            for (file, content) in files {
                let path = file.path.to_string_lossy().to_string();
                let size = file.size_bytes.min(i64::MAX as u64) as i64;
                let modified = file.modified_timestamp_ns.map(|value| value.min(i64::MAX as u64) as i64);
                let status = match &file.status {
                    IndexingStatus::Pending => "pending",
                    IndexingStatus::Indexing => "indexing",
                    IndexingStatus::Indexed => "indexed",
                    IndexingStatus::Skipped => "skipped",
                    IndexingStatus::Failed => "failed",
                };
                statement.execute(params![
                    path, file.name, file.extension, size, modified, status, content
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Returns true when size and modification time match the stored record.
    pub fn is_file_unchanged(&self, file: &FileRecord) -> Result<bool> {
        let path = file.path.to_string_lossy().to_string();
        let size = file.size_bytes.min(i64::MAX as u64) as i64;
        let modified = file.modified_timestamp_ns.map(|value| value.min(i64::MAX as u64) as i64);
        let unchanged: Option<bool> = self.connection.query_row(
            "SELECT size_bytes = ?2 AND modified_unix IS ?3
             FROM files WHERE path = ?1",
            params![path, size, modified],
            |row| row.get(0),
        ).optional()?;
        Ok(unchanged.unwrap_or(false))
    }

    /// Start a scan whose discovered paths are recorded incrementally.
    pub fn begin_scan(&self) -> Result<i64> {
        self.connection.execute("INSERT INTO scan_sessions DEFAULT VALUES", [])?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Mark a bounded batch of discovered paths as present in this scan.
    pub fn mark_scan_seen(&self, scan_id: i64, files: &[FileRecord]) -> Result<()> {
        if files.is_empty() {
            return Ok(());
        }
        let transaction = self.connection.unchecked_transaction()?;
        {
            let mut statement = transaction.prepare(
                "INSERT OR IGNORE INTO scan_seen_paths(session_id, path) VALUES (?1, ?2)"
            )?;
            for file in files {
                statement.execute(params![scan_id, file.path.to_string_lossy().as_ref()])?;
            }
        }
        transaction.commit()
    }

    /// Remove records missing from completed roots, then discard scan markers.
    pub fn finish_scan(
        &self,
        scan_id: i64,
        roots: &[(std::path::PathBuf, bool)],
    ) -> Result<u64> {
        let transaction = self.connection.unchecked_transaction()?;
        let mut deleted = 0_u64;

        for (root, is_directory) in roots {
            let root_text = root.to_string_lossy().into_owned();
            let prefix = if *is_directory {
                format!("{}{}", root_text.trim_end_matches(std::path::MAIN_SEPARATOR), std::path::MAIN_SEPARATOR)
            } else {
                root_text.clone()
            };

            let affected = if *is_directory {
                transaction.execute(
                    "DELETE FROM files
                     WHERE substr(path, 1, length(?1)) = ?1
                       AND NOT EXISTS (
                           SELECT 1 FROM scan_seen_paths seen
                           WHERE seen.session_id = ?2 AND seen.path = files.path
                       )",
                    params![prefix, scan_id],
                )?
            } else {
                transaction.execute(
                    "DELETE FROM files
                     WHERE path = ?1
                       AND NOT EXISTS (
                           SELECT 1 FROM scan_seen_paths seen
                           WHERE seen.session_id = ?2 AND seen.path = files.path
                       )",
                    params![root_text, scan_id],
                )?
            };
            deleted += affected as u64;
        }

        transaction.execute("DELETE FROM scan_sessions WHERE id = ?1", [scan_id])?;
        transaction.commit()?;
        Ok(deleted)
    }

    /// Discard markers after an incomplete or failed scan without pruning files.
    pub fn discard_scan(&self, scan_id: i64) -> Result<()> {
        self.connection.execute("DELETE FROM scan_sessions WHERE id = ?1", [scan_id])?;
        Ok(())
    }

    /// Remove stale records only inside roots whose scans completed successfully.
    pub fn prune_missing_files(
        &self,
        roots: &[(std::path::PathBuf, bool)],
        present_paths: &[std::path::PathBuf],
    ) -> Result<u64> {
        if roots.is_empty() {
            return Ok(0);
        }

        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS deep_search_seen_paths (
                path TEXT PRIMARY KEY
             );
             DELETE FROM deep_search_seen_paths;"
        )?;

        {
            let mut insert = transaction.prepare(
                "INSERT OR IGNORE INTO deep_search_seen_paths(path) VALUES (?1)"
            )?;
            for path in present_paths {
                insert.execute([path.to_string_lossy().as_ref()])?;
            }
        }

        let mut deleted = 0_u64;
        for (root, is_directory) in roots {
            let root_text = root.to_string_lossy().into_owned();
            let prefix = if *is_directory {
                format!("{}{}", root_text.trim_end_matches(std::path::MAIN_SEPARATOR), std::path::MAIN_SEPARATOR)
            } else {
                root_text.clone()
            };

            let affected = if *is_directory {
                transaction.execute(
                    "DELETE FROM files
                     WHERE substr(path, 1, length(?1)) = ?1
                       AND NOT EXISTS (
                           SELECT 1 FROM deep_search_seen_paths seen
                           WHERE seen.path = files.path
                       )",
                    [&prefix],
                )?
            } else {
                transaction.execute(
                    "DELETE FROM files
                     WHERE path = ?1
                       AND NOT EXISTS (
                           SELECT 1 FROM deep_search_seen_paths seen
                           WHERE seen.path = files.path
                       )",
                    [&root_text],
                )?
            };
            deleted += affected as u64;
        }

        transaction.commit()?;
        Ok(deleted)
    }

    /// Persist indexing roots so the background watcher can restore them after restart.
    pub fn add_index_roots<P: AsRef<Path>>(&self, roots: &[P]) -> Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        {
            let mut statement = transaction.prepare(
                "INSERT OR IGNORE INTO index_roots(path) VALUES (?1)"
            )?;
            for root in roots {
                let path = root.as_ref().to_string_lossy().into_owned();
                statement.execute(params![path])?;
            }
        }
        transaction.commit()
    }

    pub fn index_roots(&self) -> Result<Vec<std::path::PathBuf>> {
        let mut statement = self.connection.prepare(
            "SELECT path FROM index_roots ORDER BY path"
        )?;
        let rows = statement.query_map([], |row| {
            let path: String = row.get(0)?;
            Ok(std::path::PathBuf::from(path))
        })?;
        rows.collect()
    }

    /// Delete a file or an entire removed directory subtree from the index.
    pub fn remove_path_tree<P: AsRef<Path>>(&self, path: P) -> Result<u64> {
        let path = path.as_ref().to_string_lossy().into_owned();
        let prefix = format!("{}{}", path.trim_end_matches(std::path::MAIN_SEPARATOR), std::path::MAIN_SEPARATOR);
        let deleted = self.connection.execute(
            "DELETE FROM files WHERE path = ?1 OR substr(path, 1, length(?2)) = ?2",
            params![path, prefix],
        )?;
        Ok(deleted as u64)
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
        let relevance = name_relevance_order(expression, &mut values);
        let sql = format!(
            "SELECT path, name, extension, size_bytes, modified_unix FROM files WHERE {condition} ORDER BY {relevance}, name COLLATE NOCASE"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            let path: String = row.get(0)?;
            let size: i64 = row.get(3)?;
            let modified: Option<i64> = row.get(4)?;
            Ok(SearchResult {
                path: path.into(),
                name: row.get(1)?,
                extension: row.get(2)?,
                size_bytes: size.max(0) as u64,
                modified_timestamp_ns: modified.map(|value| value.max(0) as u64),
            })
        })?;
        rows.collect()
    }

    /// Return a bounded page of matching files while preserving relevance order.
    /// Page sizes are clamped to 1..=1000 to protect the search service from
    /// accidentally loading an unbounded result set.
    pub fn search_page(
        &self,
        expression: &SearchExpression,
        limit: u32,
        offset: u64,
    ) -> Result<Vec<SearchResult>> {
        self.search_page_sorted(
            expression,
            limit,
            offset,
            SearchSortField::Relevance,
            SearchSortDirection::Ascending,
        )
    }

    /// Return a bounded page using a caller-selected, deterministic sort order.
    pub fn search_page_sorted(
        &self,
        expression: &SearchExpression,
        limit: u32,
        offset: u64,
        sort_field: SearchSortField,
        sort_direction: SearchSortDirection,
    ) -> Result<Vec<SearchResult>> {
        let mut values = Vec::new();
        let condition = compile_expression(expression, &mut values);
        let order_by = match sort_field {
            SearchSortField::Relevance => name_relevance_order(expression, &mut values),
            SearchSortField::Name => "name COLLATE NOCASE".to_string(),
            SearchSortField::Path => "path COLLATE NOCASE".to_string(),
            SearchSortField::Extension => "extension COLLATE NOCASE".to_string(),
            SearchSortField::Size => "size_bytes".to_string(),
            SearchSortField::Modified => "modified_unix".to_string(),
        };
        let direction = match sort_direction {
            SearchSortDirection::Ascending => "ASC",
            SearchSortDirection::Descending => "DESC",
        };
        let order_by = if sort_field == SearchSortField::Relevance {
            format!("{order_by}, name COLLATE NOCASE, path COLLATE NOCASE")
        } else {
            format!("{order_by} {direction}, name COLLATE NOCASE, path COLLATE NOCASE")
        };
        values.push(Value::Integer(limit.clamp(1, 1000) as i64));
        values.push(Value::Integer(offset.min(i64::MAX as u64) as i64));
        let sql = format!(
            "SELECT path, name, extension, size_bytes, modified_unix FROM files
             WHERE {condition}
             ORDER BY {order_by}
             LIMIT ?{} OFFSET ?{}",
            values.len() - 1,
            values.len()
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            let path: String = row.get(0)?;
            let size: i64 = row.get(3)?;
            let modified: Option<i64> = row.get(4)?;
            Ok(SearchResult {
                path: path.into(),
                name: row.get(1)?,
                extension: row.get(2)?,
                size_bytes: size.max(0) as u64,
                modified_timestamp_ns: modified.map(|value| value.max(0) as u64),
            })
        })?;
        rows.collect()
    }

    /// Count all matches without loading their file records into memory.
    pub fn count_search_results(&self, expression: &SearchExpression) -> Result<u64> {
        let mut values = Vec::new();
        let condition = compile_expression(expression, &mut values);
        let count: i64 = self.connection.query_row(
            &format!("SELECT COUNT(*) FROM files WHERE {condition}"),
            params_from_iter(values.iter()),
            |row| row.get(0),
        )?;
        Ok(count.max(0) as u64)
    }

    pub fn search_content(&self, term: &str) -> Result<Vec<SearchResult>> {
        let term = term.trim();
        if term.is_empty() {
            return Ok(Vec::new());
        }

        if term.chars().count() >= 3 {
            let phrase = format!("\"{}\"", term.replace('"', "\"\""));
            let mut statement = self.connection.prepare(
                "SELECT path, name, extension, size_bytes, modified_unix FROM files
                 WHERE id IN (SELECT rowid FROM files_fts WHERE files_fts MATCH ?1)
                 ORDER BY name COLLATE NOCASE"
            )?;
            let rows = statement.query_map([phrase], |row| {
                let path: String = row.get(0)?;
                let size: i64 = row.get(3)?;
            let modified: Option<i64> = row.get(4)?;
                Ok(SearchResult {
                    path: path.into(),
                    name: row.get(1)?,
                    extension: row.get(2)?,
                    size_bytes: size.max(0) as u64,
                    modified_timestamp_ns: modified.map(|value| value.max(0) as u64),
                })
            })?;
            rows.collect()
        } else {
            let escaped = term
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let pattern = format!("%{escaped}%");
            let mut statement = self.connection.prepare(
                "SELECT path, name, extension, size_bytes, modified_unix FROM files
                 WHERE content LIKE ?1 ESCAPE '\\' COLLATE NOCASE
                 ORDER BY name COLLATE NOCASE"
            )?;
            let rows = statement.query_map([pattern], |row| {
                let path: String = row.get(0)?;
                let size: i64 = row.get(3)?;
            let modified: Option<i64> = row.get(4)?;
                Ok(SearchResult {
                    path: path.into(),
                    name: row.get(1)?,
                    extension: row.get(2)?,
                    size_bytes: size.max(0) as u64,
                    modified_timestamp_ns: modified.map(|value| value.max(0) as u64),
                })
            })?;
            rows.collect()
        }
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
        let prefix = format!("{}%", escaped);

        let mut statement = self.connection.prepare(
            "
            SELECT path, name, extension, size_bytes, modified_unix
            FROM files
            WHERE name LIKE ?1 ESCAPE '\\' COLLATE NOCASE
            ORDER BY
                CASE
                    WHEN name = ?2 COLLATE NOCASE THEN 0
                    WHEN name LIKE ?3 ESCAPE '\\' COLLATE NOCASE THEN 1
                    ELSE 2
                END,
                name COLLATE NOCASE
            ",
        )?;

        let rows = statement.query_map(params![pattern, term, prefix], |row| {
            let path: String = row.get(0)?;
            let size: i64 = row.get(3)?;
            let modified: Option<i64> = row.get(4)?;

            Ok(SearchResult {
                path: path.into(),
                name: row.get(1)?,
                extension: row.get(2)?,
                size_bytes: size.max(0) as u64,
                modified_timestamp_ns: modified.map(|value| value.max(0) as u64),
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
            SELECT path, name, extension, size_bytes, modified_unix
            FROM files
            WHERE extension = ?1 COLLATE NOCASE
            ORDER BY name COLLATE NOCASE
            ",
        )?;

        let rows = statement.query_map([extension], |row| {
            let path: String = row.get(0)?;
            let size: i64 = row.get(3)?;
            let modified: Option<i64> = row.get(4)?;

            Ok(SearchResult {
                path: path.into(),
                name: row.get(1)?,
                extension: row.get(2)?,
                size_bytes: size.max(0) as u64,
                modified_timestamp_ns: modified.map(|value| value.max(0) as u64),
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
            SELECT path, name, extension, size_bytes, modified_unix
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
            let modified: Option<i64> = row.get(4)?;

                Ok(SearchResult {
                    path: path.into(),
                    name: row.get(1)?,
                    extension: row.get(2)?,
                    size_bytes: size.max(0) as u64,
                    modified_timestamp_ns: modified.map(|value| value.max(0) as u64),
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
            modified_timestamp_ns: Some(1_700_000_000),
            status: IndexingStatus::Pending,
        }
    }

    #[test]
    fn detects_modification_timestamp_changes_within_the_same_second() {
        let db = Database::open(":memory:").unwrap();
        let original = sample_file("rapid-edit.txt", 10);
        db.upsert_file(&original).unwrap();
        assert!(db.is_file_unchanged(&original).unwrap());

        let mut changed = original.clone();
        changed.modified_timestamp_ns = Some(
            original.modified_timestamp_ns.unwrap() + 1_000_000
        );
        assert!(!db.is_file_unchanged(&changed).unwrap());
    }

    #[test]
    fn saves_and_counts_files() {
        let db = Database::open(":memory:").unwrap();

        db.upsert_file(&sample_file("video.mp4", 5000))
            .unwrap();

        assert_eq!(db.file_count().unwrap(), 1);
    }

    #[test]
    fn ranks_exact_and_prefix_name_matches_before_substrings() {
        let db = Database::open(":memory:").unwrap();
        db.upsert_file(&sample_file("annual report final.txt", 10)).unwrap();
        db.upsert_file(&sample_file("reporting-guide.txt", 10)).unwrap();
        db.upsert_file(&sample_file("report.txt", 10)).unwrap();

        let results = db.search_by_name("report").unwrap();
        assert_eq!(
            results.iter().map(|item| item.name.as_str()).collect::<Vec<_>>(),
            vec!["report.txt", "reporting-guide.txt", "annual report final.txt"]
        );
    }

    #[test]
    fn ranks_name_matches_inside_compound_search_expressions() {
        let db = Database::open(":memory:").unwrap();
        db.upsert_file(&sample_file("annual report final.txt", 10)).unwrap();
        db.upsert_file(&sample_file("reporting-guide.txt", 10)).unwrap();
        db.upsert_file(&sample_file("report.txt", 10)).unwrap();

        let results = db.search(&SearchExpression::NameContains("report".into())).unwrap();
        assert_eq!(
            results.iter().map(|item| item.name.as_str()).collect::<Vec<_>>(),
            vec!["report.txt", "reporting-guide.txt", "annual report final.txt"]
        );
    }

    #[test]
    fn paginates_search_results_and_counts_all_matches() {
        let db = Database::open(":memory:").unwrap();
        for name in ["report-a.txt", "report-b.txt", "report-c.txt", "notes.txt"] {
            db.upsert_file(&sample_file(name, 10)).unwrap();
        }
        let expression = SearchExpression::NameContains("report".into());

        assert_eq!(db.count_search_results(&expression).unwrap(), 3);
        let first_page = db.search_page(&expression, 2, 0).unwrap();
        let second_page = db.search_page(&expression, 2, 2).unwrap();

        assert_eq!(
            first_page.iter().map(|file| file.name.as_str()).collect::<Vec<_>>(),
            vec!["report-a.txt", "report-b.txt"]
        );
        assert_eq!(
            second_page.iter().map(|file| file.name.as_str()).collect::<Vec<_>>(),
            vec!["report-c.txt"]
        );
        assert!(db.search_page(&expression, 0, 0).unwrap().len() <= 1);
        assert!(db.search_page(&expression, 5000, 0).unwrap().len() <= 3);
    }

    #[test]
    fn supports_custom_sorting_for_paginated_search_results() {
        let db = Database::open(":memory:").unwrap();
        db.upsert_file(&sample_file("zeta.txt", 30)).unwrap();
        db.upsert_file(&sample_file("alpha.pdf", 10)).unwrap();
        db.upsert_file(&sample_file("beta.txt", 20)).unwrap();
        let expression = SearchExpression::NameContains(String::new());

        let by_size = db.search_page_sorted(
            &expression, 10, 0, SearchSortField::Size, SearchSortDirection::Descending,
        ).unwrap();
        assert_eq!(
            by_size.iter().map(|file| file.size_bytes).collect::<Vec<_>>(),
            vec![30, 20, 10],
        );

        let by_extension = db.search_page_sorted(
            &expression, 10, 0, SearchSortField::Extension, SearchSortDirection::Ascending,
        ).unwrap();
        assert_eq!(
            by_extension.iter().map(|file| file.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha.pdf", "beta.txt", "zeta.txt"],
        );

        let by_name = db.search_page_sorted(
            &expression, 10, 0, SearchSortField::Name, SearchSortDirection::Descending,
        ).unwrap();
        assert_eq!(
            by_name.iter().map(|file| file.name.as_str()).collect::<Vec<_>>(),
            vec!["zeta.txt", "beta.txt", "alpha.pdf"],
        );
    }

    #[test]
    fn returns_modification_time_and_sorts_by_it() {
        let db = Database::open(":memory:").unwrap();
        let mut older = sample_file("older.txt", 10);
        older.modified_timestamp_ns = Some(100);
        let mut newer = sample_file("newer.txt", 20);
        newer.modified_timestamp_ns = Some(300);
        let mut undated = sample_file("undated.txt", 30);
        undated.modified_timestamp_ns = None;
        db.upsert_file(&older).unwrap();
        db.upsert_file(&newer).unwrap();
        db.upsert_file(&undated).unwrap();

        let results = db.search_page_sorted(
            &SearchExpression::NameContains(String::new()),
            10,
            0,
            SearchSortField::Modified,
            SearchSortDirection::Descending,
        ).unwrap();

        assert_eq!(results[0].name, "newer.txt");
        assert_eq!(results[0].modified_timestamp_ns, Some(300));
        assert_eq!(results[1].name, "older.txt");
        assert_eq!(results[1].modified_timestamp_ns, Some(100));
        assert_eq!(results[2].name, "undated.txt");
        assert_eq!(results[2].modified_timestamp_ns, None);
    }

    #[test]
    fn paginated_search_uses_a_stable_path_tiebreaker_for_duplicate_names() {
        let db = Database::open(":memory:").unwrap();

        for directory in ["alpha", "bravo", "charlie"] {
            let mut file = sample_file("report.txt", 100);
            file.path = PathBuf::from(format!("/{directory}/report.txt"));
            db.upsert_file(&file).unwrap();
        }

        let expression = SearchExpression::NameContains("report".into());
        let first_page = db.search_page(&expression, 2, 0).unwrap();
        let second_page = db.search_page(&expression, 2, 2).unwrap();

        let paths: Vec<String> = first_page
            .iter()
            .chain(second_page.iter())
            .map(|file| file.path.to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            paths,
            vec![
                "/alpha/report.txt",
                "/bravo/report.txt",
                "/charlie/report.txt",
            ]
        );
        assert_eq!(paths.iter().collect::<std::collections::HashSet<_>>().len(), 3);
    }

    #[test]
    fn searches_by_path_and_treats_wildcards_as_literal_text() {
        let db = Database::open(":memory:").unwrap();
        let mut first = sample_file("report.pdf", 10);
        first.path = PathBuf::from("/archive/2025/report.pdf");
        let mut second = sample_file("report.pdf", 10);
        second.path = PathBuf::from("/current/report.pdf");
        let mut third = sample_file("notes.pdf", 10);
        third.path = PathBuf::from("/archive/2025/100%_complete/notes.pdf");
        db.upsert_file(&first).unwrap();
        db.upsert_file(&second).unwrap();
        db.upsert_file(&third).unwrap();

        let results = db.search(&SearchExpression::PathContains("/archive/2025".into())).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|item| item.path.to_string_lossy().contains("/archive/2025")));

        let literal = db.search(&SearchExpression::PathContains("100%_complete".into())).unwrap();
        assert_eq!(literal.len(), 1);
        assert_eq!(literal[0].name, "notes.pdf");
    }

    #[test]
    fn filters_files_by_common_file_type_categories() {
        let db = Database::open(":memory:").unwrap();
        for name in ["movie.mkv", "clip.mp4", "song.flac", "photo.webp", "report.pdf", "backup.zip", "unknown.raw"] {
            db.upsert_file(&sample_file(name, 10)).unwrap();
        }

        let videos = db.search(&SearchExpression::FileType("video".into())).unwrap();
        assert_eq!(videos.len(), 2);
        assert!(videos.iter().all(|file| ["mkv", "mp4"].contains(&file.extension.as_deref().unwrap())));

        assert_eq!(db.search(&SearchExpression::FileType("audio".into())).unwrap().len(), 1);
        assert_eq!(db.search(&SearchExpression::FileType("image".into())).unwrap().len(), 1);
        assert_eq!(db.search(&SearchExpression::FileType("document".into())).unwrap().len(), 1);
        assert_eq!(db.search(&SearchExpression::FileType("archive".into())).unwrap().len(), 1);
        assert!(db.search(&SearchExpression::FileType("unknown".into())).unwrap().is_empty());
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
    fn searches_inside_indexed_file_content() {
        let db = Database::open(":memory:").unwrap();
        let file = sample_file("notes.txt", 42);
        db.upsert_file_with_content(&file, Some("The quick brown fox jumps over the lazy dog")).unwrap();
        db.upsert_file(&sample_file("other.txt", 12)).unwrap();

        let results = db.search_content("BROWN FOX").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "notes.txt");
    }

    #[test]
    fn content_search_treats_wildcards_as_literal_text() {
        let db = Database::open(":memory:").unwrap();
        db.upsert_file_with_content(&sample_file("symbols.txt", 8), Some("100%_ready")).unwrap();
        db.upsert_file_with_content(&sample_file("other.txt", 8), Some("100XXready")).unwrap();

        let results = db.search_content("100%_").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "symbols.txt");
    }

    #[test]
    fn trigram_content_search_matches_substrings_case_insensitively() {
        let db = Database::open(":memory:").unwrap();
        db.upsert_file_with_content(
            &sample_file("notes.txt", 42),
            Some("The quick brown fox jumps over the lazy dog"),
        ).unwrap();

        let results = db.search_content("BROWN FOX").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "notes.txt");

        let expression = SearchExpression::ContentContains("quick brown".into());
        assert_eq!(db.search(&expression).unwrap().len(), 1);
    }

    #[test]
    fn content_search_falls_back_for_terms_shorter_than_three_characters() {
        let db = Database::open(":memory:").unwrap();
        db.upsert_file_with_content(
            &sample_file("short.txt", 12),
            Some("an ox in a box"),
        ).unwrap();

        assert_eq!(db.search_content("ox").unwrap().len(), 1);
        assert_eq!(
            db.search(&SearchExpression::ContentContains("ox".into())).unwrap().len(),
            1
        );
    }

    #[test]
    fn content_fts_stays_synchronized_after_update_and_delete() {
        let db = Database::open(":memory:").unwrap();
        let file = sample_file("mutable.txt", 10);
        db.upsert_file_with_content(&file, Some("old searchable phrase")).unwrap();

        let mut changed = file.clone();
        changed.size_bytes = 20;
        db.replace_file_with_content(&changed, Some("new searchable phrase")).unwrap();

        assert!(db.search_content("old searchable").unwrap().is_empty());
        assert_eq!(db.search_content("new searchable").unwrap().len(), 1);

        let root = file.path.clone();
        let deleted = db.prune_missing_files(&[(root, false)], &[]).unwrap();
        assert_eq!(deleted, 1);
        assert!(db.search_content("new searchable").unwrap().is_empty());
    }

    #[test]
    fn saves_multiple_files_with_content_in_one_batch() {
        let db = Database::open(":memory:").unwrap();
        let first = sample_file("first.txt", 5);
        let second = sample_file("second.txt", 6);
        let batch = vec![
            (first, Some("first searchable text".to_string())),
            (second, Some("second searchable text".to_string())),
        ];

        db.replace_files_with_content_batch(&batch).unwrap();

        assert_eq!(db.file_count().unwrap(), 2);
        assert_eq!(db.search_content("searchable").unwrap().len(), 2);
    }

    #[test]
    fn detects_unchanged_files_using_size_and_modification_time() {
        let db = Database::open(":memory:").unwrap();
        let file = sample_file("stable.txt", 128);
        db.upsert_file_with_content(&file, Some("stable searchable content")).unwrap();

        assert!(db.is_file_unchanged(&file).unwrap());

        let mut changed = file.clone();
        changed.size_bytes += 1;
        assert!(!db.is_file_unchanged(&changed).unwrap());

        let mut changed_time = file.clone();
        changed_time.modified_timestamp_ns = Some(1_700_000_001);
        assert!(!db.is_file_unchanged(&changed_time).unwrap());
    }

    #[test]
    fn replacement_clears_stale_content_when_extraction_fails() {
        let db = Database::open(":memory:").unwrap();
        let file = sample_file("changed.txt", 20);
        db.upsert_file_with_content(&file, Some("old searchable content")).unwrap();

        let mut changed = file.clone();
        changed.size_bytes = 30;
        db.replace_file_with_content(&changed, None).unwrap();

        assert!(db.search_content("old searchable").unwrap().is_empty());
    }

    #[test]
    fn prunes_missing_files_only_inside_completed_roots() {
        let db = Database::open(":memory:").unwrap();
        let root = PathBuf::from("/scan-root");
        let keep = root.join("keep.txt");
        let remove = root.join("remove.txt");
        let outside = PathBuf::from("/other/outside.txt");

        let mut keep_record = sample_file("keep.txt", 10);
        keep_record.path = keep.clone();
        db.upsert_file(&keep_record).unwrap();

        let mut remove_record = sample_file("remove.txt", 20);
        remove_record.path = remove.clone();
        db.upsert_file(&remove_record).unwrap();

        let mut outside_record = sample_file("outside.txt", 30);
        outside_record.path = outside.clone();
        db.upsert_file(&outside_record).unwrap();

        let deleted = db.prune_missing_files(
            &[(root.clone(), true)],
            std::slice::from_ref(&keep),
        ).unwrap();

        assert_eq!(deleted, 1);
        assert!(db.search_by_name("keep").unwrap().iter().any(|f| f.path == keep));
        assert!(db.search_by_name("remove").unwrap().is_empty());
        assert!(db.search_by_name("outside").unwrap().iter().any(|f| f.path == outside));
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
    #[test]
    fn persists_unique_index_roots() {
        let db = Database::open(":memory:").unwrap();
        let roots = vec![PathBuf::from("/data/videos"), PathBuf::from("/data/documents")];
        db.add_index_roots(&roots).unwrap();
        db.add_index_roots(&roots).unwrap();
        let mut expected = roots;
        expected.sort();
        assert_eq!(db.index_roots().unwrap(), expected);
    }

    #[test]
    fn removes_a_deleted_directory_subtree_without_touching_siblings() {
        let db = Database::open(":memory:").unwrap();
        let mut first = sample_file("first.txt", 1);
        first.path = PathBuf::from("/data/folder/first.txt");
        let mut nested = sample_file("nested.txt", 2);
        nested.path = PathBuf::from("/data/folder/nested/nested.txt");
        let mut sibling = sample_file("sibling.txt", 3);
        sibling.path = PathBuf::from("/data/other/sibling.txt");
        db.upsert_file(&first).unwrap();
        db.upsert_file(&nested).unwrap();
        db.upsert_file(&sibling).unwrap();

        assert_eq!(db.remove_path_tree("/data/folder").unwrap(), 2);
        assert!(db.search_by_name("first").unwrap().is_empty());
        assert!(db.search_by_name("nested").unwrap().is_empty());
        assert_eq!(db.search_by_name("sibling").unwrap().len(), 1);
    }

}
