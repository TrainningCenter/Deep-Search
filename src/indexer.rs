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

    report
}
