# API.md: UnduhKilat

## 1. Authentication & Authorization / Autentikasi & Otorisasi

### 1.1 Architectural Layers & Boundary Isolation
UnduhKilat is a local Windows 10/11 x64 desktop application distributed as a portable ZIP without a centralized server (DEC-005, DEC-007, FR-057). Inter-process and intra-process communication operates across three explicit transport layers:

1. **Intra-Process Tokio Async Channel (egui UI $\leftrightarrow$ Download Engine Core)**:
   - **Transport**: Strongly-typed Tokio `mpsc` and `watch` memory channels within the same executable binary (`UnduhKilat.exe`).
   - **Security Boundary**: Windows memory isolation within the local user interactive desktop session.
   - **Access**: The interactive desktop user executing the application.

2. **Native Messaging Stdin/Stdout Host (Chromium Extension $\leftrightarrow$ `unduhkilat-host.exe`)**:
   - **Transport**: Standard I/O streams using length-prefixed JSON frames in accordance with the Chromium Native Messaging protocol (DEC-006, FR-033, FR-034).
   - **Security Boundary**: The host process is launched directly by the Chromium browser (Chrome, Edge, Brave) under the current Windows user token. Communication is confined to anonymous standard input and output pipes (`stdin`/`stdout`).
   - **Origin Validation**: The Native Messaging Host verifies the calling browser extension's ID against the hardcoded allowed origins manifest registered in the Windows Registry under `HKCU\Software\Google\Chrome\NativeMessagingHosts\` (and corresponding Edge/Brave keys) (FR-034).

3. **Elevated Windows Administrator CLI (`UnduhKilat.exe --admin-*`)**:
   - **Transport**: Command-line interface and direct Win32 API invocation requiring User Account Control (UAC) elevation.
   - **Security Boundary**: Restricted to Windows accounts holding an elevated token with High or System Mandatory Integrity Level (`TOKEN_ELEVATION` verified via `GetTokenInformation`).

```
+-----------------------------------------------------------------------------------+
|                            Windows User Session                                   |
|                                                                                   |
|  +-------------------------+                     +-----------------------------+  |
|  | Chromium Extension      |   Native Messaging  | Native Messaging Host       |  |
|  | (MV3: Chrome/Edge/Brave)| <=================> | (unduhkilat-host.exe)        |  |
|  +-------------------------+   (stdin / stdout)  +-----------------------------+  |
|                                                                 || Local IPC      |
|  +-------------------------+                                    || (Named Pipe    |
|  | egui Desktop UI         |                                    || / Shared State)|
|  | (Renderer: wgpu)        | <==================================++                |
|  +-------------------------+     Intra-Process Tokio Channels                     |
|                                  (mpsc / broadcast / watch)                       |
|                                                ||                                 |
|                                  +-----------------------------+                  |
|                                  | Tokio Engine & Scheduler    |                  |
|                                  | (reqwest + rusqlite WAL)    |                  |
|                                  +-----------------------------+                  |
+-----------------------------------------------------------------------------------+
                                                 || UAC Elevation
+-----------------------------------------------------------------------------------+
| Elevated Administrative Commands (HKLM Registry, Machine-Wide Bundle Reset)        |
+-----------------------------------------------------------------------------------+
```

### 1.2 Identity Context & Message Validation
Because UnduhKilat contains no cloud identity provider or remote web API, authorization is derived strictly from the caller's execution context:

- **End User Context**: Operates with standard unprivileged user credentials. All requests from the desktop UI are intrinsically authenticated. Reusable host and proxy credentials stored in the `credentials` and `settings` SQLite tables are unshielded in-memory only when needed, protected at rest using Windows DPAPI (`CryptProtectData`) (DEC-005, FR-014, FR-015).
- **Extension Context**: Every Native Messaging frame must contain a valid message envelope. The host rejects frames originating from unregistered extension IDs with `ERR_UNAUTHORIZED`.
- **Administrator Context**: Administrative maintenance operations require process elevation. If `unduhkilat-host.exe` or `UnduhKilat.exe` detects an unelevated caller attempting an administrative endpoint, the operation fails immediately with `ERR_PERMISSION_DENIED` (FR-035, FR-061, FR-062).

### 1.3 Permission Matrix per Endpoint Group

The matrix maps operational roles defined in the PRD to endpoint/command domains:

| Command / Endpoint Group | End User | Browser Extension | Administrator | Transport / Interface |
|---|:---:|:---:|:---:|---|
| **Download Control** (`download.*`) | Full (CRUD, Pause, Resume, Restart) | Read, Intercept, Status Query | Read-only | Internal Channel / Native Messaging |
| **Queue & Scheduling** (`queue.*`, `schedule.*`) | Full (Reorder, Priority, Start, Stop) | None | None | Internal Channel |
| **Bandwidth & Limits** (`speed.*`, `concurrency.*`) | Full (Global & Per-download caps) | None | None | Internal Channel |
| **Categories & Folders** (`category.*`) | Full (Create, Edit, Delete, Map) | Query Default Paths | None | Internal Channel / Native Messaging |
| **Browser Interception** (`browser.*`) | Review & Confirm Dialogs | Emit Captured Links, Emit Media Hooks | None | Native Messaging |
| **Media Sniffer** (`media.*`) | Preview, Select & Enqueue | Hook Signals, Network Candidates | None | Native Messaging / Internal Channel |
| **Site Grabber / Batch** (`grabber.*`) | Configure Crawl, Preview, Enqueue | None | None | Internal Channel |
| **Diagnostics & Logs** (`diagnostics.*`) | View Logs, Export Local Files | None | None | Internal Channel |
| **Machine Registry Setup** (`admin.registry.*`) | None | None | Execute (`HKLM`) | Elevated CLI (`[placeholder]`) |
| **Bundle & Data Maintenance** (`admin.bundle.*`) | `[placeholder]` | None | Execute (Reset, Upgrade) | Elevated CLI (`--admin-reset`) |

---

## 2. Standard Response & Pagination Formats / Format Respons Standar & Paginasi

### 2.1 Native Messaging & Internal IPC Wire Format
All messages exchanged over standard input/output (Native Messaging) adhere to the Chromium 32-bit native byte-order length prefix, followed by a UTF-8 JSON payload:

```
+-----------------------------------+--------------------------------------------+
| 32-bit uint (Little-Endian Length)| UTF-8 JSON String of exact length          |
| [ 0x2A, 0x01, 0x00, 0x00 ]        | { "id": "req_...", "command": "...", ... } |
+-----------------------------------+--------------------------------------------+
```

Intra-process commands between the egui GUI and Tokio engine utilize the identical JSON-serializable Rust enum structures via `serde_json`, guaranteeing contract consistency across internal and native IPC boundaries.

### 2.2 Standard Request Envelope
Every request or command sent to the engine core carries a correlation identifier, command name, and payload object:

```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M51A",
  "command": "download.create",
  "payload": {
    "original_url": "https://releases.example.test/packages/runtime.zip",
    "target_path": "C:\\Downloads\\Compressed\\runtime.zip",
    "file_name": "runtime.zip",
    "connection_count": 8,
    "speed_limit_kbps": 0,
    "category_id": "c1f8a840-7e4b-449e-8a03-7bf1c3609801",
    "priority": 0
  }
}
```

#### Request Fields
- `id` (`string`, required): Unique request correlation identifier. Prefixed with `req_` followed by a 26-character Crockford Base32 ULID.
- `command` (`string`, required): Dot-notated command identifier matching domain boundaries (`download.create`, `queue.reorder`, `browser.intercept`).
- `payload` (`object`, required): Command-specific parameter map. Keys match `DATABASE.md` column names exactly.

### 2.3 Standard Success Response Envelope
All successful operations return an envelope containing `status: "ok"`, mirroring the original request correlation identifier:

```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M51A",
  "data": {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "state": "queued",
    "downloaded_bytes": 0,
    "total_bytes": 104857600,
    "created_at": "2026-03-30T14:22:10.123Z"
  }
}
```

### 2.4 Standard Error Response Envelope
If an operation fails, the engine responds with `status: "error"`, returning a canonical error code, descriptive human-readable message, and optional contextual details:

```json
{
  "status": "error",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M51A",
  "error": {
    "code": "ERR_RANGE_NOT_SUPPORTED",
    "message": "Remote server does not advertise HTTP Accept-Ranges support.",
    "details": {
      "download_id": "550e8400-e29b-41d4-a716-446655440000",
      "effective_url": "https://releases.example.test/packages/runtime.zip",
      "http_status": 200,
      "accept_ranges_header": null
    }
  }
}
```

#### Error Envelope Fields
- `code` (`string`, required): Canonical machine-readable error token formatted in `UPPER_SNAKE_CASE` (see Section 2.7).
- `message` (`string`, required): Human-readable summary suitable for display in egui diagnostic dialogs or toasts.
- `details` (`object`, optional): Structured diagnostic metadata capturing URI references, system OS errors, or schema validation failures.

### 2.5 Real-Time Push Event Envelope (Engine $\rightarrow$ UI / Extension)
For asynchronous updates (e.g., segment progress, queue dispatch state, media detection candidates), the engine emits unsolicited event frames:

```json
{
  "type": "event",
  "event": "download.progress",
  "payload": {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "state": "running",
    "downloaded_bytes": 52428800,
    "total_bytes": 104857600,
    "current_speed_bps": 10485760,
    "active_connections": 8,
    "eta_seconds": 5
  }
}
```

### 2.6 Pagination Specification
List queries (`download.list`, `diagnostics.log_tail`, `grabber.preview`) implement deterministic limit/offset or cursor-based pagination. In accordance with DEC-001 (unbounded user preferences), listing endpoints default to 50 items but permit unbounded or user-specified page sizes:

#### List Request Parameters
| Parameter | Type | Default | Constraint | Description |
|---|---|---|---|---|
| `limit` | `integer` | `50` | $\ge 1$, no upper bound (DEC-001) | Maximum number of records to return. |
| `cursor` | `string` | `null` | Optional string | Opaque base64 cursor encoding `(created_at, id)` for stable seek pagination. |
| `state` | `string` | `null` | Must match `DATABASE.md` states | Optional filter by operational state (`queued`, `running`, `paused`, etc.). |
| `category_id` | `string` | `null` | Valid Category UUIDv4 | Optional filter by category identifier. |

#### Paginated Response Envelope
```json
{
  "status": "ok",
  "id": "req_01HP8XA3M4NDJ2G5P0TR9V8Q1X",
  "data": {
    "items": [
      {
        "id": "550e8400-e29b-41d4-a716-446655440000",
        "file_name": "runtime.zip",
        "state": "running",
        "total_bytes": 104857600,
        "downloaded_bytes": 52428800,
        "created_at": "2026-03-30T14:22:10.123Z"
      }
    ],
    "pagination": {
      "limit": 50,
      "total_count": 142,
      "has_more": true,
      "next_cursor": "eyJjcmVhdGVkX2F0IjoiMjAyNi0wMy0zMFQxNDoyMjoxMC4xMjNaIiwiaWQiOiI1NTBlODQwMC1lMjliLTQxZDQtYTcxNi00NDY2NTU0NDAwMDAifQ=="
    }
  }
}
```

### 2.7 Idempotency & Concurrency Rules
1. **Mutation Idempotency**: State-changing commands (`download.pause`, `download.resume`, `download.restart`, `queue.start_all`, `queue.stop_all`) are strictly idempotent. Invoking `download.pause` on an already paused or failed download returns `status: "ok"` with the unchanged state and triggers no redundant disk I/O.
2. **Duplicate Ingestion Prevention**: Calling `download.create` or `browser.intercept` with an existing `original_url` and matching target filename does not create an unmanaged duplicate; the engine evaluates the conflict policy (`ERR_NAME_COLLISION` or auto-rename `[placeholder]` per FR-016).
3. **Queue Reordering**: Batched queue re-indexing (`queue.reorder`) runs within a single SQLite transaction, ensuring atomic sequence assignment across `queue_items.queue_position` (DEC-002, FR-021).

### 2.8 Canonical Error Code Catalogue
The following error codes are authoritative across all UI error handling, logs, and extension toasts (matching `USER_FLOW.md` Section 3 and `DATABASE.md` invariants):

| Error Code | HTTP Equiv / Category | Description | Failure Matrix Cross-Ref |
|---|:---:|---|---|
| `ERR_STORAGE_INIT_FAILED` | Internal I/O | SQLite database initialization or schema migration failed on startup. | USER_FLOW Flow 1 |
| `ERR_REGISTRY_WRITE_FAILED` | Win32 System | Registry key creation under `HKCU` or `HKLM` failed due to permissions or lock. | USER_FLOW Flow 1 |
| `ERR_NATIVE_HOST_NOT_FOUND` | Browser IPC | Browser extension cannot locate or execute the registered host binary. | USER_FLOW Flow 1 |
| `ERR_NATIVE_HOST_TIMEOUT` | Browser IPC | Stdin/stdout pipe failed to respond within timeout `[placeholder]`. | USER_FLOW Flow 1 |
| `ERR_PAYLOAD_CORRUPT` | Validation | Native messaging frame payload is malformed or violates JSON schema. | USER_FLOW Flow 2 |
| `ERR_INVALID_URL_SCHEME` | Validation | Provided URI protocol is not supported (only `http`, `https`, `ftp` allowed). | USER_FLOW Flow 2, FR-012 |
| `ERR_MEDIA_PARSE_FAILED` | Sniffer Engine | Sniffed media resource is an unsupported or unparsed stream manifest (DASH/HLS). | USER_FLOW Flow 2, FR-044 |
| `ERR_PATH_UNWRITABLE` | Filesystem | Destination directory path does not exist and cannot be created. | USER_FLOW Flow 2, FR-029 |
| `ERR_NAME_COLLISION` | Filesystem | Destination file already exists on disk and conflict policy requires action. | USER_FLOW Flow 3, FR-016 |
| `ERR_HTTP_AUTH_REQUIRED` | Network Auth | Target server returned 401 Unauthorized or 407 Proxy Authentication Required. | USER_FLOW Flow 3, FR-014 |
| `ERR_RANGE_NOT_SUPPORTED` | Protocol | Remote server does not advertise or accept `Range: bytes` requests. | USER_FLOW Flow 3, FR-005 |
| `ERR_RANGE_NOT_SATISFIABLE`| Protocol | Remote server rejected requested byte range with HTTP 416 Range Not Satisfiable. | USER_FLOW Flow 3, FR-005 |
| `ERR_STORAGE_FULL` | Filesystem | Local storage drive has insufficient disk space to write incoming chunks. | USER_FLOW Flow 3 |
| `ERR_CHECKSUM_SIZE_MISMATCH`| Integrity | Verified file byte length on completion does not equal expected `total_bytes`. | USER_FLOW Flow 3, FR-011 |
| `ERR_CONNECTION_DROPPED` | Network | Active TCP socket connection dropped and exhausted configured retry attempts. | USER_FLOW Flow 4, FR-008 |
| `ERR_FILE_NOT_FOUND` | Filesystem | Partial download payload or temporary file on disk is missing upon resume. | USER_FLOW Flow 4 |
| `ERR_INVALID_PATH` | Validation | Category or settings destination path is not a valid Windows absolute path. | USER_FLOW Flow 5, FR-031 |
| `ERR_CRAWL_FAILED` | Batch Crawl | Site Grabber depth-limited crawl halted due to network or DNS resolution failure. | USER_FLOW Flow 5, FR-046 |
| `ERR_EXPORT_FAILED` | Filesystem | Diagnostics log export failed due to target folder permission denial. | USER_FLOW Flow 5, FR-056 |
| `ERR_PACKAGE_SIGNATURE_INVALID` | Security | In-place update archive signature verification failed or is unsigned. | USER_FLOW Flow 5, FR-060 |
| `ERR_UNAUTHORIZED` | Security | Calling extension ID does not match permitted native messaging manifest. | FR-033, FR-034 |
| `ERR_PERMISSION_DENIED` | Security | Caller lacks Windows UAC administrative elevation required for `HKLM` operations. | FR-035, FR-061, FR-062 |
| `ERR_INVALID_ARGUMENT` | Validation | Command parameter value violates domain validation rules. | FR-001, FR-002 |
| `ERR_NOT_FOUND` | State Store | Requested entity UUIDv4 does not exist in SQLite tables. | DATABASE.md |
| `ERR_ALREADY_EXISTS` | State Store | Unique constraint violation on category name or credential host, port, username, and protocol identity. | DATABASE.md |
| `ERR_INTERNAL` | Runtime | Unhandled Tokio async task panic or low-level SQLite driver fault. | FR-058 |

## 3. API Endpoints / Endpoint API

### 3.1 Download Management Domain (`download.*`)

#### 3.1.1 `download.create`
- **Command / Method**: `download.create` (Internal Channel / IPC)
- **Description**: Creates a new download task in the SQLite state database (`downloads` and `queue_items`), initializes file target validation, and assigns an initial state of `queued` (or `paused` if scheduled for later) (FR-001, FR-002, FR-016, FR-039).
- **Auth and Permission**: End User role.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M501",
  "command": "download.create",
  "payload": {
    "original_url": "https://releases.example.test/files/ubuntu-24.04.iso",
    "file_name": "ubuntu-24.04.iso",
    "target_path": "C:\\Downloads\\Images\\ubuntu-24.04.iso",
    "total_bytes": 6048576000,
    "connection_count": 16,
    "speed_limit_kbps": 0,
    "category_id": "8a3d5e21-3c4b-4f8a-9e10-1a2b3c4d5e6f",
    "mime_type": "application/x-iso9660-image",
    "referer": "https://releases.example.test/",
    "user_agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64)",
    "cookies": "session_id=xyz789; auth_token=abc123",
    "priority": 1,
    "start_immediately": true
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M501",
  "data": {
    "id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
    "original_url": "https://releases.example.test/files/ubuntu-24.04.iso",
    "effective_url": "https://releases.example.test/files/ubuntu-24.04.iso",
    "file_name": "ubuntu-24.04.iso",
    "target_path": "C:\\Downloads\\Images\\ubuntu-24.04.iso",
    "total_bytes": 6048576000,
    "downloaded_bytes": 0,
    "state": "queued",
    "segmentation_supported": 1,
    "connection_count": 16,
    "speed_limit_kbps": 0,
    "category_id": "8a3d5e21-3c4b-4f8a-9e10-1a2b3c4d5e6f",
    "mime_type": "application/x-iso9660-image",
    "created_at": "2026-03-30T14:30:00.000Z",
    "updated_at": "2026-03-30T14:30:00.000Z"
  }
}
```
- **Error Codes**:
  - `ERR_INVALID_URL_SCHEME`: URL scheme is not `http`, `https`, or `ftp`.
  - `ERR_INVALID_PATH`: Target directory format is invalid under Windows path rules.
  - `ERR_PATH_UNWRITABLE`: Target filesystem directory cannot be accessed or created.
  - `ERR_NAME_COLLISION`: Target file exists and conflict prompt resolution was declined.
  - `ERR_STORAGE_FULL`: Drive hosting `target_path` lacks space for pre-allocation.
  - `ERR_INVALID_ARGUMENT`: `connection_count` < 1 or parameters fail schema validation.

---

#### 3.1.2 `download.list`
- **Command / Method**: `download.list` (Internal Channel / IPC)
- **Description**: Returns a paginated and filterable collection of downloads for egui download list rendering, supporting sorting by status, category, date, or speed (FR-048).
- **Auth and Permission**: End User, Administrator (Read-only).
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M502",
  "command": "download.list",
  "payload": {
    "limit": 50,
    "cursor": null,
    "state": "running",
    "category_id": null
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M502",
  "data": {
    "items": [
      {
        "id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
        "file_name": "ubuntu-24.04.iso",
        "total_bytes": 6048576000,
        "downloaded_bytes": 1572864000,
        "state": "running",
        "connection_count": 16,
        "speed_limit_kbps": 0,
        "category_id": "8a3d5e21-3c4b-4f8a-9e10-1a2b3c4d5e6f",
        "created_at": "2026-03-30T14:30:00.000Z",
        "updated_at": "2026-03-30T14:31:15.123Z"
      }
    ],
    "pagination": {
      "limit": 50,
      "total_count": 1,
      "has_more": false,
      "next_cursor": null
    }
  }
}
```
- **Error Codes**:
  - `ERR_INVALID_ARGUMENT`: Invalid cursor decoding or negative limit requested.

