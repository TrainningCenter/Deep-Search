# Deep-Search

A fast, lightweight, cross-platform file indexing and search engine written in Rust.

## Current capabilities

- Incremental file indexing with batched SQLite writes.
- SQLite-backed metadata storage and full-text content search (FTS5).
- Search by file name, path, content, extension, file category, and size range.
- Boolean search expressions with `AND`, `OR`, `NOT`, and parentheses.
- Paginated search with total-match counts.
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

The planned product separates the background indexing/search engine from its user interface and is intended to support Local, NAS, and Remote operating modes. The current repository is the engine foundation; the full desktop application and service-management layer are not yet implemented.

Linux is the first-priority platform, followed by Windows and macOS.
