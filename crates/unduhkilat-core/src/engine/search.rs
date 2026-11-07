//! File search across public indexes. Torrent hits become magnet links or `.torrent`
//! URLs that the user then ticks and queues like any other download.
//!
//! Privacy: a query leaves the machine only when the user presses Search, and only to
//! the providers they ticked, through the app's own HTTP client (so the configured
//! proxy applies). Nothing here downloads content; every response is untrusted text
//! (size-capped, hashes validated, titles stripped of control characters).
//!
//! Sources (all queried over HTTPS):
//! * Internet Archive: `advancedsearch.php` JSON; each item's `_archive.torrent`.
//! * Academic Torrents: no search API exists and scraping `browse.php` is discouraged by
//!   the site, so the official `rss.xml` feed (recent entries) is filtered locally.
//! * apibay (The Pirate Bay) JSON and Nyaa RSS: general indexes that list copyrighted
//!   material. They are opt-in and the user decided to include them (DEC-009).

use super::http::sanitize_file_name;
use crate::error::{AppError, ErrorCode, Result};
use futures_util::StreamExt;
use serde::Serialize;
use std::time::Duration;

const MAX_BODY: usize = 24 * 1024 * 1024;
const PER_PROVIDER_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_HITS_PER_PROVIDER: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Provider {
    InternetArchive,
    AcademicTorrents,
    Apibay,
    Nyaa,
}

pub const ALL_PROVIDERS: [Provider; 4] = [Provider::InternetArchive, Provider::AcademicTorrents, Provider::Apibay, Provider::Nyaa];

impl Provider {
    pub fn name(&self) -> &'static str {
        match self {
            Provider::InternetArchive => "Internet Archive",
            Provider::AcademicTorrents => "Academic Torrents",
            Provider::Apibay => "apibay",
            Provider::Nyaa => "Nyaa",
        }
    }
    /// Settings key holding "1"/"0".
    pub fn setting_key(&self) -> &'static str {
        match self {
            Provider::InternetArchive => "search_provider_internet_archive",
            Provider::AcademicTorrents => "search_provider_academic_torrents",
            Provider::Apibay => "search_provider_apibay",
            Provider::Nyaa => "search_provider_nyaa",
        }
    }
    /// General indexes are opt-in; the two legal catalogues are on by default.
    pub fn default_on(&self) -> bool {
        matches!(self, Provider::InternetArchive | Provider::AcademicTorrents)
    }
}

/// How a hit is added to the queue.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub enum Source {
    Magnet(String),
    /// `http(s)` URL of a `.torrent` file.
    TorrentUrl(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub provider: Provider,
    pub title: String,
    pub size: Option<i64>,
    pub seeders: Option<u32>,
    pub leechers: Option<u32>,
    pub source: Source,
    /// Human-facing page for the item, when the provider has one.
    pub page: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SearchOutcome {
    pub hits: Vec<Hit>,
    /// One entry per provider that failed; the others still return results.
    pub errors: Vec<(Provider, String)>,
}

/// Base URLs, overridable so tests can point at a local server.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub internet_archive: String,
    pub academic_rss: String,
    pub apibay: String,
    pub nyaa: String,
    pub archive_download: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            internet_archive: "https://archive.org/advancedsearch.php".into(),
            academic_rss: "https://academictorrents.com/rss.xml".into(),
            apibay: "https://apibay.org/q.php".into(),
            nyaa: "https://nyaa.si/".into(),
            archive_download: "https://archive.org/download".into(),
        }
    }
}

// ---- helpers --------------------------------------------------------------------

pub fn clean_query(q: &str) -> Result<String> {
    let q: String = q.split_whitespace().collect::<Vec<_>>().join(" ");
    let n = q.chars().count();
    if n < 2 {
        return Err(AppError::invalid("type at least 2 characters to search"));
    }
    if n > 200 {
        return Err(AppError::invalid("search text is too long (200 characters max)"));
    }
    Ok(q)
}

fn tidy_title(s: &str) -> String {
    let t: String = s.chars().filter(|c| !c.is_control()).collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    t.chars().take(300).collect()
}

fn valid_hash(h: &str) -> Option<String> {
    (h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit()) && h.bytes().any(|b| b != b'0')).then(|| h.to_ascii_lowercase())
}

fn magnet(hash: &str, title: &str) -> String {
    let dn: String = url::form_urlencoded::byte_serialize(title.as_bytes()).collect();
    format!("magnet:?xt=urn:btih:{hash}&dn={dn}")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#34;", "\"").replace("&#39;", "'").replace("&apos;", "'").replace("&amp;", "&")
}

