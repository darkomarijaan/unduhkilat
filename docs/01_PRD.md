# PRD: UnduhKilat

## Executive Summary & Product Vision

UnduhKilat is a **Windows 10 and Windows 11 (x64 only)** desktop download manager built in **Rust (stable, MSVC toolchain)** with **egui/eframe on the wgpu renderer**, a **Tokio multi-thread async runtime**, **reqwest + rustls** as the HTTP client, and a **SQLite state file** for crash-durable download and segment bookkeeping (DEC-005). The product targets functional parity with Internet Download Manager: dynamic multi-connection segmented downloads, pause/resume/restart that survives connection loss and process kill, a queue with a scheduler, global and per-download bandwidth caps, automatic file-type categories with destination folders, Chromium-class browser capture (Chrome, Edge, Brave only, via a Manifest V3 extension plus a registry-registered native messaging host — DEC-006), a hybrid media sniffer plus extension hook that populates a floating panel over the page (DEC-003), a Site Grabber for pattern-based batch downloads, HTTP/HTTPS/FTP with proxy, authentication and cookie/session support, and an egui shell with a download list, add-URL dialog, settings screen, tray icon and toasts. The decisive user decisions shape the product more than any default: **the user decides the number of concurrent downloads and connections per download with no application-imposed ceiling** (DEC-001), **queue dispatch is manual per-item priority first and FIFO for the remainder** (DEC-002), **closing the window hides to tray and downloads keep running, with a toast when a download completes** (DEC-004), and **the application ships as a portable ZIP with no installation** (DEC-007), which is why the native messaging host registers itself per user on first run instead of relying on an installer. Everything a user configures is persisted so that a hard kill loses at most one checkpoint, and the next launch recovers every interrupted download into a resumable state.

## Problem Statement & Target Users

Users who routinely move large files on Windows — installers, ISO images, media, archives, dataset dumps — hit the same three failures with browsers and minimal downloaders: a single TCP connection that cannot saturate the link, a download that restarts from zero after a dropped connection or a closed application, and no way to control *when*, *how fast*, and *in what order* files arrive. Browser-native download UX also hides the interesting controls (connection count, segment behaviour, destination rules, credentials) behind nothing at all, and browser media pages make it hard to grab the actual audio/video resource rather than the page.

UnduhKilat addresses this with an explicitly user-governed engine (DEC-001), a deterministic dispatch rule the user can override per item (DEC-002), durable state that survives crash and restart, and a shell that stays out of the way: it lives in the tray and only speaks up with a toast when work finishes (DEC-004). Because the target is a **portable ZIP** (DEC-007), the same folder can run from a USB stick or a locked-down corporate desktop with no installer and no elevation — but it still reaches Chromium-class browsers through a per-user native messaging registration (DEC-006).

Target users:

- **Heavy downloader / power user** on a personal Windows 10/11 x64 machine who wants IDM-equivalent control: connection counts, per-item priority, bandwidth caps, categories, scheduler.
- **Media collector** who needs to identify the real audio/video stream behind a web player and hand it to a segmented downloader.
- **Batch/mirror user** who needs Site Grabber to pull every link matching a pattern from a page or a shallow site walk.
- **Restricted-environment user** (no admin rights, no installer allowed) who needs a portable binary that still integrates with the browser.

## System Scope & User Roles

| Role | Permissions | Description |
|---|---|---|
| End User | create, configure, pause, resume, restart, cancel, delete, reorder, prioritize, and monitor downloads; set concurrency, connection count, and bandwidth limits; define categories, destination folders, proxy, credentials, and schedules; enable or disable clipboard monitoring; hide the window to the tray; open the diagnostics panel and export logs | The single local interactive user of the portable application on the machine. Owns all downloads, all configuration, and all local state. |
| Browser Extension | submit captured URLs and download requests; stream detected media candidates and page metadata; receive download status updates for its own requests; render the floating media panel in the page | The Manifest V3 Chromium extension (Chrome, Edge, Brave) acting on the End User's behalf inside the browser (DEC-006). It cannot change application settings or control downloads the End User started outside the browser. |
| Administrator | register the native messaging host machine-wide for all users; replace or upgrade the portable bundle in place; reset the application data directory | The Windows user holding elevation, needed only for machine-wide registration and for maintaining the bundle folder on a shared or managed machine (DEC-007 removes the installer, not the need for a privileged path). |

