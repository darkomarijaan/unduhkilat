# DEPLOYMENT.md: UnduhKilat

## 1. Deployment Scope & Principles / Ruang Lingkup Deployment & Prinsip

### 1.1 Product Definition & What Deployment Means for UnduhKilat
UnduhKilat is a client-side Windows desktop application built with Rust and egui/eframe on the wgpu renderer (DEC-005). It runs as a self-contained local desktop process, backed by Tokio multi-threaded asynchronous tasks, an in-process SQLite database in WAL mode, and a companion Windows native messaging bridge binary (`unduhkilat-host.exe`) that interfaces with Chromium browsers (Chrome, Edge, Brave; DEC-006).

For UnduhKilat, **deployment does not mean launching containers, orchestrating microservices, publishing container images to registries, or provisioning cloud clusters.** UnduhKilat has no central backend server, no cloud database, no telemetry service, and no multi-tenant API (FR-057). 

Instead, **deployment for UnduhKilat means:**
1. Compiling and linking native Windows x64 binaries (`UnduhKilat.exe` and `unduhkilat-host.exe`) against the Windows MSVC toolchain (DEC-005).
2. Packaging the compiled binaries, the Chromium Manifest V3 extension directory, the portable configuration marker, and the native host manifests into a standalone, compressed **Portable ZIP Archive** (DEC-007, FR-059).
3. Cryptographically signing the release archive and executable binaries with an Authenticode-compatible code-signing key and Minisign/Ed25519 signature to support verified in-place updates (FR-060).
4. Publishing the verified ZIP archive, SHA-256 checksums, and detached signatures to a release distribution point (e.g., GitHub Releases).
5. Ensuring the uncompressed bundle self-registers its native messaging host under the current Windows user hive (`HKCU`) on initial launch with zero elevated privileges and zero installation wizards (DEC-007, FR-034).

### 1.2 Core Architectural Principles & Invariants
- **Zero Installation & Strict Portability (DEC-007, FR-059)**: The product must execute directly from an extracted directory. The presence of `unduhkilat-portable.marker` anchors all state files (`unduhkilat.db`) and rotating logs to a local `./data` subdirectory instead of system-wide paths.
- **Unbounded Engine Governance (DEC-001)**: The build pipeline must not hardcode artificial limits or caps on concurrent downloads or connection workers; concurrency parameters remain strictly user-configurable.
- **Priority-then-FIFO Queue Invariant (DEC-002)**: Binary builds must embed the deterministic two-tier scheduler dispatch model without runtime overrides.
- **Hybrid Media Capture Availability (DEC-003)**: The extension bundle shipped inside the release artifact must contain both the DOM injection hook script and the network sniffer background worker.
- **Background Execution & Tray Persistence (DEC-004)**: Binaries are compiled as Windows GUI subsystem applications (`windows_subsystem = "windows"`) so no console window spawns, allowing the process to silently minimize to the Windows system tray and emit native toasts on transfer completion.
- **Single Architecture Target (DEC-005)**: The target triple is strictly pinned to `x86_64-pc-windows-msvc`. No 32-bit x86, ARM64, macOS, or Linux targets are built.
- **Browser Boundary Containment (DEC-006)**: Native host configuration manifests target Chromium browsers exclusively (`HKCU\Software\Google\Chrome\NativeMessagingHosts`, `HKCU\Software\Microsoft\Edge\NativeMessagingHosts`, and `HKCU\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts`).

---

## 2. Environment Strategy / Strategi Environment

Because UnduhKilat runs as a standalone client desktop executable with no server-side hosting, conventional cloud environments (staging, QA clusters, production regions) do not apply. Instead, environments represent **isolated operational contexts and delivery target channels** across the software lifecycle.

### 2.1 Environment Matrix

