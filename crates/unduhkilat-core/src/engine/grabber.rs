//! FR-045 Site Grabber (single page). Scans one page for links, filters them with a
//! wildcard pattern and returns a preview. Nothing is downloaded or queued here: the
//! caller enqueues only what the user ticks (FR-039/FR-047).
//!
//! The depth-limited walk (FR-046, TASK-46) is blocked by TBD-017 and not implemented.

use super::http::{percent_decode, sanitize_file_name};
use crate::error::{AppError, ErrorCode, Result};
use futures_util::StreamExt;
use serde::Serialize;
use std::time::Duration;

/// Pages bigger than this are not link lists; refuse instead of buffering them.
pub const MAX_PAGE_BYTES: usize = 5 * 1024 * 1024;
/// A preview of thousands of rows is not reviewable (PRD risk); cap and say so.
pub const MAX_LINKS: usize = 5000;

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub url: String,
    pub file_name: String,
    /// `None` when the server gave no Content-Length ("Ukuran: Tidak Diketahui").
    pub size: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanResult {
    pub candidates: Vec<Candidate>,
    /// More links matched than `MAX_LINKS`.
    pub truncated: bool,
    pub links_on_page: usize,
}

/// `*` = any run (also across `/`), `?` = exactly one character. Case-insensitive.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) && p[pi] != '*' {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn last_segment(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.path_segments().and_then(|mut s| s.next_back().map(|x| percent_decode(x))))
        .unwrap_or_default()
}

/// A pattern containing `/` is matched against the whole URL (`*/a/*.zip`); any other
/// pattern (`*.pdf`, `image_???.jpg`) is matched against the file name.
pub fn filter_links_by_wildcard(links: &[String], pattern: &str) -> Vec<String> {
    let pattern = pattern.trim();
    let whole = pattern.contains('/');
    links.iter().filter(|l| if whole { wildcard_match(pattern, l) } else { wildcard_match(pattern, &last_segment(l)) }).cloned().collect()
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&#x27;", "'").replace("&lt;", "<").replace("&gt;", ">")
}

