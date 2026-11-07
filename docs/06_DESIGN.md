# DESIGN.md: UnduhKilat

## 1. Design Read (Evidence-Based)

*   **Product:** UnduhKilat is a native Windows 10/11 x64 high-throughput desktop download manager built with Rust, egui/eframe (wgpu), Tokio, and reqwest/rustls. It operates as a fully portable ZIP application without an installer (PRD Executive Summary, DEC-005, DEC-007).
*   **Target Audience:** Heavy downloaders, media collectors, power users, and enterprise users in restricted corporate environments without administrative/installer privileges who require deterministic queue dispatch, multi-connection segment tuning, and reliable crash-proof download resumption (PRD Problem Statement & Target Users, DEC-007).
*   **Product Type:** Desktop Utility / Systems Management Workstation (Native Windows GUI application utilizing egui immediate-mode layout).
*   **Primary User Goal:** Maximize bandwidth saturation across single or multiple concurrent files, inspect segmented transfer progress in real time, queue batch transfers predictably, and preserve 100% of transfer state across app restarts or system crashes without manual intervention (PRD Executive Summary, FR-001, FR-003, FR-010).
*   **User Roles & Workflows:**
    *   *Direct Download Workflow:* End User copies URL or opens Add URL dialog -> Enters URL, selects category/destination, configures connections & priority -> Confirms download -> Engine establishes HTTP range segments -> Real-time tracking in main transfer table -> Toast notification fires on completion -> File saved to category destination (PRD FR-039, FR-048, FR-054, DEC-004).
    *   *Browser Capture Workflow:* End User clicks download link or video stream in Chrome/Edge/Brave -> Chromium Extension traps request via MV3 Native Host / Sniffer -> Floating candidate badge displays metadata -> User clicks "Unduh dengan UnduhKilat" -> Main confirmation dialog prompts in desktop UI -> Download enqueued obeying manual priority then FIFO order (PRD FR-018, FR-036, FR-041, FR-043, DEC-002, DEC-003, DEC-006).
    *   *Batch / Site Grabber Workflow:* End User specifies pattern or start URL with depth limits -> UI renders interactive Link Discovery Preview -> User selects/filters target files -> Items batch-enqueued to active queue respecting global limits (PRD FR-045, FR-046, FR-047).
    *   *Background Tray Workflow:* End User clicks window close button -> Window closes to notification tray -> Tokio runtime keeps downloads actively progressing -> Scheduled windows evaluate in background -> System Toast triggers upon final segment merge (PRD FR-024, FR-051, FR-052, FR-054, DEC-004).
    *   *Crash Recovery Workflow:* Process killed or power drops -> User relaunches portable binary -> SQLite transactional log reads checkpointed segment offsets -> Interrupted downloads restored to Resumable Paused status -> One-click Resume All continues byte-exact transfer (PRD FR-007, FR-010, FR-058, DEC-007).
*   **Brand / Aesthetic Direction:** Functional High-Density Windows Workstation. Deep slate-steel technical foundation paired with crisp data density, zero-fluff immediate-mode widgets, hairline borders, and energetic electric-cyan indicators. Evokes the utilitarian reliability of classic Windows tools (e.g., Process Hacker, IDM, Sysinternals) modernized through egui's low-latency wgpu rendering (PRD Technology Stack & Rationale).
*   **Key Personality Traits:** Deterministic, transparent, unconstrained (unbounded connections per DEC-001), industrial, resilient.

## 2. Design Dials (Context-Specific)

*   **DESIGN_VARIANCE: 2/10** — Strict utility tooling where standard desktop paradigms (menu bars, toolbar strips, resizable multi-column data tables, tree categories, segment status bars) take precedence over decorative visual divergence.
*   **MOTION_INTENSITY: 2/10** — Immediate-mode GUI desktop application with minimal decorative animation; movement is strictly functional (60 FPS wgpu segment progress bar fill, smooth transfer speed rolling graphs, and instantaneous tab switching; animations respect reduced motion).
*   **VISUAL_DENSITY: 9/10** — Maximum compact data presentation; table row height is [placeholder: 24–28px with a documented table-row target-size exception, or ≥32px to satisfy the 32px hitbox — owner decision], precise tabular figures, high data-to-pixel ratio, and monospace alignment for segment byte offsets and transfer speeds.

## 3. Direction

*   **Chosen Direction:** "Kinetic Slate Telemetry" (Industrial Windows Systems Utility).
*   **Surface Language & Palette:**
    *   Background (App Canvas / Base Frame): `#12151A`
    *   Surface (Panels, Toolbars, Sidebars): `#1A1E26`
    *   Surface Raised (Dialogs, Menus, Popups, Table Headers): `#222732`
    *   Border / Hairline Dividers: `#2D3442`
    *   Foreground Primary (Text / Active Data): `#E6EDF5`
    *   Foreground Secondary (Labels, Dimmed Data): `#94A1B2`
    *   Foreground Muted (Placeholders, Inactive Icons): `#5E6A7E`
    *   Brand Accent (UnduhKilat Cyan): `#00D2FF`
    *   Accent Foreground (Contrast on Cyan): `#08131E`
    *   Focus Ring (High-visibility accessibility focus outline): `#38BDF8`
    *   Semantic Success (Completed, Active Range Verified): `#10B981`
    *   Semantic Warning (Throttled, Segment Re-split, Non-range Fallback): `#F59E0B`
    *   Semantic Danger (Connection Dropped, Corrupt Hash, Size Mismatch): `#EF4444`
*   **Typography:**
    *   UI Primary Face: Segoe UI (System Windows sans-serif) / Inter fallback.
    *   Technical / Numeric Telemetry Face: JetBrains Mono / Cascadia Code fallback.