| Environment | Purpose | Target Audience | Distribution Mechanism | Registry & Data Location |
|---|---|---|---|---|
| **Local Development** | Engineering iteration, unit testing, egui live debugging. | Internal Developers | Local `cargo run --bin UnduhKilat` | Process working directory (`target/debug`), isolated dev registry keys (`HKCU\Software\UnduhKilatDev`). |
| **CI / Integration** | Automated compilation, clippy checks, headless harness verification, artifact packaging. | Automated Pipeline | GitHub Actions Windows Runner (`windows-2022`) | Ephemeral runner filesystem; no external registry impact. |
| **Nightly / Preview** | Pre-release regression testing, dogfooding, dynamic segmentation benchmarking. | QA & Early Adopters | Portable ZIP (`UnduhKilat-nightly-x64.zip`) tagged per commit | Local portable `./data` folder; standard `HKCU` keys. |
| **Production (Release)** | Stable general public distribution. | End Users | Signed Portable ZIP (`UnduhKilat-v{VERSION}-windows-x64-portable.zip`) | Local portable `./data` folder; standard `HKCU` keys. |

### 2.2 Component Map per Environment

```
+------------------------------------------------------------------------------------+
| CI / Build Environment (GitHub Actions windows-2022)                                |
|  - Rust Toolchain: stable-x86_64-pc-windows-msvc (Pinned: 1.84.0)                  |
|  - Node.js: 20.x (Extension build & packaging)                                     |
|  - Output: UnduhKilat.exe, unduhkilat-host.exe, extension.zip, bundle.zip, sigs     |
+------------------------------------------------------------------------------------+
                                         │
                                         ▼ (Signed Artifact Egress)
+------------------------------------------------------------------------------------+
| Client End-User Machine (Windows 10 / 11 x64)                                      |
|                                                                                    |
|  +------------------------------------------------------------------------------+  |
|  | Portable Bundle Directory (e.g. C:\Tools\UnduhKilat\ or D:\UnduhKilat\)      |  |
|  |  ├── UnduhKilat.exe            (Main UI & Tokio Engine Core)                 |  |
|  |  ├── unduhkilat-host.exe       (Chromium Native Messaging Host)              |  |
|  |  ├── unduhkilat-portable.marker(Enforces localized data storage)             |  |
|  |  ├── extension/                (Manifest V3 Chrome/Edge/Brave Extension)     |  |
|  |  │     ├── manifest.json, background.js, content_script.js                   |  |
|  |  └── native-host/                                                            |  |
|  |        └── com.unduhkilat.host.json (Native messaging host manifest)         |  |
|  +------------------------------------------------------------------------------+  |
|           │                                                 │                      |
|           ▼ (First-run auto registration)                   ▼ (Local data storage) |
|  +--------------------------------+             +-------------------------------+  |
|  | Windows Registry (DEC-006)     |             | ./data Subdirectory (FR-058)  |  |
|  | - HKCU\Software\Google\Chrome  |             |  ├── state/                   |  |
|  | - HKCU\Software\Microsoft\Edge |             |  │     ├── unduhkilat.db      |  |
|  | - HKCU\Software\BraveSoftware  |             |  │     ├── unduhkilat.db-wal  |  |
|  +--------------------------------+             |  │     └── unduhkilat.db-shm  |  |
|                                                 |  └── logs/                    |  |
|                                                 |        └── unduhkilat.log     |  |
|                                                 +-------------------------------+  |
+------------------------------------------------------------------------------------+
```

---

## 3. Build & Delivery Artifacts / Artefak Build & Pengiriman

### 3.1 What This Stack Ships
UnduhKilat ships strictly as a **Portable ZIP Archive** for 64-bit Windows systems (DEC-005, DEC-007). 
- **Containers & Container Registries**: **DO NOT APPLY**. UnduhKilat is a client desktop application. It requires direct access to the Windows Display Driver Model (WDDM), DirectX 12/Vulkan via `wgpu`, Win32 Shell notification APIs, and local Windows Registry hives. Docker containers, Kubernetes manifests, Helm charts, and image registries are excluded from the repository.
- **Traditional Windows Installers (MSI/InnoSetup/NSIS)**: **DO NOT APPLY**. Explicitly superseded and rejected by DEC-007 (`Portable ZIP tanpa instalasi`).

