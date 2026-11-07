# FEATURES.md: UnduhKilat

Modular feature specification for the UnduhKilat portable Windows download manager. Every module below maps to one requirement group of the PRD and to the product decisions DEC-001 … DEC-007. Data schemas, wire endpoints and internal layering are **not** restated here: see `DATABASE.md` (state store), `API.md` (native messaging and any external interface) and `ARCHITECTURE.md` (engine architecture and runtime design).

## 1. Feature Scope

| Module | Core Features | MVP Status | References |
|---|---|---|---|
| M01 — Download Engine | Dynamic multi-connection segmentation, fallback to single connection, pause/resume/restart, automatic retry, file verification, HTTP/HTTPS/FTP, authentication, proxy, cookie attachment | Siap | FR-001–FR-017, DEC-001, DEC-005, DEC-006 |
| M02 — Queue & Scheduler | Manual per-item priority dispatch, FIFO queue ordering, concurrency enforcement, queue start/stop, time/window scheduler | Siap | FR-018–FR-024, DEC-002, DEC-004 |
| M03 — Bandwidth Control | Global throughput cap, per-download speed limit, lower-bound arbitration, live throttle application | Siap | FR-025–FR-028 |
| M04 — Categories & Destinations | File extension mapping, default fallback directory, category creation/renaming, per-download destination overrides | Siap | FR-029–FR-032 |
| M05 — Browser Integration & Media Capture | Manifest V3 extension (Chrome, Edge, Brave), HKCU/HKLM native messaging host, link/cookie capture, confirmation dialog, clipboard monitoring, hybrid media detection (sniffing + hook), batch grabber, depth-limited site walk | Siap | FR-033–FR-043, FR-045–FR-047, DEC-003, DEC-006, DEC-007 |
| M05.1 — Manifest Stream Assembly | Merging and muxing segmented adaptive streams (DASH/HLS) into a single media file from detected candidates | Ditunda | FR-044, DEC-003 |
| M06 — Desktop UI, Tray, and Notifications | egui/eframe transfer grid, add-URL batch input, settings editor, tray icon with background execution, Windows completion toasts, segment visualizer, diagnostic log viewer | Siap | FR-048–FR-057, DEC-004, DEC-005 |
| M07 — Durable State and Portability | Single-file SQLite transactional checkpoints, portable ZIP execution, signature-verified in-place bundle updates, administrative registry/reset utilities | Siap | FR-058–FR-062, DEC-005, DEC-007 |

---

## 2. M01 — Download Engine

### User Story
As an End User, I want to download files over HTTP, HTTPS, and FTP using dynamic multi-connection segmentation without artificial caps on concurrency or connections, so that I can saturate my network link and resume interrupted transfers without data loss.

### Business Rules
- Concurrency limit is fully user-configured with no upper ceiling; default concurrency is `TBD` (DEC-001, FR-001).
- Connection count per download is fully user-configured with no upper ceiling; default connection count is `8` (DEC-001, FR-002).
- If the remote server responds with `Accept-Ranges: bytes` and a valid `Content-Length`, the file is partitioned into byte ranges matching the configured connection count (FR-003).
- While downloading, if segment re-split conditions are met, the least-advanced segment splits dynamically; split threshold size, minimum segment size, and re-split frequency are `TBD` (FR-004).
- If the server rejects range requests or lacks range headers, the transfer falls back to a single HTTP connection, continuing the download while visibly flagging range support as disabled (FR-005).
- Pausing flushes in-memory chunks and commits highest contiguous byte offsets per segment to the SQLite state store (FR-006, reference `DATABASE.md`).
- Resuming issues range requests starting at the persisted byte offset; already committed bytes are never requested again (FR-007).
- Transient connection failures trigger automatic retries without user intervention; retry attempts count, backoff multiplier, and final failure timeout before marking `failed` are `TBD` (FR-008).
- Restarting an item resets all segment offsets to zero and truncates the local target file to zero bytes (FR-009).
- Hard crashes restore all in-progress items to `paused` state on the subsequent boot (FR-010).
- File size on disk upon completion must match expected `Content-Length`; size mismatch transitions download to `failed` state (FR-011).
- FTP transfers support resume via the `REST` command (FR-012).
- HTTP `3xx` redirects are followed automatically; the initial `Referer` is preserved across redirect hops (FR-013).
- Basic and Digest authentication credentials are saved per host and injected on `401 Unauthorized` challenges (FR-014).
- Proxy settings support direct connection, system proxy, or manual host/port with optional credentials (FR-015).
- File naming conflicts trigger the collision resolution policy (prompt vs. auto-rename suffix), which is `TBD` (FR-016).
- Captured browser session cookies, `User-Agent`, and `Referer` are attached directly to the outbound requests (FR-017, DEC-006).

