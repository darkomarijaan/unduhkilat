# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

UnduhKilat is a Windows 10/11 x64-only (DEC-005) portable download manager in Rust (egui/eframe 0.37 UI). Product docs, comments, UI strings, and commit messages are in Indonesian; commits follow `feat(scope): ...` / `chore: versi X.Y.Z`. Specs live in `docs/` (start at `docs/18_README.md`); product decisions are `DEC-nnn` in `docs/16_DECISIONS.md` and tasks are `TASK-nn` in `docs/17_TASKS.md`. Code comments cite them (FR-xxx, DATABASE.md §x), so check there before changing behavior.

## Commands

```
cargo build --release                      # target/release/UnduhKilat.exe + unduhkilat-host.exe
cargo test --workspace                     # Rust tests
cargo test -p unduhkilat-core --test engine            # one integration test file
cargo test -p unduhkilat-core --test engine <name>     # one test
cargo test -p unduhkilat-core --test search -- --ignored --nocapture   # real-network tests are #[ignore]
node --test extension/tests                # browser-extension logic tests (Node only for this)
```

Useful env vars when running the app: `UNDUHKILAT_DATA_DIR` (isolated data), `UNDUHKILAT_NO_REGISTER`, `UNDUHKILAT_NO_AUTOLAUNCH`, `UNDUHKILAT_START_SCREEN=settings|search|grabber|diagnostics|add`. The exe also has a headless mode: `UnduhKilat.exe cli <url|magnet|.torrent> [--connections N] [--out DIR]`.

Release: use the `rilis` skill (`.claude/skills/rilis/SKILL.md`), which runs `python scripts/release.py X.Y.Z --notes <file> --trailer ...`. The script bumps `[workspace.package] version`, builds release, writes `dist/UnduhKilat-portable-X.Y.Z-x64.zip` (`packaging/BACA-SAYA.txt`, both exes, empty `unduhkilat-portable.marker`, `extension/` without `tests`), the `.sha256`, and the `.sig` (`unduhkilat-sign sign` self-verifies and checks the key matches `PUBLIC_KEY_B64`), commits `chore: versi X.Y.Z`, pushes, and creates the GitHub pre-release on `darkomarijaan/unduhkilat`. The in-app updater (`core/src/update.rs`) reads GitHub releases (pre-releases included), downloads assets via the API first with retries, and refuses any zip whose Ed25519 signature does not verify against `PUBLIC_KEY_B64`. The private key (`~/.unduhkilat-signing/release.key`) is never in the repo.

## Architecture

Cargo workspace of four crates:

- **`unduhkilat-core`** is a library with all logic and no UI. Its main parts:
  - `engine/` is the download engine. `manager.rs` is the queue (priority, then FIFO; no upper bound on concurrency, DEC-001/002). `transfer.rs` runs one transfer: probe, plan segments, spawn workers, checkpoint to SQLite, verify. `http.rs`, `ftp.rs`, `remote.rs` (SFTP/FTP/FTPS), and `torrent.rs` (librqbit) are the protocol back ends. `throttle.rs` and `proxy.rs` cover speed limits and proxies. `search.rs` and `grabber.rs` handle file search and site grabbing.
  - `server/` stores SSH/SFTP/FTP/FTPS server profiles. Secrets are DPAPI-encrypted (`dpapi.rs`, `server/secrets.rs`); it also handles jump hosts and host-key pinning.
  - `term/` holds the terminal emulator and sessions (local shell and SSH).
  - `preview/` renders text, Markdown, image, PDF, and Office previews without downloading the whole file.
  - `db.rs`, `repo.rs`, and `schema.sql` are the SQLite layer (rusqlite, bundled). `error.rs` defines `AppError`/`ErrorCode`, with stable `ERR_*` codes surfaced to the UI and the extension. Never overwrite an existing file; the contract is `ERR_NAME_COLLISION`.
- **`unduhkilat-app`** is the egui desktop app (bin name `UnduhKilat`, Windows GUI subsystem). `app.rs` is the main shell and screens, `explorer.rs` and `explorer/` are the two-pane file explorer (local plus server panes, copy between servers, sortable table), `servers.rs`, `terminal.rs`, `tray.rs`, `cli.rs`, and `boot.rs` (single-instance guard, host registration).
- **`unduhkilat-host`** is a tiny Native Messaging host that Chromium launches for the extension. It talks to the running app over a per-user, token-protected named pipe (`core/src/bridge.rs`, `ipc.rs`, `protocol.rs`, `native.rs`).
- **`unduhkilat-sign`** is the release-signing CLI (`keygen`, `sign`).
- **`extension/`** is a Manifest V3 Chromium extension (shared logic in `shared.js`, tested with `node --test`).

Cross-cutting flows to know before editing:

- **Engine to UI is event-driven.** The engine pushes `EngineEvent::{Progress, StateChanged}` over a `tokio::sync::broadcast` channel (`engine/mod.rs`). The UI and the browser bridge both subscribe, and egui reads state from the DB and snapshots rather than calling into transfers.
- **Server file transfers reuse the download queue.** Upload, download, and server-to-server copy of files and whole folders are enqueued as ordinary downloads (`engine/remote.rs`, `explorer/pair.rs`, with `PASTE_BIT` in `explorer.rs`). They therefore get pause, resume, speed limits, and progress for free. Anything showing copy progress should derive it from those queue items, not from a separate channel. Server transfers run in their own dispatch lane (`server_concurrency_limit`, default 6), separate from the download limit, and lease connections from `server/pool.rs` (at most `PER_SERVER` per server, kept 60 s, multi-connection leases reserved in sorted order to avoid deadlock) instead of logging in per file. A folder of many small files (`folder_stream::prefer_stream`) between two servers that can `exec` `tar` becomes ONE `server-tree://` item: `tar -cf -` piped into `tar -xf -` into a destination folder the item creates itself (it never writes into an existing folder; otherwise it falls back to per-file items). The copy pop-up weights such an item by its file count.
- **Resume offset convention:** a segment's persisted offset is the NEXT byte to fetch, so resume requests `bytes={offset}-{range_end}` (see `engine/transfer.rs`).
- **Portable vs installed data:** if `unduhkilat-portable.marker` sits next to the exe, data goes in `./data` next to it (git-ignored). Otherwise it goes in `%LOCALAPPDATA%`. `paths.rs` resolves this.
- **No telemetry, and no network activity the user didn't ask for.** Search only runs on explicit Search, clipboard monitoring is opt-in, and torrents refuse HTTP proxies instead of leaking direct. Preserve these when adding network features.

Tests: `crates/unduhkilat-core/tests/` has integration tests that run against in-process fake FTP/SFTP/SSH servers (`tests/common/`), so most server and remote-transfer behavior is testable offline. The `unduhkilat-app` UI has little automated coverage, so verify UI changes by running the app.

Working notes: `PRODUCT.md` and `DESIGN.md` hold product context and the "Paket Ekspres" design system (each download is a tracked parcel with a five-step track). `docs/superpowers/` holds plans and specs from earlier feature work.
