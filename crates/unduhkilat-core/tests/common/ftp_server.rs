//! In-process FTP / FTPS server for tests (loopback only). Serves a real temporary folder.
//! Supports passive mode, LIST (unix or DOS format), RETR/STOR/APPE with REST, SIZE, MKD/RMD/DELE,
//! RNFR/RNTO, explicit TLS (`AUTH TLS`, `PBSZ`, `PROT P`) and implicit TLS.

#![allow(dead_code)]

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

#[derive(Clone, Debug, Default)]
pub struct TestFtpConfig {
    pub user: String,
    pub password: String,
    /// List in `dir` DOS style instead of unix style.
    pub dos_list: bool,
    /// Offer `AUTH TLS`.
    pub tls: bool,
    /// Wrap the whole control connection in TLS from the start.
    pub implicit_tls: bool,
}

pub struct TestFtp {
    pub addr: std::net::SocketAddr,
    pub root: PathBuf,
    /// SHA-256 (lower-case hex) of the certificate this server presents.
    pub cert_sha256: String,
    pub pass_commands: Arc<AtomicUsize>,
    accept: tokio::task::JoinHandle<()>,
}

fn acceptor() -> (TlsAcceptor, String) {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()]).unwrap();
    let der = ck.cert.der().to_vec();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(ck.key_pair.serialize_der()));
    let provider = Arc::new(tokio_rustls::rustls::crypto::aws_lc_rs::default_provider());
    let cfg = ServerConfig::builder_with_provider(provider).with_safe_default_protocol_versions().unwrap().with_no_client_auth().with_single_cert(vec![CertificateDer::from(der.clone())], key).unwrap();
    (TlsAcceptor::from(Arc::new(cfg)), sha256_hex(&der))
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

