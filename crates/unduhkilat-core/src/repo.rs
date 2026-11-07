//! SQL access functions. Functions taking `&Connection` work on both the writer
//! and the read-only connections; mutating ones must run through `Db::write`.

use crate::error::{AppError, Result};
use crate::models::*;
use rusqlite::{params, Connection, OptionalExtension, Row};

pub fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

const DOWNLOAD_COLS: &str = "id, original_url, effective_url, target_path, file_name, total_bytes, \
    downloaded_bytes, state, segmentation_supported, connection_count, speed_limit_kbps, category_id, \
    mime_type, referer, user_agent, cookies, error_code, error_message, created_at, updated_at, completed_at";

fn row_to_download(r: &Row) -> rusqlite::Result<Download> {
    let state: String = r.get(7)?;
    Ok(Download {
        id: r.get(0)?,
        original_url: r.get(1)?,
        effective_url: r.get(2)?,
        target_path: r.get(3)?,
        file_name: r.get(4)?,
        total_bytes: r.get(5)?,
        downloaded_bytes: r.get(6)?,
        state: DownloadState::parse(&state).unwrap_or(DownloadState::Failed),
        segmentation_supported: r.get::<_, i64>(8)? != 0,
        connection_count: r.get::<_, i64>(9)? as u32,
        speed_limit_kbps: r.get::<_, i64>(10)? as u32,
        category_id: r.get(11)?,
        mime_type: r.get(12)?,
        referer: r.get(13)?,
        user_agent: r.get(14)?,
        cookies: r.get(15)?,
        error_code: r.get(16)?,
        error_message: r.get(17)?,
        created_at: r.get(18)?,
        updated_at: r.get(19)?,
        completed_at: r.get(20)?,
    })
}

fn row_to_segment(r: &Row) -> rusqlite::Result<Segment> {
    let st: String = r.get(6)?;
    Ok(Segment {
        id: r.get(0)?,
        download_id: r.get(1)?,
        segment_index: r.get::<_, i64>(2)? as u32,
        range_start: r.get(3)?,
        range_end: r.get(4)?,
        highest_contiguous_offset: r.get(5)?,
        state: SegmentState::parse(&st).unwrap_or(SegmentState::Pending),
        updated_at: r.get(7)?,
    })
}

// ---- downloads -------------------------------------------------------------

pub fn insert_download(conn: &Connection, d: &Download) -> Result<()> {
    conn.execute(
        "INSERT INTO downloads (id, original_url, effective_url, target_path, file_name, total_bytes, \
         downloaded_bytes, state, segmentation_supported, connection_count, speed_limit_kbps, category_id, \
         mime_type, referer, user_agent, cookies, error_code, error_message, created_at, updated_at, completed_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
        params![
            d.id, d.original_url, d.effective_url, d.target_path, d.file_name, d.total_bytes,
            d.downloaded_bytes, d.state.as_str(), d.segmentation_supported as i64, d.connection_count,
            d.speed_limit_kbps, d.category_id, d.mime_type, d.referer, d.user_agent, d.cookies,
            d.error_code, d.error_message, d.created_at, d.updated_at, d.completed_at
        ],
    )?;
    Ok(())
}

pub fn get_download(conn: &Connection, id: &str) -> Result<Download> {
    conn.query_row(&format!("SELECT {DOWNLOAD_COLS} FROM downloads WHERE id = ?1"), [id], row_to_download)
        .optional()?
        .ok_or_else(|| AppError::not_found("download", id))
}

