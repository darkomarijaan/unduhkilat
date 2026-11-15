//! Unduhan video via yt-dlp + ffmpeg (DEC-015, spec 2026-11-14): bentuk URL `video:`,
//! deteksi host video, argumen proses, pengurai progres, dan pemetaan error.
//!
//! Item video adalah unduhan biasa di tabel `downloads` dengan `original_url`
//! `video:<opsi>|<url asli>` — pola yang sama dengan awalan `server://`.

use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;

use crate::error::{AppError, ErrorCode, Result};

use super::transfer::{Outcome, TransferCtx};
use super::throttle::TokenBucket;

/// Bentuk hasil unduhan: video MP4 (batas tinggi opsional) atau audio MP3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoKind {
    Video { max_height: Option<u32> },
    AudioMp3 { kbps: u32 },
}

/// Link video yang sudah diurai dari awalan `video:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoRequest {
    pub url: String,
    pub kind: VideoKind,
}

/// Host situs video yang didukung yt-dlp (termasuk subdomain `www.`, `m.`, `music.`).
pub fn is_video_host(url: &str) -> bool {
    let Some(host) = url::Url::parse(url).ok().and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase())) else {
        return false;
    };
    let host = host.strip_prefix("www.").unwrap_or(&host);
    const HOSTS: &[&str] = &[
        "youtube.com", "youtu.be", "instagram.com", "facebook.com", "fb.watch",
        "tiktok.com", "x.com", "twitter.com", "vimeo.com", "dailymotion.com", "twitch.tv",
    ];
    HOSTS.iter().any(|h| host == *h || host.strip_suffix(h).is_some_and(|s| s.ends_with('.')))
}

/// Bungkus URL asli menjadi bentuk `video:...` (kualitas terbaik untuk video).
pub fn wrap(url: &str, kind: &VideoKind) -> String {
    let opt = match kind {
        VideoKind::Video { max_height: None } => "v=best".into(),
        VideoKind::Video { max_height: Some(h) } => format!("v={h}"),
        VideoKind::AudioMp3 { kbps } => format!("a=mp3-{kbps}"),
    };
    format!("video:{opt}|{url}")
}

/// `Some(wrap(url, terbaik))` bila host dikenal dan URL belum berawalan `video:`.
pub fn auto_wrap(url: &str) -> Option<String> {
    if parse(url).is_some() {
        return None;
    }
    is_video_host(url).then(|| wrap(url, &VideoKind::Video { max_height: None }))
}

/// Urai `video:<opsi>|<url>`; `|` pertama saja (URL asli boleh memuat `|`).
pub fn parse(original_url: &str) -> Option<VideoRequest> {
    let rest = original_url.strip_prefix("video:")?;
    let (opt, url) = rest.split_once('|')?;
    if url.is_empty() {
        return None;
    }
    let kind = if opt == "v=best" {
        VideoKind::Video { max_height: None }
    } else if let Some(Ok(h)) = opt.strip_prefix("v=").map(str::parse::<u32>) {
        VideoKind::Video { max_height: Some(h) }
    } else if let Some(Ok(k)) = opt.strip_prefix("a=mp3-").map(str::parse::<u32>) {
        VideoKind::AudioMp3 { kbps: k }
    } else {
        return None;
    };
    Some(VideoRequest { url: url.to_string(), kind })
}

/// Sampel satu baris progres yt-dlp (`--progress-template`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressSample {
    pub downloaded: i64,
    /// `-1` bila total belum diketahui (yt-dlp mencetak `N/A`).
    pub total: i64,
    pub speed: u64,
}

/// Nama berkas hasil di folder kerja, tanpa ekstensi (`-o out.%(ext)s`).
const STUB_NAME: &str = "out";

/// Nama sementara item video yang belum punya judul: unik, agar beberapa link tanpa
/// nama ke folder yang sama tidak saling bertabrakan. Diganti judul saat unduhan berjalan.
pub fn placeholder_name(kind: &VideoKind) -> String {
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
    match kind {
        VideoKind::Video { .. } => format!("video-{tag}.mp4"),
        VideoKind::AudioMp3 { .. } => format!("audio-{tag}.mp3"),
    }
}

/// `true` untuk nama sementara (bentuk lama `video.mp4`/`audio.mp3` atau [`placeholder_name`]).
pub fn is_placeholder_name(name: &str) -> bool {
    if name == "video.mp4" || name == "audio.mp3" {
        return true;
    }
    let tagged = |prefix: &str, ext: &str| {
        name.strip_prefix(prefix).and_then(|r| r.strip_suffix(ext)).is_some_and(|t| t.len() == 8 && t.bytes().all(|b| b.is_ascii_hexdigit()))
    };
    tagged("video-", ".mp4") || tagged("audio-", ".mp3")
}