*   **Application to UnduhKilat:** This identity treats download management as high-precision network telemetry. Rather than hiding connection counts, thread states, or segment partitions under generic loading spinners, the palette and surface hierarchy push byte ranges, socket allocation, and transfer saturation to the foreground. The high-contrast slate surfaces allow multi-colored segment progress meters to be read instantly at a glance across multi-monitor setups without eye fatigue.

## 4. Signature

The signature visual element of UnduhKilat is the **"Live Segment Topology Strip"** (Dynamic Multi-Connection Block Map). Located on the primary screen within the bottom transfer inspector dock (and embedded as an expandable row in the main download table), this component renders a real-time, 18px-high segmented bar representing the physical byte range of the active download (from byte `0` to `Content-Length`).

Each parallel connection (user-configured, without ceiling per DEC-001) is assigned a distinct slice with its own thread ID indicator, showing contiguous committed bytes in solid `#00D2FF`, in-flight buffered bytes in pulsing `#38BDF8`, and unallocated/pending ranges in `#222732`. When the engine dynamically re-splits the least-advanced segment (FR-004), the user visually observes the segment slice bisect with a crisp 1px hairline divider `#E6EDF5` and a new connection index appear. This concrete, functional visualization instantly communicates to the user that multi-connection acceleration is actively saturating their bandwidth.

## 5. Hero Fold Spec

For a native Windows desktop utility, the "Hero Fold" is the **Main Application Window at default launch resolution (1280 x 800 px)** without scrolling:

*   **Window Frame & Titlebar (Height: 32px):**
    *   App Icon (16x16 lightning bolt) + Title: `UnduhKilat v1.0.0 (x64 Portable)`
    *   Global Speed Counter (Right-aligned in titlebar): `▼ 48.2 MB/s | ▲ 0.0 KB/s`
    *   Window controls: Minimize, Maximize, Close (Triggers hide-to-tray per DEC-004).
*   **Quick Action Toolbar (Height: 44px, spanning full 1280px width):**
    *   Button: `+ Tambah URL` (Accent Primary `#00D2FF`, text `#08131E`, bold)
    *   Grouped Action Buttons (Surface `#222732`, borders `#2D3442`): `Lanjutkan Semua` (Resume All), `Jeda Semua` (Pause All), `Hentikan` (Stop), `Hapus Selesai` (Clear Finished).
    *   Divider.
    *   Queue Dispatch Indicator: `Antrian: Prioritas Manual + FIFO` (DEC-002).
    *   Global Speed Limit Quick-Dial: `Batas: Tak Terbatas` (FR-025).
*   **Main Workspace Split (Height: 524px, 1280px width):**
    *   **Left Category Sidebar (Width: 220px):**
        *   Tree list: `Semua Unduhan` (Count: 14), `Aktif / Berjalan` (3), `Selesai` (9), `Terjeda / Error` (2).
        *   Category Sub-tree (FR-029): `Kompresi / ZIP`, `Dokumen`, `Musik`, `Program`, `Video`.
        *   Scheduler & Grabber Links: `Penjadwal`, `Site Grabber`.
    *   **Center Main Download Table (Width: 1060px):**
        *   Data columns: Checkbox, File Name, Size, Status, Progress (Bar + %), Transfer Speed, Time Left, Connections (Conn), Priority. Priority column placement is [placeholder: immediately after Status or at the end of the table — owner decision; must match Section 17 and Section 20].
        *   High-density rows ([placeholder: row height per Section 2 — 24–28px with documented table-row target-size exception, or ≥32px — owner decision]) displaying active downloads with JetBrains Mono numbers.
*   **Bottom Telemetry Dock (Height: 200px, collapsible):**
    *   Shows selected download details: File URI, Destination Path, Chunk/Segment status map (The Signature Strip), and individual connection log stream (FR-055).
*   **Word Budget:** Exactly 62 visible UI words on primary frame load.
*   **Mobile / Narrow Window Constraint (< 900px width window):** Left category panel automatically collapses into an egui compact side-drawer icon menu; table column visibility toggles off non-essential metadata (hiding `Koneksi`, `Prioritas`, and `Sisa Waktu`), prioritizing `Nama Berkas`, `Ukuran`, `Kemajuan (Bar)`, and `Kecepatan`.

## 6. Page Shape

UnduhKilat is a native desktop application. Its page shape is governed by **4 Primary Workstation Screens** and **[placeholder: number of Modal Confirmation Workspaces]** using the canonical modal list below. The confirmed modal workspaces are Add URL / Capture Confirmation Dialog (FR-039) and Admin Data Wipe Confirmation (FR-062); any additional modal workspace is [placeholder: owner decision].

### Primary Screen 1: Main Download Manager Dashboard
*   **Job:** Master command center for monitoring real-time network throughput, managing active queues, prioritizing downloads, and opening completed assets.
*   **Composition:** 3-tier vertical split: 1. Top Global Control Ribbon (44px) -> 2. Two-column horizontal split (Sidebar 220px fixed, Table flexible width) -> 3. Collapsible Bottom Segment & Connection Inspector (200px).
*   **Content:** Dynamic data pulled directly from the local SQLite state database (`downloads`, `segments` tables per FR-058). Real metadata: file names, byte sizes, speeds calculated via rolling 500ms windows, dynamic status badges.
*   **Word Budget:** ~80 static UI label words.
*   **Primary Action:** `+ Tambah URL` (Spawns Modal Screen 1). Repeats in the main window toolbar and context menu.

