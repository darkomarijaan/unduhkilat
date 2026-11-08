//! Fast copy of a whole folder between two SSH servers as ONE stream:
//! `tar -cf -` on the source is piped through this computer into `tar -xf -` on the destination.
//!
//! Per-file copying needs several round trips per file (stat, open, write, close, verify), which
//! dominates for thousands of small files. One tar stream has none of that. The price: an
//! interrupted stream cannot resume in the middle, so a folder of a few large files keeps the
//! per-file copy (see [`prefer_stream`]), and resuming a stream starts it again.
//!
//! Safety: the destination folder must not exist yet. It is created by this transfer, and only
//! that folder (marked in the item's `effective_url`) is ever written into, so nothing that was on
//! the server before is overwritten (`ERR_NAME_COLLISION` contract).

use super::remote::{ensure_remote_dir, Meter, RemoteRef, IDLE};
use super::throttle::TokenBucket;
use super::transfer::{Outcome, TransferCtx};
use crate::error::{AppError, ErrorCode, Result};
use crate::repo;
use crate::server::exec;
use crate::server::fs::{join_remote, RemoteFs};
use russh::ChannelMsg;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Fewer files than this: per-file copying is fast enough and resumes file by file.
pub const STREAM_MIN_FILES: usize = 50;
/// Average file larger than this: the folder is mostly big files, where per-file resume matters
/// more than per-file overhead.
pub const STREAM_MAX_AVG_BYTES: u64 = 4 * 1024 * 1024;
/// Marks a stream item whose destination folder was created by the item itself.
const CREATED_MARK: &str = "&created=1";
const STDERR_CAP: usize = 16 * 1024;

/// Whether a folder of `files` files totalling `bytes` should be copied as one stream.
pub fn prefer_stream(files: usize, bytes: u64) -> bool {
    files >= STREAM_MIN_FILES && bytes / (files.max(1) as u64) <= STREAM_MAX_AVG_BYTES
}

/// Approximate size of the tar stream for progress: a 512-byte header per entry (two more blocks
/// for a name too long for the header), file data padded to 512 bytes, and the end blocks
/// rounded up to tar's 10 KiB records.
pub fn tar_size_estimate<'a>(files: impl IntoIterator<Item = (&'a str, u64)>, dirs: usize) -> i64 {
    let mut n: u64 = (dirs as u64 + 1) * 512;
    for (rel, size) in files {
        n += 512 + size.div_ceil(512) * 512;
        if rel.len() > 99 {
            n += 1024;
        }
    }
    n += 1024;
    (n.div_ceil(10_240) * 10_240) as i64
}

/// The command that writes the folder's contents as a tar stream to standard output.
pub fn source_command(dir: &str) -> String {
    format!("tar -C {} -cf - .", exec::quote(dir))
}

/// The command that unpacks a tar stream from standard input into `dir`.
pub fn dest_command(dir: &str) -> String {
    format!("tar -C {} -xf -", exec::quote(dir))
}

/// True when `fs` is an SSH connection whose account may run `tar`.
pub async fn can_stream(fs: &mut dyn RemoteFs) -> bool {
    let Some(ssh) = fs.ssh() else { return false };
    matches!(exec::run(ssh, "command -v tar", Duration::from_secs(15)).await, Ok(o) if o.status == Some(0))
}

fn refused(side: &str) -> AppError {
    AppError::new(ErrorCode::ErrRemoteFs, format!("Server {side} menolak menjalankan tar. Salin ulang folder ini; tanpa tar folder disalin per berkas."))
}

fn push_capped(buf: &mut Vec<u8>, data: &[u8]) {
    let room = STDERR_CAP.saturating_sub(buf.len());
    buf.extend_from_slice(&data[..data.len().min(room)]);
}