/// Nilai `--proxy` dari setelan proxy aplikasi: Direct = `""` (paksa langsung, abaikan proxy
/// Windows), System = tidak diberikan (yt-dlp memakai proxy sistem), Manual = proxy itu sendiri
/// (HTTP maupun SOCKS) — tidak pernah diam-diam lewat jalur langsung.
pub fn proxy_arg(p: &crate::engine::proxy::ProxyConfig) -> Option<String> {
    use crate::engine::proxy::{split_scheme, ProxyMode};
    match p.mode {
        ProxyMode::Direct => Some(String::new()),
        ProxyMode::System => None,
        ProxyMode::Manual => {
            let (scheme, host) = split_scheme(&p.host);
            let enc = |s: &str| -> String {
                s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
            };
            let auth = match (&p.username, &p.password) {
                (Some(u), Some(pw)) => format!("{}:{}@", enc(u), enc(pw)),
                (Some(u), None) => format!("{}@", enc(u)),
                _ => String::new(),
            };
            Some(format!("{scheme}://{auth}{host}:{}", p.port))
        }
    }
}

/// Susun argumen `yt-dlp` untuk satu unduhan video/audio. `out_stub` adalah jalur
/// hasil TANPA ekstensi (yt-dlp menambahkan `.mp4`/`.mp3` sendiri). Tanpa ffmpeg,
/// video diambil sebagai SATU berkas jadi (tanpa penggabungan yang akan meninggalkan
/// video tanpa suara). URL selalu paling akhir, setelah `--`.
pub fn build_download_args(req: &VideoRequest, out_stub: &Path, ffmpeg_dir: &Path, has_ffmpeg: bool, proxy: Option<&str>, limit_kbps: u32) -> Vec<String> {
    let mut a = vec![
        "--newline".into(),
        "--progress-template".into(),
        "download:UK %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.speed)s".into(),
        "--no-playlist".into(),
        "--no-overwrites".into(),
        "--continue".into(),
        "-N".into(),
        "4".into(),
        "--ffmpeg-location".into(),
        ffmpeg_dir.to_string_lossy().into_owned(),
        "-o".into(),
        format!("{}.%(ext)s", out_stub.to_string_lossy()),
    ];
    match &req.kind {
        VideoKind::Video { max_height } => {
            let cap = max_height.map(|h| format!("[height<={h}]")).unwrap_or_default();
            a.push("-f".into());
            if has_ffmpeg {
                // MP4: utamakan H.264 + AAC; kalau resolusi itu hanya ada VP9/AV1, tetap digabung ke MP4.
                a.push(format!("bv*{cap}[vcodec^=avc1]+ba[acodec^=mp4a]/bv*{cap}+ba/b"));
                a.push("--merge-output-format".into());
                a.push("mp4".into());
            } else {
                a.push(format!("b{cap}[ext=mp4]/b{cap}/b"));
            }
        }
        VideoKind::AudioMp3 { kbps } => {
            a.push("-x".into());
            a.push("--audio-format".into());
            a.push("mp3".into());
            a.push("--audio-quality".into());
            a.push(format!("{kbps}K"));
        }
    }
    if let Some(p) = proxy {
        a.push("--proxy".into());
        a.push(p.into());
    }
    if limit_kbps > 0 {
        a.push("--limit-rate".into());
        a.push(format!("{limit_kbps}K"));
    }
    a.push("--".into());
    a.push(req.url.clone());
    a
}

/// Urai satu baris progres `UK <bytes> <total> <speed>`; baris lain diabaikan.
pub fn parse_progress_line(line: &str) -> Option<ProgressSample> {
    let rest = line.trim().strip_prefix("UK ")?;
    let mut it = rest.split_whitespace();
    let num = |s: Option<&str>| -> Option<i64> {
        let s = s?;
        if s.eq_ignore_ascii_case("n/a") { return Some(-1); }
        s.parse::<i64>().ok()
    };
    let downloaded = num(it.next())?;
    let total = num(it.next())?;
    let speed = it.next()?.parse::<u64>().ok()?;
    Some(ProgressSample { downloaded, total, speed })
}

/// Nama berkas akhir dari judul: disanitasi, dipotong ±120 karakter, ekstensi sesuai bentuk.
/// Judul bukan jalur: `/` dan `\` juga diganti `_` (bukan dipakai sebagai pemisah).
pub fn final_name(title: &str, kind: &VideoKind) -> String {
    let ext = match kind {
        VideoKind::Video { .. } => ".mp4",
        VideoKind::AudioMp3 { .. } => ".mp3",
    };
    let no_path: String = title.chars().map(|c| if c == '/' || c == '\\' { '_' } else { c }).collect();
    let clean = crate::engine::http::sanitize_file_name(&no_path);
    let stem: String = clean.chars().take(120).collect();
    format!("{stem}{ext}")
}