/// Text of the first `<tag>..</tag>` in `block` (CDATA tolerated).
fn xml_tag(block: &str, tag: &str) -> Option<String> {
    // Accept `<tag>` and `<tag attr="..">`.
    let start = block.match_indices(&format!("<{tag}")).find(|(i, _)| matches!(block.as_bytes().get(i + tag.len() + 1), Some(b'>') | Some(b' ')))?.0;
    let s = start + block[start..].find('>')? + 1;
    let e = block[s..].find(&format!("</{tag}>"))? + s;
    let raw = block[s..e].trim();
    let raw = raw.strip_prefix("<![CDATA[").and_then(|r| r.strip_suffix("]]>")).unwrap_or(raw);
    Some(xml_unescape(raw))
}

fn xml_items(xml: &str) -> impl Iterator<Item = &str> {
    xml.split("<item>").skip(1).filter_map(|b| b.split("</item>").next())
}

/// "1.4 GiB" / "512 KiB" / "700 MiB" -> bytes.
pub fn parse_human_size(s: &str) -> Option<i64> {
    let s = s.trim();
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let n: f64 = s[..split].parse().ok()?;
    let mult = match s[split..].trim().to_ascii_lowercase().as_str() {
        "b" | "bytes" => 1.0,
        "kib" | "kb" => 1024.0,
        "mib" | "mb" => 1024.0 * 1024.0,
        "gib" | "gb" => 1024.0 * 1024.0 * 1024.0,
        "tib" | "tb" => 1024.0_f64.powi(4),
        _ => return None,
    };
    let v = n * mult;
    (v.is_finite() && v >= 0.0 && v < 9.0e18).then_some(v as i64)
}

// ---- parsers (pure, unit-tested) ---------------------------------------------------

pub fn parse_internet_archive(json: &str, download_base: &str) -> Vec<Hit> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    let Some(docs) = v.pointer("/response/docs").and_then(|d| d.as_array()) else { return Vec::new() };
    docs.iter()
        .filter_map(|d| {
            let id = d.get("identifier")?.as_str()?;
            // The identifier ends up in a URL path: keep it to the characters archive.org allows.
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) {
                return None;
            }
            let title = tidy_title(d.get("title").and_then(|t| t.as_str()).unwrap_or(id));
            Some(Hit {
                provider: Provider::InternetArchive,
                title,
                size: d.get("item_size").and_then(|s| s.as_i64()),
                seeders: None,
                leechers: None,
                source: Source::TorrentUrl(format!("{}/{id}/{id}_archive.torrent", download_base.trim_end_matches('/'))),
                page: Some(format!("https://archive.org/details/{id}")),
            })
        })
        .take(MAX_HITS_PER_PROVIDER)
        .collect()
}

pub fn parse_apibay(json: &str) -> Vec<Hit> {
    let Ok(v) = serde_json::from_str::<Vec<serde_json::Value>>(json) else { return Vec::new() };
    let num = |x: &serde_json::Value, k: &str| x.get(k).and_then(|n| n.as_str().and_then(|s| s.parse::<i64>().ok()).or_else(|| n.as_i64()));
    v.iter()
        .filter_map(|x| {
            // "No results" is reported as a fake row with an all-zero hash.
            let hash = valid_hash(x.get("info_hash")?.as_str()?)?;
            let title = tidy_title(x.get("name")?.as_str()?);
            if title.is_empty() {
                return None;
            }
            Some(Hit {
                provider: Provider::Apibay,
                source: Source::Magnet(magnet(&hash, &title)),
                title,
                size: num(x, "size"),
                seeders: num(x, "seeders").map(|n| n.max(0) as u32),
                leechers: num(x, "leechers").map(|n| n.max(0) as u32),
                page: None,
            })
        })
        .take(MAX_HITS_PER_PROVIDER)
        .collect()
}

pub fn parse_nyaa(xml: &str) -> Vec<Hit> {
    xml_items(xml)
        .filter_map(|it| {
            let hash = valid_hash(&xml_tag(it, "nyaa:infoHash")?)?;
            let title = tidy_title(&xml_tag(it, "title")?);
            Some(Hit {
                provider: Provider::Nyaa,
                source: Source::Magnet(magnet(&hash, &title)),
                title,
                size: xml_tag(it, "nyaa:size").and_then(|s| parse_human_size(&s)),
                seeders: xml_tag(it, "nyaa:seeders").and_then(|s| s.parse().ok()),
                leechers: xml_tag(it, "nyaa:leechers").and_then(|s| s.parse().ok()),
                page: xml_tag(it, "guid").filter(|g| g.starts_with("https://")),
            })
        })
        .take(MAX_HITS_PER_PROVIDER)
        .collect()
}