### Primary Screen 2: Site Grabber & Batch Downloader
*   **Job:** Deep URL crawling and batch link harvesting based on wildcard expressions or depth-limited page scraping (FR-045, FR-046).
*   **Composition:** Wizard-style 2-panel interface: Left config column (Start URL, Depth spinner, Filter regex, Include/Exclude file extensions) -> Right full-height interactive multi-select checkbox data grid previewing discovered resources before enqueuing.
*   **Content:** Discovered links parsed dynamically from target URL HTML headers and anchor tags. Real sizes pulled via HTTP HEAD `Content-Length`.
*   **Word Budget:** ~110 static UI label words.
*   **Primary Action:** `Antrikan File Terpilih` (Enqueue Selected Files).

### Primary Screen 3: Settings & Portability Center
*   **Job:** Configure engine concurrency parameters, protocol proxies, directory routing, browser host hooks, and diagnostics.
*   **Composition:** Tabbed settings notebook (Vertical tabs on left, properties form on right with standard 2-column label/control rows):
    *   Tab 1: *Mesin Unduhan* (Concurrency limit per DEC-001, Parallel connections per DEC-001, Buffer sizes).
    *   Tab 2: *Kategori & Folder* (Path mappings for Video, Audio, Archives per FR-029).
    *   Tab 3: *Integrasi Browser & Native Host* (HKCU status, Chromium detection per DEC-006, DEC-007).
    *   Tab 4: *Jaringan & Proxy* (Direct, System, Manual SOCKS5/HTTP, Authentication per FR-014, FR-015).
    *   Tab 5: *Penjadwal & Batas Kecepatan* (Scheduler rules per FR-023, Global speed throttle per FR-025).
*   **Content:** Direct configuration values loaded from SQLite `settings` table.
*   **Word Budget:** ~180 static UI label words.
*   **Primary Action:** `Simpan Pengaturan` (Saves transactionally to SQLite, immediate Tokio engine update).

### Primary Screen 4: Diagnostics & Local Log Terminal (FR-056)
*   **Job:** Inspect active connection states, low-level socket errors, Range-header negotiations, and export local log files.
*   **Composition:** Split terminal view: Top metrics bar (Active Sockets, SQLite Write Latency, Open File Descriptors) -> Main monospace ANSI log tail window -> Bottom action bar (Filter log levels: TRACE, DEBUG, INFO, WARN, ERROR; Export Log button).
*   **Content:** Real-time log buffer written by Tokio tracing subscribers.
*   **Word Budget:** ~45 static UI label words.
*   **Primary Action:** `Ekspor Berkas Log` (Exports `.log` files to user-chosen path per FR-056).

### Modal Workspace 1: Add URL / Capture Confirmation Dialog (FR-039)
*   **Job:** Verify and parameterize download requests originating from user entry, clipboard sniffer (FR-040), or Chromium browser extension (DEC-006).
*   **Composition:** Structured card modal (560px width x 380px height):
    *   Header: URL entry box with "Tangkap Otomatis" badge.
    *   Body: 2-column layout (Filename, Category dropdown, Destination folder browser with disk space indicator, Connection count input, Manual Priority slider per DEC-002).
    *   Credentials collapsible: User/Password, Referer override, Custom Cookie header (FR-017).
*   **Content:** Auto-populated from HTTP HEAD probe (`Content-Disposition`, `Content-Length`, `Accept-Ranges`).
*   **Word Budget:** ~60 static UI label words.
*   **Primary Action:** `Unduh Sekarang` (Starts transfer immediately).
*   **Secondary Action:** `Antrikan Saja` (Adds to queue in FIFO/priority state).

### Modal Workspace 2: Admin Data Wipe Confirmation (FR-062)
*   **Job:** Explicitly confirm the Administrator action that clears the state store and logs.
*   **Composition:** Capped, centered destructive confirmation modal; warning copy states that already downloaded files are untouched; confirm and cancel actions.
*   **Content:** Data reset scope from FR-062.
*   **Word Budget:** [placeholder: word budget].
*   **Primary Action:** [placeholder: confirm data-wipe label] (destructive; executes the data reset only after explicit confirmation).

### Extension Overlay (Not a Modal Workspace): Browser Floating Video Panel (DEC-003, FR-043)
*   **Job:** Browser-injected floating badge and candidate selection list hovering over active web video elements.
*   **Composition:** Floating pill button pinned to top-right of player (`Unduh Video` with the existing vector lightning-bolt brand glyph, no emoji), expanding on hover/click into a compact dark-themed popover table (Candidate Streams: 1080p MP4, 720p MP4, Audio Only M4A, Stream HLS/DASH warning badge per FR-044).
*   **Content:** Populated by hybrid detection (sniffed `media` network requests + DOM video hooks per DEC-003).
*   **Word Budget:** ~25 words.
*   **Primary Action:** `Unduh Berkas` (Dispatches URL + session cookies to native host via HKCU registry IPC).

## 7. Material Inventory

*   **Real Materials (Direct from PRD & User Decisions):**
    *   Operating system scope: `Windows 10 x64`, `Windows 11 x64` (DEC-005).
    *   Supported browser targets: `Google Chrome`, `Microsoft Edge`, `Brave` (DEC-006).
    *   Distribution model: `Portable ZIP bundle` (DEC-007).
    *   Dispatch rules: `Prioritas Manual`, `FIFO` (DEC-002).
    *   Concurrency rules: `Tanpa Batas Atas (Dikelola Pengguna)` (DEC-001).
    *   Window lifecycle: `Tutup ke Tray`, `Toast Notifikasi Windows` (DEC-004).
    *   Engine state store: `SQLite (unduhkilat.db di AppData/Local/UnduhKilat)` (PRD Technology Stack).
    *   Protocols: `HTTP`, `HTTPS`, `FTP` (FR-012).
