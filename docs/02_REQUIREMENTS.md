# REQUIREMENTS.md: UnduhKilat

This document establishes the testable functional and non-functional requirements for the UnduhKilat Windows download manager application using RFC 2119 keyword precision. It specifies runtime behaviors, input validations, operational states, acceptance criteria, and technical constraints required for quality assurance and verification; it does not replicate structural data schemas, which reside in `DATABASE.md`, architectural workflows defined in `PRD.md`, or inter-process communication protocols detailed in `API.md`.

---

## 1. Document Scope & Conventions / Ruang Lingkup Dokumen & Konvensi & Legenda

### 1.1 Conformance Keywords & QA Consequences

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT", "SHOULD", "SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this document are to be interpreted as described in RFC 2119.

| Keyword | Definition | Consequence for QA / Test Execution |
|---|---|---|
| **MUST** / **SHALL** / **REQUIRED** | Absolute technical requirement. | Test failure blocks release; automated/manual regression test mandatory. |
| **MUST NOT** / **SHALL NOT** | Absolute technical prohibition. | Test failure indicates invalid behavior or security violation; blocks release. |
| **SHOULD** / **RECOMMENDED** | Standard practice; valid reasons may exist in edge cases to deviate. | Requires formal QA approval and test justification if not fulfilled. |
| **SHOULD NOT** / **NOT RECOMMENDED** | Unacceptable behavior except under explicitly analyzed corner cases. | Logged as defect unless accompanied by approved waiver. |
| **MAY** / **OPTIONAL** | Purely elective behavior. | Tested for non-regression if implemented; missing feature does not fail test. |

### 1.2 Release Scope Labels

- `[MVP]`: Mandatory requirement for the minimum viable product delivery.
- `[POST-MVP]`: Deferred capability; baseline interfaces may exist, but completion is not required for initial release.

### 1.3 Unset Values & Explicit Markers

Any numeric threshold, retention period, or policy parameter not explicitly finalized by the product definition is designated as `TBD`. Test suites MUST NOT substitute arbitrary hardcoded numbers for `TBD` placeholders; tests against `TBD` properties must verify configuration exposure and validate that default behaviors match the defined fallback specifications.

---

## 2. Functional Requirements — User-Facing

### FR-001 — Maximum Concurrent Downloads Setting [MVP]
The application MUST allow the End User to configure the maximum number of simultaneous running downloads to any positive integer without an application-imposed upper ceiling, enforce this exact limit during queue dispatch, and persist the setting across application restarts (DEC-001).

**Acceptance Criteria**:
- **Given** the application is idle with `max_concurrent_downloads` set to `3`,
- **When** the user changes the setting to `15` and enqueues 20 items,
- **Then** the engine transitions exactly 15 items to `RUNNING` status and retains 5 items in `QUEUED` status;
- **When** the application process is terminated and relaunched,
- **Then** the configuration reader loads `max_concurrent_downloads = 15`.

### FR-002 — Per-Download Parallel Connection Limit [MVP]
The application MUST allow the End User to configure the number of parallel network connections per download to any positive integer without an application-imposed upper ceiling, both as an engine-wide default for new tasks and as an item-specific override, and persist these values across restarts (DEC-001).

**Acceptance Criteria**:
- **Given** the default connection count is set to `8`,
- **When** a user creates a download item with custom connection count `32`,
- **Then** the item's persisted configuration reflects `connections = 32` while newly created items default to `8`;
- **When** the item connects to a range-supporting endpoint,
- **Then** the engine opens exactly 32 concurrent TCP/TLS connections for byte segments.

### FR-003 — Multi-Connection Byte-Range Segmentation [MVP]
When a target HTTP/HTTPS server responds with `Accept-Ranges: bytes` and a valid `Content-Length`, the engine MUST split the target file payload into contiguous non-overlapping byte ranges matching the configured connection count, dispatch parallel HTTP requests using the `Range: bytes=start-end` header, and write chunks into the destination file at their respective offsets.

**Acceptance Criteria**:
- **Given** a 100 MB resource (`Content-Length: 104857600`) supporting `Accept-Ranges: bytes` and `connections = 4`,
- **When** the download starts,
- **Then** 4 concurrent workers issue requests for ranges `0-26214399`, `26214400-52428799`, `52428800-78643199`, and `78643200-104857599`, writing chunks to disk at their corresponding file offsets.

### FR-004 — Dynamic Segment Re-Splitting [MVP]
The engine MUST split the remaining byte range of the segment selected by the selection rule [placeholder] and assign the upper half to a newly allocated connection worker when the split policy determines that re-splitting reduces remaining transfer duration, according to user-configurable settings screen parameters.

**Acceptance Criteria**:
- **Given** an active download with an idle connection slot and a segment selected by the selection rule [placeholder] with remaining bytes greater than `min_split_size` (`TBD`),
- **When** the dynamic re-split evaluator executes,
- **Then** the selected segment's remaining range `[offset, end]` is truncated to `[offset, mid]` and a new segment `[mid + 1, end]` is spawned on a new worker.

### FR-005 — Non-Range Transfer Fallback [MVP]
When an upstream server omits `Accept-Ranges: bytes` or rejects a range request with HTTP status `200` instead of `206 Partial Content`, the engine MUST fall back to a single-connection streaming transfer, complete the download, and set the UI display to indicate that segmentation is disabled.

**Acceptance Criteria**:
- **Given** a URL pointing to a server returning HTTP `200 OK` without `Accept-Ranges: bytes`,
- **When** the download is initiated,
- **Then** the engine opens exactly 1 connection, updates item UI status flag `is_segmented` to `false`, streams data to the output file, and marks the item `COMPLETED` when the stream terminates.

### FR-006 — Download Pause State Preservation [MVP]
When an End User pauses an active download, the engine MUST cancel all active connection tasks, flush all in-flight buffers to disk, write the highest contiguous written offset of each segment to the SQLite state database, and transition the item status to `PAUSED`.

**Acceptance Criteria**:
- **Given** a running segmented download with 4 active connections,
- **When** the user clicks "Pause",
- **Then** Tokio tasks for all 4 workers are aborted, pending buffers flush to disk within `TBD` ms, `DATABASE.md` segment tables record current offsets, and item status transitions to `PAUSED`.

### FR-007 — Download Resumption Integrity [MVP]
When an End User resumes a `PAUSED` download, the engine MUST read the last persisted segment offsets from the state database, initiate new HTTP range requests starting from those offsets without re-fetching previously stored bytes, and produce a finished file byte-identical to the upstream resource.