/// Tanda versi yt-dlp usang: layak coba perbarui lalu ulang sekali (spec §5.1).
pub fn is_stale_error(text: &str) -> bool {
    const SIGNS: &[&str] = &[
        "nsig extraction failed",
        "Unable to extract",
        "HTTP Error 403",
        "Sign in to confirm you're not a bot",
    ];
    SIGNS.iter().any(|s| text.contains(s))
}

/// Petakan keluaran error yt-dlp ke kode stabil (spec §5.4).
pub fn map_error(text: &str) -> AppError {
    let lower = text.to_lowercase();
    if lower.contains("sign in to confirm") || lower.contains("login") || lower.contains("private") {
        return AppError::new(ErrorCode::ErrVideoLoginRequired, "Perlu login; versi ini hanya mendukung konten publik");
    }
    if lower.contains("video unavailable") || lower.contains("not available") {
        return AppError::new(ErrorCode::ErrVideoExtractFailed, "video tidak tersedia");
    }
    let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("yt-dlp gagal");
    AppError::new(ErrorCode::ErrVideoExtractFailed, format!("yt-dlp: {last}"))
}

// ---- eksekusi ---------------------------------------------------------------

/// Metadata dari `yt-dlp -J` (tombol Periksa dan penamaan otomatis).
#[derive(Debug, Clone)]
pub struct VideoMeta {
    pub title: String,
    pub duration: Option<i64>,
    /// Tinggi resolusi unik, terbesar lebih dulu.
    pub heights: Vec<u32>,
}

#[cfg(windows)]
use std::os::windows::process::CommandExt as _;

/// Spawn proses (tanpa jendela) dengan stdout/stderr piped. `args[0]` adalah program.
fn spawn(args: &[String], work: &Path) -> Result<std::process::Child> {
    let mut cmd = std::process::Command::new(&args[0]);
    cmd.args(&args[1..]).current_dir(work);
    #[cfg(windows)]
    cmd.creation_flags(crate::tools::CREATE_NO_WINDOW);
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    cmd.spawn().map_err(|e| AppError::new(ErrorCode::ErrVideoToolsMissing, format!("cannot run {}: {e}", args[0])))
}

/// Argumen lengkap termasuk program di depan: `[exe, ...flags]`.
fn with_exe(exe: &Path, args: Vec<String>) -> Vec<String> {
    let mut v = vec![exe.to_string_lossy().into_owned()];
    v.extend(args);
    v
}

/// Kegagalan yt-dlp sebelum dipetakan: teks mentahnya dipakai untuk mengenali versi usang.
struct RawFailure(String);

impl RawFailure {
    fn into_error(self) -> AppError {
        map_error(&self.0)
    }
}

/// Jalankan `yt-dlp -J --no-playlist` dan urai JSON-nya. `proxy` mengikuti [`proxy_arg`].
pub async fn probe_metadata(exe: &Path, url: &str, proxy: Option<&str>) -> Result<VideoMeta> {
    probe_raw(exe, url, proxy).await.map_err(|e| match e {
        Ok(raw) => raw.into_error(),
        Err(app) => app,
    })
}