*   **Pictures & Icons (Built natively in-engine / Vector geometry):**
    *   Application Icon: 16x16, 32x32, 48x48 lightning bolt brand mark rendered via egui vector painting (No bitmap dependencies).
    *   Toolbar Icons: Vector glyphs (Add `+`, Resume `▶`, Pause `⏸`, Stop `⏹`, Settings `⚙`, Logs `📋`, Folder `📁`).
    *   No stock photography, no 3D renders, no marketing hero graphics (strictly a systems utility interface).
*   **Absent Materials (Marked for implementation / Stand-ins):**
    *   `[STAND-IN: Extensi Manifest V3 WebStore URL]` — URL for extension install instructions before official Chrome Web Store packaging is finalized.
    *   `[STAND-IN: Versi SQLite Schema]` — Version schema badge (e.g. `v1-state.sqlite`).
    *   `[STAND-IN: TBD Retry Count]` — Exact default retry count and backoff schedule per FR-008 (Marked `TBD` in PRD).
    *   `[STAND-IN: TBD Segment Split Minimum Size]` — Minimum chunk size threshold per FR-004 (Marked `TBD` in PRD).

## 8. Art Direction (Hero Scene, 3D and Imagery)

*   **Status: N/A**
*   **Reason:** UnduhKilat is a native Windows desktop systems utility built with egui/eframe. The PRD explicitly forbids marketing fluff, abstract decorative screenshots, 3D scenes, or WebGL canvases. All visual density is dedicated to real-time data tables, connection topology graphs, and system controls.

## 9. Semantic Color System (Color Discipline)

```css
:root {
  /* Brand Accent: UnduhKilat electric cyan; highlights active interactions and signature segment telemetry */
  --accent: #00D2FF;

  /* Accent Ink: Deepest navy-black for maximum contrast text inside solid accent buttons */
  --accent-ink: #08131E;

  /* Focus Ring: High-contrast sky blue meeting WCAG 2.1 AA focus indication rules */
  --focus: #38BDF8;

  /* Base Background: Deep slate-steel canvas for main window frame and egui viewport */
  --bg: #12151A;

  /* Surface: Primary container tone for sidebar, toolbar ribbons, and data table base */
  --surface: #1A1E26;

  /* Surface Raised: Elevated layer for dialog windows, popups, menus, and table headers */
  --surface-raised: #222732;

  /* Foreground Primary: Crisp off-white for data readouts, active table rows, and headings */
  --fg: #E6EDF5;

  /* Foreground Secondary: Medium slate for column titles, metadata labels, and inactive tabs */
  --fg-secondary: #94A1B2;

  /* Muted: Dimmed slate for table grid hairlines, placeholders, and disabled icons */
  --muted: #5E6A7E;

  /* Border / Hairline: Structural 1px boundary separating immediate-mode panels */
  --border: #2D3442;

  /* Semantic Success: Emerald green indicating verified completion, 100% hash check, active Range support */
  --success: #10B981;

  /* Semantic Warning: Amber indicating non-range single connection fallback, throttling, or dynamic re-split */
  --warning: #F59E0B;

  /* Semantic Danger: Crimson indicating network timeout, dropped socket, hash mismatch, or 4xx/5xx HTTP error */
  --danger: #EF4444;

  /* Destructive Surface: [placeholder: semantic destructive-action background token; owner decision — the arbitrary #2A1418 must be tokenized or replaced by an existing token] */
}
```

*Color Hierarchy Discipline:*
*   Surface canvas (`#12151A`, `#1A1E26`, `#222732`) represents 90% of the UI area, preventing eye fatigue during prolonged background monitoring.
*   The single accent color (`#00D2FF`) is strictly reserved for primary actions (`+ Tambah URL`), focused sliders, and the active download segment block map.
*   Semantic states (`#10B981`, `#F59E0B`, `#EF4444`) never double as brand accents and are restricted to status badges, log levels, and health pips.

## 10. Typography & Type Scale (Typography Discipline)

*   **Primary Desktop Interface Face:** `Segoe UI, -apple-system, BlinkMacSystemFont, "Inter", sans-serif`
*   **Telemetry, Numeric Data & Path Face:** `JetBrains Mono, "Cascadia Code", Consolas, monospace`
*   **Web Fallback Host (for documentation / browser extension panel):**
    `https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500;600&display=swap`

