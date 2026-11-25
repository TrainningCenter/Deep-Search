
use deep_search::database::{SearchSortDirection, SearchSortField};
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

        "query-page" => {
            if args.len() < 6 {
                print_usage();
                return Ok(());
            }

            let limit: u32 = args[3].parse()
                .map_err(|_| "Page limit must be a positive integer")?;
            let offset: u64 = args[4].parse()
                .map_err(|_| "Page offset must be a non-negative integer")?;
            let mut sort_field = SearchSortField::Relevance;
            let mut sort_direction = SearchSortDirection::Ascending;
            let mut query_start = 5;

            while query_start < args.len() {
                match args[query_start].as_str() {
                    "--sort" if query_start + 1 < args.len() => {
                        sort_field = parse_sort_field(&args[query_start + 1])?;
                        query_start += 2;
                    }
                    "--order" if query_start + 1 < args.len() => {
                        sort_direction = parse_sort_direction(&args[query_start + 1])?;
                        query_start += 2;
                    }
                    _ => break,
                }
            }

            if query_start >= args.len() {
                print_usage();
                return Ok(());
            }

            let query_text = args[query_start..].join(" ");
            let expression = parse_query(&query_text)?;
            let total = engine.count_search_results(&expression)?;
            let results = engine.search_page_sorted(
                &expression, limit, offset, sort_field, sort_direction,
            )?;

            println!(
                "Showing {} result(s) from offset {} ({} total match(es)).",
                results.len(),
                offset,
                total
            );
            for file in results {
                let modified = file.modified_timestamp_ns
                    .map(|timestamp| timestamp.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                println!(
                    "{} | {} bytes | modified: {} ns | {}",
                    file.name,
                    file.size_bytes,
                    modified,
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

fn parse_sort_field(value: &str) -> Result<SearchSortField, &'static str> {
    match value.to_ascii_lowercase().as_str() {
        "relevance" => Ok(SearchSortField::Relevance),
        "name" => Ok(SearchSortField::Name),
        "path" => Ok(SearchSortField::Path),
        "extension" | "ext" => Ok(SearchSortField::Extension),
        "size" => Ok(SearchSortField::Size),
        "modified" | "date" => Ok(SearchSortField::Modified),
        _ => Err("Sort field must be relevance, name, path, extension, size, or modified"),
    }
}

fn parse_sort_direction(value: &str) -> Result<SearchSortDirection, &'static str> {
    match value.to_ascii_lowercase().as_str() {
        "asc" | "ascending" => Ok(SearchSortDirection::Ascending),
        "desc" | "descending" => Ok(SearchSortDirection::Descending),
        _ => Err("Sort order must be asc or desc"),
    }
}

#[cfg(test)]
mod cli_option_tests {
    use super::*;

    #[test]
    fn parses_supported_sort_fields_and_directions() {
        assert_eq!(parse_sort_field("modified").unwrap(), SearchSortField::Modified);
        assert_eq!(parse_sort_field("EXT").unwrap(), SearchSortField::Extension);
        assert_eq!(parse_sort_direction("DESC").unwrap(), SearchSortDirection::Descending);
        assert_eq!(parse_sort_direction("ascending").unwrap(), SearchSortDirection::Ascending);
    }

    #[test]
    fn rejects_unknown_sort_options() {
        assert!(parse_sort_field("random").is_err());
        assert!(parse_sort_direction("sideways").is_err());
    }
}

fn print_usage() {
    println!("Deep-Search");
    println!();
    println!("Commands:");
    println!("  deep-search index <database> <folder>...");
    println!("  deep-search search <database> <name>");
    println!("  deep-search content <database> <text>");
    println!(r#"  deep-search query <database> 'name:video' AND (ext:mkv OR ext:mp4) AND size>=1GB"#);
    println!(r#"  deep-search query-page <database> <limit> <offset> 'type:video' AND path:"Media Archive""#);
    println!("    Optional: --sort relevance|name|path|extension|size|modified --order asc|desc");
    println!("  deep-search extension <database> <extension>");
    println!("  deep-search count <database>");
}
