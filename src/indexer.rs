use crate::{FileRecord, IndexingStatus};
use std::fs::{self, ReadDir};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const DEFAULT_SCAN_BATCH_SIZE: usize = 128;

#[derive(Debug, Default)]
pub struct ScanSummary {
    pub files_discovered: u64,
    pub directories_scanned: u64,
    pub errors: u64,
    pub completed_roots: Vec<(PathBuf, bool)>,
}

#[derive(Debug, Default)]
pub struct ScanReport {
    pub files: Vec<FileRecord>,
    pub directories_scanned: u64,
    pub errors: u64,
    pub completed_roots: Vec<(PathBuf, bool)>,
}

/// Traverse roots depth-first, retaining directory iterators rather than every
/// discovered file path. File metadata is delivered in bounded batches.
pub fn scan_paths_batched<P, F>(
    paths: &[P],
    batch_size: usize,
    mut on_batch: F,
) -> ScanSummary
where
    P: AsRef<Path>,
    F: FnMut(&[FileRecord]) -> bool,
{
    let mut summary = ScanSummary::default();
    let batch_size = batch_size.max(1);

    for input in paths {
        let root = input.as_ref().to_path_buf();
        let errors_before = summary.errors;
        let mut callback_failed = false;

        let root_metadata = match fs::symlink_metadata(&root) {
            Ok(metadata) => metadata,
            Err(_) => {
                summary.errors += 1;
                continue;
            }
        };
        if root_metadata.file_type().is_symlink() {
            continue;
        }

        let root_is_directory = root_metadata.is_dir();
        let mut batch = Vec::with_capacity(batch_size);
        let mut directories: Vec<(PathBuf, ReadDir)> = Vec::new();

        if root_is_directory {
            match fs::read_dir(&root) {
                Ok(entries) => {
                    summary.directories_scanned += 1;
                    directories.push((root.clone(), entries));
                }
                Err(_) => summary.errors += 1,
            }
        } else if root_metadata.is_file() {
            if let Some(record) = file_record(root.clone(), &root_metadata) {
                summary.files_discovered += 1;
                batch.push(record);
            }
        }

        while !directories.is_empty() {
            let next_entry = directories.last_mut().and_then(|(_, entries)| entries.next());
            match next_entry {
                Some(Ok(entry)) => {
                    let path = entry.path();
                    let metadata = match fs::symlink_metadata(&path) {
                        Ok(metadata) => metadata,
                        Err(_) => {
                            summary.errors += 1;
                            continue;
                        }
                    };
                    if metadata.file_type().is_symlink() {
                        continue;
                    }
                    if metadata.is_dir() {
                        match fs::read_dir(&path) {
                            Ok(children) => {
                                summary.directories_scanned += 1;
                                directories.push((path, children));
                            }
                            Err(_) => summary.errors += 1,
                        }
                    } else if metadata.is_file() {
                        if let Some(record) = file_record(path, &metadata) {
                            summary.files_discovered += 1;
                            batch.push(record);
                            if batch.len() >= batch_size {
                                if !on_batch(&batch) {
                                    summary.errors += 1;
                                    callback_failed = true;
                                }
                                batch.clear();
                            }
                        }
                    }
                }
                Some(Err(_)) => summary.errors += 1,
                None => {
                    directories.pop();
                }
            }
        }

        if !batch.is_empty() && !on_batch(&batch) {
            summary.errors += 1;
            callback_failed = true;
        }

        if summary.errors == errors_before && !callback_failed {
            summary.completed_roots.push((root, root_is_directory));
        }
    }

    summary
}

fn file_record(path: PathBuf, metadata: &fs::Metadata) -> Option<FileRecord> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let extension = path.extension().map(|ext| ext.to_string_lossy().to_lowercase());
    let modified_unix = metadata.modified().ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());

    Some(FileRecord {
        path,
        name,
        extension,
        size_bytes: metadata.len(),
        modified_unix,
        status: IndexingStatus::Pending,
    })
}

/// Compatibility helper used by tests and callers that explicitly need a
/// complete in-memory report. The indexing engine uses the streaming API.
pub fn scan_paths<P: AsRef<Path>>(paths: &[P]) -> ScanReport {
    let mut report = ScanReport::default();
    let summary = scan_paths_batched(paths, DEFAULT_SCAN_BATCH_SIZE, |batch| {
        report.files.extend_from_slice(batch);
        true
    });
    report.files.sort_by(|left, right| left.path.cmp(&right.path));
    report.directories_scanned = summary.directories_scanned;
    report.errors = summary.errors;
    report.completed_roots = summary.completed_roots;
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
        assert_eq!(report.completed_roots, vec![(root.clone(), true)]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn streams_files_in_batches_without_exceeding_requested_size() {
        let root = create_temp_dir();
        for index in 0..17 {
            fs::write(root.join(format!("file-{index:02}.txt")), b"x").unwrap();
        }

        let mut batch_sizes = Vec::new();
        let report = scan_paths_batched(&[root.as_path()], 4, |batch| {
            batch_sizes.push(batch.len());
            true
        });

        assert_eq!(report.files_discovered, 17);
        assert_eq!(report.errors, 0);
        assert_eq!(batch_sizes.iter().sum::<usize>(), 17);
        assert!(batch_sizes.iter().all(|size| *size <= 4));
        assert_eq!(batch_sizes, vec![4, 4, 4, 4, 1]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_missing_paths_as_errors_and_never_marks_them_complete() {
        let root = create_temp_dir();
        let missing = root.join("does-not-exist");
        let report = scan_paths(&[missing.as_path()]);
        assert!(report.files.is_empty());
        assert_eq!(report.errors, 1);
        assert!(report.completed_roots.is_empty());
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
        assert_eq!(report.completed_roots, vec![(file_path.clone(), false)]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sorts_results_across_multiple_input_paths() {
        let root = create_temp_dir();
        let first = root.join("zeta.txt");
        let second = root.join("alpha.txt");
        fs::write(&first, b"z").unwrap();
        fs::write(&second, b"a").unwrap();
        let report = scan_paths(&[first.as_path(), second.as_path()]);
        assert_eq!(report.errors, 0);
        assert_eq!(report.files.len(), 2);
        assert_eq!(report.files[0].name, "alpha.txt");
        assert_eq!(report.files[1].name, "zeta.txt");
        assert_eq!(report.completed_roots.len(), 2);
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
        assert_eq!(report.completed_roots.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