### Acceptance Criteria
1. Given a URL returning `Accept-Ranges: bytes` and a user setting of 8 connections, when the download starts, 8 parallel HTTP requests with discrete byte ranges are established and written to their target offsets.
2. Given a running download, when the user clicks "Pause", all active Tokio network tasks abort within 1 second, and segment offsets in the SQLite state store equal the exact contiguous bytes flushed to disk.
3. Given a paused download with 45 MB of 100 MB completed, when the user resumes the download, the range requests begin at offset 45 MB, downloading exactly 55 MB without re-fetching byte 0 through 45 MB.
4. Given an active download, when the network interface disconnects, the engine retries the segment transfer according to the retry policy; once retries are exhausted, the item state becomes `failed`.
5. Given a server returning `200 OK` without `Accept-Ranges`, when the download starts, the engine opens exactly 1 connection, streams the file sequentially, and marks segmentation as unavailable in the UI.
6. Given a finished download with an expected size of 1,048,576 bytes, if the verified local file length is 1,048,000 bytes, the state machine marks the item as `failed` with reason code `ERR_SIZE_MISMATCH`.

### Edge Cases
- **Empty File (0 bytes)**: Server returns `Content-Length: 0`. The file is created locally, marked `completed` immediately without opening range connections.
- **Oversized Resource (> 4 GB on FAT32 / Large files)**: If the destination filesystem cannot allocate the disk space, the download aborts before write with error code [placeholder] (`ERR_DISK_FULL` or `ERR_FS_LIMIT`).
- **Unicode Filename**: Server sends UTF-8 or RFC 5987 encoded `Content-Disposition`. The filename is normalized and sanitized for Windows filesystem constraints (invalid characters replaced with `_`).
- **Duplicate Download**: Adding a URL that already exists in the active queue triggers [placeholder: prompt the user or raise `ERR_DUPLICATE_TASK`, selected by the undecided condition] without creating duplicate conflicting write handles.
- **Offline at Start**: Attempting to start a download while no network route exists immediately raises `ERR_NETWORK_UNREACHABLE`; [placeholder: whether the item remains `queued`/`failed` or moves to `running` with retry backoff, and the condition selecting the resulting state].

### Technical Contract
```rust
/// Extension point for download protocol handlers.
#[async_trait::async_trait]
pub trait ProtocolHandler: Send + Sync {
    async fn probe(&self, request: &DownloadRequest) -> Result<ResourceMetadata, ProtocolError>;
    async fn fetch_segment(
        &self,
        request: &DownloadRequest,
        range: ByteRange,
        sink: Box<dyn AsyncSegmentWriter>,
    ) -> Result<(), ProtocolError>;
}
```

---

## 3. M02 — Queue & Scheduler

### User Story
As an End User, I want to manage a prioritized download queue and set automated execution schedules, so that critical files download first and large transfers run unattended without exceeding my configured concurrency.

