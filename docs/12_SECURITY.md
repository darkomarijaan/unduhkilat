# SECURITY.md: UnduhKilat

## 1. Scope & Trust Model

### 1.1 Scope of the Document
This document defines the security architecture, threat model, cryptographic specifications, and operational controls for **UnduhKilat**, a crash-durable, high-throughput desktop download manager engineered for Windows 10 and Windows 11 (x64 only) using Rust (stable, MSVC toolchain) and egui/eframe on the wgpu renderer (DEC-005). 

The scope encompasses:
- The desktop graphical executable (`UnduhKilat.exe`).
- The standalone Chromium Native Messaging Host bridge (`unduhkilat-host.exe`) registered in the Windows Registry (DEC-006, FR-034).
- The Manifest V3 browser extension for Chrome, Edge, and Brave (DEC-006, FR-033).
- The local SQLite state database (`unduhkilat.db`) operating in WAL mode (FR-058).
- The Windows Data Protection API (DPAPI) credential storage subsystem (FR-014, FR-058, DEC-005).
- The asynchronous network transport layer built on `reqwest` and `rustls` (FR-012).
- The in-place cryptographic update verification system for portable ZIP bundles (DEC-007, FR-060).

### 1.2 Trust Boundaries & Execution Model
UnduhKilat executes entirely on the user's local workstation as a portable application requiring zero installer and no mandatory administrative elevation (DEC-007, FR-059). The architecture defines four trust boundaries:

```
+---------------------------------------------------------------------------------------+
| Trust Boundary 4: Remote Network (Untrusted Source Servers / CDNs / Proxies)          |
+---------------------------------------------------------------------------------------+
                                        ▲
                                        │ HTTPS/TLS 1.2/1.3 (reqwest + rustls); cleartext HTTP/FTP warning-only; FTP transport [placeholder: separate FTP client/transport or removal of FTP support — the owner has not decided]
                                        ▼
+---------------------------------------------------------------------------------------+
| Trust Boundary 3: Web Browser Sandbox (Chromium Extension MV3 - Semi-Trusted)         |
| - Intercepts downloads, DOM hooks, sniffing (DEC-003, DEC-006)                        |
+---------------------------------------------------------------------------------------+
                                        ▲
                                        │ Length-prefixed Stdin/Stdout JSON (Local IPC)
                                        ▼
+---------------------------------------------------------------------------------------+
| Trust Boundary 1: Local Interactive User Session (Standard Windows User Token)        |
| - egui Desktop UI Shell                                                               |
| - Tokio Async Segmentation & Scheduling Core                                          |
| - SQLite WAL Database (unduhkilat.db)                                                 |
| - Windows DPAPI Vault (DPAPI-protected secret blobs)                                  |
| - Windows Registry HKCU (DEC-006, FR-034)                                             |
+---------------------------------------------------------------------------------------+
                                        ▲
                                        │ Explicit Windows UAC Elevation Token
                                        ▼
+---------------------------------------------------------------------------------------+
| Trust Boundary 2: Elevated Administrative Boundary (High Integrity Process Token)     |
| - Windows Registry HKLM Registration (FR-035)                                         |
| - Machine-wide bundle replacement & state purge (FR-061, FR-062)                      |
+---------------------------------------------------------------------------------------+
```

1. **Boundary 1: Local Interactive User Session**: Standard Windows user profile context. Memory space of `UnduhKilat.exe` and `unduhkilat-host.exe` is protected by Windows NT kernel memory isolation. Local disk files are protected by NTFS Access Control Lists (ACLs) matching the interactive user.
2. **Boundary 2: Elevated Administrative Boundary**: Operations that modify machine-wide configurations (`HKLM` registry keys) or shared installation directories require elevated Windows tokens via User Account Control (UAC). Standard operational tasks never cross into Boundary 2 (DEC-007, FR-059).
3. **Boundary 3: Browser Extension Sandbox**: The Manifest V3 extension runs within the Chromium sandboxed process. Communication with the native host occurs via standard input/output pipes carrying 32-bit length-prefixed JSON frames. The extension cannot directly inspect the filesystem or memory of the desktop application.
4. **Boundary 4: Remote Network / Origin Servers**: Source servers providing downloads, proxies, and media streams are treated as untrusted external endpoints. All ingested payloads, headers, and redirects undergo strict format and range validation.

### 1.3 Asset Classification & Impact Matrix