**Acceptance Criteria**:
- **Given** a `PAUSED` download with 50 MB of 100 MB already saved across 4 segments,
- **When** the user clicks "Resume",
- **Then** 4 workers issue range requests starting strictly at each segment's persisted offset, no bytes prior to those offsets are requested, and the resulting file checksum matches the source.

### FR-008 — Automatic Network Drop Recovery [MVP]
When an active connection drops unexpectedly due to an I/O timeout, network disconnect, or socket reset, the engine MUST automatically retry the failed segment from its last verified written offset without user intervention, transitioning to `failed` only if the retry policy limits are exhausted.

**Acceptance Criteria**:
- **Given** an active segment transfer that encounters a `ConnectionReset` error,
- **When** retry count is below `max_retries` (`TBD`),
- **Then** the engine executes exponential backoff (`TBD` ms), reopens the connection with `Range: bytes=<last_offset>-<end>`, and resumes streaming;
- **When** retry attempts exceed `max_retries`,
- **Then** the item transitions to status `failed` with error code `ERR_NET_DISCONNECTED`.

### FR-009 — Download Restart from Zero [MVP]
The engine MUST allow an End User to restart an existing download task, which resets all recorded segment offsets in the state database, truncates the target file on disk to 0 bytes, and starts the transfer process afresh.

**Acceptance Criteria**:
- **Given** an item in status `PAUSED`, `failed`, or `COMPLETED`,
- **When** the user triggers the "Restart" action,
- **Then** all segment offset records are cleared, the partial file on disk is truncated to `0` bytes, and the item transitions to `QUEUED` or `RUNNING`.

### FR-010 — Post-Crash State Recovery [MVP]
If the application process terminates abruptly or the host loses power, the application MUST recover all previously `RUNNING` downloads to a resumable `PAUSED` state upon the next startup, preserving all checkpointed segment offsets, and allow resuming all recovered items via a single user action.

**Acceptance Criteria**:
- **Given** 3 downloads running when the process is killed via `taskkill /F /IM unduhkilat.exe`,
- **When** UnduhKilat is launched again,
- **Then** the 3 items appear in `PAUSED` status with valid segment progress bars reflecting the last SQLite checkpoint, and clicking "Resume All" moves all 3 to `RUNNING`.

### FR-011 — Final File Size Verification [MVP]
Upon receiving the final byte of a download, the engine MUST verify the total size of the assembled file on disk against the expected size determined from the upstream `Content-Length` or range definition, and transition the item to `failed` if any mismatch is detected.

**Acceptance Criteria**:
- **Given** an upstream resource with expected size `52428800` bytes,
- **When** the workers complete and assemble the file,
- **Then** if the disk file size equals `52428800`, status transitions to `COMPLETED`;
- **When** the disk file size equals `52420000` (truncated),
- **Then** status transitions to `failed` with error code `ERR_SIZE_MISMATCH`.

### FR-012 — Multi-Protocol Support (HTTP/HTTPS/FTP) [MVP]
The engine MUST support file transfers over HTTP, HTTPS (via `rustls`), and FTP, including resuming interrupted FTP transfers using the `REST` command.

**Acceptance Criteria**:
- **Given** an FTP URL `ftp://user:pass@example.com/file.bin`,
- **When** a partially transferred FTP file is resumed from offset `1048576`,
- **Then** the engine sends the FTP command `REST 1048576`, receives `350 Restarting at 1048576`, and resumes streaming data.

### FR-013 — Redirect and Referer Preservation [MVP]
The engine MUST follow HTTP `301`, `302`, `303`, `307`, and `308` redirects to the final target URL, record the effective URL on the download record, and retain and send the original `Referer` header across all redirect hops if specified.

**Acceptance Criteria**:
- **Given** a download initiated for `http://example.com/dl` with `Referer: http://example.com/page`,
- **When** the server responds with `302 Found` pointing to `https://cdn.example.com/asset.zip`,
- **Then** the engine connects to `https://cdn.example.com/asset.zip`, sends `Referer: http://example.com/page`, and stores the final URL as `effective_url`.

### FR-014 — Host and Item Credential Management [MVP]
The application MUST allow the End User to configure HTTP Basic/Digest and FTP authentication credentials either per download or stored per hostname, sending credentials automatically when the server challenges with HTTP `401 Unauthorized` or FTP `530`.

**Acceptance Criteria**:
- **Given** stored credentials `user: "alice"`, `password: "secret"` for host `files.corp.internal`,
- **When** a download request to `files.corp.internal/doc.pdf` returns `401 Unauthorized`,
- **Then** the engine re-issues the request with the `Authorization: Basic ...` header and transfers the resource.

### FR-015 — Proxy Support [MVP]
The application MUST support Direct, System Proxy, and Manual Proxy (HTTP/HTTPS host, port, and optional credentials) configurations, apply the proxy routing to all download connections and browser integration requests, and persist proxy settings across restarts.

**Acceptance Criteria**:
- **Given** manual proxy configured as `proxy.internal:8080` with credentials,
- **When** a download connection or extension communication executes,
- **Then** all TCP/TLS connections route through `proxy.internal:8080` with `Proxy-Authorization` headers.

### FR-016 — File Collision Handling [MVP]
When writing a completed download to a target path that already contains an existing file, the application MUST NOT silently overwrite the file; it MUST apply the configured collision policy (auto-rename with numeric suffix or prompt the user, policy parameter `TBD`) and record the actual final path in the state store.

**Acceptance Criteria**:
- **Given** an existing file `C:\Downloads\file.zip`,
- **When** a new download completes targeted at `C:\Downloads\file.zip` with auto-rename enabled,
- **Then** the file is saved as `C:\Downloads\file (1).zip` and the UI download record updates `destination_path` to the new path.

### FR-017 — Session Context Capture & Attachment [MVP]
The application MUST allow attaching captured session `Cookie`, `User-Agent`, and `Referer` headers to any download task, transmitting them in all segment requests to permit downloading session-locked resources (DEC-006).

**Acceptance Criteria**:
- **Given** a download initiated with `Cookie: session_id=xyz123`,
- **When** worker threads dispatch range requests,
- **Then** every segment request includes `Cookie: session_id=xyz123`.

### FR-018 — Two-Level Queue Dispatch Order [MVP]
The queue dispatcher MUST schedule pending downloads using a strict two-level rule: items with a higher manual priority value MUST be dispatched before items with lower priority, and items with equal priority MUST be dispatched in First-In-First-Out (FIFO) queue insertion order (DEC-002).