pub fn list_downloads(conn: &Connection, state: Option<DownloadState>) -> Result<Vec<Download>> {
    let (sql, st) = match state {
        Some(s) => (
            format!("SELECT {DOWNLOAD_COLS} FROM downloads WHERE state = ?1 ORDER BY created_at DESC, id"),
            Some(s.as_str()),
        ),
        None => (format!("SELECT {DOWNLOAD_COLS} FROM downloads ORDER BY created_at DESC, id"), None),
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = match st {
        Some(s) => stmt.query_map([s], row_to_download)?.collect::<rusqlite::Result<Vec<_>>>()?,
        None => stmt.query_map([], row_to_download)?.collect::<rusqlite::Result<Vec<_>>>()?,
    };
    Ok(rows)
}

pub fn set_state(
    conn: &Connection,
    id: &str,
    state: DownloadState,
    err: Option<(&str, &str)>,
) -> Result<()> {
    let ts = now();
    let completed = (state == DownloadState::Completed).then(|| ts.clone());
    let (code, msg) = match err {
        Some((c, m)) => (Some(c), Some(m)),
        None => (None, None),
    };
    let n = conn.execute(
        "UPDATE downloads SET state=?2, error_code=?3, error_message=?4, updated_at=?5, \
         completed_at=COALESCE(?6, CASE WHEN ?2='completed' THEN completed_at ELSE NULL END) WHERE id=?1",
        params![id, state.as_str(), code, msg, ts, completed],
    )?;
    if n == 0 {
        return Err(AppError::not_found("download", id));
    }
    Ok(())
}

pub fn update_probe_result(
    conn: &Connection,
    id: &str,
    effective_url: &str,
    total_bytes: i64,
    mime: Option<&str>,
    segmentation_supported: bool,
) -> Result<()> {
    conn.execute(
        "UPDATE downloads SET effective_url=?2, total_bytes=?3, mime_type=COALESCE(?4, mime_type), \
         segmentation_supported=?5, updated_at=?6 WHERE id=?1",
        params![id, effective_url, total_bytes, mime, segmentation_supported as i64, now()],
    )?;
    Ok(())
}

pub fn set_speed_limit(conn: &Connection, id: &str, kbps: u32) -> Result<()> {
    let n = conn.execute(
        "UPDATE downloads SET speed_limit_kbps=?2, updated_at=?3 WHERE id=?1",
        params![id, kbps, now()],
    )?;
    if n == 0 {
        return Err(AppError::not_found("download", id));
    }
    Ok(())
}

/// FR-010 / USER_FLOW Flow 4. Must commit before any worker is scheduled.
/// Returns the number of downloads normalized.
pub fn normalize_startup(conn: &mut Connection) -> Result<usize> {
    let tx = conn.transaction()?;
    let n = tx.execute(
        "UPDATE downloads SET state='paused', updated_at=?1 WHERE state='running'",
        [now()],
    )?;
    tx.execute("UPDATE segments SET state='pending', updated_at=?1 WHERE state='downloading'", [now()])?;
    tx.commit()?;
    Ok(n)
}

// ---- segments --------------------------------------------------------------

pub fn insert_segments(conn: &mut Connection, download_id: &str, ranges: &[(i64, i64)]) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut st = tx.prepare(
            "INSERT INTO segments (download_id, segment_index, range_start, range_end, \
             highest_contiguous_offset, state, updated_at) VALUES (?1,?2,?3,?4,?3,'pending',?5)",
        )?;
        let ts = now();
        for (i, (s, e)) in ranges.iter().enumerate() {
            st.execute(params![download_id, i as i64, s, e, ts])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn get_segments(conn: &Connection, download_id: &str) -> Result<Vec<Segment>> {
    let mut st = conn.prepare(
        "SELECT id, download_id, segment_index, range_start, range_end, highest_contiguous_offset, state, \
         updated_at FROM segments WHERE download_id=?1 ORDER BY segment_index",
    )?;
    let rows = st.query_map([download_id], row_to_segment)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn delete_segments(conn: &Connection, download_id: &str) -> Result<()> {
    conn.execute("DELETE FROM segments WHERE download_id=?1", [download_id])?;
    Ok(())
}

/// Atomic checkpoint: `(segment_index, next_offset)` pairs. Offsets are monotonic
/// (never lowered) and `downloaded_bytes` is recomputed from the segments.
pub fn checkpoint(conn: &mut Connection, download_id: &str, offsets: &[(u32, i64)]) -> Result<i64> {
    let tx = conn.transaction()?;
    let ts = now();
    {
        let mut st = tx.prepare(
            "UPDATE segments SET highest_contiguous_offset = MAX(highest_contiguous_offset, ?3), \
             state = CASE WHEN MAX(highest_contiguous_offset, ?3) >= range_end + 1 THEN 'completed' \
                          WHEN MAX(highest_contiguous_offset, ?3) > range_start THEN 'downloading' \
                          ELSE state END, \
             updated_at = ?4 WHERE download_id=?1 AND segment_index=?2",
        )?;
        for (idx, off) in offsets {
            st.execute(params![download_id, *idx as i64, off, ts])?;
        }
    }
    let done: i64 = tx.query_row(
        "SELECT COALESCE(SUM(highest_contiguous_offset - range_start), 0) FROM segments WHERE download_id=?1",
        [download_id],
        |r| r.get(0),
    )?;
    tx.execute("UPDATE downloads SET downloaded_bytes=?2, updated_at=?3 WHERE id=?1", params![download_id, done, ts])?;
    tx.commit()?;
    Ok(done)
}

pub fn set_downloaded_bytes(conn: &Connection, id: &str, bytes: i64) -> Result<()> {
    conn.execute("UPDATE downloads SET downloaded_bytes=?2, updated_at=?3 WHERE id=?1", params![id, bytes, now()])?;
    Ok(())
}

/// FR-009: forget all progress so the transfer starts from byte 0.
pub fn reset_progress(conn: &mut Connection, id: &str) -> Result<()> {
    let tx = conn.transaction()?;
    delete_segments(&tx, id)?;
    tx.execute(
        "UPDATE downloads SET downloaded_bytes=0, error_code=NULL, error_message=NULL, completed_at=NULL, \
         updated_at=?2 WHERE id=?1",
        params![id, now()],
    )?;
    tx.commit()?;
    Ok(())
}

// ---- queue -----------------------------------------------------------------

pub fn enqueue(conn: &Connection, download_id: &str, priority: i32) -> Result<()> {
    let pos: i64 = conn.query_row("SELECT COALESCE(MAX(queue_position), -1) + 1 FROM queue_items", [], |r| r.get(0))?;
    let ts = now();
    conn.execute(
        "INSERT INTO queue_items (id, download_id, priority, queue_position, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?5)",
        params![uuid::Uuid::new_v4().to_string(), download_id, priority, pos, ts],
    )?;
    Ok(())
}

/// Authoritative dispatch rule (DEC-002, FR-018): priority DESC, queue_position ASC.
pub fn next_dispatch_candidates(conn: &Connection, limit: usize) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT q.download_id FROM queue_items q JOIN downloads d ON d.id = q.download_id \
         WHERE d.state='queued' ORDER BY q.priority DESC, q.queue_position ASC LIMIT ?1",
    )?;
    let rows = st.query_map([limit as i64], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn set_priority(conn: &Connection, download_id: &str, priority: i32) -> Result<()> {
    let n = conn.execute(
        "UPDATE queue_items SET priority=?2, updated_at=?3 WHERE download_id=?1",
        params![download_id, priority, now()],
    )?;
    if n == 0 {
        return Err(AppError::not_found("queue item", download_id));
    }
    Ok(())
}

/// FR-021: dense re-index in one transaction. Items not listed keep their relative
/// order after the listed ones.
pub fn reorder_queue(conn: &mut Connection, ordered: &[String]) -> Result<usize> {
    if ordered.is_empty() {
        return Err(AppError::invalid("ordered_download_ids must not be empty"));
    }
    let tx = conn.transaction()?;
    for id in ordered {
        let exists: bool =
            tx.query_row("SELECT EXISTS(SELECT 1 FROM queue_items WHERE download_id=?1)", [id], |r| r.get(0))?;
        if !exists {
            return Err(AppError::not_found("queue item", id));
        }
    }
    let rest: Vec<String> = {
        let mut st = tx.prepare("SELECT download_id FROM queue_items ORDER BY queue_position ASC")?;
        let all = st.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        all.into_iter().filter(|id| !ordered.contains(id)).collect()
    };
    let ts = now();
    for (pos, id) in ordered.iter().chain(rest.iter()).enumerate() {
        tx.execute(
            "UPDATE queue_items SET queue_position=?2, updated_at=?3 WHERE download_id=?1",
            params![id, pos as i64, ts],
        )?;
    }
    tx.commit()?;
    Ok(ordered.len())
}

pub fn get_queue_item(conn: &Connection, download_id: &str) -> Result<QueueItem> {
    conn.query_row(
        "SELECT id, download_id, priority, queue_position, created_at, updated_at FROM queue_items WHERE download_id=?1",
        [download_id],
        |r| {
            Ok(QueueItem {
                id: r.get(0)?,
                download_id: r.get(1)?,
                priority: r.get(2)?,
                queue_position: r.get::<_, i64>(3)? as u32,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| AppError::not_found("queue item", download_id))
}

// ---- settings --------------------------------------------------------------

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get(0)).optional()?)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1,?2,?3) \
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
        params![key, value, now()],
    )?;
    Ok(())
}

// ---- categories ------------------------------------------------------------

fn row_to_category(r: &Row) -> rusqlite::Result<Category> {
    Ok(Category {
        id: r.get(0)?,
        name: r.get(1)?,
        target_directory: r.get(2)?,
        file_extensions: r.get(3)?,
        is_default: r.get::<_, i64>(4)? != 0,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
    })
}

pub fn list_categories(conn: &Connection) -> Result<Vec<Category>> {
    let mut st = conn.prepare(
        "SELECT id, name, target_directory, file_extensions, is_default, created_at, updated_at \
         FROM categories ORDER BY name",
    )?;
    let rows = st.query_map([], row_to_category)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_category(conn: &Connection, id: &str) -> Result<Category> {
    conn.query_row(
        "SELECT id, name, target_directory, file_extensions, is_default, created_at, updated_at \
         FROM categories WHERE id=?1",
        [id],
        row_to_category,
    )
    .optional()?
    .ok_or_else(|| AppError::not_found("category", id))
}

pub fn insert_category(conn: &mut Connection, c: &Category) -> Result<()> {
    let tx = conn.transaction()?;
    if c.is_default {
        tx.execute("UPDATE categories SET is_default=0", [])?;
    }
    tx.execute(
        "INSERT INTO categories (id, name, target_directory, file_extensions, is_default, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![c.id, c.name, c.target_directory, c.file_extensions, c.is_default as i64, c.created_at, c.updated_at],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn update_category(conn: &mut Connection, c: &Category) -> Result<()> {
    let tx = conn.transaction()?;
    if c.is_default {
        tx.execute("UPDATE categories SET is_default=0 WHERE id<>?1", [&c.id])?;
    }
    let n = tx.execute(
        "UPDATE categories SET name=?2, target_directory=?3, file_extensions=?4, is_default=?5, updated_at=?6 \
         WHERE id=?1",
        params![c.id, c.name, c.target_directory, c.file_extensions, c.is_default as i64, now()],
    )?;
    if n == 0 {
        return Err(AppError::not_found("category", &c.id));
    }
    tx.commit()?;
    Ok(())
}

pub fn delete_category(conn: &Connection, id: &str) -> Result<()> {
    let cat = get_category(conn, id)?;
    if cat.is_default {
        return Err(AppError::invalid("the default fallback category cannot be deleted"));
    }
    conn.execute("DELETE FROM categories WHERE id=?1", [id])?;
    Ok(())
}

// ---- credentials -----------------------------------------------------------

/// Endpoint resolution (DATABASE.md 3.5): exact host+port first, then host with NULL port.
pub fn find_credential(conn: &Connection, host: &str, port: Option<u16>, protocol: &str) -> Result<Option<Credential>> {
    let map = |r: &Row| -> rusqlite::Result<Credential> {
        Ok(Credential {
            id: r.get(0)?,
            host: r.get(1)?,
            username: r.get(2)?,
            password_encrypted: r.get(3)?,
            port: r.get::<_, Option<i64>>(4)?.map(|p| p as u16),
            protocol: r.get(5)?,
            created_at: r.get(6)?,
            updated_at: r.get(7)?,
        })
    };
    let cols = "id, host, username, password_encrypted, port, protocol, created_at, updated_at";
    if let Some(p) = port {
        let hit = conn
            .query_row(&format!("SELECT {cols} FROM credentials WHERE host=?1 AND port=?2 AND protocol=?3 LIMIT 1"), params![host, p, protocol], map)
            .optional()?;
        if hit.is_some() {
            return Ok(hit);
        }
    }
    Ok(conn
        .query_row(&format!("SELECT {cols} FROM credentials WHERE host=?1 AND port IS NULL AND protocol=?2 LIMIT 1"), params![host, protocol], map)
        .optional()?)
}

/// `password_encrypted` must already be a DPAPI blob; plaintext is never accepted here.
pub fn upsert_credential(conn: &Connection, host: &str, port: Option<u16>, username: &str, password_encrypted: &str, protocol: &str) -> Result<()> {
    let ts = now();
    let existing: Option<String> = match port {
        Some(p) => conn
            .query_row("SELECT id FROM credentials WHERE host=?1 AND port=?2 AND username=?3 AND protocol=?4", params![host, p, username, protocol], |r| r.get(0))
            .optional()?,
        None => conn
            .query_row("SELECT id FROM credentials WHERE host=?1 AND port IS NULL AND username=?2 AND protocol=?3", params![host, username, protocol], |r| r.get(0))
            .optional()?,
    };
    match existing {
        Some(id) => {
            conn.execute("UPDATE credentials SET password_encrypted=?2, updated_at=?3 WHERE id=?1", params![id, password_encrypted, ts])?;
        }
        None => {
            conn.execute(
                "INSERT INTO credentials (id, host, username, password_encrypted, port, protocol, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?7)",
                params![uuid::Uuid::new_v4().to_string(), host, username, password_encrypted, port, protocol, ts],
            )?;
        }
    }
    Ok(())
}

// ---- torrents (DEC-008) ---------------------------------------------------

#[derive(Debug, Clone)]
pub struct TorrentRow {
    pub download_id: String,
    pub info_hash: String,
    pub metainfo: Option<Vec<u8>>,
    pub started: bool,
    pub uploaded_bytes: i64,
    pub seed_state: String,
    pub seed_ratio: Option<f64>,
}

fn row_to_torrent(r: &Row) -> rusqlite::Result<TorrentRow> {
    Ok(TorrentRow {
        download_id: r.get(0)?,
        info_hash: r.get(1)?,
        metainfo: r.get(2)?,
        started: r.get::<_, i64>(3)? != 0,
        uploaded_bytes: r.get(4)?,
        seed_state: r.get(5)?,
        seed_ratio: r.get(6)?,
    })
}

const TORRENT_COLS: &str = "download_id, info_hash, metainfo, started, uploaded_bytes, seed_state, seed_ratio";

pub fn insert_torrent(conn: &Connection, t: &TorrentRow) -> Result<()> {
    conn.execute(
        "INSERT INTO torrents (download_id, info_hash, metainfo, started, uploaded_bytes, seed_state, seed_ratio) \
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![t.download_id, t.info_hash, t.metainfo, t.started as i64, t.uploaded_bytes, t.seed_state, t.seed_ratio],
    )?;
    Ok(())
}

pub fn get_torrent(conn: &Connection, download_id: &str) -> Result<Option<TorrentRow>> {
    Ok(conn
        .query_row(&format!("SELECT {TORRENT_COLS} FROM torrents WHERE download_id=?1"), [download_id], row_to_torrent)
        .optional()?)
}

pub fn find_torrent_by_hash(conn: &Connection, info_hash: &str) -> Result<Option<TorrentRow>> {
    Ok(conn
        .query_row(&format!("SELECT {TORRENT_COLS} FROM torrents WHERE info_hash=?1 LIMIT 1"), [info_hash], row_to_torrent)
        .optional()?)
}

pub fn list_seeding_torrents(conn: &Connection) -> Result<Vec<TorrentRow>> {
    let mut stmt = conn.prepare(&format!("SELECT {TORRENT_COLS} FROM torrents WHERE seed_state='seeding'"))?;
    let rows = stmt.query_map([], row_to_torrent)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Metadata arrived (magnet) or was known up front (.torrent): record the real name,
/// size and on-disk location, and keep the metainfo so a resume never needs DHT again.
pub fn set_torrent_metadata(conn: &Connection, id: &str, metainfo: &[u8], file_name: &str, target_path: &str, total: i64) -> Result<()> {
    conn.execute("UPDATE torrents SET metainfo=?2 WHERE download_id=?1", params![id, metainfo])?;
    conn.execute(
        "UPDATE downloads SET file_name=?2, target_path=?3, total_bytes=?4, updated_at=?5 WHERE id=?1",
        params![id, file_name, target_path, total, now()],
    )?;
    Ok(())
}

pub fn set_torrent_started(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("UPDATE torrents SET started=1 WHERE download_id=?1", [id])?;
    Ok(())
}

pub fn set_torrent_seed(conn: &Connection, id: &str, uploaded: i64, state: &str) -> Result<()> {
    conn.execute("UPDATE torrents SET uploaded_bytes=?2, seed_state=?3 WHERE download_id=?1", params![id, uploaded, state])?;
    Ok(())
}

pub fn set_torrent_ratio(conn: &Connection, id: &str, ratio: Option<f64>) -> Result<()> {
    conn.execute("UPDATE torrents SET seed_ratio=?2 WHERE download_id=?1", params![id, ratio])?;
    Ok(())
}
