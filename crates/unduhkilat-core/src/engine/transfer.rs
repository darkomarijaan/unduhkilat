//! One transfer run: probe, plan segments, spawn workers, checkpoint, verify.
//!
//! Offset convention (DATABASE.md §3.2): a segment's persisted offset is the NEXT
//! byte to fetch. Resume therefore requests `bytes={offset}-{range_end}`.

use super::ftp::{self, FtpConn, FtpTarget, Timeouts};
use super::http::{self, RangeSupport, RequestContext};
use super::proxy::{HttpProxy, ProxyConfig};
use super::segments::{allocate_segments, remaining_range};
use super::throttle::{Throttler, TokenBucket};
use super::EngineEvent;
use crate::config::EngineConfig;
use crate::db::Db;
use crate::error::{AppError, ErrorCode, Result};
use crate::models::Download;
use crate::repo;
use bytes::Bytes;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;
use zeroize::Zeroizing;
use reqwest::header::RANGE;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;
use tokio::sync::broadcast;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub struct TransferCtx {
    pub db: Db,
    /// Swapped when the proxy settings change; running transfers keep their old client.
    pub client: RwLock<reqwest::Client>,
    pub proxy: RwLock<ProxyConfig>,
    pub throttle: Arc<Throttler>,
    pub cfg: EngineConfig,
    pub events: broadcast::Sender<EngineEvent>,
    pub torrent: super::torrent::TorrentEngine,
}

impl TransferCtx {
    pub fn client(&self) -> reqwest::Client {
        self.client.read().unwrap().clone()
    }
    pub fn proxy(&self) -> ProxyConfig {
        self.proxy.read().unwrap().clone()
    }
}

/// FR-014: stored credentials for an FTP host (only used when the URL carries none).
fn resolve_ftp_target(ctx: &TransferCtx, url: &str) -> Result<FtpTarget> {
    let mut t = ftp::parse(url)?;
    if t.anonymous {
        let (host, port) = (t.host.clone(), t.port);
        if let Some(c) = ctx.db.read(|c| repo::find_credential(c, &host, Some(port), "ftp"))? {
            t.user = c.username;
            t.pass = Zeroizing::new((*crate::dpapi::dpapi_decrypt(&c.password_encrypted)?).clone());
            t.anonymous = false;
        }
    }
    Ok(t)
}

fn stored_basic(ctx: &TransferCtx, url: &str) -> Option<(String, Zeroizing<String>)> {
    let u = url::Url::parse(url).ok()?;
    let host = u.host_str()?.to_string();
    let c = ctx.db.read(|c| repo::find_credential(c, &host, u.port(), u.scheme())).ok().flatten()?;
    let pw = crate::dpapi::dpapi_decrypt(&c.password_encrypted).ok()?;
    Some((c.username, Zeroizing::new((*pw).clone())))
}

#[derive(Clone)]
enum Source {
    Http { client: reqwest::Client, url: String, rctx: RequestContext },
    Ftp { target: Arc<FtpTarget>, proxy: Option<HttpProxy>, timeouts: Timeouts },
}

#[derive(Debug)]
pub enum Outcome {
    Completed,
    /// Cancelled by the caller; offsets were flushed. The caller decides the next state.
    Stopped,
    Failed(AppError),
}

/// In-memory view of one segment. `next` is the next byte to fetch.
struct SegRt {
    index: u32,
    start: i64,
    /// Inclusive. `-1` means the length is unknown (single streaming connection).
    end: i64,
    next: AtomicI64,
}

impl SegRt {
    fn done(&self) -> bool {
        self.end >= 0 && self.next.load(Ordering::Acquire) > self.end
    }
}

#[cfg(windows)]
fn write_at(file: &File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut done = 0;
    while done < buf.len() {
        let n = file.seek_write(&buf[done..], offset + done as u64)?;
        if n == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        done += n;
    }
    Ok(())
}

#[cfg(unix)]
fn write_at(file: &File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(buf, offset)
}

pub async fn run_transfer(ctx: &TransferCtx, id: &str, cancel: CancellationToken, item_bucket: Arc<TokenBucket>) -> Outcome {
    match run_inner(ctx, id, &cancel, item_bucket).await {
        Ok(o) => o,
        Err(e) => Outcome::Failed(e),
    }
}

