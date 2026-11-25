# Deep-Search

# Desktop application (alpha)

The first cross-platform desktop window is available behind the `desktop` feature. It connects to the background service at `127.0.0.1:4765`, lets you submit one or more indexing paths, and displays search matches in a resizable table. Search results are retrieved automatically across transport batches; the 1000-record service limit is not a cap on the total number of displayed matches.

Start the background service first:

```bash
deep-search serve ~/.local/share/deep-search/index.sqlite
```

Then launch the desktop application:

```bash
deep-search-ui
```

For development, run it with:

```bash
cargo run --features desktop --bin deep-search-ui
```

The desktop alpha currently provides connection/status, indexing-path entry, query input, sorting, and a resizable results table. Saved view profiles, full column management, Arabic RTL, tray integration, and graphical folder picking are not implemented yet.



A fast, lightweight, cross-platform file indexing and search engine written in Rust.

## Current capabilities

- Incremental file indexing with batched SQLite writes.
- SQLite-backed metadata storage and full-text content search (FTS5).
- Search by file name, path, content, extension, file category, and size range.
- Boolean search expressions with `AND`, `OR`, `NOT`, and parentheses.
- Search returns every matching file, with total-match counts and bounded batches for efficient transfer; pagination is an internal transport detail, not a cap on total results.
- Deterministic sorting by relevance, name, path, extension, size, or modification time.
- Extracts searchable text from common text files, PDF, RTF, DOCX, ODT, PPTX, and XLSX.
- Reconciles deleted files after a completed scan while preserving records when a scan fails or a source is unavailable.

## Build

Requires a stable Rust toolchain.

```bash
cargo build --release
```

## CLI usage

Index one or more files or directories:

```bash
deep-search index index.sqlite /home/user/Documents /mnt/storage
```

Search by name:

```bash
deep-search search index.sqlite report
```

Search indexed document contents:

```bash
deep-search content index.sqlite "quarterly revenue"
```

Use the query language:

```bash
deep-search query index.sqlite 'name:"holiday video" AND (ext:mkv OR ext:mp4) AND size>=1GB'
deep-search query index.sqlite 'type:document AND path:"project archive"'
deep-search query index.sqlite 'content:"project roadmap" AND ext:pdf'
deep-search query index.sqlite 'NOT ext:tmp AND size<=500MB'
```

Get a bounded page of results:

```bash
deep-search query-page index.sqlite 50 0 'type:video'
```

Sort a page by modification time (newest first), file size (largest first), or any supported field:

```bash
deep-search query-page index.sqlite 50 0 --sort modified --order desc 'type:video'
deep-search query-page index.sqlite 50 0 --sort size --order desc 'type:video'
deep-search query-page index.sqlite 50 0 --sort name --order asc 'ext:pdf'
```

Supported sort fields: `relevance`, `name`, `path`, `extension`, `size`, `modified`.

Supported sort orders: `asc` and `desc`.

File categories currently supported by `type:`: `video`, `audio`, `image`, `document`, and `archive`.

Show the number of indexed files:

```bash
deep-search count index.sqlite
```

## Local background service (early)

The engine can now run as a persistent local service. It accepts one JSON request per TCP connection, with one JSON response line. By default it binds only to `127.0.0.1:4765`; this keeps the initial service local to the machine.

Start it:

```bash
deep-search serve index.sqlite
```

You can override the bind address explicitly:

```bash
deep-search serve index.sqlite 127.0.0.1:4765
```

Available request examples (send each as a single line):

```json
{"command":"status"}
{"command":"index","paths":["/home/user/Documents","/mnt/storage"]}
{"command":"search","query":"type:video AND (ext:mkv OR ext:mp4)","limit":50,"offset":0,"sort":"size","order":"desc"}
{"command":"content","term":"quarterly revenue"}
```

