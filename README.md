# Deep-Search

# Download prebuilt alpha packages

GitHub Actions builds precompiled desktop packages for Linux x86_64, Windows x64, and macOS Apple Silicon. Open the [Deep-Search Alpha Builds workflow](https://github.com/TrainningCenter/Deep-Search/actions/workflows/release.yml), select the latest **successful** run, and download the artifact for your platform:

- Linux: `deep-search-linux-x64`
- Windows: `deep-search-windows-x64`
- macOS Apple Silicon: `deep-search-macos-arm64`

For Linux, extract the downloaded archive, open a terminal in the extracted directory, and install the desktop application and background service with:

```bash
sudo bash packaging/linux/install.sh "$USER"
```

These are alpha builds. The workflow runs the automated tests before packaging; hardware-specific desktop behavior and physical Windows/macOS service behavior still require real-machine validation.

# Desktop application (alpha)

The first cross-platform desktop window is available behind the `desktop` feature. It connects to the background service at `127.0.0.1:4765`, lets you submit one or more indexing paths, and displays search matches in a resizable table. Search results stream into the table as transport batches arrive; the 1000-record service limit is not a cap on the total number of displayed matches.

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

The desktop alpha currently provides automatic connection to the local service on launch, indexing-path entry (native folder picker and drag-and-drop paths), query input, sorting, and a resizable results table. The results view supports adding/removing/showing/hiding columns, drag-and-drop or button-based reordering, adjusting each column's width, row-height control, content-based auto-fit (sampling up to 500 loaded results), and persistent named view profiles. Default profiles include Default, Videos, Audio, Documents, Large files, and NAS. The Videos and Audio profiles expose optional media columns such as duration, resolution, codecs, frame rate, bit rate, sample rate, and channels. Profiles are stored locally in the user's application configuration directory. An Arabic-language toggle and installed-font fallback are available, but complete right-to-left layout and contextual shaping still need work. With the platform service helper installed, the UI exposes runtime start/stop and a boot-startup toggle on Linux, macOS, and Windows, using the platform's authorization prompt. It also offers **Stop service when user logs out**, disabled by default; when enabled, the background service waits for the account's first login session and exits cleanly after logout. If the OS cannot reliably report the session state, the service stays running rather than stopping unexpectedly. The UI disables this option and explains the limitation when session detection is unavailable. The system tray is enabled by default and provides Open, Pause/Resume Indexing, Start/Stop Engine, Settings, and Exit actions. Closing the main window hides it in the tray by default; both tray visibility and close-to-tray behavior are configurable in application settings. Desktop notifications can independently be enabled or disabled for desktop-initiated indexing completion, indexing errors, and service start/stop events. Frozen leading columns are supported by the current table. Platform service controls are wired to the Linux systemd, macOS launchd, and Windows Task Scheduler helpers; they still need runtime verification on physical macOS and Windows machines. The app loads an installed system Arabic font as a fallback when available. Separate macOS launchd and Windows startup-task installers are included in their platform packages.



A fast, lightweight, cross-platform file indexing and search engine written in Rust.

## Current capabilities

- Incremental file indexing with batched SQLite writes and parallel CPU-based document extraction.
- Persistent indexing roots and automatic filesystem change monitoring (create, modify, rename, delete) with debounced targeted refreshes; a 10-minute reconciliation pass catches missed events and changes on filesystems with limited notification support.
- SQLite-backed metadata storage and full-text content search (FTS5).
- Search by file name, path, content, extension, file category, and size range.
- Boolean search expressions with `AND`, `OR`, `NOT`, and parentheses.
- Search returns every matching file, with total-match counts and bounded batches for efficient transfer; pagination is an internal transport detail, not a cap on total results.
- Deterministic sorting by relevance, name, path, extension, size, or modification time.
- Extracts searchable text from common text files, PDF, RTF, DOCX, ODT, PPTX, and XLSX.
- Extracts optional audio/video technical metadata through `ffprobe` when it is installed: duration, container, codecs, resolution, frame rate, bit rate, sample rate, and channel count. Media probing is best-effort and never blocks normal file indexing. Set `DEEP_SEARCH_FFPROBE` to an alternate executable path if needed.
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

File categories currently supported by `type:`: `video`, `audio`, `image`, `document`, and `archive`. The video category includes common playback formats (including ASF, DIVX, F4V, MPEG-1/2, RealMedia, and the usual MP4/MKV/MOV/AVI/WMV/WebM formats) while intentionally excluding camera-specific recording containers such as 3GP, MTS, M2TS, MXF, MOD, TOD, and DV.

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
{"command":"set_stop_on_logout","enabled":true}
{"command":"index","paths":["/home/user/Documents","/mnt/storage"]}
{"command":"pause_indexing"}
{"command":"resume_indexing"}
{"command":"search","query":"type:video AND (ext:mkv OR ext:mp4)","limit":50,"offset":0,"sort":"size","order":"desc"}
{"command":"content","term":"quarterly revenue"}
```

The service supports `status`, `index`, `pause_indexing`, `resume_indexing`, `search`, and `content`. Pause and resume take effect at safe indexing batch boundaries and apply to both manual and automatic filesystem indexing. The desktop application exposes the same controls in its service panel and system tray menu. It also supports `set_stop_on_logout` to configure session-exit behavior. The `set_stop_on_logout` command persists the optional logout behavior. It is disabled by default; when enabled, the service waits until the account has logged in at least once, then exits cleanly after two consecutive checks find no interactive session. Each client connection gets its own SQLite connection, allowing multiple local clients to search concurrently while index writes are serialized. SQLite WAL mode lets readers continue during indexing. Search accepts optional `limit` (1–1000 results per transport batch, default 100), `offset` (default 0), `sort` (`relevance`, `name`, `path`, `extension`, `size`, or `modified`), and `order` (`asc` or `desc`). **There is no maximum total number of search matches:** clients retrieve subsequent batches by increasing `offset` until `has_more` is false. The desktop interface displays each batch as soon as it arrives in one continuous results table; users will not navigate pages or lose results because of the batch size. The response includes `results`, `total`, `limit`, `offset`, and `has_more`. Responses contain `ok: true` and a `data` object on success, or `ok: false` and an `error` string on failure.

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


**Security and lifecycle:** the service is loopback-only by default and has no authentication, TLS, or remote client transport. Do not expose it to an untrusted network. The indexer can index local folders and already-mounted network shares; remote access to the service itself is not implemented. Boot-start installers are included for Linux systemd, macOS launchd, and Windows Task Scheduler. The system tray integration is implemented; physical runtime validation of tray behavior and the macOS/Windows service managers remains necessary on their respective operating systems.

### Linux system service (systemd)

A system-wide systemd unit can start the engine at boot, before the selected user logs in, while running the process under that user's own Linux account (not root). Each user gets a separate database at `~/.local/share/deep-search/index.sqlite`.

Extract the Linux package, open a terminal in the extracted directory, then install the desktop application and background service together:

```bash
sudo bash packaging/linux/install.sh "$USER"
```

The installer places both executables in `/usr/local/bin`, adds Deep-Search to the desktop application menu, and enables the systemd service for the selected non-root account. You can then launch Deep-Search from the application menu or run `deep-search-ui`.

For a service-only installation (without the desktop launcher), use:

```bash
sudo bash packaging/linux/systemd/install-service.sh "$USER" ./deep-search
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

`enable` and `disable` control automatic startup at boot; they do not start or stop the running process. Use `start` or `stop` for runtime control. The desktop Settings panel also controls whether the service should exit after the last interactive session logs out; this preference is off by default, so the service normally continues running after logout. `logs` shows the latest 100 journal entries and `logs-follow` streams new entries. The helper validates the Linux account and refuses to manage a root-owned instance.

To uninstall only that user's service instance (the index database is preserved):

```bash
sudo packaging/linux/systemd/uninstall-service.sh "$USER"
```

The installer requires systemd, root privileges, an existing non-root Linux account, and a compiled executable. The service inherits that account's filesystem permissions; it can index only locations that account can read. The unit uses a restrictive umask and binds the engine to loopback by default. The desktop Settings panel can enable or disable boot startup through the installed service helper. Equivalent installers and helpers are included in the macOS and Windows packages; physical runtime validation is still required on those platforms.

### macOS background service (launchd)

The macOS package includes a LaunchDaemon that starts before login, runs under the selected non-root account, and stays alive across logout. Install from the extracted package:

```bash
sudo bash packaging/macos/launchd/install-service.sh "$USER" ./deep-search
```

When `deep-search-ui` is present beside the engine binary in the extracted package, the installer also creates `/Applications/Deep-Search.app`. The installer also installs `deep-search-service` in `/usr/local/bin`. Launch the desktop app from Applications or with `open -a Deep-Search`. Check or control the service:

```bash
deep-search-service status
deep-search-service is-active
deep-search-service is-enabled
sudo deep-search-service enable
sudo deep-search-service disable
deep-search-service start
deep-search-service stop
deep-search-service restart
```

Uninstall the LaunchDaemon without deleting the index database:

```bash
sudo bash packaging/macos/launchd/uninstall-service.sh
```

### Windows background service (Task Scheduler)

The Windows package includes an elevated PowerShell installer. It registers a Task Scheduler task triggered at system startup, runs the engine under the selected account without requiring an interactive login, and restarts it after failure. When `deep-search-ui.exe` is beside the engine binary, it also installs the desktop application and creates a Start Menu shortcut:

```powershell
.\packaging\windows\task-scheduler\install-service.ps1 -UserName $env:USERNAME -BinaryPath .\deep-search.exe
```

By default, the task uses an S4U logon and does not store the user's password; this supports local storage but cannot authenticate to protected network shares. To allow access to authenticated NAS/SMB shares through UNC paths, run the installer with `-EnableAuthenticatedNetworkShares`. It will request the account password and let Windows Task Scheduler protect the stored task credential. Use this only when needed and ensure the selected account has the required share and filesystem permissions.

Manage the task from an elevated PowerShell session:

```powershell
& "$env:ProgramFiles\Deep-Search\deep-search-service.ps1" status
& "$env:ProgramFiles\Deep-Search\deep-search-service.ps1" start
& "$env:ProgramFiles\Deep-Search\deep-search-service.ps1" stop
& "$env:ProgramFiles\Deep-Search\deep-search-service.ps1" enable
& "$env:ProgramFiles\Deep-Search\deep-search-service.ps1" disable
```

Remove the scheduled task and installed binaries while preserving the index database:

```powershell
.\packaging\windows\task-scheduler\uninstall-service.ps1
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

The product separates the background indexing/search engine from its user interface and is intended to support Local, NAS, and Remote operating modes. The current repository contains the Rust engine, local JSON service, desktop interface, filesystem change watcher, and platform service managers for Linux systemd, macOS launchd, and Windows Task Scheduler. Remote service transport, tray integration, and full Arabic RTL behavior remain under development.

Linux is the first-priority platform, followed by Windows and macOS.