async fn run_inner(ctx: &TransferCtx, id: &str, cancel: &CancellationToken, item_bucket: Arc<TokenBucket>) -> Result<Outcome> {
    let mut dl = ctx.db.read(|c| repo::get_download(c, id))?;
    if super::torrent::is_magnet(&dl.original_url) {
        return super::torrent::run_torrent(ctx, id, cancel, item_bucket).await;
    }
    if super::remote::is_remote(&dl.original_url) {
        return super::remote::run_remote(ctx, id, cancel, item_bucket).await;
    }
    let scheme = url::Url::parse(&dl.original_url).map(|u| u.scheme().to_string()).unwrap_or_default();
    let is_ftp = match scheme.as_str() {
        "http" | "https" => false,
        "ftp" => true,
        _ => return Err(AppError::new(ErrorCode::ErrInvalidUrlScheme, format!("unsupported scheme in {}", dl.original_url))),
    };
    let mut rctx = RequestContext::from_download(&dl);
    let timeouts = Timeouts { connect: ctx.cfg.connect_timeout, read: ctx.cfg.read_timeout };
    let (ftp_target, ftp_proxy) = if is_ftp {
        (Some(Arc::new(resolve_ftp_target(ctx, &dl.original_url)?)), ctx.proxy().ftp_proxy()?)
    } else {
        rctx.basic = stored_basic(ctx, &dl.original_url);
        (None, None)
    };
    let path = PathBuf::from(&dl.target_path);

    let mut segs = ctx.db.read(|c| repo::get_segments(c, id))?;
    // A server that cannot do ranges cannot resume: start over (FR-005 fallback).
    if !segs.is_empty() && !dl.segmentation_supported {
        let id2 = id.to_string();
        ctx.db.write(move |c| repo::reset_progress(c, &id2)).await?;
        segs.clear();
    }
    let fresh = segs.is_empty();

    if fresh {
        let probe = match &ftp_target {
            Some(t) => ftp::probe(&dl.original_url, t, ftp_proxy.as_ref(), &timeouts).await?,
            None => http::probe(&ctx.client(), &dl.original_url, &rctx).await?,
        };
        let supported = probe.range == RangeSupport::Supported;
        let total = probe.total_bytes.unwrap_or(-1);
        let ranges: Vec<(i64, i64)> = match (supported, total) {
            (true, t) if t > 0 => allocate_segments(t, dl.connection_count),
            (false, t) if t > 0 => vec![(0, t - 1)], // single connection, still tracked
            _ => Vec::new(),
        };
        let (id2, eff, mime) = (id.to_string(), probe.effective_url.clone(), probe.mime_type.clone());
        ctx.db
            .write(move |c| {
                repo::update_probe_result(c, &id2, &eff, total, mime.as_deref(), supported)?;
                if !ranges.is_empty() {
                    repo::insert_segments(c, &id2, &ranges)?;
                }
                Ok(())
            })
            .await?;
        dl = ctx.db.read(|c| repo::get_download(c, id))?;
        segs = ctx.db.read(|c| repo::get_segments(c, id))?;
    } else if !path.exists() {
        return Err(AppError::new(ErrorCode::ErrFileNotFound, format!("partial file {} is missing", path.display())));
    }

    // Open (or create) the destination. Never truncates existing partial data on resume.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::new(ErrorCode::ErrPathUnwritable, e.to_string()))?;
    }
    let file = Arc::new(
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| AppError::new(ErrorCode::ErrPathUnwritable, format!("{}: {e}", path.display())))?,
    );
    if fresh {
        // Pre-allocate when the size is known; otherwise start empty.
        file.set_len(dl.total_bytes.max(0) as u64)?;
    }

    let ranged = dl.segmentation_supported;
    let unknown_size = dl.total_bytes < 0;
    let rts: Vec<Arc<SegRt>> = if unknown_size {
        vec![Arc::new(SegRt { index: 0, start: 0, end: -1, next: AtomicI64::new(0) })]
    } else {
        segs.iter()
            .map(|s| Arc::new(SegRt { index: s.segment_index, start: s.range_start, end: s.range_end, next: AtomicI64::new(s.highest_contiguous_offset) }))
            .collect()
    };

    tracing::info!(download = id, segments = rts.len(), ranged, total = dl.total_bytes, "transfer starting");

    let source = match &ftp_target {
        Some(t) => Source::Ftp { target: t.clone(), proxy: ftp_proxy.clone(), timeouts: timeouts.clone() },
        None => Source::Http { client: ctx.client(), url: dl.effective_url.clone(), rctx: rctx.clone() },
    };
    let child = cancel.child_token();
    let mut workers: JoinSet<Result<()>> = JoinSet::new();
    for seg in rts.iter().filter(|s| !s.done()) {
        workers.spawn(segment_worker(WorkerArgs {
            source: source.clone(),
            file: file.clone(),
            seg: seg.clone(),
            ranged,
            item_bucket: item_bucket.clone(),
            throttle: ctx.throttle.clone(),
            cancel: child.clone(),
        }));
    }

    let mut first_err: Option<AppError> = None;
    let mut ticker = tokio::time::interval(ctx.cfg.checkpoint_interval);
    ticker.tick().await; // first tick is immediate
    let (mut last_bytes, mut last_at) = (rts.iter().map(|s| s.next.load(Ordering::Acquire) - s.start).sum::<i64>(), Instant::now());

    while !workers.is_empty() {
        tokio::select! {
            _ = ticker.tick() => {
                match flush(ctx, id, &rts, &file, unknown_size).await {
                    Ok(bytes) => {
                        let dt = last_at.elapsed().as_secs_f64().max(0.001);
                        let speed = ((bytes - last_bytes).max(0) as f64 / dt) as u64;
                        (last_bytes, last_at) = (bytes, Instant::now());
                        let live = workers.len() as u32;
                        let _ = ctx.events.send(EngineEvent::Progress {
                            id: id.to_string(), downloaded_bytes: bytes, total_bytes: dl.total_bytes,
                            current_speed_bps: speed, active_connections: live,
                        });
                    }
                    Err(e) => { if first_err.is_none() { first_err = Some(e); child.cancel(); } }
                }
            }
            joined = workers.join_next() => match joined {
                None => break,
                Some(Ok(Ok(()))) => {}
                Some(Ok(Err(e))) => { if first_err.is_none() { first_err = Some(e); child.cancel(); } }
                Some(Err(join)) => {
                    if first_err.is_none() { first_err = Some(AppError::internal(format!("worker panicked: {join}"))); child.cancel(); }
                }
            }
        }
    }

    // Always persist what we have, even on failure/stop, so no received byte is re-fetched.
    let flushed = flush(ctx, id, &rts, &file, unknown_size).await;
    if let Some(e) = first_err {
        return Ok(Outcome::Failed(e));
    }
    flushed?;
    if cancel.is_cancelled() {
        return Ok(Outcome::Stopped);
    }
    verify_and_finalize(ctx, id, &dl, &rts, &file, unknown_size).await
}

