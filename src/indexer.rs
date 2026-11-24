use crate::{FileRecord, IndexingStatus};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Default)]
pub struct ScanReport {
    pub files: Vec<FileRecord>,
    pub directories_scanned: u64,
    pub errors: u64,
}

pub fn scan_paths<P: AsRef<Path>>(paths: &[P]) -> ScanReport {
    let mut report = ScanReport::default();
    let mut pending: Vec<PathBuf> = paths
        .iter()
        .map(|path| path.as_ref().to_path_buf())
        .collect();

    while let Some(path) = pending.pop() {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                report.errors += 1;
                continue;
            }
        };

        if metadata.file_type().is_symlink() {
            continue;
        }

        if metadata.is_dir() {
            report.directories_scanned += 1;

            let entries = match fs::read_dir(&path) {
                Ok(entries) => entries,
                Err(_) => {
                    report.errors += 1;
                    continue;
                }
            };

            for entry in entries {
                match entry {
                    Ok(entry) => pending.push(entry.path()),
                    Err(_) => report.errors += 1,
                }
            }

            continue;
        }

        if metadata.is_file() {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();

            let extension = path
                .extension()
                .map(|ext| ext.to_string_lossy().to_lowercase());

            let modified_unix = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs());

            report.files.push(FileRecord {
                path,
                name,
                extension,
                size_bytes: metadata.len(),
                modified_unix,
                status: IndexingStatus::Pending,
            });
        }
    }

    report.files.sort_by(|left, right| left.path.cmp(&right.path));
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn create_temp_dir() -> PathBuf {
        let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "deep-search-indexer-{}-{id}",
            std::process::id()
        ));

        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn recursively_discovers_files_and_metadata() {
        let root = create_temp_dir();
        let nested = root.join("nested").join("deeper");
        fs::create_dir_all(&nested).unwrap();

        fs::write(root.join("alpha.TXT"), b"hello").unwrap();
        fs::write(nested.join("video.MKV"), b"video-data").unwrap();

        let report = scan_paths(&[root.as_path()]);

        assert_eq!(report.errors, 0);
        assert_eq!(report.directories_scanned, 3);
        assert_eq!(report.files.len(), 2);
        assert_eq!(report.files[0].name, "alpha.TXT");
        assert_eq!(report.files[0].extension.as_deref(), Some("txt"));
        assert_eq!(report.files[0].size_bytes, 5);
        assert_eq!(report.files[0].status, IndexingStatus::Pending);
        assert_eq!(report.files[1].name, "video.MKV");
        assert_eq!(report.files[1].extension.as_deref(), Some("mkv"));
        assert_eq!(report.files[1].size_bytes, 10);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_missing_paths_as_errors() {
        let root = create_temp_dir();
        let missing = root.join("does-not-exist");

        let report = scan_paths(&[missing.as_path()]);

        assert!(report.files.is_empty());
        assert_eq!(report.errors, 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn can_scan_a_single_file_path() {
        let root = create_temp_dir();
        let file_path = root.join("single.pdf");
        fs::write(&file_path, b"pdf-data").unwrap();

        let report = scan_paths(&[file_path.as_path()]);

        assert_eq!(report.errors, 0);
        assert_eq!(report.directories_scanned, 0);
        assert_eq!(report.files.len(), 1);
        assert_eq!(report.files[0].name, "single.pdf");
        assert_eq!(report.files[0].extension.as_deref(), Some("pdf"));
        assert_eq!(report.files[0].size_bytes, 8);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sorts_results_across_multiple_input_paths() {
        let root = create_temp_dir();
        let first = root.join("zeta.txt");
        let second = root.join("alpha.txt");
        fs::write(&first, b"z").unwrap();
        fs::write(&second, b"a").unwrap();

        // Deliberately provide paths in reverse lexical order.
        let report = scan_paths(&[first.as_path(), second.as_path()]);

        assert_eq!(report.errors, 0);
        assert_eq!(report.files.len(), 2);
        assert_eq!(report.files[0].name, "alpha.txt");
        assert_eq!(report.files[1].name, "zeta.txt");

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symbolic_links() {
        use std::os::unix::fs::symlink;

        let root = create_temp_dir();
        let target_dir = root.join("target");
        let scan_dir = root.join("scan");
        fs::create_dir_all(&target_dir).unwrap();
        fs::create_dir_all(&scan_dir).unwrap();

        fs::write(target_dir.join("target.txt"), b"target").unwrap();
        fs::write(root.join("target-file.txt"), b"file").unwrap();
        symlink(&target_dir, scan_dir.join("linked-dir")).unwrap();
        symlink(root.join("target-file.txt"), scan_dir.join("linked-file.txt")).unwrap();

        let report = scan_paths(&[scan_dir.as_path()]);

        assert_eq!(report.errors, 0);
        assert_eq!(report.directories_scanned, 1);
        assert!(report.files.is_empty());

        fs::remove_dir_all(root).unwrap();
    }
}