### 3.2 Canonical Artifact Layout

The release workflow produces an authoritative ZIP archive named:
`UnduhKilat-v{VERSION}-windows-x64-portable.zip`

Contents of the portable archive:
```text
UnduhKilat-v1.0.0-windows-x64-portable/
├── UnduhKilat.exe                   # Main executable (egui GUI, Tokio runtime, reqwest)
├── unduhkilat-host.exe              # Stdin/stdout Native Messaging host binary
├── unduhkilat-portable.marker       # Marker file anchoring SQLite & logs to ./data
├── LICENSE                          # Legal license file
├── README.txt                       # Quick start instructions & browser setup guide
├── native-host/
│   ├── com.unduhkilat.host.json     # Native host manifest template for Chromium
│   └── register_host.bat            # Optional manual helper script for HKCU registration
└── extension/                       # Unpacked Chromium Manifest V3 extension
    ├── manifest.json
    ├── background.js                # WebRequest sniffer worker (DEC-003, DEC-006)
    ├── content_script.js            # Video/audio DOM hook script (DEC-003)
    ├── floating_panel.html          # Floating media sniffer overlay
    ├── floating_panel.js
    └── icons/
        ├── icon-16.png
        ├── icon-48.png
        └── icon-128.png
```

Accompanying verification artifacts:
- `UnduhKilat-v{VERSION}-windows-x64-portable.zip.sha256`: SHA-256 hash file.
- `UnduhKilat-v{VERSION}-windows-x64-portable.zip.minisig`: Minisign cryptographic signature.

---

## 4. CI/CD Pipeline

The CI/CD pipeline runs on GitHub Actions using Microsoft Windows Server 2022 virtual environments (`windows-2022`). The pipeline enforces strict toolchain pinning, comprehensive linting and formatting, automated test execution, binary compilation, code signing, and release artifact assembly.

### 4.1 Toolchain & Dependency Pinning
- **Rust Toolchain**: `1.84.0` (channel: `stable`, target: `x86_64-pc-windows-msvc`).
- **Cargo Components**: `clippy`, `rustfmt`.
- **Node.js**: `20.11.1` (used strictly for bundling the Chromium Manifest V3 extension).
- **LLVM / Clang**: Built into runner (for SQLite compilation bindings).
- **SignTool.exe**: Provided by the Windows 10/11 SDK on GitHub Actions runners.
- **Minisign**: Pinned version `0.11` for standalone release verification.

### 4.2 Pipeline Stages & Workflow File

The workflow is located at `.github/workflows/release.yml`:

