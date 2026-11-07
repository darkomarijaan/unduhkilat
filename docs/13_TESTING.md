# TESTING.md: UnduhKilat

## 1. Testing Strategy & Scope / Strategi Pengujian & Ruang Lingkup

### 1.1 Strategy Principles & Guarantees
The test architecture for UnduhKilat enforces functional, operational, and crash durability requirements without relying on remote network infrastructure or undocumented assumptions. Testing is structured around three foundational principles:

1. **Strict Determinism**:
   - Asynchronous transfers, queue dispatching, and segment re-splitting run in controlled test harnesses using virtual timers, deterministic byte-stream mocks, and isolated in-memory or ephemeral SQLite database instances.
   - Tests assert discrete literal values: exact byte offsets, exact HTTP status codes, strict enum strings, and error tokens matching `API.md` and `DATABASE.md`.
2. **Hard Process & Boundary Isolation**:
   - Network interactions are isolated behind Rust traits (`HttpTransport`, `FileSystemDriver`, `RegistryAccess`, `NotificationDispatcher`). CI test suites run hermetically without external Internet egress.
   - External download targets and remote servers are strictly mocked via local loopback mocks (`wiremock`, `mockito`) or synthetic async stream generators.
   - Browser Native Messaging tests execute in decoupled child process pipes with strictly validated length-prefixed JSON wire envelopes.
3. **Crash Durability & Invariant Verification**:
   - All state transitions and dynamic segment partitions are verified against the single-file SQLite database schema (`unduhkilat.db`) configured in WAL mode with `PRAGMA synchronous = NORMAL;` (FR-058, `DATABASE.md`).
   - Abrupt termination tests simulate hard kills (`TerminateProcess`) mid-transfer, verifying that subsequent startup normalizes `running` downloads to `paused` without corrupting offsets or re-downloading bytes (FR-007, FR-010, KPI-005, KPI-006).

### 1.2 User Decisions Alignment Matrix
The test suite validates and adheres strictly to all user decisions:

| Decision ID | Constraint / Behavior | Test Implementation Consequence |
|---|---|---|
| **DEC-001** | Unbounded concurrency & connections per download | Test cases verify that settings and downloads accept arbitrarily large positive integers without engine clamping or database check constraint failures (FR-001, FR-002, NFR-003). |
| **DEC-002** | Manual priority first, remaining FIFO | Queue dispatch tests populate multi-item queues with varied priorities and FIFO positions, asserting deterministic order: `ORDER BY priority DESC, queue_position ASC` (FR-018, FR-021). |
| **DEC-003** | Hybrid media sniffer (sniffing + hook) | Integration tests feed candidates from synthetic network sniffers and DOM hook events, verifying URL deduplication and merged candidate listing (FR-041, FR-042). |
| **DEC-004** | Close to tray, background continuity, completion toast | Shell tests assert window close events do not abort active Tokio tasks, scheduler timers evaluate while hidden, and completion toast events fire (FR-024, FR-051, FR-052, FR-054). |
| **DEC-005** | Windows 10 & 11 x64 exclusive | Test suite and CI matrix run exclusively on `windows-latest` (x64) runners using the MSVC toolchain; non-Windows targets are excluded (NFR-001, NFR-002). |
| **DEC-006** | Chromium browsers only (Chrome, Edge, Brave) | Native messaging contract tests execute against Chromium Manifest V3 JSON schemas and standard length-prefixed I/O streams (FR-033, FR-034, FR-036). |
| **DEC-007** | Portable ZIP distribution | Portability tests verify execution from arbitrary user-writable extracted directories, reading/writing state only within portable or local data boundaries (FR-059). |

### 1.3 Traceability Matrix: Functional Requirements to Verification Strategy

