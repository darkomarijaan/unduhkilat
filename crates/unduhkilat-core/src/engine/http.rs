//! HTTP/HTTPS client construction and the range-capability probe
//! (ARCHITECTURE.md §6.1 step 1, FR-003/005/013/017).

use crate::config::EngineConfig;
use crate::error::{AppError, ErrorCode, Result};
use crate::models::Download;
use reqwest::header::{HeaderMap, ACCEPT_ENCODING, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, COOKIE, RANGE, REFERER, USER_AGENT};
use reqwest::{redirect, Client, RequestBuilder, Response, StatusCode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeSupport {
    Supported,
    Unsupported,
}

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub effective_url: String,
    /// `None` when the server does not state a size (chunked / unknown).
    pub total_bytes: Option<i64>,
    pub range: RangeSupport,
    pub mime_type: Option<String>,
    pub file_name: Option<String>,
}

/// Headers captured from the browser or the user, replayed verbatim on every request.
#[derive(Clone, Default)]
pub struct RequestContext {
    pub referer: Option<String>,
    pub user_agent: Option<String>,
    pub cookies: Option<String>,
    /// Stored per-host credentials (FR-014). reqwest drops Authorization on cross-origin redirects.
    pub basic: Option<(String, zeroize::Zeroizing<String>)>,
}

impl std::fmt::Debug for RequestContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestContext")
            .field("referer", &self.referer)
            .field("user_agent", &self.user_agent)
            .field("cookies", &self.cookies.as_ref().map(|_| "[REDACTED]"))
            .field("basic", &self.basic.as_ref().map(|(u, _)| format!("{u}:[REDACTED]")))
            .finish()
    }
}

impl RequestContext {
    pub fn from_download(d: &Download) -> Self {
        Self { referer: d.referer.clone(), user_agent: d.user_agent.clone(), cookies: d.cookies.clone(), basic: None }
    }

    pub fn apply(&self, mut rb: RequestBuilder) -> RequestBuilder {
        if let Some(r) = &self.referer {
            rb = rb.header(REFERER, r);
        }
        if let Some(u) = &self.user_agent {
            rb = rb.header(USER_AGENT, u);
        }
        if let Some(c) = &self.cookies {
            rb = rb.header(COOKIE, c);
        }
        if let Some((u, p)) = &self.basic {
            rb = rb.basic_auth(u, Some(p.as_str()));
        }
        // Compression would change byte offsets relative to Range requests.
        rb.header(ACCEPT_ENCODING, "identity")
    }
}

pub fn build_client(cfg: &EngineConfig, proxy: &super::proxy::ProxyConfig) -> Result<Client> {
    let builder = Client::builder()
        .user_agent(&cfg.user_agent)
        .connect_timeout(cfg.connect_timeout)
        .read_timeout(cfg.read_timeout)
        .redirect(redirect::Policy::limited(10))
        // reqwest would otherwise overwrite Referer with the previous hop on every
        // redirect; FR-013 requires the ORIGINAL request's Referer to be preserved.
        .referer(false);
    proxy
        .apply(builder)?
        .build()
        .map_err(|e| AppError::internal(format!("http client: {e}")))
}

/// Map a non-success HTTP status to a canonical error.
pub fn status_error(status: StatusCode, url: &str) -> AppError {
    let code = match status.as_u16() {
        401 | 407 => ErrorCode::ErrHttpAuthRequired,
        404 | 410 => ErrorCode::ErrFileNotFound,
        416 => ErrorCode::ErrRangeNotSatisfiable,
        _ => ErrorCode::ErrConnectionDropped,
    };
    AppError::new(code, format!("HTTP {status} for {url}"))
        .with_details(serde_json::json!({ "http_status": status.as_u16(), "effective_url": url }))
}

/// `Content-Range: bytes 0-0/12345` or `bytes */12345` -> 12345.
fn total_from_content_range(h: &HeaderMap) -> Option<i64> {
    let v = h.get(CONTENT_RANGE)?.to_str().ok()?;
    let total = v.rsplit('/').next()?.trim();
    if total == "*" { None } else { total.parse().ok() }
}

fn content_length(h: &HeaderMap) -> Option<i64> {
    h.get(CONTENT_LENGTH)?.to_str().ok()?.trim().parse().ok()
}