---

#### 3.1.3 `download.get`
- **Command / Method**: `download.get` (Internal Channel / IPC)
- **Description**: Fetches single download task metadata, header configuration, and execution flags by UUIDv4 (FR-048, FR-055).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M503",
  "command": "download.get",
  "payload": {
    "id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M503",
  "data": {
    "id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
    "original_url": "https://releases.example.test/files/ubuntu-24.04.iso",
    "effective_url": "https://mirrors.edge.example.test/files/ubuntu-24.04.iso",
    "target_path": "C:\\Downloads\\Images\\ubuntu-24.04.iso",
    "file_name": "ubuntu-24.04.iso",
    "total_bytes": 6048576000,
    "downloaded_bytes": 1572864000,
    "state": "running",
    "segmentation_supported": 1,
    "connection_count": 16,
    "speed_limit_kbps": 0,
    "category_id": "8a3d5e21-3c4b-4f8a-9e10-1a2b3c4d5e6f",
    "mime_type": "application/x-iso9660-image",
    "referer": "https://releases.example.test/",
    "user_agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64)",
    "cookies": "session_id=xyz789; auth_token=abc123",
    "error_code": null,
    "error_message": null,
    "created_at": "2026-03-30T14:30:00.000Z",
    "updated_at": "2026-03-30T14:31:15.123Z",
    "completed_at": null
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: Download UUIDv4 does not exist in `downloads` table.

---

#### 3.1.4 `download.pause`
- **Command / Method**: `download.pause` (Internal Channel / IPC)
- **Description**: Cancels in-flight Tokio connection workers, flushes buffered chunks and segment `highest_contiguous_offset` values to SQLite transactionally, and marks the task `state = paused` (FR-006, FR-058). Idempotent if already paused.
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M504",
  "command": "download.pause",
  "payload": {
    "id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M504",
  "data": {
    "id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
    "state": "paused",
    "downloaded_bytes": 1625292800,
    "updated_at": "2026-03-30T14:31:45.789Z"
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: Download UUIDv4 does not exist.
  - `ERR_STORAGE_FULL`: Storage write failure during progress flush.

---

#### 3.1.5 `download.resume`
- **Command / Method**: `download.resume` (Internal Channel / IPC)
- **Description**: Resumes an interrupted or paused download. If concurrency permits, transitions state to `running`, reads persisted segment offsets from `segments`, and initiates byte-range HTTP/FTP requests starting strictly from `highest_contiguous_offset` (FR-007, FR-010, FR-020). If concurrency limit is reached, transitions to `queued`.
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M505",
  "command": "download.resume",
  "payload": {
    "id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M505",
  "data": {
    "id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
    "state": "running",
    "active_connections": 16,
    "updated_at": "2026-03-30T14:32:00.123Z"
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: Download UUIDv4 does not exist.
  - `ERR_FILE_NOT_FOUND`: Checkpointed partial file was moved or deleted from disk.
  - `ERR_RANGE_NOT_SUPPORTED`: Server rejected range resumption request.
  - `ERR_HTTP_AUTH_REQUIRED`: Server rejected resumed transfer due to expired auth.

---

#### 3.1.6 `download.restart`
- **Command / Method**: `download.restart` (Internal Channel / IPC)
- **Description**: Discards persisted segment records, truncates the local partial file to 0 bytes, resets `downloaded_bytes = 0`, and fetches the resource anew from byte offset 0 (FR-009).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M506",
  "command": "download.restart",
  "payload": {
    "id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M506",
  "data": {
    "id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
    "state": "running",
    "downloaded_bytes": 0,
    "updated_at": "2026-03-30T14:32:30.000Z"
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: Download UUIDv4 does not exist.
  - `ERR_PATH_UNWRITABLE`: File cannot be truncated due to OS lock or permission.

---

#### 3.1.7 `download.cancel`
- **Command / Method**: `download.cancel` (Internal Channel / IPC)
- **Description**: Stops transfer workers, transitions state to `cancelled`, removes entry from `queue_items`, and optionally deletes the partial file payload from the Windows filesystem.
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M507",
  "command": "download.cancel",
  "payload": {
    "id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
    "delete_file": true
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M507",
  "data": {
    "id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
    "state": "cancelled",
    "file_deleted": true,
    "updated_at": "2026-03-30T14:33:00.000Z"
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: Download UUIDv4 does not exist.

---

#### 3.1.8 `download.delete`
- **Command / Method**: `download.delete` (Internal Channel / IPC)
- **Description**: Purges record from `downloads` (cascading deletes to `segments` and `queue_items`). If `delete_file = true`, deletes the payload on disk (FR-011).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M508",
  "command": "download.delete",
  "payload": {
    "id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
    "delete_file": false
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M508",
  "data": {
    "id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
    "deleted": true
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: Download UUIDv4 does not exist.

---

#### 3.1.9 `download.get_segments`
- **Command / Method**: `download.get_segments` (Internal Channel / IPC)
- **Description**: Returns dynamic byte-range segments and checkpointed progress for the per-item details view (FR-055).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M509",
  "command": "download.get_segments",
  "payload": {
    "download_id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M509",
  "data": {
    "download_id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
    "active_connections": 2,
    "segments": [
      {
        "id": 101,
        "segment_index": 0,
        "range_start": 0,
        "range_end": 3024288000,
        "highest_contiguous_offset": 1048576000,
        "state": "downloading",
        "updated_at": "2026-03-30T14:31:45.000Z"
      },
      {
        "id": 102,
        "segment_index": 1,
        "range_start": 3024288001,
        "range_end": 6048575999,
        "highest_contiguous_offset": 3548576001,
        "state": "downloading",
        "updated_at": "2026-03-30T14:31:45.000Z"
      }
    ]
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: Download UUIDv4 does not exist.

---

### 3.2 Queue & Scheduler Domain (`queue.*`, `schedule.*`)

#### 3.2.1 `queue.list`
- **Command / Method**: `queue.list` (Internal Channel / IPC)
- **Description**: Retrieves items ordered by the authoritative dispatch rule: `priority DESC, queue_position ASC` (DEC-002, FR-018, FR-048).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M510",
  "command": "queue.list",
  "payload": {
    "limit": 50,
    "cursor": null
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M510",
  "data": {
    "items": [
      {
        "id": "q1e2a3b4-5c6d-7e8f-9a0b-1c2d3e4f5a6b",
        "download_id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
        "file_name": "ubuntu-24.04.iso",
        "priority": 2,
        "queue_position": 0,
        "state": "running",
        "created_at": "2026-03-30T14:30:00.000Z"
      },
      {
        "id": "q2e2a3b4-5c6d-7e8f-9a0b-1c2d3e4f5a6c",
        "download_id": "8c9e2f3a-6d4e-5f0a-9b2c-3d4e5f6a7b8c",
        "file_name": "compiler-msvc.zip",
        "priority": 0,
        "queue_position": 1,
        "state": "queued",
        "created_at": "2026-03-30T14:30:10.000Z"
      }
    ],
    "pagination": {
      "limit": 50,
      "total_count": 2,
      "has_more": false,
      "next_cursor": null
    }
  }
}
```
- **Error Codes**:
  - `ERR_INVALID_ARGUMENT`: Invalid pagination arguments.

---

#### 3.2.2 `queue.reorder`
- **Command / Method**: `queue.reorder` (Internal Channel / IPC)
- **Description**: Batches reassignment of `queue_position` values within an atomic SQLite transaction, defining manual FIFO positions for items with equal priority (DEC-002, FR-021).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M511",
  "command": "queue.reorder",
  "payload": {
    "ordered_download_ids": [
      "8c9e2f3a-6d4e-5f0a-9b2c-3d4e5f6a7b8c",
      "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b"
    ]
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M511",
  "data": {
    "reordered_count": 2,
    "updated_at": "2026-03-30T14:34:00.000Z"
  }
}
```
- **Error Codes**:
  - `ERR_INVALID_ARGUMENT`: Empty list or mismatched download identifiers.
  - `ERR_NOT_FOUND`: Referenced download identifier does not exist in `queue_items`.

---

#### 3.2.3 `queue.set_priority`
- **Command / Method**: `queue.set_priority` (Internal Channel / IPC)
- **Description**: Sets an item's manual execution priority (`priority`); the clear representation is `[placeholder]`. Takes immediate effect at the next scheduler dispatch decision without disturbing running tasks (DEC-002, FR-019).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M512",
  "command": "queue.set_priority",
  "payload": {
    "download_id": "8c9e2f3a-6d4e-5f0a-9b2c-3d4e5f6a7b8c",
    "priority": 5
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M512",
  "data": {
    "download_id": "8c9e2f3a-6d4e-5f0a-9b2c-3d4e5f6a7b8c",
    "priority": 5,
    "updated_at": "2026-03-30T14:34:30.000Z"
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: Download UUIDv4 does not exist in `queue_items`.

---

#### 3.2.4 `queue.start_all`
- **Command / Method**: `queue.start_all` (Internal Channel / IPC)
- **Description**: Transitions all paused or waiting queue items to `queued` or `running` (bounded by `concurrency_limit`), maintaining strict priority-then-FIFO order (FR-020, FR-022).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M513",
  "command": "queue.start_all",
  "payload": {}
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M513",
  "data": {
    "started_count": 4,
    "running_count": 2,
    "queued_count": 2
  }
}
```
- **Error Codes**: None (idempotent operation).

---

#### 3.2.5 `queue.stop_all`
- **Command / Method**: `queue.stop_all` (Internal Channel / IPC)
- **Description**: Suspends all active transfers across the queue, commits segment progress checkpoints to SQLite, and transitions items to `state = paused` (FR-022).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M514",
  "command": "queue.stop_all",
  "payload": {}
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M514",
  "data": {
    "paused_count": 2
  }
}
```
- **Error Codes**: None (idempotent operation).

---

#### 3.2.6 `schedule.create`
- **Command / Method: `schedule.create` (Internal Channel / IPC)
- **Description**: Defines an automated queue execution window or recurring daily time trigger in the `schedules` table, evaluated by Tokio timers whether the UI is visible or hidden to the system tray (DEC-004, FR-023, FR-024).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M515",
  "command": "schedule.create",
  "payload": {
    "name": "Nightly Schedule",
    "start_time": "02:00:00",
    "stop_time": "06:00:00",
    "days_of_week": "1,2,3,4,5",
    "is_active": true
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M515",
  "data": {
    "id": "s1a2b3c4-5d6e-7f8a-9b0c-1d2e3f4a5b6c",
    "name": "Nightly Schedule",
    "start_time": "02:00:00",
    "stop_time": "06:00:00",
    "days_of_week": "1,2,3,4,5",
    "is_active": 1,
    "created_at": "2026-03-30T14:35:00.000Z",
    "updated_at": "2026-03-30T14:35:00.000Z"
  }
}
```
- **Error Codes**:
  - `ERR_INVALID_ARGUMENT`: Both `start_time` and `stop_time` are null, time string format is not `HH:MM:SS`, or `days_of_week` contains invalid weekday tokens.

---

#### 3.2.7 `schedule.list`
- **Command / Method**: `schedule.list` (Internal Channel / IPC)
- **Description**: Returns all user-configured schedule definitions (FR-023, FR-050).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M516",
  "command": "schedule.list",
  "payload": {}
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M516",
  "data": {
    "items": [
      {
        "id": "s1a2b3c4-5d6e-7f8a-9b0c-1d2e3f4a5b6c",
        "name": "Nightly Schedule",
        "start_time": "02:00:00",
        "stop_time": "06:00:00",
        "days_of_week": "1,2,3,4,5",
        "is_active": 1,
        "created_at": "2026-03-30T14:35:00.000Z",
        "updated_at": "2026-03-30T14:35:00.000Z"
      }
    ]
  }
}
```
- **Error Codes**: None.

---

#### 3.2.8 `schedule.update`
- **Command / Method**: `schedule.update` (Internal Channel / IPC)
- **Description**: Modifies schedule time triggers, active days, or toggles activation state (`is_active`) (FR-023).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M517",
  "command": "schedule.update",
  "payload": {
    "id": "s1a2b3c4-5d6e-7f8a-9b0c-1d2e3f4a5b6c",
    "name": "Weekend Batch",
    "start_time": "01:00:00",
    "stop_time": "07:00:00",
    "days_of_week": "6,7",
    "is_active": true
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M517",
  "data": {
    "id": "s1a2b3c4-5d6e-7f8a-9b0c-1d2e3f4a5b6c",
    "name": "Weekend Batch",
    "start_time": "01:00:00",
    "stop_time": "07:00:00",
    "days_of_week": "6,7",
    "is_active": 1,
    "updated_at": "2026-03-30T14:36:00.000Z"
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: Schedule UUIDv4 does not exist.
  - `ERR_INVALID_ARGUMENT`: Invalid time format or empty time window.

---

#### 3.2.9 `schedule.delete`
- **Command / Method**: `schedule.delete` (Internal Channel / IPC)
- **Description**: Permanently removes a schedule entry from SQLite (FR-023).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M518",
  "command": "schedule.delete",
  "payload": {
    "id": "s1a2b3c4-5d6e-7f8a-9b0c-1d2e3f4a5b6c"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M518",
  "data": {
    "id": "s1a2b3c4-5d6e-7f8a-9b0c-1d2e3f4a5b6c",
    "deleted": true
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: Schedule UUIDv4 does not exist.

---

### 3.3 Bandwidth & Speed Control Domain (`speed.*`)

#### 3.3.1 `speed.set_global`
- **Command / Method**: `speed.set_global` (Internal Channel / IPC)
- **Description**: Sets the aggregate download throughput cap across all running Tokio connection tasks in KB/s. Value `0` lifts all limits (unlimited). Persisted to `settings` under key `global_speed_limit_kbps` and applied live within the next throttle interval without pausing or restarting transfers (FR-025, FR-028).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M519",
  "command": "speed.set_global",
  "payload": {
    "limit_kbps": 2048
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M519",
  "data": {
    "global_speed_limit_kbps": 2048,
    "applied_live": true,
    "updated_at": "2026-03-30T14:37:00.000Z"
  }
}
```
- **Error Codes**:
  - `ERR_INVALID_ARGUMENT`: Negative integer value provided.

---

#### 3.3.2 `speed.set_download`
- **Command / Method**: `speed.set_download` (Internal Channel / IPC)
- **Description**: Configures a per-download speed limit in KB/s, updated directly on `downloads.speed_limit_kbps`. Value `0` lifts the per-item limit so only the global limit applies. Takes effect live; when `item_limit = 0`, effective throughput is `global_limit`; otherwise it is $\min(\text{global\_limit}, \text{item\_limit})$ (FR-026, FR-027, FR-028).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M520",
  "command": "speed.set_download",
  "payload": {
    "download_id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
    "limit_kbps": 512
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M520",
  "data": {
    "download_id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
    "speed_limit_kbps": 512,
    "effective_limit_kbps": 512,
    "applied_live": true,
    "updated_at": "2026-03-30T14:37:30.000Z"
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: Download UUIDv4 does not exist.
  - `ERR_INVALID_ARGUMENT`: Negative integer provided.

---

#### 3.3.3 `speed.get_status`
- **Command / Method**: `speed.get_status` (Internal Channel / IPC)
- **Description**: Returns live measured throughput (aggregate bytes per second across all connections) and effective bandwidth limit configuration for the egui status bar (FR-027, FR-048).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M521",
  "command": "speed.get_status",
  "payload": {}
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M521",
  "data": {
    "current_aggregate_bps": 2097152,
    "global_speed_limit_kbps": 2048,
    "active_downloads_count": 2,
    "active_connections_count": 24
  }
}
```
- **Error Codes**: None.

---

### 3.4 Categories & Destinations Domain (`category.*`)

#### 3.4.1 `category.list`
- **Command / Method**: `category.list` (Internal Channel / IPC / Native Messaging)
- **Description**: Lists all defined file categories, extension sets, and destination directory paths (FR-029, FR-031).
- **Auth and Permission**: End User, Browser Extension (Read-only).
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M522",
  "command": "category.list",
  "payload": {}
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M522",
  "data": {
    "items": [
      {
        "id": "c1f8a840-7e4b-449e-8a03-7bf1c3609801",
        "name": "Compressed",
        "target_directory": "C:\\Downloads\\Compressed",
        "file_extensions": "zip,rar,7z,tar,gz,bz2,xz",
        "is_default": 0,
        "created_at": "2026-03-30T14:00:00.000Z",
        "updated_at": "2026-03-30T14:00:00.000Z"
      },
      {
        "id": "c2f8a840-7e4b-449e-8a03-7bf1c3609802",
        "name": "General",
        "target_directory": "C:\\Downloads\\General",
        "file_extensions": "*",
        "is_default": 1,
        "created_at": "2026-03-30T14:00:00.000Z",
        "updated_at": "2026-03-30T14:00:00.000Z"
      }
    ]
  }
}
```
- **Error Codes**: None.

---

#### 3.4.2 `category.create`
- **Command / Method**: `category.create` (Internal Channel / IPC)
- **Description**: Creates a new file category with target directory and extension bindings in the `categories` table (FR-029, FR-031).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M523",
  "command": "category.create",
  "payload": {
    "name": "Disk Images",
    "target_directory": "D:\\Images\\ISOs",
    "file_extensions": "iso,img,vhd,vmdk",
    "is_default": false
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M523",
  "data": {
    "id": "c3f8a840-7e4b-449e-8a03-7bf1c3609803",
    "name": "Disk Images",
    "target_directory": "D:\\Images\\ISOs",
    "file_extensions": "iso,img,vhd,vmdk",
    "is_default": 0,
    "created_at": "2026-03-30T14:38:00.000Z",
    "updated_at": "2026-03-30T14:38:00.000Z"
  }
}
```
- **Error Codes**:
  - `ERR_ALREADY_EXISTS`: Category name already exists.
  - `ERR_INVALID_PATH`: Directory path is invalid under Windows syntax.
  - `ERR_INVALID_ARGUMENT`: Empty category name or missing extension list.

---

#### 3.4.3 `category.update`
- **Command / Method**: `category.update` (Internal Channel / IPC)
- **Description**: Updates category name, extensions, or directory. Applying new directory paths affects only future downloads; existing completed files remain at their original destination (FR-031).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M524",
  "command": "category.update",
  "payload": {
    "id": "c3f8a840-7e4b-449e-8a03-7bf1c3609803",
    "name": "Virtual Disk Images",
    "target_directory": "E:\\VM\\Disks",
    "file_extensions": "iso,img,vhd,vhdx,vmdk,qcow2",
    "is_default": false
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M524",
  "data": {
    "id": "c3f8a840-7e4b-449e-8a03-7bf1c3609803",
    "name": "Virtual Disk Images",
    "target_directory": "E:\\VM\\Disks",
    "file_extensions": "iso,img,vhd,vhdx,vmdk,qcow2",
    "is_default": 0,
    "updated_at": "2026-03-30T14:38:30.000Z"
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: Category UUIDv4 does not exist.
  - `ERR_ALREADY_EXISTS`: Category name conflicts with another entry.
  - `ERR_INVALID_PATH`: Target directory path format is invalid.

---

#### 3.4.4 `category.delete`
- **Command / Method**: `category.delete` (Internal Channel / IPC)
- **Description**: Deletes a category. Foreign keys on `downloads.category_id` are set to `NULL` via `ON DELETE SET NULL`, preserving download task records and completed files on disk (FR-031). The default fallback category cannot be deleted.
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M525",
  "command": "category.delete",
  "payload": {
    "id": "c3f8a840-7e4b-449e-8a03-7bf1c3609803"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M525",
  "data": {
    "id": "c3f8a840-7e4b-449e-8a03-7bf1c3609803",
    "deleted": true
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: Category UUIDv4 does not exist.
  - `ERR_INVALID_ARGUMENT`: Attempted to delete the designated fallback category (`is_default = 1`).

---

### 3.5 Host Credentials & Authentication Domain (`credential.*`)

#### 3.5.1 `credential.list`
- **Command / Method**: `credential.list` (Internal Channel / IPC)
- **Description**: Lists saved per-host credentials. Plaintext passwords are never returned; `password_encrypted` values remain masked for UI display (FR-014, FR-050).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M526",
  "command": "credential.list",
  "payload": {}
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M526",
  "data": {
    "items": [
      {
        "id": "cr1a2b3c-4d5e-6f7a-8b9c-0d1e2f3a4b5c",
        "host": "private-vault.internal.test",
        "username": "storage_admin",
        "port": 443,
        "protocol": "https",
        "created_at": "2026-03-30T14:10:00.000Z",
        "updated_at": "2026-03-30T14:10:00.000Z"
      }
    ]
  }
}
```
- **Error Codes**: None.

---

#### 3.5.2 `credential.create`
- **Command / Method**: `credential.create` (Internal Channel / IPC)
- **Description**: Stores reusable credentials for a specific host and protocol. Plaintext password is encrypted at rest using Windows DPAPI (`CryptProtectData`) before insertion into the `credentials` table (DEC-005, FR-014, FR-058).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M527",
  "command": "credential.create",
  "payload": {
    "host": "ftp.firmware.test",
    "username": "developer",
    "password_plaintext": "Secret_P@ssw0rd!2026",
    "port": 21,
    "protocol": "ftp"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M527",
  "data": {
    "id": "cr2a2b3c-4d5e-6f7a-8b9c-0d1e2f3a4b5d",
    "host": "ftp.firmware.test",
    "username": "developer",
    "port": 21,
    "protocol": "ftp",
    "created_at": "2026-03-30T14:39:00.000Z",
    "updated_at": "2026-03-30T14:39:00.000Z"
  }
}
```
- **Error Codes**:
  - `ERR_ALREADY_EXISTS`: An identity with the same host, port, username, and protocol exists.
  - `ERR_INVALID_ARGUMENT`: Unsupported protocol or missing required host/username.
  - `ERR_INTERNAL`: Win32 DPAPI `CryptProtectData` failure.

---

#### 3.5.3 `credential.delete`
- **Command / Method**: `credential.delete` (Internal Channel / IPC)
- **Description**: Deletes stored host credentials from SQLite (FR-014).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M528",
  "command": "credential.delete",
  "payload": {
    "id": "cr2a2b3c-4d5e-6f7a-8b9c-0d1e2f3a4b5d"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M528",
  "data": {
    "id": "cr2a2b3c-4d5e-6f7a-8b9c-0d1e2f3a4b5d",
    "deleted": true
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: Credential UUIDv4 does not exist.

---

### 3.6 Browser Interception & Native Messaging Domain (`browser.*`)

#### 3.6.1 `browser.ping`
- **Command / Method**: `browser.ping` (Native Messaging Stdin/Stdout)
- **Description**: Handshake ping emitted by the Manifest V3 extension background worker upon connecting to `unduhkilat-host.exe`. Verifies bidirectional communication and returns host binary version and engine operational state (FR-033, FR-034, USER_FLOW Flow 1).
- **Auth and Permission**: Browser Extension (Allowed origins in Windows Registry).
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M529",
  "command": "browser.ping",
  "payload": {
    "browser_type": "chrome",
    "extension_version": "1.0.0"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M529",
  "data": {
    "app_name": "UnduhKilat",
    "version": "1.0.0",
    "protocol_version": 1,
    "connected": true,
    "app_running": true
  }
}
```
- **Error Codes**:
  - `ERR_UNAUTHORIZED`: Calling extension origin is not registered in the native messaging manifest.
  - `ERR_NATIVE_HOST_TIMEOUT`: Host binary failed to process the request within `[placeholder]` ms.

---

#### 3.6.2 `browser.intercept`
- **Command / Method**: `browser.intercept` (Native Messaging Stdin/Stdout)
- **Description**: Intercepts downloadable link navigations and downloads initiated from Chrome, Edge, or Brave. Transfers the full URL, cookies, `Referer`, and `User-Agent` context to the desktop core, opening the egui confirmation dialog (DEC-006, FR-017, FR-036, FR-037, FR-039, USER_FLOW Flow 2 Path A).
- **Auth and Permission**: Browser Extension (Origin verified).
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M530",
  "command": "browser.intercept",
  "payload": {
    "url": "https://software-dist.example.test/builds/v2.5/installer.exe",
    "suggested_filename": "installer.exe",
    "total_bytes": 157286400,
    "mime_type": "application/x-msdownload",
    "referer": "https://software-dist.example.test/downloads.html",
    "user_agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36",
    "cookies": "sess_auth=a98f12c3; pref_lang=en-US",
    "tab_id": 14205
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M530",
  "data": {
    "intercepted": true,
    "dialog_presented": true,
    "resolved_category": "Programs",
    "resolved_target_directory": "C:\\Downloads\\Programs"
  }
}
```
- **Error Codes**:
  - `ERR_INVALID_URL_SCHEME`: URI scheme is not HTTP, HTTPS, or FTP.
  - `ERR_PAYLOAD_CORRUPT`: JSON frame is missing mandatory navigation attributes.
  - `ERR_UNAUTHORIZED`: Native messaging request origin validation failed.

---

#### 3.6.3 `browser.query_status`
- **Command / Method**: `browser.query_status` (Native Messaging Stdin/Stdout)
- **Description**: Allows the Chromium extension to query the state of a download initiated from that browser instance (FR-038).
- **Auth and Permission**: Browser Extension.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M531",
  "command": "browser.query_status",
  "payload": {
    "download_id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M531",
  "data": {
    "id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
    "file_name": "ubuntu-24.04.iso",
    "state": "running",
    "total_bytes": 6048576000,
    "downloaded_bytes": 2048576000,
    "progress_percent": 33.86,
    "current_speed_bps": 12582912,
    "eta_seconds": 317
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: Download UUIDv4 does not exist in `downloads` table.
  - `ERR_UNAUTHORIZED`: Origin extension not permitted to inspect this task.

---

#### 3.6.4 `browser.push_status` (Host $\rightarrow$ Extension Event)
- **Command / Method**: `browser.push_status` (Native Messaging Push Event)
- **Description**: Real-time push notification sent from `unduhkilat-host.exe` to the Chromium extension whenever a browser-initiated download transitions state (`queued`, `running`, `paused`, `completed`, `failed`, `cancelled`), updating the extension popup badge and task list (FR-038, DATABASE.md).
- **Auth and Permission**: Desktop Engine Host (Emitted to connected extension stdin).
- **Event Example**:
```json
{
  "type": "event",
  "event": "browser.push_status",
  "payload": {
    "download_id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
    "state": "completed",
    "file_name": "ubuntu-24.04.iso",
    "total_bytes": 6048576000,
    "downloaded_bytes": 6048576000,
    "completed_at": "2026-03-30T14:45:10.500Z",
    "target_path": "C:\\Downloads\\Images\\ubuntu-24.04.iso"
  }
}
```
- **Error Codes**: None (Unsolicited push event).

### 3.7 Hybrid Media Sniffer Domain (`media.*`)

#### 3.7.1 `media.candidate_detected`
- **Command / Method**: `media.candidate_detected` (Native Messaging Stdin/Stdout)
- **Description**: Emitted by the Manifest V3 extension whenever media streams are discovered on the active web page via hybrid detection: network request sniffing (`webRequest` / `declarativeNetRequest`) or DOM hook signals (`<video>`, `<audio>`, embedded players) (DEC-003, FR-041, USER_FLOW Flow 2 Path B). Desktop engine deduplicates incoming candidates by resolved canonical URL and merges them into the candidate cache (FR-042).
- **Auth and Permission**: Browser Extension.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M532",
  "command": "media.candidate_detected",
  "payload": {
    "tab_id": 14205,
    "page_url": "https://video-platform.example.test/watch?v=demo123",
    "page_title": "Product Demo High Quality",
    "media_url": "https://cdn.example.test/streams/demo123-1080p.mp4",
    "mime_type": "video/mp4",
    "detection_source": "hybrid_network_sniff",
    "discovered_bytes": 104857600,
    "stream_type": "progressive",
    "headers": {
      "referer": "https://video-platform.example.test/",
      "user_agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/122.0.0.0 Safari/537.36",
      "cookie": "session_id=xyz789"
    }
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M532",
  "data": {
    "candidate_id": "med_01HP8XA7B1K9L0M4N2P3Q5R6S7",
    "deduplicated": true,
    "is_new": true,
    "active_candidates_for_tab": 3
  }
}
```
- **Error Codes**:
  - `ERR_PAYLOAD_CORRUPT`: Missing `media_url` or `tab_id`.
  - `ERR_INVALID_URL_SCHEME`: Scheme is not HTTP, HTTPS, or FTP.
  - `ERR_MEDIA_PARSE_FAILED`: Stream manifest cannot be parsed (FR-044).

---

#### 3.7.2 `media.list_candidates`
- **Command / Method**: `media.list_candidates` (Native Messaging Stdin/Stdout / Internal Channel)
- **Description**: Returns all deduplicated media candidates discovered for a specific browser tab or session, rendered inside the browser extension's floating panel overlay (DEC-003, FR-042, FR-043). Manifest-based streams (DASH/HLS) are listed with their discovered attributes and marked as experimental/unsupported handling `TBD` (FR-044).
- **Auth and Permission**: Browser Extension, End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M533",
  "command": "media.list_candidates",
  "payload": {
    "tab_id": 14205
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M533",
  "data": {
    "tab_id": 14205,
    "candidates": [
      {
        "candidate_id": "med_01HP8XA7B1K9L0M4N2P3Q5R6S7",
        "media_url": "https://cdn.example.test/streams/demo123-1080p.mp4",
        "mime_type": "video/mp4",
        "discovered_bytes": 104857600,
        "detection_source": "hybrid_network_sniff",
        "stream_type": "progressive",
        "quality_label": "1080p"
      },
      {
        "candidate_id": "med_01HP8XA7B1K9L0M4N2P3Q5R6S8",
        "media_url": "https://cdn.example.test/streams/demo123-master.m3u8",
        "mime_type": "application/x-mpegURL",
        "discovered_bytes": -1,
        "detection_source": "extension_dom_hook",
        "stream_type": "hls_manifest",
        "quality_label": "Adaptive (HLS)"
      }
    ]
  }
}
```
- **Error Codes**:
  - `ERR_INVALID_ARGUMENT`: `tab_id` is invalid.

---

#### 3.7.3 `media.send_to_app`
- **Command / Method**: `media.send_to_app` (Native Messaging Stdin/Stdout)
- **Description**: Triggered when the End User clicks "Download" on an entry in the floating video/audio sniffer panel over the web page (FR-043). Forwards the candidate to the desktop engine, opening the download confirmation dialog pre-populated with discovered metadata and headers (FR-039).
- **Auth and Permission**: Browser Extension.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M534",
  "command": "media.send_to_app",
  "payload": {
    "candidate_id": "med_01HP8XA7B1K9L0M4N2P3Q5R6S7"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M534",
  "data": {
    "confirmation_dialog_opened": true,
    "suggested_file_name": "Product_Demo_High_Quality_1080p.mp4",
    "resolved_category_id": "8a3d5e21-3c4b-4f8a-9e10-1a2b3c4d5e6f"
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: Candidate ID has expired or does not exist in memory cache.
  - `ERR_MEDIA_PARSE_FAILED`: Candidate represents an unhandled stream manifest (FR-044).

---

### 3.8 Site Grabber & Batch Domain (`grabber.*`)

#### 3.8.1 `grabber.crawl`
- **Command / Method**: `grabber.crawl` (Internal Channel / IPC)
- **Description**: Initiates a depth-limited website crawl or page link walk from a starting URL. Evaluates user-defined inclusion/exclusion wildcard patterns and extracts downloadable hyperlinks (FR-045, FR-046, USER_FLOW Flow 5 Path C). Maximum crawl depth defaults to `3` via the `site_grabber_max_depth` setting and is bounded only by user configuration.
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M535",
  "command": "grabber.crawl",
  "payload": {
    "start_url": "https://archive.example.test/documents/2026/",
    "max_depth": 2,
    "include_patterns": ["*.pdf", "*.epub"],
    "exclude_patterns": ["*/drafts/*", "*temp*"],
    "credentials_id": "cr1a2b3c-4d5e-6f7a-8b9c-0d1e2f3a4b5c"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M535",
  "data": {
    "crawl_id": "crw_01HP8XB1N4M7V2P8Q0R3S5T6U7",
    "status": "in_progress",
    "pages_visited": 1,
    "links_discovered": 14
  }
}
```
- **Error Codes**:
  - `ERR_INVALID_URL_SCHEME`: `start_url` protocol is unsupported.
  - `ERR_INVALID_ARGUMENT`: `max_depth` < 0 or patterns contain invalid regex/wildcard syntax.
  - `ERR_CRAWL_FAILED`: Host connection refused or DNS failure during crawl initialization.

---

#### 3.8.2 `grabber.preview`
- **Command / Method**: `grabber.preview` (Internal Channel / IPC)
- **Description**: Returns discovered links from an active or finished site crawl for user inspection prior to queueing (FR-045, FR-046). Never starts downloads or modifies `downloads` table without confirmation (FR-039).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M536",
  "command": "grabber.preview",
  "payload": {
    "crawl_id": "crw_01HP8XB1N4M7V2P8Q0R3S5T6U7",
    "limit": 50,
    "cursor": null
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M536",
  "data": {
    "crawl_id": "crw_01HP8XB1N4M7V2P8Q0R3S5T6U7",
    "status": "completed",
    "items": [
      {
        "url": "https://archive.example.test/documents/2026/whitepaper.pdf",
        "file_name": "whitepaper.pdf",
        "depth": 1,
        "estimated_bytes": 5242880,
        "selected": true
      },
      {
        "url": "https://archive.example.test/documents/2026/manual.epub",
        "file_name": "manual.epub",
        "depth": 2,
        "estimated_bytes": 12582912,
        "selected": true
      }
    ],
    "pagination": {
      "limit": 50,
      "total_count": 2,
      "has_more": false,
      "next_cursor": null
    }
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: `crawl_id` does not exist or was discarded.

---

#### 3.8.3 `grabber.enqueue`
- **Command / Method**: `grabber.enqueue` (Internal Channel / IPC)
- **Description**: Takes confirmed URLs from the Site Grabber preview and inserts them as standard queue items in `downloads` and `queue_items` (FR-047). Batch items strictly obey the configured concurrency limit, manual priority, category routing, and bandwidth limits (FR-018, FR-020, FR-025, FR-029).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M537",
  "command": "grabber.enqueue",
  "payload": {
    "crawl_id": "crw_01HP8XB1N4M7V2P8Q0R3S5T6U7",
    "selected_urls": [
      "https://archive.example.test/documents/2026/whitepaper.pdf",
      "https://archive.example.test/documents/2026/manual.epub"
    ],
    "category_id": "c4f8a840-7e4b-449e-8a03-7bf1c3609804",
    "target_directory": null,
    "connection_count": 8,
    "priority": 0
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M537",
  "data": {
    "enqueued_count": 2,
    "created_download_ids": [
      "9a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d",
      "9a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5e"
    ]
  }
}
```
- **Error Codes**:
  - `ERR_NOT_FOUND`: `crawl_id` does not exist.
  - `ERR_INVALID_ARGUMENT`: `selected_urls` array is empty.

---

### 3.9 UI Shell, Tray & Notification Domain (`ui.*`)

#### 3.9.1 `ui.clipboard_notify`
- **Command / Method**: `ui.clipboard_notify` (Internal Event / IPC)
- **Description**: Triggered by the desktop clipboard monitoring loop when a valid downloadable URL (`http`, `https`, `ftp`) is detected on the Windows clipboard (FR-040, USER_FLOW Flow 2 Path C). Opens the Add URL confirmation dialog prefilled with URL and inferred filename.
- **Auth and Permission**: System Internal / End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M538",
  "command": "ui.clipboard_notify",
  "payload": {
    "clipboard_url": "https://software.example.test/downloads/tool.zip"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M538",
  "data": {
    "dialog_presented": true,
    "clipboard_url": "https://software.example.test/downloads/tool.zip"
  }
}
```
- **Error Codes**:
  - `ERR_INVALID_URL_SCHEME`: Clipboard content is not an allowed scheme.

---

#### 3.9.2 `ui.dialog_confirm`
- **Command / Method**: `ui.dialog_confirm` (Internal Channel / IPC)
- **Description**: Submits the user's explicit decision from the Add URL or Interception confirmation modal (FR-039, FR-040, FR-049, USER_FLOW Flow 2 Step 3). If `confirmed = true`, writes records to SQLite and transitions state to `queued` (or `paused` if "Download Later" was chosen). If `confirmed = false`, closes dialog without sending requests or writing state.
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M539",
  "command": "ui.dialog_confirm",
  "payload": {
    "confirmed": true,
    "action": "download_now",
    "original_url": "https://software.example.test/downloads/tool.zip",
    "file_name": "tool-v1.0.zip",
    "target_path": "C:\\Downloads\\Compressed\\tool-v1.0.zip",
    "category_id": "c1f8a840-7e4b-449e-8a03-7bf1c3609801",
    "connection_count": 8,
    "priority": 1,
    "referer": "https://software.example.test/",
    "cookies": null
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M539",
  "data": {
    "download_id": "1b2c3d4e-5f6a-7b8c-9d0e-1f2a3b4c5d6e",
    "state": "queued"
  }
}
```
- **Error Codes**:
  - `ERR_INVALID_PATH`: Provided target path is malformed.
  - `ERR_PATH_UNWRITABLE`: Destination folder cannot be written.

---

#### 3.9.3 `ui.tray_action`
- **Command / Method**: `ui.tray_action` (Internal Channel / IPC)
- **Description**: Handles system tray menu interactions: toggling window visibility, queue execution, or explicit application termination (DEC-004, FR-051, FR-052, FR-053). Closing the main window invokes `action = "hide_to_tray"` leaving Tokio transfers active. Invoking `action = "exit"` raises a warning, checkpoints in-progress downloads to `paused`, and terminates cleanly (FR-053).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M540",
  "command": "ui.tray_action",
  "payload": {
    "action": "hide_to_tray"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M540",
  "data": {
    "window_visible": false,
    "transfers_active": true,
    "tray_icon_active": true
  }
}
```
- **Error Codes**:
  - `ERR_INVALID_ARGUMENT`: Unrecognized action (must be `show`, `hide_to_tray`, `start_queue`, `stop_queue`, `exit`).

---

#### 3.9.4 `ui.toast_notify`
- **Command / Method**: `ui.toast_notify` (Internal Event / Win32 Toast)
- **Description**: Dispatches a native Windows toast notification via WinRT / Win32 notification facility when a download completes, firing while the application window is hidden in the system tray (DEC-004, FR-054). Suppressed if disabled in user settings or if download failed due to size mismatch (FR-011, FR-054).
- **Auth and Permission**: System Internal.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M541",
  "command": "ui.toast_notify",
  "payload": {
    "download_id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
    "title": "UnduhKilat - Download Complete",
    "message": "ubuntu-24.04.iso has finished downloading.",
    "target_path": "C:\\Downloads\\Images\\ubuntu-24.04.iso"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M541",
  "data": {
    "toast_dispatched": true
  }
}
```
- **Error Codes**: None (Failures logged silently to disk log without crashing engine).

---

### 3.10 Configuration & Settings Domain (`settings.*`)

#### 3.10.1 `settings.get_all`
- **Command / Method**: `settings.get_all` (Internal Channel / IPC)
- **Description**: Retrieves all application-wide configuration entries stored in the `settings` table, including concurrency limits, proxy settings, speed caps, and clipboard toggles (FR-001, FR-002, FR-015, FR-050).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M542",
  "command": "settings.get_all",
  "payload": {}
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M542",
  "data": {
    "concurrency_limit": 5,
    "default_connections_per_download": 8,
    "global_speed_limit_kbps": 0,
    "clipboard_monitoring_enabled": 1,
    "close_to_tray_enabled": 1,
    "completion_toast_enabled": 1,
    "proxy_mode": "system",
    "proxy_host": null,
    "proxy_port": null,
    "proxy_username": null,
    "log_level": "info",
    "dynamic_split_min_segment_bytes": 1048576,
    "checkpoint_interval_ms": 1000,
    "retry_count_limit": 5,
    "retry_backoff_ms": 2000,
    "site_grabber_max_depth": 3,
    "browser_integration_registered": true
  }
}
```
- **Error Codes**: None.

---

#### 3.10.2 `settings.update`
- **Command / Method**: `settings.update` (Internal Channel / IPC)
- **Description**: Updates application-wide configuration keys in the `settings` table. Live settings (speed limits, concurrency limits) apply immediately across active Tokio connection pools (DEC-001, FR-001, FR-028, FR-050). In accordance with DEC-001, `concurrency_limit` and `default_connections_per_download` permit any positive integer with no artificial upper ceiling. Sensitive fields (e.g. `proxy_password`) are DPAPI-encrypted before persistence (DEC-005).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M543",
  "command": "settings.update",
  "payload": {
    "settings": {
      "concurrency_limit": "12",
      "default_connections_per_download": "32",
      "global_speed_limit_kbps": "4096",
      "clipboard_monitoring_enabled": "1",
      "completion_toast_enabled": "1",
      "close_to_tray_enabled": "1",
      "proxy_mode": "manual",
      "proxy_host": "proxy.corp.test",
      "proxy_port": "8080",
      "proxy_username": "corporate_user",
      "proxy_password_plaintext": "CorpSecret!2026",
      "log_level": "debug"
    }
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M543",
  "data": {
    "updated_keys": [
      "concurrency_limit",
      "default_connections_per_download",
      "global_speed_limit_kbps",
      "clipboard_monitoring_enabled",
      "completion_toast_enabled",
      "close_to_tray_enabled",
      "proxy_mode",
      "proxy_host",
      "proxy_port",
      "proxy_username",
      "proxy_password_encrypted",
      "log_level"
    ],
    "updated_at": "2026-03-30T14:40:00.000Z"
  }
}
```
- **Error Codes**:
  - `ERR_INVALID_ARGUMENT`: Negative or zero integer provided for concurrency/connections, or invalid proxy mode string.

---

### 3.11 Diagnostics & Logs Domain (`diagnostics.*`)

#### 3.11.1 `diagnostics.log_tail`
- **Command / Method**: `diagnostics.log_tail` (Internal Channel / IPC)
- **Description**: Reads the most recent log entries from the local rotating disk log files for display inside the in-application diagnostics panel (FR-056, FR-057).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M544",
  "command": "diagnostics.log_tail",
  "payload": {
    "lines_count": 100,
    "min_level": "info"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M544",
  "data": {
    "lines": [
      "2026-03-30T14:31:10.123Z [WARN] [unduhkilat_core::network] Connection 3 dropped on task 7b8e1f2a; retrying from offset 2097152000 (attempt 1/5)",
      "2026-03-30T14:31:12.456Z [INFO] [unduhkilat_core::network] Connection 3 re-established on task 7b8e1f2a"
    ],
    "log_file_path": "C:\\Tools\\UnduhKilat\\data\\logs\\unduhkilat.log",
    "total_lines_available": 1420
  }
}
```
- **Error Codes**:
  - `ERR_FILE_NOT_FOUND`: Log file does not exist on disk.

---

#### 3.11.2 `diagnostics.get_system_info`
- **Command / Method**: `diagnostics.get_system_info` (Internal Channel / IPC)
- **Description**: Returns live runtime metrics: active Tokio worker counts, live connection count per running download, aggregate socket count, and last failure code per failed task (FR-055, FR-056).
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M545",
  "command": "diagnostics.get_system_info",
  "payload": {}
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M545",
  "data": {
    "runtime": {
      "tokio_workers_active": 16,
      "total_open_sockets": 24,
      "memory_working_set_bytes": 48234496
    },
    "active_downloads": [
      {
        "download_id": "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b",
        "file_name": "ubuntu-24.04.iso",
        "live_connections": 16,
        "segment_count": 16,
        "current_speed_bps": 12582912
      },
      {
        "download_id": "8c9e2f3a-6d4e-5f0a-9b2c-3d4e5f6a7b8c",
        "file_name": "compiler-msvc.zip",
        "live_connections": 8,
        "segment_count": 8,
        "current_speed_bps": 4194304
      }
    ],
    "failed_downloads_summary": [
      {
        "download_id": "3d4e5f6a-7b8c-9d0e-1f2a-3b4c5d6e7f8a",
        "file_name": "dataset-corrupt.tar",
        "last_error_code": "ERR_CHECKSUM_SIZE_MISMATCH",
        "last_error_message": "Verified size 5242879 does not match expected Content-Length 5242880",
        "updated_at": "2026-03-30T14:15:22.000Z"
      }
    ]
  }
}
```
- **Error Codes**: None.

---

#### 3.11.3 `diagnostics.export_logs`
- **Command / Method**: `diagnostics.export_logs` (Internal Channel / IPC)
- **Description**: Copies rotating local log files into a user-specified destination folder without transmitting any telemetry or log content off the local machine (FR-056, FR-057, NFR-011, NFR-016, USER_FLOW Flow 5 Path D). Destination must be a valid, writable Windows filesystem directory.
- **Auth and Permission**: End User.
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M546",
  "command": "diagnostics.export_logs",
  "payload": {
    "target_directory": "D:\\Diagnostics\\UnduhKilat-Logs-2026-03-30"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M546",
  "data": {
    "exported_files_count": 3,
    "target_directory": "D:\\Diagnostics\\UnduhKilat-Logs-2026-03-30",
    "exported_files": [
      "unduhkilat.log",
      "unduhkilat.log.2026-03-29",
      "unduhkilat.log.2026-03-28"
    ],
    "total_bytes_written": 14680064
  }
}
```
- **Error Codes**:
  - `ERR_INVALID_PATH`: Provided target directory path is invalid under Windows path rules.
  - `ERR_EXPORT_FAILED`: Target folder is read-only, access is denied, or volume has insufficient space.

---

### 3.12 Elevated Administration Domain (`admin.*`)

#### 3.12.1 `admin.registry.register_machine_wide`
- **Command / Method**: `admin.registry.register_machine_wide` (Elevated CLI: `[placeholder]`)
- **Description**: Registers the Native Messaging Host machine-wide under `HKLM\Software\Google\Chrome\NativeMessagingHosts\` and matching Edge/Brave keys so every Windows user account on the machine can utilize browser capture without running the app first (DEC-006, FR-035). Requires Windows administrative elevation (High Mandatory Integrity Level).
- **Auth and Permission**: Administrator (Elevated token required).
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M547",
  "command": "admin.registry.register_machine_wide",
  "payload": {
    "bundle_root_directory": "C:\\Program Files\\UnduhKilat"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M547",
  "data": {
    "hklm_keys_created": [
      "HKLM\\Software\\Google\\Chrome\\NativeMessagingHosts\\com.unduhkilat.host",
      "HKLM\\Software\\Microsoft\\Edge\\NativeMessagingHosts\\com.unduhkilat.host",
      "HKLM\\Software\\BraveSoftware\\Brave-Browser\\NativeMessagingHosts\\com.unduhkilat.host"
    ],
    "manifest_path": "C:\\Program Files\\UnduhKilat\\native-host\\com.unduhkilat.host.json",
    "host_binary_target": "C:\\Program Files\\UnduhKilat\\unduhkilat-host.exe"
  }
}
```
- **Error Codes**:
  - `ERR_PERMISSION_DENIED`: Caller does not possess Windows administrative elevation.
  - `ERR_REGISTRY_WRITE_FAILED`: Windows Registry access denied or key locked by system security policy.
  - `ERR_FILE_NOT_FOUND`: Specified host binary does not exist at target path.

---

#### 3.12.2 `admin.bundle.verify_and_update`
- **Command / Method**: `admin.bundle.verify_and_update` (Internal Channel / Elevated CLI)
- **Description**: Verifies cryptographic digital signature of an update ZIP archive against embedded public keys before replacing portable bundle binaries in place (DEC-007, FR-060, NFR-013, USER_FLOW Flow 5 Path E). If the signature is invalid or unsigned, update is aborted immediately and bundle contents remain unmodified.
- **Auth and Permission**: `[placeholder]` (whether End User may execute for user-writable bundles or Administrator-only is undecided; FR-061).
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M548",
  "command": "admin.bundle.verify_and_update",
  "payload": {
    "package_archive_path": "C:\\Users\\User\\Downloads\\UnduhKilat-v1.1.0-win64.zip",
    "signature_file_path": "C:\\Users\\User\\Downloads\\UnduhKilat-v1.1.0-win64.zip.sig"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M548",
  "data": {
    "signature_verified": true,
    "package_version": "1.1.0",
    "files_replaced_count": 12,
    "restart_required": true,
    "backup_path": "C:\\Tools\\UnduhKilat\\.backup_1.0.0"
  }
}
```
- **Error Codes**:
  - `ERR_FILE_NOT_FOUND`: Package archive or signature file missing from filesystem.
  - `ERR_PACKAGE_SIGNATURE_INVALID`: Signature verification failed or package archive hash mismatch.
  - `ERR_PERMISSION_DENIED`: Target bundle directory is write-protected and caller lacks elevation.

---

#### 3.12.3 `admin.bundle.replace_machine_wide`
- **Command / Method**: `admin.bundle.replace_machine_wide` (Elevated CLI: `UnduhKilat.exe --admin-upgrade-bundle`)
- **Description**: Replaces the complete portable bundle in a shared or system-protected folder with a verified newer bundle, ensuring machine-wide `HKLM` native host registrations resolve to the new binary path (DEC-006, FR-061).
- **Auth and Permission**: Administrator (Elevated token required).
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M549",
  "command": "admin.bundle.replace_machine_wide",
  "payload": {
    "new_bundle_archive_path": "D:\\Shared\\Installers\\UnduhKilat-v1.2.0-win64.zip",
    "shared_install_path": "C:\\Program Files\\UnduhKilat"
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M549",
  "data": {
    "replaced": true,
    "installed_version": "1.2.0",
    "shared_path": "C:\\Program Files\\UnduhKilat",
    "hklm_registration_verified": true
  }
}
```
- **Error Codes**:
  - `ERR_PERMISSION_DENIED`: Operation executed without Windows UAC elevation.
  - `ERR_PACKAGE_SIGNATURE_INVALID`: Replacement package failed cryptographic signature validation.
  - `ERR_PATH_UNWRITABLE`: Shared directory locked by active processes.

---

#### 3.12.4 `admin.bundle.reset_data`
- **Command / Method**: `admin.bundle.reset_data` (Elevated CLI: `UnduhKilat.exe --admin-reset-data`)
- **Description**: Clears application state directory, purging SQLite database (`unduhkilat.db`, `wal`, `shm`) and rotating log files after requiring explicit confirmation parameter `confirm_reset = true` (FR-062, USER_FLOW Flow 5 Path F). Already completed files on disk remain strictly untouched.
- **Auth and Permission**: Administrator (Elevated token required).
- **Request Example**:
```json
{
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M550",
  "command": "admin.bundle.reset_data",
  "payload": {
    "confirm_reset": true,
    "clear_database": true,
    "clear_logs": true
  }
}
```
- **Response Example**:
```json
{
  "status": "ok",
  "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M550",
  "data": {
    "database_purged": true,
    "logs_purged": true,
    "downloaded_files_touched": 0,
    "reset_completed_at": "2026-03-30T14:48:00.000Z"
  }
}
```
- **Error Codes**:
  - `ERR_PERMISSION_DENIED`: Executed without administrator elevation.
  - `ERR_INVALID_ARGUMENT`: `confirm_reset` parameter is false or omitted.
  - `ERR_STORAGE_FULL`: Underlying filesystem error preventing directory recreation.

---

## 4. Contract Consistency Notes / Catatan Konsistensi Kontrak

### 4.1 Identifier Prefixes & Encodings
All entity identifiers, request correlation IDs, and transient candidate tokens adhere to strict prefixing conventions:

| Entity / Object | Format & Representation | Example | Cross-Reference |
|---|---|---|---|
| **Request ID** | `req_` + 26-character Crockford Base32 ULID | `req_01HP8X9Q2KVM7D6N1E9BZ4M501` | API.md Section 2.2 |
| **Download ID** | Canonical UUIDv4 string (36 chars, lowercase, hyphens) | `7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b` | DATABASE.md `downloads.id` |
| **Category ID** | Canonical UUIDv4 string (36 chars, lowercase, hyphens) | `c1f8a840-7e4b-449e-8a03-7bf1c3609801` | DATABASE.md `categories.id` |
| **Credential ID**| `[placeholder]` | `[placeholder]` | DATABASE.md `credentials.id` |
| **Schedule ID**  | `[placeholder]` | `[placeholder]` | DATABASE.md `schedules.id` |
| **Queue Item ID**| `[placeholder]` | `[placeholder]` | DATABASE.md `queue_items.id` |
| **Media Candidate**| `med_` + 26-character Crockford Base32 ULID | `med_01HP8XA7B1K9L0M4N2P3Q5R6S7` | API.md Section 3.7.1 |
| **Crawl Job ID** | `crw_` + 26-character Crockford Base32 ULID | `crw_01HP8XB1N4M7V2P8Q0R3S5T6U7` | API.md Section 3.8.1 |
| **Segment ID**   | Signed 64-bit auto-incrementing integer | `101` | DATABASE.md `segments.id` |

### 4.2 Timestamps & Time Representation
1. **System Timestamps**: All persisted and wire timestamps (`created_at`, `updated_at`, `completed_at`, `reset_completed_at`) are serialized strictly as UTC ISO-8601 strings with millisecond precision and a trailing `Z` designator (`YYYY-MM-DDTHH:MM:SS.SSSZ`, e.g., `2026-03-30T14:30:00.000Z`).
2. **Scheduler Clock Times**: Wall-clock trigger times in `schedules` (`start_time`, `stop_time`) are formatted as 24-hour local time strings `HH:MM:SS` (e.g., `02:00:00`, `23:30:00`).

### 4.3 Unit Suffix Conventions
All quantitative schema fields and command payload attributes carry explicit unit suffixes to eliminate dimension ambiguity across Rust and JavaScript/WebExtension boundaries:

- `_bytes`: Uncompressed byte quantities (signed 64-bit integer, e.g., `total_bytes`, `downloaded_bytes`, `discovered_bytes`). `-1` indicates unknown or unqueried length.
- `_offset`: Contiguous file byte positions (`highest_contiguous_offset`, `range_start`, `range_end`).
- `_bps`: Real-time transfer throughput measured in bytes per second (`current_speed_bps`).
- `_kbps`: Bandwidth rate limits configured in kilobytes per second (`global_speed_limit_kbps`, `speed_limit_kbps`).
- `_ms`: Time spans and network timeouts in milliseconds (`checkpoint_interval_ms`, `retry_backoff_ms`).
- `_seconds`: Time intervals and durations in seconds (`eta_seconds`).

### 4.4 Idempotency Guarantees
- **State Transitions**: `download.pause`, `download.resume`, `download.restart`, `download.cancel`, `queue.start_all`, and `queue.stop_all` are deterministic and idempotent. Invoking an operation on an item already in the target state returns `status: "ok"` without repeating side effects or generating duplicate transactional checkpoints (FR-006, FR-007, FR-058).
- **Transactional Atomic Dispatch**: Batch operations such as `queue.reorder` and `download.create` execute inside explicit SQLite transactions (`BEGIN IMMEDIATE ... COMMIT`). If any constraint check fails (e.g., duplicate category name, foreign key mismatch), the transaction rolls back completely, ensuring zero half-updated state records.

### 4.5 Standard Event Names
Asynchronous push notifications emitted by the Tokio engine core follow a two-part dot-notated naming convention:

| Event Name | Emitter Source | Target Audience | Payload Summary |
|---|---|---|---|
| `download.progress` | Download Engine | egui Desktop UI | `id`, `downloaded_bytes`, `total_bytes`, `current_speed_bps`, `eta_seconds` |
| `download.state_change` | Download Engine | egui Desktop UI | `id`, `previous_state`, `new_state`, `error_code` |
| `browser.push_status` | Native Host | Chromium Extension | `download_id`, `state`, `file_name`, `total_bytes`, `downloaded_bytes` |
| `media.candidate_found` | Extension Hook / Sniffer | Desktop Engine / Panel | `candidate_id`, `media_url`, `mime_type`, `discovered_bytes` |
| `queue.dispatched` | Scheduler | egui Desktop UI | `download_id`, `priority`, `queue_position`, `concurrency_slot` |

### 4.6 Extension Policy for Error Codes
When new error conditions arise from future engine capabilities:
1. Error codes must be formatted in strict `UPPER_SNAKE_CASE` starting with `ERR_`.
2. The error must be added to the Canonical Error Code Catalogue in Section 2.8 of this document with its failure matrix mapping before implementation.
3. The error token must be represented as a variant of the Rust domain error enum (`src/domain/error.rs`) and registered in the TypeScript/JavaScript type definitions of the browser extension (`src/types/errors.ts`).
4. Any error surfaced to the end user must provide an actionable remedy string suitable for egui toast rendering or diagnostic panel export (FR-056).