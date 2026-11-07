//! Download engine: probe, segmentation, throttling, transfer workers, queue manager.

pub mod ftp;
pub mod grabber;
pub mod http;
pub mod manager;
pub mod proxy;
pub mod search;
pub mod segments;
pub mod throttle;
pub mod torrent;
pub mod transfer;

use crate::models::DownloadState;
use serde::Serialize;

/// Telemetry pushed from the engine to the UI / browser bridge (API.md §2.5).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineEvent {
    Progress {
        id: String,
        downloaded_bytes: i64,
        total_bytes: i64,
        current_speed_bps: u64,
        active_connections: u32,
    },
    StateChanged {
        id: String,
        state: DownloadState,
        error_code: Option<String>,
    },
}