## Functional Requirements

### Download Engine

**FR-001** – The End User can set the maximum number of simultaneously running downloads to any positive integer, the application enforces exactly that number and no hidden ceiling, and the value persists across restarts (DEC-001).

**FR-002** – The End User can set the number of parallel connections for a single download to any positive integer with no application-imposed upper bound, both as a per-download value and as a value applied to new downloads, and both persist across restarts (DEC-001).

**FR-003** – A download started against a server that answers with `Accept-Ranges: bytes` is split into as many byte-range segments as the configured connection count, each segment is fetched with its own HTTP range request on its own connection, and the segments are written into the target file at their offsets.

**FR-004** – While a segmented download runs, the engine re-splits the least-advanced segment and hands part of its remaining range to one of the connections already configured for that download, so a re-split never opens an additional connection and the number of connections for the download never exceeds the configured connection count (FR-002, FR-003); the split thresholds, minimum segment size, and re-split frequency are TBD and must be configurable in the settings screen.

**FR-005** – When a server does not advertise byte-range support or rejects a range request, the download falls back to a single-connection transfer, completes successfully, and the download list visibly shows that segmentation was unavailable.

**FR-006** – Pausing a download cancels its in-flight requests, flushes the received bytes and the highest contiguous offset of every segment to the state store, and leaves the item in a paused state that survives an application restart.

**FR-007** – Resuming a paused download continues every segment from its persisted offset with a new range request, never re-downloading bytes already on disk, and the resulting file is byte-identical to the source resource.

**FR-008** – When a connection drops mid-transfer, the engine retries and resumes that segment from the last received byte without user action; retry count, backoff schedule, and the give-up condition that moves the item to an error state are TBD.

**FR-009** – The End User can restart a download from zero, which discards persisted segment offsets and truncates the partial file before transferring again.

**FR-010** – If the process is killed or the machine loses power, the next launch restores every in-progress download to a resumable paused state with its segment offsets intact, and the End User can resume all of them with one action.

**FR-011** – On completion the application verifies the final file size against the expected size derived from `Content-Length` or the completed range set, and an item with a mismatched size is marked as failed rather than completed.

**FR-012** – Downloads over HTTP and HTTPS are supported, and downloads over FTP are supported including resume through the FTP restart (`REST`) command.

**FR-013** – HTTP redirects are followed to the final resource, the effective URL is recorded on the item, and the `Referer` used by the original request is preserved for servers that require it.

**FR-014** – The End User can supply credentials per download and per host, stored so they can be reused for later downloads from the same host, and the application sends them when the server challenges the request.

**FR-015** – Proxy configuration supports direct connection, system proxy, and manual host/port with optional credentials, applies to all downloads and to browser-integration requests, and persists across restarts.

**FR-016** – When a target file already exists, the application never silently overwrites it; the conflict resolution policy (automatic rename versus prompt) is TBD, and the final path shown in the download list always matches the file actually written.

**FR-017** – Cookies, `User-Agent`, and `Referer` captured from a browser or from a previous session can be attached to a download so that authenticated or session-bound resources transfer successfully (DEC-006).

### Queue & Scheduler

**FR-018** – Download dispatch orders the queue by a manually assigned per-item priority first and by FIFO position within equal priority (DEC-002). Whether priority is a numeric scale or a named set of levels, and how many levels exist, is [placeholder] and must be fixed before FR-019 and KPI-004 can be tested.

**FR-019** – The End User can raise, lower, or clear the priority of any queued item at any time, and the change takes effect at the next dispatch decision without restarting the queue.

**FR-020** – The number of running downloads never exceeds the configured concurrency limit, and items above the limit hold a queued state and start automatically when a running item finishes, is paused, or is cancelled.

**FR-021** – The End User can reorder queued items, and the manual order defines the FIFO position used by FR-018.

**FR-022** – "Start all" and "stop all" act on the entire queue: "start all" moves queued items to running only up to the configured concurrency limit (FR-020) and leaves the remaining items queued, while "stop all" moves every item to paused; both preserve each item's persisted progress.