```yaml
name: Build & Release UnduhKilat Windows Portable

on:
  push:
    tags:
      - 'v*'
  workflow_dispatch:
    inputs:
      dry_run:
        description: 'Perform dry-run build without publishing release'
        required: true
        default: 'false'
        type: choice
        options:
          - 'false'
          - 'true'

permissions:
  contents: write

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1
  RUSTFLAGS: "-D warnings"

jobs:
  build-and-package:
    name: Build, Sign & Package Windows x64 Portable
    runs-on: windows-2022

    steps:
      - name: Checkout Source Code
        uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Extract Release Version
        id: versioning
        shell: pwsh
        run: |
          $rawTag = "${{ github.ref_name }}"
          if ($rawTag -match "^v?(\d+\.\d+\.\d+.*)$") {
            $version = $Matches[1]
          } else {
            $version = "0.0.0-dev"
          }
          Write-Output "APP_VERSION=$version" >> $env:GITHUB_OUTPUT
          Write-Host "Configured application version: $version"

      - name: Set up Pinned Rust Toolchain
        uses: dtolnay/rust-toolchain@master
        with:
          toolchain: 1.84.0
          targets: x86_64-pc-windows-msvc
          components: rustfmt, clippy

      - name: Rust Cache
        uses: Swatinem/rust-cache@v2
        with:
          prefix-key: "v1-rust-cache"
          shared-key: "windows-x64-release"

      - name: Set up Node.js for Extension Bundle
        uses: actions/setup-node@v4
        with:
          node-version: 20.11.1
          cache: 'npm'
          cache-dependency-path: 'extension/package-lock.json'

      - name: Code Formatting Check
        shell: pwsh
        run: cargo fmt --all --check

      - name: Run Clippy Linter
        shell: pwsh
        run: cargo clippy --all-targets --all-features -- -D warnings

      - name: Execute Automated Test Suite
        shell: pwsh
        run: cargo test --workspace --all-features --verbose

      - name: Compile Windows Native Binaries
        shell: pwsh
        run: |
          cargo build --release --target x86_64-pc-windows-msvc --bin UnduhKilat --bin unduhkilat-host

      - name: Validate PE Machine Header (x64)
        shell: pwsh
        run: |
          $expectedMachine = 0x8664   # IMAGE_FILE_MACHINE_AMD64
          $binariesToCheck = @(
            "target/x86_64-pc-windows-msvc/release/UnduhKilat.exe",
            "target/x86_64-pc-windows-msvc/release/unduhkilat-host.exe"
          )

          foreach ($binary in $binariesToCheck) {
            $bytes = [System.IO.File]::ReadAllBytes($binary)
            $peHeaderOffset = [System.BitConverter]::ToInt32($bytes, 0x3C)
            $machine = [System.BitConverter]::ToUInt16($bytes, $peHeaderOffset + 4)
            if ($machine -ne $expectedMachine) {
              Write-Error "PE architecture assertion failed for $binary : machine header 0x$($machine.ToString('X4')) is not IMAGE_FILE_MACHINE_AMD64 (0x8664)."
              exit 1
            }
            Write-Host "$binary verified as x64 PE (IMAGE_FILE_MACHINE_AMD64 = 0x8664)."
          }

      - name: Build Chromium Extension Bundle
        shell: pwsh
        working-directory: extension
        run: |
          npm ci
          npm run build

      - name: Prepare Minisign Signing Tool
        shell: pwsh
        run: |
          Invoke-WebRequest -Uri "https://github.com/jedisct1/minisign/releases/download/0.11/minisign-0.11-win64.zip" -OutFile "minisign.zip"
          Expand-Archive -Path "minisign.zip" -DestinationPath "$env:RUNNER_TEMP\minisign"
          Write-Output "$env:RUNNER_TEMP\minisign\minisign-win64" >> $env:GITHUB_PATH

      - name: Authenticode Sign Executables
        # [placeholder: release signing policy — whether a tag build must fail the job when
        # CODE_SIGNING_CERT_BASE64 is absent (enforcing the signed binaries required by §1.1
        # and the §9 checklist), or whether unsigned releases are permitted — the owner has
        # not decided]
        if: ${{ env.CODE_SIGNING_CERT_BASE64 != '' }}
        shell: pwsh
        env:
          CODE_SIGNING_CERT_BASE64: ${{ secrets.CODE_SIGNING_CERT_BASE64 }}
          CODE_SIGNING_CERT_PASSWORD: ${{ secrets.CODE_SIGNING_CERT_PASSWORD }}
        run: |
          $certPath = "$env:RUNNER_TEMP\cert.pfx"
          [System.IO.File]::WriteAllBytes($certPath, [System.Convert]::FromBase64String($env:CODE_SIGNING_CERT_BASE64))
          
          $filesToSign = @(
            "target/x86_64-pc-windows-msvc/release/UnduhKilat.exe",
            "target/x86_64-pc-windows-msvc/release/unduhkilat-host.exe"
          )
          
          foreach ($file in $filesToSign) {
            Write-Host "Signing $file with Authenticode..."
            & "signtool.exe" sign /f $certPath /p $env:CODE_SIGNING_CERT_PASSWORD /tr http://timestamp.digicert.com /td sha256 /fd sha256 $file
          }
          Remove-Item $certPath -Force

      - name: Assemble Portable Distribution Directory
        shell: pwsh
        run: |
          $ver = "${{ steps.versioning.outputs.APP_VERSION }}"
          $distDir = "dist/UnduhKilat-v$ver-windows-x64-portable"
          New-Item -ItemType Directory -Force -Path "$distDir"
          New-Item -ItemType Directory -Force -Path "$distDir/native-host"
          New-Item -ItemType Directory -Force -Path "$distDir/extension"

          # Copy compiled binaries
          Copy-Item "target/x86_64-pc-windows-msvc/release/UnduhKilat.exe" "$distDir/"
          Copy-Item "target/x86_64-pc-windows-msvc/release/unduhkilat-host.exe" "$distDir/"
          
          # Place portable marker file (DEC-007, FR-059)
          New-Item -ItemType File -Force -Path "$distDir/unduhkilat-portable.marker"

          # Copy docs and licenses
          Copy-Item "LICENSE" "$distDir/"
          Copy-Item "README.md" "$distDir/README.txt"

          # Copy Native Messaging Host manifest
          Copy-Item "packaging/windows/com.unduhkilat.host.json" "$distDir/native-host/"
          Copy-Item "packaging/windows/register_host.bat" "$distDir/native-host/"

          # Copy Extension artifacts
          Copy-Item -Recurse "extension/dist/*" "$distDir/extension/"

          # Create distribution ZIP
          $zipPath = "dist/UnduhKilat-v$ver-windows-x64-portable.zip"
          Compress-Archive -Path "$distDir" -DestinationPath "$zipPath" -CompressionLevel Optimal
          Write-Output "ZIP_PATH=$zipPath" >> $env:GITHUB_OUTPUT

      - name: Generate Checksums and Sign Package
        shell: pwsh
        env:
          MINISIGN_SECRET_KEY: ${{ secrets.MINISIGN_SECRET_KEY }}
          # [placeholder: whether the Minisign secret key is passphrase-protected — if it is,
          # MINISIGN_PASSPHRASE must be supplied to minisign and the `-W` flag removed; if it
          # is not, MINISIGN_PASSPHRASE is dropped from Section 4.2 and Section 5.1 — the
          # owner has not decided]
          MINISIGN_PASSPHRASE: ${{ secrets.MINISIGN_PASSPHRASE }}
        run: |
          $ver = "${{ steps.versioning.outputs.APP_VERSION }}"
          $zipPath = "dist/UnduhKilat-v$ver-windows-x64-portable.zip"
          
          # Calculate SHA-256
          $hash = (Get-FileHash -Path $zipPath -Algorithm SHA256).Hash.ToLower()
          "$hash *$([System.IO.Path]::GetFileName($zipPath))" | Out-File -FilePath "$zipPath.sha256" -Encoding ASCII

          # Minisign signature for in-app auto-update verification (FR-060)
          if ($env:MINISIGN_SECRET_KEY -ne $null -and $env:MINISIGN_SECRET_KEY -ne "") {
            $keyPath = "$env:RUNNER_TEMP\minisign.key"
            [System.IO.File]::WriteAllText($keyPath, $env:MINISIGN_SECRET_KEY)
            minisign -S -s $keyPath -m $zipPath -p packaging/keys/minisign.pub -t "UnduhKilat release $ver" -W
            Remove-Item $keyPath -Force
          }

      - name: Verify Minisign Package Signature
        shell: pwsh
        run: |
          $ver = "${{ steps.versioning.outputs.APP_VERSION }}"
          $zipPath = "dist/UnduhKilat-v$ver-windows-x64-portable.zip"

          # Deterministic security verification against packaging/keys/minisign.pub
          minisign -V -m $zipPath -p packaging/keys/minisign.pub
          if ($LASTEXITCODE -ne 0) {
            Write-Error "Minisign verification failed for $zipPath against packaging/keys/minisign.pub."
            exit 1
          }
          Write-Host "Minisign signature verified against packaging/keys/minisign.pub."

      - name: Publish GitHub Release
        if: ${{ github.event.inputs.dry_run != 'true' && startsWith(github.ref, 'refs/tags/v') }}
        uses: softprops/action-gh-release@v2
        with:
          files: |
            dist/*.zip
            dist/*.sha256
            dist/*.minisig
          draft: false
          prerelease: false
          generate_release_notes: true
```

