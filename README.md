# Deep-Search

# Download prebuilt alpha packages

**Ready-to-use downloads are published on the GitHub Releases page** — you do not need to download workflow artifacts.

Open the [latest Deep-Search Alpha Release](https://github.com/TrainningCenter/Deep-Search/releases/latest) and download the package for your platform:

| Platform | Release asset |
| --- | --- |
| Windows 10/11, x64 | `deep-search-windows-x64.zip` |
| Linux, x86_64 | `deep-search-linux-x64.tar.gz` |
| macOS, Apple Silicon (ARM64) | `deep-search-macos-arm64.tar.gz` |
| Docker service, Linux x86_64 | `deep-search-docker-linux-x64.tar` |

Each package has a matching `.sha256` file. Verify downloads before installation:

## Maintainers: automatic GitHub Releases

The release workflow publishes a new alpha release after the Linux, Windows, macOS, and Docker packages have all built and passed package verification. It uses the workflow's built-in `GITHUB_TOKEN`, with `contents: write` permission scoped to the publishing job. No personal access token or repository secret is required for the current workflow.

If release publishing is denied after changing repository or organization Actions policies, check that the workflow is allowed to write repository contents and create releases. Never commit a personal access token or other secret to the repository.

- Linux/macOS: `sha256sum -c deep-search-<platform>.sha256`
- Windows PowerShell: compare `(Get-FileHash .\deep-search-windows-x64.zip -Algorithm SHA256).Hash.ToLower()` with the hash in `deep-search-windows-x64.sha256`.

## Install the desktop application and background service

### Linux

1. Extract `deep-search-linux-x64.tar.gz`.
2. Open a terminal in the extracted directory.
3. Install the desktop application and boot-starting background service for your current account:

```bash
sudo bash packaging/linux/install.sh "$USER"
```

Launch **Deep-Search** from the desktop application menu, or run `deep-search-ui`. The installer places the UI in `/usr/local/bin` and enables the systemd service at boot.

### Windows 10/11 (x64)

1. Extract `deep-search-windows-x64.zip`. This is the complete package; there is no second archive to extract.
2. Open **PowerShell as Administrator** in the extracted directory.
3. Run:

```powershell
Set-ExecutionPolicy -Scope Process Bypass
.\packaging\windows\task-scheduler\install-service.ps1 -UserName "$env:USERDOMAIN\$env:USERNAME" -BinaryPath ".\deep-search.exe"
```

The installer copies both executables into Program Files, creates a Start Menu shortcut, and registers the background service to start at boot. By default, Windows S4U logon cannot access password-protected network shares. To enable authenticated UNC/NAS shares, add `-EnableAuthenticatedNetworkShares`; the installer will ask for the account password.

### macOS (Apple Silicon)

1. Extract `deep-search-macos-arm64.tar.gz`.
2. Open Terminal in the extracted directory.
3. Run:

```bash
sudo bash packaging/macos/launchd/install-service.sh "$USER" "./deep-search"
```

This installs the background LaunchDaemon and creates `/Applications/Deep-Search.app`.

These are alpha builds. Automated tests and cross-platform builds run before publishing. Hardware-specific desktop behavior and physical Windows/macOS service behavior still need real-machine validation.

## Docker service (headless, Linux x86_64)

Download these three assets from the same GitHub Release into one folder: `deep-search-docker-linux-x64.tar`, `compose.yaml`, and `Dockerfile`. The `.tar` is a Docker image archive: do not unpack it with tar.

1. Create a `scan` folder beside `compose.yaml`, or edit the bind mount to point to a mounted NAS/network share.
2. Load the image and start the service:

```bash
docker load -i deep-search-docker-linux-x64.tar
docker compose up -d
```

The Compose file publishes the service on `127.0.0.1:4765` only, stores the index in a persistent Docker volume, and mounts `./scan` read-only at `/scan`. In the desktop application's indexing-path field, enter `/scan`. For a NAS mount, replace `./scan:/scan:ro` with the host mount, for example `/mnt/nas:/scan:ro`.

Useful commands:

```bash
docker compose logs -f
docker compose down
```

The service has no authentication/TLS layer yet. Do not change the host binding to `0.0.0.0` or expose the container port to an untrusted network. Remote service access must wait for the secured transport implementation.

### Secure remote desktop access with an SSH tunnel (interim)

The native service protocol is raw TCP, not HTTPS. Until native authenticated TLS transport is implemented, an SSH local-forwarding tunnel is the supported way to connect a desktop client on another computer without exposing the service port. Keep the service bound to loopback on the server (the Docker Compose default already does this), and make sure SSH access is restricted to trusted accounts and preferably SSH keys.

On the **client computer**, first stop its own local Deep-Search service if it is already using port 4765. Then open a terminal and run:

```sh
ssh -N -o ExitOnForwardFailure=yes -o ServerAliveInterval=30 -o ServerAliveCountMax=3 -L 127.0.0.1:4765:127.0.0.1:4765 your-user@your-server
```

Replace `your-user@your-server` with the SSH account and hostname of the computer running Deep-Search. Keep this terminal open while using the desktop application; the application's normal `127.0.0.1:4765` connection is carried through the encrypted SSH tunnel to the server's loopback-only service. Verify the SSH host key on first connection. Do not open TCP port 4765 in the server firewall or change the service bind address to a public/LAN interface. Close the tunnel with Ctrl+C when finished.

This is a secure transport workaround, not native multi-user authentication: SSH controls who can create the tunnel, while Deep-Search itself does not yet identify individual remote users or authorize per-user indexing actions. The client currently uses the fixed local port 4765, so stop a competing local service before opening the tunnel.

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

The desktop alpha currently provides automatic connection to the local service on launch, indexing-path entry (native folder picker and drag-and-drop paths), query input, sorting, and a resizable results table. The results view supports adding/removing/showing/hiding columns, drag-and-drop or button-based reordering, adjusting each column's width, row-height control, content-based auto-fit across **all loaded results**, and persistent named view profiles. Default profiles include Default, Videos, Audio, Documents, Large files, and NAS. The Videos and Audio profiles expose optional media columns such as duration, resolution, codecs, frame rate, bit rate, sample rate, and channels. Profiles are stored locally in the user's application configuration directory. An Arabic-language toggle, contextual letter shaping, Unicode bidirectional rendering for labels and result text, and mirrored table-column order are implemented. Text-edit cursor/selection behavior and full RTL interaction still need real-use validation. With the platform service helper installed, the UI exposes runtime start/stop and a boot-startup toggle on Linux, macOS, and Windows, using the platform's authorization prompt. It also offers **Stop service when user logs out**, disabled by default; when enabled, the background service waits for the account's first login session and exits cleanly after logout. If the OS cannot reliably report the session state, the service stays running rather than stopping unexpectedly. The UI disables this option and explains the limitation when session detection is unavailable. The system tray is enabled by default and provides Open, Pause/Resume Indexing, Start/Stop Engine, Settings, and Exit actions. Closing the main window hides it in the tray by default; both tray visibility and close-to-tray behavior are configurable in application settings. Desktop notifications are configurable independently: indexing-failure notifications are enabled by default, while completion and service start/stop notifications are disabled by default. NAS disconnect/reconnect monitoring is available from Application settings: add each mounted network-share path on a separate line, then optionally enable notifications. The desktop checks configured locations every 15 seconds. Linux detects network mounts from `/proc/self/mountinfo`, macOS checks mounted SMB/NFS/AFP/WebDAV volumes, and Windows monitors UNC paths and mapped network drive letters when visible to the running process. The initial check establishes the baseline without sending a notification. Frozen leading columns are supported by the current table. Platform service controls are wired to the Linux systemd, macOS launchd, and Windows Task Scheduler helpers; they still need runtime verification on physical macOS and Windows machines. The app loads an installed system Arabic font as a fallback when available. Separate macOS launchd and Windows startup-task installers are included in their platform packages.

## Current capabilities

- Per-source search inclusion controls: disable a source from the result set without deleting its index or stopping its background indexing; the source can be re-enabled immediately.
- Every result carries its originating source path in a dedicated, sortable **Source** column; use `source:"/mnt/nas"` to filter by source.
- Unquoted search terms expand common English inflections (for example, `invoice`, `invoices`, and `invoicing`); quoted terms use case-insensitive exact word/phrase boundaries without inflection expansion.
- Advanced Search Builder supports multiple AND/OR conditions, enabling/disabling and reordering individual conditions, and saving named search layouts.
- Media filters support duration (seconds, minutes, hours, clock notation), resolution presets and exact dimensions, and frame rate (including fractional rates such as 29.97), for example `duration>=90s`, `resolution:4K`, `resolution:1920x1080`, and `fps:>=59.94`.
- Image metadata includes dimensions, detected format, and available EXIF camera make/model, capture time, and orientation; the Images view profile exposes these fields.
- Incremental file indexing with batched SQLite writes and parallel CPU-based document extraction.
- Persistent indexing roots and automatic filesystem change monitoring (create, modify, rename, delete) with debounced targeted refreshes; a 10-minute reconciliation pass catches missed events and changes on filesystems with limited notification support.
- SQLite-backed metadata storage and full-text content search (FTS5).
- Search by file name, path, content, extension, file category, and size range.
- Boolean search expressions with `AND`, `OR`, `NOT`, and parentheses.
- Search returns every matching file, with total-match counts and bounded batches for efficient transfer; pagination is an internal transport detail, not a cap on total results.
- Deterministic sorting by relevance, name, path, extension, size, or modification time.
- Extracts searchable text from common text/code files, PDF, RTF, DOCX, ODT, PPTX, XLSX, SubRip (SRT), WebVTT subtitles, and ZIP-family containers (ZIP, JAR, EPUB, CBZ, APK). Nested ZIP-family archives are traversed iteratively without a depth limit, with cumulative entry/expanded-byte/output safeguards against archive bombs. Password-protected ZIP entries are not yet extracted.
- Extracts optional audio/video technical metadata through `ffprobe`: duration, container, codecs, resolution, frame rate, bit rate, sample rate, and channel count. The Docker image includes FFmpeg/ffprobe; native desktop installs need `ffprobe` available on `PATH` or configured through `DEEP_SEARCH_FFPROBE`. Media probing is best-effort and never blocks normal file indexing.
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

Use the query language. Bare terms search file names, paths, and indexed contents; quote multi-word phrases. Field qualifiers remain available for precise searches:

```bash
deep-search query index.sqlite 'invoice'                 # expands invoice / invoices / invoicing
deep-search query index.sqlite '"invoice"'               # exact word/phrase; no inflection expansion
deep-search query index.sqlite 'name:"holiday video" AND type:video'
deep-search query index.sqlite 'name:"holiday video" AND (ext:mkv OR ext:mp4) AND size>=1GB'
deep-search query index.sqlite 'type:document AND path:"project archive"'
deep-search query index.sqlite 'content:"project roadmap" AND ext:pdf'
deep-search query index.sqlite 'NOT ext:tmp AND size<=500MB'
deep-search query index.sqlite 'type:video AND duration>=90s AND fps:>=60'
deep-search query index.sqlite 'source:"/mnt/nas" AND ext:mp4'
deep-search query index.sqlite 'resolution:4K AND fps:>=60'
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

You can override the loopback bind address explicitly:

```bash
deep-search serve index.sqlite 127.0.0.1:4765
```

Native service builds reject non-loopback bind addresses (including `0.0.0.0` and `[::]`) while the protocol has no authentication or encryption. The Docker image is compiled with a dedicated container-bridge feature so the service can bind inside its isolated container; the supplied Compose configuration still publishes the host port on `127.0.0.1` only. Do not change that host binding or expose the service to an untrusted network. Remote access will be enabled only after an authenticated and encrypted transport is implemented.

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

The product separates the background indexing/search engine from its user interface and is intended to support Local, NAS, and Remote operating modes. The current repository contains the Rust engine, local JSON service, desktop interface, filesystem change watcher, and platform service managers for Linux systemd, macOS launchd, and Windows Task Scheduler. Secure remote service transport and full Arabic RTL behavior remain under development.

Linux is the first-priority platform, followed by Windows and macOS.