| Requirement | Description | Verification Type | Primary Test Fixture / Method |
|---|---|---|---|
| **FR-001** | User-set concurrency limit | Unit & Integration | `test_concurrency_unbounded_limit()`, Tokio semaphore harness |
| **FR-002** | User-set connections per download | Unit & Integration | `test_connection_count_validation()`, segment partitioner |
| **FR-003** | Multi-connection segmented transfer | Integration | `wiremock` range-server simulating 2 parallel byte-range streams |
| **FR-004** | Dynamic segment re-splitting | Unit & Integration | Dynamic split algorithm unit tests with simulated lagging connection |
| **FR-005** | Single-connection fallback | Integration | Mock server omitting `Accept-Ranges` and rejecting HTTP 206 |
| **FR-006** | Pause transfer & flush offsets | Integration | Mid-transfer task cancellation asserting SQLite segment checkpoints |
| **FR-007** | Resume without re-downloading | Integration | Resumed download against range logger asserting 0 duplicate bytes |
| **FR-008** | Automatic retry on dropped connection | Integration | Fault-injected mock server closing TCP connection mid-stream |
| **FR-009** | Restart from zero | Integration | File truncation assertion and segment offset reset verification |
| **FR-010** | Crash recovery to resumable state | Integration / Fault Injection | SQLite state check post-`TerminateProcess` simulation |
| **FR-011** | Final file size verification | Unit & Integration | Checksum & size validation unit tests asserting `ERR_CHECKSUM_SIZE_MISMATCH` |
| **FR-012** | HTTP, HTTPS, FTP support | Integration | Synthetic HTTP, rustls TLS, and local mock FTP server (`REST` command) |
| **FR-013** | HTTP redirect & Referer preservation | Integration | 302 redirect chain assertion preserving initial `Referer` header |
| **FR-014** | Per-host & per-download credentials | Unit & Integration | DPAPI roundtrip test and 401 challenge response handler |
| **FR-015** | Proxy routing & credentials | Integration | Local HTTP proxy mock with basic authentication verification |
| **FR-016** | Target file conflict handling | Integration | Pre-existing destination file collision and rename/prompt check |
| **FR-017** | Captured browser cookies & headers | Integration | Request envelope builder asserting attached cookies and headers |
| **FR-018** | Two-level dispatch (priority then FIFO) | Unit & Integration | `test_queue_dispatch_order()`, SQLite dispatch index evaluation |
| **FR-019** | Live priority elevation | Unit & Integration | Dynamic priority mutation asserting next candidate selection |
| **FR-020** | Concurrency ceiling enforcement | Integration | 3 queued items with concurrency 2 asserting active slot count |
| **FR-021** | Manual FIFO queue reordering | Integration | Transactional re-indexing of `queue_position` in SQLite |
| **FR-022** | Start all and stop all | Integration | Bulk state transitions asserting persisted offsets and no restart from 0 |
| **FR-023** | Schedule time triggers | Unit & Integration | Mock clock scheduler firing queue start/stop actions |
| **FR-024** | Scheduler runs while in tray | Integration | Event loop assertion with GUI window detached/hidden |
| **FR-025** | Global bandwidth throttling | Integration | Token-bucket rate limiter asserting aggregate throughput in KB/s |
| **FR-026** | Per-download bandwidth throttling | Integration | Per-item rate limiter asserting isolated item cap |
| **FR-027** | Combined speed limit resolution | Unit | `test_effective_speed_limit_resolution()`, asserting 0 means unlimited and the lowest non-zero limit wins |
| **FR-028** | Live speed limit adjustment | Integration | Dynamic channel update to active token bucket without task restart |
| **FR-029** | File category extension routing | Unit & Integration | Extension parser mapping `.iso` to category destination directory |
| **FR-030** | Fallback category for unknown files | Unit & Integration | Mime/extension resolver asserting fallback category selection |
| **FR-031** | Category CRUD and path isolation | Integration | Category modification asserting existing files remain untouched |
| **FR-032** | Per-item category & path override | Integration | Add URL dialog override persisting single-item destination |
| **FR-033** | Chromium Manifest V3 extension | Contract | Manifest V3 schema validator and JSON payload conformance |
| **FR-034** | HKCU Native Host registration | Integration | Registry mock / Win32 registry test asserting `HKCU` entries |
| **FR-035 (Admin)**| HKLM Native Host registration | Integration | Elevated registry verification asserting `HKLM` keys |
| **FR-036** | Navigation interception & context | Contract & E2E | Native messaging frame parsing `url`, `Cookie`, `Referer`, `User-Agent` |
| **FR-037** | Full cookie set transfer | Contract & Integration | Multi-cookie payload parsing and injection into reqwest jar |
| **FR-038** | Status push-back to browser | Contract | `browser.push_status` event serialization and state enum matching |
| **FR-039** | Explicit confirmation dialog | E2E / egui harness | UI event harness asserting no task created before confirmation |
| **FR-040** | Clipboard URL monitoring toggle | Integration | Windows clipboard polling mock with enabled/disabled flag |
| **FR-041** | Hybrid media detection union | Integration | Feed candidate from sniff + candidate from hook, asserting union |
| **FR-042** | Deduplication by resolved URL | Unit & Integration | Candidate normalizer asserting duplicate URL suppression |
| **FR-043** | Floating panel & send-to-app | Contract & E2E | Native messaging command `media.send_to_app` triggering dialog |
| **FR-044** | Stream manifest candidate listing | Unit & Contract | DASH/HLS mime-type listing assertion without broken auto-merge |
| **FR-045** | Batch wildcard link grabber | Unit & Integration | Wildcard regex compiler filtering fixture link list |
| **FR-046** | Depth-limited site crawler | Integration | Mock HTTP link crawler bounded by `max_depth` and patterns |
| **FR-047** | Batch links as standard queue items | Integration | Crawl enqueue asserting individual items follow queue rules |
| **FR-048** | Download list live metrics & sorting | UI / Harness | egui frame render harness checking progress, speed, ETA |
| **FR-049** | Add URL multi-item batch dialog | Integration | Add URL parser handling multi-line input and batch properties |
| **FR-050** | Settings coverage & persistence | Integration | Settings repository roundtrip for all keys in `DATABASE.md` |
| **FR-051** | System tray icon and actions | E2E / Windows Shell | Tray menu command dispatcher (`show`, `hide`, `start`, `stop`, `exit`) |
| **FR-052** | Window close hides to tray | Integration | Close event interception verifying active downloads remain `running` |
| **FR-053** | Explicit exit warning & checkpoint | Integration | Clean termination routine asserting checkpoint write before shutdown |
| **FR-054** | Completion toast notification | Integration | WinRT toast dispatcher mock asserting notification firing |
| **FR-055** | Segment details inspector view | UI / Harness | Details view inspection matching SQLite `segments` table |
| **FR-056** | In-app diagnostics & log export | Integration | Tail log buffer reader and export file copy verification |
| **FR-057** | Rotating local logs, no egress | Integration & Security | Network packet capture asserting 0 external telemetry egress |
| **FR-058** | SQLite transactional checkpoints | Integration / Fault Injection | Concurrent checkpointing under WAL mode during active I/O |
| **FR-059** | Portable ZIP execution & isolation | E2E | Launch from isolated folder asserting no outside writes |
| **FR-060** | Signed in-place package update | Unit & Integration | Ed25519 signature validator rejecting corrupt/unsigned ZIP |
| **FR-061 (Admin)**| Machine-wide bundle replacement | Integration | Admin replacement asserting `HKLM` path pointer consistency |
| **FR-062 (Admin)**| State directory data reset | Integration | Reset command asserting DB/logs deleted and downloads untouched |

---

## 2. Tooling & Configuration / Tooling & Konfigurasi

### 2.1 Test Frameworks & Cargo Dependencies
The test stack leverages native Rust crates optimized for Tokio async execution, Windows Win32 API mocking, and HTTP protocol virtualization.

```toml
# Cargo.toml [dev-dependencies] snippet
[dev-dependencies]
tokio = { version = "1.36", features = ["full", "test-util"] }
tokio-test = "0.4"
wiremock = "0.6"
tempfile = "3.10"
predicates = "3.1"
assert_matches = "1.5"
serde_json = "1.0"
rusqlite = { version = "0.31", features = ["bundled"] }
windows = { version = "0.54", features = ["Win32_Security_Cryptography"] }
ed25519-dalek = "2.1"
```