**FR-023** – The End User can define schedule entries that start or stop the queue at a chosen time or within a chosen daily window, and the schedule is persisted and evaluated while the application runs.

**FR-024** – Scheduled start and stop actions fire while the main window is hidden in the tray and while no user is interacting with the application (DEC-004).

### Bandwidth Control

**FR-025** – The End User can set a global download speed limit in KB/s that caps the aggregate throughput of all running downloads.

**FR-026** – The End User can set a per-download speed limit in KB/s that caps that item independently of the global limit.

**FR-027** – When both limits apply, the item's effective throughput is the lower of the two, and the measured rate reported in the download list reflects the throttle.

**FR-028** – Changing a global or per-download limit takes effect on already running downloads within the next throttle interval without pausing or restarting them, and a value of zero means unlimited.

### Categories & Destinations

**FR-029** – The application ships with file categories mapped to destination folders, assigns a new download to a category from its file extension, and saves the file directly into that category's folder.

**FR-030** – A resource whose extension is unknown or whose `Content-Type` does not map to a category is placed in a fallback category whose folder is configurable.

**FR-031** – The End User can create, rename, and delete categories and change each category's destination folder, and the change applies to downloads started afterwards without affecting files already saved.

**FR-032** – The Add URL dialog and the download's properties allow overriding the category and the destination folder for that single item.

### Browser Integration

**FR-033** – A Manifest V3 extension is provided for Chromium-class browsers and is functional on Chrome, Edge, andBrave, which are the only browsers in scope; no other browser family is required to work (DEC-006).

**FR-034** – On first run the application registers the native messaging host for the current Windows user under `HKCU` in the registry, pointing at the host binary inside the portable bundle, so browser integration works without elevation and without an installation step (DEC-006, DEC-007).

**FR-035 (Admin)** – The Administrator can register the native messaging host machine-wide under `HKLM` so that every Windows user of the machine can use browser capture without running the application first, and the host path survives an in-place bundle replacement (DEC-006).

**FR-036** – The extension intercepts navigations and link activations that point at a downloadable resource and hands the URL plus the request's cookies, `Referer`, and `User-Agent` to the application, which then opens the download confirmation dialog (FR-039) (DEC-006).

**FR-037** – The extension can hand the full cookie set for the target host, and the application can attach those cookies to the download so session-authenticated resources transfer successfully (DEC-006, FR-017).

**FR-038** – The application pushes status changes for downloads initiated from the browser back to the extension, and the extension displays that status for its own requests using the state names defined in the download state machine document.

**FR-039** – Whenever a download is proposed from the browser, from the clipboard, or through the Add URL dialog, a confirmation dialog shows the URL, proposed filename, known size, category, and destination, allows the End User to edit them, and starts nothing unless the End User confirms.

**FR-040** – The End User can enable or disable clipboard URL monitoring; while it is enabled, copying a URL opens the confirmation dialog prefilled with that URL, and no download ever starts from the clipboard without explicit confirmation (DEC-006).

**FR-041** – Media detection is hybrid: the application accepts media candidates both from network request sniffing during the page session and from page-level hook signals emitted by the extension, and the floating panel presents the union of the two candidate sets (DEC-003).

**FR-042** – A media candidate found by only one of the two detection sources still appears in the panel, candidates are deduplicated by resolved URL, and each entry shows URL, media type, and any size discoverable from the response (DEC-003).

**FR-043** – The extension renders a floating panel over the web page of the active tab listing that tab's candidates, and the End User can send [placeholder] to the application for a segmented download from the panel (DEC-003). Which candidates are sendable is undecided: either only candidates that do not resolve to a manifest-based stream, or also manifest-based candidates, in which case their handling follows FR-044 (TBD) and does not produce a segmented download.

**FR-044** – Candidates that resolve to a manifest-based stream (for example DASH or HLS) are listed in the panel like any other candidate, and their download handling is TBD (DEC-003).

**FR-045** – The End User can request a batch download of every link on a page that matches a URL pattern containing wildcards, preview the matched links before anything starts, and then enqueue or discard the preview.

**FR-046** – The End User can run a depth-limited site walk from a start URL with include and exclude patterns, preview the matched links, and enqueue the selection; the maximum depth is user-configurable and its default value is TBD.