impl TestFtp {
    pub async fn start(cfg: TestFtpConfig) -> TestFtp {
        let root = std::env::temp_dir().join(format!("uk-ftp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (acc, cert_sha256) = acceptor();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let pass_commands = Arc::new(AtomicUsize::new(0));
        let (r, p) = (root.clone(), pass_commands.clone());
        let accept = tokio::spawn(async move {
            loop {
                let Ok((s, _)) = listener.accept().await else { break };
                let (cfg, root, acc, p) = (cfg.clone(), r.clone(), acc.clone(), p.clone());
                tokio::spawn(async move {
                    let _ = serve(s, cfg, root, acc, p).await;
                });
            }
        });
        TestFtp { addr, root, cert_sha256, pass_commands, accept }
    }

    pub fn stop(self) {
        self.accept.abort();
    }
}

struct Ctl {
    io: Box<dyn Io>,
    buf: Vec<u8>,
}

impl Ctl {
    async fn line(&mut self) -> Option<String> {
        loop {
            if let Some(i) = self.buf.windows(2).position(|w| w == b"\r\n") {
                let l = String::from_utf8_lossy(&self.buf[..i]).into_owned();
                self.buf.drain(..i + 2);
                return Some(l);
            }
            let mut tmp = [0u8; 512];
            let n = self.io.read(&mut tmp).await.ok()?;
            if n == 0 {
                return None;
            }
            self.buf.extend_from_slice(&tmp[..n]);
        }
    }

    async fn say(&mut self, s: &str) {
        let _ = self.io.write_all(format!("{s}\r\n").as_bytes()).await;
        let _ = self.io.flush().await;
    }
}

fn resolve(root: &Path, cwd: &str, arg: &str) -> Option<PathBuf> {
    let joined = if arg.starts_with('/') { arg.to_string() } else { format!("{}/{}", cwd.trim_end_matches('/'), arg) };
    let mut out = root.to_path_buf();
    for part in Path::new(joined.trim_start_matches('/')).components() {
        match part {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(out)
}

fn list_text(dir: &Path, dos: bool) -> String {
    let mut out = String::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let m = e.metadata().unwrap();
        let name = e.file_name().to_string_lossy().into_owned();
        if dos {
            let kind = if m.is_dir() { "<DIR>         ".to_string() } else { format!("{:>14}", m.len()) };
            out.push_str(&format!("11-08-26  01:00AM {kind} {name}\r\n"));
        } else {
            let (t, sz) = if m.is_dir() { ('d', 4096) } else { ('-', m.len()) };
            out.push_str(&format!("{t}rw-r--r-- 1 eko eko {sz:>10} Nov  8 01:00 {name}\r\n"));
        }
    }
    out
}

async fn serve(stream: TcpStream, cfg: TestFtpConfig, root: PathBuf, acc: TlsAcceptor, pass_commands: Arc<AtomicUsize>) -> Option<()> {
    let io: Box<dyn Io> = if cfg.implicit_tls { Box::new(acc.accept(stream).await.ok()?) } else { Box::new(stream) };
    let mut c = Ctl { io, buf: Vec::new() };
    let mut protect = cfg.implicit_tls;
    let (mut cwd, mut rest, mut pasv, mut rnfr): (String, u64, Option<TcpListener>, Option<PathBuf>) = ("/".into(), 0, None, None);
    let mut authed = false;
    c.say("220 test ftp").await;
    while let Some(line) = c.line().await {
        let (cmd, arg) = line.split_once(' ').map(|(a, b)| (a.to_ascii_uppercase(), b.to_string())).unwrap_or((line.to_ascii_uppercase(), String::new()));
        match cmd.as_str() {
            "AUTH" if cfg.tls && arg.eq_ignore_ascii_case("TLS") => {
                c.say("234 go").await;
                let plain = std::mem::replace(&mut c.io, Box::new(tokio::io::join(tokio::io::empty(), tokio::io::sink())));
                c.io = Box::new(acc.accept(plain).await.ok()?);
            }
            "PBSZ" => c.say("200 ok").await,
            "PROT" => {
                protect = arg.eq_ignore_ascii_case("P");
                c.say("200 ok").await
            }
            "USER" => {
                authed = arg == cfg.user;
                c.say("331 password").await
            }
            "PASS" => {
                pass_commands.fetch_add(1, Ordering::SeqCst);
                if authed && arg == cfg.password {
                    c.say("230 in").await
                } else {
                    authed = false;
                    c.say("530 no").await
                }
            }
            "SYST" => c.say("215 UNIX Type: L8").await,
            "FEAT" => c.say("211-Features:\r\n UTF8\r\n SIZE\r\n REST STREAM\r\n211 end").await,
            "OPTS" | "TYPE" | "NOOP" => c.say("200 ok").await,
            "PWD" => c.say(&format!("257 \"{cwd}\" is current")).await,
            "CWD" => match resolve(&root, &cwd, &arg) {
                Some(p) if p.is_dir() => {
                    cwd = format!("/{}", p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"));
                    c.say("250 ok").await
                }
                _ => c.say("550 no such directory").await,
            },
            "CDUP" => {
                cwd = {
                    let t = cwd.trim_end_matches('/');
                    match t.rfind('/') {
                        Some(0) | None => "/".into(),
                        Some(i) => t[..i].to_string(),
                    }
                };
                c.say("250 ok").await
            }
            "PASV" => {
                let l = TcpListener::bind("127.0.0.1:0").await.ok()?;
                let port = l.local_addr().ok()?.port();
                c.say(&format!("227 Entering Passive Mode (127,0,0,1,{},{})", port / 256, port % 256)).await;
                pasv = Some(l);
            }
            "LIST" | "RETR" | "STOR" | "APPE" => {
                let Some(l) = pasv.take() else {
                    c.say("425 use PASV first").await;
                    continue;
                };
                let target = if cmd == "LIST" {
                    let a = arg.trim_start_matches("-a").trim();
                    resolve(&root, &cwd, if a.is_empty() { "." } else { a })
                } else {
                    resolve(&root, &cwd, &arg)
                };
                let Some(target) = target else {
                    c.say("550 bad path").await;
                    continue;
                };
                if (cmd == "RETR" && !target.is_file()) || (cmd == "LIST" && !target.is_dir()) {
                    c.say("550 not found").await;
                    continue;
                }
                c.say("150 opening").await;
                let (raw, _) = l.accept().await.ok()?;
                let mut data: Box<dyn Io> = if protect { Box::new(acc.accept(raw).await.ok()?) } else { Box::new(raw) };
                let ok = match cmd.as_str() {
                    "LIST" => data.write_all(list_text(&target, cfg.dos_list).as_bytes()).await.is_ok(),
                    "RETR" => {
                        let bytes = std::fs::read(&target).unwrap_or_default();
                        let from = (rest as usize).min(bytes.len());
                        rest = 0;
                        data.write_all(&bytes[from..]).await.is_ok()
                    }
                    _ => {
                        let mut body = Vec::new();
                        let _ = data.read_to_end(&mut body).await;
                        let mut f = if cmd == "APPE" {
                            std::fs::OpenOptions::new().append(true).create(true).open(&target)
                        } else {
                            std::fs::OpenOptions::new().write(true).create(true).truncate(rest == 0).open(&target)
                        }
                        .ok()?;
                        use std::io::{Seek, Write};
                        if cmd == "STOR" && rest > 0 {
                            f.seek(std::io::SeekFrom::Start(rest)).ok()?;
                        }
                        rest = 0;
                        f.write_all(&body).is_ok()
                    }
                };
                let _ = data.shutdown().await;
                drop(data);
                c.say(if ok { "226 done" } else { "426 failed" }).await;
            }
            "REST" => {
                rest = arg.parse().unwrap_or(0);
                c.say("350 ok").await
            }
            "SIZE" => match resolve(&root, &cwd, &arg).and_then(|p| std::fs::metadata(p).ok()) {
                Some(m) if m.is_file() => c.say(&format!("213 {}", m.len())).await,
                _ => c.say("550 no").await,
            },
            "MKD" => match resolve(&root, &cwd, &arg) {
                Some(p) if std::fs::create_dir(&p).is_ok() => c.say("257 created").await,
                _ => c.say("550 failed").await,
            },
            "RMD" => match resolve(&root, &cwd, &arg) {
                Some(p) if std::fs::remove_dir(&p).is_ok() => c.say("250 removed").await,
                _ => c.say("550 failed").await,
            },
            "DELE" => match resolve(&root, &cwd, &arg) {
                Some(p) if std::fs::remove_file(&p).is_ok() => c.say("250 deleted").await,
                _ => c.say("550 failed").await,
            },
            "RNFR" => {
                rnfr = resolve(&root, &cwd, &arg).filter(|p| p.exists());
                c.say(if rnfr.is_some() { "350 ready" } else { "550 no" }).await
            }
            "RNTO" => match (rnfr.take(), resolve(&root, &cwd, &arg)) {
                (Some(a), Some(b)) if std::fs::rename(&a, &b).is_ok() => c.say("250 renamed").await,
                _ => c.say("550 failed").await,
            },
            "QUIT" => {
                c.say("221 bye").await;
                break;
            }
            _ => c.say("502 not implemented").await,
        }
    }
    Some(())
}