### 2.2 Cargo Test Runner Configuration (`.cargo/config.toml`)
To guarantee execution matches target production hardware and MSVC requirements (DEC-005):

```toml
# .cargo/config.toml
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]

[env]
RUST_BACKTRACE = "1"
UNDUHKILAT_TEST_MODE = "1"
```

### 2.3 Coverage Tooling & Targets
Code coverage is collected using `cargo-llvm-cov` to generate machine-readable LCOV and summary reports without requiring external profiling toolchains.

- **Coverage Metrics & Thresholds**:
  - Critical Domain Engine (`src/engine/`): `TBD`
  - Protocol & Native Messaging Parsers (`src/api/`): `TBD`
  - Persistence & SQLite Migrations (`src/storage/`): `TBD`
  - Overall Project Coverage: `TBD`

*(All specific coverage percentage targets are `TBD` pending baseline performance metrics).*

---

## 3. Unit Test — Critical Domain Logic

Unit tests evaluate isolated pure functions, mathematical algorithms, state machines, and protocol parsers without disk or network I/O.

### 3.1 Dynamic Segment Partitioning & Re-splitting (FR-003, FR-004)
Calculates initial byte offsets and dynamic re-splitting of lagging segments.

```rust
#[cfg(test)]
mod tests {
    use crate::domain::segment::{calculate_initial_segments, split_segment, SegmentRange};

    #[test]
    fn test_calculate_initial_segments_four_connections() {
        // Given total file size 1,048,576 bytes and 4 connections (FR-003)
        let total_bytes: i64 = 1_048_576;
        let connection_count: u32 = 4;

        // When segments are computed
        let segments = calculate_initial_segments(total_bytes, connection_count).unwrap();

        // Then 4 contiguous, non-overlapping segments covering the full range are generated
        assert_eq!(segments.len(), 4);
        assert_eq!(segments[0], SegmentRange { index: 0, start: 0, end: 262_143 });
        assert_eq!(segments[1], SegmentRange { index: 1, start: 262_144, end: 524_287 });
        assert_eq!(segments[2], SegmentRange { index: 2, start: 524_288, end: 786_431 });
        assert_eq!(segments[3], SegmentRange { index: 3, start: 786_432, end: 1_048_575 });
    }

    #[test]
    fn test_dynamic_segment_split_midpoint() {
        // Given a lagging segment 0 with range 0..1,000,000 and current offset at 200,000 (FR-004)
        let original = SegmentRange { index: 0, start: 0, end: 1_000_000 };
        let current_offset: i64 = 200_000;
        let min_segment_size: i64 = 100_000; // TBD configurable threshold

        // When splitting the remainder (800,000 bytes remaining)
        let (updated_original, new_segment) = split_segment(&original, current_offset, 1, min_segment_size).unwrap();

        // Then the remaining range 200,001..1,000,000 is split at midpoint (600,000)
        assert_eq!(updated_original.start, 0);
        assert_eq!(updated_original.end, 600_000);
        assert_eq!(new_segment.index, 1);
        assert_eq!(new_segment.start, 600_001);
        assert_eq!(new_segment.end, 1_000_000);
    }
}
```

### 3.2 Effective Bandwidth Limit Resolution (FR-025, FR-026, FR-027, FR-028)
Calculates effective rate-limiting tokens where `0` denotes unlimited.

```rust
#[cfg(test)]
mod tests {
    use crate::domain::bandwidth::calculate_effective_limit;

    #[test]
    fn test_effective_speed_limit_resolution() {
        // Case 1: Both limits set, per-download is lower (FR-027)
        assert_eq!(calculate_effective_limit(2048, 512), 512);

        // Case 2: Both limits set, global is lower (FR-027)
        assert_eq!(calculate_effective_limit(512, 2048), 512);

        // Case 3: Global set, per-download unlimited (0) (FR-028)
        assert_eq!(calculate_effective_limit(1024, 0), 1024);

        // Case 4: Global unlimited (0), per-download set (FR-028)
        assert_eq!(calculate_effective_limit(0, 512), 512);

        // Case 5: Both unlimited (0)
        assert_eq!(calculate_effective_limit(0, 0), 0);
    }
}
```

### 3.3 Two-Level Queue Dispatch Ordering (DEC-002, FR-018, FR-021)
Enforces manual priority first, followed by FIFO sequence.

```rust
#[cfg(test)]
mod tests {
    use crate::domain::queue::{sort_queue_items, QueueCandidate};

    #[test]
    fn test_queue_dispatch_order_priority_then_fifo() {
        // Given four candidates with varying priorities and positions (DEC-002, FR-018)
        let items = vec![
            QueueCandidate { id: "item_a".into(), priority: 0, position: 0 },
            QueueCandidate { id: "item_b".into(), priority: 1, position: 1 },
            QueueCandidate { id: "item_c".into(), priority: 0, position: 2 },
            QueueCandidate { id: "item_d".into(), priority: 2, position: 3 },
        ];

        // When sorted according to dispatch logic
        let sorted = sort_queue_items(items);

        // Then expected dispatch order is D (prio 2), B (prio 1), A (prio 0, pos 0), C (prio 0, pos 2)
        let sorted_ids: Vec<String> = sorted.into_iter().map(|c| c.id).collect();
        assert_eq!(sorted_ids, vec!["item_d", "item_b", "item_a", "item_c"]);
    }
}
```

### 3.4 Media Candidate URL Normalization & Deduplication (DEC-003, FR-041, FR-042)
Validates deduplication of media candidates surfaced via network sniffing and DOM hooks.