### 4.3 Build Health Checks & Verification
Every CI build enforces the following validation barriers before an artifact is minted:
1. **Zero-Warning Cargo Check**: `RUSTFLAGS="-D warnings"` prevents unhandled results or deprecated APIs from entering production binaries.
2. **Deterministic Security Verification**: Minisign detached signature check asserts that the generated archive can be decrypted and validated with `packaging/keys/minisign.pub`.
3. **PE Binary Architecture Validation**: Post-compile PowerShell assertion verifying that `UnduhKilat.exe` has an x64 machine header (`IMAGE_FILE_MACHINE_AMD64 = 0x8664`).

---

## 5. Environment Variables / Konfigurasi Environment Variables

UnduhKilat is a local client application. It does not read secrets from cloud vaults or production `.env` files during standard runtime. 

Runtime settings (such as proxy configuration, connection counts, and bandwidth limits) are stored directly in the local SQLite `settings` table (refer to `DATABASE.md`) and encrypted using Windows DPAPI (refer to `API.md` Section 1.2).

Environment variables are reserved exclusively for **build-time compilation** and **local diagnostics overrides**.

### 5.1 Build-Time Environment Variables (CI / Compilation)

| Variable Name | Purpose | Required / Default | Secret? | Validation Rule |
|---|---|:---:|:---:|---|
| `CARGO_TERM_COLOR` | Controls ANSI terminal color formatting. | Optional (`always`) | No | Literal `always` or `auto`. |
| `RUST_BACKTRACE` | Prints symbolicated stack traces on panic. | Optional (`1`) | No | `0`, `1`, or `full`. |
| `RUSTFLAGS` | Enforces compiler warning rejection. | Required (`-D warnings`) | No | Valid rustc compiler flags. |
| `CODE_SIGNING_CERT_BASE64` | Base64-encoded Authenticode `.pfx` certificate. | Optional (CI Release) | **Yes** | Valid PFX binary certificate. |
| `CODE_SIGNING_CERT_PASSWORD`| Password for Authenticode certificate. | Optional (CI Release) | **Yes** | Alphanumeric string. |
| `MINISIGN_SECRET_KEY` | Ed25519 Minisign private key string. | Optional (CI Release) | **Yes** | Formatted Minisign secret key file. |
| `MINISIGN_PASSPHRASE` | Decryption passphrase for Minisign private key. `[placeholder: whether the Minisign key is passphrase-protected at all, and therefore whether this variable is consumed by the signing step — the owner has not decided]` | Optional (CI Release) | **Yes** | Secure passphrase. |