/// FR-011: an item is `completed` only if every segment finished AND the on-disk size matches.
async fn verify_and_finalize(ctx: &TransferCtx, id: &str, dl: &Download, rts: &[Arc<SegRt>], file: &Arc<File>, unknown_size: bool) -> Result<Outcome> {
    if let Some(s) = rts.iter().find(|s| s.end >= 0 && !s.done()) {
        return Ok(Outcome::Failed(AppError::new(
            ErrorCode::ErrConnectionDropped,
            format!("segment {} ended early at byte {} of {}", s.index, s.next.load(Ordering::Acquire), s.end + 1),
        )));
    }
    let received: i64 = rts.iter().map(|s| s.next.load(Ordering::Acquire) - s.start).sum();
    let f = file.clone();
    let on_disk = tokio::task::spawn_blocking(move || -> std::io::Result<u64> {
        f.sync_all()?;
        Ok(f.metadata()?.len())
    })
    .await
    .map_err(|e| AppError::internal(e.to_string()))??;

    let expected = if unknown_size { received } else { dl.total_bytes };
    if on_disk as i64 != expected || received != expected {
        return Ok(Outcome::Failed(
            AppError::new(
                ErrorCode::ErrChecksumSizeMismatch,
                format!("expected {expected} bytes, received {received}, on disk {on_disk}"),
            )
            .with_details(serde_json::json!({ "expected": expected, "received": received, "on_disk": on_disk })),
        ));
    }
    if unknown_size {
        let id2 = id.to_string();
        ctx.db
            .write(move |c| {
                c.execute("UPDATE downloads SET total_bytes=?2 WHERE id=?1", rusqlite::params![id2, expected])?;
                Ok(())
            })
            .await?;
    }
    Ok(Outcome::Completed)
}