```rust
#[cfg(test)]
mod tests {
    use crate::domain::media::{deduplicate_candidates, MediaCandidate};

    #[test]
    fn test_media_candidate_deduplication_by_canonical_url() {
        let candidates = vec![
            MediaCandidate {
                url: "https://cdn.example.test/stream.mp4?auth=123".into(),
                canonical_url: "https://cdn.example.test/stream.mp4".into(),
                mime_type: "video/mp4".into(),
                size_bytes: 10_485_760,
                source: "hybrid_network_sniff".into(),
            },
            MediaCandidate {
                url: "https://cdn.example.test/stream.mp4?other=456".into(),
                canonical_url: "https://cdn.example.test/stream.mp4".into(),
                mime_type: "video/mp4".into(),
                size_bytes: 10_485_760,
                source: "extension_dom_hook".into(),
            },
        ];

        let deduplicated = deduplicate_candidates(candidates);

        assert_eq!(deduplicated.len(), 1);
        assert_eq!(deduplicated[0].canonical_url, "https://cdn.example.test/stream.mp4");
        assert_eq!(deduplicated[0].size_bytes, 10_485_760);
    }
}
```

### 3.5 Wildcard Link Pattern Matcher (FR-045, FR-046)
Tests pattern evaluation for Site Grabber link extraction.

```rust
#[cfg(test)]
mod tests {
    use crate::domain::grabber::filter_links_by_wildcard;

    #[test]
    fn test_wildcard_batch_filtering() {
        let links = vec![
            "https://example.test/a/archive1.zip",
            "https://example.test/a/archive2.zip",
            "https://example.test/b/archive3.zip",
            "https://example.test/a/readme.txt",
        ];

        let pattern = "*/a/*.zip";
        let matched = filter_links_by_wildcard(&links, pattern);

        assert_eq!(matched, vec![
            "https://example.test/a/archive1.zip",
            "https://example.test/a/archive2.zip",
        ]);
    }
}
```

---

## 4. Contract Test — Response Envelope & Error Codes

Contract tests guarantee wire compatibility across the Chromium Native Messaging interface and intra-process Tokio command channels adhering to `API.md` schemas.

### 4.1 Native Messaging JSON Length-Prefix Framing (DEC-006, FR-033, FR-034)
Verifies the Chromium 32-bit native byte-order prefix serialization.

```rust
#[cfg(test)]
mod tests {
    use crate::api::framing::{read_native_frame, write_native_frame};
    use std::io::Cursor;

    #[tokio::test]
    async fn test_native_messaging_wire_framing_roundtrip() {
        let raw_json = r#"{"id":"req_01HP8X9Q2KVM7D6N1E9BZ4M529","command":"browser.ping","payload":{"browser_type":"chrome","extension_version":"1.0.0"}}"#;
        let mut buffer = Vec::new();

        // When written to stream with Chromium 32-bit length prefix
        write_native_frame(&mut buffer, raw_json).expect("Write frame must succeed");

        // Then first 4 bytes indicate exact payload length in Little-Endian
        let expected_len = raw_json.len() as u32;
        let prefix = u32::from_le_bytes(buffer[0..4].try_into().unwrap());
        assert_eq!(prefix, expected_len);

        // And reading from buffer yields original JSON
        let mut reader = Cursor::new(buffer);
        let parsed = read_native_frame(&mut reader).expect("Read frame must succeed");
        assert_eq!(parsed, raw_json);
    }
}
```

### 4.2 Standard Error Code Envelopes (API.md Section 2.4 & 2.8)
Asserts error envelopes carry canonical tokens and HTTP-equivalent structures.

```rust
#[cfg(test)]
mod tests {
    use crate::api::envelope::{ApiErrorEnvelope, ErrorBody};
    use serde_json::json;

    #[test]
    fn test_error_envelope_serialization() {
        let error_env = ApiErrorEnvelope {
            status: "error".to_string(),
            id: "req_01HP8X9Q2KVM7D6N1E9BZ4M501".to_string(),
            error: ErrorBody {
                code: "ERR_RANGE_NOT_SUPPORTED".to_string(),
                message: "Remote server does not advertise HTTP Accept-Ranges support.".to_string(),
                details: Some(json!({
                    "download_id": "550e8400-e29b-41d4-a716-446655440000",
                    "effective_url": "https://releases.example.test/packages/runtime.zip"
                })),
            },
        };

        let serialized = serde_json::to_value(&error_env).unwrap();

        assert_eq!(serialized["status"], "error");
        assert_eq!(serialized["id"], "req_01HP8X9Q2KVM7D6N1E9BZ4M501");
        assert_eq!(serialized["error"]["code"], "ERR_RANGE_NOT_SUPPORTED");
        assert_eq!(serialized["error"]["details"]["download_id"], "550e8400-e29b-41d4-a716-446655440000");
    }
}
```

---

## 5. Integration Test — Database and Workflows

Integration tests verify SQLite schema persistence, state machine invariants, transactional commits, and concurrency dispatching using transient database instances.

### 5.1 SQLite WAL Concurrency & Startup Normalization (FR-010, FR-058, DATABASE.md)
Validates that abrupt crashes leave the SQLite database consistent and that restarted processes normalize `running` state to `paused`.

```rust
#[cfg(test)]
mod tests {
    use crate::storage::sqlite::DatabaseStore;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_crash_recovery_startup_normalization() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("unduhkilat.db");

        // 1. Initialize store and run migrations
        let store = DatabaseStore::open(&db_path).await.unwrap();

        // 2. Insert mock download in 'running' state simulating abrupt kill
        let download_id = "550e8400-e29b-41d4-a716-446655440000";
        store.execute_raw(
            "INSERT INTO downloads (id, original_url, effective_url, target_path, file_name, state, created_at, updated_at) 
             VALUES (?1, 'https://example.test/file.iso', 'https://example.test/file.iso', 'C:\\Downloads\\file.iso', 'file.iso', 'running', '2026-03-30T14:00:00.000Z', '2026-03-30T14:00:00.000Z')",
            &[&download_id],
        ).await.unwrap();

        // 3. Drop connection abruptly simulating process death
        drop(store);

        // 4. Reopen database simulating application restart
        let recovered_store = DatabaseStore::open(&db_path).await.unwrap();
        recovered_store.run_startup_recovery().await.unwrap();

        // 5. Assert download state is normalized to 'paused'
        let state: String = recovered_store.query_scalar(
            "SELECT state FROM downloads WHERE id = ?1",
            &[&download_id],
        ).await.unwrap();

        assert_eq!(state, "paused");
    }
}
```