**Acceptance Criteria**:
- **Given** queued items A (Priority: Normal, Enqueued: 10:00), B (Priority: Normal, Enqueued: 10:05), and C (Priority: High, Enqueued: 10:10),
- **When** a download concurrency slot opens,
- **Then** Item C is dispatched first; when another slot opens, Item A is dispatched, followed by Item B.

### FR-019 — Dynamic Priority Adjustment [MVP]
The End User MUST be able to raise, lower, or clear the priority of any queued item at any time, and the updated priority MUST take effect at the next dispatch evaluation without requiring a queue restart.

**Acceptance Criteria**:
- **Given** Item X in `QUEUED` status with `priority = Normal` behind Item Y with `priority = Normal`,
- **When** the user changes Item X's priority to `High`,
- **Then** at the next dispatch event, Item X is selected before Item Y.

### FR-020 — Concurrency Slot Enforcement [MVP]
The engine MUST continuously enforce that the number of simultaneously active (`RUNNING`) downloads does not exceed `max_concurrent_downloads`, holding excess items in `QUEUED` status and automatically transitioning the next eligible item to `RUNNING` when an active item transitions to `COMPLETED`, `PAUSED`, `failed`, or `CANCELLED`.

**Acceptance Criteria**:
- **Given** `max_concurrent_downloads = 2`, with 2 items `RUNNING` and 1 item `QUEUED`,
- **When** one of the running items transitions to `COMPLETED`,
- **Then** the engine immediately shifts the top `QUEUED` item to `RUNNING`.

### FR-021 — Manual Queue Reordering [MVP]
The End User MUST be able to reorder queued items via the UI, and the resulting list order MUST update the FIFO sequence used for dispatching items of identical priority.

**Acceptance Criteria**:
- **Given** items A and B both having `Normal` priority with A ordered above B,
- **When** the user drags B above A,
- **Then** B's queue position updates and B is dispatched before A when a slot opens.

### FR-022 — Start All and Stop All Actions [MVP]
The application MUST provide "Start All" and "Stop All" controls that transition all eligible queued items to running (subject to the concurrency ceiling) or transition all active running items to paused, while preserving each item's saved segment offsets.

**Acceptance Criteria**:
- **Given** 5 items in `PAUSED` or `QUEUED` states and `max_concurrent_downloads = 2`,
- **When** "Start All" is clicked,
- **Then** the first 2 items transition to `RUNNING` and the remaining 3 transition to `QUEUED`;
- **When** "Stop All" is clicked,
- **Then** all running items transition to `PAUSED` with segment progress saved to SQLite.

### FR-023 — Scheduler Execution [MVP]
The application MUST allow the End User to configure persistent schedule rules specifying exact times or daily time windows for starting and stopping the queue, and the engine MUST execute these actions automatically while running.

**Acceptance Criteria**:
- **Given** a schedule rule configured to "Start queue at 02:00" and "Stop queue at 06:00",
- **When** system time reaches 02:00:00,
- **Then** the engine invokes "Start All";
- **When** system time reaches 06:00:00,
- **Then** the engine invokes "Stop All".

### FR-024 — Background Scheduled Execution [MVP]
The scheduler MUST continue monitoring and executing scheduled start and stop triggers while the main application window is hidden in the system tray and without user interaction (DEC-004).

**Acceptance Criteria**:
- **Given** the application window is hidden to the system tray and a start trigger is scheduled for 03:00,
- **When** the system clock reaches 03:00,
- **Then** the engine activates the queue and downloads begin running in the background.

### FR-025 — Global Bandwidth Throttling [MVP]
The End User MUST be able to set a global download speed limit in KB/s, and the engine MUST throttle the aggregate network throughput across all active transfers so that it does not exceed this limit.

**Acceptance Criteria**:
- **Given** global speed limit set to `1000` KB/s and 2 running downloads,
- **When** both downloads transfer simultaneously,
- **Then** the sum of their throughput does not exceed `1000` KB/s over the measurement window (`TBD`).

### FR-026 — Per-Download Bandwidth Throttling [MVP]
The End User MUST be able to set a per-download speed limit in KB/s on any item, and the engine MUST cap that individual item's throughput to that limit, subject to the dual-throttle arbitration defined in FR-027.

**Acceptance Criteria**:
- **Given** an individual item configured with a speed limit of `250` KB/s,
- **When** the item runs on an unconstrained network,
- **Then** the item's throughput does not exceed `250` KB/s.

### FR-027 — Dual-Throttle Arbitration [MVP]
When both a global speed limit and an item-specific speed limit are active, the engine MUST enforce an effective rate ceiling equal to the minimum of the item limit and the item's proportional share of the global limit, where the item's proportional share is computed by the owner-defined formula [placeholder], reflecting the throttled rate in the UI.

**Acceptance Criteria**:
- **Given** a global limit of `500` KB/s and an item limit of `200` KB/s,
- **When** the item runs alone,
- **Then** its throughput caps at `200` KB/s;
- **When** the global limit is lowered to `100` KB/s,
- **Then** the item's throughput caps at `100` KB/s.
- **Given** a global limit and more than one running item,
- **When** the arbitrator computes each item's share of the global limit,
- **Then** each item's share is computed by the owner-defined formula [placeholder], and the effective ceiling is the minimum of that item's limit and its computed share.

### FR-028 — Dynamic Throttle Adjustment [MVP]
The engine MUST apply changes made to global or per-download speed limits to running transfers within the next throttle interval without interrupting, pausing, or restarting active connections, treating a value of `0` as unconstrained.

**Acceptance Criteria**:
- **Given** an active download running at `500` KB/s under a limit of `500` KB/s,
- **When** the user updates the limit to `1500` KB/s,
- **Then** within `TBD` ms the transfer accelerates toward `1500` KB/s without connection resets.

### FR-029 — Category Routing by Extension [MVP]
The application MUST categorize incoming downloads based on file extension matching pre-defined system categories (Compressed, Documents, Music, Programs, Video) and route the downloaded file into the directory assigned to that category.

**Acceptance Criteria**:
- **Given** category "Video" mapped to folder `C:\Downloads\Video` for extension `.mp4`,
- **When** a download for `http://example.com/movie.mp4` is added,
- **Then** its destination directory automatically sets to `C:\Downloads\Video`.

### FR-030 — Fallback Category for Unmatched Types [MVP]
When a download's file extension or `Content-Type` does not match any configured category rule, the application MUST assign the item to a designated default fallback category targeting a configurable default folder.

**Acceptance Criteria**:
- **Given** default fallback category mapped to `C:\Downloads\General`,
- **When** a file with unknown extension `.xyz` is enqueued,
- **Then** the destination folder defaults to `C:\Downloads\General`.

