//! Minimal FTP client for downloads (FR-012, TASK-10): login, binary mode, SIZE,
//! passive data connections (EPSV, falling back to PASV) and resume through `REST`.
//!
//! Each segment worker opens its own control + data connection, so the user's
//! connection count is honoured exactly (DEC-001). Data connections always go to the
//! control connection's host, never to the address a server puts in a PASV reply
//! (prevents FTP bounce / SSRF).

use super::http::{file_name_from_url, ProbeResult, RangeSupport};
use super::proxy::HttpProxy;
use crate::error::{AppError, ErrorCode, Result};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf};
use tokio::net::TcpStream;
use zeroize::Zeroizing;

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

#[derive(Clone)]
pub struct Timeouts {
    pub connect: Duration,
    pub read: Duration,
}

#[derive(Clone)]
pub struct FtpTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub pass: Zeroizing<String>,
    /// Absolute remote path, percent-decoded.
    pub path: String,
    /// True when the URL carried no credentials (so stored ones may be used).
    pub anonymous: bool,
}

impl std::fmt::Debug for FtpTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FtpTarget").field("host", &self.host).field("port", &self.port).field("user", &self.user).field("pass", &"[REDACTED]").field("path", &self.path).finish()
    }
}

use super::http::percent_decode as pct_decode;

/// FTP commands are line based: a CR or LF in any argument would let a hostile URL inject
/// extra commands, so such input is refused outright.
fn reject_control_chars(label: &str, v: &str) -> Result<()> {
    if v.contains(['\r', '\n', '\0']) {
        return Err(AppError::invalid(format!("{label} contains control characters")));
    }
    Ok(())
}

pub fn parse(url: &str) -> Result<FtpTarget> {
    let u = url::Url::parse(url).map_err(|_| AppError::new(ErrorCode::ErrInvalidUrlScheme, format!("not a valid URL: {url}")))?;
    if u.scheme() != "ftp" {
        return Err(AppError::new(ErrorCode::ErrInvalidUrlScheme, "not an ftp:// URL"));
    }
    let host = u.host_str().ok_or_else(|| AppError::invalid("ftp URL has no host"))?.to_string();
    let anonymous = u.username().is_empty();
    let user = if anonymous { "anonymous".to_string() } else { pct_decode(u.username()) };
    let pass = if anonymous { "anonymous@".to_string() } else { pct_decode(u.password().unwrap_or("")) };
    // Drop an RFC 1738 ";type=i" suffix; we always transfer in binary.
    let raw_path = u.path().split(';').next().unwrap_or("/");
    let path = pct_decode(raw_path);
    for (l, v) in [("user", &user), ("password", &pass), ("path", &path)] {
        reject_control_chars(l, v)?;
    }
    if path.is_empty() || path.ends_with('/') {
        return Err(AppError::invalid("ftp URL must point at a file"));
    }
    Ok(FtpTarget { host, port: u.port().unwrap_or(21), user, pass: Zeroizing::new(pass), path, anonymous })
}

fn map_reply(code: u16, text: &str, what: &str) -> AppError {
    let c = match code {
        530 | 331 | 332 | 532 => ErrorCode::ErrHttpAuthRequired,
        550 | 450 if what == "RETR" || what == "SIZE" => ErrorCode::ErrFileNotFound,
        421 | 425 | 426 | 450 | 451 => ErrorCode::ErrConnectionDropped,
        _ => ErrorCode::ErrConnectionDropped,
    };
    AppError::new(c, format!("FTP {what} failed: {code} {}", text.trim())).with_details(serde_json::json!({ "ftp_code": code }))
}

/// Open a TCP connection, optionally tunnelled through an HTTP proxy with `CONNECT`.
pub async fn connect_tcp(host: &str, port: u16, proxy: Option<&HttpProxy>, t: &Timeouts) -> Result<Box<dyn Io>> {
    let timed = |e: tokio::time::error::Elapsed| AppError::new(ErrorCode::ErrConnectionDropped, format!("connect timed out: {e}"));
    match proxy {
        None => {
            let s = tokio::time::timeout(t.connect, TcpStream::connect((host, port))).await.map_err(timed)??;
            s.set_nodelay(true).ok();
            Ok(Box::new(s))
        }
        Some(p) => {
            let mut s = tokio::time::timeout(t.connect, TcpStream::connect((p.host.as_str(), p.port))).await.map_err(timed)??;
            let mut req = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
            if let Some(h) = p.basic_header() {
                req.push_str(&format!("Proxy-Authorization: {h}\r\n"));
            }
            req.push_str("\r\n");
            s.write_all(req.as_bytes()).await?;
            // Read the response head one byte at a time so no tunnelled bytes are consumed.
            let mut head = Vec::new();
            let mut b = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if head.len() > 16 * 1024 || tokio::time::timeout(t.read, s.read_exact(&mut b)).await.map_err(timed)?.is_err() {
                    return Err(AppError::new(ErrorCode::ErrConnectionDropped, "proxy closed the connection during CONNECT"));
                }
                head.push(b[0]);
            }
            let first = String::from_utf8_lossy(&head).lines().next().unwrap_or("").to_string();
            let status: u16 = first.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
            match status {
                200 => Ok(Box::new(s)),
                407 => Err(AppError::new(ErrorCode::ErrHttpAuthRequired, "proxy requires authentication (HTTP 407)")),
                _ => Err(AppError::new(ErrorCode::ErrConnectionDropped, format!("proxy refused CONNECT: {first}"))),
            }
        }
    }
}

