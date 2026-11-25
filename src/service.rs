use crate::engine::SearchEngine;
use crate::query::parse_query;
use serde_json::{json, Value};
use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::Duration;

const MAX_REQUEST_BYTES: usize = 1_048_576;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);

/// Run the local JSON-line service. It intentionally binds to loopback by default;
/// remote/NAS access requires a separately secured transport layer.
pub fn run(database_path: &Path, address: &str) -> Result<(), Box<dyn Error>> {
    let engine = SearchEngine::open(database_path)?;
    let listener = TcpListener::bind(address)?;
    eprintln!("Deep-Search service listening on {address}");

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                if let Err(error) = handle_connection(stream, &engine) {
                    eprintln!("Service request failed: {error}");
                }
            }
            Err(error) => eprintln!("Incoming connection failed: {error}"),
        }
    }
    Ok(())
}

fn handle_connection(mut stream: TcpStream, engine: &SearchEngine) -> Result<(), Box<dyn Error>> {
    stream.set_read_timeout(Some(CONNECTION_TIMEOUT))?;
    stream.set_write_timeout(Some(CONNECTION_TIMEOUT))?;
    let mut request = String::new();
    let limited_stream = stream.try_clone()?.take((MAX_REQUEST_BYTES + 1) as u64);
    BufReader::new(limited_stream).read_line(&mut request)?;
    let response = if !is_valid_request_line(&request) {
        json!({"ok": false, "error": "Request must be newline-terminated and no larger than 1 MiB"})
    } else {
        handle_request(&request, engine)
    };
    serde_json::to_writer(&mut stream, &response)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn is_valid_request_line(request: &str) -> bool {
    request.len() <= MAX_REQUEST_BYTES && request.ends_with('\n')
}

fn handle_request(input: &str, engine: &SearchEngine) -> Value {
    let result = (|| -> Result<Value, Box<dyn Error>> {
        let request: Value = serde_json::from_str(input)?;
        let command = request.get("command").and_then(Value::as_str)
            .ok_or("Missing string field: command")?;

        match command {
            "status" => Ok(json!({
                "indexed_files": engine.indexed_file_count()?,
                "version": crate::VERSION
            })),
            "index" => {
                let paths = request.get("paths").and_then(Value::as_array)
                    .ok_or("Missing array field: paths")?;
                let paths: Result<Vec<&str>, _> = paths.iter()
                    .map(|path| path.as_str().ok_or("Every path must be a string"))
                    .collect();
                let paths = paths?;
                if paths.is_empty() {
                    return Err("At least one path is required".into());
                }
                let report = engine.index_paths(&paths);
                Ok(json!({
                    "files_discovered": report.files_discovered,
                    "directories_scanned": report.directories_scanned,
                    "files_saved": report.files_saved,
                    "files_unchanged": report.files_unchanged,
                    "files_removed": report.files_removed,
                    "errors": report.errors
                }))
            }
            "search" => {
                let query = request.get("query").and_then(Value::as_str)
                    .ok_or("Missing string field: query")?;
                let expression = parse_query(query)?;
                let results = engine.search_page(&expression, 100, 0)?;
                Ok(json!({"results": results, "limit": 100}))
            }
            "content" => {
                let term = request.get("term").and_then(Value::as_str)
                    .ok_or("Missing string field: term")?;
                let results = engine.search_content(term)?;
                Ok(json!({"results": results}))
            }
            _ => Err(format!("Unknown command: {command}").into()),
        }
    })();

    match result {
        Ok(data) => json!({"ok": true, "data": data}),
        Err(error) => json!({"ok": false, "error": error.to_string()}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_lines_are_bounded_and_terminated() {
        assert!(is_valid_request_line("{\"command\":\"status\"}\n"));
        assert!(!is_valid_request_line("{\"command\":\"status\"}"));
        assert!(!is_valid_request_line(&format!("{}\n", "x".repeat(MAX_REQUEST_BYTES))));
    }

    #[test]
    fn rejects_invalid_json_and_unknown_commands() {
        let db_path = std::env::temp_dir().join(format!(
            "deep-search-service-test-{}.sqlite", std::process::id()
        ));
        let engine = SearchEngine::open(&db_path).unwrap();

        assert_eq!(handle_request("{", &engine)["ok"], false);
        let response = handle_request(r#"{"command":"not-a-command"}"#, &engine);
        assert_eq!(response["ok"], false);
        assert!(response["error"].as_str().unwrap().contains("Unknown command"));

        drop(engine);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn reports_service_status() {
        let db_path = std::env::temp_dir().join(format!(
            "deep-search-service-status-{}.sqlite", std::process::id()
        ));
        let engine = SearchEngine::open(&db_path).unwrap();
        let response = handle_request(r#"{"command":"status"}"#, &engine);

        assert_eq!(response["ok"], true);
        assert_eq!(response["data"]["indexed_files"], 0);
        assert_eq!(response["data"]["version"], crate::VERSION);

        drop(engine);
        let _ = std::fs::remove_file(db_path);
    }
}