### FR-031 — Custom Category Management [MVP]
The End User MUST be able to create, edit, rename, and delete custom categories, adjust their file extension associations, and modify their target destination paths, applying changes immediately to future downloads without modifying completed files.

**Acceptance Criteria**:
- **Given** a user creates category "Torrents" mapped to `.torrent` and directory `D:\Torrents`,
- **When** a `.torrent` link is captured,
- **Then** the download dialog assigns category "Torrents" and destination `D:\Torrents`.

### FR-032 — Category and Path Override [MVP]
The Add URL confirmation dialog and the item properties view MUST allow the End User to override the automatically deduced category and destination path for any individual download.

**Acceptance Criteria**:
- **Given** an incoming `.iso` file categorized as "Compressed" (`C:\Downloads\Compressed`),
- **When** the user manually selects `D:\ISOs` in the Add URL dialog and confirms,
- **Then** the file is saved directly into `D:\ISOs`.

### FR-033 — Chromium Browser Extension Reach [MVP]
The project MUST provide a Manifest V3 browser extension compatible exclusively with Chromium-class browsers, specifically Google Chrome, Microsoft Edge, and Brave; no other browser engines SHALL be supported (DEC-006).

**Acceptance Criteria**:
- **Given** a target environment with Google Chrome, Microsoft Edge, and Brave installed,
- **When** the extension is loaded,
- **Then** the extension executes without runtime syntax or manifest errors across all three browsers.

### FR-034 — Per-User Native Messaging Auto-Registration [MVP]
On its initial run, the portable application MUST register the Native Messaging host in the current Windows user registry under `HKCU\Software\Google\Chrome\NativeMessagingHosts`, `HKCU\Software\Microsoft\Edge\NativeMessagingHosts`, and the corresponding Brave path, referencing the host binary inside the portable directory without requiring elevation (DEC-006, DEC-007).

**Acceptance Criteria**:
- **Given** an uninstalled portable ZIP extracted to `D:\Tools\UnduhKilat`,
- **When** `unduhkilat.exe` is launched by a standard non-elevated user,
- **Then** registry entries under `HKCU\Software\Google\Chrome\NativeMessagingHosts\com.unduhkilat.host` are created pointing to the JSON manifest referencing `D:\Tools\UnduhKilat\native_host.exe`.

### FR-036 — Browser Navigation & Download Interception [MVP]
The browser extension MUST intercept user navigations and download events targeting downloadable file types, extract the URL, `Referer`, `User-Agent`, and session cookies, and transmit them via Native Messaging to the desktop application to open the confirmation dialog (DEC-006).

**Acceptance Criteria**:
- **Given** the extension is active in Microsoft Edge,
- **When** the user clicks an `.exe` download link,
- **Then** the browser's default download is cancelled, an IPC message containing URL, headers, and cookies is sent to UnduhKilat, and the confirmation dialog appears (FR-039).

### FR-037 — Browser Cookie Extraction [MVP]
The browser extension MUST extract all cookies associated with the target resource host and deliver them to UnduhKilat so authenticated downloads can proceed without credential errors (DEC-006).

**Acceptance Criteria**:
- **Given** a user logged into a private portal on Chrome,
- **When** a download is triggered from that page,
- **Then** the IPC payload to the host includes the complete cookie jar for that domain, and the desktop engine transmits those cookies in range requests.

### FR-038 — Two-Way Status Reporting to Extension [MVP]
The application MUST send download progress and status transitions for extension-initiated tasks back through Native Messaging to the extension, and the extension MUST reflect these states in its popup UI.

**Acceptance Criteria**:
- **Given** a download initiated via the browser extension,
- **When** the download state changes from `RUNNING` to `COMPLETED`,
- **Then** an IPC event `{"type": "STATUS_UPDATE", "id": 42, "status": "COMPLETED"}` is sent to the extension, and the extension UI marks the item complete.

### FR-039 — Explicit Download Confirmation Dialog [MVP]
Whenever a download is submitted from the browser extension, the clipboard monitor, or the manual Add URL dialog, the application MUST present a modal confirmation dialog displaying URL, suggested filename, detected file size, category, and destination path, and MUST NOT start the download until the user explicitly confirms (FR-011, FR-040).

**Acceptance Criteria**:
- **Given** a download trigger received from the browser,
- **When** the IPC message arrives,
- **Then** the Add URL confirmation dialog appears pre-populated with file metadata, no network worker threads are started, and the download is created only after clicking "Start Now" or "Start Later".

### FR-040 — Clipboard URL Monitoring [MVP]
The application MUST provide a toggleable clipboard monitoring feature that inspects the Windows clipboard for URLs matching downloadable extensions; when active, copying a matching URL MUST launch the confirmation dialog prefilled with the URL, and no download SHALL start without explicit user confirmation (DEC-006).

**Acceptance Criteria**:
- **Given** clipboard monitoring is enabled,
- **When** the user copies `https://example.com/archive.zip` to the Windows clipboard,
- **Then** the Add URL dialog opens with `https://example.com/archive.zip` pre-filled;
- **When** clipboard monitoring is disabled and the same URL is copied,
- **Then** no dialog opens.

### FR-041 — Hybrid Media Sniffing & Hook Detection [MVP]
The media capture system MUST operate a hybrid detection engine combining network request inspection (sniffing web requests via extension APIs) with DOM-level media element hook signals to detect audio/video streams on active web pages (DEC-003).

**Acceptance Criteria**:
- **Given** a web page with an embedded video player,
- **When** the page issues an HTTP request for a `.mp4` stream AND the DOM injects an `<audio>`/`<video>` element,
- **Then** the hybrid detection pipeline merges candidate events from both sources into a unified media candidate list.

### FR-042 — Media Candidate Deduplication [MVP]
The hybrid media detector MUST deduplicate candidate streams by canonical URL, presenting each candidate in the floating panel with its detected MIME type, media resolution/bitrate (if available), and estimated size (DEC-003).

**Acceptance Criteria**:
- **Given** a video stream detected simultaneously by the network sniffer and the DOM hook with identical canonical URL `https://media.org/v1.mp4`,
- **When** the candidate list is rendered,
- **Then** exactly 1 entry for `v1.mp4` appears in the list displaying its type and file size.

### FR-043 — Floating Video Download Panel [MVP]
The browser extension MUST render a floating UI panel overlaid near active media elements on the web page, allowing the End User to click the panel and send the selected stream directly to UnduhKilat for segmented download (DEC-003).