pub fn parse_academic_torrents(xml: &str, query: &str) -> Vec<Hit> {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
    xml_items(xml)
        .filter_map(|it| {
            let hash = valid_hash(&xml_tag(it, "infohash")?)?;
            let title = tidy_title(&xml_tag(it, "title")?);
            let hay = format!("{} {}", title.to_lowercase(), xml_tag(it, "description").unwrap_or_default().to_lowercase());
            if !words.iter().all(|w| hay.contains(w)) {
                return None;
            }
            Some(Hit {
                provider: Provider::AcademicTorrents,
                source: Source::Magnet(magnet(&hash, &title)),
                size: xml_tag(it, "size").and_then(|s| s.parse().ok()),
                seeders: None,
                leechers: None,
                page: Some(format!("https://academictorrents.com/details/{hash}")),
                title,
            })
        })
        .take(MAX_HITS_PER_PROVIDER)
        .collect()
}

// ---- network -------------------------------------------------------------------------

async fn get_text(client: &reqwest::Client, url: url::Url) -> Result<String> {
    let resp = client.get(url).send().await.map_err(|e| AppError::new(ErrorCode::ErrConnectionDropped, e.to_string()))?;
    if !resp.status().is_success() {
        return Err(AppError::new(ErrorCode::ErrConnectionDropped, format!("server answered {}", resp.status())));
    }
    if resp.content_length().is_some_and(|l| l as usize > MAX_BODY) {
        return Err(AppError::invalid("response is too large"));
    }
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk.map_err(|e| AppError::new(ErrorCode::ErrConnectionDropped, e.to_string()))?);
        if body.len() > MAX_BODY {
            return Err(AppError::invalid("response is too large"));
        }
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

fn with_query(base: &str, pairs: &[(&str, &str)]) -> Result<url::Url> {
    let mut u = url::Url::parse(base).map_err(|_| AppError::invalid(format!("bad endpoint {base}")))?;
    u.query_pairs_mut().extend_pairs(pairs.iter().copied());
    Ok(u)
}

async fn one(client: &reqwest::Client, p: Provider, q: &str, ep: &Endpoints) -> Result<Vec<Hit>> {
    match p {
        Provider::InternetArchive => {
            let u = url::Url::parse_with_params(
                &ep.internet_archive,
                &[("q", q), ("fl[]", "identifier"), ("fl[]", "title"), ("fl[]", "item_size"), ("rows", "50"), ("output", "json"), ("sort[]", "downloads desc")],
            )
            .map_err(|_| AppError::invalid("bad endpoint"))?;
            Ok(parse_internet_archive(&get_text(client, u).await?, &ep.archive_download))
        }
        Provider::AcademicTorrents => {
            let u = url::Url::parse(&ep.academic_rss).map_err(|_| AppError::invalid("bad endpoint"))?;
            Ok(parse_academic_torrents(&get_text(client, u).await?, q))
        }
        Provider::Apibay => Ok(parse_apibay(&get_text(client, with_query(&ep.apibay, &[("q", q)])?).await?)),
        Provider::Nyaa => Ok(parse_nyaa(&get_text(client, with_query(&ep.nyaa, &[("page", "rss"), ("q", q)])?).await?)),
    }
}

/// Query every chosen provider concurrently. A failing provider is reported, not fatal.
pub async fn search(client: &reqwest::Client, providers: &[Provider], query: &str, ep: &Endpoints) -> Result<SearchOutcome> {
    let q = clean_query(query)?;
    if providers.is_empty() {
        return Err(AppError::invalid("choose at least one search source"));
    }
    let futs = providers.iter().map(|&p| {
        let (client, q, ep) = (client.clone(), q.clone(), ep.clone());
        async move {
            let r = match tokio::time::timeout(PER_PROVIDER_TIMEOUT, one(&client, p, &q, &ep)).await {
                Ok(r) => r,
                Err(_) => Err(AppError::new(ErrorCode::ErrConnectionDropped, "timed out")),
            };
            (p, r)
        }
    });
    let mut out = SearchOutcome::default();
    for (p, r) in futures_util::future::join_all(futs).await {
        match r {
            Ok(mut h) => out.hits.append(&mut h),
            Err(e) => out.errors.push((p, format!("{}: {}", e.code, e.message))),
        }
    }
    // Best-supported first; within that, biggest swarm.
    out.hits.sort_by(|a, b| b.seeders.unwrap_or(0).cmp(&a.seeders.unwrap_or(0)));
    // The same torrent can appear in several indexes: keep one (the first, i.e. most seeded).
    let mut seen = std::collections::HashSet::new();
    out.hits.retain(|h| match &h.source {
        Source::Magnet(m) => seen.insert(m.split('&').next().unwrap_or(m).to_string()),
        Source::TorrentUrl(u) => seen.insert(u.clone()),
    });
    Ok(out)
}