### 5.2 Runtime Diagnostic Overrides (Developer & Advanced Local Use)

All runtime variables are optional and strictly validated at process initialization. If an invalid value is supplied, UnduhKilat logs a warning and falls back to safe defaults.

| Variable Name | Purpose | Default Value | Allowed Values |
|---|---|---|---|
| `UNDUHKILAT_LOG` | Overrides the disk and console log filter. | `info` | `error`, `warn`, `info`, `debug`, `trace` |
| `UNDUHKILAT_PORTABLE` | Forces portable directory resolution mode. | Inferred from `.marker` | `0` (false), `1` (true) |
| `UNDUHKILAT_DATA_DIR` | Explicit override for SQLite and logs directory path. | `./data` | Valid absolute Windows filesystem path |
| `UNDUHKILAT_DISABLE_TOAST` | Suppresses WinRT toast notifications during tests. | `0` | `0` (enabled), `1` (disabled) |

---

## 6. Database Migration & Data Operations / Migrasi Data

UnduhKilat persists all mutable state to a single SQLite database (`unduhkilat.db`). For exhaustive schema definitions, column constraints, and foreign key rules, refer to `DATABASE.md`.

### 6.1 Additive & Backward-Compatible Migration Strategy
Because UnduhKilat is distributed as a portable executable without centralized DBA supervision, schema upgrades run **in-process at application startup** before Tokio worker pools initialize:
1. **Schema Version Tracking**: Database state versioning is tracked via SQLite's built-in `PRAGMA user_version`.
2. **Strict Additive Migrations**: Migrations must only add columns (`ALTER TABLE ... ADD COLUMN ...`), create new indexes, or create new tables. Columns are never dropped or renamed in place to ensure that an older binary version run against a newer database file does not panic.
3. **Transactional DDL Execution**: All schema migrations execute inside explicit transactional wrappers (`BEGIN IMMEDIATE ... COMMIT`). If a migration step fails, the transaction rolls back cleanly, leaving the database uncorrupted.