**Acceptance Criteria**:
- **Given** a detected video candidate on the active tab,
- **When** the user hovers over or views the media element,
- **Then** a floating "Download Video" widget appears over the page;
- **When** clicked,
- **Then** the stream URL and page headers are transmitted to UnduhKilat and the confirmation dialog displays.

### FR-044 — Manifest Stream Detection Display [MVP]
The media detection panel MUST detect and list manifest-based streams (such as DASH `.mpd` and HLS `.m3u8`), displaying their format type to the user while marking downstream merging handling as `TBD` (DEC-003).

**Acceptance Criteria**:
- **Given** a web page streaming via HLS (`playlist.m3u8`),
- **When** the media sniffer captures the playlist URL,
- **Then** the floating panel displays the item with format label `HLS (m3u8)`.

### FR-045 — Pattern-Based Page Batch Downloader [MVP]
The application MUST allow the End User to supply a webpage URL and a pattern string containing wildcards (e.g. `*.pdf`, `image_???.jpg`), scan the page for matching hyperlink targets, display a preview list of all matched URLs, and enqueue only the user-checked items.

**Acceptance Criteria**:
- **Given** a webpage containing links to 10 `.pdf` files and 5 `.zip` files,
- **When** the user runs the Batch Downloader targeting `*.pdf`,
- **Then** a preview modal displays exactly the 10 `.pdf` URLs, and clicking "Download Selected" enqueues only the selected items.

### FR-046 — Depth-Limited Site Grabber Walk [MVP]
The application MUST support recursive site-walking starting from a root URL up to a user-configured link depth (default depth `TBD`) using include and exclude regex/wildcard filters, generating an interactive preview of discovered assets prior to queue insertion.

**Acceptance Criteria**:
- **Given** a root URL and max crawl depth configured to `2`, with include filter `*.png`,
- **When** the crawl completes,
- **Then** an asset tree preview displays all `.png` links found up to depth 2, allowing the user to select items and assign them to the download queue.

### FR-047 — Batch Item Governance [MVP]
All download tasks generated via the Batch Downloader or Site Grabber MUST enter the queue as standard download items governed by the global concurrency limit (FR-020), priority-FIFO dispatching (FR-018), category rules (FR-029), and bandwidth limits (FR-025, FR-026).

**Acceptance Criteria**:
- **Given** 50 links enqueued from a site walk with `max_concurrent_downloads = 3`,
- **When** batch enqueue is confirmed,
- **Then** exactly 3 items start running simultaneously, routing files to their matching category folders under active throttle limits.

### FR-048 — Real-Time Download List View [MVP]
The egui desktop UI MUST display a table of all downloads showing for each item: filename, file size, status, progress percentage, current download speed (KB/s or MB/s), estimated time remaining (ETA), category, destination folder, and priority, updating values in real time and supporting column sorting and category/status filtering.

**Acceptance Criteria**:
- **Given** active transfers running in the engine,
- **When** the UI renders its frame loop,
- **Then** progress bars, transfer rates, and ETAs update smoothly without freezing the UI thread, and clicking column headers sorts items ascending/descending.

### FR-049 — Multi-URL Add Dialog [MVP]
The Add URL dialog MUST support inputting multiple URLs simultaneously (one per line) and allow the End User to apply common settings (category, destination path, connection count, priority, credentials, and referer) to all entered items before enqueueing them.

**Acceptance Criteria**:
- **Given** 5 URLs pasted into the Add URL text area,
- **When** the user specifies `connections = 16` and category "Documents" and clicks "OK",
- **Then** 5 distinct download tasks are created in SQLite, each configured with 16 connections and the Documents folder.

### FR-050 — Settings Management Interface [MVP]
The desktop UI MUST provide a settings screen exposing configuration for: concurrency limits, default connections per download, global/item speed limits, categories and folders, proxy configuration, saved host credentials, scheduler windows, clipboard monitoring toggle, tray/toast options, browser integration status, and log level, persisting every modification across restarts.

**Acceptance Criteria**:
- **Given** the settings window open,
- **When** the user modifies the log level from `INFO` to `DEBUG` and changes the default download directory,
- **Then** the settings file in SQLite updates immediately, and restarting the application retains both values.

### FR-051 — System Tray Lifecycle & Context Menu [MVP]
The application MUST maintain an icon in the Windows notification area (system tray) throughout its entire execution lifetime, offering a context menu with options to show/hide the main window, start the queue, stop the queue, and exit the application (DEC-004).

**Acceptance Criteria**:
- **Given** UnduhKilat is running,
- **When** the user right-clicks the system tray icon,
- **Then** a context menu displays items: "Open", "Start Queue", "Stop Queue", and "Exit".

### FR-052 — Close-to-Tray Window Behavior [MVP]
When the End User clicks the main window close button (`X`), the application MUST hide the window to the system tray instead of exiting, allowing all active downloads and scheduler tasks to continue executing uninterrupted (DEC-004).

**Acceptance Criteria**:
- **Given** 2 downloads running at 2 MB/s,
- **When** the user clicks the title bar close button (`X`),
- **Then** the main window hides from the taskbar, the tray icon remains visible, and network traffic shows transfers continue at 2 MB/s without interruption.

### FR-053 — Clean Application Termination [MVP]
The application MUST terminate only when the user explicitly triggers "Exit" from the tray menu or application file menu, executing a clean shutdown that checkpoints all running download segments, flushes pending SQLite transactions, notifies the user that active downloads are stopping, and releases file handles.

**Acceptance Criteria**:
- **Given** active running downloads,
- **When** the user clicks "Exit" in the tray context menu,
- **Then** a confirmation prompt or notification states downloads will stop, all segment buffers flush to disk, SQLite commits segment offsets, and the process exits with return code `0`.

### FR-054 — Windows Toast Notification on Completion [MVP]
The application MUST display a native Windows toast notification whenever a download completes, firing even when the main window is hidden in the system tray, and providing an option in settings to disable notifications (DEC-004).

**Acceptance Criteria**:
- **Given** the window is hidden in the tray and notifications are enabled,
- **When** a download finishes,
- **Then** a Windows toast notification appears displaying the filename and a "Completed" message;
- **When** notifications are disabled in settings,
- **Then** no toast appears.

### FR-055 — Segment Progress Visualizer [MVP]
The desktop UI MUST provide a detailed segment view for any selected download, visually displaying each individual segment's allocated byte range, current progress offset, and connection status matching the internal state store.

**Acceptance Criteria**:
- **Given** a 4-connection download running,
- **When** the user opens the item's segment inspector,
- **Then** 4 distinct progress bars display the start byte, current byte, end byte, and download speed of each worker.