### 5.2 Dynamic Segment Checkpointing & Foreign Key Cascades (FR-006, FR-058)
Asserts that segment progress flushes to SQLite and cascades on deletion.

```rust
#[cfg(test)]
mod tests {
    use crate::storage::sqlite::DatabaseStore;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_segment_checkpoint_and_cascade_delete() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("unduhkilat.db");
        let store = DatabaseStore::open(&db_path).await.unwrap();

        let download_id = "7b8e1f2a-5c3d-4e9f-8a1b-2c3d4e5f6a7b";
        store.create_download_fixture(download_id).await.unwrap();

        // Insert segment record
        store.execute_raw(
            "INSERT INTO segments (download_id, segment_index, range_start, range_end, highest_contiguous_offset, state, updated_at)
             VALUES (?1, 0, 0, 1048575, 524288, 'downloading', '2026-03-30T14:10:00.000Z')",
            &[&download_id],
        ).await.unwrap();

        // Verify checkpointed offset
        let offset: i64 = store.query_scalar(
            "SELECT highest_contiguous_offset FROM segments WHERE download_id = ?1 AND segment_index = 0",
            &[&download_id],
        ).await.unwrap();
        assert_eq!(offset, 524_288);

        // Delete parent download and assert cascade delete on segments
        store.execute_raw("DELETE FROM downloads WHERE id = ?1", &[&download_id]).await.unwrap();

        let count: i64 = store.query_scalar(
            "SELECT COUNT(*) FROM segments WHERE download_id = ?1",
            &[&download_id],
        ).await.unwrap();
        assert_eq!(count, 0);
    }
}
```

### 5.3 Deterministic Concurrency Slots & FIFO Dispatch (DEC-001, DEC-002, FR-020)
Verifies that when concurrency limit is reached, excess tasks wait in `queued` state.

```rust
#[tokio::test]
async fn test_concurrency_slot_dispatch_ceiling() {
    let dir = tempdir().unwrap();
    let store = DatabaseStore::open(&dir.path().join("unduhkilat.db")).await.unwrap();

    // Set concurrency limit to 2
    store.set_setting("concurrency_limit", "2").await.unwrap();

    // Create 3 queued downloads
    store.create_queued_item("item_1", 0, 0).await.unwrap();
    store.create_queued_item("item_2", 0, 1).await.unwrap();
    store.create_queued_item("item_3", 0, 2).await.unwrap();

    let mut scheduler = DownloadScheduler::new(store.clone());
    let dispatched = scheduler.dispatch_next_slots().await.unwrap();

    // Exactly 2 items dispatched to running
    assert_eq!(dispatched.len(), 2);
    assert_eq!(dispatched[0], "item_1");
    assert_eq!(dispatched[1], "item_2");

    // Third item remains queued
    let state_item3 = store.get_download_state("item_3").await.unwrap();
    assert_eq!(state_item3, "queued");
}
```

---

## 6. External Provider Test

In accordance with strict isolation principles, external providers (remote HTTP servers, proxy servers, Windows DPAPI, and browser native hosts) are mocked behind interfaces. **No live network calls are made during CI runs.**

### 6.1 Multi-Connection HTTP Range Server Mock (FR-003, FR-007, FR-008)
Simulates range responses, dropped connections, and fallback behaviors using `wiremock`.

```rust
#[cfg(test)]
mod tests {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_range_server_segmented_response() {
        let server = MockServer::start().await;

        // Mock Accept-Ranges HEAD response
        Mock::given(method("HEAD"))
            .and(path("/file.iso"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Accept-Ranges", "bytes")
                    .insert_header("Content-Length", "1048576"),
            )
            .mount(&server)
            .await;

        // Mock 206 Partial Content response for Segment 0
        Mock::given(method("GET"))
            .and(path("/file.iso"))
            .and(header("Range", "bytes=0-524287"))
            .respond_with(
                ResponseTemplate::new(206)
                    .set_body_bytes(vec![0xAA; 524288])
                    .insert_header("Content-Range", "bytes 0-524287/1048576"),
            )
            .mount(&server)
            .await;

        // Mock 206 Partial Content response for Segment 1
        Mock::given(method("GET"))
            .and(path("/file.iso"))
            .and(header("Range", "bytes=524288-1048575"))
            .respond_with(
                ResponseTemplate::new(206)
                    .set_body_bytes(vec![0xBB; 524288])
                    .insert_header("Content-Range", "bytes 524288-1048575/1048576"),
            )
            .mount(&server)
            .await;

        let client = crate::engine::http::HttpClient::new();
        let result = client.download_segmented(&format!("{}/file.iso", server.uri()), 2).await;

        assert!(result.is_ok());
        let downloaded = result.unwrap();
        assert_eq!(downloaded.len(), 1_048_576);
        assert_eq!(downloaded[0], 0xAA);
        assert_eq!(downloaded[524_288], 0xBB);
    }
}
```

### 6.2 DPAPI Protection Mock / Win32 Abstraction (DEC-005, FR-014, FR-058)
Verifies credential encryption roundtrip.

```rust
#[cfg(test)]
mod tests {
    use crate::security::dpapi::{dpapi_decrypt, dpapi_encrypt};

    #[test]
    fn test_dpapi_credential_encryption_roundtrip() {
        let plaintext_password = "Secret_P@ssw0rd!2026";

        // When encrypted via DPAPI CryptProtectData
        let encrypted_base64 = dpapi_encrypt(plaintext_password).expect("DPAPI encryption must succeed");
        assert_ne!(encrypted_base64, plaintext_password);

        // When decrypted via CryptUnprotectData
        let decrypted = dpapi_decrypt(&encrypted_base64).expect("DPAPI decryption must succeed");
        assert_eq!(decrypted, plaintext_password);
    }
}
```