/// Filename from `Content-Disposition` (RFC 6266 filename*/filename), else the URL path.
pub fn file_name_from(headers: &HeaderMap, url: &str) -> Option<String> {
    if let Some(cd) = headers.get(CONTENT_DISPOSITION).and_then(|v| v.to_str().ok()) {
        let find = |key: &str| -> Option<String> {
            let lower = cd.to_ascii_lowercase();
            let pos = lower.find(key)? + key.len();
            let rest = cd[pos..].trim_start();
            let rest = rest.strip_prefix('=')?.trim_start();
            let val = if let Some(q) = rest.strip_prefix('"') { q.split('"').next()? } else { rest.split(';').next()?.trim() };
            Some(val.to_string())
        };
        if let Some(v) = find("filename*") {
            // filename*=UTF-8''percent%20encoded
            let enc = v.rsplit("''").next().unwrap_or(&v);
            let decoded = percent_decode(enc);
            if !decoded.is_empty() {
                return Some(sanitize_file_name(&decoded));
            }
        }
        if let Some(v) = find("filename") {
            if !v.is_empty() {
                return Some(sanitize_file_name(&v));
            }
        }
    }
    file_name_from_url(url)
}

pub fn file_name_from_url(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let seg = u.path_segments()?.next_back()?;
    let decoded = percent_decode(seg);
    let clean = sanitize_file_name(&decoded);
    (!clean.is_empty()).then_some(clean)
}

pub(crate) fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Strip path separators and characters Windows forbids in file names.
/// Google Drive share links are preview pages, not files. For a public *file* link
/// (`/file/d/ID/view`, `/open?id=ID`, `/uc?id=ID`) return the direct-download URL; the
/// `confirm=t` form skips the "cannot scan for viruses" interstitial that would otherwise
/// be saved as an HTML file. Folders cannot be downloaded as one file and are refused.
/// `Ok(None)` means "not a Drive link, leave it alone".
pub fn normalize_drive_url(url: &str) -> Result<Option<String>> {
    let Ok(u) = url::Url::parse(url) else { return Ok(None) };
    if u.host_str() != Some("drive.google.com") || !matches!(u.scheme(), "http" | "https") {
        return Ok(None);
    }
    let segs: Vec<&str> = u.path_segments().map(|s| s.collect()).unwrap_or_default();
    if segs.contains(&"folders") || segs.get(..2) == Some(&["drive", "folders"][..]) {
        return Err(AppError::new(ErrorCode::ErrInvalidUrlScheme, "a Google Drive folder cannot be downloaded as one file; open a single file's link instead"));
    }
    let query = |k: &str| u.query_pairs().find(|(a, _)| a == k).map(|(_, v)| v.into_owned());
    let id = match segs.as_slice() {
        // /file/d/ID/view and /file/u/0/d/ID/view
        ["file", rest @ ..] => rest.iter().position(|s| *s == "d").and_then(|i| rest.get(i + 1)).map(|s| (*s).to_string()),
        ["open"] | ["uc"] => query("id"),
        _ => None,
    };
    let Some(id) = id else { return Ok(None) };
    if id.len() < 10 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(AppError::new(ErrorCode::ErrInvalidUrlScheme, "that does not look like a valid Google Drive file link"));
    }
    let mut out = format!("https://drive.usercontent.google.com/download?id={id}&export=download&confirm=t");
    if let Some(rk) = query("resourcekey").filter(|r| r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')) {
        out.push_str(&format!("&resourcekey={rk}"));
    }
    Ok(Some(out))
}

pub fn sanitize_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base.chars().map(|c| if matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || c.is_control() { '_' } else { c }).collect();
    cleaned.trim().trim_end_matches('.').to_string()
}