/// Snapshot offsets, fsync file data, then commit one transactional checkpoint.
/// Snapshot-before-sync guarantees persisted offsets never exceed durable data.
async fn flush(ctx: &TransferCtx, id: &str, rts: &[Arc<SegRt>], file: &Arc<File>, unknown_size: bool) -> Result<i64> {
    let offs: Vec<(u32, i64)> = rts.iter().map(|s| (s.index, s.next.load(Ordering::Acquire))).collect();
    let f = file.clone();
    tokio::task::spawn_blocking(move || f.sync_data()).await.map_err(|e| AppError::internal(e.to_string()))??;
    let id2 = id.to_string();
    if unknown_size {
        let n: i64 = offs.iter().map(|(_, o)| *o).sum();
        ctx.db.write(move |c| repo::set_downloaded_bytes(c, &id2, n)).await?;
        Ok(n)
    } else {
        ctx.db.write(move |c| repo::checkpoint(c, &id2, &offs)).await
    }
}

struct WorkerArgs {
    source: Source,
    file: Arc<File>,
    seg: Arc<SegRt>,
    ranged: bool,
    item_bucket: Arc<TokenBucket>,
    throttle: Arc<Throttler>,
    cancel: CancellationToken,
}

async fn segment_worker(a: WorkerArgs) -> Result<()> {
    let WorkerArgs { source, file, seg, ranged, item_bucket, throttle, cancel } = a;
    let from = seg.next.load(Ordering::Acquire);
    if seg.end >= 0 && remaining_range(seg.start, seg.end, from).is_none() {
        return Ok(());
    }

    let mut stream: BoxStream<'static, Result<Bytes>> = match source {
        Source::Http { client, url, rctx } => {
            let mut rb = rctx.apply(client.get(&url));
            if ranged {
                rb = rb.header(RANGE, format!("bytes={}-{}", from, seg.end));
            }
            let resp = tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                r = rb.send() => r?,
            };
            let status = resp.status();
            if ranged {
                match status.as_u16() {
                    206 => {}
                    200 => return Err(AppError::new(ErrorCode::ErrRangeNotSupported, "server answered a range request with 200").with_details(serde_json::json!({ "http_status": 200, "effective_url": url }))),
                    _ => return Err(http::status_error(status, &url)),
                }
            } else if status.as_u16() != 200 {
                return Err(http::status_error(status, &url));
            }
            resp.bytes_stream().map(|r| r.map_err(|e| AppError::new(ErrorCode::ErrConnectionDropped, e.to_string()))).boxed()
        }
        Source::Ftp { target, proxy, timeouts } => {
            // One control + data connection per segment; REST positions the resume point.
            let conn = tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                c = FtpConn::open(&target, proxy.as_ref(), &timeouts) => c?,
            };
            let offset = if ranged { from as u64 } else { 0 };
            let data = tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                d = conn.retr(&target.path, offset) => d?,
            };
            let limit = if seg.end >= 0 { (seg.end + 1 - from) as u64 } else { u64::MAX };
            ReaderStream::with_capacity(data.take(limit), 64 * 1024).map(|r| r.map_err(|e| AppError::new(ErrorCode::ErrConnectionDropped, e.to_string()))).boxed()
        }
    };
    loop {
        let item = tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            it = stream.next() => it,
        };
        let mut chunk = match item {
            None => break,
            Some(Err(e)) => return Err(e),
            Some(Ok(c)) => c,
        };
        let off = seg.next.load(Ordering::Acquire);
        if seg.end >= 0 {
            let remaining = seg.end + 1 - off;
            if remaining <= 0 {
                break;
            }
            if chunk.len() as i64 > remaining {
                chunk.truncate(remaining as usize);
            }
        }
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            _ = throttle.acquire(&item_bucket, chunk.len()) => {}
        }
        let len = chunk.len() as i64;
        let f = file.clone();
        tokio::task::spawn_blocking(move || write_at(&f, &chunk, off as u64))
            .await
            .map_err(|e| AppError::internal(e.to_string()))??;
        // Publish progress only after the bytes are in the file.
        seg.next.store(off + len, Ordering::Release);
        if seg.end >= 0 && off + len > seg.end {
            break;
        }
    }
    if seg.end >= 0 && seg.next.load(Ordering::Acquire) <= seg.end {
        return Err(AppError::new(
            ErrorCode::ErrConnectionDropped,
            format!("stream ended at byte {} before range end {}", seg.next.load(Ordering::Acquire), seg.end),
        ));
    }
    Ok(())
}
