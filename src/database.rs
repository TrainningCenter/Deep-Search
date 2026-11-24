
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
    fn searches_files_by_name() {
        let db = Database::open(":memory:").unwrap();

        db.upsert_file(&sample_file("report.pdf", 1000))
            .unwrap();

        let results = db.search_by_name("report").unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "report.pdf");
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