| Asset | Location | Classification | Impact if Compromised |
|---|---|---|---|
| **Host Authentication Credentials** (HTTP/FTP basic/digest passwords) | SQLite `credentials.password_encrypted` | Restricted | High: Unauthorized access to private remote resources, repositories, or FTP shares. |
| **Proxy Authentication Secrets** | SQLite `settings.value` (`proxy_password_encrypted`) | Confidential | High: Unauthorized network traversal or corporate proxy impersonation. |
| **Browser Session Cookies & Tokens** | SQLite `downloads.cookies` / Process Memory | Restricted | High: Session hijacking on origin sites whose downloads were captured. |
| **Download Payloads & Temporary Segments** | Local filesystem (`target_path`, temp files) | Confidential / User Data | Medium to High: Exposure of private downloaded documents, binaries, or media. |
| **State Database & Download History** | `%LOCALAPPDATA%\UnduhKilat\state\unduhkilat.db` or `.\data\state\` | Confidential | Medium: Disclosure of user browsing habits, download origins, and file structures. |
| **Local Diagnostic Logs** | Rotating log files (`unduhkilat.log*`) | Internal / Diagnostic | Low to Medium: Exposure of accessed URLs and network topology if unredacted. |
| **Native Messaging Host Registry Keys** | Windows Registry (`HKCU` / `HKLM`) | Integrity Sensitive | High (integrity): Execution hijacking if registry paths are tampered to point to malicious replacement binaries; key leakage has no confidentiality impact. |
| **Update Verification Public Key** | Embedded inside `UnduhKilat.exe` binary | Integrity Sensitive | Critical (integrity): Tampering with the embedded public key would allow arbitrary remote code execution via forged updates. Public-key leakage has no confidentiality impact. |

---

## 2. Threat Model (STRIDE)

The threat model evaluates UnduhKilat against the STRIDE taxonomy, tailored specifically to a native Windows x64 desktop architecture with Chromium native messaging (DEC-005, DEC-006, DEC-007).

| STRIDE Category | Threat Description | Affected Asset | Mitigation Control |
|---|---|---|---|
| **Spoofing** | A malicious local process or unauthorized browser extension sends IPC frames to `unduhkilat-host.exe` to trigger unwanted transfers. | Native Messaging IPC, Local Filesystem | Extension ID validation against native messaging manifest; strict origin validation; confirmation dialog on every download (FR-034, FR-039). |
| **Spoofing** | Attacker impersonates the vendor update server to deliver a trojanized update package. | Application Binaries (`UnduhKilat.exe`, `unduhkilat-host.exe`) | Cryptographic Ed25519 digital signature verification before applying in-place update archives (FR-060, NFR-013). |
| **Tampering** | A non-privileged local process or malware tampers with the SQLite database file to alter segment offsets or insert malicious commands. | SQLite Database (`unduhkilat.db`), Segments | NTFS user ACLs; database integrity checks on boot; checksum and size validation upon assembly (`ERR_CHECKSUM_SIZE_MISMATCH`, FR-011). |
| **Tampering** | Man-in-the-Middle (MITM) adversary tampers with chunk data during in-flight transfers over network sockets. | Download Payloads, Target Files | Strict TLS certificate validation via `reqwest` and `rustls` (FR-012); byte length assertion on stream completion (FR-011). |
| **Tampering** | Path traversal sequence in server `Content-Disposition` or intercepted URL (e.g., `..\..\Windows\System32\cmd.exe`). | Windows Host Filesystem | Path sanitization: stripping directory traversal elements, enforcing configured category directories (FR-016, FR-029). |
| **Repudiation** | User claims a scheduled or background download occurred without knowledge or authorization. | Download History, Audit Logs | Transactional audit records in SQLite state database; local rotating diagnostic logs recording dispatch events (FR-023, FR-057, FR-058). |
| **Information Disclosure** | Plaintext credentials extracted from the SQLite state store by offline inspection of the database file. | Host and Proxy Passwords | Encryption at rest via Windows Data Protection API (`CryptProtectData`) using user-bound master keys (DEC-005, FR-014, FR-058). |
| **Information Disclosure** | Sensitive authentication cookies, Bearer tokens, or passwords written to local rotating diagnostic log files. | Diagnostic Logs, UI Diagnostics Panel | Automatic redaction pipeline filtering headers (`Authorization`, `Cookie`, `Set-Cookie`) before disk write (FR-056, FR-057). |
| **Information Disclosure** | Shoulder surfing or unprivileged process querying active downloads via browser extension. | Active Download State, URLs | Extension is restricted to querying only transfers initiated from its own browser tab/session (FR-038). |
| **Denial of Service** | User-configured unbounded concurrency or connections (DEC-001) exhausts Windows socket descriptors (`WSAENOBUFS`) or memory. | System Network Stack, Memory | Asynchronous non-blocking I/O via Tokio; reusable 64 KB memory chunk buffers; diagnostics monitoring of live connections (FR-056). |
| **Denial of Service** | Malicious webpage floods the extension with fake media candidates or link interceptions, exhausting IPC buffers. | IPC Pipes, egui UI Shell | Bounded Tokio mpsc channels; rate-limiting candidate ingestion; deduplication by canonical URL (DEC-003, FR-041, FR-042). |
| **Denial of Service** | Incomplete downloads fill the local disk volume completely, causing host OS instability. | Local Storage Volumes | Pre-allocation checks on initial HEAD/GET probe; `ERR_STORAGE_FULL` handling; pause on write failure (FR-003, FR-008). |
| **Elevation of Privilege** | Local adversary abuses the Native Messaging Host binary to execute arbitrary shell commands via parameter injection. | Host Process Execution Token | Host accepts strictly serialized JSON envelopes; no shell invocation (`cmd.exe`/`powershell.exe`) is used to launch processes. |
| **Elevation of Privilege** | An unelevated user executes administrative maintenance commands (`--admin-register-host`, `--admin-upgrade-bundle`, `--admin-reset-data`). | Windows Registry `HKLM`, Bundle Files | Explicit Windows token elevation checks (`TOKEN_ELEVATION`) via `GetTokenInformation`; abort with `ERR_PERMISSION_DENIED` (FR-035, FR-061, FR-062). |

---

## 3. Authentication & Authorization

### 3.1 Local Interactive Identity & Sessions
As a standalone Windows desktop client, UnduhKilat does not implement a multi-tenant web session or remote user login system (DEC-005, DEC-007). Identity is anchored directly to the active Windows Interactive User Session:
- **No In-App Master Password**: Access to the running application and system tray interface is governed by physical/OS-level Windows authentication (PIN, password, Windows Hello).
- **Process Boundaries**: The egui GUI view and Tokio async engine execute within the same process (`UnduhKilat.exe`) under the security context of the logged-in user.
- **Intra-Process Isolation**: Memory safety is enforced by the Rust compiler (MSVC toolchain), eliminating use-after-free, double-free, and buffer overflow vulnerabilities across internal channels (`tokio::sync::mpsc`).

### 3.2 Remote Origin Authentication & Credential Storage
To fetch resources protected by HTTP Basic, HTTP Digest, or FTP authentication, and to authenticate against corporate proxies (FR-012, FR-014, FR-015):
- **Plaintext Secret Ingestion**: User inputs credentials via the Add URL modal, category settings, or proxy configuration screens.
- **DPAPI Encryption**: Prior to writing to `unduhkilat.db`, plaintext secrets are passed to the Win32 `CryptProtectData` API. The resulting ciphertext is base64-encoded and stored in `credentials.password_encrypted` or `settings.value` (for proxy secrets).
- **Ephemeral In-Memory Lifecycle**: Secrets are decrypted via `CryptUnprotectData` exclusively when an HTTP 401/407 challenge or FTP authentication handshake occurs. The plaintext byte slice is wrapped in a memory guard that calls `zeroize` upon completion of the request header dispatch.

### 3.3 Roles and Permission Checks

```
+-----------------------------------------------------------------------------------+
| UnduhKilat Role-Based Permission Matrix                                           |
+-----------------------------------------------------------------------------------+
| Operation / Endpoint Group      | End User      | Browser Extension | Admin Token  |
+---------------------------------+---------------+-------------------+--------------+
| download.create, resume, pause  | Allowed       | Trigger Only*     | Denied       |
| download.list, get_segments     | Allowed       | Scoped to Tab/Own | Allowed      |
| download.delete, cancel         | Allowed       | Denied            | Denied       |
| queue.*, schedule.*             | Allowed       | Denied            | Denied       |
| speed.*, settings.*             | Allowed       | Denied            | Denied       |
| category.*                      | Allowed       | Read-only (paths) | Denied       |
| browser.intercept               | Confirm Dialog| Allowed (Emit)    | Denied       |
| media.candidate_detected        | Confirm Dialog| Allowed (Emit)    | Denied       |
| admin.registry.* (HKLM)         | Denied        | Denied            | Allowed      |
| admin.bundle.* (Reset / Upgrade)| Denied        | Denied            | Allowed      |
+-----------------------------------------------------------------------------------+
* Browser-initiated transfers require explicit interactive End User confirmation (FR-039).
```

- **End User**: Holds full unprivileged control over the engine: scheduling, queue priority manipulation (DEC-002), speed throttling (FR-025), and category routing.
- **Browser Extension (Manifest V3)**: Strictly confined to emitting intercepted URLs and media candidate metadata (DEC-003, DEC-006, FR-036, FR-041). It possesses no capability to read stored credentials, alter global settings, or inspect downloads started by other browser sessions.
- **Administrator (Elevated)**: Required exclusively for machine-wide host registration under `HKLM` (FR-035), replacing the application bundle in shared directories (FR-061), or executing a global state wipe (FR-062). Administrative operations assert Windows UAC token elevation via the following verification check:

```rust
// Administrative Elevation Check invariant in src/security/uac.rs
pub fn assert_elevated_token() -> Result<(), SecurityError> {
    unsafe {
        let mut token_handle: winapi::um::winnt::HANDLE = std::ptr::null_mut();
        if winapi::um::processthreadsapi::OpenProcessToken(
            winapi::um::processthreadsapi::GetCurrentProcess(),
            winapi::um::winnt::TOKEN_QUERY,
            &mut token_handle,
        ) == 0 {
            return Err(SecurityError::ElevationCheckFailed);
        }

        let mut elevation: winapi::um::winnt::TOKEN_ELEVATION = std::mem::zeroed();
        let mut ret_len: u32 = 0;
        let success = winapi::um::securitybaseapi::GetTokenInformation(
            token_handle,
            winapi::um::winnt::TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<winapi::um::winnt::TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        );
        winapi::um::handleapi::CloseHandle(token_handle);

        if success != 0 && elevation.TokenIsElevated != 0 {
            Ok(())
        } else {
            Err(SecurityError::PermissionDenied("ERR_PERMISSION_DENIED"))
        }
    }
}
```

### 3.4 Tenancy and User Isolation
UnduhKilat adheres to a single-user desktop tenancy model (DEC-005):
- State and log files are isolated within the executing Windows user's roaming or local directory: `%LOCALAPPDATA%\UnduhKilat\`.
- Under the portable ZIP deployment (DEC-007), if `unduhkilat-portable.marker` is present, state is confined to `.\data\` relative to the binary, protected by NTFS directory ACLs.
- In multi-user Windows environments, one user cannot inspect another user's SQLite database or DPAPI-encrypted secrets, as DPAPI binds encryption to the specific Windows user's Security Identifier (SID) and logon credentials.

---

## 4. Data Encryption

### 4.1 Data in Transit
All TLS-protected network communications (HTTPS) crossing machine or process boundaries enforce strict cryptographic isolation. Cleartext HTTP and cleartext FTP transfers (FR-012) are explicit exceptions: they provide no cryptographic isolation and receive only warning-level protection through visible UI indications.

1. **Remote HTTP/HTTPS Transfers**:
   - Built on `reqwest` using `rustls` (MSVC toolchain, FR-012). Rustls eliminates legacy OpenSSL flaws and enforces TLS 1.2 and TLS 1.3 with modern AEAD cipher suites:
     - `TLS_AES_256_GCM_SHA384`
     - `TLS_CHACHA20_POLY1305_SHA256`
     - `TLS_AES_128_GCM_SHA256`
     - `TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384`
     - `TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384`
   - **Root Certificate Store**: Enforces validation using the Windows system root certificate store via `rustls-native-certs`. Revocation checking via CRL/OCSP is `TBD`.
   - **Plaintext Protocol Warning**: Unencrypted protocols (`http://` and cleartext `ftp://`) are supported per functional requirements (FR-012). The Add URL and confirmation dialogs visibly indicate unencrypted transport to prevent unintentional data transmission over cleartext links.
