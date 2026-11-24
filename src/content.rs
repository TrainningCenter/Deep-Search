use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

const MAX_PLAIN_TEXT_SIZE: u64 = 1024 * 1024;
const MAX_PDF_SIZE: u64 = 20 * 1024 * 1024;
const MAX_ARCHIVE_SIZE: u64 = 50 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 1_000;
const MAX_ARCHIVE_UNCOMPRESSED_SIZE: u64 = 32 * 1024 * 1024;
const MAX_XML_SIZE: u64 = 16 * 1024 * 1024;
const MAX_EXTRACTED_TEXT_SIZE: usize = 5 * 1024 * 1024;

fn limit_text(mut text: String) -> Option<String> {
    if text.len() > MAX_EXTRACTED_TEXT_SIZE {
        let mut end = MAX_EXTRACTED_TEXT_SIZE;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }

    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn extract_xml_text(xml: &str) -> Option<String> {
    let document = roxmltree::Document::parse(xml).ok()?;
    let mut output = String::new();

    for node in document.descendants().filter(|node| node.is_text()) {
        let value = node.text().unwrap_or_default().trim();
        if value.is_empty() {
            continue;
        }
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(value);
        if output.len() >= MAX_EXTRACTED_TEXT_SIZE {
            break;
        }
    }

    limit_text(output)
}

fn read_archive_xml(path: &Path, entry_name: &str) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;

    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return None;
    }

    let mut total_uncompressed = 0_u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).ok()?;
        total_uncompressed = total_uncompressed.checked_add(entry.size())?;
        if total_uncompressed > MAX_ARCHIVE_UNCOMPRESSED_SIZE {
            return None;
        }
    }

    let entry = archive.by_name(entry_name).ok()?;
    if entry.size() > MAX_XML_SIZE {
        return None;
    }

    let mut xml = String::with_capacity(entry.size() as usize);
    entry
        .take(MAX_XML_SIZE + 1)
        .read_to_string(&mut xml)
        .ok()?;
    if xml.len() as u64 > MAX_XML_SIZE {
        return None;
    }

    Some(xml)
}

fn extract_office_text(path: &Path, entry_name: &str) -> Option<String> {
    let xml = read_archive_xml(path, entry_name)?;
    extract_xml_text(&xml)
}

/// Extract text from multiple XML parts in presentation and spreadsheet archives.
/// The archive is validated as a whole before any XML part is read.
fn extract_archive_xml_parts<F>(path: &Path, mut matches: F) -> Option<String>
where
    F: FnMut(&str) -> bool,
{
    let file = File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;

    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return None;
    }

    let mut total_uncompressed = 0_u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).ok()?;
        total_uncompressed = total_uncompressed.checked_add(entry.size())?;
        if total_uncompressed > MAX_ARCHIVE_UNCOMPRESSED_SIZE {
            return None;
        }
    }

    let mut output = String::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).ok()?;
        let name = entry.name().to_string();
        if !matches(&name) || entry.size() > MAX_XML_SIZE {
            continue;
        }

        let mut xml = String::with_capacity(entry.size() as usize);
        entry
            .by_ref()
            .take(MAX_XML_SIZE + 1)
            .read_to_string(&mut xml)
            .ok()?;
        if xml.len() as u64 > MAX_XML_SIZE {
            return None;
        }

        if let Some(part_text) = extract_xml_text(&xml) {
            if !output.is_empty() {
                output.push(' ');
            }
            output.push_str(&part_text);
            if output.len() >= MAX_EXTRACTED_TEXT_SIZE {
                break;
            }
        }
    }

    limit_text(output)
}

fn extract_powerpoint_text(path: &Path) -> Option<String> {
    extract_archive_xml_parts(path, |name| {
        name.starts_with("ppt/slides/slide") && name.ends_with(".xml")
    })
}

fn extract_spreadsheet_text(path: &Path) -> Option<String> {
    extract_archive_xml_parts(path, |name| {
        name == "xl/sharedStrings.xml"
            || (name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml"))
    })
}

/// Extract searchable text from supported document formats.
/// Plain text is limited to 1 MiB; PDF and office archives have stricter
/// input, archive-entry, decompressed-size, and output limits.
pub fn extract_text(path: &Path, size_bytes: u64) -> Option<String> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();

    match extension.as_str() {
        "txt" | "md" | "markdown" | "log" | "csv" | "tsv" |
        "json" | "jsonl" | "xml" | "html" | "htm" | "css" |
        "js" | "jsx" | "ts" | "tsx" | "rs" | "toml" |
        "yaml" | "yml" | "ini" | "conf" | "cfg" | "sql" |
        "py" | "sh" | "bash" | "c" | "h" | "cpp" | "hpp" |
        "java" | "go" | "php" | "rb" | "swift" | "kt" |
        "env" | "rst" | "tex" => {
            if size_bytes > MAX_PLAIN_TEXT_SIZE {
                return None;
            }
            let content = fs::read_to_string(path).ok()?;
            if content.contains('\0') {
                return None;
            }
            Some(content)
        }
        "pdf" => {
            if size_bytes > MAX_PDF_SIZE {
                return None;
            }
            let text = pdf_extract::extract_text(path).ok()?;
            limit_text(text)
        }
        "docx" => {
            if size_bytes > MAX_ARCHIVE_SIZE {
                return None;
            }
            extract_office_text(path, "word/document.xml")
        }
        "odt" => {
            if size_bytes > MAX_ARCHIVE_SIZE {
                return None;
            }
            extract_office_text(path, "content.xml")
        }
        "pptx" => {
            if size_bytes > MAX_ARCHIVE_SIZE {
                return None;
            }
            extract_powerpoint_text(path)
        }
        "xlsx" => {
            if size_bytes > MAX_ARCHIVE_SIZE {
                return None;
            }
            extract_spreadsheet_text(path)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_text_from_xml_document_nodes() {
        let xml = r#"<document><p>Project roadmap</p><p>Second section</p></document>"#;
        let text = extract_xml_text(xml).unwrap();
        assert!(text.contains("Project roadmap"));
        assert!(text.contains("Second section"));
    }

    #[test]
    fn reads_docx_and_odt_xml_from_archives() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "deep-search-office-{}-{unique}",
            std::process::id()
        ));

        for (extension, entry_name) in [
            ("docx", "word/document.xml"),
            ("odt", "content.xml"),
        ] {
            let path = base.with_extension(extension);
            let file = File::create(&path).unwrap();
            let mut archive = zip::ZipWriter::new(file);
            archive
                .start_file(entry_name, SimpleFileOptions::default())
                .unwrap();
            archive
                .write_all(b"<document><p>Indexed office document</p></document>")
                .unwrap();
            archive.finish().unwrap();

            let text = extract_office_text(&path, entry_name).unwrap();
            assert!(text.contains("Indexed office document"));
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn rejects_invalid_xml() {
        assert!(extract_xml_text("<document>").is_none());
    }

    #[test]
    fn limits_extracted_text_without_splitting_utf8() {
        let text = "أ".repeat(MAX_EXTRACTED_TEXT_SIZE / 2 + 10);
        let limited = limit_text(text).unwrap();
        assert!(limited.len() <= MAX_EXTRACTED_TEXT_SIZE);
        assert!(limited.is_char_boundary(limited.len()));
    }
}