```sql
-- Conceptual startup migration gate (executed by SQLite Writer Actor)
BEGIN IMMEDIATE;
-- Example: Migrate schema from v1 to v2
-- PRAGMA user_version check performed in Rust driver
-- ALTER TABLE downloads ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;
PRAGMA user_version = 2;
COMMIT;
```

### 6.2 SQLite Online Backup & WAL Checkpoint Operations
Direct file copies (`CopyFile`) of a live SQLite database while in `WAL` mode can yield inconsistent or corrupted database files due to pending memory pages and `.db-shm` mapping. 

Data backups must use **SQLite Online File Backup API** paired with a preceding **WAL Checkpoint**:

```rust
// Conceptual Rust implementation of the online backup command
pub fn perform_online_backup(src_conn: &rusqlite::Connection, backup_path: &std::path::Path) -> rusqlite::Result<()> {
    // 1. Force all WAL journal pages into the main database file
    src_conn.pragma_update(None, "wal_checkpoint", "PASSIVE")?;
    
    // 2. Open destination database connection
    let mut dst_conn = rusqlite::Connection::open(backup_path)?;
    
    // 3. Execute SQLite Online Backup API step-by-step
    let backup = rusqlite::backup::Backup::new(src_conn, &mut dst_conn)?;
    backup.run_to_completion(5, std::time::Duration::from_millis(250), None)?;
    
    Ok(())
}
```

### 6.3 Automated Startup Recovery & Disaster Verification
1. **Startup Normalization**: On cold launch, the database writer actor runs:
   ```sql
   UPDATE downloads 
   SET state = 'paused', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') 
   WHERE state = 'running';
   ```
   This guarantees that downloads interrupted by power failure or `TerminateProcess` transition to a safe `paused` state without re-downloading existing chunks (FR-010, KPI-006).
2. **Integrity Probe**: During automated test suites and pre-release validation, a non-destructive integrity check is executed:
   ```sql
   PRAGMA quick_check;
   ```
   Any response other than `ok` aborts the test run and flags the build as failed.

---

## 7. Observability & Logging

In accordance with FR-057 and NFR-016, UnduhKilat collects **zero telemetry, transmits no usage data, and maintains no external monitoring connections.** Observability is entirely localized to the user's workstation.

### 7.1 Local Diagnostic Logging
- **Framework**: Rust `tracing` ecosystem with `tracing-subscriber` and `tracing-appender`.
- **Log Location**: Stored in `./data/logs/unduhkilat.log` (when portable marker is present).
- **Log Rotation**: `[placeholder: the single rotation trigger that applies — whether rotation is daily or at a fixed maximum file size — the owner has not decided; the rotation ceiling remains TBD per PRD]`, retaining a maximum of 3 archived files.
- **Privacy & Redaction**: Password strings, DPAPI-encrypted blobs, session tokens, and query credentials are unconditionally excluded from log formatters.

