//! Komponen video (DEC-015): pengelolaan `yt-dlp.exe`, `ffmpeg.exe`, `ffprobe.exe`
//! di `<data>/tools` — unduhan atas persetujuan pengguna, verifikasi SHA-256,
//! dan pembaruan otomatis yt-dlp (ikut setelan `update_check_enabled`).

use crate::config::keys;
use crate::db::Db;
use crate::error::{AppError, ErrorCode, Result};
use std::path::{Path, PathBuf};

const YTDLP_LATEST: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";
/// Build LGPL win64 BtbN (rilis bertanggal, tidak bergerak) dengan encoder MP3 `libmp3lame`;
/// versi + SHA-256 dipin dari `checksums.sha256` rilis yang sama (spec §6). ±160 MB.
const FFMPEG_URL: &str = "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-15-12-40/ffmpeg-n8.1.3-35-gf1acb0a266-win64-lgpl-8.1.zip";
const FFMPEG_SHA256: &str = "272140baa96cbb9553eb57c03590e0b8d2c93ce352aa7f876c34fbc657536836";

/// Kumpulan jalur komponen video.
#[derive(Debug, Clone)]
pub struct Tools {
    pub dir: PathBuf,
}

impl Tools {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn yt_dlp(&self) -> PathBuf {
        self.dir.join("yt-dlp.exe")
    }

    pub fn ffmpeg(&self) -> PathBuf {
        self.dir.join("ffmpeg.exe")
    }

    pub fn ffprobe(&self) -> PathBuf {
        self.dir.join("ffprobe.exe")
    }
}

/// Urai isi `SHA2-256SUMS` yt-dlp: `<hash>  <nama>` per baris.
pub fn parse_sha256_sums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let (h, n) = l.trim().split_once(char::is_whitespace)?;
            let name = n.trim_start();
            let name = name.strip_prefix('*').unwrap_or(name);
            (h.len() == 64).then(|| (h.to_ascii_lowercase(), name.to_string()))
        })
        .collect()
}