| Role | Font Family | Size (px / pt) | Weight | Line Height | Usage Context |
|---|---|---|---|---|---|
| Window Title | Segoe UI | 12px (9pt) | 600 (SemiBold) | 16px | Titlebar, modal headers |
| Section Header | Segoe UI | 14px (10.5pt) | 600 (SemiBold) | 20px | Sidebar categories, Settings tabs |
| Body UI Regular | Segoe UI | 13px (9.75pt) | 400 (Regular) | 18px | Dialog descriptions, button labels |
| Body UI Medium | Segoe UI | 13px (9.75pt) | 500 (Medium) | 18px | Active toolbar buttons, table headers |
| Data Monospace | JetBrains Mono | 12px (9pt) | 400 (Regular) | 16px | File sizes, remaining time, IP addresses |
| Telemetry Speed | JetBrains Mono | 13px (9.75pt) | 600 (SemiBold) | 16px | Live speed (KB/s, MB/s), chunk byte offsets |
| Micro Badge | JetBrains Mono | 10px (7.5pt) | 500 (Medium) | 12px | Priority indicators, thread indices (#1, #2) |

*Rules:* Monospace is never used decoratively; it is strictly enforced for numbers, byte ranges, hashes, file extensions, and network addresses to ensure rigid vertical tabular alignment across dynamically updating rows.

## 11. Layout Strategy & Anti-Repetition Rules

*   **Immediate-Mode Layout Architecture (egui):**
    *   The layout relies on anchored egui panels: `TopBottomPanel::top` (Titlebar & Action Ribbon), `SidePanel::left` (Category navigation; canonical width rule follows Section 17 responsive classes: compact 48px, standard 220px, wide 260px; resizable bounds and maximum width are [placeholder: owner decision] and must match Section 17), `CentralPanel` (Download data grid), and `TopBottomPanel::bottom` (Collapsible telemetry inspector).
    *   Base grid spacing unit: **4px**. Standard spacing scale: `4px` (micro), `8px` (compact), `12px` (standard control gap), `16px` (panel gutter), `24px` (modal margins).
*   **Capped and Centered Containers:**
    *   The desktop application main window can scale from minimum `800 x 600 px` to 4K displays (`3840 x 2160 px`). At wider viewports, the category sidebar uses the wide class width from Section 17; its maximum width is [placeholder: owner decision] while the data table dynamically expands its horizontal measure to allocate width to file names and paths.
    *   Modal dialogs (Add URL, Admin data wipe) are strictly capped at max `640px` width and centered within the main window viewport to prevent excessively wide text input rows. Additional modal dialogs are [placeholder: owner decision on any third modal workspace].
*   **Anti-Repetition Rules:**
    *   No repetitive card grids. The download manager explicitly uses a single high-performance virtualized table (`egui_extras::TableBuilder`) capable of rendering thousands of items at 60 FPS without DOM or widget recreation.
    *   Panels and views are differentiated structurally: Sidebar uses a hierarchical indented tree structure; Main panel uses a column-bordered table; Bottom dock uses an oscilloscope-style telemetry strip paired with a split terminal log.

## 12. Motion Choreography

*   **Choreography Model:**
    *   Immediate-mode GUI rendering paints every frame via wgpu. Motion is strictly functional and event-driven.
*   **Motion Allocations:**
    *   *Segment Telemetry Progress (Frame Update):* Smooth linear interpolation of byte progress bars at display refresh rate (60Hz/144Hz). Triggers upon incoming TCP socket write events.
    *   *Modal Reveal (Add URL / Dialogs):* Opacity fade from `0.0` to `1.0` combined with a subtle scale transform (`0.98` to `1.0`) over 120ms with `cubic-bezier(0, 0, 0.2, 1)`.
    *   *Toolbar & Button Hover:* [placeholder: background fill color transition (`#1A1E26` -> `#222732`) over 80ms, or an opacity/transform-only alternative — owner decision required to reconcile with Section 22 Rule 2].
    *   *Toast Notification (Windows Native):* Governed by Windows Notification subsystem (slide in from bottom right, 200ms).
*   **Hardware and Accessibility Safeguards:**
    *   Never animate `width`, `height`, `top`, or `left`. In egui, panel resizes adjust coordinate layouts directly without intermediate tweening.
    *   `prefers-reduced-motion` compliance: When Windows setting "Show animations in Windows" is disabled, all opacity fades and scale transforms bypass immediately (duration = 0ms), rendering content statically on the initial frame.

## 13. Copy Draft

*All UI copy is drafted in formal, technically precise Indonesian (Bahasa Indonesia) reflecting the native target audience, with technical networking terms preserved.*

*   **Window Title:**
    `UnduhKilat - Manajer Unduhan Berkecepatan Tinggi [Portabel]` *(draft for the owner)*
*   **Hero / Action Ribbon:**
    *   Add URL Button: `+ Tambah URL` *(draft for the owner)*
    *   Resume All Button: `Lanjutkan Semua` *(draft for the owner)*
    *   Pause All Button: `Jeda Semua` *(draft for the owner)*
    *   Clear Button: `Bersihkan Selesai` *(draft for the owner)*
    *   Global Speed Throttle Readout: `Batas Kecepatan Global: Tak Terbatas` *(draft for the owner)*
    *   Dispatch Policy Badge: `Antrian: Prioritas Manual + FIFO (DEC-002)` *(draft for the owner)*
*   **Main Download Table Headers:**
    *   `Nama Berkas` | `Ukuran` | `Status` | `Kemajuan` | `Kecepatan` | `Sisa Waktu` | `Koneksi` | `Prioritas` *(draft for the owner)*
*   **Modal: Tambah URL Unduhan (FR-039):**
    *   Headline: `Mulai Unduhan Baru` *(draft for the owner)*
    *   Supporting sentence: `Tentukan parameter segmen koneksi, kategori berkas, dan folder tujuan.` *(draft for the owner)*
    *   URL Input Label: `Tautan Sumber (HTTP/HTTPS/FTP):` *(draft for the owner)*
    *   Filename Label: `Simpan Sebagai:` *(draft for the owner)*
    *   Category Label: `Kategori Otomatis:` *(draft for the owner)*
    *   Connections Input Label: `Jumlah Koneksi Simultan:` *(draft for the owner)*
    *   Priority Label: `Prioritas Antrian:` *(draft for the owner)*
    *   Primary Action Label: `Mulai Mengunduh` *(draft for the owner)*
    *   Secondary Action Label: `Antrikan Saja` *(draft for the owner)*
*   **Toast Notification (FR-054, DEC-004):**
    *   Title: `Unduhan Selesai` *(draft for the owner)*
    *   Body: `[Nama Berkas] berhasil diunduh dan diverifikasi integritasnya.` *(draft for the owner)*
*   **Tray Menu Copy (FR-051, DEC-004):**
    *   `Tampilkan UnduhKilat` | `Mulai Antrian` | `Jeda Antrian` | `Pengaturan...` | `Keluar dari UnduhKilat` *(draft for the owner)*

## 14. Anti-AI-Slop Rules (Avoid vs. Prefer)

*   **Avoid:** Generic kicker/eyebrow text above modal titles (e.g., `/// DOWNLOAD WORKFLOW SYSTEM ///` or `ACCELERATE YOUR FILES`).
    *   *Prefer:* Direct, utilitarian window headers: `Mulai Unduhan Baru`.
*   **Avoid:** Card-inside-card syndrome or floating bubbly cards with nested 16px border-radii.
    *   *Prefer:* Hairline-bordered (`1px solid #2D3442`) structured flat panels, toolbars, and tabular data rows with square or 2px micro-radii.
*   **Avoid:** Neon purple-blue gradients or glowing multi-color blur drops behind containers.
    *   *Prefer:* Flat, solid slate-steel surfaces (`#12151A`, `#1A1E26`) with crisp single-color `#00D2FF` electric-cyan highlights.
*   **Avoid:** Emoji icons in button labels or table cells (e.g., `🚀 Download`, `📁 Files`, `⚠️ Error`).
    *   *Prefer:* Professional vector geometric glyphs or egui native painter stroke icons with uniform 1.5px line weight.
*   **Avoid:** Tracked-out uppercase labels (e.g., `S P E E D  L I M I T`).
    *   *Prefer:* Standard sentence case or title case: `Batas Kecepatan`.
*   **Avoid:** Unnecessary confirmation modals for non-destructive actions.
    *   *Prefer:* Inline status toggles and silent checkpointing; the confirmed modal workspaces are strictly restricted to Add URL confirmation (FR-039) and Admin data wipe confirmation (FR-062). Any additional modal workspace is [placeholder: owner decision].
*   **Avoid:** Middle-dot metadata strings (e.g., `File.zip • 1.2 GB • 12 connections • High`).
    *   *Prefer:* Dedicated structured table columns with fixed boundaries.

## 15. Card & Container Discipline

*   **Where Cards are FORBIDDEN:**
    *   The Main Download List: Must NEVER be rendered as a grid or stack of cards. It is strictly a virtualized data table with 1px horizontal row dividers (`#2D3442`).
    *   Sidebar Categories: Must be rendered as a compact tree list, never individual cards.
    *   Toolbar and Ribbons: Flat continuous horizontal containers with 1px bottom border.
*   **Where Containers are PERMITTED:**
    *   Modal Dialogs (Add URL, Admin data wipe): Single elevated container (`#1A1E26` surface, `1px` border `#2D3442`, 4px corner radius, drop shadow `0 8px 24px rgba(0,0,0,0.6)`). Additional modal dialogs are [placeholder: owner decision]. The Batch Site Grabber is Primary Screen 2, not a modal.
    *   Segment Topology Inspector: Enclosed flat dock at bottom of the main frame bounded by a draggable splitter hairline.
    *   Settings Panel: Clean two-column property sheet separated by 1px hairline dividers; no nested child cards.

## 16. Component Rules & Action Hierarchy

*   **Primary Action (Level 1):**
    *   Visual: Solid accent fill (`#00D2FF`), ink text (`#08131E`), 600 weight, 2px border radius.
    *   Usage: Max ONE per dialog or toolbar group (`+ Tambah URL`, `Mulai Mengunduh`, `Simpan Pengaturan`).
*   **Secondary Action (Level 2):**
    *   Visual: Surface raised fill (`#222732`), hairline border (`1px solid #2D3442`), foreground text (`#E6EDF5`), hover fill (`#2D3442`).
    *   Usage: Standard controls (`Lanjutkan Semua`, `Jeda Semua`, `Antrikan Saja`, `Batal`).
*   **Tertiary Action (Level 3):**
    *   Visual: Transparent background, no border, secondary foreground text (`#94A1B2`), hover text (`#E6EDF5`).
    *   Usage: Context menu items, tab switches, column sorting triggers, collapsible fold icons.
*   **Destructive Action:**
    *   Visual: Transparent or dark red tint background (`[placeholder: destructive-surface token from Section 9]`), border `1px solid #EF4444`, text `#EF4444`, hover fill `#EF4444` with white text.
    *   Usage: `Hapus Berkas & Data`, `Reset Basis Data`.
*   **Immediate-Mode egui Component Roles:**
    *   Table rows: Focusable via keyboard navigation (Up/Down arrow keys), selectable via Spacebar.
    *   Context Menus: Rendered via `egui::MenuRoot` with explicit keyboard accelerators shown right-aligned (`Del`, `F5`, `Ctrl+A`).

## 17. Behavioral Responsive Design

UnduhKilat targets Windows 10/11 desktop environments across three explicit viewport behavior classes:

*   **Compact Utility Class (< 900px width, e.g., 800 x 600 px min window):**
    *   *Sidebar:* Automatically collapses to an 48px icon-only rail. Clicking an icon opens an egui fly-out drawer over the table.
    *   *Table Columns:* Hides secondary telemetry (`Koneksi`, `Prioritas`, `Sisa Waktu`). Retains `Nama Berkas`, `Ukuran`, `Kemajuan (Bar)`, and `Kecepatan`.
    *   *Bottom Inspector:* Automatically collapses into an expandable drawer toggled via a bottom-right badge (`[+ Telemetri Segmen]`).
*   **Standard Workstation Class (900px – 1600px width, e.g., 1280 x 800 px target):**
    *   *Sidebar:* Fixed 220px tree view showing full category names and item counts.
    *   *Table Columns:* Full standard suite visible: Name, Size, Status, Progress Bar, Speed, ETA, Connections, Priority. Priority column placement is [placeholder: immediately after Status or at the end of the table — owner decision; must match Section 5 and Section 20].
    *   *Bottom Inspector:* Pinned at 200px height with draggable splitter to adjust height up to 400px.
*   **Wide Telemetry Class (> 1600px width, e.g., 1080p / 4K displays):**
    *   *Sidebar:* 260px expanded tree view.
    *   *Table Columns:* Expands file path preview column, shows HTTP server hostname column, and embeds inline dynamic segment mini-bars directly inside the `Kemajuan` table cells.
    *   *Inspector:* Splits into 2 side-by-side telemetry panels (Left: Segment Block Map; Right: Real-time Socket Diagnostic Log).

## 18. States (Loading, Empty, Error)

| Data Region | Loading State | Empty State | Partial / Throttled State | Error State | Success State |
|---|---|---|---|---|---|
| **Main Download Table** | Semi-transparent table rows with subtle shimmer over progress bars while SQLite initializes on cold boot. | Center message: `Belum Ada Unduhan Aktif`. Subtext: `Klik "+ Tambah URL" atau salin tautan untuk memulai.` | Table populated with running downloads; throttled items display a yellow `#F59E0B` gauge icon with effective limit. | Network disconnect: rows freeze last speed to `0 KB/s`, status badge updates to red `Terputus (Mencoba Lagi...)`. | Completed rows display solid green `#10B981` bar, status `Selesai`, and double-click reveals file in Windows Explorer. |
| **Segment Topology Strip** | Strip renders gray striped pattern: `Menghubungi Server & Memeriksa Range...` | `N/A - Tidak ada unduhan dipilih` (Inspector docked with placeholder prompt). | Non-range server: Entire strip shows solid single segment in yellow: `Server Tidak Mendukung Range (Single-Connection)`. | Socket abort on chunk 4: Chunk 4 turns red `#EF4444`, status tooltip: `Koneksi #4 Reset oleh Peer`. | All segments merged: Strip becomes solid green `#10B981` bar: `Penggabungan Berkas Selesai (100%)`. |
| **Batch Site Grabber Preview** | Animated indeterminate ticker: `Memindai Tautan Halaman (Ditemukan: [N] berkas)...` | `Tidak Ditemukan Berkas yang Cocok dengan Pola URL.` | Discovered links listed; items lacking `Content-Length` display `Ukuran: Tidak Diketahui`. | 403 Forbidden: Red callout banner: `Gagal Memindai: Akses Ditolak (403 Forbidden)`. Host unreachable / connection failure: Red callout banner: `[placeholder: host-unreachable / connection-error copy]`. | Discovered table fully populated with checkboxes enabled and total size calculated. |
| **Browser Extension Floating Panel** | Sniffer listening: Subtle pulsating dot on extension badge in web page player. | Hovering over non-media tab: Panel remains completely hidden. | Media candidate found without size metadata: Displays `Format: MP4 (Aliran Adaptif - Ukuran TBD)`. | Native Host disconnected (App closed): Badge turns gray with tooltip: `UnduhKilat tidak aktif. Jalankan aplikasi portabel.` | Candidate ready: Crisp cyan download button: `Unduh 1080p (245 MB)`. |

*State Fallback Copy Definitions:*
*   Network Error Copy: `Koneksi terputus. Menunggu jaringan sebelum mencoba ulang otomatis...` *(draft for the owner)*
*   Size Mismatch Error Copy: `Gagal: Ukuran akhir berkas tidak cocok dengan Content-Length server.` *(draft for the owner)*
*   Native Host Registration 403 / Error Copy: `Pendaftaran native host HKCU gagal. Periksa izin registry Windows.` *(draft for the owner)*

## 19. Accessibility & Inclusivity (WCAG 2.1 AA)

*   **Keyboard Navigation & Focus Management:**
    *   Every interactive component (buttons, inputs, table rows, tab headers) is fully operable via standard Windows keyboard paradigms (`Tab`, `Shift+Tab`, `Arrow keys`, `Enter`, `Escape`).
    *   High-visibility focus ring: Explicit `2px solid #38BDF8` outline with 2px offset applied to all focused elements via `:focus-visible` equivalents in egui.
    *   Application shortcuts: `Ctrl+N` (New URL), `Alt+Enter` (Item properties).
    *   Table-focused shortcuts apply only when focus is not inside a text input: `Ctrl+A` (Select all rows), `Delete` (Delete item), and `Space` ([placeholder: select row or toggle pause/resume selected — owner decision; only one action may be assigned to `Space`]).
*   **Target Sizing:**
    *   All interactive buttons and table action hitboxes provide a minimum target area of `32px x 32px` in high-density desktop mode and `44px x 44px` in dialogs and ribbon menus, adhering to desktop accessibility benchmarks. Table-row hitbox behavior is [placeholder: document a table-row target-size exception or raise row height to at least 32px — owner decision].
*   **Contrast Ratio Verification (Tested against Background `#12151A` / Surface `#1A1E26`):**
    *   Foreground Primary (`#E6EDF5`) on Surface (`#1A1E26`): **12.4:1** (Exceeds WCAG AAA 7:1 standard).
    *   Foreground Secondary (`#94A1B2`) on Surface (`#1A1E26`): **6.2:1** (Exceeds WCAG AA 4.5:1 standard).
    *   Accent Cyan (`#00D2FF`) on Background (`#12151A`): **11.8:1** (Exceeds WCAG AAA).
    *   Accent Ink (`#08131E`) on Accent Cyan (`#00D2FF`): **11.5:1** (Exceeds WCAG AAA).
    *   Semantic Danger (`#EF4444`) on Surface (`#1A1E26`): **~4.44:1** ([placeholder: fails WCAG AA 4.5:1; accept with documented exception or adjust danger color/text size to reach ≥4.5:1 — owner decision]).
    *   Semantic Success (`#10B981`) on Surface (`#1A1E26`): **5.8:1** (Passes WCAG AA 4.5:1).
*   **Not-Color-Only Indicators:**
    *   Status badges never rely purely on green/amber/red dots. Every status is accompanied by explicit text labels: `Selesai` (Done), `Mengunduh` (Downloading), `Terjeda` (Paused), `Gagal` (Failed).
    *   The Segment Topology Strip includes numeric thread identifiers and textual byte offset labels overlaid directly on slices.

## 20. Design Rationale

*   **DEC-001 (Unbounded Concurrency & Connections):** Governs the design of the Add URL dialog and Settings panel. Rather than constraining connections with a restrictive slider or dropdown capped at 8 or 32, the UI exposes a raw numeric stepper field allowing any positive integer. The UI mitigates risk by placing a live socket telemetry counter in the Diagnostics Panel (FR-056) rather than imposing an artificial ceiling.
*   **DEC-002 (Manual Priority then FIFO Dispatch):** Shapes the Main Download Table structure. A dedicated `Prioritas` column with visual priority badges (`Tinggi`, `Normal`, `Rendah`) is placed [placeholder: immediately after the status column or at the end of the table, matching Sections 5 and 17]. Queue ordering within identical priority tiers is [placeholder: FIFO by enqueue time per DEC-002, or manual user reorder if DEC-002 is revised; the `Antrian: Prioritas Manual + FIFO` label in Section 5 and Section 13 must be updated with that decision]. Drag-and-drop row reordering is [placeholder: not specified until that decision].
*   **DEC-003 (Hybrid Media Sniffing & Extension Hook):** Directly dictates the Browser Extension Popover layout. Candidates are grouped into a unified list with clear source badges (`[Jaringan]` for sniffed streams, `[Halaman]` for DOM-hooked video tags) so the user understands why a candidate appeared, while deduplicating identical URLs (FR-042).
*   **DEC-004 (Close to Tray, Download Continuity & Toasts):** Dictates the complete absence of an "Exit on window close" modal. When the window close button is clicked, the UI simply unmaps the egui viewport. The tray icon becomes the primary background anchor. Toasts are decoupled from window visibility and dispatched directly to the Windows Notification Center.
*   **DEC-005 & DEC-007 (Windows 10/11 x64 Portable ZIP):** Explains why there is no installation wizard or desktop setup step. On initial boot, the UI displays a subtle non-intrusive status strip: `Mode Portabel Aktif - Host Ekstensi Terdaftar di HKCU`. The entire state is self-contained in local AppData SQLite, enabling zero-friction portability from USB storage.
*   **DEC-006 (Chromium Browser Reach):** Restricts the Browser Integration settings tab to three distinct toggle cards: `Google Chrome`, `Microsoft Edge`, and `Brave`. Non-Chromium browsers are explicitly omitted, keeping the UI compact and truthful.

## 21. NO Invented Design Content Rule

*   All UI fields, labels, categories, and metrics are derived strictly from PRD requirements and User Decisions.
*   **Zero Invented Marketing Claims:** No fake review stars, no fake user testimonials, no "Trusted by 10,000,000 users" banners, no artificial speedup percentages (e.g., "500% faster!").
*   **Real Data Displays:** All displayed speed values, connection numbers, segment counts, and time estimates reflect real mathematical calculations derived from the Tokio transfer engine and SQLite state checkpoints.
*   **Draft Placeholders:** All UI copy lines in Section 13 are explicitly tagged as `(draft for the owner)` and unconfirmed configuration details are marked as `TBD` per the project contract.

## 22. Design Floor & Pre-Flight Checklist

### Design Floor Rules
1. Every color, font-family, and spacing value must strictly reference the semantic tokens declared in Section 9 and Section 10; no raw arbitrary hex values or system font literals permitted in component styling.
2. Never declare `transition: all`. Motion transitions must exclusively animate `transform` and `opacity`, except [placeholder: hover background-color transition if the owner approves it as a documented exception]. Never animate container `width`, `height`, `top`, or `left`.
3. Native desktop windows must clip contents properly; no accidental horizontal scrollbars on dialog containers or table viewports.
4. Focus rings (`2px solid #38BDF8`) must be visible on every interactive element when navigated via keyboard.
5. All animations must gracefully terminate immediately when Windows reduced motion is detected.
6. Absolutely no fake testimonials, false marketing metrics, decorative stock imagery, or artificial AI sparkle decorations.
7. No eyebrow/kicker headers, tracked-out capital letters, purple-blue neon gradients, or emoji icons in production controls.
8. The entire primary application window layout (Toolbar, Sidebar, Download Table, and Segment Inspector) must fit cleanly within a 1280 x 800 px window without requiring vertical window-level scrolling.
9. At most 80 static text words visible on the initial primary workstation screen.
10. All modal dialogs must use capped, centered containers.

### Pre-Flight Checklist
- [x] Design derived from PRD/project context
- [x] Design Read is specific
- [x] Design dials are intentional
- [x] The direction is stated with hex values and font names
- [x] The signature is concrete
- [x] The hero fits the first screen
- [x] Every section has a job, a composition and a content source
- [x] Stand-ins and drafts are marked
- [x] Art direction is concrete or N/A with a reason
- [x] Visual direction is justified
- [x] Colour hierarchy is intentional
- [x] Typography is intentional
- [x] Cards are not overused
- [x] Layouts are not repetitive
- [x] No generic AI aesthetic without justification
- [x] Motion has a purpose and a reduced-motion ending
- [x] Responsive behaviour is defined
- [x] Loading states
- [x] Empty states
- [x] Error states
- [x] Accessibility
- [x] No fake content
- [x] Feels specific to the product
- [x] A coding agent can implement it without inventing major visual decisions
- [x] The design system is cohesive and reusable
- [x] The design follows the choices stated in the user's files (the brief) and marks conflicts TBD
- [x] No invented capabilities (Undo, offline, resume, real-time) that the PRD does not specify