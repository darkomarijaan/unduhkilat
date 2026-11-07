//! Domain models mapped to SQLite rows (DATABASE.md §4.1).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadState {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl DownloadState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SegmentState {
    Pending,
    Downloading,
    Completed,
}

impl SegmentState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Downloading => "downloading",
            Self::Completed => "completed",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "downloading" => Self::Downloading,
            "completed" => Self::Completed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub target_directory: String,
    pub file_extensions: String,
    pub is_default: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    pub id: String,
    pub original_url: String,
    pub effective_url: String,
    pub target_path: String,
    pub file_name: String,
    pub total_bytes: i64,
    pub downloaded_bytes: i64,
    pub state: DownloadState,
    pub segmentation_supported: bool,
    pub connection_count: u32,
    pub speed_limit_kbps: u32,
    pub category_id: Option<String>,
    pub mime_type: Option<String>,
    pub referer: Option<String>,
    pub user_agent: Option<String>,
    pub cookies: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

/// `highest_contiguous_offset` is the offset of the NEXT byte to fetch
/// (DATABASE.md §3.2): `range_start` when untouched, `range_end + 1` when done.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub id: i64,
    pub download_id: String,
    pub segment_index: u32,
    pub range_start: i64,
    pub range_end: i64,
    pub highest_contiguous_offset: i64,
    pub state: SegmentState,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: String,
    pub download_id: String,
    pub priority: i32,
    pub queue_position: u32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    pub id: String,
    pub host: String,
    pub username: String,
    pub password_encrypted: String,
    pub port: Option<u16>,
    pub protocol: String,
    pub created_at: String,
    pub updated_at: String,
}
