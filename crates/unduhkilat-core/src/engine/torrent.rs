//! BitTorrent transfers (DEC-008) on top of `librqbit`.
//!
//! Division of labour: librqbit owns the swarm (trackers, DHT, PEX, peer wire, piece
//! hashing, disk layout). This module owns everything the rest of the app relies on:
//! queue slot, pause/cancel semantics, database state, progress events, proxy policy,
//! the never-overwrite rule and seeding limits.
//!
//! Security / privacy policy (all deliberate, none silent):
//! * The session (and therefore any listening port) is created lazily on the first
//!   torrent, never at app start for users who do not use torrents.
//! * A manual SOCKS5 proxy is honoured for peer connections; in that case DHT, local
//!   service discovery and the listening port are switched off because they use UDP/
//!   inbound traffic that cannot be proxied. An HTTP proxy cannot carry BitTorrent, so
//!   torrents are refused (rather than leaking the real address) while one is configured.
//! * Existing files are never overwritten: the first start refuses with
//!   `ERR_NAME_COLLISION`; only a torrent this app already started may reuse its files.

use super::http::sanitize_file_name;
use super::proxy::{split_scheme, ProxyConfig, ProxyMode};
use super::throttle::TokenBucket;
use super::transfer::{Outcome, TransferCtx};
use super::EngineEvent;
use crate::config::keys;
use crate::db::Db;
use crate::error::{AppError, ErrorCode, Result};
use crate::repo;
use librqbit::api::TorrentIdOrHash;
use librqbit::limits::LimitsConfig;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ConnectionOptions, DhtSessionConfig, ListenerMode, ListenerOptions, ManagedTorrent,
    Session, SessionOptions,
};
use serde::Serialize;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// How long a magnet link may search for metadata before the run fails.
const METADATA_TIMEOUT: Duration = Duration::from_secs(300);
/// A `.torrent` fetched over HTTP is metadata, not content: cap it.
pub const MAX_TORRENT_FILE_BYTES: usize = 8 * 1024 * 1024;

type Handle = Arc<ManagedTorrent>;

pub fn is_magnet(url: &str) -> bool {
    url.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("magnet:"))
}

/// What the app needs to know before the swarm is involved at all.
#[derive(Debug, Clone)]
pub struct TorrentMeta {
    pub info_hash: String,
    /// Display/folder name; `None` for a magnet without `dn`.
    pub name: Option<String>,
    /// Known only for `.torrent` files.
    pub total_bytes: Option<i64>,
}

pub fn parse_magnet(url: &str) -> Result<TorrentMeta> {
    let m = librqbit::Magnet::parse(url).map_err(|e| AppError::invalid(format!("invalid magnet link: {e:#}")))?;
    let hash = m.as_id20().ok_or_else(|| AppError::invalid("only BitTorrent v1 (btih) magnet links are supported"))?;
    Ok(TorrentMeta { info_hash: hash.as_string(), name: m.name.map(|n| sanitize_file_name(&n)).filter(|n| !n.is_empty()), total_bytes: None })
}

pub fn parse_torrent_bytes(bytes: &[u8]) -> Result<TorrentMeta> {
    if bytes.len() > MAX_TORRENT_FILE_BYTES {
        return Err(AppError::invalid("torrent file is too large"));
    }
    let t = librqbit::torrent_from_bytes(bytes).map_err(|e| AppError::new(ErrorCode::ErrPayloadCorrupt, format!("not a valid .torrent file: {e}")))?;
    let name = t.info.data.name.as_ref().map(|n| sanitize_file_name(&String::from_utf8_lossy(n.as_ref()))).filter(|n| !n.is_empty());
    let total = match (&t.info.data.length, &t.info.data.files) {
        (Some(l), _) => Some(*l as i64),
        (None, Some(files)) => Some(files.iter().map(|f| f.length as i64).sum()),
        _ => None,
    };
    Ok(TorrentMeta { info_hash: t.info_hash.as_string(), name, total_bytes: total })
}