/// `Err(Ok(raw))` = yt-dlp gagal (teks mentah); `Err(Err(e))` = gagal menjalankan/mengurai.
async fn probe_raw(exe: &Path, url: &str, proxy: Option<&str>) -> std::result::Result<VideoMeta, std::result::Result<RawFailure, AppError>> {
    let mut args = vec![exe.to_string_lossy().into_owned(), "-J".into(), "--no-playlist".into()];
    if let Some(p) = proxy {
        args.push("--proxy".into());
        args.push(p.into());
    }
    args.push("--".into());
    args.push(url.into());
    let out = tokio::task::spawn_blocking(move || {
        let mut c = std::process::Command::new(&args[0]);
        c.args(&args[1..]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        #[cfg(windows)]
        c.creation_flags(crate::tools::CREATE_NO_WINDOW);
        c.output()
    })
    .await
    .map_err(|e| Err(AppError::internal(e.to_string())))?
    .map_err(|e| Err(AppError::new(ErrorCode::ErrVideoToolsMissing, format!("cannot run yt-dlp: {e}"))))?;
    if !out.status.success() {
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        return Err(Ok(RawFailure(text)));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| Err(AppError::new(ErrorCode::ErrMediaParseFailed, format!("metadata bukan JSON: {e}"))))?;
    let title = v["title"].as_str().unwrap_or("video").to_string();
    let duration = v["duration"].as_i64();
    let mut heights: Vec<u32> = v["formats"].as_array().map(|fs| fs.iter().filter_map(|f| f["height"].as_u64()).map(|h| h as u32).collect()).unwrap_or_default();
    heights.sort_unstable();
    heights.dedup();
    heights.reverse();
    Ok(VideoMeta { title, duration, heights })
}

/// Jalankan satu unduhan video di antrian (dipanggil `run_inner` untuk URL `video:`).
pub async fn run_video(ctx: &TransferCtx, id: &str, req: &VideoRequest, cancel: &CancellationToken, _bucket: std::sync::Arc<TokenBucket>) -> Result<Outcome> {
    // 1. Komponen harus ada.
    let tools_dir = ctx.cfg.tools_dir.clone().ok_or_else(|| AppError::new(ErrorCode::ErrVideoToolsMissing, "komponen video belum terpasang"))?;
    let exe = tools_dir.join(&ctx.cfg.video_ytdlp_name);
    if !exe.exists() {
        return Err(AppError::new(ErrorCode::ErrVideoToolsMissing, format!("{} tidak ditemukan", exe.display())));
    }

    // 2. Batas kecepatan: yang lebih kecil antara item dan global (0 = tanpa batas).
    let dl = ctx.db.read(|c| crate::repo::get_download(c, id))?;
    let global = ctx.db.read(|c| crate::repo::get_setting(c, crate::config::keys::GLOBAL_SPEED_LIMIT_KBPS))?.and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let limit = match (dl.speed_limit_kbps, global) {
        (0, g) => g,
        (a, 0) => a,
        (a, b) => a.min(b),
    };
    let proxy = proxy_arg(&ctx.proxy());
    let has_ffmpeg = tools_dir.join("ffmpeg.exe").exists();
    if matches!(req.kind, VideoKind::AudioMp3 { .. }) && !has_ffmpeg {
        // Konversi MP3 butuh ffmpeg; jangan biarkan yt-dlp menyimpan berkas audio lain diam-diam.
        return Err(AppError::new(ErrorCode::ErrVideoToolsMissing, "ffmpeg belum terpasang; MP3 butuh ffmpeg"));
    }

    // 3. Metadata untuk nama akhir (judul). Versi usang (mis. cek bot) → perbarui sekali, ulang.
    let target0 = PathBuf::from(&dl.target_path);
    let out_dir = target0.parent().ok_or_else(|| AppError::new(ErrorCode::ErrInvalidPath, "target has no parent"))?.to_path_buf();
    std::fs::create_dir_all(&out_dir).map_err(|e| AppError::new(ErrorCode::ErrPathUnwritable, e.to_string()))?;
    let mut stale_retried = false;
    let meta = loop {
        match probe_raw(&exe, &req.url, proxy.as_deref()).await {
            Ok(m) => break Some(m),
            Err(Ok(raw)) if is_stale_error(&raw.0) && !stale_retried => {
                stale_retried = true;
                let _ = crate::tools::maybe_auto_update(&ctx.db, &ctx.client(), &tools_dir, true).await;
            }
            Err(Ok(raw)) => {
                let e = raw.into_error();
                if e.code == ErrorCode::ErrVideoLoginRequired || e.message.contains("tidak tersedia") {
                    return Err(e);
                }
                break None; // metadata gagal: lanjut tanpa judul (nama dari DB)
            }
            Err(Err(_)) => break None,
        }
    };
    let placeholder = is_placeholder_name(&dl.file_name);
    let final_name = if placeholder {
        let title = meta.as_ref().map(|m| m.title.as_str()).unwrap_or("video");
        final_name(title, &req.kind)
    } else {
        dl.file_name.clone()
    };
    let final_path = out_dir.join(&final_name);
    if final_path.exists() {
        return Err(AppError::new(ErrorCode::ErrNameCollision, format!("{} already exists", final_path.display())));
    }

    // 4. Folder kerja tersembunyi + stub output.
    let work = out_dir.join(format!(".uk-video-{id}"));
    std::fs::create_dir_all(&work).map_err(|e| AppError::new(ErrorCode::ErrPathUnwritable, e.to_string()))?;
    let stub = work.join(STUB_NAME);

    // 5. Jalankan dengan satu kali ulang bila keluaran mentah menandai versi usang
    //    (dicek SEBELUM pemetaan: "Sign in to confirm you're not a bot" juga tampak seperti login).
    let result = loop {
        match run_once(ctx, id, req, &exe, &stub, has_ffmpeg, proxy.as_deref(), limit, &work, &final_path, cancel).await {
            Ok(Err(raw)) if is_stale_error(&raw.0) && !stale_retried => {
                stale_retried = true; // jaring pengaman: perbarui yt-dlp sekali, lalu ulang
                let _ = crate::tools::maybe_auto_update(&ctx.db, &ctx.client(), &tools_dir, true).await;
            }
            Ok(Err(raw)) => break Err(raw.into_error()),
            Ok(Ok(o)) => break Ok(o),
            Err(e) => break Err(e),
        }
    };

    // 6. Beres-beres. Berhenti (jeda/batal/ulang) menyimpan folder kerja: `.part` di dalamnya
    //    dilanjutkan yt-dlp (`--continue`) saat item dijalankan lagi. Batal dan ulang
    //    membersihkannya lewat manager ([`discard_work`]).
    if !matches!(result, Ok(Outcome::Stopped)) {
        std::fs::remove_dir_all(&work).ok();
    }
    result
}

/// Folder kerja item video (`<folder tujuan>/.uk-video-<id>`).
pub fn work_dir(target_path: &Path, id: &str) -> Option<PathBuf> {
    target_path.parent().map(|d| d.join(format!(".uk-video-{id}")))
}

/// Buang sisa unduhan video yang dihentikan (dipakai saat batal dan ulang dari awal).
pub fn discard_work(target_path: &Path, id: &str) {
    if let Some(w) = work_dir(target_path, id) {
        std::fs::remove_dir_all(w).ok();
    }
}

/// Satu eksekusi yt-dlp: spawn dalam Job Object (KILL_ON_JOB_CLOSE), progres per baris,
/// hasil dipindah ke nama akhir. `Ok(Err(raw))` = yt-dlp gagal; teks mentahnya dikembalikan
/// supaya penelepon bisa mengenali versi usang sebelum memetakannya ke kode error.
#[allow(clippy::too_many_arguments)]
async fn run_once(
    ctx: &TransferCtx,
    id: &str,
    req: &VideoRequest,
    exe: &Path,
    stub: &Path,
    has_ffmpeg: bool,
    proxy: Option<&str>,
    limit: u32,
    work: &Path,
    final_path: &Path,
    cancel: &CancellationToken,
) -> Result<std::result::Result<Outcome, RawFailure>> {
    let args = with_exe(exe, build_download_args(req, stub, ctx.cfg.tools_dir.as_deref().unwrap_or(work), has_ffmpeg, proxy, limit));
    let mut child = spawn(&args, work)?;
    let pid = child.id();

    // Job Object: seluruh pohon proses (termasuk ffmpeg) dibunuh saat guard drop.
    #[cfg(windows)]
    let _job = unsafe { JobGuard::assign(&child)? };

    // Pembaca stdout: tiap baris progres diteruskan via channel (tidak memblokir cancel).
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ProgressSample>();
    let stdout = child.stdout.take().expect("stdout piped");
    let reader = tokio::task::spawn_blocking(move || {
        use std::io::BufRead;
        let mut r = std::io::BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match r.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if let Some(p) = parse_progress_line(&line) {
                        if tx.send(p).is_err() {
                            break;
                        }
                    }
                }
            }
        }
    });
    // Pembaca stderr (teks error bila gagal).
    let stderr = child.stderr.take().expect("stderr piped");
    let err_task = tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut s = Vec::new();
        let mut e = stderr;
        let _ = e.read_to_end(&mut s);
        String::from_utf8_lossy(&s).into_owned()
    });

    // Tunggu proses selesai; sambil jalan, sebar progres.
    let wait = tokio::task::spawn_blocking(move || child.wait());
    let mut sum_downloaded = 0i64; // jumlah berkas-berkas yang sudah selesai (video lalu audio)
    let mut prev_part = 0i64; // downloaded berkas yang sedang berjalan
    let mut last_total = -1i64;
    let mut cancelled = false;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                cancelled = true;
                kill_tree(pid);
                break;
            }
            sample = rx.recv() => {
                let Some(p) = sample else { break }; // pipa tutup: proses hampir selesai
                if last_total >= 0 && p.total >= 0 && p.total != last_total && p.downloaded < prev_part {
                    sum_downloaded += prev_part; // berkas baru mulai (video -> audio)
                }
                last_total = p.total;
                prev_part = p.downloaded;
                let total = if p.total > 0 { sum_downloaded + p.total } else { -1 };
                let _ = ctx.events.send(super::EngineEvent::Progress {
                    id: id.to_string(), downloaded_bytes: sum_downloaded + prev_part, total_bytes: total,
                    current_speed_bps: p.speed, active_connections: 1, phase: None,
                });
            }
        }
    }
    let status = match wait.await {
        Ok(Ok(st)) => Some(st),
        _ => None,
    };
    let _ = reader.await;
    let err_text = err_task.await.unwrap_or_default();
    if cancelled {
        return Ok(Ok(Outcome::Stopped));
    }

    // Proses selesai normal: exit sukses DAN berkas hasil ada (bukan sukses diam).
    if !status.is_some_and(|s| s.success()) {
        return Ok(Err(RawFailure(err_text)));
    }
    let produced = find_produced(work, req)?;
    let Some(produced) = produced else {
        return Err(AppError::new(ErrorCode::ErrVideoExtractFailed, "yt-dlp selesai tetapi tidak ada berkas hasil"));
    };
    if final_path.exists() {
        return Err(AppError::new(ErrorCode::ErrNameCollision, format!("{} already exists", final_path.display())));
    }
    std::fs::rename(&produced, final_path).map_err(|e| AppError::new(ErrorCode::ErrPathUnwritable, e.to_string()))?;
    let size = std::fs::metadata(final_path).map(|m| m.len() as i64).unwrap_or(0);
    let (id2, t, f) = (id.to_string(), size, final_path.to_string_lossy().into_owned());
    let name2 = final_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    ctx.db.write(move |c| {
        crate::repo::set_downloaded_bytes(c, &id2, t)?;
        c.execute("UPDATE downloads SET total_bytes=?2, target_path=?3, file_name=?4 WHERE id=?1", rusqlite::params![id2, t, f, name2]).map(|_| ()).map_err(Into::into)
    }).await?;
    let _ = ctx.events.send(super::EngineEvent::Progress {
        id: id.to_string(), downloaded_bytes: size, total_bytes: size,
        current_speed_bps: 0, active_connections: 0, phase: Some("menggabungkan".into()),
    });
    Ok(Ok(Outcome::Completed))
}

