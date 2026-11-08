//! Queue manager: dispatch (priority DESC, FIFO), lifecycle commands, crash recovery.

use super::proxy::ProxyConfig;
use super::throttle::{Throttler, TokenBucket};
use super::transfer::{run_transfer, Outcome, TransferCtx};
use super::{grabber, http, search, torrent, EngineEvent};
use crate::category;
use crate::config::{keys, EngineConfig};
use crate::db::Db;
use crate::error::{AppError, ErrorCode, Result};
use crate::models::{Download, DownloadState};
use crate::repo;
use futures_util::future::BoxFuture;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopIntent {
    /// Pause (also used for app exit and stop-all).
    Pause,
    Cancel { delete_file: bool },
    Restart,
}

struct RunningEntry {
    cancel: CancellationToken,
    intent: Arc<Mutex<StopIntent>>,
    bucket: Arc<TokenBucket>,
}

#[derive(Debug, Clone, Default)]
pub struct NewDownload {
    pub url: String,
    pub file_name: Option<String>,
    /// Full destination path. Overrides category routing (FR-032).
    pub target_path: Option<PathBuf>,
    pub category_id: Option<String>,
    pub connection_count: Option<u32>,
    pub speed_limit_kbps: u32,
    pub priority: i32,
    pub referer: Option<String>,
    pub user_agent: Option<String>,
    pub cookies: Option<String>,
    /// `false` creates the item `paused` (e.g. scheduled for later).
    pub start_immediately: bool,
}

#[derive(Debug, Default)]
pub struct BatchOutcome {
    pub created: Vec<String>,
    pub failed: Vec<(String, String)>,
}

struct Inner {
    ctx: Arc<TransferCtx>,
    running: Mutex<HashMap<String, RunningEntry>>,
    dispatch_gate: tokio::sync::Mutex<()>,
}

#[derive(Clone)]
pub struct Manager {
    inner: Arc<Inner>,
}

/// What queuing a whole folder produced.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FolderQueued {
    pub queued: usize,
    /// The queue items created, so a caller can follow their progress.
    pub ids: Vec<String>,
    pub failed: Vec<String>,
}

