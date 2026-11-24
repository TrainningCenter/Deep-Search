#[derive(Debug, Clone)]
pub struct SearchResult {
    pub path: std::path::PathBuf,
    pub name: String,
    pub extension: Option<String>,
    pub size_bytes: u64,
}

impl Database {
    pub fn search_by_name(&self, term: &str) -> Result<Vec<SearchResult>> {
        let pattern = format!("%{}%", term);

        let mut statement = self.connection.prepare(
            "
            SELECT path, name, extension, size_bytes
            FROM files
            WHERE name LIKE ?1 COLLATE NOCASE
            ORDER BY name COLLATE NOCASE
            "
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
}