/// Cari berkas hasil di folder kerja: tepat `out.mp4`/`out.mp3`. Potongan format yang belum
/// digabung (`out.f137.mp4`) dan `.part` bukan hasil — menerimanya berarti video tanpa suara.
fn find_produced(work: &Path, req: &VideoRequest) -> Result<Option<PathBuf>> {
    let want = match req.kind {
        VideoKind::Video { .. } => format!("{STUB_NAME}.mp4"),
        VideoKind::AudioMp3 { .. } => format!("{STUB_NAME}.mp3"),
    };
    let entries = std::fs::read_dir(work).map_err(|e| AppError::new(ErrorCode::ErrPathUnwritable, e.to_string()))?;
    Ok(entries.flatten().map(|e| e.path()).find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case(&want))))
}

/// Kill proses beserta anaknya (pohon proses) — tanpa jendela.
fn kill_tree(pid: u32) {
    #[cfg(windows)]
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(crate::tools::CREATE_NO_WINDOW)
        .output();
    #[cfg(not(windows))]
    let _ = pid;
}

/// Job Object KILL_ON_JOB_CLOSE: proses anak dimasukkan saat spawn; drop guard =
/// seluruh pohon dibunuh Windows (pengaman kedua di luar `taskkill`).
/// Raw pointer → kirim Send manual: hanya dipakai untuk Terminate/Close dari thread mana pun.
#[cfg(windows)]
struct JobGuard {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
unsafe impl Send for JobGuard {}

#[cfg(windows)]
impl JobGuard {
    unsafe fn assign(child: &std::process::Child) -> Result<Self> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects as jo;
        let handle = jo::CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if handle.is_null() {
            return Err(AppError::internal("CreateJobObjectW gagal"));
        }
        let mut info: jo::JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = jo::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = jo::SetInformationJobObject(handle, jo::JobObjectExtendedLimitInformation, &info as *const _ as *const std::ffi::c_void, std::mem::size_of::<jo::JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);
        if ok == 0 {
            CloseHandle(handle);
            return Err(AppError::internal("SetInformationJobObject gagal"));
        }
        let proc_handle = windows_sys::Win32::System::Threading::OpenProcess(windows_sys::Win32::System::Threading::PROCESS_SET_QUOTA | windows_sys::Win32::System::Threading::PROCESS_TERMINATE, 0, child.id());
        if proc_handle.is_null() {
            CloseHandle(handle);
            return Err(AppError::internal("OpenProcess gagal"));
        }
        let ok = jo::AssignProcessToJobObject(handle, proc_handle);
        CloseHandle(proc_handle);
        if ok == 0 {
            CloseHandle(handle);
            return Err(AppError::internal("AssignProcessToJobObject gagal"));
        }
        Ok(Self { handle })
    }
}