### Business Rules
- Queue dispatch priority is strictly two-tiered: manual per-item priority value first (higher integer value runs first), followed by FIFO queue ordering for items with equal priority (DEC-002, FR-018).
- Manual priority values can be updated at any time; changes take effect immediately on the subsequent dispatch tick without stopping running items (FR-019).
- Active downloads cannot exceed the global concurrency limit; items exceeding the limit remain in `queued` state (DEC-001, FR-020).
- Queue position reordering directly sets the FIFO sequence (FR-021).
- "Start All" transitions all eligible `paused` or `queued` items to active evaluation; "Stop All" pauses all currently active items (FR-022).
- Scheduled triggers define start and stop times, or daily execution windows; schedule configuration persists in SQLite (FR-023).
- Scheduler rules trigger actions regardless of whether the main window is visible or minimized to the system tray (DEC-004, FR-024).

### Acceptance Criteria
1. Given a concurrency limit of 2, with running items A and B, when Item C has priority 10 and Item D has priority 5, if Item A completes, Item C starts running immediately before Item D.
2. Given two queued items with default priority (0), Item X added at 10:00 AM and Item Y added at 10:01 AM, when an execution slot opens, Item X transitions to `running` before Item Y.
3. Given a scheduled start rule configured for 02:00:00 UTC, when the local system time reaches 02:00:00 UTC and the application is in the tray, the engine triggers "Start All" and eligible items begin downloading.
4. Given a user manual reorder dragging Item Z above Item W (both priority 0), when an execution slot frees, Item Z starts before Item W.

### Edge Cases
- **Empty Queue**: Triggering "Start All" or a scheduled start on an empty queue executes as a no-op without error.
- **Oversized Queue (> 10,000 items)**: The scheduler evaluates dispatch decisions using bounded SQL queries (`ORDER BY priority DESC, queue_index ASC LIMIT N`) without loading the entire queue into RAM.
- **Unicode Queue Names**: Queue schedule tags and user-defined labels support full UTF-8 encoding.
- **Duplicate Priority Values**: Items with identical priority resolve strictly by `queue_index` (FIFO).
- **Offline Trigger**: A scheduled start occurring while offline [placeholder: whether items remain `queued`/`failed` or move to `running` with retry backoffs until the network is restored].

---

## 4. M03 — Bandwidth Control

### User Story
As an End User, I want to set global and per-download download speed caps that adapt dynamically, so that download traffic does not saturate my connection during other tasks.

### Business Rules
- The End User can configure a global download speed limit in KB/s (FR-025).
- The End User can configure a per-download speed limit in KB/s on individual transfers (FR-026).
- If both limits are active, an item's effective bandwidth is `min(per_download_limit, allocated_global_slice)`; a value of 0 indicates unlimited (FR-027, FR-028).
- Throttling changes apply dynamically to active connections within the next token-bucket throttle tick without pausing or reconnecting (FR-028).
- Measurement tolerances, sampling interval, and token-bucket distribution slices are `TBD`.

### Acceptance Criteria
1. Given an active download with an unconstrained rate of 5,000 KB/s, when the user sets its speed limit to 500 KB/s, the measured throughput stabilizes at 500 KB/s (within `TBD` tolerance) at the next measurement cycle.
2. Given a global limit of 1,000 KB/s and two identical downloads running concurrently without individual limits, both transfers balance to approximately 500 KB/s each, with their aggregate not exceeding 1,000 KB/s.
3. Given an item with a per-download limit of 200 KB/s running under a global cap of 1,000 KB/s, the item transfers at no more than 200 KB/s.
4. Given a throttled download, when the user changes the limit to 0 (unlimited), the rate ramps up to network saturation without breaking active connections.

### Edge Cases
- **Zero / Negative Value**: Entering 0 removes the limit. Negative inputs are rejected by the UI validation layer with error code `ERR_INVALID_LIMIT`.
- **Oversized Limits (> 100 Gbps)**: Values larger than hardware interfaces operate as unlimited without integer overflow in the rate limiter logic.
- **Offline / Zero Throughput**: When throughput drops to 0 due to network stall, the token bucket stops accumulating excess tokens past its burst capacity `TBD`.
- **Dynamic Connection Fluctuation**: Adding connections to a segmented download distributes the byte budget across all active segment workers without aggregate limit leakage.