### 7.2 In-Application Diagnostics & Log Export
- **Live Diagnostics Panel (FR-056)**: Real-time UI view rendering memory usage, active Tokio tasks, open socket counts, and the tail of the current disk log.
- **Log Export Functionality**: Users can click "Export Logs" in the diagnostics view to copy rotating logs into a target directory (e.g., for troubleshooting or bug reporting). The underlying IPC command is `diagnostics.export_logs` (refer to `API.md` Section 3.11.3).

---

## 8. Rollback Procedure / Prosedur Rollback

Because UnduhKilat runs without cloud backends, rollback represents restoring a previously known good binary and its compatible database file on an end-user workstation.

### 8.1 In-Place Update Rollback (Automated)
When an in-place auto-update is applied (FR-060, `API.md` Section 3.12.2):
1. **Pre-Update Snapshot**: The update engine renames the current directory binaries to `UnduhKilat.exe.bak` and `unduhkilat-host.exe.bak` before unpacking the replacement files.
2. **Failure Detection**: If the newly installed binary crashes on launch, panics during startup database migration, or fails the internal handshake ping (`browser.ping`), the updater restores `.bak` binaries immediately.
3. **Database Guard**: If `PRAGMA user_version` in `unduhkilat.db` was incremented, the database is restored from the automatic pre-migration snapshot taken in `./data/state/backup_pre_migration.db`.

### 8.2 Manual Portable Rollback (User Action)
If a user upgrades manually by extracting a new portable ZIP over an old directory and encounters issues:
1. **Preserve Downloads**: Downloaded user files reside in their designated category folders (e.g., `C:\Downloads\Compressed`) and are completely unaffected by binary rollbacks.
2. **Binary Replacement**: Replace `UnduhKilat.exe` and `unduhkilat-host.exe` with the binaries from the previous release ZIP (e.g., `v1.0.0`).
3. **Database Downgrade Check**: Because SQLite migrations are strictly additive (Section 6.1), the previous version executable can continue reading the existing `unduhkilat.db` without crashing.

---

## 9. Pre-Release Checklist / Checklist Pra-Rilis

Every release tag (`vX.Y.Z`) must pass this checklist before the portable distribution archive is published for general release:

- [ ] **Toolchain Pinned**: Rust compiler verified at `1.84.0` (MSVC) and Node.js at `20.11.1`.
- [ ] **Architecture Check**: Compilation verified strictly for `x86_64-pc-windows-msvc`; no 32-bit or non-Windows binaries built (DEC-005).
- [ ] **Static Analysis**: `cargo clippy --all-targets --all-features` passes with zero warnings.
- [ ] **Test Suite Coverage**: `cargo test --workspace` passes 100% of unit, integration, and crash-recovery simulation tests.
- [ ] **Authenticode Signed**: `signtool.exe verify /pa /v` validates signatures on `UnduhKilat.exe` and `unduhkilat-host.exe`.
- [ ] **Package Signing**: Minisign signature verified against public key `packaging/keys/minisign.pub`.
- [ ] **Portable Marker Included**: Archive contains `unduhkilat-portable.marker` at root (DEC-007, FR-059).
- [ ] **Browser Extension Bundled**: `extension/dist/` contains valid Manifest V3 manifest, background sniffer, and DOM hook scripts (DEC-003, DEC-006).
- [ ] **Clean First-Run Verification**: Fresh extraction on Windows 10 and Windows 11 registers native messaging keys in `HKCU` with zero elevation prompts.
- [ ] **Crash Durability Tested**: Terminating process mid-transfer leaves SQLite database healthy (`PRAGMA quick_check;`) and restarts items in `paused` state (FR-010).