#[cfg(windows)]
impl Drop for JobGuard {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.handle, 1) };
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_end_with_the_video_url() {
        let req = VideoRequest { url: "https://youtu.be/1".into(), kind: VideoKind::Video { max_height: None } };
        let a = build_download_args(&req, Path::new("C:/t/w"), Path::new("C:/t/tools"), true, None, 0);
        assert_eq!(&a[a.len() - 2..], ["--".to_string(), "https://youtu.be/1".to_string()]);
    }

    #[test]
    fn args_without_ffmpeg_pick_a_single_file_format() {
        let req = VideoRequest { url: "https://youtu.be/1".into(), kind: VideoKind::Video { max_height: Some(720) } };
        let s = build_download_args(&req, Path::new("C:/t/w"), Path::new("C:/t/tools"), false, None, 0).join(" ");
        assert!(s.contains("-f b[height<=720][ext=mp4]/b[height<=720]/b"), "{s}");
        assert!(!s.contains("merge-output-format") && !s.contains("+ba"), "{s}");
    }

    #[test]
    fn proxy_arg_follows_the_proxy_mode() {
        use crate::engine::proxy::{ProxyConfig, ProxyMode};
        assert_eq!(proxy_arg(&ProxyConfig { mode: ProxyMode::Direct, ..Default::default() }).as_deref(), Some(""));
        assert_eq!(proxy_arg(&ProxyConfig { mode: ProxyMode::System, ..Default::default() }), None);
        let http = ProxyConfig { mode: ProxyMode::Manual, host: "10.0.0.1".into(), port: 8080, ..Default::default() };
        assert_eq!(proxy_arg(&http).as_deref(), Some("http://10.0.0.1:8080"));
        let socks = ProxyConfig {
            mode: ProxyMode::Manual, host: "socks5://h".into(), port: 1080,
            username: Some("u".into()), password: Some(zeroize::Zeroizing::new("p:w".into())),
        };
        assert_eq!(proxy_arg(&socks).as_deref(), Some("socks5://u:p%3Aw@h:1080"));
    }

    #[test]
    fn placeholder_names_are_unique_and_recognised() {
        let a = placeholder_name(&VideoKind::Video { max_height: None });
        let b = placeholder_name(&VideoKind::Video { max_height: None });
        assert_ne!(a, b);
        assert!(a.starts_with("video-") && a.ends_with(".mp4"), "{a}");
        assert!(placeholder_name(&VideoKind::AudioMp3 { kbps: 192 }).ends_with(".mp3"));
        for n in [a.as_str(), "video.mp4", "audio.mp3"] {
            assert!(is_placeholder_name(n), "{n}");
        }
        for n in ["Judul Video.mp4", "video-nothex1.mp4", "video.mkv", "my-video.mp4"] {
            assert!(!is_placeholder_name(n), "{n}");
        }
    }

    #[test]
    fn find_produced_skips_unmerged_format_parts() {
        let dir = std::env::temp_dir().join(format!("uk-produced-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let req = VideoRequest { url: "u".into(), kind: VideoKind::Video { max_height: None } };
        std::fs::write(dir.join("out.f137.mp4"), b"video only").unwrap();
        std::fs::write(dir.join("out.mp4.part"), b"partial").unwrap();
        assert_eq!(find_produced(&dir, &req).unwrap(), None);
        std::fs::write(dir.join("out.mp4"), b"merged").unwrap();
        assert_eq!(find_produced(&dir, &req).unwrap(), Some(dir.join("out.mp4")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn args_video_caps_height_and_merges_mp4() {
        let req = VideoRequest { url: "https://youtu.be/1".into(), kind: VideoKind::Video { max_height: Some(720) } };
        let a = build_download_args(&req, Path::new("C:/t/w"), Path::new("C:/t/tools"), true, None, 0);
        let s = a.join(" ");
        assert!(s.contains("-f bv*[height<=720]"), "{s}");
        assert!(s.contains("vcodec^=avc1"), "{s}");
        assert!(s.contains("--merge-output-format mp4"), "{s}");
        assert!(s.contains("--ffmpeg-location C:/t/tools"), "{s}");
        assert!(s.contains("--no-playlist --no-overwrites --continue"), "{s}");
        assert!(s.contains("-N 4"), "{s}");
        assert!(s.contains("-o C:/t/w.%(ext)s"), "{s}");
        assert!(!s.contains("--limit-rate"), "{s}");
    }

    #[test]
    fn args_audio_uses_mp3_kbps() {
        let req = VideoRequest { url: "https://youtu.be/1".into(), kind: VideoKind::AudioMp3 { kbps: 192 } };
        let a = build_download_args(&req, Path::new("C:/t/w"), Path::new("C:/t/tools"), true, None, 0);
        let s = a.join(" ");
        assert!(s.contains("-x --audio-format mp3 --audio-quality 192K"), "{s}");
        assert!(!s.contains("merge-output-format"), "{s}");
    }

    #[test]
    fn args_limit_rate_and_proxy_when_set() {
        let req = VideoRequest { url: "https://youtu.be/1".into(), kind: VideoKind::Video { max_height: None } };
        let a = build_download_args(&req, Path::new("C:/t/w"), Path::new("C:/t/tools"), true, Some("socks5://10.0.0.1:1080"), 500);
        let s = a.join(" ");
        assert!(s.contains("--proxy socks5://10.0.0.1:1080"), "{s}");
        assert!(s.contains("--limit-rate 500K"), "{s}");
    }

    #[test]
    fn progress_line_parses_and_ignores_other_lines() {
        let p = parse_progress_line("UK 500 1000 8000").unwrap();
        assert_eq!((p.downloaded, p.total, p.speed), (500, 1000, 8000));
        let p = parse_progress_line("UK 0 N/A 100").unwrap();
        assert_eq!((p.downloaded, p.total, p.speed), (0, -1, 100));
        assert!(parse_progress_line("[download] 45.2% of 10MiB").is_none());
        assert!(parse_progress_line("").is_none());
        assert!(parse_progress_line("UK x y z").is_none());
    }

    #[test]
    fn final_name_sanitizes_and_truncates() {
        assert_eq!(final_name("My Video", &VideoKind::Video { max_height: None }), "My Video.mp4");
        assert_eq!(final_name("a<b>:c\"d/e", &VideoKind::AudioMp3 { kbps: 128 }), "a_b__c_d_e.mp3");
        let long = final_name(&"x".repeat(300), &VideoKind::Video { max_height: None });
        assert_eq!(long.len(), 124, "120 karakter + .mp4");
        let windows = "g\\h:i*j?k|l<m>n\"o".replace('|', "_");
        let n = final_name(&windows, &VideoKind::Video { max_height: None });
        assert!(!n.contains(['\\', ':', '*', '?', '"', '<', '>']), "{n}");
    }

    #[test]
    fn is_stale_error_matches_samples() {
        for t in [
            "ERROR: nsig extraction failed: some junk",
            "ERROR: Unable to extract uploader id",
            "ERROR: HTTP Error 403: Forbidden",
            "Sign in to confirm you're not a bot",
        ] {
            assert!(is_stale_error(t), "{t}");
        }
        assert!(!is_stale_error("ERROR: Video unavailable"));
    }

    #[test]
    fn map_error_maps_login_unavailable_and_generic() {
        assert_eq!(map_error("Sign in to confirm you're not a bot").code, ErrorCode::ErrVideoLoginRequired);
        assert_eq!(map_error("This video is private").code, ErrorCode::ErrVideoLoginRequired);
        assert_eq!(map_error("ERROR: Video unavailable").code, ErrorCode::ErrVideoExtractFailed);
        let e = map_error("baris1\nbaris2\nERROR: detail terakhir");
        assert_eq!(e.code, ErrorCode::ErrVideoExtractFailed);
        assert!(e.message.contains("detail terakhir"), "{}", e.message);
    }

    #[test]
    fn parse_roundtrips_video_and_audio() {
        let v = parse("video:v=best|https://youtu.be/x?a|b").unwrap();
        assert_eq!(v.url, "https://youtu.be/x?a|b");
        assert_eq!(v.kind, VideoKind::Video { max_height: None });
        let h = parse("video:v=1080|https://youtube.com/watch?v=1").unwrap();
        assert_eq!(h.kind, VideoKind::Video { max_height: Some(1080) });
        let a = parse("video:a=mp3-192|https://www.tiktok.com/@u/video/1").unwrap();
        assert_eq!(a.kind, VideoKind::AudioMp3 { kbps: 192 });
    }

    #[test]
    fn parse_splits_on_the_first_pipe() {
        let v = parse("video:v=best|https://x/a?b=1|2").unwrap();
        assert_eq!(v.url, "https://x/a?b=1|2");
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse("video:").is_none());
        assert!(parse("video:v=best").is_none());
        assert!(parse("video:x|https://a").is_none());
        assert!(parse("video:a=mp3-abc|https://a").is_none());
        assert!(parse("https://youtube.com/watch?v=1").is_none());
    }

    #[test]
    fn is_video_host_matches_subdomains_and_scheme() {
        for u in [
            "https://www.youtube.com/watch?v=1",
            "https://m.youtube.com/watch?v=1",
            "https://music.youtube.com/watch?v=1",
            "https://youtu.be/abc",
            "http://www.instagram.com/reel/1/",
            "https://fb.watch/abc/",
            "https://x.com/u/status/1",
            "https://www.tiktok.com/@u/video/1",
        ] {
            assert!(is_video_host(u), "{u}");
        }
    }

    #[test]
    fn is_video_host_rejects_other_hosts() {
        for u in [
            "https://example.com/video?a=youtube.com",
            "https://notyoutube.com/watch?v=1",
            "https://evil.com/?u=https://youtube.com/watch?v=1",
            "bukan url",
            "",
        ] {
            assert!(!is_video_host(u), "{u}");
        }
    }

    #[test]
    fn auto_wrap_never_double_wraps() {
        assert_eq!(auto_wrap("https://youtu.be/abc").as_deref(), Some("video:v=best|https://youtu.be/abc"));
        assert!(auto_wrap("video:v=best|https://youtu.be/abc").is_none());
        assert!(auto_wrap("https://example.com/f.zip").is_none());
    }
}
