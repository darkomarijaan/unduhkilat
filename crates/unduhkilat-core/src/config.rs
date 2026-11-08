//! Engine tunables.
//!
//! IMPORTANT: several of these values are `TBD` in the product documents
//! (PRD NFR table, DATABASE.md §3.7). The values below are PROVISIONAL
//! development defaults so the engine can run; they are NOT product decisions
//! (docs/18_README.md: do not silently finalize TBDs). Each is overridable at
//! runtime via the `settings` table or by constructing `EngineConfig` directly,
//! and each is listed in the project report for the owner to decide.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// PROVISIONAL (TBD: `checkpoint_interval_ms`, FR-058 / NFR crash durability).
    pub checkpoint_interval: Duration,
    /// PROVISIONAL (TBD: `concurrency_limit` default, DATABASE.md §3.7).
    pub default_concurrency: usize,
    /// Documented default (DATABASE.md §3.7 `default_connections_per_download` = 8).
    pub default_connections: u32,
    /// PROVISIONAL technical guard against a connection that stalls forever.
    /// Not in the docs; a stall surfaces as ERR_CONNECTION_DROPPED.
    pub read_timeout: Duration,
    pub connect_timeout: Duration,
    pub user_agent: String,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            checkpoint_interval: Duration::from_millis(500),
            default_concurrency: 3,
            default_connections: 8,
            read_timeout: Duration::from_secs(60),
            connect_timeout: Duration::from_secs(30),
            user_agent: format!("UnduhKilat/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

pub mod keys {
    pub const CONCURRENCY_LIMIT: &str = "concurrency_limit";
    /// Server transfers (download/upload/copy with saved servers) run in their own lanes.
    pub const SERVER_CONCURRENCY_LIMIT: &str = "server_concurrency_limit";
    pub const DEFAULT_CONNECTIONS: &str = "default_connections_per_download";
    pub const GLOBAL_SPEED_LIMIT_KBPS: &str = "global_speed_limit_kbps";
    pub const LOG_LEVEL: &str = "log_level";
    /// DEC-004. Default on.
    pub const CLOSE_TO_TRAY: &str = "close_to_tray_enabled";
    /// DEC-004 / FR-054. Default on.
    pub const COMPLETION_TOAST: &str = "completion_toast_enabled";
    /// FR-040. Default OFF: reading the clipboard is opt-in.
    pub const CLIPBOARD_MONITOR: &str = "clipboard_monitor_enabled";
    pub const UPDATE_CHECK: &str = "update_check_enabled";
    pub const TERMINAL_SHELL: &str = "terminal_shell";
    pub const TERMINAL_START_DIR: &str = "terminal_start_dir";
    pub const TERMINAL_FONT_SIZE: &str = "terminal_font_size";
    pub const TERMINAL_CTRL_V_PASTES: &str = "terminal_ctrl_v_pastes";
    pub const TERMINAL_LAYOUT: &str = "terminal_layout";
    /// Largest file the preview will fetch whole, in MB (default 25).
    pub const PREVIEW_MAX_MB: &str = "preview_max_mb";
    /// DEC-008. PROVISIONAL defaults: seeding on, ratio 1.0, DHT on, UPnP off, random port.
    pub const TORRENT_SEEDING: &str = "torrent_seeding_enabled";
    /// Global target share ratio; `0` means seed until the user stops it.
    pub const TORRENT_SEED_RATIO: &str = "torrent_seed_ratio";
    pub const TORRENT_DHT: &str = "torrent_dht_enabled";
    pub const TORRENT_UPNP: &str = "torrent_upnp_enabled";
    /// `0` = let the OS pick.
    pub const TORRENT_PORT: &str = "torrent_listen_port";
}