/// `true` bila tag rilis terbaru (`YYYY.MM.DD[.n]`) lebih baru daripada versi terpasang.
pub fn is_ytdlp_newer(latest_tag: &str, installed: &str) -> bool {
    fn parts(v: &str) -> Option<(u32, u32, u32, u32)> {
        let mut it = v.trim().split('.');
        let y = it.next()?.parse().ok()?;
        let m = it.next()?.parse().ok()?;
        let d = it.next()?.parse().ok()?;
        let n: u32 = it.next().map(|s| s.parse().unwrap_or(0)).unwrap_or(0);
        it.next().is_none().then_some((y, m, d, n))
    }
    match (parts(latest_tag), parts(installed)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Tulis berkas secara atomik: tulis `.new`, lalu rename (rename Windows menolak yang ada).
pub(crate) fn write_atomic(target: &Path, bytes: &[u8]) -> Result<()> {
    let new = target.with_extension("new");
    std::fs::write(&new, bytes).map_err(|e| AppError::new(ErrorCode::ErrPathUnwritable, e.to_string()))?;
    for _ in 0..25 {
        if std::fs::rename(&new, target).is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Err(AppError::new(ErrorCode::ErrStorageWriteFailed, format!("cannot replace {}", target.display())))
}

/// Verifikasi SHA-256 lalu tulis atomik.
pub(crate) fn verify_and_write(target: &Path, bytes: &[u8], expected_lower_hex: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    let got = hex_lower(&h.finalize());
    if !got.eq_ignore_ascii_case(expected_lower_hex) {
        return Err(AppError::new(ErrorCode::ErrPayloadCorrupt, format!("checksum mismatch: got {got}, want {expected_lower_hex}")));
    }
    write_atomic(target, bytes)
}

fn hex_lower(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Tools {
    /// Versi terpasang `(yt-dlp, ffmpeg)`; `None` bila komponen tidak ada atau tidak menjawab.
    pub async fn versions(&self) -> (Option<String>, Option<String>) {
        let ytdlp = ytdlp_version(&self.yt_dlp()).await;
        // `ffmpeg version 7.1-... Copyright` → `7.1-...`
        let ffmpeg = run_version(&self.ffmpeg(), "-version", |l| {
            l.strip_prefix("ffmpeg version ").unwrap_or(l).split_whitespace().next().unwrap_or(l).to_string()
        })
        .await;
        (ytdlp, ffmpeg)
    }

    /// Unduh dan pasang kedua komponen (atas permintaan eksplisit pengguna):
    /// yt-dlp diverifikasi dengan `SHA2-256SUMS` dari rilis yang sama; ffmpeg dari
    /// build LGPL yang dipin. `on_progress` menerima jumlah byte yang sudah terunduh.
    pub async fn install(&self, client: &reqwest::Client, on_progress: &(dyn Fn(u64) + Send + Sync)) -> Result<()> {
        std::fs::create_dir_all(&self.dir).map_err(|e| AppError::new(ErrorCode::ErrPathUnwritable, e.to_string()))?;
        install_ytdlp(client, &self.yt_dlp(), on_progress).await?;
        install_ffmpeg(client, &self.dir, on_progress).await?;
        Ok(())
    }
}

/// Versi yt-dlp terpasang (`YYYY.MM.DD[.n]`). yt-dlp hanya mengenal `--version`;
/// `-version` dibaca sebagai opsi pendek dan gagal.
pub async fn ytdlp_version(exe: &Path) -> Option<String> {
    run_version(exe, "--version", |l| l.trim().to_string()).await
}

async fn run_version(exe: &Path, flag: &'static str, clean: impl Fn(&str) -> String) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let exe = exe.to_path_buf();
    let out = tokio::task::spawn_blocking(move || std::process::Command::new(exe).arg(flag).creation_flags(CREATE_NO_WINDOW).output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    s.lines().next().map(clean).filter(|l| !l.trim().is_empty())
}

/// `CREATE_NO_WINDOW`: tidak ada jendela konsol hitam saat komponen dijalankan.
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

async fn fetch(client: &reqwest::Client, url: &str, on_progress: &(dyn Fn(u64) + Send + Sync)) -> Result<Vec<u8>> {
    let resp = client
        .get(url)
        .header("User-Agent", "UnduhKilat-video-tools")
        .send()
        .await
        .map_err(|e| AppError::new(ErrorCode::ErrConnectionDropped, format!("pengambilan komponen gagal: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::new(ErrorCode::ErrConnectionDropped, format!("pengambilan komponen gagal: {e}")))?;
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| AppError::new(ErrorCode::ErrConnectionDropped, e.to_string()))?;
        body.extend_from_slice(&chunk);
        on_progress(body.len() as u64);
    }
    Ok(body)
}

async fn install_ytdlp(client: &reqwest::Client, target: &Path, on_progress: &(dyn Fn(u64) + Send + Sync)) -> Result<()> {
    let bytes = fetch(client, &format!("{YTDLP_LATEST}/yt-dlp.exe"), on_progress).await?;
    let sums = fetch(client, &format!("{YTDLP_LATEST}/SHA2-256SUMS"), on_progress).await?;
    let text = String::from_utf8_lossy(&sums);
    let want = parse_sha256_sums(&text)
        .into_iter()
        .find(|(_, n)| n == "yt-dlp.exe")
        .map(|(h, _)| h)
        .ok_or_else(|| AppError::new(ErrorCode::ErrPayloadCorrupt, "SHA2-256SUMS tidak memuat yt-dlp.exe"))?;
    verify_and_write(target, &bytes, &want)
}

async fn install_ffmpeg(client: &reqwest::Client, dir: &Path, on_progress: &(dyn Fn(u64) + Send + Sync)) -> Result<()> {
    let bytes = fetch(client, FFMPEG_URL, on_progress).await?;
    let zip_path = dir.join("ffmpeg.zip");
    verify_and_write(&zip_path, &bytes, FFMPEG_SHA256)?;
    drop(bytes);
    let r = extract_ffmpeg(&zip_path, dir);
    std::fs::remove_file(&zip_path).ok();
    r
}

/// Ambil `ffmpeg.exe` dan `ffprobe.exe` dari zip build. Di zip BtbN, `bin/` ada di bawah
/// folder bernama versi (`ffmpeg-n8.1.3-...-win64-lgpl-8.1/bin/`), jadi dicari menurut akhiran.
fn extract_ffmpeg(zip_path: &Path, dir: &Path) -> Result<()> {
    let bad = |e: &dyn std::fmt::Display| AppError::new(ErrorCode::ErrPayloadCorrupt, e.to_string());
    let f = std::fs::File::open(zip_path).map_err(|e| AppError::new(ErrorCode::ErrPathUnwritable, e.to_string()))?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| bad(&e))?;
    for name in ["ffmpeg.exe", "ffprobe.exe"] {
        let suffix = format!("/bin/{name}");
        let entry = archive
            .file_names()
            .find(|n| n.ends_with(&suffix) || *n == &suffix[1..])
            .map(str::to_string)
            .ok_or_else(|| AppError::new(ErrorCode::ErrPayloadCorrupt, format!("{name} tidak ada di zip ffmpeg")))?;
        let mut src = archive.by_name(&entry).map_err(|e| bad(&e))?;
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut src, &mut buf).map_err(|e| bad(&e))?;
        write_atomic(&dir.join(name), &buf)?;
    }
    Ok(())
}

/// Cek pembaruan yt-dlp. `force=false` menghormati setelan "Cek pembaruan" dan jeda
/// 24 jam; `force=true` dipicu unduhan yang gagal karena versi usang (jaring pengaman).
/// `Ok(true)` berarti komponen baru terpasang.
pub async fn maybe_auto_update(db: &Db, client: &reqwest::Client, dir: &Path, force: bool) -> Result<bool> {
    let now_epoch = chrono::Utc::now().timestamp();
    if !force {
        let enabled = db.read(|c| crate::repo::get_setting(c, keys::UPDATE_CHECK))?.map(|v| v != "0" && v != "false").unwrap_or(true);
        if !enabled {
            return Ok(false);
        }
        let last: i64 = db.read(|c| crate::repo::get_setting(c, keys::VIDEO_TOOLS_LAST_CHECK))?.and_then(|v| v.parse().ok()).unwrap_or(0);
        if now_epoch - last < 24 * 3600 {
            return Ok(false);
        }
    }
    let tools = Tools::new(dir);
    let Some(installed) = ytdlp_version(&tools.yt_dlp()).await else {
        return Ok(false); // belum terpasang: tidak ada yang diperbarui
    };
    let tag = fetch_latest_tag(client).await?;
    db.write(move |c| crate::repo::set_setting(c, keys::VIDEO_TOOLS_LAST_CHECK, &now_epoch.to_string())).await?;
    if !is_ytdlp_newer(&tag, &installed) {
        return Ok(false);
    }
    install_ytdlp(client, &tools.yt_dlp(), &|_| {}).await?;
    Ok(true)
}

/// Tag `YYYY.MM.DD[.n]` dari rilis yt-dlp terbaru.
async fn fetch_latest_tag(client: &reqwest::Client) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct Rel {
        tag_name: String,
    }
    let text = client
        .get("https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest")
        .header("User-Agent", "UnduhKilat-video-tools")
        .send()
        .await
        .map_err(|e| AppError::new(ErrorCode::ErrConnectionDropped, e.to_string()))?
        .error_for_status()
        .map_err(|e| AppError::new(ErrorCode::ErrConnectionDropped, e.to_string()))?
        .text()
        .await
        .map_err(|e| AppError::new(ErrorCode::ErrConnectionDropped, e.to_string()))?;
    let rel: Rel = serde_json::from_str(&text).map_err(|e| AppError::new(ErrorCode::ErrPayloadCorrupt, e.to_string()))?;
    Ok(rel.tag_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sha256_sums_parses_real_format() {
        let t = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 *yt-dlp.exe\n\
                 bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  SHA2-256SUMS\n\
                 not a hash  x.exe\n";
        let v = parse_sha256_sums(t);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], ("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(), "yt-dlp.exe".into()));
    }

    #[test]
    fn is_ytdlp_newer_compares_date_tags() {
        assert!(is_ytdlp_newer("2026.11.10", "2026.10.30"));
        assert!(is_ytdlp_newer("2026.11.10.1", "2026.11.10"));
        assert!(!is_ytdlp_newer("2026.11.10", "2026.11.10"));
        assert!(!is_ytdlp_newer("2026.10.30", "2026.11.10"));
        assert!(!is_ytdlp_newer("sampah", "2026.11.10"));
        assert!(!is_ytdlp_newer("2026.11.10", "versi-rusak"));
    }

    #[test]
    fn ffmpeg_pin_is_a_dated_lgpl_build() {
        assert!(FFMPEG_URL.contains("/releases/download/autobuild-"), "a dated tag, never `latest`");
        assert!(FFMPEG_URL.ends_with("-win64-lgpl-8.1.zip"));
        assert_eq!(FFMPEG_SHA256.len(), 64);
        assert!(FFMPEG_SHA256.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn extract_ffmpeg_finds_bin_under_the_versioned_folder() {
        let dir = std::env::temp_dir().join(format!("uk-ffz-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("f.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
            let opt = zip::write::SimpleFileOptions::default();
            for (n, body) in [("ffmpeg-n8.1-win64-lgpl-8.1/bin/ffmpeg.exe", "F"), ("ffmpeg-n8.1-win64-lgpl-8.1/bin/ffprobe.exe", "P"), ("ffmpeg-n8.1-win64-lgpl-8.1/bin/ffplay.exe", "X")] {
                w.start_file(n, opt).unwrap();
                std::io::Write::write_all(&mut w, body.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        extract_ffmpeg(&zip_path, &dir).unwrap();
        assert_eq!(std::fs::read(dir.join("ffmpeg.exe")).unwrap(), b"F");
        assert_eq!(std::fs::read(dir.join("ffprobe.exe")).unwrap(), b"P");
        assert!(!dir.join("ffplay.exe").exists(), "only what yt-dlp needs");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn ytdlp_version_uses_double_dash_flag() {
        let dir = std::env::temp_dir().join(format!("uk-ver-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("v.cmd");
        // Like real yt-dlp: only `--version` prints the version; `-version` is an error.
        std::fs::write(&exe, "@echo off\r\nif \"%~1\"==\"--version\" (\r\n  echo 2026.11.10\r\n  exit /b 0\r\n)\r\necho bad option 1>&2\r\nexit /b 2\r\n").unwrap();
        assert_eq!(ytdlp_version(&exe).await.as_deref(), Some("2026.11.10"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tools_paths_lay_under_dir() {
        let t = Tools::new("C:/data/tools");
        assert_eq!(t.yt_dlp(), PathBuf::from("C:/data/tools/yt-dlp.exe"));
        assert_eq!(t.ffmpeg(), PathBuf::from("C:/data/tools/ffmpeg.exe"));
        assert_eq!(t.ffprobe(), PathBuf::from("C:/data/tools/ffprobe.exe"));
    }

    #[test]
    fn verify_and_write_checks_hash_then_writes() {
        let dir = std::env::temp_dir().join(format!("uk-tools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("a.exe");
        let sha = hex_lower(&{
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(b"payload");
            h.finalize()
        });
        assert!(verify_and_write(&target, b"payload", &sha).is_ok());
        assert_eq!(std::fs::read(&target).unwrap(), b"payload");
        assert!(verify_and_write(&target, b"payload", "nope").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_atomic_replaces_existing_file() {
        let dir = std::env::temp_dir().join(format!("uk-tools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("b.exe");
        std::fs::write(&target, b"old").unwrap();
        write_atomic(&target, b"new").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        std::fs::remove_dir_all(&dir).ok();
    }
}
