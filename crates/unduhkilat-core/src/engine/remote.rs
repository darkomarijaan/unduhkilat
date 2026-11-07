//! Transfers to and from saved servers (SFTP, FTP, FTPS) through the normal queue.
//!
//! A queue item is recognised by its URL: `server://<server id>/<remote path>` is a download (the
//! local destination is `target_path`) and `server-up://<server id>/<remote path>` is an upload (the
//! local source is `target_path`). `?overwrite=1` is added only after the user confirmed replacing an
//! existing file. Pause, resume, priority, speed limits and notifications come from the shared engine.

use super::throttle::TokenBucket;
use super::transfer::{Outcome, TransferCtx};
use super::EngineEvent;
use crate::error::{AppError, ErrorCode, Result};
use crate::repo;
use crate::server::fs::RemoteFs;
use crate::server::secrets::{SecretKind, SecretString};
use crate::server::service::ServerService;
use crate::server::ssh::Interact;
use futures_util::future::BoxFuture;
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

const DOWN_PREFIX: &str = "server://";
const UP_PREFIX: &str = "server-up://";
const CHUNK: usize = 64 * 1024;
/// Everything but `/` is escaped, so names with spaces, `?`, `#`, or Unicode survive the URL.
const PATH_SET: &AsciiSet = &NON_ALPHANUMERIC.remove(b'/');

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Download,
    Upload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRef {
    pub direction: Direction,
    pub server_id: String,
    pub path: String,
    pub overwrite: bool,
}

pub fn is_remote(url: &str) -> bool {
    url.starts_with(DOWN_PREFIX) || url.starts_with(UP_PREFIX)
}

pub fn is_upload(url: &str) -> bool {
    url.starts_with(UP_PREFIX)
}

pub fn make_url(direction: Direction, server_id: &str, remote_path: &str, overwrite: bool) -> String {
    let prefix = if direction == Direction::Download { DOWN_PREFIX } else { UP_PREFIX };
    let path = if remote_path.starts_with('/') { remote_path.to_string() } else { format!("/{remote_path}") };
    let mut url = format!("{prefix}{server_id}{}", utf8_percent_encode(&path, PATH_SET));
    if overwrite {
        url.push_str("?overwrite=1");
    }
    url
}

pub fn parse_url(url: &str) -> Option<RemoteRef> {
    let (direction, rest) = if let Some(r) = url.strip_prefix(DOWN_PREFIX) {
        (Direction::Download, r)
    } else {
        (Direction::Upload, url.strip_prefix(UP_PREFIX)?)
    };
    let (rest, overwrite) = match rest.strip_suffix("?overwrite=1") {
        Some(r) => (r, true),
        None => (rest, false),
    };
    let slash = rest.find('/')?;
    let (server_id, path) = rest.split_at(slash);
    if server_id.is_empty() {
        return None;
    }
    Some(RemoteRef { direction, server_id: server_id.to_string(), path: percent_decode_str(path).decode_utf8().ok()?.into_owned(), overwrite })
}

/// Queue items cannot show dialogs: anything that needs the user to answer fails with a hint.
struct NoPrompt;

impl Interact for NoPrompt {
    fn confirm_host_key<'a>(&'a self, _h: &'a str, _p: u16, _t: &'a str, _f: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async { false })
    }
    fn secret<'a>(&'a self, _n: &'a str, _k: SecretKind) -> BoxFuture<'a, Option<SecretString>> {
        Box::pin(async { None })
    }
}

fn needs_the_user(mut e: AppError) -> AppError {
    if matches!(e.code, ErrorCode::ErrHostKeyUnknown | ErrorCode::ErrSshAuthFailed) {
        e.message = format!("{} Buka layar Server, tekan Uji sambungan untuk mengonfirmasi, lalu lanjutkan transfer ini.", e.message);
    }
    e
}

