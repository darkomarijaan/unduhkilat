//! Unduhan video via yt-dlp + ffmpeg (DEC-015, spec 2026-11-14): bentuk URL `video:`,
//! deteksi host video, argumen proses, pengurai progres, dan pemetaan error.
//!
//! Item video adalah unduhan biasa di tabel `downloads` dengan `original_url`
//! `video:<opsi>|<url asli>` — pola yang sama dengan awalan `server://`.

use std::path::Path;

use crate::error::{AppError, ErrorCode};

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

/// Susun argumen `yt-dlp` untuk satu unduhan video/audio. `out_stub` adalah jalur
/// hasil TANPA ekstensi (yt-dlp menambahkan `.mp4`/`.mp3` sendiri).
pub fn build_download_args(req: &VideoRequest, out_stub: &Path, ffmpeg_dir: &Path, proxy: Option<&str>, limit_kbps: u32) -> Vec<String> {
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
            // MP4: utamakan H.264 + AAC; kalau resolusi itu hanya ada VP9/AV1, tetap digabung ke MP4.
            let cap = max_height.map(|h| format!("[height<={h}]")).unwrap_or_default();
            a.push("-f".into());
            a.push(format!("bv*{cap}[vcodec^=avc1]+ba[acodec^=mp4a]/bv*{cap}+ba/b"));
            a.push("--merge-output-format".into());
            a.push("mp4".into());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_video_caps_height_and_merges_mp4() {
        let req = VideoRequest { url: "https://youtu.be/1".into(), kind: VideoKind::Video { max_height: Some(720) } };
        let a = build_download_args(&req, Path::new("C:/t/w"), Path::new("C:/t/tools"), None, 0);
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
        let a = build_download_args(&req, Path::new("C:/t/w"), Path::new("C:/t/tools"), None, 0);
        let s = a.join(" ");
        assert!(s.contains("-x --audio-format mp3 --audio-quality 192K"), "{s}");
        assert!(!s.contains("merge-output-format"), "{s}");
    }

    #[test]
    fn args_limit_rate_and_proxy_when_set() {
        let req = VideoRequest { url: "https://youtu.be/1".into(), kind: VideoKind::Video { max_height: None } };
        let a = build_download_args(&req, Path::new("C:/t/w"), Path::new("C:/t/tools"), Some("socks5://10.0.0.1:1080"), 500);
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