/// Values of `attr` on every `<tag ...>` in `html` (quoted or bare). Deliberately a small
/// scanner, not a full HTML parser: it only needs link targets.
fn attr_values(html: &str, tags: &[&str], attr: &str) -> Vec<String> {
    let b = html.as_bytes();
    let lower = html.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = lower[i..].find('<') {
        let start = i + off + 1;
        let name_end = start + lb[start..].iter().position(|c| !c.is_ascii_alphanumeric()).unwrap_or(lb.len() - start);
        let name = &lower[start..name_end];
        let tag_end = lower[name_end..].find('>').map(|e| name_end + e).unwrap_or(lb.len());
        if tags.contains(&name) {
            // Walk attributes inside this tag.
            let mut j = name_end;
            while j < tag_end {
                while j < tag_end && (lb[j].is_ascii_whitespace() || lb[j] == b'/') {
                    j += 1;
                }
                let k0 = j;
                while j < tag_end && !lb[j].is_ascii_whitespace() && lb[j] != b'=' && lb[j] != b'>' && lb[j] != b'/' {
                    j += 1;
                }
                let key = &lower[k0..j];
                while j < tag_end && lb[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < tag_end && lb[j] == b'=' {
                    j += 1;
                    while j < tag_end && lb[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    let (v0, v1) = if j < tag_end && (b[j] == b'"' || b[j] == b'\'') {
                        let q = b[j];
                        let s0 = j + 1;
                        let e = b[s0..tag_end.max(s0)].iter().position(|c| *c == q).map(|p| s0 + p).unwrap_or(tag_end);
                        j = e + 1;
                        (s0, e)
                    } else {
                        let s0 = j;
                        while j < tag_end && !lb[j].is_ascii_whitespace() {
                            j += 1;
                        }
                        (s0, j)
                    };
                    if key == attr && v0 <= v1 {
                        out.push(decode_entities(html[v0..v1].trim()));
                    }
                } else if j == k0 {
                    j += 1;
                }
            }
        }
        i = tag_end.max(i + 1);
        if i >= lb.len() {
            break;
        }
    }
    out
}

/// Absolute, de-duplicated http/https/ftp link targets of `html`, in page order.
pub fn extract_links(html: &str, page_url: &str) -> Vec<String> {
    let Ok(page) = url::Url::parse(page_url) else { return Vec::new() };
    let base = attr_values(html, &["base"], "href").first().and_then(|h| page.join(h).ok()).unwrap_or(page);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for raw in attr_values(html, &["a", "area"], "href") {
        if raw.is_empty() || raw.starts_with('#') {
            continue;
        }
        let Ok(mut u) = base.join(&raw) else { continue };
        if !matches!(u.scheme(), "http" | "https" | "ftp") {
            continue;
        }
        u.set_fragment(None);
        let s = u.to_string();
        if seen.insert(s.clone()) {
            out.push(s);
        }
    }
    out
}

fn crawl_err(msg: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::ErrCrawlFailed, msg)
}

async fn fetch_page(client: &reqwest::Client, url: &str) -> Result<String> {
    let resp = client.get(url).send().await.map_err(|e| crawl_err(format!("cannot reach {url}: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(crawl_err(format!("server answered {status}")));
    }
    if resp.content_length().is_some_and(|l| l as usize > MAX_PAGE_BYTES) {
        return Err(crawl_err("page is too large to scan"));
    }
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk.map_err(|e| crawl_err(e.to_string()))?);
        if body.len() > MAX_PAGE_BYTES {
            return Err(crawl_err("page is too large to scan"));
        }
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Real sizes via HEAD (best effort, a few at a time, short timeout).
async fn fill_sizes(client: &reqwest::Client, items: &mut [Candidate], referer: &str) {
    let sizes: Vec<Option<i64>> = futures_util::stream::iter(items.iter().map(|c| c.url.clone()).collect::<Vec<String>>())
        .map(|u| {
            let client = client.clone();
            let referer = referer.to_string();
            async move {
                if !u.starts_with("http") {
                    return None;
                }
                let r = tokio::time::timeout(Duration::from_secs(8), client.head(&u).header(reqwest::header::REFERER, referer).send()).await.ok()?.ok()?;
                if !r.status().is_success() {
                    return None;
                }
                // reqwest's content_length() is the (empty) body size for HEAD; read the header.
                r.headers().get(reqwest::header::CONTENT_LENGTH)?.to_str().ok()?.parse::<i64>().ok()
            }
        })
        .buffered(6)
        .collect()
        .await;
    for (c, s) in items.iter_mut().zip(sizes) {
        c.size = s;
    }
}

/// Fetch `page_url`, keep the links matching `pattern` and describe them for the preview.
pub async fn scan_page(client: &reqwest::Client, page_url: &str, pattern: &str, with_sizes: bool) -> Result<ScanResult> {
    let parsed = url::Url::parse(page_url).map_err(|_| AppError::new(ErrorCode::ErrInvalidUrlScheme, format!("not a valid URL: {page_url}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(AppError::new(ErrorCode::ErrInvalidUrlScheme, "the page to scan must be http or https"));
    }
    if pattern.trim().is_empty() {
        return Err(AppError::invalid("a wildcard pattern is required, e.g. *.pdf"));
    }
    let html = fetch_page(client, page_url).await?;
    let links = extract_links(&html, page_url);
    let mut matched = filter_links_by_wildcard(&links, pattern);
    let truncated = matched.len() > MAX_LINKS;
    matched.truncate(MAX_LINKS);
    let mut candidates: Vec<Candidate> = matched
        .into_iter()
        .map(|url| {
            let n = sanitize_file_name(&last_segment(&url));
            Candidate { file_name: if n.is_empty() { "download".into() } else { n }, url, size: None }
        })
        .collect();
    if with_sizes {
        fill_sizes(client, &mut candidates, page_url).await;
    }
    Ok(ScanResult { candidates, truncated, links_on_page: links.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_semantics() {
        assert!(wildcard_match("*.pdf", "Report.PDF"));
        assert!(!wildcard_match("*.pdf", "report.pdf.zip"));
        assert!(wildcard_match("image_???.jpg", "image_001.jpg"));
        assert!(!wildcard_match("image_???.jpg", "image_01.jpg"));
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("a*b*c", "aXXbYYc"));
        assert!(!wildcard_match("a*b*c", "aXXbYY"));
        assert!(wildcard_match("*/a/*.zip", "https://e.test/a/x/y.zip"));
    }

    #[test]
    fn spec_example_from_testing_doc() {
        let links: Vec<String> = ["https://example.test/a/archive1.zip", "https://example.test/a/archive2.zip", "https://example.test/b/archive3.zip", "https://example.test/a/readme.txt"].map(String::from).to_vec();
        assert_eq!(filter_links_by_wildcard(&links, "*/a/*.zip"), vec!["https://example.test/a/archive1.zip", "https://example.test/a/archive2.zip"]);
        assert_eq!(filter_links_by_wildcard(&links, "*.zip").len(), 3);
        assert_eq!(filter_links_by_wildcard(&links, "readme.*").len(), 1);
    }

    #[test]
    fn extracts_and_resolves_links() {
        let html = r##"<html><head><base href="/docs/"></head><body>
            <a href="a.pdf">A</a> <A HREF='/root.pdf'>B</A> <a href=bare.pdf>C</a>
            <a href="a.pdf">dup</a> <a href="#top">frag</a> <a href="mailto:x@y.z">m</a>
            <a href="javascript:void(0)">j</a> <a href="https://other.test/o.zip?x=1&amp;y=2#f">o</a>
            <a href="ftp://f.test/pub/f.iso">f</a> <a name="x">no href</a> <img src="i.png"></body></html>"##;
        let l = extract_links(html, "https://site.test/index.html");
        assert_eq!(l, vec!["https://site.test/docs/a.pdf", "https://site.test/root.pdf", "https://site.test/docs/bare.pdf", "https://other.test/o.zip?x=1&y=2", "ftp://f.test/pub/f.iso"]);
    }

    #[test]
    fn hostile_html_does_not_hang_or_panic() {
        for h in ["<a href=\"unterminated", "<a href=", "<<<<a href=x>", "<a", "", "<a href='\u{1F600}'>", &"<a href=x ".repeat(2000)] {
            let _ = extract_links(h, "https://site.test/");
        }
    }
}
