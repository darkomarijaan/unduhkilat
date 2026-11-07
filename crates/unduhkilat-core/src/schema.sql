-- 0001_initial_schema (DATABASE.md section 4)
CREATE TABLE IF NOT EXISTS categories (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    target_directory TEXT NOT NULL,
    file_extensions TEXT NOT NULL,
    is_default INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_categories_name ON categories (name);
CREATE INDEX IF NOT EXISTS idx_categories_is_default ON categories (is_default);

CREATE TABLE IF NOT EXISTS downloads (
    id TEXT PRIMARY KEY NOT NULL,
    original_url TEXT NOT NULL,
    effective_url TEXT NOT NULL,
    target_path TEXT NOT NULL,
    file_name TEXT NOT NULL,
    total_bytes INTEGER NOT NULL DEFAULT -1,
    downloaded_bytes INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL DEFAULT 'queued'
        CHECK (state IN ('queued', 'running', 'paused', 'completed', 'failed', 'cancelled')),
    segmentation_supported INTEGER NOT NULL DEFAULT 1 CHECK (segmentation_supported IN (0, 1)),
    connection_count INTEGER NOT NULL DEFAULT 8 CHECK (connection_count >= 1),
    speed_limit_kbps INTEGER NOT NULL DEFAULT 0 CHECK (speed_limit_kbps >= 0),
    category_id TEXT NULL,
    mime_type TEXT NULL,
    referer TEXT NULL,
    user_agent TEXT NULL,
    cookies TEXT NULL,
    error_code TEXT NULL,
    error_message TEXT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT NULL,
    FOREIGN KEY (category_id) REFERENCES categories (id) ON DELETE SET NULL ON UPDATE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_downloads_state ON downloads (state);
CREATE INDEX IF NOT EXISTS idx_downloads_category_id ON downloads (category_id);
CREATE INDEX IF NOT EXISTS idx_downloads_created_at ON downloads (created_at DESC);

CREATE TABLE IF NOT EXISTS segments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    download_id TEXT NOT NULL,
    segment_index INTEGER NOT NULL CHECK (segment_index >= 0),
    range_start INTEGER NOT NULL CHECK (range_start >= 0),
    range_end INTEGER NOT NULL CHECK (range_end >= range_start),
    highest_contiguous_offset INTEGER NOT NULL CHECK (highest_contiguous_offset >= range_start),
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'downloading', 'completed')),
    updated_at TEXT NOT NULL,
    FOREIGN KEY (download_id) REFERENCES downloads (id) ON DELETE CASCADE ON UPDATE CASCADE,
    CONSTRAINT uq_segments_download_index UNIQUE (download_id, segment_index)
);
CREATE INDEX IF NOT EXISTS idx_segments_download_id ON segments (download_id);

CREATE TABLE IF NOT EXISTS queue_items (
    id TEXT PRIMARY KEY NOT NULL,
    download_id TEXT NOT NULL UNIQUE,
    priority INTEGER NOT NULL DEFAULT 0,
    queue_position INTEGER NOT NULL CHECK (queue_position >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (download_id) REFERENCES downloads (id) ON DELETE CASCADE ON UPDATE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_queue_dispatch ON queue_items (priority DESC, queue_position ASC);
CREATE UNIQUE INDEX IF NOT EXISTS uq_queue_download_id ON queue_items (download_id);

CREATE TABLE IF NOT EXISTS credentials (
    id TEXT PRIMARY KEY NOT NULL,
    host TEXT NOT NULL,
    username TEXT NOT NULL,
    password_encrypted TEXT NOT NULL,
    port INTEGER NULL CHECK (port IS NULL OR (port >= 1 AND port <= 65535)),
    protocol TEXT NOT NULL DEFAULT 'http' CHECK (protocol IN ('http', 'https', 'ftp', 'proxy')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CONSTRAINT uq_credentials_host_port_user UNIQUE (host, port, username, protocol)
);
CREATE INDEX IF NOT EXISTS idx_credentials_host_protocol ON credentials (host, protocol);
CREATE UNIQUE INDEX IF NOT EXISTS uq_credentials_host_null_port_user
    ON credentials (host, username, protocol) WHERE port IS NULL;

CREATE TABLE IF NOT EXISTS schedules (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    start_time TEXT NULL,
    stop_time TEXT NULL,
    days_of_week TEXT NOT NULL DEFAULT '1,2,3,4,5,6,7',
    is_active INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_schedules_is_active ON schedules (is_active);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- 0002_torrents (DEC-008): BitTorrent state kept next to the generic download row.
CREATE TABLE IF NOT EXISTS torrents (
    download_id TEXT PRIMARY KEY NOT NULL,
    info_hash TEXT NOT NULL,
    metainfo BLOB NULL,
    started INTEGER NOT NULL DEFAULT 0 CHECK (started IN (0, 1)),
    uploaded_bytes INTEGER NOT NULL DEFAULT 0 CHECK (uploaded_bytes >= 0),
    seed_state TEXT NOT NULL DEFAULT 'none' CHECK (seed_state IN ('none', 'seeding', 'done')),
    seed_ratio REAL NULL CHECK (seed_ratio IS NULL OR seed_ratio >= 0),
    FOREIGN KEY (download_id) REFERENCES downloads (id) ON DELETE CASCADE ON UPDATE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_torrents_info_hash ON torrents (info_hash);

-- 0003_servers: saved SSH/SFTP/FTP/FTPS servers, jump chains, encrypted secrets and pinned host keys.
-- Additive only: no existing table or column is changed, so an older build ignores these tables.
CREATE TABLE IF NOT EXISTS servers (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE,
    protocol TEXT NOT NULL CHECK (protocol IN ('ssh', 'sftp', 'ftp', 'ftps')),
    host TEXT NOT NULL,
    port INTEGER NOT NULL CHECK (port >= 1 AND port <= 65535),
    username TEXT NOT NULL,
    auth TEXT NOT NULL CHECK (auth IN ('password', 'key', 'agent')),
    key_path TEXT NULL,
    save_secret INTEGER NOT NULL DEFAULT 0 CHECK (save_secret IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Jump chain of a server: position 0 is the outermost hop (reached first).
CREATE TABLE IF NOT EXISTS server_hops (
    server_id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    hop_id TEXT NOT NULL,
    PRIMARY KEY (server_id, position),
    FOREIGN KEY (server_id) REFERENCES servers (id) ON DELETE CASCADE,
    FOREIGN KEY (hop_id) REFERENCES servers (id) ON DELETE RESTRICT
);
CREATE INDEX IF NOT EXISTS idx_server_hops_hop ON server_hops (hop_id);

CREATE TABLE IF NOT EXISTS server_secrets (
    server_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('password', 'passphrase')),
    ciphertext TEXT NOT NULL,
    PRIMARY KEY (server_id, kind),
    FOREIGN KEY (server_id) REFERENCES servers (id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS host_keys (
    host TEXT NOT NULL,
    port INTEGER NOT NULL,
    key_type TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    trusted_at TEXT NOT NULL,
    PRIMARY KEY (host, port, key_type)
);