2. **Native Messaging IPC (Browser $\leftrightarrow$ Native Host)**:
   - Communication operates across local standard input/output anonymous pipes instantiated directly by the Chromium browser parent process under the user's desktop session. Because data never touches external network sockets or loopback interfaces, network sniffing is mitigated by OS kernel pipe isolation.
3. **Local Intra-Process Channels**:
   - Thread communication between UI and download engine occurs via Tokio memory channels (`mpsc`, `broadcast`). Data resides exclusively in process virtual memory protected by Windows virtual memory isolation.

### 4.2 Data at Rest
Data at rest falls into two distinct categories: relational state metadata and sensitive authentication secrets.

```
+-----------------------------------------------------------------------------------+
| Local Data Storage Security Architecture                                          |
+-----------------------------------------------------------------------------------+
|                                                                                   |
|  +-------------------------------------+  +------------------------------------+  |
|  | SQLite Relational State Store       |  | Windows DPAPI Cryptographic Vault  |  |
|  | (unduhkilat.db - WAL Mode)          |  | (DPAPI-protected secret blobs)     |  |
|  | - Download Metadata (URLs, paths)   |  | - Host HTTP/FTP Basic Passwords    |  |
|  | - Byte-Range Partitions & Offsets   |  | - Proxy Authentication Passwords   |  |
|  | - Categories, Schedules, Priorities |  |                                    |  |
|  +-------------------------------------+  +------------------------------------+  |
|                   │                                          │                    |
|                   └──────────────┬───────────────────────────┘                    |
|                                  ▼                                                |
|  +-----------------------------------------------------------------------------+  |
|  | Windows Filesystem (NTFS DACL: Authenticated User Full Control Only)         |  |
|  | Path: %LOCALAPPDATA%\UnduhKilat\state\ OR .\data\state\ (DEC-007)            |  |
|  +-----------------------------------------------------------------------------+  |
+-----------------------------------------------------------------------------------+
```