---

## 7. End-to-End, Performance, Security Tests & CI Gates

### 7.1 End-to-End Workflows

#### 7.1.1 Full Download Lifecycle with Resume & Verification (FR-003, FR-006, FR-007, FR-011)
Executes a segmented download, initiates a pause mid-transfer, restarts and resumes to completion, and verifies SHA-256 integrity.

```rust
#[tokio::test]
async fn test_e2e_download_lifecycle_pause_resume_integrity() {
    let server = MockServer::start().await;
    let file_payload = vec![0x5A; 2_097_152]; // 2 MB fixture
    let expected_hash = ring::digest::digest(&ring::digest::SHA256, &file_payload);

    setup_range_server(&server, &file_payload).await;

    let temp_dir = tempfile::tempdir().unwrap();
    let target_file = temp_dir.path().join("payload.bin");

    let engine = TestEngine::init(temp_dir.path()).await;
    let download_id = engine.create_task(&format!("{}/payload.bin", server.uri()), &target_file, 4).await.unwrap();

    // Start transfer
    engine.start_task(&download_id).await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        // Pause mid-transfer (FR-006)
    engine.pause_task(&download_id).await.unwrap();
    let paused_state = engine.get_state(&download_id).await.unwrap();
    assert_eq!(paused_state, DownloadState::Paused);

    let checkpointed_bytes = engine.get_downloaded_bytes(&download_id).await.unwrap();
    assert!(checkpointed_bytes > 0);
    assert!(checkpointed_bytes < 2_097_152);

    // Resume from persisted offsets (FR-007)
    engine.resume_task(&download_id).await.unwrap();
    engine.await_completion(&download_id).await.unwrap();

    // Verify final state and byte-identical integrity (FR-011)
    let final_state = engine.get_state(&download_id).await.unwrap();
    assert_eq!(final_state, DownloadState::Completed);

    let written = tokio::fs::read(&target_file).await.unwrap();
    assert_eq!(written.len(), 2_097_152);
    let actual_hash = ring::digest::digest(&ring::digest::SHA256, &written);
    assert_eq!(actual_hash.as_ref(), expected_hash.as_ref());
}
```

#### 7.1.2 Hard-Kill Crash Recovery Across Restarts (FR-010, NFR-004, KPI-005)
Spawns the application as a child process, kills it with `TerminateProcess`, and asserts full restoration of resumable state.

```rust
#[tokio::test]
async fn test_e2e_hard_kill_restores_all_in_progress_items() {
    let temp_dir = tempfile::tempdir().unwrap();
    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_unduhkilat"))
        .env("UNDUHKILAT_TEST_MODE", "1")
        .env("UNDUHKILAT_DATA_DIR", temp_dir.path())
        .spawn()
        .unwrap();

    // Seed three running downloads through the IPC test harness
    let harness = TestHarness::connect(temp_dir.path()).await.unwrap();
    let ids = harness.seed_running_downloads(3).await.unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Hard-kill the process
    kill_process_tree(child.id().unwrap());

    // Restart the application
    let _child2 = tokio::process::Command::new(env!("CARGO_BIN_EXE_unduhkilat"))
        .env("UNDUHKILAT_TEST_MODE", "1")
        .env("UNDUHKILAT_DATA_DIR", temp_dir.path())
        .spawn()
        .unwrap();

    let harness2 = TestHarness::connect(temp_dir.path()).await.unwrap();
    for id in ids {
        let state = harness2.get_state(&id).await.unwrap();
        assert_eq!(state, DownloadState::Paused);
        let offsets = harness2.get_segment_offsets(&id).await.unwrap();
        assert!(!offsets.is_empty());
    }
}
```

#### 7.1.3 Portable First-Run & No-Elevation Compliance (DEC-007, FR-059, KPI-017)
Extracts the ZIP to a clean user-writable folder and completes a download without any elevation prompt.

```rust
#[test]
fn test_e2e_portable_first_run_no_elevation() {
    let temp_dir = tempfile::tempdir().unwrap();
    extract_portable_zip(&temp_dir).unwrap();

    let status = std::process::Command::new(temp_dir.path().join("UnduhKilat.exe"))
        .arg("--headless-first-run-check")
        .env("UNDUHKILAT_DATA_DIR", temp_dir.path().join("data"))
        .status()
        .unwrap();

    assert!(status.success());
    // Confirm no artefacts written outside the bundle folder, data dirs, or HKCU
    assert_no_outside_writes(&temp_dir);
}
```

#### 7.1.4 Native Host Registration & Browser Capture Path (DEC-006, FR-034, FR-036, KPI-010)
Verifies that per-user first-run registration writes `HKCU` keys pointing at the host binary and that a synthetic native messaging frame is accepted.

```rust
#[tokio::test]
async fn test_e2e_browser_capture_handshake() {
    let temp_dir = tempfile::tempdir().unwrap();
    let app = TestApp::start_from(temp_dir.path()).await.unwrap();

    // First-run registration under HKCU (FR-034)
    app.ensure_hkcu_registration().await.unwrap();
    let host_path = read_hkcu_native_host_path("com.unduhkilat.host").unwrap();
    assert!(host_path.ends_with("unduhkilat-host.exe"));
    assert!(std::path::Path::new(&host_path).exists());

    // Send a synthetic browser.intercept frame
    let frame = json!({
        "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M530",
        "command": "browser.intercept",
        "payload": {
            "url": "https://example.test/file.bin",
            "suggested_filename": "file.bin",
            "referer": "https://example.test/page.html",
            "user_agent": "Mozilla/5.0",
            "cookies": "sess_auth=xyz",
            "tab_id": 14205
        }
    });
    let resp = app.send_native_frame(&frame).await.unwrap();
    assert_eq!(resp["status"], "ok");
    assert_eq!(resp["data"]["dialog_presented"], true);
}
```

### 7.2 Performance Tests