pub async fn run_remote(ctx: &TransferCtx, id: &str, cancel: &CancellationToken, bucket: Arc<TokenBucket>) -> Result<Outcome> {
    let dl = ctx.db.read(|c| repo::get_download(c, id))?;
    let r = parse_url(&dl.original_url).ok_or_else(|| AppError::new(ErrorCode::ErrInvalidUrlScheme, "Tautan transfer server tidak valid."))?;
    let proxy = ctx.proxy();
    let svc = ServerService::new(ctx.db.clone(), Arc::new(move || proxy.clone()));
    let mut fs = svc.open_fs(&r.server_id, Arc::new(NoPrompt), cancel.clone()).await.map_err(needs_the_user);
    if cancel.is_cancelled() {
        if let Ok(fs) = fs {
            fs.close().await;
        }
        return Ok(Outcome::Stopped);
    }
    let result = match fs.as_mut() {
        Ok(fs) => match r.direction {
            Direction::Download => download(ctx, id, &dl.target_path, &r, fs.as_mut(), cancel, &bucket).await,
            Direction::Upload => upload(ctx, id, &dl.target_path, &r, fs.as_mut(), cancel, &bucket).await,
        },
        Err(e) => Err(e.clone()),
    };
    if let Ok(fs) = fs {
        fs.close().await;
    }
    result
}

/// Tracks progress: events every 250 ms, durable checkpoints about once a second.
struct Meter {
    started: Instant,
    last_emit: Instant,
    last_bytes: i64,
    last_save: Instant,
}

impl Meter {
    fn new(at: i64) -> Self {
        let now = Instant::now();
        Meter { started: now, last_emit: now, last_bytes: at, last_save: now }
    }

    async fn tick(&mut self, ctx: &TransferCtx, id: &str, bytes: i64, total: i64) {
        if self.last_emit.elapsed() >= Duration::from_millis(250) {
            let dt = self.last_emit.elapsed().as_secs_f64().max(0.001);
            let speed = ((bytes - self.last_bytes).max(0) as f64 / dt) as u64;
            let _ = ctx.events.send(EngineEvent::Progress { id: id.to_string(), downloaded_bytes: bytes, total_bytes: total, current_speed_bps: speed, active_connections: 1 });
            self.last_emit = Instant::now();
            self.last_bytes = bytes;
        }
        if self.last_save.elapsed() >= Duration::from_secs(1) {
            self.save(ctx, id, bytes).await;
        }
        let _ = self.started;
    }

    async fn save(&mut self, ctx: &TransferCtx, id: &str, bytes: i64) {
        let id2 = id.to_string();
        let _ = ctx.db.write(move |c| repo::set_downloaded_bytes(c, &id2, bytes)).await;
        self.last_save = Instant::now();
    }
}

fn io_err(what: &str, e: std::io::Error) -> AppError {
    AppError::new(ErrorCode::ErrPathUnwritable, format!("{what}: {e}"))
}

async fn download(ctx: &TransferCtx, id: &str, target: &str, r: &RemoteRef, fs: &mut dyn RemoteFs, cancel: &CancellationToken, bucket: &Arc<TokenBucket>) -> Result<Outcome> {
    let entry = fs.stat(&r.path).await?;
    if entry.is_dir {
        return Err(AppError::new(ErrorCode::ErrInvalidArgument, "Folder tidak bisa diunduh sebagai satu berkas."));
    }
    let total = entry.size as i64;
    let (id2, url) = (id.to_string(), make_url(Direction::Download, &r.server_id, &r.path, r.overwrite));
    ctx.db.write(move |c| repo::update_probe_result(c, &id2, &url, total, None, false)).await?;

    let path = std::path::PathBuf::from(target);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_err("folder tujuan", e))?;
    }
    let dl = ctx.db.read(|c| repo::get_download(c, id))?;
    let have = std::fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);
    // Resume from what was recorded, never beyond what is really on disk.
    let offset = if dl.downloaded_bytes > 0 { dl.downloaded_bytes.min(have).min(total) } else { 0 };
    let mut file = tokio::fs::OpenOptions::new().write(true).create(true).truncate(false).open(&path).await.map_err(|e| io_err(&path.display().to_string(), e))?;
    file.set_len(offset as u64).await.map_err(|e| io_err("memotong berkas", e))?;
    file.seek(std::io::SeekFrom::Start(offset as u64)).await.map_err(|e| io_err("mencari posisi berkas", e))?;

    let mut reader = fs.open_read(&r.path, offset as u64).await?;
    let mut buf = vec![0u8; CHUNK];
    let mut written = offset;
    let mut meter = Meter::new(offset);
    loop {
        if cancel.is_cancelled() {
            file.flush().await.ok();
            meter.save(ctx, id, written).await;
            return Ok(Outcome::Stopped);
        }
        let n = tokio::select! {
            _ = cancel.cancelled() => continue,
            n = reader.read(&mut buf) => n.map_err(|e| AppError::new(ErrorCode::ErrConnectionDropped, format!("Unduhan dari server terputus: {e}")))?,
        };
        if n == 0 {
            break;
        }
        ctx.throttle.acquire(bucket, n).await;
        file.write_all(&buf[..n]).await.map_err(|e| io_err("menulis berkas", e))?;
        written += n as i64;
        meter.tick(ctx, id, written, total).await;
    }
    file.flush().await.map_err(|e| io_err("menulis berkas", e))?;
    meter.save(ctx, id, written).await;
    if written != total {
        return Err(AppError::new(ErrorCode::ErrConnectionDropped, format!("Unduhan terputus sebelum selesai ({written} dari {total} byte). Lanjutkan untuk meneruskan.")));
    }
    Ok(Outcome::Completed)
}