1. **State Store (`unduhkilat.db`)**:
   - Relational metadata (task status, downloaded offsets, priority levels) is stored in SQLite 3 in WAL mode (FR-058). Database encryption at rest (e.g., SQLCipher) is `TBD`.
2. **Windows DPAPI Vault for Secrets**:
   - Credential strings stored in `credentials.password_encrypted` and `settings.value` (where `key = 'proxy_password_encrypted'`) are encrypted using Win32 `CryptProtectData` (DEC-005, FR-014, FR-015):
     - Cipher: 3-Key Triple-DES or AES-256 (Windows Cryptographic Service Provider / CNG default).
     - Entropy Salt: Optional machine/user salt `TBD`.
     - Key Management: Delegated to the Windows Local Security Authority (LSA), which derives master keys from the user's Windows logon password hash.
   - Plaintext host and proxy passwords are never written to disk or preserved in SQLite rollback journals. The at-rest policy for browser session cookies/tokens in `downloads.cookies` is [placeholder: DPAPI encryption before storage, memory-only storage, or another policy — the owner has not decided].

### 4.3 Key Management & Update Verification Keys
1. **Operating System Managed Keys**: DPAPI master keys are owned, stored, and rotated entirely by the Windows operating system. UnduhKilat retains no physical key files on disk for credential storage.
2. **Update Archive Verification Key**:
   - UnduhKilat embeds a hardcoded Ed25519 public key within the `UnduhKilat.exe` executable.
   - When an update archive (`.zip`) is downloaded (FR-060), the application reads the detached signature (`.sig`), verifies the SHA-256 hash of the archive against the signature using the embedded public key, and aborts with `ERR_PACKAGE_SIGNATURE_INVALID` if verification fails.
   - Key rotation mechanism and fallback secondary keys are `TBD`.