**FR-047** – Every link produced by a batch job or a site walk becomes an ordinary queue item that obeys the concurrency limit (FR-020), the priority-then-FIFO dispatch rule (FR-018), the category mapping (FR-029), and the bandwidth limits (FR-025, FR-026).

### Desktop UI, Tray and Notifications

**FR-048** – The download list shows per item the name, size, status, progress, current speed, estimated remaining time, category, destination path, and priority, updates while transfers run, and supports sorting and filtering by status and category.

**FR-049** – The Add URL dialog accepts one or more URLs at once and lets the End User set category, destination, connection count, priority, credentials, and referer for the whole batch before creating one queue item per URL.

**FR-050** – The settings screen exposes concurrency, connections per download, global and per-download speed limits, categories and destination folders, proxy, credentials, schedule entries, clipboard monitoring, tray and notification behaviour, browser integration state, and log level, and every change persists across restarts.

**FR-051** – A tray icon is present for the whole lifetime of the running application, and its menu shows or hides the window, starts the queue, stops the queue, and exits the application (DEC-004).

**FR-052** – Closing the main window hides it to the tray and leaves every running download progressing; the application terminates only through the explicit exit action in the tray menu or the application menu (DEC-004).

**FR-053** – On explicit exit, in-progress downloads are checkpointed and marked so the next launch restores them as resumable (FR-010), and the End User is told that downloads will stop.

**FR-054** – A toast notification is shown when a download completes, can be switched off, and fires while the window is hidden in the tray (DEC-004).

**FR-055** – A per-item details view shows each segment's byte range, its last checkpointed offset, and the item's active connection count, matching what the state store holds.

**FR-056** – An in-application diagnostics panel shows the tail of the current log file, the last error recorded for each download, and the live connection count per download, and can export the log files to a folder chosen by the End User.

**FR-057** – The application writes rotating local log files at a configurable level and never transmits log or diagnostic data off the machine; rotation size, file count, retention window, and export format are TBD.

### Durable State and Portability

**FR-058** – All mutable state — downloads, segment offsets, queue order, priorities, categories, settings, schedule, and stored credentials — is persisted to one local SQLite file through transactional checkpoints taken while transfers run, so that a process kill leaves a store the next launch can read consistently.

**FR-059** – The application runs from an extracted ZIP folder with no installer and no elevation for normal operation, writes nothing outside its own folder, its state and log locations, and its `HKCU` native-host registration keys, and is removed by deleting the folder (DEC-007).

**FR-060** – Update packages for the bundle are signature-verified before any file is replaced, an unsigned or invalidly signed package is rejected with a visible error, and a valid update replaces the portable bundle in place without an installer (DEC-007).

**FR-061 (Admin)** – The Administrator can replace the whole portable bundle in a shared or protected folder with a newer signed bundle, after which machine-wide native-host registration resolves to the host binary at the new path.

**FR-062 (Admin)** – The Administrator can reset the application data directory, which clears the state store and the logs after an explicit confirmation and leaves already downloaded files untouched.

## Non-Functional Requirements

| Category | Target |
|---|---|
| Supported OS | Windows 10 x64 and Windows 11 x64 only; no other version is supported or tested (DEC-005). |
| Architecture | x64 build only; no x86 or ARM64 artifact is produced (DEC-005). |
| Concurrency | No application-imposed ceiling on simultaneous downloads or on connections per download; any positive value the End User sets is honoured (DEC-001). Maximum sustained values on reference hardware: TBD. |
| Crash durability | After `TerminateProcess` or power loss during transfer, the next launch restores 100% of in-progress items to a resumable state with their checkpointed offsets; at most one checkpoint interval of progress is lost; the checkpoint interval is TBD. |
| Data integrity | A resumed, retried, or crash-recovered download produces a file byte-identical to the source in 100% of recovery test runs (FR-011). |
| UI responsiveness | Progress, speed, and remaining time refresh live while transfers run; the exact refresh interval is TBD. |
| Throttle accuracy | Measured throughput respects the effective limit; the numeric tolerance and sampling window are TBD (FR-025–FR-028). |
| Startup time | TBD |
| Idle memory | TBD |
| Memory per active connection | TBD |
| Logging | Local rotating files only, no remote transmission; rotation size, file count, and retention are TBD. |
| Portability | Runs from an extracted ZIP folder; no installer and no elevation for normal operation (DEC-007). |
| Update integrity | 100% of update packages signature-verified before applying; invalid packages rejected (FR-060). |
| Browser reach | Chrome, Edge, and Brave only (DEC-006). |
| Network protocols | HTTP, HTTPS, and FTP, with optional HTTP proxy and rustls-based TLS (FR-012, FR-015). |
| Telemetry | None; no central server is contacted beyond update packages and the End User's own download URLs. |
| Localization | TBD |
| Accessibility | TBD |
| Credential protection at rest | TBD |

