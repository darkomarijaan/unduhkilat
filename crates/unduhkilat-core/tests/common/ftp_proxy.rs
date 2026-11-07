//! Scriptable FTP origin and HTTP proxy for the integration tests.
#![allow(dead_code)]

use super::pattern;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

pub const FTP_PATH: &str = "/pub/file.bin";

pub struct FtpState {
    pub data: Vec<u8>,
    pub rest: AtomicBool,
    pub size: AtomicBool,
    pub epsv: AtomicBool,
    pub delay_ms: AtomicU64,
    pub logins: AtomicUsize,
    /// When set, only this user/password may log in.
    pub require: Mutex<Option<(String, String)>>,
    /// Every command received (PASS arguments are never recorded).
    pub log: Mutex<Vec<String>>,
}

pub struct FtpServer {
    pub addr: SocketAddr,
    pub state: Arc<FtpState>,
}

impl FtpServer {
    pub async fn start(size: usize) -> FtpServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(FtpState {
            data: pattern(size),
            rest: AtomicBool::new(true),
            size: AtomicBool::new(true),
            epsv: AtomicBool::new(true),
            delay_ms: AtomicU64::new(0),
            logins: AtomicUsize::new(0),
            require: Mutex::new(None),
            log: Mutex::new(Vec::new()),
        });
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((s, _)) = listener.accept().await else { break };
                tokio::spawn(ftp_session(s, st.clone()));
            }
        });
        FtpServer { addr, state }
    }
    pub fn url(&self) -> String {
        format!("ftp://{}{}", self.addr, FTP_PATH)
    }
    pub fn url_with_login(&self, user: &str, pass: &str) -> String {
        format!("ftp://{user}:{pass}@{}{}", self.addr, FTP_PATH)
    }
    pub fn commands(&self) -> Vec<String> {
        self.state.log.lock().unwrap().clone()
    }
    pub fn clear(&self) {
        self.state.log.lock().unwrap().clear();
    }
}

async fn ftp_session(sock: TcpStream, st: Arc<FtpState>) {
    let (r, mut w) = sock.into_split();
    let mut r = BufReader::new(r);
    let _ = w.write_all(b"220 test ftp ready\r\n").await;
    let (mut user, mut authed, mut rest) = (String::new(), false, 0usize);
    let mut data_listener: Option<TcpListener> = None;
    let need_auth = st.require.lock().unwrap().is_some();
    if !need_auth {
        authed = true;
    }
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }
        let l = line.trim_end().to_string();
        let (cmd, arg) = l.split_once(' ').map(|(a, b)| (a.to_ascii_uppercase(), b.to_string())).unwrap_or((l.to_ascii_uppercase(), String::new()));
        st.log.lock().unwrap().push(if cmd == "PASS" { "PASS ****".into() } else { l.clone() });
        let reply: String = match cmd.as_str() {
            "USER" => {
                user = arg.clone();
                if need_auth { "331 password required\r\n".into() } else { st.logins.fetch_add(1, Ordering::SeqCst); "230 anonymous ok\r\n".into() }
            }
            "PASS" => {
                let ok = st.require.lock().unwrap().as_ref().map(|(u, p)| *u == user && *p == arg).unwrap_or(true);
                if ok { authed = true; st.logins.fetch_add(1, Ordering::SeqCst); "230 logged in\r\n".into() } else { "530 login incorrect\r\n".into() }
            }
            _ if !authed => "530 please login with USER and PASS\r\n".into(),
            "TYPE" => "200 binary\r\n".into(),
            "SIZE" => {
                if st.size.load(Ordering::SeqCst) && arg == FTP_PATH { format!("213 {}\r\n", st.data.len()) } else { "550 no such file or size unsupported\r\n".into() }
            }
            "EPSV" | "PASV" if cmd == "EPSV" && !st.epsv.load(Ordering::SeqCst) => "500 EPSV not understood\r\n".into(),
            "EPSV" | "PASV" => {
                let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let port = l.local_addr().unwrap().port();
                data_listener = Some(l);
                if cmd == "EPSV" {
                    format!("229 Entering Extended Passive Mode (|||{port}|)\r\n")
                } else {
                    // A hostile server could advertise another address here; clients must ignore it.
                    format!("227 Entering Passive Mode (10,255,255,1,{},{})\r\n", port / 256, port % 256)
                }
            }
            "REST" => {
                if st.rest.load(Ordering::SeqCst) {
                    rest = arg.parse().unwrap_or(0);
                    "350 restarting\r\n".into()
                } else {
                    "502 REST not implemented\r\n".into()
                }
            }
            "RETR" => {
                if arg != FTP_PATH {
                    "550 file not found\r\n".into()
                } else if let Some(l) = data_listener.take() {
                    let _ = w.write_all(b"150 opening data connection\r\n").await;
                    let Ok(Ok((mut d, _))) = tokio::time::timeout(Duration::from_secs(5), l.accept()).await else { return };
                    let body = &st.data[rest.min(st.data.len())..];
                    let mut sent = 0;
                    let mut ok = true;
                    while sent < body.len() {
                        let end = (sent + 16 * 1024).min(body.len());
                        if d.write_all(&body[sent..end]).await.is_err() {
                            ok = false;
                            break;
                        }
                        sent = end;
                        let ms = st.delay_ms.load(Ordering::SeqCst);
                        if ms > 0 {
                            tokio::time::sleep(Duration::from_millis(ms)).await;
                        }
                    }
                    let _ = d.shutdown().await;
                    rest = 0;
                    if ok { "226 transfer complete\r\n".into() } else { "426 connection closed\r\n".into() }
                } else {
                    "425 use PASV first\r\n".into()
                }
            }
            "QUIT" => {
                let _ = w.write_all(b"221 bye\r\n").await;
                return;
            }
            _ => "502 command not implemented\r\n".into(),
        };
        if w.write_all(reply.as_bytes()).await.is_err() {
            return;
        }
    }
}