### 4.4 Redaction of Personal Data in Logs
The logging subsystem utilizes `tracing-subscriber` configured with custom redaction filters. Before any log line is flushed to rotating disk files (`unduhkilat.log*`) or displayed in the in-app diagnostics panel (FR-056, FR-057):
- **Credential Masking**: Any header matching `Authorization`, `Proxy-Authorization`, or user-supplied password parameters is replaced with `[REDACTED]`.
- **Cookie Sanitization**: Cookie values (`Cookie`, `Set-Cookie`, `downloads.cookies`) are stripped of their values, logging only cookie names (e.g., `session_id=[REDACTED]`).
- **URL Parameter Stripping**: Query string tokens containing `token=`, `key=`, `secret=`, `auth=`, or `signature=` are redacted to `token=[REDACTED]`.

---

## 5. OWASP Mitigations

The table below presents mitigations adapted specifically to a Windows desktop client utilizing asynchronous network sockets and browser native messaging:

| OWASP Top 10 Category (Adapted) | Mitigation Strategy in UnduhKilat | Architectural Reference |
|---|---|---|
| **A01: Broken Access Control (Insecure IPC & Registry Hijack)** | Native Messaging Host validates the calling extension ID against hardcoded IDs. Admin maintenance commands assert UAC token elevation. Insecure desktop IPC mechanisms (e.g., open TCP ports) are rejected in favor of anonymous OS standard I/O pipes (FR-034, FR-035). | API.md Section 1.2 |
| **A02: Cryptographic Failures (Insecure Secret Storage & TLS)** | Plaintext credentials and proxy secrets encrypted at rest via Windows DPAPI (`CryptProtectData`). Network transfers enforce TLS 1.2/1.3 via `rustls` using system trust roots, preventing cipher downgrade attacks (FR-012, FR-014). | ARCHITECTURE.md Section 6.3 |
| **A03: Injection (SQL Injection, Path Traversal & Command Injection)** | Zero dynamic SQL concatenation; all SQLite queries use strictly parameterized statements via `rusqlite`/`sqlx`. Ingested filenames and categories are sanitized to strip path traversal sequences (`..\`, `C:`, null bytes) and invalid Windows filename characters (`<`, `>`, `:`, `"`, `/`, `\`, `?`, `*`, and the vertical-bar character) before writing to disk (FR-016, FR-029). | DATABASE.md Section 4 |
| **A04: Insecure Design (Unconfirmed Capture & Silent Overwrites)** | Strict user confirmation invariant: no download proposed from browser interception, clipboard monitoring, or batch grabber begins without an explicit user confirmation modal (FR-039, FR-040, FR-045). File collision detection prevents silent payload destruction (FR-016). | PRD.md Risk Analysis |
| **A05: Security Misconfiguration (Registry Permissions & Storage ACLs)** | First-run registration defaults strictly to the current user registry tree (`HKCU`), avoiding unnecessary administrative privileges (DEC-007, FR-034). Database files created with restrictive Windows NTFS file ACLs limited to the active interactive user token. | ARCHITECTURE.md Section 9 |
| **A06: Vulnerable and Outdated Components (Dependency Risk)** | Standalone compilation with zero runtime external dependencies (DEC-005). Rust dependencies vetted via `cargo audit` in CI pipelines. Bundled SQLite engine locked to SQLite 3.40+; OpenSSL completely excluded by choosing `rustls` (FR-012). | PRD.md Executive Summary |
| **A07: Identification and Authentication Failures (Session Reuse)** | Stored cookies and session headers captured from browsers are tied strictly to the originating host URL (`effective_url`). Plaintext passwords in memory are zeroized immediately after authentication challenge dispatch (`zeroize` crate) (FR-017). | ARCHITECTURE.md Section 4 |
| **A08: Software and Data Integrity Failures (Tampered Updates & Corruption)** | Auto-update archives undergo Ed25519 cryptographic signature verification against the embedded vendor public key before bundle extraction (FR-060). Assembled files are verified against remote `Content-Length` upon stream completion (`ERR_CHECKSUM_SIZE_MISMATCH`, FR-011). | PRD.md Section FR-060 |
| **A09: Security Logging and Monitoring Failures (Diagnostic Blindspots)** | High-fidelity local rotating file logging captures connection retries, authentication failures, and scheduler transitions without leaking secrets. In-app diagnostics panel allows immediate user troubleshooting without external data egress (FR-056, FR-057). | API.md Section 3.11 |
| **A10: Server-Side / Client-Side Request Forgery (SSRF / Unrestricted Schemes)** | URL ingestion validates URI schemes strictly against an allowed whitelist (`http`, `https`, `ftp`). Dangerous or arbitrary local URI schemes (`file://`, `javascript:`, `data:`, `ms-appx:`) are rejected immediately with `ERR_INVALID_URL_SCHEME`. | API.md Section 2.8 |

---

## 6. Third-Party Integration Security

### 6.1 Payment Gateways & External Webhooks
- **Payment Processing**: Out of scope. UnduhKilat contains no billing, payment gateway SDKs, license key activation servers, or financial transaction processing.
- **Inbound/Outbound Webhooks**: Out of scope. The engine does not expose external webhook listeners or dispatch outbound webhooks to third-party endpoints.

### 6.2 External APIs & Network Integration
1. **Remote Download Origins (HTTP/HTTPS/FTP)**:
   - UnduhKilat connects to arbitrary user-specified endpoints across the internet. The client treats all external endpoints as untrusted.
   - Redirect Safety: Follows HTTP redirects up to a maximum limit `TBD` (FR-013). Scheme switching from `https://` to `http://` raises a security warning `TBD`.
   - FTP Handshakes: Commands issued to FTP servers (`USER`, `PASS`, `REST`, `RETR`) are strictly confined to the standard RFC 959 / RFC 3659 protocol grammar (FR-012).
2. **Chromium Extension Manifest V3 Integration**:
   - The extension communicates solely with `unduhkilat-host.exe` via `chrome.runtime.connectNative("com.unduhkilat.host")`.
   - The native host manifest specifies `allowed_origins` matching only the official extension ID registered in Chrome, Edge, and Brave (DEC-006, FR-033).
   - Inbound frames from the extension are size-capped at 1 MB (matching Chromium's native messaging limit). Any malformed JSON or unexpected schema payload returns `ERR_PAYLOAD_CORRUPT`.
3. **In-Place Update Repository**:
   - Binary updates are polled from the official vendor repository over HTTPS.
   - The package payload must arrive with an accompanying detached cryptographic signature file (`.sig`).
   - The update runner extracts the archive only after signature verification passes, replacing portable files in place (DEC-007, FR-060).

---

## 7. Logging, Audit & Error Handling

### 7.1 Audited Security Events
UnduhKilat records significant operational and security events to local rotating log files (`unduhkilat.log*`) via the `tracing` framework:

1. **Application Lifecycle**: Startup crash normalization executions (reverting `running` tasks to `paused`, FR-010), shutdown flush checkpoints, and UAC elevation assertion results.
2. **Authentication Events**: HTTP 401/407 and FTP 530 authentication challenge receipts, matching credential lookups in the DPAPI vault, and DPAPI decryption failures (`ERR_AUTH_CHALLENGE_FAILED`).
3. **Filesystem Security**: Target path sanitization failures, traversal attempts, file creation errors (`ERR_PATH_UNWRITABLE`), and disk space exhaustion warnings (`ERR_STORAGE_FULL`).
4. **Data Integrity Failures**: Segment offset anomalies, byte length mismatches between disk files and source `Content-Length` (`ERR_CHECKSUM_SIZE_MISMATCH`, FR-011).
5. **Administrative Invocations**: Execution of CLI commands (`--admin-register-host`, `--admin-upgrade-bundle`, `--admin-reset-data`) including process PID and token elevation state (FR-035, FR-061, FR-062).
6. **Update Verification**: Success or cryptographic failure of update package digital signatures (`ERR_PACKAGE_SIGNATURE_INVALID`, FR-060).

### 7.2 Explicit Non-Logging Invariants (What is Never Logged)
To prevent accidental leakage of sensitive personal data or credentials into diagnostic logs (FR-057):
- **Plaintext Passwords**: Passwords ingested for host authentication, FTP access, or proxy configurations must never appear in log statements or stack traces.
- **Decrypted DPAPI Memory Buffers**: Output byte arrays from `CryptUnprotectData` are forbidden from log strings.
- **Session Cookie Payloads**: Captured `Cookie` headers and serialized session state cookies are stripped before log writing.
- **Downloaded File Payloads**: Chunk byte buffers received across network sockets are never emitted to log tracing layers.
- **Full Windows User Authentication Tokens**: NT logon tokens, Windows Hello cryptographic handles, and LSA master keys are never logged.

### 7.3 Error Handling & Information Leakage Prevention
Error responses returned across internal channels, native messaging pipes, and UI toasts follow the canonical error envelope defined in `API.md` Section 2.4:
- System panics are caught at Tokio task boundaries via [placeholder: `std::panic::catch_unwind` around the synchronous task body or handling `JoinError` from the Tokio `JoinHandle` — the owner has not decided] and translated to `ERR_INTERNAL`, preventing raw Rust stack traces and internal memory pointers from being surfaced in egui dialogs.
- Filesystem and network errors present standardized human-readable strings to the user without disclosing internal application structure, full system environment variables, or other users' profile paths.

---

## 8. Compliance & Data Retention

### 8.1 Applicable Privacy Regulations
UnduhKilat is engineered to respect personal data protection principles established under global privacy frameworks, specifically Indonesia's **Undang-Undang Pelindungan Data Pribadi (UU PDP / Law No. 27 of 2022)** and the **General Data Protection Regulation (GDPR)**:
- **Zero Remote Telemetry Mandate**: UnduhKilat does not transmit user data, download histories, URL metadata, IP addresses, system telemetry, or diagnostic logs off the local workstation (FR-057, NFR-016). No centralized telemetry server exists.
- **Local Data Controller**: Because all data processing occurs strictly on the user's local machine within their own Windows user account, the user maintains 100% control, ownership, and custodianship over their personal data.
- **Data Minimization**: The Chromium browser extension collects only navigation URLs and session cookies strictly necessary to fulfill the user's direct download or media sniffing requests (DEC-003, DEC-006, FR-036, FR-037).

### 8.2 Data Retention & Purge Rules

| Data Store / Category | Storage Location | Retention Rule / Lifecycle | Purge Mechanism |
|---|---|---|---|
| **Download History & Metadata** | SQLite `downloads` | Retained indefinitely until manually cleared by user | User-initiated `download.delete` or administrative reset (FR-062). |
| **Dynamic Segment Checkpoints** | SQLite `segments` | Retained during active, paused, or failed transfers; deleted upon task completion or deletion | Automatically purged on successful assembly or via `download.delete`. |
| **Saved Host & Proxy Credentials** | SQLite `credentials` (host) and `settings.value` (proxy) | Retained indefinitely until deleted or edited by user | User-initiated `credential.delete` or administrative reset (FR-062). |
| **Rotating Diagnostic Disk Logs** | Local file system (`.\data\logs\` or `%LOCALAPPDATA%\...`) | Bounded by size and generation count; default retention period is `TBD` | Automatic rolling purge via `tracing-appender` upon reaching size limit `TBD`. |
| **Media Candidate Sniffer Cache** | Volatile process memory | Transient; purged when the browser tab closes or when the desktop application terminates | Automatically wiped on tab close or process termination. |
| **Application State Wipe** | Entire local data tree | On-demand administrative action (FR-062) | Elevated CLI command `UnduhKilat.exe --admin-reset-data` with explicit confirmation. |

---

## 9. Penetration Test Scope

The penetration testing scope for UnduhKilat covers the native Windows desktop client, inter-process communication mechanisms, local storage durability, and network protocol parsing:

```
+-----------------------------------------------------------------------------------+
| Penetration Testing Assessment Boundaries                                         |
+-----------------------------------------------------------------------------------+
| [ IN SCOPE ]                                                                      |
| - Chromium Native Messaging Host IPC protocol (fuzzing stdin/stdout envelopes)     |
| - Manifest V3 extension privilege escalation and DOM hook injection attacks        |
| - Windows DPAPI credential protection at rest in unduhkilat.db                    |
| - Parameterized SQLite query inspection (SQL injection assessment)                |
| - Path traversal via Content-Disposition headers and URL paths                    |
| - Asynchronous HTTP/FTP stream range assembly and buffer overflow resistance       |
| - In-place update archive Ed25519 cryptographic signature verification logic      |
| - UAC token elevation checks on administrative CLI switches                       |
| - Unbounded concurrency and connection socket exhaustion handling (DEC-001)       |
+-----------------------------------------------------------------------------------+
| [ OUT OF SCOPE ]                                                                  |
| - Remote web application penetration testing (no central server exists)           |
| - Vulnerabilities in third-party origin download servers or mirrors               |
| - Physical attacks requiring hardware-level physical access to an unlocked device  |
| - Windows OS kernel privilege escalation unrelated to UnduhKilat binaries         |
| - Non-supported browser engines (Firefox, Safari; excluded by DEC-006)            |
+-----------------------------------------------------------------------------------+
```

### High-Priority Test Scenarios
1. **Native Messaging Fuzzing**: Injecting oversized, malformed, non-UTF8, or deeply nested JSON payloads into `unduhkilat-host.exe` via standard input to identify panics, buffer overflows, or unhandled errors (`ERR_PAYLOAD_CORRUPT`).
2. **Path Traversal on Ingestion**: Delivering HTTP response headers with malicious filenames (e.g., `filename="../../../../Windows/System32/calc.exe"`) to verify that destination path normalization confines files strictly to the target category folder.
3. **Malicious Extension Impersonation**: Forcing local processes to invoke `unduhkilat-host.exe` with arbitrary or spoofed extension IDs to verify origin rejection (`ERR_UNAUTHORIZED`).
4. **Signature Forgery and Package Tampering**: Modifying bytes inside an update ZIP archive or generating a forged `.sig` signature to verify that in-place updates abort without altering local binary files (`ERR_PACKAGE_SIGNATURE_INVALID`, FR-060).
5. **State File Corruption and Race Recovery**: Corrupting `unduhkilat.db` or killing the process mid-split to verify that crash normalization recovers all interrupted tasks into a resumable state without data loss or corruption (FR-010, FR-058).

---

## 10. Control to Requirement Mapping

Every security control defined in this document maps directly to functional requirements, architectural invariants, and product decisions:

| Security Control ID | Security Control Description | Functional Requirement (FR) | Section Reference |
|---|---|---|---|
| **SEC-001** | Unbounded concurrency and connection socket management | FR-001, FR-002, DEC-001 | Section 2 (STRIDE - DoS) |
| **SEC-002** | Contiguous segment offset isolation & size mismatch assertion | FR-003, FR-011 | Section 1.3, Section 4.1 |
| **SEC-003** | Startup crash normalization (reverting running to paused) | FR-010, FR-058 | Section 3.1, Section 7.1 |
| **SEC-004** | TLS 1.2/1.3 enforcement with system trust root validation | FR-012 | Section 4.1 |
| **SEC-005** | HTTP redirect safety and referer header preservation | FR-013, FR-017 | Section 6.2 |
| **SEC-006** | Windows DPAPI encryption of host and proxy credentials | FR-014, FR-015, DEC-005 | Section 3.2, Section 4.2 |
| **SEC-007** | Destination path sanitization and collision prevention | FR-016, FR-029 | Section 2 (STRIDE - Tampering) |
| **SEC-008** | Deterministic priority-then-FIFO dispatch sequencing | FR-018, DEC-002 | Section 3.3 |
| **SEC-009** | Background execution continuity and tray lifecycle security | FR-024, FR-052, DEC-004 | Section 1.2, Section 3.1 |
| **SEC-010** | Manifest V3 Chromium-only extension reach enforcement | FR-033, DEC-006 | Section 1.2, Section 6.2 |
| **SEC-011** | Current user (`HKCU`) un-elevated native host registration | FR-034, DEC-007 | Section 1.2, Section 3.3 |
| **SEC-012** | Administrative UAC token elevation assertion for `HKLM` setup | FR-035, FR-061 | Section 1.2, Section 3.3 |
| **SEC-013** | Strict interactive user confirmation modal for intercepted links | FR-039, FR-040, FR-045 | Section 2 (STRIDE - Spoofing) |
| **SEC-014** | Hybrid media candidate deduplication and canonical parsing | FR-041, FR-042, DEC-003 | Section 2 (STRIDE - DoS) |
| **SEC-015** | In-application diagnostic panel with local log tail view | FR-056, FR-057 | Section 4.4, Section 7.1 |
| **SEC-016** | Zero-telemetry local rotating log files with secret redaction | FR-057 | Section 4.4, Section 7.2, Section 8.1 |
| **SEC-017** | SQLite Write-Ahead Logging (WAL) single-writer transaction pattern | FR-058 | Section 1.2, Section 4.2 |
| **SEC-018** | Portable bundle self-contained folder security (no installer) | FR-059, DEC-007 | Section 1.2, Section 3.4 |
| **SEC-019** | Ed25519 cryptographic signature verification for bundle updates | FR-060, NFR-013 | Section 2 (STRIDE - Tampering), Section 4.3 |
| **SEC-020** | Administrative state purge verification and file isolation | FR-062 | Section 3.3, Section 8.2 |

---

## 11. Open Security Decisions (TBD)

The following security-sensitive specifications and operational parameters must be formally resolved prior to general production release:

1. **Database Encryption at Rest**: Whether SQLite database encryption (e.g., via SQLCipher or Windows BitLocker / NTFS EFS reliance) is mandatory for non-credential metadata in `unduhkilat.db` is `TBD`.
2. **DPAPI Additional Entropy Salt**: Specification of an application-wide or per-user machine entropy string passed to `CryptProtectData` (`pOptionalEntropy`) is `TBD`.
3. **Log Rotation Limits & Retention Schedule**: Maximum file size per log file before rolling, total number of retained historical log files, and automated log expiration periods are `TBD`.
4. **Update Key Management & Rotation Policy**: Operational procedures for rotating the vendor Ed25519 public update key embedded within the binary, including fallback secondary keys and revocation lists, are `TBD`.
5. **Maximum HTTP Redirect Hops**: The numerical threshold capping followed HTTP redirects before triggering `ERR_CONNECTION_DROPPED` is `TBD`.
6. **HTTPS to HTTP Protocol Downgrade Policy**: Explicit user interaction rules when a redirect transitions from HTTPS to unencrypted HTTP (automatic abort versus user confirmation modal) are `TBD`.
7. **Native Messaging Timeout Threshold**: The precise timeout in milliseconds before `unduhkilat-host.exe` terminates an unresponsive pipe connection with `ERR_NATIVE_HOST_TIMEOUT` is `TBD`.
8. **DASH / HLS Stream Capture Handling**: Security and download processing strategy for adaptive streaming manifests detected by the hybrid sniffer (FR-044) is `TBD`.