The service currently supports `status`, `index`, `search`, and `content`. Search accepts optional `limit` (1–1000 results per transport batch, default 100), `offset` (default 0), `sort` (`relevance`, `name`, `path`, `extension`, `size`, or `modified`), and `order` (`asc` or `desc`). **There is no maximum total number of search matches:** clients retrieve subsequent batches by increasing `offset` until `has_more` is false. The desktop interface will combine those batches into one continuous results table; users will not navigate pages or lose results because of the batch size. The response includes `results`, `total`, `limit`, `offset`, and `has_more`. Responses contain `ok: true` and a `data` object on success, or `ok: false` and an `error` string on failure.

Rust applications can use the built-in service client to retrieve the complete result set without managing offsets themselves:

```rust
use deep_search::client::ServiceClient;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = ServiceClient::new("127.0.0.1:4765");
    let search = client.search_all("type:video", "size", "desc")?;

    println!("Retrieved {} of {} matches", search.results.len(), search.total);
    for file in search.results {
        println!("{}", file["path"].as_str().unwrap_or_default());
    }
    Ok(())
}
```

`search_all` automatically requests each batch, appends it to one collection, checks that the total remains consistent, and reports incomplete responses instead of silently dropping results.


**Security and lifecycle:** the service is loopback-only by default and has no authentication, TLS, or remote/NAS transport. Do not expose it to an untrusted network. Linux systemd boot startup is available; tray integration and Windows/macOS service installers remain future work.

### Linux system service (systemd)

A system-wide systemd unit can start the engine at boot, before the selected user logs in, while running the process under that user's own Linux account (not root). Each user gets a separate database at `~/.local/share/deep-search/index.sqlite`.

Build the release binary, then install and enable the service for the intended account:

```bash
cargo build --release
sudo packaging/linux/systemd/install-service.sh "$USER" ./target/release/deep-search
```

Check or control the service with the installed helper:

```bash
deep-search-service status "$USER"
deep-search-service is-active "$USER"
deep-search-service is-enabled "$USER"
deep-search-service logs "$USER"
deep-search-service logs-follow "$USER"
sudo deep-search-service start "$USER"
sudo deep-search-service stop "$USER"
sudo deep-search-service restart "$USER"
sudo deep-search-service enable "$USER"
sudo deep-search-service disable "$USER"
```

`enable` and `disable` control automatic startup at boot; they do not start or stop the running process. Use `start` or `stop` for runtime control. `logs` shows the latest 100 journal entries and `logs-follow` streams new entries. The helper validates the Linux account and refuses to manage a root-owned instance.

To uninstall only that user's service instance (the index database is preserved):

```bash
sudo packaging/linux/systemd/uninstall-service.sh "$USER"
```

The installer requires systemd, root privileges, an existing non-root Linux account, and a compiled executable. The service inherits that account's filesystem permissions; it can index only locations that account can read. The unit uses a restrictive umask and binds the engine to loopback by default. The Settings UI toggle for automatic startup, and equivalent Windows/macOS installers, are not implemented yet.

## Query language

| Condition | Meaning | Example |
|---|---|---|
| `name:` | File name contains text | `name:invoice` |
| `path:` | Full path contains text | `path:"work archive"` |
| `content:` | Indexed document content contains text | `content:"annual report"` |
| `ext:` | Exact extension | `ext:pdf` |
| `type:` | File category | `type:video` |
| `size>=` | Minimum file size | `size>=1GB` |
| `size<=` | Maximum file size | `size<=500MB` |

Size units include `B`, `KB`/`KiB`, `MB`/`MiB`, `GB`/`GiB`, and `TB`/`TiB`. Boolean operators are case-insensitive. Use double quotes for values containing spaces.

## Project direction

The product separates the background indexing/search engine from its user interface and is intended to support Local, NAS, and Remote operating modes. The current repository contains the Rust engine, local JSON service, and an initial Linux systemd lifecycle controller; the desktop interface and cross-platform service managers are still under development.

Linux is the first-priority platform, followed by Windows and macOS.