/// Probe with `GET Range: bytes=0-0`.
/// * 206 + parseable total  -> range supported
/// * 200                    -> unsupported (server ignored the range), size from Content-Length
/// * 416                    -> unsupported (e.g. empty resource), size from `Content-Range: */N`
pub async fn probe(client: &Client, url: &str, ctx: &RequestContext) -> Result<ProbeResult> {
    let resp: Response = ctx.apply(client.get(url)).header(RANGE, "bytes=0-0").send().await?;
    let status = resp.status();
    let effective_url = resp.url().to_string();
    let headers = resp.headers().clone();
    let mime_type = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(|s| s.split(';').next().unwrap_or(s).trim().to_string());
    let file_name = file_name_from(&headers, &effective_url);
    // We only need the headers; dropping the response closes the connection.
    drop(resp);

    let (range, total) = match status.as_u16() {
        206 => match total_from_content_range(&headers) {
            Some(t) => (RangeSupport::Supported, Some(t)),
            None => (RangeSupport::Unsupported, None), // unknown total: cannot partition
        },
        200 => (RangeSupport::Unsupported, content_length(&headers)),
        416 => (RangeSupport::Unsupported, total_from_content_range(&headers).or(Some(0))),
        _ => return Err(status_error(status, &effective_url)),
    };
    Ok(ProbeResult { effective_url, total_bytes: total, range, mime_type, file_name })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn content_range_parsing() {
        let mut h = HeaderMap::new();
        h.insert(CONTENT_RANGE, HeaderValue::from_static("bytes 0-0/12345"));
        assert_eq!(total_from_content_range(&h), Some(12345));
        h.insert(CONTENT_RANGE, HeaderValue::from_static("bytes */777"));
        assert_eq!(total_from_content_range(&h), Some(777));
        h.insert(CONTENT_RANGE, HeaderValue::from_static("bytes 0-0/*"));
        assert_eq!(total_from_content_range(&h), None);
    }

    #[test]
    fn file_names() {
        let mut h = HeaderMap::new();
        h.insert(CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename=\"a b.zip\""));
        assert_eq!(file_name_from(&h, "http://x/y").as_deref(), Some("a b.zip"));
        h.insert(CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf"));
        assert_eq!(file_name_from(&h, "http://x/y").as_deref(), Some("résumé.pdf"));
        assert_eq!(file_name_from(&HeaderMap::new(), "http://x/dir/My%20File.iso?x=1").as_deref(), Some("My File.iso"));
        assert_eq!(file_name_from(&HeaderMap::new(), "http://x/"), None);
    }

    #[test]
    fn sanitizer_blocks_traversal_and_forbidden_chars() {
        assert_eq!(sanitize_file_name("..\\..\\evil.exe"), "evil.exe");
        assert_eq!(sanitize_file_name("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(sanitize_file_name("../x"), "x");
    }
}

#[cfg(test)]
mod drive_tests {
    use super::normalize_drive_url as n;

    const ID: &str = "1AbCdEfGhIjKlMnOpQrStUvWxYz012345";

    #[test]
    fn file_links_become_direct_downloads() {
        let want = format!("https://drive.usercontent.google.com/download?id={ID}&export=download&confirm=t");
        for u in [format!("https://drive.google.com/file/d/{ID}/view?usp=sharing"), format!("https://drive.google.com/file/d/{ID}/edit"), format!("https://drive.google.com/open?id={ID}"), format!("https://drive.google.com/uc?id={ID}&export=download"), format!("https://drive.google.com/file/u/0/d/{ID}/view")] {
            assert_eq!(n(&u).unwrap().as_deref(), Some(want.as_str()), "{u}");
        }
        let rk = n(&format!("https://drive.google.com/file/d/{ID}/view?resourcekey=0-abc_DEF")).unwrap().unwrap();
        assert!(rk.ends_with("&resourcekey=0-abc_DEF"));
    }

    #[test]
    fn folders_ids_and_other_hosts() {
        assert!(n("https://drive.google.com/drive/folders/1AbCdEfGhIjKlMnOp").is_err());
        assert!(n("https://drive.google.com/drive/u/0/folders/1AbCdEfGhIjKlMnOp").is_err());
        assert!(n("https://drive.google.com/file/d/short/view").is_err());
        assert!(n("https://drive.google.com/file/d/..%2F..%2Fetc/view").is_err());
        assert_eq!(n("https://example.com/file/d/1AbCdEfGhIjKlMnOp/view").unwrap(), None);
        assert_eq!(n("https://drive.google.com/").unwrap(), None);
        assert_eq!(n("not a url").unwrap(), None);
        // The rewrite never leaves Google's download host, whatever the input claims.
        let out = n(&format!("https://drive.google.com/uc?id={ID}&export=download&evil=https://x.test")).unwrap().unwrap();
        assert!(out.starts_with("https://drive.usercontent.google.com/download?id=") && !out.contains("x.test"));
    }
}