### FR-056 — Diagnostic Log & Connection Viewer [MVP]
The application MUST include an in-application diagnostics panel showing the tail of the current local log file, the last recorded error for each failed item, and active TCP connection counts, with a button to export log files to a user-chosen directory.

**Acceptance Criteria**:
- **Given** an error occurred during a download,
- **When** the user opens the Diagnostics panel,
- **Then** the panel displays the exact error stack trace from the log tail, live connection statistics, and an "Export Logs" button that writes `.log` files to the selected path.

### FR-057 — Local Rotating File Logging [MVP]
The application MUST log operational events and errors to local rotating log files on the host machine, enforce rotation based on file size and count (`TBD`), and MUST NOT transmit any telemetry or log data over the network.

**Acceptance Criteria**:
- **Given** UnduhKilat running under normal operations,
- **When** log messages are emitted,
- **Then** entries write strictly to local disk files, rotating when reaching max size (`TBD` MB), and network analysis confirms 0 bytes sent to external analytics/telemetry endpoints.

### FR-058 — Durable State Store with Transactional Checkpoints [MVP]
All application state—including downloads, segment offsets, queue sequence, priorities, categories, credentials, and settings—MUST be stored in a single local SQLite database file, updated via transactional checkpoints during active transfers to guarantee recovery following process kill.

**Acceptance Criteria**:
- **Given** an active download writing data,
- **When** SQLite commits a progress checkpoint,
- **Then** the transaction updates segment offsets with `WAL` journaling mode enabled;
- **When** the process is terminated mid-write,
- **Then** SQLite integrity check passes upon relaunch with 0 corrupted database pages.

### FR-059 — Zero-Install Portable Execution [MVP]
The application MUST run entirely from an extracted portable ZIP folder without requiring an installer or elevated Administrator privileges for standard operation, writing state files and logs strictly inside its local folder structure or user AppData, and removing completely by deleting the directory (DEC-007).

**Acceptance Criteria**:
- **Given** a fresh standard Windows 10/11 x64 user account without administrative rights,
- **When** `UnduhKilat.zip` is extracted to `C:\Users\User\Downloads\UnduhKilat` and `unduhkilat.exe` is executed,
- **Then** the application launches, initializes its database, registers user native messaging, and performs downloads without requesting UAC elevation.

### FR-060 — Cryptographic Update Signature Verification [MVP]
Before replacing any portable executable or bundle component during an auto-update, the application MUST cryptographically verify the digital signature of the downloaded update package against a bundled public key, and MUST reject unsigned or invalidly signed packages with an error dialog (DEC-007).

**Acceptance Criteria**:
- **Given** an update package `update.zip` that is corrupted or signed with an untrusted key,
- **When** the update verification routine executes,
- **Then** verification fails with error `ERR_PACKAGE_SIGNATURE_INVALID`, no local files are overwritten, and an error prompt alerts the user.

---

## 3. Functional Requirements — Admin-Facing

### FR-035 — Machine-Wide Native Messaging Registration (Admin) [MVP]
The application MUST provide a command-line or administrative option to register the Native Messaging host under `HKLM\Software\Google\Chrome\NativeMessagingHosts` and `HKLM\Software\Microsoft\Edge\NativeMessagingHosts`, enabling browser capture for all Windows user accounts on the machine, and ensuring the registration survives an in-place portable bundle update (DEC-006).

**Acceptance Criteria**:
- **Given** an elevated Command Prompt (`runas /user:Administrator`),
- **When** the command `UnduhKilat.exe --admin-register-host` is executed,
- **Then** registry keys are created under `HKLM\Software\Google\Chrome\NativeMessagingHosts\com.unduhkilat.host` and `HKLM\Software\Microsoft\Edge\NativeMessagingHosts\com.unduhkilat.host` referencing the absolute path of the host JSON manifest, exiting with status code `0`;
- **When** a non-elevated user runs the same command,
- **Then** the process fails with error code `ERR_PERMISSION_DENIED`.

### FR-061 — In-Place Bundle Replacement (Admin) [MVP]
The application MUST support being replaced in-place within a shared or protected filesystem directory by an Administrator deploying a newer signed portable bundle, such that existing machine-wide registry registrations remain valid and resolve correctly to the updated binary executable.

