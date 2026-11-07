# USER_FLOW.md: UnduhKilat

## 1. Conventions & Actors / Konvensi & Aktor

### 1.1 Document Scope & Conventions / Ruang Lingkup Dokumen & Konvensi
This document specifies the critical interactive and automated end-to-end user flows for **UnduhKilat**, a crash-durable, high-throughput desktop download manager for Windows built with Rust, egui/eframe, and Tokio. It covers initial portable deployment and browser pairing, link and media interception, queue management, segmented execution, pause/resume/crash-recovery lifecycles, and configuration management.

#### Interface & Data Conventions
- **Canonical Status Identifiers**: Download states strictly follow DATABASE.md: `queued`, `running`, `paused`, `completed`, `failed`, `cancelled`. Segment states strictly follow `pending`, `downloading`, `completed`.
- **Database Tables**: Persisted SQLite tables are written in `snake_case`: `downloads`, `segments`, `categories`, `queue_items`, `credentials`, `schedules`, `settings`.
- **IPC / Native Messaging Commands**: Two-way native messaging commands between the Chromium extension and the Rust desktop host are written in `camelCase` or `UPPER_SNAKE` message envelopes (e.g., `interceptDownload`, `mediaCandidateDetected`, `queryDownloadState`, `pushStatusUpdate`).
- **Error Codes**: All system errors, protocol errors, and validation failures are represented in canonical `UPPER_SNAKE` format (e.g., `ERR_RANGE_NOT_SUPPORTED`, `ERR_FILE_IO_FAILURE`).
- **Unspecified Policies**: In accordance with project instructions, undefined numeric thresholds, intervals, retries, retention limits, and fallback strategies are designated strictly as `TBD`.

### 1.2 Actors & MVP Limits

| Actor | Description | MVP limits |
|---|---|---|
| **End User** | The primary local Windows operator running the portable desktop application (DEC-007). Directly initiates downloads, reorders the queue, overrides concurrency, and updates configurations. | Single-session local desktop operator; runs un-elevated on Windows 10/11 x64 (DEC-005). |
| **Browser Extension** | Manifest V3 background worker and content script running inside Chromium browsers (Chrome, Edge, Brave; DEC-006). Captures downloads and sniffs page media. | Restricted to Chromium family; communicates exclusively via standard I/O Native Messaging through the registered host binary. |
| **Download Engine / Scheduler (System)** | Tokio-driven multi-threaded background async runtime executing parallel connection pooling, dynamic range segmenting, bandwidth throttling, and SQLite transactional checkpoints. | Bounded only by host OS resources; enforces user-defined limits with no application ceiling (DEC-001). |
| **Administrator** | Privileged Windows user capable of machine-wide native host registration, shared bundle upgrades, or clearing data directories. | Required only for `HKLM` machine-wide operations and global directory resets; normal usage remains un-elevated. |

---

## 2. User Flows / Alur Pengguna

### Flow 1 — First-Run, Environment Initialization & Browser Pairing
Covers portable extraction, first-run initialization, per-user native messaging host registration under `HKCU`, and loading the Chromium extension (Chrome, Edge, Brave).