async fn upload(ctx: &TransferCtx, id: &str, source: &str, r: &RemoteRef, fs: &mut dyn RemoteFs, cancel: &CancellationToken, bucket: &Arc<TokenBucket>) -> Result<Outcome> {
    let mut file = tokio::fs::File::open(source).await.map_err(|e| AppError::new(ErrorCode::ErrFileNotFound, format!("Berkas sumber tidak bisa dibuka ({source}): {e}")))?;
    let total = file.metadata().await.map_err(|e| io_err("membaca berkas sumber", e))?.len() as i64;
    let (id2, url) = (id.to_string(), make_url(Direction::Upload, &r.server_id, &r.path, r.overwrite));
    ctx.db.write(move |c| repo::update_probe_result(c, &id2, &url, total, None, false)).await?;

    let dl = ctx.db.read(|c| repo::get_download(c, id))?;
    // What is already on the server decides between a collision and a resume.
    let offset = match fs.stat(&r.path).await {
        Ok(e) if e.is_dir => return Err(AppError::new(ErrorCode::ErrNameCollision, "Di server sudah ada folder dengan nama itu.")),
        Ok(e) if dl.downloaded_bytes > 0 && (e.size as i64) <= total => e.size as i64,
        Ok(_) if r.overwrite => 0,
        Ok(_) => return Err(AppError::new(ErrorCode::ErrNameCollision, "Berkas dengan nama itu sudah ada di server dan tidak ditimpa.")),
        Err(e) if e.code == ErrorCode::ErrNotFound => 0,
        Err(e) => return Err(e),
    };
    file.seek(std::io::SeekFrom::Start(offset as u64)).await.map_err(|e| io_err("mencari posisi berkas", e))?;
    let mut writer = fs.open_write(&r.path, offset as u64, offset > 0).await?;
    let mut buf = vec![0u8; CHUNK];
    let mut sent = offset;
    let mut meter = Meter::new(offset);
    loop {
        if cancel.is_cancelled() {
            let _ = writer.flush().await;
            drop(writer);
            meter.save(ctx, id, sent).await;
            return Ok(Outcome::Stopped);
        }
        let n = file.read(&mut buf).await.map_err(|e| io_err("membaca berkas sumber", e))?;
        if n == 0 {
            break;
        }
        ctx.throttle.acquire(bucket, n).await;
        writer.write_all(&buf[..n]).await.map_err(|e| AppError::new(ErrorCode::ErrConnectionDropped, format!("Unggahan ke server terputus: {e}")))?;
        sent += n as i64;
        meter.tick(ctx, id, sent, total).await;
    }
    writer.shutdown().await.map_err(|e| AppError::new(ErrorCode::ErrConnectionDropped, format!("Server tidak mengonfirmasi unggahan: {e}")))?;
    drop(writer);
    meter.save(ctx, id, sent).await;
    let size = fs.stat(&r.path).await?.size as i64;
    if size != total {
        return Err(AppError::new(ErrorCode::ErrConnectionDropped, format!("Ukuran di server ({size}) berbeda dari berkas ({total}). Lanjutkan untuk meneruskan.")));
    }
    Ok(Outcome::Completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_roundtrip_with_awkward_names() {
        for path in ["/a.txt", "/dir with space/\u{4F60}\u{597D} #1?.txt", "/100%/x"] {
            for (dir, ow) in [(Direction::Download, false), (Direction::Upload, true)] {
                let u = make_url(dir, "srv-1", path, ow);
                assert!(is_remote(&u));
                assert_eq!(parse_url(&u), Some(RemoteRef { direction: dir, server_id: "srv-1".into(), path: path.into(), overwrite: ow }), "{u}");
            }
        }
        assert!(parse_url("https://example.com/x").is_none());
        assert!(parse_url("server:///x").is_none(), "no server id");
    }
}
