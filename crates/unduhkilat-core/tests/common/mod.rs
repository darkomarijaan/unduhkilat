//! Minimal hand-rolled HTTP/1.1 origin used by the integration tests.
//! Supports Range, no-Range, unknown length, redirects, slow bodies and
//! deliberately truncated responses, and logs every request.

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use unduhkilat_core::config::EngineConfig;
use unduhkilat_core::db::Db;
use unduhkilat_core::engine::manager::Manager;

pub mod ftp_proxy;
pub mod ssh_server;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Ranges,
    NoRanges,
    /// 200 without Content-Length (body delimited by close).
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Logged {
    pub path: String,
    pub range: Option<String>,
    pub referer: Option<String>,
    pub cookie: Option<String>,
    pub user_agent: Option<String>,
}

pub struct ServerState {
    pub data: Vec<u8>,
    pub mode: Mutex<Mode>,
    pub chunk_delay_ms: AtomicU64,
    /// Number of upcoming non-probe responses to cut short after 20 000 bytes.
    pub truncate_budget: AtomicI64,
    pub log: Mutex<Vec<Logged>>,
}

pub struct TestServer {
    pub addr: SocketAddr,
    pub state: Arc<ServerState>,
}

pub fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i.wrapping_mul(31) + 7) % 251) as u8).collect()
}

impl TestServer {
    pub async fn start(size: usize, mode: Mode) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(ServerState {
            data: pattern(size),
            mode: Mutex::new(mode),
            chunk_delay_ms: AtomicU64::new(0),
            truncate_budget: AtomicI64::new(0),
            log: Mutex::new(Vec::new()),
        });
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else { break };
                tokio::spawn(handle(sock, st.clone()));
            }
        });
        TestServer { addr, state }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
    pub fn set_delay_ms(&self, ms: u64) {
        self.state.chunk_delay_ms.store(ms, Ordering::SeqCst);
    }
    pub fn log(&self) -> Vec<Logged> {
        self.state.log.lock().unwrap().clone()
    }
    pub fn clear_log(&self) {
        self.state.log.lock().unwrap().clear();
    }
}

fn parse_range(v: &str, total: usize) -> Option<(usize, usize)> {
    let spec = v.strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    let a: usize = a.parse().ok()?;
    let b: usize = if b.is_empty() { total.saturating_sub(1) } else { b.parse().ok()? };
    Some((a, b))
}

async fn handle(mut s: TcpStream, st: Arc<ServerState>) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match s.read(&mut tmp).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
        if buf.len() > 32 * 1024 {
            return;
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or("");
    let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
    let headers: HashMap<String, String> = lines
        .take_while(|l| !l.is_empty())
        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())))
        .collect();
    st.log.lock().unwrap().push(Logged {
        path: path.clone(),
        range: headers.get("range").cloned(),
        referer: headers.get("referer").cloned(),
        cookie: headers.get("cookie").cloned(),
        user_agent: headers.get("user-agent").cloned(),
    });

    if path == "/redirect" {
        let _ = s.write_all(b"HTTP/1.1 302 Found\r\nLocation: /file\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
        return;
    }
    if path != "/file" {
        let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
        return;
    }

    let total = st.data.len();
    let mode = *st.mode.lock().unwrap();
    let range = headers.get("range").and_then(|r| parse_range(r, total));
    let is_probe = range == Some((0, 0));

    let (head, body): (String, &[u8]) = match (mode, range) {
        (Mode::Ranges, Some((a, b))) => {
            let b = b.min(total.saturating_sub(1));
            if a >= total {
                let _ = s
                    .write_all(format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes())
                    .await;
                return;
            }
            (
                format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {a}-{b}/{total}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                    b - a + 1
                ),
                &st.data[a..=b],
            )
        }
        (Mode::Ranges, None) => (
            format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"),
            &st.data[..],
        ),
        (Mode::NoRanges, _) => (
            format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n"),
            &st.data[..],
        ),
        (Mode::Unknown, _) => ("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_string(), &st.data[..]),
    };
    if s.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    if is_probe {
        let _ = s.write_all(body).await; // 1 byte
        return;
    }

    let mut limit = body.len();
    if st.truncate_budget.load(Ordering::SeqCst) > 0 && st.truncate_budget.fetch_sub(1, Ordering::SeqCst) > 0 {
        limit = limit.min(20_000);
    }
    let mut sent = 0;
    while sent < limit {
        let end = (sent + 16 * 1024).min(limit);
        if s.write_all(&body[sent..end]).await.is_err() {
            return;
        }
        sent = end;
        let d = st.chunk_delay_ms.load(Ordering::SeqCst);
        if d > 0 {
            tokio::time::sleep(Duration::from_millis(d)).await;
        }
    }
    let _ = s.shutdown().await;
}

pub struct Env {
    pub dir: PathBuf,
    pub manager: Manager,
    pub db_path: PathBuf,
}

impl Env {
    pub async fn new() -> Env {
        Self::with_cfg(|_| {}).await
    }

    pub async fn with_cfg(f: impl FnOnce(&mut EngineConfig)) -> Env {
        let dir = std::env::temp_dir().join(format!("uk-it-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("state").join("unduhkilat.db");
        let db = Db::open(&db_path).unwrap();
        unduhkilat_core::category::seed_defaults(&db, &dir.join("dl")).unwrap();
        let mut cfg = EngineConfig::default();
        cfg.checkpoint_interval = Duration::from_millis(40);
        f(&mut cfg);
        let manager = Manager::start(db, cfg).await.unwrap();
        // Deterministic tests: never let a proxy configured on the developer machine interfere.
        manager
            .set_proxy(unduhkilat_core::engine::proxy::ProxyConfig { mode: unduhkilat_core::engine::proxy::ProxyMode::Direct, ..Default::default() })
            .await
            .unwrap();
        Env { dir, manager, db_path }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join("out").join(name)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}