// ---------------------------------------------------------------------------

pub struct ProxyState {
    /// Expected `Proxy-Authorization` value, e.g. `Basic dTpw`.
    pub require_auth: Mutex<Option<String>>,
    pub requests: Mutex<Vec<String>>,
    pub auth_seen: Mutex<Vec<Option<String>>>,
    pub tunnels: AtomicUsize,
}

pub struct ProxyServer {
    pub addr: SocketAddr,
    pub state: Arc<ProxyState>,
}

impl ProxyServer {
    pub async fn start() -> ProxyServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(ProxyState { require_auth: Mutex::new(None), requests: Mutex::new(Vec::new()), auth_seen: Mutex::new(Vec::new()), tunnels: AtomicUsize::new(0) });
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((s, _)) = listener.accept().await else { break };
                tokio::spawn(proxy_session(s, st.clone()));
            }
        });
        ProxyServer { addr, state }
    }
    pub fn requests(&self) -> Vec<String> {
        self.state.requests.lock().unwrap().clone()
    }
    pub fn set_auth(&self, basic: &str) {
        *self.state.require_auth.lock().unwrap() = Some(basic.to_string());
    }
}

async fn proxy_session(mut c: TcpStream, st: Arc<ProxyState>) {
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if c.read_exact(&mut b).await.is_err() || head.len() > 32 * 1024 {
            return;
        }
        head.push(b[0]);
    }
    let text = String::from_utf8_lossy(&head).to_string();
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or("").to_string();
    st.requests.lock().unwrap().push(first.clone());
    let mut auth = None;
    let mut kept = Vec::new();
    for l in lines.filter(|l| !l.is_empty()) {
        if l.to_ascii_lowercase().starts_with("proxy-authorization:") {
            auth = Some(l.split_once(':').unwrap().1.trim().to_string());
        } else {
            kept.push(l.to_string());
        }
    }
    st.auth_seen.lock().unwrap().push(auth.clone());
    let required = st.require_auth.lock().unwrap().clone();
    if let Some(want) = required {
        if auth.as_deref() != Some(want.as_str()) {
            let _ = c.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"t\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            return;
        }
    }
    let mut parts = first.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if method == "CONNECT" {
        let Ok(mut up) = TcpStream::connect(target).await else {
            let _ = c.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n").await;
            return;
        };
        st.tunnels.fetch_add(1, Ordering::SeqCst);
        let _ = c.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await;
        let _ = tokio::io::copy_bidirectional(&mut c, &mut up).await;
    } else {
        // Absolute-form request: http://host:port/path
        let Ok(u) = url::Url::parse(target) else { return };
        let addr = format!("{}:{}", u.host_str().unwrap_or(""), u.port_or_known_default().unwrap_or(80));
        let Ok(mut up) = TcpStream::connect(addr).await else { return };
        let path = match u.query() {
            Some(q) => format!("{}?{q}", u.path()),
            None => u.path().to_string(),
        };
        let mut req = format!("{method} {path} HTTP/1.1\r\n");
        for l in kept {
            req.push_str(&l);
            req.push_str("\r\n");
        }
        req.push_str("\r\n");
        if up.write_all(req.as_bytes()).await.is_err() {
            return;
        }
        let _ = tokio::io::copy_bidirectional(&mut c, &mut up).await;
    }
}
