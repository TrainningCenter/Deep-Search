use crate::database::{SearchSortDirection, SearchSortField};
use crate::engine::{IndexingReport, SearchEngine};
use crate::query::parse_query;
use serde_json::{json, Value};
use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const MAX_REQUEST_BYTES: usize = 1_048_576;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);

/// Run the local JSON-line service. It intentionally binds to loopback by default;
/// remote/NAS access requires a separately secured transport layer.
pub fn run(database_path: &Path, address: &str) -> Result<(), Box<dyn Error>> {
    let engine = SearchEngine::open(database_path)?;
    let initial_roots = engine.index_roots()?;
    let index_lock = Arc::new(Mutex::new(()));
    let watcher_database_path = std::fs::canonicalize(database_path)
        .unwrap_or_else(|_| database_path.to_path_buf());
    let watch_roots = spawn_filesystem_watcher(
        watcher_database_path,
        initial_roots,
        Arc::clone(&index_lock),
    );
    let listener = TcpListener::bind(address)?;
    eprintln!("Deep-Search service listening on {address}");

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                if let Err(error) = handle_connection(
                    stream, &engine, &watch_roots, &index_lock,
                ) {
                    eprintln!("Service request failed: {error}");
                }
            }
            Err(error) => eprintln!("Incoming connection failed: {error}"),
        }
    }
    Ok(())
}