#### 7.2.1 Unbounded Concurrency Honour (DEC-001, NFR-003)
Verifies that settings accept and enforce any positive user-specified value without an internal clamp.

```rust
#[tokio::test]
async fn test_concurrency_no_ceiling_honoured() {
    let store = DatabaseStore::in_memory().await.unwrap();
    for n in [1_u32, 8, 64, 500, 2000] {
        store.set_setting("concurrency_limit", &n.to_string()).await.unwrap();
        let read_back: u32 = store.get_setting("concurrency_limit").await.unwrap();
        assert_eq!(read_back, n, "concurrency_limit must not be clamped");

        store.set_setting("default_connections_per_download", &n.to_string()).await.unwrap();
        let read_back: u32 = store.get_setting("default_connections_per_download").await.unwrap();
        assert_eq!(read_back, n, "default_connections_per_download must not be clamped");
    }
}
```

#### 7.2.2 Throttle Accuracy under Aggregate Load (NFR-007, KPI-009)
Measures aggregate throughput against the configured cap over a sampled window; tolerance and sampling window remain `TBD`, so the test records the observed metric instead of asserting a bound (TBD-020, §7.6 item 6).

```rust
#[tokio::test]
async fn test_global_speed_limit_respected() {
    let server = MockServer::start().await;
    setup_unthrottled_stream(&server, 4 * 1024 * 1024).await; // 4 MB fixture

    let engine = TestEngine::init_in_memory().await;
    engine.set_global_speed_limit(100).await.unwrap(); // 100 KB/s
    let id = engine.create_task(&format!("{}/payload.bin", server.uri()), 4).await.unwrap();
    engine.start_task(&id).await.unwrap();

    let samples = engine.sample_speeds_ms(1000, 5).await; // sampling window TBD

    for s in samples {
        // Report observed throughput; no pass/fail until the throttle tolerance is fixed (TBD-020).
        record_metric("throttle.aggregate_bps", s.aggregate_bps as f64);
        // Reinstate the bound assertion once the tolerance is specified:
        // assert!(s.aggregate_bps <= 100 * 1024 + /* tolerance TBD */);
    }
}
```

#### 7.2.3 UI Refresh Cadence Baseline (NFR-006)
Records observed refresh interval while transfers run; the numeric target is `TBD`.

```rust
#[test]
fn test_ui_refresh_interval_measurement() {
    // Harness runs the egui update loop and logs the timestamps of progress updates
    let log = uifx::capture_update_ticks(4 /* downloads */, 2 /* seconds */);
    assert!(!log.progress_ticks.is_empty());
    // Report observed interval; no pass/fail until target fixed (TBD)
    record_metric("ui.refresh_interval_ms", log.mean_interval_ms());
}
```

### 7.3 Security Tests

#### 7.3.1 Signed-Update Verification & Rejection (FR-060, NFR-013, KPI-018)
Asserts unsigned and corrupted packages are rejected and leave the bundle untouched.

```rust
#[test]
fn test_unsigned_and_corrupt_update_rejected() {
    let bundle = tempfile::tempdir().unwrap();
    seed_bundle(&bundle);
    let before = hash_tree(bundle.path());

    // Unsigned package
    let unsigned = tempfile::NamedTempFile::new().unwrap();
    zero_fill(&unsigned, 4096);
    let err = apply_update(bundle.path(), unsigned.path(), None).unwrap_err();
    assert_eq!(err.code, "ERR_PACKAGE_SIGNATURE_INVALID");
    assert_eq!(hash_tree(bundle.path()), before);

    // Corrupted signature package
    let signed = make_valid_signed_package();
    let sig = make_corrupted_signature();
    let err = apply_update(bundle.path(), &signed, Some(&sig)).unwrap_err();
    assert_eq!(err.code, "ERR_PACKAGE_SIGNATURE_INVALID");
    assert_eq!(hash_tree(bundle.path()), before);

    // Valid package succeeds
    let resp = apply_update(bundle.path(), &signed, Some(&make_valid_signature(&signed)));
    assert!(resp.is_ok());
}
```

#### 7.3.2 No Off-Machine Telemetry Egress (FR-057, NFR-016)
Full session captured under an egress firewall; only the download URLs and update endpoints are contacted.

```rust
#[tokio::test]
async fn test_no_telemetry_egress_in_full_session() {
    let capture = EgressCapture::start();
    let app = TestApp::start_with_egress_capture(&capture).await.unwrap();

    app.run_full_scenario(
        /* downloads */ 3,
        /* captures */ 2,
        /* export_logs */ true,
        /* check_update */ true,
    ).await.unwrap();

    let destinations = capture.destinations();
    for dest in destinations {
        assert!(is_allowed_destination(&dest),
            "Unexpected egress to {} (only download URLs and update channel are permitted)", dest);
    }
}
```

#### 7.3.3 Native Host Origin Validation (FR-033, FR-034)
A native messaging frame from an unregistered extension ID is rejected with `ERR_UNAUTHORIZED`.

```rust
#[tokio::test]
async fn test_native_host_origin_validation() {
    let host = NativeHost::start_registered_with(&["chrome-extension://good-id", "chrome-extension://brave-id"]);
    let resp = host.send_frame_from("chrome-extension://untrusted-id", json!({
        "id": "req_01HP8X9Q2KVM7D6N1E9BZ4M999",
        "command": "browser.ping",
        "payload": { "browser_type": "chrome", "extension_version": "1.0.0" }
    })).await.unwrap();

    assert_eq!(resp["status"], "error");
    assert_eq!(resp["error"]["code"], "ERR_UNAUTHORIZED");
}
```

#### 7.3.4 Admin Operations Require Elevation (FR-035, FR-061, FR-062)
`admin.*` operations invoked from a non-elevated context fail with `ERR_PERMISSION_DENIED` and produce no side effects.

