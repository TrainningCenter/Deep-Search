pub mod indexer;

pub const APP_NAME: &str = "Deep-Search";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexingStatus {
    Pending,
    Indexing,
    Indexed,
    Skipped,
    Failed,
}

#[derive(Debug, Clone)]
pub struct FileRecord {
    pub path: std::path::PathBuf,
    pub name: String,
    pub extension: Option<String>,
    pub size_bytes: u64,
    pub modified_unix: Option<u64>,
    pub status: IndexingStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_identity_is_available() {
        assert_eq!(APP_NAME, "Deep-Search");
        assert_eq!(VERSION, "0.1.0");
    }

    #[test]
    fn new_file_can_be_represented() {
        let file = FileRecord {
            path: "documents/report.pdf".into(),
            name: "report.pdf".into(),
            extension: Some("pdf".into()),
            size_bytes: 1024,
            modified_unix: None,
            status: IndexingStatus::Pending,
        };

        assert_eq!(file.size_bytes, 1024);
        assert_eq!(file.status, IndexingStatus::Pending);
    }
}