## Technology Stack & Rationale

| Component | Technology | Rationale |
|---|---|---|
| Language | Rust (stable, MSVC toolchain) | Required by the product idea and gives a native Windows binary with no additional runtime. |
| GUI | egui/eframe with wgpu renderer | Specified by the idea; eframe supplies the window and update loop needed to render real-time progress. |
| Async Runtime | Tokio multi-thread | Dozens of concurrent HTTP connections per download demand a mature async scheduler and task cancellation on pause (FR-006, DEC-001). |
| HTTP Client | reqwest + rustls | Needs Range, proxy, cookie, and TLS support across source servers; rustls avoids an OpenSSL dependency on Windows (FR-003, FR-015, FR-017). |
| State Database | SQLite (single file) | One PC per user with no server, so a local file with transactions is sufficient for download and segment state (FR-058). Per DEC-007 the bundle is portable, so the single-file store stays authoritative while the exact location rule remains TBD (FR-059); the store location is [placeholder] until that rule is decided. |
| Browser Integration | Manifest V3 extension + native messaging host via Windows registry | Automaticlink capture and the floating video panel need a two-way channel that clipboard monitoring alone cannot provide (DEC-003, DEC-006, FR-034).
| Distribution | Portable ZIP bundle + signed auto-update | The End User runs the product without installation (DEC-007), so the bundle is shipped as a ZIP and refreshed in place only by signature-verified update packages; the installer path listed in the stack is superseded by DEC-007 and is not built (FR-059, FR-060). |
| Logging & Diagnostics | Local rotating log files + in-application diagnostics panel | There is no central server, so connection and resume problems must be inspectable on the End User's own machine (FR-056, FR-057). |

## Success Metrics & KPIs

All targets below are derived from the functional requirements. Numeric thresholds that the product idea and decisions do not fix are `TBD` and must be set before the corresponding requirement is accepted.

| KPI | Definition | Derived from | Target |
|---|---|---|---|
| KPI-001 Segmented transfer achieved | Share of range-capable downloads that ran with more than one connection at some point | FR-001, FR-003, FR-004 | TBD |
| KPI-002 User-configured concurrency honoured | Share of runs where observed running downloads never exceeded the configured limit and reached it when enough items were queued | FR-001, FR-020 | 100% |
| KPI-003 User-configured connection count honoured | Share of range-capable downloads that opened exactly the configured number of connections at steady state and never exceeded it, including after a re-split | FR-002, FR-003, FR-004 | TBD |
| KPI-004 Dispatch correctness | Share of dispatch decisions that matched the recorded priority-then-FIFO order | FR-018, FR-019, FR-021 | 100% |
| KPI-005 Crash recovery rate | Share of killed-mid-transfer sessions where every in-progress item was restored to resumable state on the next launch | FR-010, FR-058 | 100% |
| KPI-006 Checkpointed bytes re-downloaded after pause/resume | Share of already-checkpointed bytes fetched again after pause and resume | FR-006, FR-007 | 0% |
| KPI-007 File integrity | Share of completed downloads byte-identical to the source, including resumed, retried, and crash-recovered items | FR-011 | 100% |
| KPI-008 Fallback success | Share of non-range or range-rejecting sources that still completed successfully on a single connection | FR-005 | 100% |
| KPI-009 Throttle accuracy | Deviation between the configured effective limit and measured throughput over the sampling window | FR-025, FR-026, FR-027, FR-028 | TBD |
| KPI-010 Browser capture reach | Share of capture attempts that succeeded on Chrome, Edge, and Brave | FR-033, FR-036 | 100% per supported browser |
| KPI-011 Confirmation compliance | Share of browser, clipboard, and dialog-proposed downloads that started only after explicit confirmation | FR-039, FR-040 | 100% |
| KPI-012 Hybrid detection yield | Share of media candidates surfaced by exactly one of the two detection sources that still reached the floating panel | FR-041, FR-042 | TBD |
| KPI-013 Tray continuity | Share of close-to-tray events where every running download kept progressing and finished | FR-052, FR-054 | 100% |
| KPI-014 Completion toast delivery | Share of completed downloads that raised a toast while the window was hidden, when toasts are enabled | FR-054 | TBD |
| KPI-015 Category routing | Share of downloads saved to the folder of their assigned category without manual relocation | FR-029, FR-032 | 100% |
| KPI-016 Batch funnel | Share of previewed batch and site-walk links that became queue items obeying the concurrency limit and dispatch order | FR-045, FR-046, FR-047 | 100% |
| KPI-017 Portable operation | Share of first runs that completed from an extracted folder with no installer and no elevation | FR-059 | 100% |
| KPI-018 Update integrity | Share of unsigned or invalidly signed update packages rejected without modifying the bundle | FR-060 | 100% |
| KPI-019 Diagnostics availability | Share of reported connection or resume problems diagnosable from the in-app panel and exported logs alone | FR-056, FR-057 | TBD |