/// BEP 9 `x.pe=host:port` peer hints in a magnet link. Invalid entries are ignored.
pub fn peers_from_magnet(url: &str) -> Vec<std::net::SocketAddr> {
    url::Url::parse(url)
        .map(|u| u.query_pairs().filter(|(k, _)| k == "x.pe").filter_map(|(_, v)| v.parse().ok()).collect())
        .unwrap_or_default()
}

/// BitTorrent over a proxy: only SOCKS5 can carry it.
pub fn socks_url(p: &ProxyConfig) -> Result<Option<String>> {
    if p.mode != ProxyMode::Manual {
        return Ok(None);
    }
    let (scheme, host) = split_scheme(&p.host);
    if !matches!(scheme, "socks5" | "socks5h") {
        return Err(AppError::invalid("BitTorrent cannot use an HTTP proxy; switch the proxy to SOCKS5 or Direct to use torrents"));
    }
    let auth = match (&p.username, &p.password) {
        (Some(u), Some(pw)) => format!("{}:{}@", urlencode(u), urlencode(pw)),
        (Some(u), None) => format!("{}@", urlencode(u)),
        _ => String::new(),
    };
    Ok(Some(format!("socks5://{auth}{host}:{}", p.port)))
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn hex_to_hash(h: &str) -> Option<librqbit::dht::Id20> {
    if h.len() != 40 {
        return None;
    }
    let mut out = [0u8; 20];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(h.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(librqbit::dht::Id20::new(out))
}

fn bps(kbps: u64) -> Option<NonZeroU32> {
    NonZeroU32::new(u32::try_from(kbps.saturating_mul(1024)).unwrap_or(u32::MAX))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionKey {
    socks: Option<String>,
    dht: bool,
    upnp: bool,
    port: u16,
}

struct SeedEntry {
    handle: Handle,
    /// `0` = unlimited.
    ratio: f64,
    total: u64,
    /// Uploaded bytes from earlier runs; librqbit only counts this process.
    base: u64,
}

/// Live snapshot for the UI details panel.
#[derive(Debug, Clone, Serialize)]
pub struct TorrentInfo {
    pub info_hash: String,
    pub seed_state: String,
    pub seed_ratio_limit: Option<f64>,
    pub uploaded_bytes: u64,
    pub ratio: f64,
    pub live: bool,
    pub peers_connected: u32,
    pub peers_seen: u32,
    pub download_bps: u64,
    pub upload_bps: u64,
}

pub struct TorrentEngine {
    db: Db,
    session: tokio::sync::Mutex<Option<(SessionKey, Arc<Session>, CancellationToken)>>,
    seeds: Arc<Mutex<HashMap<String, SeedEntry>>>,
    monitor_running: AtomicBool,
}

fn setting(db: &Db, key: &str) -> Option<String> {
    db.read(|c| repo::get_setting(c, key)).ok().flatten()
}

fn flag(db: &Db, key: &str, default: bool) -> bool {
    match setting(db, key).as_deref() {
        Some("0") | Some("false") => false,
        Some("1") | Some("true") => true,
        _ => default,
    }
}

fn global_ratio(db: &Db) -> f64 {
    setting(db, keys::TORRENT_SEED_RATIO).and_then(|v| v.parse::<f64>().ok()).filter(|r| r.is_finite() && *r >= 0.0).unwrap_or(1.0)
}

impl TorrentEngine {
    pub fn new(db: Db) -> Self {
        TorrentEngine { db, session: tokio::sync::Mutex::new(None), seeds: Arc::new(Mutex::new(HashMap::new())), monitor_running: AtomicBool::new(false) }
    }

    fn wanted_key(&self, proxy: &ProxyConfig) -> Result<SessionKey> {
        let socks = socks_url(proxy)?;
        let port = setting(&self.db, keys::TORRENT_PORT).and_then(|v| v.parse::<u16>().ok()).unwrap_or(0);
        Ok(SessionKey {
            dht: socks.is_none() && flag(&self.db, keys::TORRENT_DHT, true),
            upnp: socks.is_none() && flag(&self.db, keys::TORRENT_UPNP, false),
            port,
            socks,
        })
    }

    /// The shared session, created on first use. A changed proxy or network setting
    /// needs a new session, which is only allowed while nothing is attached to the old
    /// one (the manager pauses torrents before calling [`reset`]).
    pub async fn session(&self, proxy: &ProxyConfig) -> Result<Arc<Session>> {
        let key = self.wanted_key(proxy)?;
        let mut guard = self.session.lock().await;
        if let Some((k, s, _)) = guard.as_ref() {
            if *k == key {
                return Ok(s.clone());
            }
            return Err(AppError::invalid("torrent network settings changed; pause active torrents and retry"));
        }
        let global = setting(&self.db, keys::GLOBAL_SPEED_LIMIT_KBPS).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let proxied = key.socks.is_some();
        let opts = SessionOptions {
            dht: key.dht.then(DhtSessionConfig::default),
            persistence: None,
            fastresume: false,
            // No inbound port when proxied: the proxy cannot accept connections for us.
            listen: (!proxied).then(|| ListenerOptions {
                mode: ListenerMode::TcpOnly,
                listen_addr: (Ipv4Addr::UNSPECIFIED, key.port).into(),
                enable_upnp_port_forwarding: key.upnp,
                ipv4_only: true,
                ..Default::default()
            }),
            connect: Some(ConnectionOptions { proxy_url: key.socks.clone(), ..Default::default() }),
            ratelimits: LimitsConfig { download_bps: bps(global), upload_bps: None },
            disable_local_service_discovery: true,
            ipv4_only: true,
            ..Default::default()
        };
        let session = Session::new_with_opts(std::env::temp_dir(), opts).await.map_err(|e| AppError::internal(format!("cannot start BitTorrent session: {e:#}")))?;
        let token = CancellationToken::new();
        *guard = Some((key, session.clone(), token.clone()));
        drop(guard);
        self.start_monitor(session.clone(), token);
        Ok(session)
    }

    /// Tear the session down (proxy/network settings changed, or app exit).
    pub async fn reset(&self) {
        let taken = self.session.lock().await.take();
        self.seeds.lock().unwrap().clear();
        if let Some((_, s, token)) = taken {
            token.cancel();
            s.stop().await;
        }
    }

    /// App exit: remember how much each seeding torrent has uploaded, then stop the session.
    pub async fn flush_and_stop(&self) {
        let entries: Vec<(String, u64)> = self.seeds.lock().unwrap().iter().map(|(id, e)| (id.clone(), e.base + e.handle.stats().uploaded_bytes)).collect();
        for (id, up) in entries {
            let _ = self.db.write(move |c| repo::set_torrent_seed(c, &id, up as i64, "seeding")).await;
        }
        self.reset().await;
    }

    pub async fn is_active(&self) -> bool {
        self.session.lock().await.is_some()
    }

    /// Global download cap in kbit-free KB/s (`0` = unlimited), applied live.
    pub async fn set_global_limit(&self, kbps: u64) {
        if let Some((_, s, _)) = self.session.lock().await.as_ref() {
            s.ratelimits.set_download_bps(bps(kbps));
        }
    }

    async fn existing(&self) -> Option<Arc<Session>> {
        self.session.lock().await.as_ref().map(|(_, s, _)| s.clone())
    }

    fn handle_for(session: &Session, hash: &str) -> Option<Handle> {
        session.get(TorrentIdOrHash::Hash(hex_to_hash(hash)?))
    }

    // ---- seeding -----------------------------------------------------------

    fn start_monitor(&self, session: Arc<Session>, token: CancellationToken) {
        if self.monitor_running.swap(true, Ordering::SeqCst) {
            // A previous session's monitor exits on its token; start a fresh one.
        }
        let (seeds, db) = (self.seeds.clone(), self.db.clone());
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            let mut n: u32 = 0;
            loop {
                tokio::select! {
                    _ = token.cancelled() => return,
                    _ = tick.tick() => {}
                }
                n = n.wrapping_add(1);
                let seeding_on = flag(&db, keys::TORRENT_SEEDING, true);
                let entries: Vec<(String, Handle, f64, u64, u64)> =
                    seeds.lock().unwrap().iter().map(|(id, e)| (id.clone(), e.handle.clone(), e.ratio, e.total, e.base)).collect();
                for (id, handle, ratio, total, base) in entries {
                    let uploaded = base + handle.stats().uploaded_bytes;
                    let reached = ratio > 0.0 && total > 0 && uploaded as f64 / total as f64 >= ratio;
                    if reached || !seeding_on {
                        let _ = session.pause(&handle).await;
                        seeds.lock().unwrap().remove(&id);
                        let (id2, up) = (id.clone(), uploaded as i64);
                        let _ = db.write(move |c| repo::set_torrent_seed(c, &id2, up, "done")).await;
                        tracing::info!(download = %id, ratio = uploaded as f64 / total.max(1) as f64, "seeding finished");
                    } else if n % 15 == 0 {
                        let (id2, up) = (id.clone(), uploaded as i64);
                        let _ = db.write(move |c| repo::set_torrent_seed(c, &id2, up, "seeding")).await;
                    }
                }
            }
        });
    }

    fn register_seed(&self, id: &str, handle: Handle, row: &repo::TorrentRow, total: u64) {
        let ratio = row.seed_ratio.unwrap_or_else(|| global_ratio(&self.db));
        self.seeds.lock().unwrap().insert(id.to_string(), SeedEntry { handle, ratio, total, base: row.uploaded_bytes.max(0) as u64 });
    }

    /// Stop seeding a finished torrent (it stays `completed`).
    pub async fn stop_seeding(&self, id: &str) -> Result<()> {
        let entry = self.seeds.lock().unwrap().remove(id);
        if let (Some(e), Some(s)) = (entry, self.existing().await) {
            let uploaded = e.base + e.handle.stats().uploaded_bytes;
            let _ = s.pause(&e.handle).await;
            let (id2, up) = (id.to_string(), uploaded as i64);
            self.db.write(move |c| repo::set_torrent_seed(c, &id2, up, "done")).await?;
        } else {
            let id2 = id.to_string();
            self.db.write(move |c| repo::set_torrent_ratio(c, &id2, None).and_then(|_| c.execute("UPDATE torrents SET seed_state='done' WHERE download_id=?1 AND seed_state='seeding'", [&id2]).map(|_| ()).map_err(Into::into))).await?;
        }
        Ok(())
    }

    /// Start (or resume) seeding a finished torrent whose files are still on disk.
    pub async fn start_seeding(&self, proxy: &ProxyConfig, id: &str) -> Result<()> {
        let dl = self.db.read(|c| repo::get_download(c, id))?;
        let row = self.db.read(|c| repo::get_torrent(c, id))?.ok_or_else(|| AppError::not_found("torrent", id))?;
        let meta = row.metainfo.clone().ok_or_else(|| AppError::new(ErrorCode::ErrFileNotFound, "torrent metadata is missing"))?;
        if !PathBuf::from(&dl.target_path).exists() {
            return Err(AppError::new(ErrorCode::ErrFileNotFound, format!("{} is missing; nothing to seed", dl.target_path)));
        }
        let session = self.session(proxy).await?;
        let out = PathBuf::from(&dl.target_path).parent().map(|p| p.to_string_lossy().into_owned());
        let opts = AddTorrentOptions { overwrite: true, output_folder: out, ..Default::default() };
        let resp = session.add_torrent(AddTorrent::from_bytes(meta), Some(opts)).await.map_err(|e| AppError::internal(format!("{e:#}")))?;
        let handle = resp.into_handle().ok_or_else(|| AppError::internal("torrent was not added"))?;
        if handle.is_paused() {
            session.unpause(&handle).await.map_err(|e| AppError::internal(format!("{e:#}")))?;
        }
        let total = dl.total_bytes.max(0) as u64;
        self.register_seed(id, handle, &row, total);
        let id2 = id.to_string();
        let up = row.uploaded_bytes;
        self.db.write(move |c| repo::set_torrent_seed(c, &id2, up, "seeding")).await?;
        Ok(())
    }

    /// Per-torrent share-ratio target (`None` = global default, `0` = unlimited).
    pub async fn set_seed_ratio(&self, id: &str, ratio: Option<f64>) -> Result<()> {
        if ratio.is_some_and(|r| !r.is_finite() || r < 0.0) {
            return Err(AppError::invalid("seed ratio must be a non-negative number"));
        }
        let id2 = id.to_string();
        self.db.write(move |c| repo::set_torrent_ratio(c, &id2, ratio)).await?;
        if let Some(e) = self.seeds.lock().unwrap().get_mut(id) {
            e.ratio = ratio.unwrap_or_else(|| global_ratio(&self.db));
        }
        Ok(())
    }

    // ---- info / removal ----------------------------------------------------

    pub async fn info(&self, id: &str) -> Result<Option<TorrentInfo>> {
        let Some(row) = self.db.read(|c| repo::get_torrent(c, id))? else { return Ok(None) };
        let total = self.db.read(|c| repo::get_download(c, id))?.total_bytes.max(0) as f64;
        let mut info = TorrentInfo {
            info_hash: row.info_hash.clone(),
            seed_state: row.seed_state.clone(),
            seed_ratio_limit: row.seed_ratio,
            uploaded_bytes: row.uploaded_bytes.max(0) as u64,
            ratio: 0.0,
            live: false,
            peers_connected: 0,
            peers_seen: 0,
            download_bps: 0,
            upload_bps: 0,
        };
        if let Some(h) = self.existing().await.and_then(|s| Self::handle_for(&s, &row.info_hash)) {
            let st = h.stats();
            info.uploaded_bytes = row.uploaded_bytes.max(0) as u64 + st.uploaded_bytes;
            if let Some(l) = st.live.as_ref() {
                info.live = true;
                info.peers_connected = l.snapshot.peer_stats.live as u32;
                info.peers_seen = l.snapshot.peer_stats.seen as u32;
                info.download_bps = l.download_speed.as_bytes();
                info.upload_bps = l.upload_speed.as_bytes();
            }
        }
        if total > 0.0 {
            info.ratio = info.uploaded_bytes as f64 / total;
        }
        Ok(Some(info))
    }

    /// Detach a torrent from the session, optionally deleting only the files it owns.
    pub async fn forget(&self, id: &str, delete_files: bool) -> Result<()> {
        self.seeds.lock().unwrap().remove(id);
        let Some(row) = self.db.read(|c| repo::get_torrent(c, id))? else { return Ok(()) };
        if let Some(s) = self.existing().await {
            if let Some(h) = Self::handle_for(&s, &row.info_hash) {
                let _ = s.delete(TorrentIdOrHash::Id(h.id()), delete_files).await;
            }
        }
        Ok(())
    }

    /// Re-seed everything that was still seeding when the app last closed.
    pub async fn restore_seeding(&self, proxy: &ProxyConfig) {
        if !flag(&self.db, keys::TORRENT_SEEDING, true) {
            return;
        }
        let rows = self.db.read(repo::list_seeding_torrents).unwrap_or_default();
        for r in rows {
            if let Err(e) = self.start_seeding(proxy, &r.download_id).await {
                tracing::warn!(download = %r.download_id, error = %e, "could not resume seeding");
                let id = r.download_id.clone();
                let _ = self.db.write(move |c| repo::set_torrent_seed(c, &id, r.uploaded_bytes, "done")).await;
            }
        }
    }
}

fn add_error(e: anyhow_like::Error) -> AppError {
    let msg = format!("{e:#}");
    let lower = msg.to_lowercase();
    if lower.contains("already exist") || lower.contains("overwrite") {
        AppError::new(ErrorCode::ErrNameCollision, format!("torrent files already exist on disk: {msg}"))
    } else {
        AppError::internal(format!("BitTorrent error: {msg}"))
    }
}

/// librqbit reports `anyhow::Error`; alias it without taking a direct dependency.
mod anyhow_like {
    pub type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
}

/// One torrent transfer run (called from `run_transfer` for magnet downloads).
pub async fn run_torrent(ctx: &TransferCtx, id: &str, cancel: &CancellationToken, _bucket: Arc<TokenBucket>) -> Result<Outcome> {
    let dl = ctx.db.read(|c| repo::get_download(c, id))?;
    let mut row = ctx.db.read(|c| repo::get_torrent(c, id))?.ok_or_else(|| AppError::not_found("torrent", id))?;
    let proxy = ctx.proxy();
    let engine = &ctx.torrent;
    let session = engine.session(&proxy).await?;
    let out_dir = PathBuf::from(&dl.target_path)
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| AppError::new(ErrorCode::ErrInvalidPath, "torrent has no output directory"))?;
    std::fs::create_dir_all(&out_dir).map_err(|e| AppError::new(ErrorCode::ErrPathUnwritable, e.to_string()))?;
    let out_s = out_dir.to_string_lossy().into_owned();

    let peers = peers_from_magnet(&dl.original_url);

    // 1. Metadata. A magnet resolves it from the swarm via a list-only add (nothing is
    //    written to disk), so the real name is known before any file exists.
    if row.metainfo.is_none() {
        let probe = AddTorrentOptions {
            list_only: true,
            output_folder: Some(out_s.clone()),
            overwrite: false,
            initial_peers: (!peers.is_empty()).then(|| peers.clone()),
            ..Default::default()
        };
        let fut = session.add_torrent(AddTorrent::from_url(dl.original_url.clone()), Some(probe));
        let resp = tokio::select! {
            _ = cancel.cancelled() => return Ok(Outcome::Stopped),
            r = tokio::time::timeout(METADATA_TIMEOUT, fut) => match r {
                Err(_) => return Err(AppError::new(ErrorCode::ErrConnectionDropped, "no peer provided the torrent metadata in time")),
                Ok(Err(e)) => return Err(add_error(e.into())),
                Ok(Ok(v)) => v,
            }
        };
        let AddTorrentResponse::ListOnly(lo) = resp else { return Err(AppError::internal("unexpected metadata response")) };
        let name = lo.info.name().map(|n| sanitize_file_name(&n)).filter(|n| !n.is_empty()).unwrap_or_else(|| row.info_hash.clone());
        let total = lo.info.lengths().total_length() as i64;
        let target = out_dir.join(&name).to_string_lossy().into_owned();
        let bytes = lo.torrent_bytes.to_vec();
        let (id2, b2, n2) = (id.to_string(), bytes.clone(), name);
        ctx.db.write(move |c| repo::set_torrent_metadata(c, &id2, &b2, &n2, &target, total)).await?;
        row.metainfo = Some(bytes);
    }
    let dl = ctx.db.read(|c| repo::get_download(c, id))?;

    // 2. Never overwrite: a torrent this app has not started yet must find a free path.
    if !row.started && PathBuf::from(&dl.target_path).exists() {
        return Err(AppError::new(ErrorCode::ErrNameCollision, format!("{} already exists", dl.target_path)));
    }

    // 3. Real add (from stored metadata, so no swarm lookup is needed again).
    let opts = AddTorrentOptions {
        overwrite: row.started,
        output_folder: Some(out_s),
        ratelimits: LimitsConfig { download_bps: bps(dl.speed_limit_kbps as u64), upload_bps: None },
        initial_peers: (!peers.is_empty()).then(|| peers.clone()),
        ..Default::default()
    };
    let meta = row.metainfo.clone().expect("metadata set above");
    let resp = session.add_torrent(AddTorrent::from_bytes(meta), Some(opts)).await.map_err(|e| add_error(e.into()))?;
    let handle = resp.into_handle().ok_or_else(|| AppError::internal("torrent was not added"))?;
    if handle.is_paused() {
        session.unpause(&handle).await.map_err(|e| AppError::internal(format!("{e:#}")))?;
    }
    engine.seeds.lock().unwrap().remove(id); // finished earlier, being downloaded again
    let id2 = id.to_string();
    ctx.db.write(move |c| repo::set_torrent_started(c, &id2).and_then(|_| repo::set_torrent_seed(c, &id2, row.uploaded_bytes, "none"))).await?;

    // 4. Watch it.
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut last_ckpt = Instant::now();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                let uploaded = row.uploaded_bytes.max(0) as u64 + handle.stats().uploaded_bytes;
                let _ = session.pause(&handle).await;
                let (id2, up) = (id.to_string(), uploaded as i64);
                let _ = ctx.db.write(move |c| repo::set_torrent_seed(c, &id2, up, "none")).await;
                return Ok(Outcome::Stopped);
            }
            _ = tick.tick() => {}
        }
        let st = handle.stats();
        if let Some(err) = &st.error {
            return Err(AppError::internal(format!("BitTorrent error: {err}")));
        }
        let (speed, conns) = st.live.as_ref().map(|l| (l.download_speed.as_bytes(), l.snapshot.peer_stats.live as u32)).unwrap_or((0, 0));
        let total = st.total_bytes as i64;
        let done = st.progress_bytes as i64;
        let _ = ctx.events.send(EngineEvent::Progress { id: id.to_string(), downloaded_bytes: done, total_bytes: total, current_speed_bps: speed, active_connections: conns });
        if st.finished {
            let (id2, d) = (id.to_string(), total);
            ctx.db.write(move |c| repo::set_downloaded_bytes(c, &id2, d)).await?;
            let uploaded = row.uploaded_bytes.max(0) as u64 + st.uploaded_bytes;
            let ratio = row.seed_ratio.unwrap_or_else(|| global_ratio(&ctx.db));
            let want = flag(&ctx.db, keys::TORRENT_SEEDING, true) && (ratio == 0.0 || (uploaded as f64) < ratio * total.max(1) as f64);
            let (id2, up) = (id.to_string(), uploaded as i64);
            if want {
                engine.register_seed(id, handle.clone(), &row, total.max(0) as u64);
                ctx.db.write(move |c| repo::set_torrent_seed(c, &id2, up, "seeding")).await?;
            } else {
                let _ = session.pause(&handle).await;
                ctx.db.write(move |c| repo::set_torrent_seed(c, &id2, up, "done")).await?;
            }
            return Ok(Outcome::Completed);
        }
        if last_ckpt.elapsed() >= ctx.cfg.checkpoint_interval {
            last_ckpt = Instant::now();
            let (id2, d) = (id.to_string(), done);
            let _ = ctx.db.write(move |c| repo::set_downloaded_bytes(c, &id2, d)).await;
            if dl.total_bytes != total && total > 0 {
                let id3 = id.to_string();
                let _ = ctx.db.write(move |c| c.execute("UPDATE downloads SET total_bytes=?2 WHERE id=?1", rusqlite::params![id3, total]).map(|_| ()).map_err(Into::into)).await;
            }
        }
    }
}