/// File name a hit will most likely get (for display only).
pub fn display_name(h: &Hit) -> String {
    sanitize_file_name(&h.title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_rules() {
        assert_eq!(clean_query("  big   buck\tbunny ").unwrap(), "big buck bunny");
        assert!(clean_query("a").is_err());
        assert!(clean_query(&"x".repeat(201)).is_err());
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_human_size("1.5 GiB"), Some(1_610_612_736));
        assert_eq!(parse_human_size("512 KiB"), Some(524_288));
        assert_eq!(parse_human_size("12 parsecs"), None);
        assert_eq!(parse_human_size("abc"), None);
    }

    #[test]
    fn apibay_rows_and_the_no_results_placeholder() {
        let json = r#"[{"id":"1","name":"Sintel.2010.1080p","info_hash":"43F4001DE4AB25D521C63684E2B69804193ED9D9","leechers":"3","seeders":"22","size":"583689809"},
            {"id":"2","name":"bad hash","info_hash":"zzzz","seeders":"1","leechers":"0","size":"1"},
            {"id":"3","name":"ctrl\u0007chars  here","info_hash":"150922BCE30A1091E347741EBBF96072593A69B4","seeders":"5","leechers":"1","size":"10"}]"#;
        let h = parse_apibay(json);
        assert_eq!(h.len(), 2, "invalid hash row dropped");
        assert_eq!(h[0].seeders, Some(22));
        assert_eq!(h[0].size, Some(583_689_809));
        assert!(matches!(&h[0].source, Source::Magnet(m) if m.starts_with("magnet:?xt=urn:btih:43f4001de4ab25d521c63684e2b69804193ed9d9&dn=Sintel.2010.1080p")));
        assert_eq!(h[1].title, "ctrlchars here");
        let none = r#"[{"id":"0","name":"No results returned","info_hash":"0000000000000000000000000000000000000000","leechers":"0","seeders":"0","size":"0"}]"#;
        assert!(parse_apibay(none).is_empty());
        assert!(parse_apibay("not json").is_empty());
    }

    #[test]
    fn nyaa_rss() {
        let xml = r#"<rss><channel><item><title>[Fan] Sintel &amp; Co 2010</title><link>https://nyaa.si/download/1.torrent</link>
            <guid isPermaLink="true">https://nyaa.si/view/1</guid><nyaa:seeders>7</nyaa:seeders><nyaa:leechers>2</nyaa:leechers>
            <nyaa:infoHash>0123456789abcdef0123456789abcdef01234567</nyaa:infoHash><nyaa:size>1.4 GiB</nyaa:size></item>
            <item><title>no hash</title></item></channel></rss>"#;
        let h = parse_nyaa(xml);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].title, "[Fan] Sintel & Co 2010");
        assert_eq!(h[0].size, Some(1_503_238_553));
        assert_eq!(h[0].page.as_deref(), Some("https://nyaa.si/view/1"));
    }

    #[test]
    fn academic_torrents_filters_locally() {
        let xml = r#"<channel><item><title>ImageNet subset</title><infohash>447df5f09ca5c48b0cf2844cfffc1425ae18aaa4</infohash><description>images of plants</description><size>100</size></item>
            <item><title>Audio corpus</title><infohash>1111111111111111111111111111111111111111</infohash><description>speech</description><size>5</size></item></channel>"#;
        assert_eq!(parse_academic_torrents(xml, "imagenet").len(), 1);
        assert_eq!(parse_academic_torrents(xml, "plants images").len(), 1, "all words must match, any order");
        assert_eq!(parse_academic_torrents(xml, "video").len(), 0);
    }

    #[test]
    fn internet_archive_json_and_hostile_identifiers() {
        let json = r#"{"response":{"docs":[{"identifier":"sintel_movie","title":"Sintel","item_size":419328},
            {"identifier":"../../etc/passwd","title":"evil"},{"identifier":"a b","title":"space"}]}}"#;
        let h = parse_internet_archive(json, "https://archive.org/download");
        assert_eq!(h.len(), 1, "path-like identifiers are rejected");
        assert_eq!(h[0].source, Source::TorrentUrl("https://archive.org/download/sintel_movie/sintel_movie_archive.torrent".into()));
        assert_eq!(h[0].size, Some(419_328));
    }
}