| No | Actor | Action/Step | System Response | Alternative/Error Path |
|---|---|---|---|---|
| 1 | End User | Extracts portable ZIP to an unprivileged directory (e.g., `C:\Tools\UnduhKilat\`) and executes `UnduhKilat.exe` (DEC-005, DEC-007). | System boots Tokio runtime, checks AppData directory, initializes single-file SQLite database (`unduhkilat.db`), and verifies schema integrity (FR-058, FR-059). | **ERR_STORAGE_INIT_FAILED**: If database cannot be written or opened, UI displays a modal: "Database Initialization Failure. Verify write permissions for application storage." with "Retry" and "Quit" buttons. The app halts only when "Quit" is chosen. |
| 2 | System | Checks Windows Registry for Native Messaging Host registration under `HKCU\Software\Google\Chrome\NativeMessagingHosts\`, Edge, and Brave keys (DEC-006, FR-034). | If keys are absent or point to a broken path, writes JSON manifest pointing to `unduhkilat-host.exe` and registers keys under `HKCU` without requiring administrative elevation (FR-034, FR-059). | **ERR_REGISTRY_WRITE_FAILED**: If registry access is denied, UI opens with a yellow warning badge in Settings: "Browser integration host could not be registered automatically." |
| 3 | System | Launches egui desktop GUI using the wgpu renderer, initializes tray icon, and enters the main event loop (DEC-004, TC-002). | Window displays empty download list (`downloads`), active toolbar, and status bar showing "Ready". | None. |
| 4 | End User | Opens Chromium browser (Chrome, Edge, or Brave), navigates to `chrome://extensions`, enables "Developer mode", and loads the unpacked extension from the bundle's `extension/` directory (FR-033). | Extension background service worker starts, probes the registered Native Messaging Host via `chrome.runtime.connectNative("com.unduhkilat.host")`. | **ERR_NATIVE_HOST_NOT_FOUND**: Extension icon turns grey with badge "!". Tooltip indicates: "UnduhKilat Host not found. Ensure application has run at least once." |
| 5 | Browser Extension | Sends ping handshake to desktop application via standard I/O native messaging. | Desktop host confirms connection; extension updates its icon badge to active status. Desktop settings panel updates "Browser Integration" status to "Connected (Chromium)" (FR-050). | **ERR_NATIVE_HOST_TIMEOUT**: If host binary fails to respond within `TBD` ms, extension marks integration as "Disconnected" and retries with backoff `TBD`. |

- **Trigger**: End User launches the portable executable for the very first time on a machine.
- **Pre-conditions**: Windows 10 or 11 x64 environment (DEC-005); portable ZIP unpacked to a writable storage volume; supported Chromium browser installed (DEC-006).
- **Post-conditions**: `unduhkilat.db` schema initialized; `HKCU` registry entries created; native messaging host operational; egui shell idling with tray icon visible.

---

### Flow 2 — Link Interception, Hybrid Media Sniffing & Download Ingestion
Covers the core capture paths: automatic browser link interception, hybrid video/audio detection, clipboard URL monitoring, and manual URL batch input.

| No | Actor | Action/Step | System Response | Alternative/Error Path |
|---|---|---|---|---|
| 1a | End User / Browser | **Path A (Link Click)**: End User clicks a direct file link (e.g., `.iso`, `.zip`, `.exe`) in a supported browser (DEC-006, FR-036). | Extension intercepts navigation, extracts URL, cookies, `Referer`, and `User-Agent` (FR-017, FR-037), and forwards payload to desktop app via Native Messaging command `interceptDownload`. | **ERR_PAYLOAD_CORRUPT**: If forwarded data is malformed, desktop app logs error and ignores request. Browser falls back to native download if configured `TBD`. |
| 1b | End User / Browser Extension | **Path B (Hybrid Media Sniffing)**: End User plays video/audio on a webpage. Content script hooks `<video>/<audio>` tags while background worker sniffs network media requests (DEC-003, FR-041). | Extension merges, deduplicates by resolved URL (FR-042), and renders a floating overlay panel listing available media streams, resolutions, and discovered file sizes (FR-043). End User clicks "Download" on an entry. | **ERR_MEDIA_PARSE_FAILED**: If media is an unparsed manifest (DASH/HLS), panel displays the entry labelled [placeholder: label text for unparsed manifest candidates] per FR-044, and [placeholder: whether clicking the entry shows a warning, and the warning text]. |
| 1c | End User | **Path C (Clipboard Monitoring)**: End User copies a downloadable URL to the Windows clipboard with clipboard monitoring enabled (FR-040). | Desktop app detects valid URL via clipboard polling/Win32 hook and brings download confirmation dialog to foreground. | If clipboard contains non-URL or invalid protocol (e.g. `file://`), clipboard listener silently discards it. |
| 1d | End User | **Path D (Manual / Batch Add)**: End User clicks "Add URL" button in egui toolbar, pastes one or more URLs into the text area, optionally providing host credentials (FR-049). | Desktop parses lines, validates URL format (HTTP/HTTPS/FTP), and pre-populates the ingestion confirmation modal. | **ERR_INVALID_URL_SCHEME**: If one or more lines are malformed, inline validation error flags the problematic line in red: "Invalid protocol or malformed URL." |
| 2 | System | Checks file extension against `categories` table and matches default download destination directory (FR-029, FR-030). Evaluates pre-existing files on disk. | Ingestion confirmation modal opens (FR-039), displaying URL, proposed filename, known size (`TBD` if unqueried), auto-assigned category, target path, connection count, and priority. | **ERR_PATH_UNWRITABLE**: If assigned directory is non-existent and cannot be created, falls back to fallback category folder and displays warning text. |
| 3 | End User | Reviews fields, optionally modifies filename, destination path, connection count, or priority, and clicks "Download Now" or "Download Later" (FR-032, FR-039). | If "Download Now": commits item into `downloads` and `queue_items` with initial `state = queued` (or `state = running` if concurrency permits) (FR-020). If "Download Later": commits with `state = paused`. | If End User clicks "Cancel", modal closes, no record is written to SQLite, and no HTTP requests are sent (FR-039). If triggered via extension, extension receives `CANCELLED` response. |
| 4 | System | Pushes creation confirmation back to extension if download was browser-initiated (FR-038). | Extension updates local task table to reflect the new state. In egui main list, the download appears as an active row displaying file details and progress bar (FR-048). | None. |

- **Trigger**: End User interacts with a link in a browser, selects an item from the floating video sniffer panel, copies a URL to the clipboard, or submits manual URLs via the Add URL dialog.
- **Pre-conditions**: Desktop app is running (or native host triggers app launch); extension is installed and active; destination drive has free space.
- **Post-conditions**: A new record is inserted into `downloads` and `queue_items` with complete metadata, cookies, and headers; no unconfirmed downloads are started.

---

### Flow 3 — Segmented Download Execution, Dynamic Splitting & Queue Dispatch
Covers the core downloading lifecycle: range detection, dynamic multi-connection segmentation without upper limits, dynamic re-splitting, and priority-then-FIFO queue scheduling.

| No | Actor | Action/Step | System Response | Alternative/Error Path |
|---|---|---|---|---|
| 1 | Download Engine | Queue dispatcher selects `downloads` rows with `state = queued` joined to their `queue_items` record and orders by `queue_items.priority DESC, queue_items.queue_position ASC` (DEC-002, FR-018). | If active running downloads count is less than user-configured concurrency limit (DEC-001, FR-001, FR-020), transitions highest-ranked candidate to `state = running`. | If running count equals concurrency limit, all remaining items remain in `state = queued` until a slot frees up (FR-020). |
| 2 | Download Engine | Worker task sends HTTP `HEAD` / initial `GET` request using `reqwest` + `rustls` (including attached cookies, Referer, and User-Agent; FR-017). | Inspects response headers. Records `Content-Length` and checks for `Accept-Ranges: bytes`. Checks if target file already exists at destination (FR-016). | **ERR_NAME_COLLISION**: If file already exists, applies conflict resolution policy `TBD` (auto-renames or prompts; FR-016), updating `target_path` in SQLite. |
| 3 | Download Engine | Evaluates range support. | **Case A (Range Supported)**: Splits file into $N$ contiguous byte-range segments matching user's configured connection count (DEC-001, FR-002, FR-003). Inserts $N$ rows into `segments` table with `state = pending`.<br>**Case B (Range Unsupported/Rejected)**: Falls back to single-connection transfer (FR-005). Sets item `segmentation_supported = false`. | **ERR_HTTP_AUTH_REQUIRED**: If response is `401 Unauthorized` or `407 Proxy Auth Required`, checks `credentials` table for host (FR-014, FR-015). If missing or rejected, moves to `state = failed` with error code `ERR_HTTP_AUTH_REQUIRED`. |
| 4 | Download Engine | Spawns Tokio tasks for each segment. Tasks issue `GET` with `Range: bytes=start-end` (FR-003). Allocates/prepares target file on disk. | Received chunks are written directly to their respective byte offsets. Bandwidth limiter throttles aggregate chunk stream to respect global and per-download speed limits (FR-025, FR-026, FR-027). | **ERR_RANGE_NOT_SATISFIABLE**: If server returns `416 Range Not Satisfiable`, aborts multi-connection mode and falls back to single-connection re-fetch (FR-005). |
| 5 | Download Engine | Periodic background timer commits segment progress (`highest_contiguous_offset`) to SQLite transactionally at interval `TBD` (FR-058, NFR-004). | Main egui window renders live updates: downloaded bytes, percentage, current speed, time remaining, and segment bar visualization in details panel (FR-048, FR-055, NFR-006). | **ERR_STORAGE_FULL**: If disk writes fail (e.g. disk full), the download fails automatically, marks `state = failed`, and logs error in diagnostics panel; the warning modal offers to resume after space is freed. |
| 6 | Download Engine | Evaluates dynamic re-split policy: if a segment finishes early and remaining segments exceed `TBD` minimum size, splits the least-advanced segment into sub-ranges (FR-004). | Engine spawns an additional connection task for the un-fetched half of the range without interrupting other active connections (FR-004). Total active connections adjust up to configured preference. | If server rejects additional concurrent connections (e.g. `429 Too Many Requests` or connection drop), engine consolidates remaining range back to the primary worker and continues. |
| 7 | Download Engine | All segments complete downloading their allocated byte ranges. | Engine flushes buffers, closes file descriptors, verifies total file size against expected `Content-Length` / range sum (FR-011). Commits `state = completed` to `downloads`. | **ERR_CHECKSUM_SIZE_MISMATCH**: If verified file size does not match expected size, moves item to `state = failed` with `ERR_CHECKSUM_SIZE_MISMATCH`. No completion toast is shown (FR-011, FR-054). |
| 8 | System | Dispatches completion events: raises Windows toast notification if enabled (DEC-004, FR-054), updates extension via native host (FR-038). | Queue dispatcher immediately triggers Step 1 to claim the newly opened concurrency slot for the next queued item (FR-020). | None. |

- **Trigger**: Download item enters `state = queued` and a concurrency slot becomes available, or scheduler starts the queue at a configured time (FR-023).
- **Pre-conditions**: Valid URL; network connection available; destination folder accessible.
- **Post-conditions**: Complete, byte-identical file exists on disk (NFR-005); database state records `completed`; next eligible queue item is dispatched.

---

### Flow 4 — Interruption, Pause/Resume, Crash Recovery & Background Tray Continuity
Covers manual pause/resume, connection loss recovery, hard application crashes/power failure recovery, and close-to-tray background operation.

| No | Actor | Action/Step | System Response | Alternative/Error Path |
|---|---|---|---|---|
| 1a | End User | **Path A (Manual Pause)**: Clicks "Pause" button on an active download row in egui GUI (FR-006). | Download engine signals cancellation tokens to Tokio segment tasks. Tasks flush received buffers and highest contiguous offsets to `segments` table in SQLite. Item transitions to `state = paused` (FR-006). | If pause is clicked repeatedly, redundant events are ignored; no duplicate database commits occur. |
| 1b | System | **Path B (Connection Drop)**: An active socket abruptly drops or times out mid-transfer (FR-008). | Engine enters automatic retry loop. Suspends segment worker, waits according to backoff policy `TBD`, and issues a new `Range` request from the last checkpointed byte offset (FR-007, FR-008). | **ERR_CONNECTION_DROPPED**: If retries exceed limit `TBD`, engine transitions item to `state = failed` with `ERR_NETWORK_TIMEOUT` and surfaces failure in the diagnostics panel (FR-056). |
| 1c | External / OS | **Path C (Process Kill / Crash / Power Outage)**: Windows process is killed via Task Manager (`TerminateProcess`) or machine loses power during transfer (DEC-007, FR-010, NFR-004). | Process terminates abruptly. Because SQLite state was committed transactionally during transfers (FR-058), segment progress up to the last checkpoint is durable on disk. | At most one checkpoint interval `TBD` of in-flight bytes is lost; no corrupted database state (FR-010). |
| 2 | End User | Relaunches `UnduhKilat.exe` following a crash or hard termination (FR-010). | Startup scanner inspects `downloads` table. Any download found in `state = running` is automatically normalized to `state = paused` with its segment offsets intact (FR-010). | If partial file on disk is missing or was deleted externally, item transitions to `state = failed` with `ERR_FILE_NOT_FOUND`. |
| 3 | End User | Clicks "Resume" on the paused item, or clicks "Resume All" in the toolbar (FR-007, FR-010, FR-022). | Engine reads persisted offsets from `segments`. Issues HTTP range requests starting strictly from `highest_contiguous_offset` (0 bytes re-downloaded; KPI-006). Transitions item to `state = running`. | **ERR_RANGE_NOT_SUPPORTED**: If server rejects resumption (e.g., dynamic link expired or no ranges), UI prompts End User: "Server cannot resume download. Restart from zero?" If confirmed, truncates file and restarts (FR-009). |
| 4 | End User | Clicks the main window close button `[X]` while downloads are actively running (DEC-004, FR-052). | Application window hides immediately to the Windows taskbar tray (DEC-004, FR-052). Transfers continue running at full speed on background Tokio runtime. Tray tooltip shows aggregate download rate and active count. | None. |
| 5 | System | An active download completes while the main window is hidden in the tray (DEC-004, FR-054). | Desktop engine fires a native Windows toast notification: "Download complete: [filename]" (FR-054). Active tray icon remains present. | If notifications are disabled in Settings, toast is suppressed (FR-054). |
| 6 | End User | Right-clicks tray icon and selects "Show" to restore window, or selects "Exit" to shut down (FR-051, FR-052). | If "Show": egui window un-hides in place with live progress (FR-048). If "Exit": engine shows confirmation warning that transfers will stop (FR-053), checkpoints all running downloads to `state = paused`, flushes SQLite, removes tray icon, and cleanly terminates process. | If End User cancels the exit prompt, application continues running in current state (FR-053). |

- **Trigger**: End User pauses/resumes a download, network connectivity is lost, process is killed/restarted, or window is closed to tray.
- **Pre-conditions**: Downloads exist in database; partial file data matches checkpoint records.
- **Post-conditions**: No byte duplication upon resume; zero database corruption after abnormal termination; background execution maintained when hidden to tray.

---

### Flow 5 — Data, Category & Lifecycle Management
Covers category customization, live speed limit throttling, batch site grabbing, diagnostic log export, signed in-place updates, and administrator data reset.

| No | Actor | Action/Step | System Response | Alternative/Error Path |
|---|---|---|---|---|
| 1a | End User | **Path A (Category Management)**: Opens Settings > Categories. Creates a new category (e.g. "Archives" -> `D:\Archives\`, extensions: `.zip`, `.rar`, `.7z`) (FR-029, FR-031). | SQLite updates `categories` table. Subsequent downloads matching these extensions automatically route to `D:\Archives\` (FR-029). Existing files remain untouched (FR-031). | **ERR_INVALID_PATH**: If specified directory string is invalid under Windows path rules, UI highlights input with error: "Invalid Windows folder path." |
| 1b | End User | **Path B (Live Bandwidth Throttling)**: Adjusts global download speed cap from 0 (unlimited) to 2048 KB/s while transfers are running (FR-025, FR-028). | Speed throttle controller applies rate-limiting tokens across all active Tokio socket reader tasks within the next throttle interval without interrupting or pausing transfers (FR-028). | If user enters 0, throttling is immediately lifted (FR-028). |
| 1c | End User | **Path C (Site Grabber / Batch Crawl)**: Opens Tools > Site Grabber. Enters base URL, wildcard filters (`*.pdf`), and crawl depth limit `TBD` (FR-045, FR-046). | Engine performs depth-limited crawl, parses HTML links matching inclusion/exclusion patterns, and displays a multi-item preview list (FR-045, FR-046). End User confirms selection; system enqueues all checked links as standard queue items (FR-047). | **ERR_CRAWL_FAILED**: If host refuses crawl or credentials fail, dialog displays "Crawl halted: [error message]" and lists all links discovered prior to failure. |
| 1d | End User | **Path D (Diagnostics & Log Export)**: Navigates to Diagnostics panel. Views real-time scrolling logs, per-download error logs, and active connection counts (FR-056). Clicks "Export Logs". | System prompts for destination folder. Copies rotating local log files into target directory and displays confirmation message (FR-056, FR-057). | **ERR_EXPORT_FAILED**: If target folder is write-protected, displays: "Could not export logs: access denied." Existing logs remain intact (FR-056). |
| 1e | End User | **Path E (Signed In-Place Update)**: Receives update notification or imports an update package archive (FR-060, TC-007). | System reads package, verifies cryptographic signature against embedded public key (FR-060). Upon validation, replaces bundle binaries in place and schedules restart (DEC-007, FR-060). | **ERR_PACKAGE_SIGNATURE_INVALID**: If package signature is missing or corrupted, update is rejected immediately. No files are modified; UI displays: "Update aborted: Invalid digital signature." (FR-060). |
| 1f | Administrator | **Path F (Admin Clean Reset)**: Launches reset utility with elevated privileges to wipe application state (FR-062). | Displays high-severity prompt: "This will permanently delete download history, settings, and logs. Downloaded files will not be touched." Upon confirmation, deletes `unduhkilat.db` and log directory (FR-062). | If Administrator cancels, no files are modified. |

- **Trigger**: End User or Administrator accesses Settings, Tools, or maintenance actions.
- **Pre-conditions**: UnduhKilat is running with access to local SQLite file and target filesystem paths.
- **Post-conditions**: Settings/categories updated; logs safely exported; updates verified and applied in place; or database reset to clean state without deleting user files.

---

## 3. Failure Matrix Across Flows / Matriks Titik Kegagalan Lintas Alur

The following failure matrix unifies error codes referenced across all system flows and defines their standard egui/extension UI behaviours.

| Error Code | Affected Flows | Standard UI Behaviour |
|---|---|---|
| `ERR_STORAGE_INIT_FAILED` | Flow 1 | Modal dialog on startup: "Database Initialization Failure. Verify write permissions for application storage." Offers "Retry" or "Quit". |
| `ERR_REGISTRY_WRITE_FAILED` | Flow 1 | Persistent yellow alert banner in Settings: "Browser integration host could not be registered automatically. Try running as Administrator to register machine-wide." |
| `ERR_NATIVE_HOST_NOT_FOUND` | Flow 1, Flow 2 | Chromium extension icon displays an exclamation badge (`!`). Popup states: "Native Host Not Found. Launch UnduhKilat desktop application." |
| `ERR_NATIVE_HOST_TIMEOUT` | Flow 1, Flow 2 | Extension banner displays: "Connection to UnduhKilat timed out. Retrying in background..." Action buttons temporarily disabled. |
| `ERR_PAYLOAD_CORRUPT` | Flow 2 | Error logged to local diagnostics file. Desktop application displays transient toast: "Malformed link intercepted from browser." No queue item created. |
| `ERR_INVALID_URL_SCHEME` | Flow 2 | Ingestion modal highlights input box in red with label: "Unsupported protocol. Only HTTP, HTTPS, and FTP URLs are supported." OK button disabled. |
| `ERR_MEDIA_PARSE_FAILED` | Flow 2 | Floating video sniffer panel marks entry as [placeholder: label text for unparsed manifest candidates] with an info tooltip. [placeholder: whether clicking the entry shows a warning, and whether the download button is enabled for that candidate]. |
| `ERR_PATH_UNWRITABLE` | Flow 2, Flow 5 | Target folder input box displays inline warning: "Destination path inaccessible. Defaulting to fallback download directory." |
| `ERR_NAME_COLLISION` | Flow 3 | If conflict policy is prompt, displays modal: "File already exists. [Overwrite] [Rename] [Cancel]". If auto-rename, silently updates filename to `name (1).ext`. |
| `ERR_HTTP_AUTH_REQUIRED` | Flow 3 | Download status moves to `failed`. Row shows padlock icon. Right-click menu offers: "Edit Credentials & Retry". |
| `ERR_RANGE_NOT_SUPPORTED` | Flow 3, Flow 4 | If starting: badges download list row with "Single connection (No Range)". If resuming: prompts "Server cannot resume download. [Restart from 0] [Cancel]". |
| `ERR_RANGE_NOT_SATISFIABLE` | Flow 3 | Engine automatically falls back to single-connection transfer starting from byte 0. Status bar displays: "Range rejected by server; falling back." |
| `ERR_STORAGE_FULL` | Flow 3 | Download fails automatically. Status changes to `failed` with a red indicator. Warning modal: "Storage volume full. Free space and resume." |
| `ERR_CHECKSUM_SIZE_MISMATCH` | Flow 3 | Status changes to `failed`. Details panel shows: "Size mismatch: expected X bytes, received Y bytes." Toast notification is suppressed. |
| `ERR_CONNECTION_DROPPED` | Flow 3, Flow 4 | Row displays "Reconnecting (attempt X/Y)..." with retry countdown. If give-up condition reached, status marks `failed` with "Connection dropped". |
| `ERR_FILE_NOT_FOUND` | Flow 4 | Status changes to `failed`. List row details indicate: "Partial file missing from disk. Use Restart from zero to re-download." |
| `ERR_INVALID_PATH` | Flow 5 | Category settings input field bordered in red with label: "Invalid directory path format." Save button disabled until corrected. |
| `ERR_CRAWL_FAILED` | Flow 5 | Site Grabber modal displays red alert bar: "Crawl interrupted: [Reason]". Partial results remain available for selection. |
| `ERR_EXPORT_FAILED` | Flow 5 | Pop-up notification: "Log export failed: Destination path is not writable." Original log files remain undisturbed. |
| `ERR_PACKAGE_SIGNATURE_INVALID` | Flow 5 | Update dialog displays error icon: "Package verification failed: Invalid cryptographic signature. The package has been discarded." |

---

## 4. Cross-Document Consistency Notes / Catatan Konsistensi Lintas Dokumen

To ensure architectural alignment across `USER_FLOW.md`, `API.md`, `DATABASE.md`, and `REQUIREMENTS.md`, the following canonical invariants must be maintained:

### 4.1 State Machine Consistency (DATABASE.md)
- **Download States**: Strictly limited to `queued`, `running`, `paused`, `completed`, `failed`, `cancelled`.
- **Segment States**: Strictly limited to `pending`, `downloading`, `completed`.
- **Normalization Invariant**: Upon application launch following a crash or unexpected kill, any download with `state = 'running'` in `downloads` MUST be normalized to `state = 'paused'` before scheduler dispatch begins (FR-010).

### 4.2 Database Schema & Table Ownership (DATABASE.md)
The following tables are authoritative in SQLite:
- `downloads`: Master record for all transfers (IDs, URLs, headers, cookie sets, paths, status, timestamps).
- `segments`: Contiguous byte ranges (`segment_index`, `range_start`, `range_end`, `highest_contiguous_offset`, `state`).
- `categories`: File type mappings to directory targets (`category_id`, `name`, `target_directory`, `extensions`).
- `queue_items`: Queue ordering, manual priorities, and scheduling parameters (`priority`, `queue_position`).
- `credentials`: Per-host authentication records (`host`, `username`, `password_encrypted`).
- `schedules`: Scheduled execution windows and daily queue triggers.
- `settings`: Key-value application configurations (concurrency, default connection counts, speed caps).

### 4.3 Native Messaging IPC Contract (API.md)
- Two-way communication over standard I/O between Chromium and Rust native host uses length-prefixed JSON frames.
- Outbound messages from Extension to Host:
  - `interceptDownload` (URL, headers, cookies, Referer, User-Agent)
  - `mediaCandidateDetected` (URL, MIME type, stream type, discovered size)
  - `queryDownloadState` (target URL or download ID)
- Inbound messages from Host to Extension:
  - `pushStatusUpdate` (download ID, canonical state, progress percentage, speed)
  - `ackIntercept` (success boolean, error code if rejected)

### 4.4 Traceability to User Decisions (DEC-XXX)
- **DEC-001**: Concurrency and connections per download have **no hard-coded ceiling** in flows or validation logic; any positive integer entered by the user is accepted and persisted (FR-001, FR-002).
- **DEC-002**: Queue dispatch strictly prioritizes items by **manual priority first**, followed by **FIFO** queue position for equal priorities (FR-018, Flow 3).
- **DEC-003**: Web media capture uses a **hybrid architecture** combining request sniffing with DOM extension hooks, deduplicating candidate URLs into a unified floating panel (FR-041, FR-042, Flow 2).
- **DEC-004**: Closing the main window **hides to tray** while active transfers continue running; completion triggers a **Windows toast notification** (FR-052, FR-054, Flow 4).
- **DEC-005**: Target platform is strictly **Windows 10 and 11, x64 only** (TC-009).
- **DEC-006**: Browser integration scope is limited to **Chromium browsers only** (Chrome, Edge, Brave; FR-033).
- **DEC-007**: Deployment model is strictly a **portable ZIP without an installer** (FR-059). Machine-wide registration is an optional Administrator capability (FR-035).