**Acceptance Criteria**:
- **Given** an existing installation registered machine-wide at `C:\Program Files\UnduhKilat\`,
- **When** an Administrator overwrites the directory contents with a verified update bundle having identical executable names,
- **Then** subsequent browser launches execute the updated `native_host.exe` from `C:\Program Files\UnduhKilat\` without re-registration, maintaining IPC protocol compatibility.

### FR-062 — Application Data Reset (Admin) [MVP]
The application MUST provide an administrative reset command that, upon explicit user confirmation, purges the SQLite database file and all rotating log files from the data directory while leaving completed files in user download directories untouched.

**Acceptance Criteria**:
- **Given** an existing SQLite database containing 50 download records and 10 MB of local logs,
- **When** the administrator runs `unduhkilat.exe --admin-reset-data`,
- **Then** the database file `unduhkilat.db` and log files `*.log` are deleted, previously completed files in destination folders remain intact, and the command exits with status code `0`.

---

## 4. Non-Functional Requirements

### NFR-001 — Supported Operating System Environment
The application MUST execute natively on 64-bit Windows 10 (version 1809 or higher) and Windows 11; all other Windows versions (including Windows 7, 8, and 8.1) and non-Windows platforms SHALL NOT be supported (DEC-005).
- **Verification Method**: Automated execution of test suite on clean target images of Windows 10 x64 and Windows 11 x64; build target check verifying `x86_64-pc-windows-msvc`.

### NFR-002 — Binary Architecture
The application binary artifacts MUST be built strictly for the `x64` (`x86_64`) architecture; no 32-bit (`x86`) or ARM64 binaries SHALL be generated or distributed (DEC-005).
- **Verification Method**: Automated inspection of PE binary headers using `dumpbin /headers unduhkilat.exe` validating `IMAGE_FILE_MACHINE_AMD64`.

### NFR-003 — Arbitrary Concurrency Scaling
The download engine MUST support user-configured concurrent downloads and per-download connection counts with no hardcoded application limits (DEC-001), scaling up to the operating system's socket and file handle limits. Maximum sustained operational thresholds on reference hardware remain `TBD`.
- **Verification Method**: Stress testing the scheduler with `max_concurrent_downloads = 100` and `connections = 64` on range-capable test servers, verifying that TCP sockets are opened without internal clamping.

### NFR-004 — Crash Durability & Checkpoint Recovery
The engine MUST record transactional state checkpoints to the SQLite database during active transfers such that, following an ungraceful termination (`TerminateProcess`, task kill, or simulated power failure), 100% of interrupted downloads are restored to a resumable `PAUSED` state on the subsequent launch. The accepted post-checkpoint data-loss tolerance is [placeholder], and the checkpoint interval is `TBD` ms; bytes written after the last checkpoint are handled according to that tolerance.
- **Verification Method**: Injecting `SIGKILL` / `taskkill /F` during multi-gigabyte transfers across 100 iterations; validating that 100% of runs recover to `PAUSED`, that observed post-checkpoint data loss matches the tolerance recorded in [placeholder], and that final delivered files satisfy NFR-005.

### NFR-005 — Data Integrity Verification
In 100% of successful downloads (including fresh, paused/resumed, retried, and crash-recovered items), the assembled output file MUST be byte-identical to the upstream source file, verified against SHA-256 or MD5 hashes.
- **Verification Method**: Checksum validation comparing source test files against assembled files on disk across normal transfers, dropped-connection simulations, and crash-recovery cycles.

### NFR-006 — UI Responsiveness
The egui desktop interface MUST maintain real-time updates for transfer progress, instantaneous throughput, and ETA while transfers run under maximum load; the exact UI frame rate and progress refresh interval are `TBD`.
- **Verification Method**: Profiling UI frame times under active multi-connection workloads, ensuring no main-thread blocking occurs during I/O operations.

### NFR-007 — Bandwidth Throttle Precision
Measured transfer throughput under active global or per-download bandwidth limits MUST conform to the user-configured speed limit within an acceptable tolerance percentage (`TBD` %) measured over a rolling sampling window (`TBD` ms).
- **Verification Method**: Measuring network interface throughput during throttled transfers (e.g., limit set to 1000 KB/s) using packet capture tools.

### NFR-008 — Process Startup Latency
Cold and warm startup time of the desktop application from execution invocation to interactive UI presentation SHALL be `TBD` ms.
- **Verification Method**: Benchmark testing measuring elapsed milliseconds from process launch to the first rendered eframe frame.

### NFR-009 — Memory Footprint (Idle)
The resident memory consumption (working set) of the application while idle with no active transfers SHALL be `TBD` MB.
- **Verification Method**: Performance counter monitoring via Windows Performance Monitor (`Process: Working Set - Private`).

### NFR-010 — Memory Allocation per Active Connection
The additional resident memory allocated per active download connection task SHALL NOT exceed `TBD` KB.
- **Verification Method**: Measuring heap delta during step-up connection scaling tests (from 1 connection to 64 connections).

### NFR-011 — Local Logging Security and Privacy
Log files MUST be retained strictly on the local file system with automatic file size rotation; rotation size, retained file count, and retention duration are `TBD`. Log files MUST NOT record plain-text user passwords or authorization tokens, and MUST NOT be transmitted to any external network location.
- **Verification Method**: Log file inspection verifying masking of sensitive auth headers; packet capture ensuring zero telemetry requests.

### NFR-012 — Zero-Installation Portability
The application MUST operate fully from a standalone extracted ZIP directory, leaving zero orphaned files outside its portable root, the standard local AppData store, and user registry paths (`HKCU`) upon folder deletion (DEC-007).
- **Verification Method**: File system and registry differential audits before execution and after folder deletion on a clean Windows VM.

### NFR-013 — Cryptographic Signature Verification
All self-update packages MUST be verified using digital signatures prior to archive extraction; packages with missing, mismatched, or untrusted signatures MUST be rejected with zero filesystem changes.
- **Verification Method**: Automated update tests presenting tampered, truncated, self-signed, and legitimately signed update archives to the updater module.

### NFR-014 — Supported Browser Reach
Browser link interception and the floating media detection panel MUST function on Google Chrome, Microsoft Edge, and Brave; compatibility testing with other browser engines is explicitly out of scope (DEC-006).
- **Verification Method**: End-to-end automated UI tests loading the Manifest V3 extension in Chrome, Edge, and Brave, verifying successful link capture and media candidate surfacing.

### NFR-015 — Network Protocol Conformance
HTTP, HTTPS, and FTP protocols MUST conform to RFC 7230, RFC 7233 (Range Requests), and RFC 959 (FTP), with TLS operations implemented through `rustls` to avoid external OpenSSL dependencies.
- **Verification Method**: Interoperability test matrix testing downloads against Nginx, Apache, IIS, and standard FTP daemons (vsftpd/ProFTPD).

### NFR-016 — Telemetry-Free Architecture
The application MUST NOT transmit any telemetry, usage analytics, crash reports, or operational heartbeats to external endpoints; network traffic SHALL be restricted exclusively to user-specified download URLs, user-configured proxies, and official signed update endpoints.
- **Verification Method**: Network firewall monitoring across all operational states logging and verifying all outbound destination IPs.

### NFR-017 — Credential Protection at Rest
Protection mechanisms for storing HTTP and FTP credentials at rest in the local SQLite database are `TBD`.
- **Verification Method**: Security audit of the state store file validating credential obfuscation or encryption.

---

## 5. Technical Constraints

### TC-001 — Target Platform & Architecture
The binary MUST be compiled natively for `x86_64-pc-windows-msvc` targeting Windows 10 (1809+) and Windows 11 only (DEC-005).

### TC-002 — Programming Language & Toolchain
All application logic, native messaging host binaries, and desktop UI code MUST be implemented in stable Rust using the MSVC toolchain; no C/C++ runtime dependencies outside the Windows SDK SHALL be introduced.

### TC-003 — UI Framework & Rendering Backend
The graphical user interface MUST be built with `egui` and `eframe` utilizing the `wgpu` rendering backend, providing native GPU-accelerated drawing and native system tray event handling.

### TC-004 — Asynchronous Runtime
Network concurrency, socket polling, timeouts, and segment task cancellation MUST be managed through the multi-threaded Tokio runtime (`tokio` with `rt-multi-thread`).

### TC-005 — HTTP & TLS Stack
Outbound HTTP/HTTPS client operations MUST be executed using `reqwest` backed by `rustls` for TLS termination, eliminating OpenSSL linkage or external DLL dependencies.

### TC-006 — State Storage Engine
State persistence MUST utilize an embedded SQLite database engine configured in WAL (Write-Ahead Logging) mode, residing in a single file on the local machine.

### TC-007 — Browser Integration Architecture
Browser integration MUST be implemented as a Manifest V3 extension communicating with a standalone Rust Native Messaging host executable via standard input/standard output (`stdin`/`stdout`) length-prefixed JSON protocol (DEC-006).

### TC-008 — Packaging and Distribution Format
Distribution MUST be packaged as a standalone portable ZIP archive requiring no installer executables, setup wizards, or administrative privileges for standard user operation (DEC-007).

---

## 6. Assumptions

### AS-001 — Single-User Execution Model
It is assumed that the application runs under a single interactive Windows user session accessing its own dedicated SQLite state database, with no multi-user concurrent database sharing over network paths.

### AS-002 — Upstream Range Request Support
It is assumed that high-speed multi-connection downloads depend on upstream server compliance with RFC 7233 (`Accept-Ranges: bytes`); non-compliant endpoints are expected to fall back to single-connection transfers (FR-005).

### AS-003 — Filesystem Write Permissions
It is assumed that the End User possesses read and write permissions to the application bundle directory, the local application data directory, and all configured download target paths.

### AS-004 — Local System Clock Accuracy
It is assumed that the host Windows system clock is reasonably synchronized to evaluate scheduled queue operations (FR-023) and generate timestamped log records.

### AS-005 — Windows Notification Facility
It is assumed that the Windows Action Center and Toast Notification subsystems are functional on the host machine to deliver completion toast notifications (FR-054).

### AS-006 — Browser Extension Deployment
It is assumed that the Chromium extension is installed into supported browsers by the user (either via developer mode unpacked loading or Web Store deployment) and that the Native Messaging host registry keys are readable by the browser process.

---

## 7. Requirement → Module Traceability Matrix / Matriks Traceability Requirement → Modul

| Requirement ID | Module / Component | Owning Specification Document |
|---|---|---|
| FR-001 | Engine / Queue Manager | `PRD.md`, `API.md` |
| FR-002 | Engine / Segment Dispatcher | `PRD.md`, `API.md` |
| FR-003 | Engine / Network Worker | `PRD.md`, `DATABASE.md` |
| FR-004 | Engine / Dynamic Splitter | `PRD.md` |
| FR-005 | Engine / Network Worker | `PRD.md` |
| FR-006 | Engine / Task Controller | `PRD.md`, `DATABASE.md` |
| FR-007 | Engine / Resume Controller | `PRD.md`, `DATABASE.md` |
| FR-008 | Engine / Connection Watchdog | `PRD.md` |
| FR-009 | Engine / Task Controller | `PRD.md`, `DATABASE.md` |
| FR-010 | State Manager / Recovery Engine | `PRD.md`, `DATABASE.md` |
| FR-011 | Engine / Integrity Verifier | `PRD.md` |
| FR-012 | Engine / Protocol Adapter (FTP/HTTP) | `PRD.md` |
| FR-013 | Engine / HTTP Client | `PRD.md` |
| FR-014 | Security / Credential Vault | `PRD.md`, `DATABASE.md` |
| FR-015 | Network / Proxy Manager | `PRD.md`, `DATABASE.md` |
| FR-016 | Storage / File Allocator | `PRD.md` |
| FR-017 | Engine / HTTP Client | `PRD.md`, `API.md` |
| FR-018 | Queue / Priority-FIFO Scheduler | `PRD.md`, `DATABASE.md` |
| FR-019 | Queue / Item Controller | `PRD.md`, `DATABASE.md` |
| FR-020 | Queue / Concurrency Dispatcher | `PRD.md` |
| FR-021 | Queue / Reorder Controller | `PRD.md`, `DATABASE.md` |
| FR-022 | Queue / Batch State Controller | `PRD.md` |
| FR-023 | Scheduler / Timer Service | `PRD.md`, `DATABASE.md` |
| FR-024 | Scheduler / Background Runner | `PRD.md` |
| FR-025 | Throttle / Global Rate Limiter | `PRD.md` |
| FR-026 | Throttle / Per-Item Rate Limiter | `PRD.md` |
| FR-027 | Throttle / Rate Arbitrator | `PRD.md` |
| FR-028 | Throttle / Dynamic Controller | `PRD.md` |
| FR-029 | Category / Routing Engine | `PRD.md`, `DATABASE.md` |
| FR-030 | Category / Routing Engine | `PRD.md`, `DATABASE.md` |
| FR-031 | Category / Manager | `PRD.md`, `DATABASE.md` |
| FR-032 | UI / Add Download Dialog | `PRD.md` |
| FR-033 | Browser / Manifest V3 Extension | `PRD.md`, `API.md` |
| FR-034 | Installer / Registry Configurator | `PRD.md` |
| FR-035 (Admin) | Admin / Machine Registry Tool | `PRD.md` |
| FR-036 | Browser / Navigation Interceptor | `PRD.md`, `API.md` |
| FR-037 | Browser / Cookie Extractor | `PRD.md`, `API.md` |
| FR-038 | Browser / Native Messaging Host | `PRD.md`, `API.md` |
| FR-039 | UI / Confirmation Modal | `PRD.md` |
| FR-040 | System / Clipboard Monitor | `PRD.md` |
| FR-041 | Browser / Media Detector (Hybrid) | `PRD.md`, `API.md` |
| FR-042 | Browser / Media Deduplicator | `PRD.md` |
| FR-043 | Browser / Floating UI Panel | `PRD.md`, `API.md` |
| FR-044 | Browser / Stream Manifest Sniffer | `PRD.md` |
| FR-045 | Tools / Batch Downloader | `PRD.md` |
| FR-046 | Tools / Site Grabber Crawler | `PRD.md` |
| FR-047 | Queue / Item Ingestion Pipeline | `PRD.md` |
| FR-048 | UI / Main Table View | `PRD.md` |
| FR-049 | UI / Multi-URL Add Dialog | `PRD.md` |
| FR-050 | UI / Settings View | `PRD.md`, `DATABASE.md` |
| FR-051 | UI / System Tray Service | `PRD.md` |
| FR-052 | UI / Window Lifecycle Manager | `PRD.md` |
| FR-053 | Application / Shutdown Coordinator | `PRD.md`, `DATABASE.md` |
| FR-054 | System / Windows Toast Service | `PRD.md` |
| FR-055 | UI / Segment Inspector | `PRD.md` |
| FR-056 | UI / Diagnostics & Log Viewer | `PRD.md` |
| FR-057 | Logging / Rotating Log Service | `PRD.md` |
| FR-058 | Storage / SQLite State Engine | `PRD.md`, `DATABASE.md` |
| FR-059 | Distribution / Portable Bundle | `PRD.md` |
| FR-060 | Security / Update Verifier | `PRD.md` |
| FR-061 (Admin) | Admin / Bundle Updater | `PRD.md` |
| FR-062 (Admin) | Admin / Data Reset Tool | `PRD.md`, `DATABASE.md` |