pub struct FtpConn {
    ctl: BufReader<Box<dyn Io>>,
    host: String,
    proxy: Option<HttpProxy>,
    t: Timeouts,
}

impl FtpConn {
    /// Connect, log in and switch to binary mode.
    pub async fn open(target: &FtpTarget, proxy: Option<&HttpProxy>, t: &Timeouts) -> Result<FtpConn> {
        let stream = connect_tcp(&target.host, target.port, proxy, t).await?;
        let mut c = FtpConn { ctl: BufReader::new(stream), host: target.host.clone(), proxy: proxy.cloned(), t: t.clone() };
        let (mut code, mut text) = c.reply().await?;
        if code == 120 {
            (code, text) = c.reply().await?; // "service ready in nnn minutes"
        }
        if code != 220 {
            return Err(map_reply(code, &text, "greeting"));
        }
        let (code, text) = c.cmd(&format!("USER {}", target.user)).await?;
        match code {
            230 => {}
            331 | 332 => {
                // Not logged: the password must never reach the logs.
                let (code, text) = c.cmd_secret(&format!("PASS {}", &*target.pass)).await?;
                if code != 230 && code != 202 {
                    return Err(map_reply(code, &text, "PASS"));
                }
            }
            _ => return Err(map_reply(code, &text, "USER")),
        }
        let (code, text) = c.cmd("TYPE I").await?;
        if code != 200 {
            return Err(map_reply(code, &text, "TYPE"));
        }
        Ok(c)
    }

    async fn line(&mut self) -> Result<String> {
        let mut s = String::new();
        let n = tokio::time::timeout(self.t.read, self.ctl.read_line(&mut s))
            .await
            .map_err(|_| AppError::new(ErrorCode::ErrConnectionDropped, "FTP server stopped responding"))??;
        if n == 0 {
            return Err(AppError::new(ErrorCode::ErrConnectionDropped, "FTP control connection closed"));
        }
        Ok(s.trim_end_matches(['\r', '\n']).to_string())
    }

    /// Read one (possibly multi-line) reply.
    async fn reply(&mut self) -> Result<(u16, String)> {
        let first = self.line().await?;
        let bytes = first.as_bytes();
        if bytes.len() < 3 || !bytes[..3].iter().all(u8::is_ascii_digit) {
            return Err(AppError::new(ErrorCode::ErrConnectionDropped, format!("malformed FTP reply: {first}")));
        }
        let code: u16 = first[..3].parse().unwrap();
        let mut text = first.get(4..).unwrap_or("").to_string();
        if bytes.get(3) == Some(&b'-') {
            let end = format!("{code} ");
            loop {
                let l = self.line().await?;
                if l.starts_with(&end) {
                    text.push('\n');
                    text.push_str(&l[4..]);
                    break;
                }
                text.push('\n');
                text.push_str(&l);
            }
        }
        Ok((code, text))
    }

    async fn send(&mut self, line: &str) -> Result<()> {
        reject_control_chars("command", line)?;
        self.ctl.get_mut().write_all(format!("{line}\r\n").as_bytes()).await?;
        self.ctl.get_mut().flush().await?;
        Ok(())
    }

    async fn cmd(&mut self, line: &str) -> Result<(u16, String)> {
        tracing::trace!(command = line, "ftp >");
        self.send(line).await?;
        self.reply().await
    }

    async fn cmd_secret(&mut self, line: &str) -> Result<(u16, String)> {
        self.send(line).await?;
        self.reply().await
    }