---

## 5. M04 — Categories & Destinations

### User Story
As an End User, I want files to automatically route to specific directories based on their file type or extension, so that my downloads stay organized without manual folder selection.

### Business Rules
- Built-in categories (e.g., Compressed, Documents, Music, Programs, Video) map default file extensions to preconfigured destination folders (FR-029).
- Resources with unknown extensions or unmapped `Content-Type` headers route to a default fallback folder (FR-030).
- Users can create, update, and delete categories and modify their target paths; changes apply only to future downloads (FR-031).
- Per-download category and directory overrides can be specified in the Add URL dialog and download properties (FR-032).

### Acceptance Criteria
1. Given a URL ending in `.zip`, when added without manual overrides, the download destination is automatically set to the configured `Compressed` directory.
2. Given a URL returning an unknown extension `.xyz`, the destination resolves to the configured fallback directory.
3. Given an existing category `Video` set to `D:\Videos`, when the user updates the path to `E:\Media\Video`, any subsequent `.mp4` download uses `E:\Media\Video`, while previously completed files remain at their existing paths.
4. Given a URL `https://example.com/file.iso`, when the user explicitly overrides the category to `Documents` in the confirmation dialog, the file is saved to the `Documents` directory instead of the ISO default.

### Edge Cases
- **Missing / Deleted Destination Directory**: If the target category folder does not exist on disk when the download begins, the engine creates the directory recursively; if path creation fails, the item errors with `ERR_PATH_TRAVERSAL_OR_INVALID`.
- **Unicode Paths**: Folder paths containing spaces, non-ASCII Unicode, or RTL characters are supported natively via Windows wide-character path APIs.
- **Path Length Exceeding MAX_PATH**: Paths longer than 260 characters use Windows extended-length path prefix (`\\?\`) to avoid `ERR_PATH_TOO_LONG`.
- **File Extension Ambiguity**: Files without extensions in the URL check the HTTP response `Content-Type` header to infer extension and category before writing to disk.

---

## 6. M05 — Browser Integration & Media Capture

### User Story
As an End User, I want my Chromium browser (Chrome, Edge, Brave) to hand downloads and detected streaming media directly to UnduhKilat with session cookies attached, so that I can capture downloads seamlessly without installation wizards.

### Business Rules
- Integration is supported exclusively on Chrome, Edge, and Brave via a Manifest V3 extension (DEC-006, FR-033).
- On first launch, the portable binary registers the native messaging host manifest in the Windows Registry under `HKCU\Software\Google\Chrome\NativeMessagingHosts`, `HKCU\Software\Microsoft\Edge\NativeMessagingHosts`, and `HKCU\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts` (DEC-006, DEC-007, FR-034; reference `API.md`).
- Administrators can invoke an elevated switch to register the native messaging host under `HKLM` for all system users (FR-035).
- Browser navigation to downloadable files intercepts the link, passing target URL, `Referer`, `User-Agent`, and session cookies to the desktop app (DEC-006, FR-036, FR-037).
- Download state transitions are reported back to the extension via the native messaging pipe (FR-038; reference `API.md`).
- Intercepted URLs trigger the Add URL Confirmation Dialog; no download starts automatically without user confirmation (FR-039).
- Clipboard monitoring polls the Windows clipboard for valid URLs; if enabled, copying a URL opens the confirmation dialog (FR-040).
- Web media detection uses a hybrid mechanism: combining network request sniffing and page-level DOM hook events from the extension (DEC-003, FR-041).
- Detected media candidates are deduplicated by resolved canonical URL, displaying URL, media format, and discovered size (DEC-003, FR-042).
- The extension displays a floating overlay panel over the active browser tab listing detected media candidates (DEC-003, FR-043).
- Manifest-based streams (e.g., DASH, HLS) appear in the detected media candidate list; full single-file capture of manifest streams is deferred (`Ditunda`) (FR-044).
- Batch downloads accept wildcard URL patterns to enqueue multiple matching links after user preview (FR-045).
- Site Grabber performs depth-limited crawls using include/exclude regex filters with user-configurable max depth (default `TBD`) (FR-046).
- All batch and grabber items enter the queue as standard items obeying concurrency, bandwidth, and priority rules (FR-047).

### Acceptance Criteria
1. Given a portable application extracted to a folder, when executed for the first time, registry keys pointing to `native_host.exe` exist under `HKCU` for Chrome, Edge, and Brave.
2. Given an active browser tab, when a link with an executable extension (`.exe`) is clicked, the browser download is intercepted, and the UnduhKilat confirmation dialog opens with matching URL, Referer, and Cookies populated.
3. Given an authenticated website session, when a download is captured, the engine attaches captured cookies to its request; the transfer completes with `200 OK` rather than redirecting to a login page.
4. Given a page playing an HTML5 video stream, the floating media panel appears over the page, displaying the direct video URL and file type captured via the hybrid sniffer.
5. Given a user copying `https://example.com/archive.tar.gz` to the clipboard with clipboard monitoring enabled, the Add URL Confirmation Dialog pops up automatically within `TBD` milliseconds.
6. Given a batch pattern `https://example.com/files/img[01-03].png`, the preview screen presents exactly 3 candidate URLs; upon confirmation, 3 discrete queue items are registered.

### Edge Cases
- **Extension Disconnected / Host Process Killed**: Native host failure logs an IPC disconnection event and falls back to manual URL copying and clipboard monitoring with error code `ERR_NATIVE_IPC_DISCONNECTED`.
- **Malformed URL from Extension / Clipboard**: Text on clipboard or received from browser containing invalid URIs is discarded silently without opening the confirmation dialog.
- **Unicode Query Parameters & Internationalized Domain Names (IDN)**: URLs containing non-ASCII hostnames are converted to Punycode; percent-encoded Unicode paths are preserved.
- **Manifest Stream Selection (HLS/DASH)**: Selecting an `.m3u8` or `.mpd` resource warns the user that manifest merging is postponed; [placeholder: save the manifest file directly or raise `ERR_MANIFEST_STREAM_UNSUPPORTED`, selected by the undecided condition].
- **Recursive / Infinite Site Crawling Loop**: Site Grabber tracks visited canonical URLs; visits to already traversed endpoints or URLs exceeding `max_depth` are ignored.

### Technical Contract
```rust
/// IPC extension point for incoming browser and clipboard capture events.
pub trait CaptureChannel: Send + Sync {
    fn on_link_intercepted(&self, payload: BrowserInterceptPayload) -> Result<(), IpcError>;
    fn on_media_detected(&self, candidates: Vec<MediaCandidate>) -> Result<(), IpcError>;
    fn on_clipboard_url(&self, raw_url: &str) -> Result<(), IpcError>;
}
```

---

## 7. M06 — Desktop UI, Tray, and Notifications

### User Story
As an End User, I want a lightweight Windows desktop interface with an active transfer list, tray persistence, and Windows completion notifications, so that I can monitor and control downloads without disrupting my workflow.

### Business Rules
- Built using Rust and egui/eframe on the `wgpu` rendering backend (DEC-005).
- Transfer list displays name, file size, download state, progress bar, instant transfer speed, estimated time remaining (ETA), category, destination path, and priority (FR-048).
- The Add URL dialog supports single or multi-line batch URL input, allowing batch overrides for credentials, priority, connections, and destination (FR-049).
- Settings interface exposes engine concurrency, per-download connections, rate limits, proxy, categories, scheduler, clipboard toggle, notifications, and logging levels (FR-050).
- The Windows system tray icon remains active throughout the application lifecycle, exposing a context menu to show/hide window, start queue, stop queue, and exit (DEC-004, FR-051).
- Closing the main window (clicking the window `X` button) hides the window to the tray; downloads continue uninterrupted in the background (DEC-004, FR-052).
- The application only terminates through the explicit "Exit" action in the tray or menu; exit triggers a flush checkpoint of all active download offsets before shutting down (FR-053).
- Windows Toast notifications fire when a download completes, even if the window is hidden to the tray, unless disabled in settings (DEC-004, FR-054).
- Item details inspect view renders individual segment ranges, checkpointed offsets, and live connection counts (FR-055).
- Diagnostics panel displays real-time log tails, per-item last error, and live connection metrics, with an export function to save logs locally (FR-056).
- Logs rotate locally based on file size and retention policies (specific retention window and file size are `TBD`); no telemetry or remote logs are transmitted (FR-057).

### Acceptance Criteria
1. Given 3 active downloads, when the user clicks the window close button (`X`), the window closes, the tray icon remains present, and network transfer rates remain unchanged (DEC-004).
2. Given a running download, when it completes while the window is hidden, a Windows Toast notification appears stating that the file download has completed (DEC-004).
3. Given the application running in the tray, when the user right-clicks the tray icon and selects "Exit", all active downloads are paused, segment states are committed to SQLite, and the process exits within `TBD` seconds.
4. Given the Diagnostics panel, clicking "Export Logs" prompts for a destination path and writes the rotated log files to that directory.

### Edge Cases
- **Display Scaling / High DPI**: egui interface scales according to Windows display scaling factors (100%, 125%, 150%, 200%) without clipped text.
- **Empty Download List**: UI displays an empty placeholder state ("No downloads in queue") with an accessible "Add URL" button.
- **Excessive Log Output**: High-frequency connection debugging truncates memory-buffered log tails to the last `TBD` lines to prevent UI rendering lag.
- **Toast Service Unavailable**: If Windows Push Notification Platform (WPNP) is disabled or blocked by Focus Assist, notification failure logs a warning and does not crash the application.
- **Non-ASCII Paths in File Picker**: File and directory picker dialogs support paths containing Unicode symbols and long Windows path prefixes.

---

## 8. M07 — Durable State and Portability

### User Story
As an End User or Administrator, I want to run the application from an unzipped folder with crash-resilient transactional storage, so that I can use the tool without Windows administrative installation and survive system crashes without losing progress.

### Business Rules
- All state (downloads, segments, priorities, settings, schedules, credentials) is committed to a single local SQLite database file in transactional checkpoints (FR-058; reference `DATABASE.md`).
- Operates entirely as a portable executable from an extracted ZIP directory without an MSI or installer wizard; no elevation required for standard operation (DEC-007, FR-059).
- During standard non-elevated operation, the application writes strictly to its executable folder, its local AppData state store, and `HKCU` registry entries (FR-059); elevated Administrator actions are the only exceptions and may additionally write `HKLM` native-messaging registration entries and the machine-wide binary directory (FR-035, FR-061).
- Updates are distributed as signed portable archive packages; updates verify cryptographic signatures before replacing local binaries in-place (FR-060).
- Administrators can register the native messaging host under `HKLM` or upgrade the binary directory machine-wide for multi-user setups (FR-035, FR-061).
- Administrators can trigger an application data reset, which purges the SQLite database and log files upon explicit user confirmation, leaving completed downloaded files intact on disk (FR-062).

### Acceptance Criteria
1. Given an active multi-segmented download, when the `UnduhKilat.exe` process is abruptly killed using Windows Task Manager (`TerminateProcess`), relaunching the executable restores the download to a resumable `paused` state with its last checkpointed segment offsets intact (FR-010, FR-058).
2. Given a new portable ZIP release package, when the application runs an update check and downloads the archive, it verifies the cryptographic signature; if the signature check fails, the update aborts, no files are replaced, and `ERR_PACKAGE_SIGNATURE_INVALID` is surfaced.
3. Given an existing portable folder moved to a different drive letter or USB directory, running the binary updates the `HKCU` registry native messaging path to the current executable path without requiring administrative privileges (DEC-007, FR-034).
4. Given an Administrator invoking the data directory reset utility, after confirming the prompt, the SQLite state database and rotated log files are removed, while files previously downloaded to user category folders remain unmodified on disk (FR-062).

### Edge Cases
- **Database File Lock / SQLite Busy**: If another process or antivirus scanner locks the SQLite file, operations retry up to a timeout of `TBD` milliseconds; persistent locks return `ERR_DB_LOCKED`.
- **Corrupt State Database**: If the SQLite file header is corrupted due to unbuffered drive failure, the application renames the corrupted file with an `.err` suffix, initializes a clean database, and logs `ERR_DB_CORRUPTED`.
- **Read-Only Media (Running directly from CD/DVD/Locked USB)**: If the portable executable directory or AppData path is read-only, the engine surfaces `ERR_STORAGE_READ_ONLY` and disables checkpoint writes to prevent crashes.
- **Disk Full during Checkpoint**: If a disk write fails during state checkpointing, the transaction rolls back, preserving the last valid checkpoint and flagging `ERR_DISK_FULL`.
- **Unsigned Update Bundle**: Providing an unsigned update archive to the in-place updater is rejected immediately with error code `ERR_UPDATE_UNSIGNED`.

### Technical Contract
```rust
/// Contract for state persistence and transactional checkpointing.
#[async_trait::async_trait]
pub trait StateStore: Send + Sync {
    async fn checkpoint_download(&self, snapshot: &DownloadSnapshot) -> Result<(), StateStoreError>;
    async fn recover_interrupted_downloads(&self) -> Result<Vec<RecoveredDownload>, StateStoreError>;
    async fn apply_migration(&self) -> Result<(), StateStoreError>;
}
```

---

## 9. Out of Feature Scope

The following items are explicitly excluded from the MVP feature scope based on product decisions and PRD constraints:

1. **Manifest Stream Dynamic Assembly / Muxing (FR-044, DEC-003)**: Capturing and listing manifest-based stream candidate URLs (DASH `.mpd`, HLS `.m3u8`) is supported, but automatic download, sequential segment fetching, audio/video stream muxing, and single-file assembly are postponed (`Ditunda`).
2. **Installer-Based / MSI Distribution Wizard (DEC-007, FR-059)**: The product ships strictly as a portable ZIP archive. Setup wizards, MSI packages, and non-portable desktop installations are excluded.
3. **Non-Chromium Browser Extensions (DEC-006, FR-033)**: Firefox (Gecko), Safari (WebKit), Opera, and Vivaldi extension builds are excluded; only Google Chrome, Microsoft Edge, and Brave are supported.
4. **Non-Windows / 32-bit / ARM Architectures (DEC-005)**: Windows 7, Windows 8, Windows 8.1, 32-bit x86 builds, and Windows on ARM64 are out of scope.
5. **Application-Imposed Concurrency Limits (DEC-001, FR-001, FR-002)**: Hardcoded ceilings on simultaneous downloads and per-download parallel connections are rejected; values remain entirely user-configured.
6. **Automatic Size- or Speed-Based Dispatch Algorithms (DEC-002, FR-018)**: Dispatch ordering based on smallest-first, largest-first, or dynamic speed balancing is excluded in favor of manual priority plus FIFO.
7. **Cloud Synchronization and Remote Telemetry (FR-057)**: User accounts, cloud state sync across machines, centralized logging, and usage telemetry are strictly out of scope.
8. **Exit-on-Close Window Behavior (DEC-004, FR-052)**: Terminating the application by clicking the main window's close button is excluded; the window unconditionally hides to the Windows system tray while transfers proceed.