fn handle_connection(
    mut stream: TcpStream,
    engine: &SearchEngine,
    watch_roots: &mpsc::Sender<PathBuf>,
    index_lock: &Arc<Mutex<()>>,
) -> Result<(), Box<dyn Error>> {
    stream.set_read_timeout(Some(CONNECTION_TIMEOUT))?;
    stream.set_write_timeout(Some(CONNECTION_TIMEOUT))?;
    let mut request = String::new();
    let limited_stream = stream.try_clone()?.take((MAX_REQUEST_BYTES + 1) as u64);
    BufReader::new(limited_stream).read_line(&mut request)?;
    let response = if !is_valid_request_line(&request) {
        json!({"ok": false, "error": "Request must be newline-terminated and no larger than 1 MiB"})
    } else {
        handle_request_with_context(&request, engine, Some(watch_roots), Some(index_lock))
    };
    serde_json::to_writer(&mut stream, &response)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn spawn_filesystem_watcher(
    database_path: PathBuf,
    initial_roots: Vec<PathBuf>,
    index_lock: Arc<Mutex<()>>,
) -> mpsc::Sender<PathBuf> {
    let (root_sender, root_receiver) = mpsc::channel::<PathBuf>();
    thread::spawn(move || {
        let engine = match SearchEngine::open(&database_path) {
            Ok(engine) => engine,
            Err(error) => {
                eprintln!("Filesystem watcher could not open the index: {error}");
                return;
            }
        };
        let (event_sender, event_receiver) = mpsc::channel();
        let mut watcher: RecommendedWatcher = match notify::recommended_watcher(
            move |event| { let _ = event_sender.send(event); },
        ) {
            Ok(watcher) => watcher,
            Err(error) => {
                eprintln!("Filesystem notifications unavailable; using periodic reconciliation: {error}");
                if let Ok(_guard) = index_lock.lock() {
                    if let Ok(roots) = engine.index_roots() {
                        let paths = roots.iter().map(PathBuf::as_path).collect::<Vec<_>>();
                        let report = engine.index_paths(&paths);
                        eprintln!(
                            "Startup fallback reconciliation complete: {} discovered, {} saved, {} removed, {} errors",
                            report.files_discovered, report.files_saved,
                            report.files_removed, report.errors
                        );
                    }
                }
                loop {
                    thread::sleep(Duration::from_secs(600));
                    if let Ok(_guard) = index_lock.lock() {
                        if let Ok(roots) = engine.index_roots() {
                            let paths = roots.iter().map(PathBuf::as_path).collect::<Vec<_>>();
                            let report = engine.index_paths(&paths);
                            eprintln!(
                                "Periodic reconciliation complete: {} discovered, {} saved, {} removed, {} errors",
                                report.files_discovered, report.files_saved,
                                report.files_removed, report.errors
                            );
                        }
                    }
                }
            }
        };
        let mut watched = HashSet::new();
        for root in &initial_roots {
            register_watch(&mut watcher, root, &mut watched);
        }
        if !initial_roots.is_empty() {
            match index_lock.lock() {
                Ok(_guard) => {
                    let paths = initial_roots.iter().map(PathBuf::as_path).collect::<Vec<_>>();
                    let report = engine.index_paths(&paths);
                    eprintln!(
                        "Startup reconciliation complete: {} discovered, {} saved, {} removed, {} errors",
                        report.files_discovered, report.files_saved,
                        report.files_removed, report.errors
                    );
                }
                Err(_) => eprintln!("Startup reconciliation skipped because the indexing lock was poisoned"),
            }
        }

        let mut pending_paths = HashSet::<PathBuf>::new();
        let mut pending = false;
        let mut force_full_refresh = false;
        let mut last_event = Instant::now();
        let mut last_full_refresh = Instant::now();
        loop {
            while let Ok(root) = root_receiver.try_recv() {
                register_watch(&mut watcher, &root, &mut watched);
            }

            match event_receiver.recv_timeout(Duration::from_millis(250)) {
                Ok(Ok(event)) => {
                    if event_requires_reindex(&event.kind)
                        && !event_is_database_internal(&event, &database_path)
                    {
                        if event.paths.is_empty() {
                            force_full_refresh = true;
                        } else {
                            pending_paths.extend(event.paths.iter().cloned());
                        }
                        pending = true;
                        last_event = Instant::now();
                    }
                }
                Ok(Err(error)) => eprintln!("Filesystem watcher event error: {error}"),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }

            let periodic_refresh_due = last_full_refresh.elapsed() >= Duration::from_secs(600);
            let event_refresh_due = pending && last_event.elapsed() >= Duration::from_millis(750);
            if periodic_refresh_due || event_refresh_due {
                match index_lock.lock() {
                    Ok(_guard) => {
                        let report = if periodic_refresh_due || force_full_refresh {
                            match engine.index_roots() {
                                Ok(roots) if !roots.is_empty() => {
                                    let paths = roots.iter().map(PathBuf::as_path).collect::<Vec<_>>();
                                    for root in &roots {
                                        register_watch(&mut watcher, root, &mut watched);
                                    }
                                    engine.index_paths(&paths)
                                }
                                Ok(_) => IndexingReport::default(),
                                Err(error) => {
                                    eprintln!("Could not read configured index roots: {error}");
                                    IndexingReport { errors: 1, ..IndexingReport::default() }
                                }
                            }
                        } else {
                            let paths = pending_paths.iter().cloned().collect::<Vec<_>>();
                            engine.refresh_changed_paths(&paths)
                        };
                        eprintln!(
                            "Automatic refresh complete: {} discovered, {} saved, {} removed, {} errors",
                            report.files_discovered, report.files_saved,
                            report.files_removed, report.errors
                        );
                        if periodic_refresh_due || force_full_refresh {
                            last_full_refresh = Instant::now();
                        }
                        for root in engine.index_roots().unwrap_or_default() {
                            register_watch(&mut watcher, &root, &mut watched);
                        }
                    }
                    Err(_) => eprintln!("Automatic refresh skipped because the indexing lock was poisoned"),
                }
                pending_paths.clear();
                pending = false;
                force_full_refresh = false;
            }
        }
    });
    root_sender
}

fn register_watch(
    watcher: &mut RecommendedWatcher,
    root: &Path,
    watched: &mut HashSet<PathBuf>,
) {
    let target = if root.exists() {
        root.to_path_buf()
    } else if let Some(parent) = root.parent().filter(|parent| parent.exists()) {
        parent.to_path_buf()
    } else {
        return;
    };
    if !watched.insert(target.clone()) {
        return;
    }
    let mode = if target.is_dir() {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    if let Err(error) = watcher.watch(&target, mode) {
        watched.remove(&target);
        eprintln!("Could not watch {}: {error}", target.display());
    }
}

fn event_requires_reindex(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    )
}

fn event_is_database_internal(event: &notify::Event, database_path: &Path) -> bool {
    if event.paths.is_empty() {
        return false;
    }
    let database = database_path.to_string_lossy().into_owned();
    event.paths.iter().all(|path| {
        let value = path.to_string_lossy();
        value == database
            || value == format!("{database}-wal")
            || value == format!("{database}-shm")
            || value == format!("{database}-journal")
    })
}

fn is_valid_request_line(request: &str) -> bool {
    request.len() <= MAX_REQUEST_BYTES && request.ends_with('\n')
}

fn handle_request(input: &str, engine: &SearchEngine) -> Value {
    handle_request_with_context(input, engine, None, None)
}

fn handle_request_with_context(
    input: &str,
    engine: &SearchEngine,
    watch_roots: Option<&mpsc::Sender<PathBuf>>,
    index_lock: Option<&Arc<Mutex<()>>>,
) -> Value {
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
                if let Some(sender) = watch_roots {
                    for path in &paths {
                        let _ = sender.send(PathBuf::from(path));
                    }
                }
                let report = if let Some(lock) = index_lock {
                    let _guard = lock.lock().map_err(|_| "Indexing lock is unavailable")?;
                    engine.index_paths(&paths)
                } else {
                    engine.index_paths(&paths)
                };
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
                let limit = request.get("limit").and_then(Value::as_u64).unwrap_or(100);
                if !(1..=1000).contains(&limit) {
                    return Err("Search limit must be between 1 and 1000".into());
                }
                let offset = request.get("offset").and_then(Value::as_u64).unwrap_or(0);
                let sort = match request.get("sort").and_then(Value::as_str).unwrap_or("relevance") {
                    "relevance" => SearchSortField::Relevance,
                    "name" => SearchSortField::Name,
                    "path" => SearchSortField::Path,
                    "extension" | "ext" => SearchSortField::Extension,
                    "size" => SearchSortField::Size,
                    "modified" | "date" => SearchSortField::Modified,
                    _ => return Err("Sort must be relevance, name, path, extension, size, or modified".into()),
                };
                let order = match request.get("order").and_then(Value::as_str).unwrap_or("asc") {
                    "asc" | "ascending" => SearchSortDirection::Ascending,
                    "desc" | "descending" => SearchSortDirection::Descending,
                    _ => return Err("Order must be asc or desc".into()),
                };
                let total = engine.count_search_results(&expression)?;
                let results = engine.search_page_sorted(
                    &expression, limit as u32, offset, sort, order,
                )?;
                Ok(json!({
                    "results": results,
                    "total": total,
                    "limit": limit,
                    "offset": offset,
                    "has_more": offset.saturating_add(results.len() as u64) < total
                }))
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
    fn search_api_supports_pagination_and_sorting() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let base = std::env::temp_dir().join(format!(
            "deep-search-service-page-{}-{unique}", std::process::id()
        ));
        let root = base.join("files");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("report-small.txt"), b"small").unwrap();
        std::fs::write(root.join("report-large.txt"), b"a larger report").unwrap();

        let engine = SearchEngine::open(base.join("index.sqlite")).unwrap();
        let indexed = engine.index_paths(&[root.as_path()]);
        assert_eq!(indexed.errors, 0);
        assert_eq!(indexed.files_saved, 2);

        let response = handle_request(
            r#"{"command":"search","query":"name:report","limit":1,"offset":0,"sort":"size","order":"desc"}"#,
            &engine,
        );
        assert_eq!(response["ok"], true);
        assert_eq!(response["data"]["limit"], 1);
        assert_eq!(response["data"]["offset"], 0);
        assert_eq!(response["data"]["total"], 2);
        assert_eq!(response["data"]["has_more"], true);
        assert_eq!(response["data"]["results"].as_array().unwrap().len(), 1);
        assert_eq!(response["data"]["results"][0]["name"], "report-large.txt");

        let next_page = handle_request(
            r#"{"command":"search","query":"name:report","limit":1,"offset":1,"sort":"size","order":"desc"}"#,
            &engine,
        );
        assert_eq!(next_page["data"]["has_more"], false);
        assert_eq!(next_page["data"]["results"][0]["name"], "report-small.txt");

        let invalid = handle_request(
            r#"{"command":"search","query":"name:report","limit":1001}"#,
            &engine,
        );
        assert_eq!(invalid["ok"], false);
        assert!(invalid["error"].as_str().unwrap().contains("between 1 and 1000"));

        drop(engine);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn search_api_returns_all_matches_across_automatic_client_batches() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let base = std::env::temp_dir().join(format!(
            "deep-search-service-unbounded-{}-{unique}", std::process::id()
        ));
        let root = base.join("files");
        std::fs::create_dir_all(&root).unwrap();
        for index in 0..1005 {
            std::fs::write(root.join(format!("report-{index:04}.txt")), b"report").unwrap();
        }

        let engine = SearchEngine::open(base.join("index.sqlite")).unwrap();
        let indexed = engine.index_paths(&[root.as_path()]);
        assert_eq!(indexed.errors, 0);
        assert_eq!(indexed.files_saved, 1005);

        // Model the desktop client's behavior: fetch each transport batch automatically
        // and append it to one continuous result collection until the server says stop.
        let mut offset = 0_u64;
        let mut all_names = Vec::new();
        loop {
            let request = format!(
                r#"{{"command":"search","query":"name:report","limit":1000,"offset":{offset},"sort":"name","order":"asc"}}"#
            );
            let response = handle_request(&request, &engine);
            assert_eq!(response["ok"], true);
            assert_eq!(response["data"]["total"], 1005);

            let batch = response["data"]["results"].as_array().unwrap();
            assert!(batch.len() <= 1000);
            assert!(!batch.is_empty());
            all_names.extend(batch.iter().map(|item| {
                item["name"].as_str().unwrap().to_owned()
            }));
            offset += batch.len() as u64;

            if !response["data"]["has_more"].as_bool().unwrap() {
                break;
            }
        }

        assert_eq!(all_names.len(), 1005);
        let mut unique_names = all_names.clone();
        unique_names.sort();
        unique_names.dedup();
        assert_eq!(unique_names.len(), 1005);
        assert_eq!(all_names.first().unwrap(), "report-0000.txt");
        assert_eq!(all_names.last().unwrap(), "report-1004.txt");

        drop(engine);
        let _ = std::fs::remove_dir_all(base);
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
    #[test]
    fn filesystem_watcher_filters_non_content_events_and_database_sidecars() {
        assert!(event_requires_reindex(&EventKind::Any));
        assert!(!event_requires_reindex(&EventKind::Other));

        let database = PathBuf::from("/tmp/deep-search-index.sqlite");
        let wal_event = notify::Event::new(EventKind::Any)
            .add_path(PathBuf::from("/tmp/deep-search-index.sqlite-wal"));
        let user_file_event = notify::Event::new(EventKind::Any)
            .add_path(PathBuf::from("/tmp/notes.txt"));
        assert!(event_is_database_internal(&wal_event, &database));
        assert!(!event_is_database_internal(&user_file_event, &database));
    }

}