KPI-006 measures pause and resume only. Crash recovery (FR-010) is explicitly out of its scope: the crash durability NFR allows at most one checkpoint interval of progress to be lost, so bytes received after the last checkpoint may legitimately be fetched again after a crash and are excluded from both the numerator and the denominator of KPI-006.

## Risk Analysis & Mitigation

| Risk | Impact | Mitigation |
|---|---|---|
| Unlimited user-chosen concurrency and connection counts (DEC-001) exhaust sockets, file handles, or memory and degrade the whole machine | High | Enforce exactly the configured values (FR-001, FR-002) but surface live connection counts per download and in aggregate in the diagnostics panel (FR-056) so the End User can see the cost of the setting; record the pre-change value so a runaway setting is recoverable; default values remain TBD |
| Servers that advertise `Accept-Ranges` but throttle or reject many parallel range requests | Medium | Fall back to a single connection and mark the item accordingly (FR-005); keep per-host behaviour visible in the item details (FR-055) |
| Resumed or re-split segments write at wrong offsets and corrupt the file | High | Persist offsets transactionally before and after writes (FR-058), verify final size against the expected size (FR-011), and treat mismatch as failure instead of completion |
| Hard kill or power loss between checkpoints loses segment state | High | Checkpoint during transfers rather than only on pause (FR-058); restore every in-progress item as resumable on next launch (FR-010); keep the loss window to one checkpoint interval |
| Native messaging host registration fails or drifts after the portable folder moves (DEC-007) | High | Register per user on first run (FR-034), provide machine-wide registration for Administrators (FR-035), and expose browser integration state in the settings screen (FR-050) so the End User can detect a broken link |
| Chromium extension store policy or browser version changes restrict native messaging or the floating panel | Medium | Scope is limited to Chrome, Edge, and Brave (DEC-006); keep clipboard monitoring and the Add URL dialog as independent capture paths (FR-039, FR-040) so capture never depends on the extension alone |
| Hybrid detection surfaces duplicate or wrong media candidates (DEC-003) | Medium | Deduplicate by resolved URL and show type and known size per entry (FR-042); nothing is downloaded without confirmation (FR-039, FR-043) |
| Manifest-based streams (DASH/HLS) appear as candidates but cannot be downloaded as a single file (FR-044) | Medium | List them in the panel for transparency and treat their handling as TBD rather than silently producing a broken download |
| Close-to-tray behaviour (DEC-004) confuses users who expect the process to end | Medium | Toast and tray menu make the running state visible (FR-051, FR-054); exit is explicit and checkpoints before terminating (FR-053) |
| Throttling is inaccurate under many connections, so the configured limit is not respected | Medium | Apply limits at the item and aggregate level with the effective value being the lower of the two (FR-027); measure and report actual rate (KPI-009) |
| Credentials and cookies stored locally are readable by other processes on the machine | High | Keep the store local and off any network path (FR-058, FR-057); protection mechanism at rest is TBD and must be specified before release |
| Portable bundle replaced or partially extracted leaves a mixed-version folder (DEC-007) | Medium | Verify signature before replacing any file (FR-060) and allow Administrators to replace the whole bundle (FR-061) |
| Site Grabber or site walk multiplies into thousands of queue items | Medium | Always preview matched links before enqueueing (FR-045, FR-046) and route them through the normal concurrency limit and dispatch order (FR-047) |

