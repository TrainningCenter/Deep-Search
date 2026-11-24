
use deep_search::engine::SearchEngine;
use deep_search::query::parse_query;
use std::env;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        print_usage();
        return Ok(());
    }

    let command = args[1].as_str();
    let database_path = Path::new(&args[2]);

    if let Some(parent) = database_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let engine = SearchEngine::open(database_path)?;

    match command {
        "index" => {
            if args.len() < 4 {
                print_usage();
                return Ok(());
            }

            let paths: Vec<_> = args[3..]
                .iter()
                .map(|path| path.as_str())
                .collect();

            let report = engine.index_paths(&paths);

            println!("Indexing completed.");
            println!("Files discovered: {}", report.files_discovered);
            println!("Files saved: {}", report.files_saved);
            println!("Files unchanged: {}", report.files_unchanged);
            println!("Files removed: {}", report.files_removed);
            println!("Directories scanned: {}", report.directories_scanned);
            println!("Errors: {}", report.errors);
        }

        "search" => {
            if args.len() < 4 {
                print_usage();
                return Ok(());
            }

            let term = args[3..].join(" ");
            let results = engine.search_by_name(&term)?;

            println!("Found {} result(s).", results.len());

            for file in results {
                println!(
                    "{} | {} bytes | {}",
                    file.name,
                    file.size_bytes,
                    file.path.display()
                );
            }
        }

        "query" => {
            if args.len() < 4 {
                print_usage();
                return Ok(());
            }

            let query_text = args[3..].join(" ");
            let expression = parse_query(&query_text)?;
            let results = engine.search(&expression)?;

            println!("Found {} result(s).", results.len());
            for file in results {
                println!(
                    "{} | {} bytes | {}",
                    file.name,
                    file.size_bytes,
                    file.path.display()
                );
            }
        }

        "content" => {
            if args.len() < 4 {
                print_usage();
                return Ok(());
            }

            let term = args[3..].join(" ");
            let results = engine.search_content(&term)?;
            println!("Found {} result(s).", results.len());
            for file in results {
                println!(
                    "{} | {} bytes | {}",
                    file.name,
                    file.size_bytes,
                    file.path.display()
                );
            }
        }

        "extension" => {
            if args.len() != 4 {
                print_usage();
                return Ok(());
            }

            let results = engine.search_by_extension(&args[3])?;

            println!("Found {} result(s).", results.len());

            for file in results {
                println!(
                    "{} | {} bytes | {}",
                    file.name,
                    file.size_bytes,
                    file.path.display()
                );
            }
        }

        "count" => {
            println!(
                "Indexed files: {}",
                engine.indexed_file_count()?
            );
        }

        _ => print_usage(),
    }

    Ok(())
}

fn print_usage() {
    println!("Deep-Search");
    println!();
    println!("Commands:");
    println!("  deep-search index <database> <folder>...");
    println!("  deep-search search <database> <name>");
    println!("  deep-search content <database> <text>");
    println!(r#"  deep-search query <database> 'name:video' AND (ext:mkv OR ext:mp4) AND size>=1GB"#);
    println!("  deep-search extension <database> <extension>");
    println!("  deep-search count <database>");
}