#[allow(clippy::too_many_arguments)]
pub async fn run(ctx: &TransferCtx, id: &str, r: &RemoteRef, src: &mut dyn RemoteFs, dst: &mut dyn RemoteFs, cancel: &CancellationToken, bucket: &Arc<TokenBucket>) -> Result<Outcome> {
    let dl = ctx.db.read(|c| repo::get_download(c, id))?;
    let (_, dst_parent) = r.dest.clone().ok_or_else(|| AppError::new(ErrorCode::ErrInvalidUrlScheme, "Tautan salinan folder tidak punya tujuan."))?;
    let name = r.path.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
    if name.is_empty() {
        return Err(AppError::new(ErrorCode::ErrInvalidPath, "Nama folder sumber kosong."));
    }
    let target = join_remote(&dst_parent, &name);
    if src.ssh().is_none() || dst.ssh().is_none() {
        return Err(AppError::new(ErrorCode::ErrRemoteFs, "Salin folder cepat butuh sambungan SSH di kedua server."));
    }

    // Claim the destination folder: only a folder this transfer created is ever written into.
    if !dl.effective_url.ends_with(CREATED_MARK) {
        ensure_remote_dir(dst, &dst_parent).await?;
        match dst.stat(&target).await {
            Ok(_) => return Err(AppError::new(ErrorCode::ErrNameCollision, format!("{target} sudah ada di server tujuan; isinya tidak ditimpa."))),
            Err(e) if e.code == ErrorCode::ErrNotFound => {}
            Err(e) => return Err(e),
        }
        dst.mkdir(&target).await?;
        let (id2, url, total) = (id.to_string(), format!("{}{CREATED_MARK}", dl.original_url), dl.total_bytes);
        ctx.db.write(move |c| repo::update_probe_result(c, &id2, &url, total, None, false)).await?;
    }

    let (src_ssh, dst_ssh) = (src.ssh().expect("checked above"), dst.ssh().expect("checked above"));
    let mut src_ch = exec::open(src_ssh, &source_command(&r.path)).await?;
    let (mut dst_rd, dst_wr) = exec::open(dst_ssh, &dest_command(&target)).await?.split();

    let estimate = dl.total_bytes.max(1);
    let mut sent: i64 = 0;
    let mut meter = Meter::new(0);
    let (mut src_err, mut dst_err) = (Vec::new(), Vec::new());
    let (mut src_status, mut dst_status) = (None, None);
    let mut src_open = true;
    let mut write_failed = false;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                let _ = src_ch.close().await;
                let _ = dst_wr.close().await;
                meter.save(ctx, id, sent).await;
                return Ok(Outcome::Stopped);
            }
            m = tokio::time::timeout(IDLE, src_ch.wait()), if src_open => match m {
                Err(_) => {
                    let _ = src_ch.close().await;
                    let _ = dst_wr.close().await;
                    return Err(AppError::new(ErrorCode::ErrConnectionDropped, "Server sumber berhenti mengirim data. Lanjutkan untuk mengulang salinan folder."));
                }
                Ok(Some(ChannelMsg::Data { data })) => {
                    let n = data.len();
                    ctx.throttle.acquire(bucket, n).await;
                    match tokio::time::timeout(IDLE, dst_wr.data_bytes(data.to_vec())).await {
                        Ok(Ok(())) => {
                            sent += n as i64;
                            meter.tick(ctx, id, sent, estimate.max(sent)).await;
                        }
                        // The destination tar stopped (its reason arrives on its own channel).
                        _ => {
                            write_failed = true;
                            src_open = false;
                            let _ = src_ch.close().await;
                        }
                    }
                }
                Ok(Some(ChannelMsg::ExtendedData { data, .. })) => push_capped(&mut src_err, &data),
                Ok(Some(ChannelMsg::ExitStatus { exit_status })) => src_status = Some(exit_status),
                Ok(Some(ChannelMsg::Failure)) => {
                    let _ = dst_wr.close().await;
                    return Err(refused("sumber"));
                }
                Ok(Some(ChannelMsg::Close)) | Ok(None) => {
                    src_open = false;
                    let _ = dst_wr.eof().await;
                }
                Ok(Some(_)) => {}
            },
            m = tokio::time::timeout(if src_open { Duration::MAX } else { IDLE }, dst_rd.wait()) => match m {
                Err(_) => return Err(AppError::new(ErrorCode::ErrConnectionDropped, "Server tujuan tidak mengonfirmasi akhir salinan (kehabisan waktu).")),
                Ok(Some(ChannelMsg::ExtendedData { data, .. })) => push_capped(&mut dst_err, &data),
                Ok(Some(ChannelMsg::ExitStatus { exit_status })) => dst_status = Some(exit_status),
                Ok(Some(ChannelMsg::Failure)) => {
                    let _ = src_ch.close().await;
                    return Err(refused("tujuan"));
                }
                Ok(Some(ChannelMsg::Close)) | Ok(None) => break,
                Ok(Some(_)) => {}
            },
        }
    }
    if src_open {
        // The destination ended before the source finished sending.
        let _ = src_ch.close().await;
        write_failed = true;
    }
    meter.save(ctx, id, sent).await;

    if dst_status != Some(0) || write_failed {
        let why = exec::stderr_tail(&dst_err);
        let why = if why.is_empty() { "tanpa keterangan".to_string() } else { why };
        return Err(AppError::new(ErrorCode::ErrConnectionDropped, format!("tar di server tujuan gagal: {why}. Lanjutkan untuk mengulang salinan folder.")));
    }
    // GNU tar exits 1 when a file changed while it was read: the copy is complete but that file
    // may be a mix of old and new content. 2 means files could not be read at all.
    match src_status {
        Some(0) | Some(1) => {}
        _ => {
            let why = exec::stderr_tail(&src_err);
            let why = if why.is_empty() { "tanpa keterangan".to_string() } else { why };
            return Err(AppError::new(ErrorCode::ErrRemoteFs, format!("Sebagian isi folder tidak terbaca di server sumber: {why}")));
        }
    }
    let (id2, url) = (id.to_string(), ctx.db.read(|c| repo::get_download(c, id))?.effective_url);
    ctx.db.write(move |c| repo::update_probe_result(c, &id2, &url, sent, None, false)).await?;
    Ok(Outcome::Completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn many_small_files_stream_while_few_or_big_files_do_not() {
        assert!(prefer_stream(11_000, 11_000 * 40_000));
        assert!(!prefer_stream(10, 10 * 1_000), "a handful of files is copied per file");
        assert!(!prefer_stream(200, 200 * 50 * 1024 * 1024), "big files keep per-file resume");
        assert!(prefer_stream(50, 0), "empty files still benefit");
    }

    #[test]
    fn tar_estimate_counts_headers_padding_and_records() {
        assert_eq!(tar_size_estimate([], 0), 10_240);
        // header + 1 block of data + root dir header + end blocks = 2560, rounded to one record.
        assert_eq!(tar_size_estimate([("a.txt", 5)], 0), 10_240);
        let n = tar_size_estimate((0..100).map(|_| ("f", 1000u64)), 3);
        assert_eq!(n % 10_240, 0);
        assert!(n >= 100 * (512 + 1024));
    }

    #[test]
    fn commands_quote_paths() {
        assert_eq!(source_command("/srv/data saya"), "tar -C '/srv/data saya' -cf - .");
        assert_eq!(dest_command("/x/it's"), r"tar -C '/x/it'\''s' -xf -");
    }
}