    async fn open_data(&mut self) -> Result<Box<dyn Io>> {
        let (code, text) = self.cmd("EPSV").await?;
        let port = if code == 229 {
            // "Entering Extended Passive Mode (|||6446|)"
            text.split("|||").nth(1).and_then(|s| s.split('|').next()).and_then(|p| p.parse::<u16>().ok())
        } else {
            None
        };
        let port = match port {
            Some(p) => p,
            None => {
                let (code, text) = self.cmd("PASV").await?;
                if code != 227 {
                    return Err(map_reply(code, &text, "PASV"));
                }
                // "(h1,h2,h3,h4,p1,p2)": only the port is used, never the address.
                let nums: Vec<u16> = text
                    .split(['(', ')'])
                    .nth(1)
                    .unwrap_or("")
                    .split(',')
                    .filter_map(|n| n.trim().parse().ok())
                    .collect();
                if nums.len() != 6 {
                    return Err(AppError::new(ErrorCode::ErrConnectionDropped, format!("cannot parse PASV reply: {text}")));
                }
                nums[4] * 256 + nums[5]
            }
        };
        connect_tcp(&self.host, port, self.proxy.as_ref(), &self.t).await
    }

    /// Start `RETR` at `offset`. The returned stream owns the control connection.
    pub async fn retr(mut self, path: &str, offset: u64) -> Result<FtpStream> {
        let data = self.open_data().await?;
        if offset > 0 {
            let (code, text) = self.cmd(&format!("REST {offset}")).await?;
            if code != 350 {
                return Err(AppError::new(ErrorCode::ErrRangeNotSupported, format!("server refused REST {offset}: {code} {}", text.trim())));
            }
        }
        let (code, text) = self.cmd(&format!("RETR {path}")).await?;
        if code != 150 && code != 125 {
            return Err(map_reply(code, &text, "RETR"));
        }
        Ok(FtpStream { data, _ctl: self })
    }

    pub async fn size(&mut self, path: &str) -> Option<i64> {
        match self.cmd(&format!("SIZE {path}")).await {
            Ok((213, t)) => t.trim().parse().ok(),
            _ => None,
        }
    }

    pub async fn supports_rest(&mut self) -> bool {
        matches!(self.cmd("REST 0").await, Ok((350, _)))
    }

    pub async fn quit(mut self) {
        let _ = self.cmd("QUIT").await;
    }
}

/// Data connection; keeps the control connection alive while it is read.
pub struct FtpStream {
    data: Box<dyn Io>,
    _ctl: FtpConn,
}

impl AsyncRead for FtpStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.data).poll_read(cx, buf)
    }
}

/// FR-003/FR-005 probe for FTP: size from SIZE, range support from REST.
pub async fn probe(url: &str, target: &FtpTarget, proxy: Option<&HttpProxy>, t: &Timeouts) -> Result<ProbeResult> {
    let mut c = FtpConn::open(target, proxy, t).await?;
    let size = c.size(&target.path).await;
    let rest = c.supports_rest().await;
    c.quit().await;
    let supported = rest && size.map(|s| s > 0).unwrap_or(false);
    Ok(ProbeResult {
        effective_url: url.to_string(),
        total_bytes: size,
        range: if supported { RangeSupport::Supported } else { RangeSupport::Unsupported },
        mime_type: None,
        file_name: file_name_from_url(url),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_credentials_port_and_path() {
        let t = parse("ftp://bob:p%40ss@files.example.test:2121/pub/My%20File.iso").unwrap();
        assert_eq!((t.host.as_str(), t.port, t.user.as_str()), ("files.example.test", 2121, "bob"));
        assert_eq!(&*t.pass, "p@ss");
        assert_eq!(t.path, "/pub/My File.iso");
        assert!(!t.anonymous);
    }

    #[test]
    fn anonymous_default_and_type_suffix() {
        let t = parse("ftp://h.test/a/b.bin;type=i").unwrap();
        assert!(t.anonymous);
        assert_eq!(t.user, "anonymous");
        assert_eq!(t.path, "/a/b.bin");
        assert_eq!(t.port, 21);
    }

    #[test]
    fn crlf_injection_is_refused() {
        assert!(parse("ftp://h.test/a%0D%0ADELE%20x").is_err());
        assert!(parse("ftp://bob:x%0D%0AQUIT@h.test/a").is_err());
        assert!(parse("ftp://bo%0Ab@h.test/a").is_err());
    }

    #[test]
    fn rejects_non_ftp_and_directories() {
        assert!(parse("http://h.test/a").is_err());
        assert!(parse("ftp://h.test/dir/").is_err());
        assert!(parse("ftp://h.test").is_err());
    }

    #[test]
    fn debug_never_prints_the_password() {
        let t = parse("ftp://bob:hunter2@h.test/a").unwrap();
        assert!(!format!("{t:?}").contains("hunter2"));
    }
}