impl Manager {
    /// Build the manager. Crash normalization (FR-010) commits BEFORE this returns,
    /// i.e. before any worker can be scheduled.
    pub async fn start(db: Db, cfg: EngineConfig) -> Result<Manager> {
        let normalized = db.write(repo::normalize_startup).await?;
        if normalized > 0 {
            tracing::warn!(count = normalized, "recovered interrupted downloads as paused");
        }
        let global = db.read(|c| repo::get_setting(c, keys::GLOBAL_SPEED_LIMIT_KBPS))?.and_then(|v| v.parse().ok()).unwrap_or(0);
        // A broken stored proxy (e.g. a DPAPI blob from another Windows user) must not stop
        // the app from starting: fall back to the default and say so.
        let proxy = db.read(ProxyConfig::load).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "stored proxy settings are unreadable; using system proxy");
            ProxyConfig::default()
        });
        let client = http::build_client(&cfg, &proxy).or_else(|e| {
            tracing::warn!(error = %e, "stored proxy settings are invalid; using system proxy");
            http::build_client(&cfg, &ProxyConfig::default())
        })?;
        let (events, _) = broadcast::channel(1024);
        let db2 = db.clone();
        let ctx = Arc::new(TransferCtx { db, client: RwLock::new(client), proxy: RwLock::new(proxy), throttle: Arc::new(Throttler::new(global)), cfg, events, torrent: super::torrent::TorrentEngine::new(db2) });
        Ok(Manager { inner: Arc::new(Inner { ctx, running: Mutex::new(HashMap::new()), dispatch_gate: tokio::sync::Mutex::new(()) }) })
    }

    pub fn db(&self) -> &Db {
        &self.inner.ctx.db
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.inner.ctx.events.subscribe()
    }

    /// The HTTP client every probe and download uses, so the proxy applies to the Add URL
    /// dialog's probe and to browser-initiated requests too (FR-015).
    pub fn client(&self) -> reqwest::Client {
        self.inner.ctx.client()
    }

    pub fn proxy_config(&self) -> ProxyConfig {
        self.inner.ctx.proxy()
    }

    /// Saved servers (SSH/SFTP/FTP/FTPS). Connections follow the proxy settings in force when they start.
    pub fn servers(&self) -> crate::server::service::ServerService {
        let m = self.clone();
        crate::server::service::ServerService::new(self.db().clone(), std::sync::Arc::new(move || m.proxy_config()))
    }

    /// Persist and apply new proxy settings. Transfers started afterwards use them;
    /// ones already running keep their connections.
    pub async fn set_proxy(&self, cfg: ProxyConfig) -> Result<()> {
        cfg.validate()?;
        let new_client = http::build_client(&self.inner.ctx.cfg, &cfg)?;
        let c2 = cfg.clone();
        self.db().write(move |conn| c2.save(conn)).await?;
        *self.inner.ctx.client.write().unwrap() = new_client;
        *self.inner.ctx.proxy.write().unwrap() = cfg;
        self.reset_torrents().await;
        Ok(())
    }

    /// Store a host credential (FR-014). The password is DPAPI-encrypted before it is written.
    pub async fn save_credential(&self, host: &str, port: Option<u16>, username: &str, password: &str, protocol: &str) -> Result<()> {
        if !matches!(protocol, "http" | "https" | "ftp") {
            return Err(AppError::invalid("protocol must be http, https or ftp"));
        }
        if host.trim().is_empty() || username.is_empty() {
            return Err(AppError::invalid("host and username are required"));
        }
        let enc = crate::dpapi::dpapi_encrypt(password)?;
        let (h, u, p) = (host.trim().to_string(), username.to_string(), protocol.to_string());
        self.db().write(move |c| repo::upsert_credential(c, &h, port, &u, &enc, &p)).await
    }

    // ---- File search (DEC-009) -----------------------------------------------

    /// Which providers the user has ticked (settings; the legal catalogues default on).
    pub fn search_providers(&self) -> Vec<search::Provider> {
        search::ALL_PROVIDERS
            .into_iter()
            .filter(|p| match self.db().read(|c| repo::get_setting(c, p.setting_key())).ok().flatten().as_deref() {
                Some("1") => true,
                Some(_) => false,
                None => p.default_on(),
            })
            .collect()
    }

    pub async fn set_search_provider(&self, p: search::Provider, on: bool) -> Result<()> {
        self.db().write(move |c| repo::set_setting(c, p.setting_key(), if on { "1" } else { "0" })).await
    }

    /// Runs only on an explicit Search click; nothing is sent otherwise.
    pub async fn search_files(&self, query: &str, providers: &[search::Provider]) -> Result<search::SearchOutcome> {
        self.search_files_with(query, providers, &search::Endpoints::default()).await
    }

    pub async fn search_files_with(&self, query: &str, providers: &[search::Provider], ep: &search::Endpoints) -> Result<search::SearchOutcome> {
        search::search(&self.client(), providers, query, ep).await
    }

    /// Queue the hits the user ticked, as ordinary items (never started by the search itself).
    pub async fn enqueue_hits(&self, hits: Vec<search::Hit>, priority: i32) -> BatchOutcome {
        let mut out = BatchOutcome::default();
        for h in hits {
            let req = NewDownload { priority, start_immediately: true, ..Default::default() };
            let (label, res) = match &h.source {
                search::Source::Magnet(m) => (h.title.clone(), self.create(NewDownload { url: m.clone(), ..req }).await),
                search::Source::TorrentUrl(u) => (h.title.clone(), self.create_torrent_url(u, req).await),
            };
            match res {
                Ok(d) => out.created.push(d.id),
                Err(e) => out.failed.push((label, format!("{}: {}", e.code, e.message))),
            }
        }
        out
    }

    // ---- Site Grabber (FR-045, FR-047) ---------------------------------------

    /// Scan one page and preview the links matching `pattern`. Starts nothing.
    pub async fn scan_links(&self, page_url: &str, pattern: &str) -> Result<grabber::ScanResult> {
        grabber::scan_page(&self.client(), page_url, pattern, true).await
    }

    /// Enqueue the links the user ticked. Each becomes an ordinary queue item, so the
    /// concurrency limit, priority/FIFO dispatch, category routing and speed limits all
    /// apply (FR-047). Failures (e.g. a name collision) are reported per link and do not
    /// stop the rest.
    pub async fn enqueue_batch(&self, urls: Vec<String>, priority: i32, category_id: Option<String>) -> BatchOutcome {
        let mut out = BatchOutcome::default();
        for url in urls {
            let req = NewDownload { url: url.clone(), priority, category_id: category_id.clone(), start_immediately: true, ..Default::default() };
            match self.create(req).await {
                Ok(d) => out.created.push(d.id),
                Err(e) => out.failed.push((url, format!("{}: {}", e.code, e.message))),
            }
        }
        out
    }

    // ---- BitTorrent (DEC-008) ------------------------------------------------

    fn is_torrent(&self, id: &str) -> bool {
        self.get(id).map(|d| torrent::is_magnet(&d.original_url)).unwrap_or(false)
    }

    /// A proxy or network change must not leave a torrent session running with the old
    /// (possibly direct) route: pause torrent transfers, then drop the session.
    /// Paused torrents are resumed by the user, which builds a session with the new route.
    pub async fn reset_torrents(&self) {
        let ids: Vec<String> = self.inner.running.lock().unwrap().keys().cloned().collect();
        for id in ids.into_iter().filter(|i| self.is_torrent(i)) {
            self.signal(&id, StopIntent::Pause);
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.inner.running.lock().unwrap().contains_key(&id) && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        }
        self.inner.ctx.torrent.reset().await;
    }

    pub async fn torrent_info(&self, id: &str) -> Result<Option<torrent::TorrentInfo>> {
        self.inner.ctx.torrent.info(id).await
    }

    pub async fn torrent_start_seeding(&self, id: &str) -> Result<()> {
        if !self.is_torrent(id) {
            return Err(AppError::invalid("not a torrent"));
        }
        let proxy = self.inner.ctx.proxy();
        self.inner.ctx.torrent.start_seeding(&proxy, id).await
    }

    pub async fn torrent_stop_seeding(&self, id: &str) -> Result<()> {
        self.inner.ctx.torrent.stop_seeding(id).await
    }

    pub async fn torrent_set_seed_ratio(&self, id: &str, ratio: Option<f64>) -> Result<()> {
        self.inner.ctx.torrent.set_seed_ratio(id, ratio).await
    }

    /// Call once at app start: torrents that were still seeding when the app closed.
    pub async fn restore_seeding(&self) {
        let proxy = self.inner.ctx.proxy();
        self.inner.ctx.torrent.restore_seeding(&proxy).await;
    }

    pub fn running_count(&self) -> usize {
        self.inner.running.lock().unwrap().len()
    }

    fn concurrency_limit(&self) -> usize {
        let cfg = &self.inner.ctx.cfg;
        self.db()
            .read(|c| repo::get_setting(c, keys::CONCURRENCY_LIMIT))
            .ok()
            .flatten()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n >= 1) // DEC-001: only >= 1 is required; no ceiling.
            .unwrap_or(cfg.default_concurrency)
    }

    /// FR-001: any positive integer is honoured. Takes effect at the next dispatch.
    pub async fn set_concurrency_limit(&self, n: usize) -> Result<()> {
        if n < 1 {
            return Err(AppError::invalid("concurrency_limit must be >= 1"));
        }
        self.db().write(move |c| repo::set_setting(c, keys::CONCURRENCY_LIMIT, &n.to_string())).await?;
        self.dispatch().await
    }

    pub async fn set_global_speed_limit(&self, kbps: u64) -> Result<()> {
        self.db().write(move |c| repo::set_setting(c, keys::GLOBAL_SPEED_LIMIT_KBPS, &kbps.to_string())).await?;
        self.inner.ctx.throttle.global.set_rate_kbps(kbps);
        self.inner.ctx.torrent.set_global_limit(kbps).await;
        Ok(())
    }

    pub async fn set_download_speed_limit(&self, id: &str, kbps: u32) -> Result<()> {
        let id2 = id.to_string();
        self.db().write(move |c| repo::set_speed_limit(c, &id2, kbps)).await?;
        if let Some(e) = self.inner.running.lock().unwrap().get(id) {
            e.bucket.set_rate_kbps(kbps as u64);
        }
        Ok(())
    }

    // ---- create ------------------------------------------------------------

    pub async fn create(&self, req: NewDownload) -> Result<Download> {
        if torrent::is_magnet(&req.url) {
            let meta = torrent::parse_magnet(&req.url)?;
            return self.create_inner(req, Some((meta, None))).await;
        }
        self.create_inner(req, None).await
    }

    /// DEC-008: add a torrent from the bytes of a `.torrent` file.
    pub async fn create_torrent_file(&self, bytes: Vec<u8>, mut req: NewDownload) -> Result<Download> {
        let meta = torrent::parse_torrent_bytes(&bytes)?;
        let dn = meta.name.as_deref().map(|n| url::form_urlencoded::byte_serialize(n.as_bytes()).collect::<String>());
        // Keep BEP 9 peer hints if the caller passed a magnet alongside the file.
        let hints: String = torrent::peers_from_magnet(&req.url).iter().map(|p| format!("&x.pe={p}")).collect();
        req.url = match dn {
            Some(n) => format!("magnet:?xt=urn:btih:{}&dn={n}{hints}", meta.info_hash),
            None => format!("magnet:?xt=urn:btih:{}{hints}", meta.info_hash),
        };
        self.create_inner(req, Some((meta, Some(bytes)))).await
    }

    /// Fetch a `.torrent` over HTTP(S) through the configured proxy and add it.
    pub async fn create_torrent_url(&self, url: &str, req: NewDownload) -> Result<Download> {
        let parsed = url::Url::parse(url).map_err(|_| AppError::new(ErrorCode::ErrInvalidUrlScheme, format!("not a valid URL: {url}")))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(AppError::new(ErrorCode::ErrInvalidUrlScheme, "a .torrent can only be fetched over http or https"));
        }
        let resp = self.client().get(url).send().await.map_err(|e| AppError::new(ErrorCode::ErrConnectionDropped, e.to_string()))?;
        if !resp.status().is_success() {
            return Err(AppError::new(ErrorCode::ErrConnectionDropped, format!("server answered {}", resp.status())));
        }
        if resp.content_length().is_some_and(|l| l as usize > torrent::MAX_TORRENT_FILE_BYTES) {
            return Err(AppError::invalid("torrent file is too large"));
        }
        let mut body = Vec::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
            body.extend_from_slice(&chunk.map_err(|e| AppError::new(ErrorCode::ErrConnectionDropped, e.to_string()))?);
            if body.len() > torrent::MAX_TORRENT_FILE_BYTES {
                return Err(AppError::invalid("torrent file is too large"));
            }
        }
        self.create_torrent_file(body, req).await
    }

    async fn create_inner(&self, mut req: NewDownload, torrent: Option<(torrent::TorrentMeta, Option<Vec<u8>>)>) -> Result<Download> {
        if torrent.is_none() {
            // A Drive share link is a preview page; download the file behind it.
            if let Some(direct) = http::normalize_drive_url(&req.url)? {
                req.url = direct;
            }
        }
        if let Some((m, _)) = &torrent {
            // Report "already in the list" before any file-name check can mask it.
            let hash = m.info_hash.clone();
            let dup = self.db().read(move |c| {
                Ok(match repo::find_torrent_by_hash(c, &hash)? {
                    Some(o) => c.query_row("SELECT state FROM downloads WHERE id=?1", [&o.download_id], |r| r.get::<_, String>(0))? != "cancelled",
                    None => false,
                })
            })?;
            if dup {
                return Err(AppError::new(ErrorCode::ErrAlreadyExists, "this torrent is already in the list"));
            }
        }
        if torrent.is_none() {
            let parsed = url::Url::parse(&req.url).map_err(|_| AppError::new(ErrorCode::ErrInvalidUrlScheme, format!("not a valid URL: {}", req.url)))?;
            if !matches!(parsed.scheme(), "http" | "https" | "ftp") {
                return Err(AppError::new(ErrorCode::ErrInvalidUrlScheme, format!("only http, https, ftp and magnet links are supported, got {}", parsed.scheme())));
            }
        }
        let conns = match req.connection_count {
            Some(0) => return Err(AppError::invalid("connection_count must be >= 1")),
            Some(n) => n, // DEC-001: no ceiling
            None => self
                .db()
                .read(|c| repo::get_setting(c, keys::DEFAULT_CONNECTIONS))?
                .and_then(|v| v.parse::<u32>().ok())
                .filter(|n| *n >= 1)
                .unwrap_or(self.inner.ctx.cfg.default_connections),
        };
        let file_name = req
            .file_name
            .as_deref()
            .map(http::sanitize_file_name)
            .filter(|n| !n.is_empty())
            .or_else(|| match &torrent {
                Some((m, _)) => Some(m.name.clone().unwrap_or_else(|| m.info_hash.clone())),
                None => http::file_name_from_url(&req.url),
            })
            .unwrap_or_else(|| "download".to_string());

        // Category routing (FR-029/030/032).
        let cats = self.db().read(repo::list_categories)?;
        let category = match &req.category_id {
            Some(id) => Some(cats.iter().find(|c| &c.id == id).cloned().ok_or_else(|| AppError::not_found("category", id))?),
            None => category::classify(&file_name, &cats).cloned().or_else(|| torrent.as_ref().and_then(|_| cats.iter().find(|c| c.is_default).cloned())),
        };
        let target = match (&req.target_path, &category) {
            (Some(p), _) => p.clone(),
            (None, Some(c)) => PathBuf::from(&c.target_directory).join(&file_name),
            (None, None) => return Err(AppError::new(ErrorCode::ErrInvalidPath, "no target_path and no category to derive one from")),
        };
        if !target.is_absolute() {
            return Err(AppError::new(ErrorCode::ErrInvalidPath, format!("{} is not an absolute path", target.display())));
        }
        // FR-016: never silently overwrite. The auto-rename-vs-prompt policy is TBD-010,
        // so the conservative behaviour is to refuse (ERR_NAME_COLLISION) and let the caller decide.
        if target.exists() {
            return Err(AppError::new(ErrorCode::ErrNameCollision, format!("{} already exists", target.display())));
        }
        let target_str = target.to_string_lossy().into_owned();

        let ts = repo::now();
        let dl = Download {
            id: uuid::Uuid::new_v4().to_string(),
            original_url: req.url.clone(),
            effective_url: req.url.clone(),
            target_path: target_str.clone(),
            file_name: target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(file_name),
            total_bytes: torrent.as_ref().and_then(|(m, _)| m.total_bytes).unwrap_or(-1),
            downloaded_bytes: 0,
            state: if req.start_immediately { DownloadState::Queued } else { DownloadState::Paused },
            segmentation_supported: true,
            connection_count: conns,
            speed_limit_kbps: req.speed_limit_kbps,
            category_id: category.map(|c| c.id),
            mime_type: None,
            referer: req.referer,
            user_agent: req.user_agent,
            cookies: req.cookies,
            error_code: None,
            error_message: None,
            created_at: ts.clone(),
            updated_at: ts,
            completed_at: None,
        };
        let (d2, prio) = (dl.clone(), req.priority);
        let trow = torrent.map(|(m, bytes)| repo::TorrentRow {
            download_id: dl.id.clone(),
            info_hash: m.info_hash,
            metainfo: bytes,
            started: false,
            uploaded_bytes: 0,
            seed_state: "none".into(),
            seed_ratio: None,
        });
        self.db()
            .write(move |c| {
                if let Some(t) = &trow {
                    if let Some(other) = repo::find_torrent_by_hash(c, &t.info_hash)? {
                        let st: String = c.query_row("SELECT state FROM downloads WHERE id=?1", [&other.download_id], |r| r.get(0))?;
                        if st != "cancelled" {
                            return Err(AppError::new(ErrorCode::ErrAlreadyExists, "this torrent is already in the list"));
                        }
                    }
                }
                let clash: bool = c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM downloads WHERE target_path=?1 AND state IN ('queued','running','paused'))",
                    [&d2.target_path],
                    |r| r.get(0),
                )?;
                if clash {
                    return Err(AppError::new(ErrorCode::ErrNameCollision, format!("{} is the target of another active download", d2.target_path)));
                }
                let tx = c.transaction()?;
                repo::insert_download(&tx, &d2)?;
                if let Some(t) = &trow {
                    repo::insert_torrent(&tx, t)?;
                }
                repo::enqueue(&tx, &d2.id, prio)?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        self.emit_state(&dl.id, dl.state, None);
        self.dispatch().await?;
        self.get(&dl.id)
    }

    // ---- transfers with saved servers ----------------------------------------

    /// Queue a download of `remote_path` from a saved server into `local_dir`. The local name is the
    /// remote name made safe. An existing local file is refused unless `overwrite` (the caller asked
    /// the user first).
    pub async fn enqueue_remote_download(&self, server_id: &str, remote_path: &str, local_dir: &std::path::Path, overwrite: bool) -> Result<Download> {
        let remote_name = remote_path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        let name = crate::server::fs::safe_local_name(remote_name).ok_or_else(|| AppError::new(ErrorCode::ErrInvalidPath, "Nama berkas dari server kosong atau tidak aman untuk dibuat di komputer ini."))?;
        let target = local_dir.join(&name);
        if !target.is_absolute() {
            return Err(AppError::new(ErrorCode::ErrInvalidPath, format!("{} bukan jalur absolut.", target.display())));
        }
        if target.exists() && !overwrite {
            return Err(AppError::new(ErrorCode::ErrNameCollision, format!("{} sudah ada.", target.display())));
        }
        // With a confirmed overwrite the old file stays until this download actually starts.
        let url = super::remote::make_url(super::remote::Direction::Download, server_id, remote_path, overwrite);
        self.insert_remote(url, target, name, true).await
    }

    /// Queue an upload of `local_file` into `remote_dir` on a saved server. Replacing an existing
    /// server file needs `overwrite` (the caller asked the user first); otherwise the item stops with
    /// `ERR_NAME_COLLISION`.
    pub async fn enqueue_remote_upload(&self, server_id: &str, local_file: &std::path::Path, remote_dir: &str, overwrite: bool) -> Result<Download> {
        if !local_file.is_file() {
            return Err(AppError::new(ErrorCode::ErrFileNotFound, format!("{} bukan berkas.", local_file.display())));
        }
        let name = local_file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.is_empty() || name.contains('/') {
            return Err(AppError::new(ErrorCode::ErrInvalidPath, "Nama berkas sumber tidak valid."));
        }
        let url = super::remote::make_url(super::remote::Direction::Upload, server_id, &crate::server::fs::join_remote(remote_dir, &name), overwrite);
        self.insert_remote(url, local_file.to_path_buf(), name, false).await
    }

    /// Queue a copy of `src_path` on one saved server into `dst_dir` on another (or the same) server.
    /// The data streams through this computer without being stored. An existing file at the
    /// destination is refused unless `overwrite` (the caller asked the user first), and a file can
    /// never be copied onto itself.
    pub async fn enqueue_remote_copy(&self, src_server: &str, src_path: &str, dst_server: &str, dst_dir: &str, overwrite: bool) -> Result<Download> {
        let name = src_path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        if name.is_empty() || name == "." || name == ".." {
            return Err(AppError::new(ErrorCode::ErrInvalidPath, "Nama berkas sumber kosong atau tidak valid."));
        }
        let dst_path = crate::server::fs::join_remote(dst_dir, name);
        let src_abs = if src_path.starts_with('/') { src_path.to_string() } else { format!("/{src_path}") };
        if src_server == dst_server && src_abs == dst_path {
            return Err(AppError::new(ErrorCode::ErrInvalidArgument, "Sumber dan tujuan adalah berkas yang sama. Pilih folder tujuan yang lain."));
        }
        let url = super::remote::make_copy_url(src_server, src_path, dst_server, &dst_path, overwrite);
        // `target_path` is only a label here: nothing is ever written to or removed from this computer.
        let label = format!("server://{dst_server}{dst_path}");
        self.insert_remote(url, PathBuf::from(label), name.to_string(), false).await
    }

    /// Queue every file under the remote folder `remote_dir` for download into `local_dir`, keeping the
    /// folder structure (the folder itself is created inside `local_dir`). One queue item per file; a
    /// file that cannot be queued (for example it already exists) is listed in `failed`.
    pub async fn enqueue_folder_download(&self, fs: &mut dyn crate::server::fs::RemoteFs, server_id: &str, remote_dir: &str, local_dir: &std::path::Path, overwrite: bool) -> Result<FolderQueued> {
        let found = crate::server::fs::walk::walk_remote(fs, remote_dir).await?;
        let mut out = FolderQueued::default();
        for f in found {
            let mut dir = local_dir.to_path_buf();
            let mut safe = true;
            for part in &f.rel_dir {
                match crate::server::fs::safe_local_name(part) {
                    Some(n) => dir.push(n),
                    None => {
                        safe = false;
                        break;
                    }
                }
            }
            if !safe {
                out.failed.push(format!("{}: nama folder tidak aman untuk dibuat di komputer ini", f.item.path));
                continue;
            }
            match self.enqueue_remote_download(server_id, &f.item.path, &dir, overwrite).await {
                Ok(d) => {
                    out.queued += 1;
                    out.ids.push(d.id);
                }
                Err(e) => out.failed.push(format!("{}: {}", f.item.path, e.message)),
            }
        }
        Ok(out)
    }

    /// Queue every file under the local folder `local_dir` for upload into `remote_dir`, keeping the
    /// folder structure (missing server folders are created when each file is sent).
    pub async fn enqueue_folder_upload(&self, server_id: &str, local_dir: &std::path::Path, remote_dir: &str, overwrite: bool) -> Result<FolderQueued> {
        let root = local_dir.to_path_buf();
        let found = tokio::task::spawn_blocking(move || crate::server::fs::walk::walk_local(&root)).await.map_err(|_| AppError::internal("pembacaan folder berhenti tiba-tiba"))??;
        let mut out = FolderQueued::default();
        for f in found {
            let dir = f.rel_dir.iter().fold(remote_dir.to_string(), |acc, part| crate::server::fs::join_remote(&acc, part));
            match self.enqueue_remote_upload(server_id, &f.item, &dir, overwrite).await {
                Ok(d) => {
                    out.queued += 1;
                    out.ids.push(d.id);
                }
                Err(e) => out.failed.push(format!("{}: {}", f.item.display(), e.message)),
            }
        }
        Ok(out)
    }

    /// Queue a copy of every file under `src_dir` (read through `src_fs`) into `dst_dir` on another or
    /// the same server, keeping the folder structure. A folder cannot be copied into itself.
    pub async fn enqueue_folder_copy(&self, src_fs: &mut dyn crate::server::fs::RemoteFs, src_server: &str, src_dir: &str, dst_server: &str, dst_dir: &str, overwrite: bool) -> Result<FolderQueued> {
        let src = src_dir.trim_end_matches('/');
        let dst = dst_dir.trim_end_matches('/');
        if src_server == dst_server && (dst == src || dst.starts_with(&format!("{src}/"))) {
            return Err(AppError::new(ErrorCode::ErrInvalidArgument, "Folder tidak bisa disalin ke dalam dirinya sendiri."));
        }
        let found = crate::server::fs::walk::walk_remote(src_fs, src_dir).await?;
        let mut out = FolderQueued::default();
        for f in found {
            let dir = f.rel_dir.iter().fold(dst_dir.to_string(), |acc, part| crate::server::fs::join_remote(&acc, part));
            match self.enqueue_remote_copy(src_server, &f.item.path, dst_server, &dir, overwrite).await {
                Ok(d) => {
                    out.queued += 1;
                    out.ids.push(d.id);
                }
                Err(e) => out.failed.push(format!("{}: {}", f.item.path, e.message)),
            }
        }
        Ok(out)
    }

    async fn insert_remote(&self, url: String, local: PathBuf, file_name: String, check_target_clash: bool) -> Result<Download> {
        let ts = repo::now();
        let dl = Download {
            id: uuid::Uuid::new_v4().to_string(),
            original_url: url.clone(),
            effective_url: url,
            target_path: local.to_string_lossy().into_owned(),
            file_name,
            total_bytes: -1,
            downloaded_bytes: 0,
            state: DownloadState::Queued,
            segmentation_supported: false,
            connection_count: 1,
            speed_limit_kbps: 0,
            category_id: None,
            mime_type: None,
            referer: None,
            user_agent: None,
            cookies: None,
            error_code: None,
            error_message: None,
            created_at: ts.clone(),
            updated_at: ts,
            completed_at: None,
        };
        let d2 = dl.clone();
        self.db()
            .write(move |c| {
                if check_target_clash {
                    let clash: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM downloads WHERE target_path=?1 AND state IN ('queued','running','paused'))", [&d2.target_path], |r| r.get(0))?;
                    if clash {
                        return Err(AppError::new(ErrorCode::ErrNameCollision, format!("{} adalah tujuan unduhan aktif lain.", d2.target_path)));
                    }
                }
                let tx = c.transaction()?;
                repo::insert_download(&tx, &d2)?;
                repo::enqueue(&tx, &d2.id, 0)?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        self.emit_state(&dl.id, dl.state, None);
        self.dispatch().await?;
        self.get(&dl.id)
    }

    pub fn get(&self, id: &str) -> Result<Download> {
        self.db().read(|c| repo::get_download(c, id))
    }

    // ---- dispatch ----------------------------------------------------------

    /// Start as many queued items as the concurrency limit allows, strictly by
    /// `priority DESC, queue_position ASC` (DEC-002, FR-018, FR-020).
    ///
    /// Returns a boxed future: dispatch -> spawn_transfer -> (task) -> dispatch is
    /// recursive, and boxing breaks the auto-trait inference cycle.
    pub fn dispatch(&self) -> BoxFuture<'static, Result<()>> {
        let this = self.clone();
        Box::pin(async move { this.dispatch_impl().await })
    }

    async fn dispatch_impl(&self) -> Result<()> {
        let _gate = self.inner.dispatch_gate.lock().await;
        let limit = self.concurrency_limit();
        let running = self.running_count();
        if running >= limit {
            return Ok(());
        }
        let ids = self.db().read(|c| repo::next_dispatch_candidates(c, limit - running))?;
        for id in ids {
            self.spawn_transfer(id).await?;
        }
        Ok(())
    }

    async fn spawn_transfer(&self, id: String) -> Result<()> {
        let dl = self.get(&id)?;
        let id2 = id.clone();
        self.db().write(move |c| repo::set_state(c, &id2, DownloadState::Running, None)).await?;
        self.emit_state(&id, DownloadState::Running, None);

        let cancel = CancellationToken::new();
        let intent = Arc::new(Mutex::new(StopIntent::Pause));
        let bucket = Arc::new(TokenBucket::new(dl.speed_limit_kbps as u64));
        self.inner.running.lock().unwrap().insert(id.clone(), RunningEntry { cancel: cancel.clone(), intent: intent.clone(), bucket: bucket.clone() });

        let this = self.clone();
        tokio::spawn(async move {
            let outcome = run_transfer(&this.inner.ctx, &id, cancel, bucket).await;
            let stop = *intent.lock().unwrap();
            if let Err(e) = this.finish(&id, outcome, stop).await {
                tracing::error!(download = %id, error = %e, "failed to record transfer outcome");
            }
            this.inner.running.lock().unwrap().remove(&id);
            if let Err(e) = this.dispatch().await {
                tracing::error!(error = %e, "dispatch after completion failed");
            }
        });
        Ok(())
    }

    async fn finish(&self, id: &str, outcome: Outcome, stop: StopIntent) -> Result<()> {
        let id_s = id.to_string();
        let (state, code): (DownloadState, Option<String>) = match outcome {
            Outcome::Completed => {
                self.db().write(move |c| repo::set_state(c, &id_s, DownloadState::Completed, None)).await?;
                tracing::info!(download = id, "download completed");
                (DownloadState::Completed, None)
            }
            Outcome::Failed(e) => {
                tracing::warn!(download = id, code = %e.code, error = %e.message, "download failed");
                let (code, msg) = (e.code.as_str().to_string(), e.message.clone());
                self.db().write(move |c| repo::set_state(c, &id_s, DownloadState::Failed, Some((&code, &msg)))).await?;
                (DownloadState::Failed, Some(e.code.as_str().to_string()))
            }
            Outcome::Stopped => match stop {
                StopIntent::Pause => {
                    self.db().write(move |c| repo::set_state(c, &id_s, DownloadState::Paused, None)).await?;
                    (DownloadState::Paused, None)
                }
                StopIntent::Cancel { delete_file } => {
                    self.apply_cancel(id, delete_file).await?;
                    (DownloadState::Cancelled, None)
                }
                StopIntent::Restart => {
                    self.apply_restart(id).await?;
                    (DownloadState::Queued, None)
                }
            },
        };
        self.emit_state(id, state, code);
        Ok(())
    }

    fn emit_state(&self, id: &str, state: DownloadState, code: Option<String>) {
        let _ = self.inner.ctx.events.send(EngineEvent::StateChanged { id: id.to_string(), state, error_code: code });
    }

    // ---- lifecycle commands (all idempotent, API.md §2.7) ------------------

    fn signal(&self, id: &str, intent: StopIntent) -> bool {
        match self.inner.running.lock().unwrap().get(id) {
            Some(e) => {
                *e.intent.lock().unwrap() = intent;
                e.cancel.cancel();
                true
            }
            None => false,
        }
    }

    pub async fn pause(&self, id: &str) -> Result<DownloadState> {
        let dl = self.get(id)?;
        if self.signal(id, StopIntent::Pause) {
            return Ok(DownloadState::Paused);
        }
        if dl.state == DownloadState::Queued {
            let id2 = id.to_string();
            self.db().write(move |c| repo::set_state(c, &id2, DownloadState::Paused, None)).await?;
            self.emit_state(id, DownloadState::Paused, None);
            return Ok(DownloadState::Paused);
        }
        Ok(dl.state)
    }

    /// Paused or failed items go back to the queue; they start when a slot is free.
    pub async fn resume(&self, id: &str) -> Result<DownloadState> {
        let dl = self.get(id)?;
        if matches!(dl.state, DownloadState::Paused | DownloadState::Failed) && !self.inner.running.lock().unwrap().contains_key(id) {
            let id2 = id.to_string();
            self.db().write(move |c| repo::set_state(c, &id2, DownloadState::Queued, None)).await?;
            self.emit_state(id, DownloadState::Queued, None);
            self.dispatch().await?;
        }
        Ok(self.get(id)?.state)
    }

    /// FR-009: discard segment offsets, truncate the partial file, fetch from byte 0.
    pub async fn restart(&self, id: &str) -> Result<DownloadState> {
        let dl = self.get(id)?;
        if self.signal(id, StopIntent::Restart) {
            return Ok(DownloadState::Queued);
        }
        if dl.state != DownloadState::Cancelled {
            self.apply_restart(id).await?;
            self.emit_state(id, DownloadState::Queued, None);
            self.dispatch().await?;
        }
        Ok(self.get(id)?.state)
    }

    async fn apply_restart(&self, id: &str) -> Result<()> {
        let dl = self.get(id)?;
        let id2 = id.to_string();
        self.db()
            .write(move |c| {
                repo::reset_progress(c, &id2)?;
                repo::set_state(c, &id2, DownloadState::Queued, None)
            })
            .await?;
        if torrent::is_magnet(&dl.original_url) {
            // Start over: drop the torrent's own files, then let the next run add it fresh.
            self.inner.ctx.torrent.forget(id, true).await?;
            let id3 = id.to_string();
            self.db().write(move |c| Ok(c.execute("UPDATE torrents SET started=0, uploaded_bytes=0, seed_state='none' WHERE download_id=?1", [&id3]).map(|_| ())?)).await?;
            return Ok(());
        }
        // The segment plan is gone, so the next run re-probes and re-allocates. For an upload
        // `target_path` is the user's own file: it is never touched.
        let path = PathBuf::from(&dl.target_path);
        if path.exists() && !super::remote::leaves_local_files_alone(&dl.original_url) {
            let f = std::fs::OpenOptions::new().write(true).open(&path).map_err(|e| AppError::new(ErrorCode::ErrPathUnwritable, e.to_string()))?;
            f.set_len(0).map_err(|e| AppError::new(ErrorCode::ErrPathUnwritable, e.to_string()))?;
        }
        Ok(())
    }

    pub async fn cancel(&self, id: &str, delete_file: bool) -> Result<DownloadState> {
        let dl = self.get(id)?;
        if self.signal(id, StopIntent::Cancel { delete_file }) {
            return Ok(DownloadState::Cancelled);
        }
        if !matches!(dl.state, DownloadState::Cancelled | DownloadState::Completed) {
            self.apply_cancel(id, delete_file).await?;
            self.emit_state(id, DownloadState::Cancelled, None);
        }
        Ok(self.get(id)?.state)
    }

    async fn apply_cancel(&self, id: &str, delete_file: bool) -> Result<()> {
        let dl = self.get(id)?;
        // Remove the file BEFORE publishing `cancelled`: anyone who observes that state
        // (UI, tests, a crash-restart) must never find the file still on disk.
        if torrent::is_magnet(&dl.original_url) {
            self.inner.ctx.torrent.forget(id, delete_file).await?;
        } else if delete_file && !super::remote::leaves_local_files_alone(&dl.original_url) {
            // For an upload `target_path` is the user's own source file: it is never deleted.
            let _ = std::fs::remove_file(&dl.target_path);
        }
        let id2 = id.to_string();
        self.db()
            .write(move |c| {
                c.execute("DELETE FROM queue_items WHERE download_id=?1", [&id2])?;
                repo::set_state(c, &id2, DownloadState::Cancelled, None)
            })
            .await?;
        Ok(())
    }

    /// `download.delete` (API.md §3.1.8): remove the record (segments and queue entry
    /// cascade). A running transfer is cancelled first. The file is kept unless asked.
    pub async fn delete(&self, id: &str, delete_file: bool) -> Result<()> {
        let dl = self.get(id)?;
        if self.signal(id, StopIntent::Cancel { delete_file }) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.inner.running.lock().unwrap().contains_key(id) {
                if Instant::now() > deadline {
                    return Err(AppError::internal("timed out waiting for the transfer to stop"));
                }
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        }
        let is_torrent = torrent::is_magnet(&dl.original_url);
        if is_torrent {
            // Needs the torrents row, so it runs before the record disappears.
            self.inner.ctx.torrent.forget(id, delete_file).await?;
        }
        let id2 = id.to_string();
        self.db().write(move |c| Ok(c.execute("DELETE FROM downloads WHERE id=?1", [&id2])?)).await?;
        if delete_file && !is_torrent && !super::remote::leaves_local_files_alone(&dl.original_url) {
            let _ = std::fs::remove_file(&dl.target_path);
        }
        Ok(())
    }

    /// Toolbar "Bersihkan Selesai": drop completed and cancelled records; files stay on disk.
    pub async fn clear_finished(&self) -> Result<usize> {
        self.db().write(|c| Ok(c.execute("DELETE FROM downloads WHERE state IN ('completed','cancelled')", [])?)).await
    }

    /// FR-022. Paused items become queued; only up to the concurrency limit start.
    pub async fn start_all(&self) -> Result<(usize, usize)> {
        let n = self
            .db()
            .write(|c| {
                Ok(c.execute(
                    "UPDATE downloads SET state='queued', error_code=NULL, error_message=NULL, updated_at=?1 \
                     WHERE state='paused' AND id IN (SELECT download_id FROM queue_items)",
                    [repo::now()],
                )?)
            })
            .await?;
        self.dispatch().await?;
        Ok((n, self.running_count()))
    }

    /// FR-022. Running items checkpoint and pause; queued items become paused.
    pub async fn stop_all(&self) -> Result<usize> {
        let queued = self
            .db()
            .write(|c| Ok(c.execute("UPDATE downloads SET state='paused', updated_at=?1 WHERE state='queued'", [repo::now()])?))
            .await?;
        let ids: Vec<String> = self.inner.running.lock().unwrap().keys().cloned().collect();
        for id in &ids {
            self.signal(id, StopIntent::Pause);
        }
        Ok(queued + ids.len())
    }

    pub async fn set_priority(&self, id: &str, priority: i32) -> Result<()> {
        let id2 = id.to_string();
        self.db().write(move |c| repo::set_priority(c, &id2, priority)).await
    }

    pub async fn reorder(&self, ordered: Vec<String>) -> Result<usize> {
        self.db().write(move |c| repo::reorder_queue(c, &ordered)).await
    }

    /// FR-053: pause every running transfer, wait for the checkpoints to land.
    pub async fn shutdown(&self, timeout: Duration) -> Result<()> {
        let ids: Vec<String> = self.inner.running.lock().unwrap().keys().cloned().collect();
        for id in &ids {
            self.signal(id, StopIntent::Pause);
        }
        let deadline = Instant::now() + timeout;
        while self.running_count() > 0 {
            if Instant::now() > deadline {
                return Err(AppError::internal("timed out waiting for transfers to checkpoint"));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // Seeding torrents keep `seed_state='seeding'` so the next start resumes them.
        self.inner.ctx.torrent.flush_and_stop().await;
        Ok(())
    }

    /// Poll until `id` reaches `state` (used by the UI shell and tests).
    pub async fn wait_for_state(&self, id: &str, state: DownloadState, timeout: Duration) -> Result<Download> {
        let deadline = Instant::now() + timeout;
        loop {
            let d = self.get(id)?;
            if d.state == state {
                return Ok(d);
            }
            if Instant::now() > deadline {
                return Err(AppError::internal(format!("timed out waiting for {id} to reach {}; it is {}", state.as_str(), d.state.as_str())));
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }
}