```rust
#[tokio::test]
async fn test_admin_endpoints_require_elevation() {
    let ctx = AdminContext::unelevated();
    for cmd in ["admin.registry.register_machine_wide", "admin.bundle.replace_machine_wide", "admin.bundle.reset_data"] {
        let resp = ctx.invoke(cmd, json!({ "confirm_reset": true })).await.unwrap();
        assert_eq!(resp["status"], "error");
        assert_eq!(resp["error"]["code"], "ERR_PERMISSION_DENIED");
    }
    // No HKLM write, no bundle modification
    assert!(read_hklm_native_host_path().is_none());
}
```

### 7.4 CI Gates & Pipeline Integration

The CI pipeline enforces every gate listed below on `windows-latest` (x64, MSVC toolchain) — the only supported platform (DEC-005, TC-001, TC-009).

| Gate | Command | Required Outcome | Blocking |
|---|---|---|---|
| Format | `cargo fmt --all -- --check` | Pass | Yes |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` | Pass | Yes |
| Build | `cargo build --release --target x86_64-pc-windows-msvc` | Pass | Yes |
| Unit | `cargo test --lib` | All pass | Yes |
| Contract | `cargo test --test contract` | All pass | Yes |
| Integration | `cargo test --test integration` | All pass | Yes |
| E2E (headless) | `cargo test --test e2e -- --test-threads=1` | All pass | Yes |
| Coverage | `cargo llvm-cov --workspace --summary-only` | Thresholds `TBD` | Advisory until thresholds fixed |
| NFR gates | `cargo test --test nfr` (concurrency honour, crash durability repeat, throttle) | All pass | Yes |
| Security | `cargo test --test security` (unsigned update, egress, origin validation, admin elevation) | All pass | Yes |
| Licence/Audit | `cargo deny check` | Pass | Yes |
| No-network check | Run integration suite with egress blocked (localhost only) | Pass | Yes |

**.github/workflows/ci.yml (excerpt)**

```yaml
name: ci
on: [push, pull_request]
jobs:
  test:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: stable-x86_64-pc-windows-msvc
          components: rustfmt, clippy, llvm-tools-preview
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all -- --check
      - run: cargo clippy --all-targets --all-features -- -D warnings
      - run: cargo build --release --target x86_64-pc-windows-msvc
      - run: cargo test --lib
      - run: cargo test --test contract
      - run: cargo test --test integration
      - run: cargo test --test security
      - run: cargo test --test nfr
      - run: cargo test --test e2e -- --test-threads=1
      - name: Coverage
        run: |
          cargo install cargo-llvm-cov --locked
          cargo llvm-cov --workspace --summary-only
```

### 7.5 Open `TBD` Register for Test Coverage

| TBD ID | Open Item Affected | Test Impact |
|---|---|---|
| TBD-001 | Split thresholds for dynamic re-splitting | `test_dynamic_segment_split_midpoint` injects the threshold as a literal parameter (`let min_segment_size: i64 = 100_000;`); no configuration lookup is exercised, so the assertion on the shipped default is blocked. |
| TBD-002 | Minimum segment size | Injected as a literal parameter in the split test; no configuration lookup and no assertion on shipped default. |
| TBD-003 | Re-split frequency | Measured via harness; no numeric pass/fail until fixed. |
| TBD-004 / TBD-005 | Validation of `0` / negative concurrency and connections | Tests assert only that positive integers are accepted without clamping (DEC-001). |
| TBD-006 / TBD-007 | Retry count, backoff, give-up condition | `test_range_server_segmented_response` covers resume; give-up test marked `blocked`. |
| TBD-008 | Restart on completed item | Behaviour observed, not asserted. |
| TBD-009 | Checkpoint interval | Concealed by crash test; only "at most one interval lost" property asserted. |
| TBD-010 | File-conflict resolution policy | Assertions limited to invariants in FR-016 acceptance criteria. |
| TBD-011 | Cookie scoping rule across redirects | Redirect test asserts only that `Referer` is preserved; cookie scope observed. |
| TBD-012 | Proxy support for FTP | Direct HTTP proxy path tested; FTP proxy behaviour observed. |
| TBD-013 | Creation of missing category folders | Destination path equality asserted; creation is observed. |
| TBD-014 | Log-level value set | Log level round-trip tested; exact token set observed. |
| TBD-015 | Log rotation and export format | Rotation observed; export file count/paths asserted only as configured. |
| TBD-016 | DASH/HLS handling | Panel listing asserted; download outcome observed, never asserted as completed merge. |
| TBD-017 | Default maximum site-walk depth | Depth field tested as user-configurable; shipped default observed. |
| TBD-018 | Scheduler clock change across boundary | Behaviour observed; scheduler evaluation rule asserted only as configured. |
| TBD-019 | Coverage percentage targets (§2.3) | Advisory until thresholds are fixed. |
| TBD-020 | Numeric UI refresh, throttle tolerance, sampling window | Recorded metrics; no pass/fail until fixed. |
| TBD-021 | Native host timeout (`ERR_NATIVE_HOST_TIMEOUT`) | Timeout tests assert token only; ms value injected from configuration. |
| TBD-022 | Repetition count N for the crash-durability (KPI-005) and integrity (KPI-007) sign-off runs | §7.6 item 3 records N as `[placeholder]`; the sign-off criterion cannot be verified or tracked until N is fixed. |

### 7.6 Acceptance Sign-off Criteria

A release is accepted only when all of the following hold:

1. Every `MUST` / `MUST NOT` statement in REQUIREMENTS §2 and §3 has at least one passing executable case in the suites above, except cases explicitly marked `blocked: TBD` in §7.5.
2. Every non-blocking `TBD` case is logged in the release notes with the observed value recorded verbatim for later specification.
3. The crash-durability suite (KPI-005) and integrity suite (KPI-007) pass on N consecutive runs where N = `[placeholder]` (registered as TBD-022 in §7.5).
4. The security suite passes with zero external egress beyond download URLs and update packages (FR-057, NFR-016).
5. All gates in §7.4 pass on `windows-latest` x64 (DEC-005, NFR-001, NFR-002).
6. No test asserts a concrete number for an open `TBD` item; any such assertion is a defect in the test suite itself.