//! Unduhan video via yt-dlp + ffmpeg (DEC-015, spec 2026-11-14): bentuk URL `video:`,
//! deteksi host video, argumen proses, pengurai progres, dan pemetaan error.
//!
//! Item video adalah unduhan biasa di tabel `downloads` dengan `original_url`
//! `video:<opsi>|<url asli>` — pola yang sama dengan awalan `server://`.

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

#[cfg(test)]
mod tests {
    use super::*;

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