## Constraints & Assumptions

**Constraints**

- Target platform is Windows 10 and Windows 11, x64 only; no other Windows version and no other architecture is supported (DEC-005, FR-059).
- The interface is built with Rust and egui/eframe with the wgpu renderer; the HTTP client is reqwest with rustls; async execution is Tokio multi-thread; state lives in a single local SQLite file.
- There is no application-imposed ceiling on concurrent downloads or on connections per download; only the End User's configuration bounds them (DEC-001).
- Dispatch order is fixed to manual per-item priority first, then FIFO; no other automatic dispatch policy is offered (DEC-002).
- Media detection must be hybrid, covering both sniffing and extension hooks; a single-source design is not acceptable (DEC-003).
- Browser reach is limited to Chromium-class Chrome, Edge, and Brave; no other browser family is required (DEC-006).
- The product is distributed and operated as a portable ZIP with no installation (DEC-007); the installer path named in the technology stack is not built.
- There is no central server and no telemetry; log and diagnostic data stay on the End User's machine (FR-057).
- Downloads must never start from the browser, the clipboard, or a batch preview without explicit confirmation (FR-039, FR-040, FR-045).
- Normal operation must not require elevation; only machine-wide host registration, bundle replacement, and data reset are Administrator actions (FR-035, FR-061, FR-062).

**Assumptions**

- Each Windows user runs one instance of the application against one local state store, and the store is not shared between machines or users.
- Source servers are reachable over the End User's own network, and any proxy, credentials, cookies, or referer needed are supplied by the End User or captured from the browser.
- Range-capable servers accept the number of parallel range requests the End User configures, subject to the fallback in FR-005.
- The Chromium extension and the native messaging host are updated together as part of the same portable bundle.
- Downloaded files remain on local or mounted storage that stays available between pause and resume.
- The End User's machine provides the media playback context the floating panel overlays; the application does not render web pages itself.
- Time, schedule evaluation, and toasts rely on the local system clock and the Windows notification facility, whose availability is assumed rather than guaranteed.
- Update packages are produced and signed by the project; the End User is assumed to obtain them from that channel.

## Out of Scope

- **Installer-based or MSI distribution and any setup wizard.** Rejected by DEC-007; the product is a portable ZIP, and the installer path named in the technology stack is not implemented (FR-059).
- **Firefox, Safari, Opera, Vivaldi, and every other non-Chromium browser.** Excluded by DEC-006 (FR-033).
- **Windows 7, Windows 8/8.1, x86 builds, and ARM64 builds.** Excluded by DEC-005.
- **An application-imposed maximum on simultaneous downloads or connections per download, and any automatic cap the End User cannot override.** Rejected by DEC-001 (FR-001, FR-002).
- **Automatic dispatch policies other than manual priority plus FIFO — for example size-based, oldest-first, or speed-based ordering.** Rejected by DEC-002 (FR-018).
- **Sniffing-only media detection and hook-only media detection as standalone modes.** Rejected by DEC-003; both sources are required together (FR-041).
- **Completing a manifest-based stream (DASH/HLS) into a single merged media file.** Postponed; only candidate listing is specified and the handling remains TBD (FR-044).
- **Exit-on-close and minimize-only window behaviour that stops transfers.** Rejected by DEC-004; closing hides to tray and downloads continue (FR-052).
- **Any remote or cloud service: account system, sync of downloads and settings between machines, central log collection, and telemetry.** No server component exists (FR-057).
- **Scheduling driven by an external service or by wake-from-sleep guarantees.** The scheduler is local and evaluates while the application runs (FR-023, FR-024).
- **Elevation for normal daily operation.** Only the Administrator actions in FR-035, FR-061, and FR-062 run with elevation.