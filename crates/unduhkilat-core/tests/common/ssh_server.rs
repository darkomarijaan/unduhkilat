//! In-process SSH server for tests (loopback only). Supports password and public-key login and
//! `direct-tcpip` forwarding (what a jump host does). Shell, SFTP and PTY channels are added by
//! later plans.

#![allow(dead_code)]

use russh::keys::{Algorithm, HashAlg, PrivateKey, PublicKey};
use russh::server::{self, Auth, Msg, Server as _, Session, VerifiedPublicKey};
use russh::{Channel, ChannelId};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Debug)]
pub enum TestCred {
    Password(String),
    /// A public key in OpenSSH one-line format (`ssh-ed25519 AAAA...`).
    Key(String),
}

#[derive(Clone, Debug, Default)]
pub struct TestSshConfig {
    pub users: Vec<(String, TestCred)>,
    pub allow_forwarding: bool,
    /// Sleep this long after accepting a TCP connection before speaking SSH (a slow or dead hop).
    pub accept_delay: Option<Duration>,
    /// Accept PTY and shell requests and echo input back (a toy shell). `exit` ends the session with status 0.
    /// When false, PTY and shell requests are refused.
    pub shell: bool,
}

#[derive(Clone)]
struct Shared {
    cfg: TestSshConfig,
    logins: Arc<AtomicUsize>,
    forwards: Arc<AtomicUsize>,
    pty: Arc<Mutex<Option<(u16, u16)>>>,
    resizes: Arc<AtomicUsize>,
    open_shells: Arc<AtomicUsize>,
    shell_channels: Arc<Mutex<Vec<(ChannelId, server::Handle)>>>,
}

#[derive(Clone)]
struct Conn(Shared);

impl server::Handler for Conn {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        let ok = self.0.cfg.users.iter().any(|(u, c)| u == user && matches!(c, TestCred::Password(p) if p == password));
        if ok {
            self.0.logins.fetch_add(1, Ordering::SeqCst);
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn auth_publickey(&mut self, user: &str, key: VerifiedPublicKey<'_>) -> Result<Auth, Self::Error> {
        let ok = self.0.cfg.users.iter().any(|(u, c)| {
            u == user
                && match c {
                    TestCred::Key(line) => PublicKey::from_openssh(line).map(|k| k.key_data() == key.public_key.key_data()).unwrap_or(false),
                    _ => false,
                }
        });
        if ok {
            self.0.logins.fetch_add(1, Ordering::SeqCst);
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host: &str,
        port: u32,
        _orig_addr: &str,
        _orig_port: u32,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if !self.0.cfg.allow_forwarding {
            return Ok(()); // not accepted = refused
        }
        self.0.forwards.fetch_add(1, Ordering::SeqCst);
        let target = format!("{host}:{port}");
        reply.accept().await;
        tokio::spawn(async move {
            if let Ok(mut tcp) = TcpStream::connect(target).await {
                let mut ch = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut tcp, &mut ch).await;
            }
        });
        Ok(())
    }

    async fn channel_open_session(&mut self, _c: Channel<Msg>, reply: server::ChannelOpenHandle, _s: &mut Session) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(&mut self, channel: ChannelId, _term: &str, cols: u32, rows: u32, _pw: u32, _ph: u32, _modes: &[(russh::Pty, u32)], session: &mut Session) -> Result<(), Self::Error> {
        if self.0.cfg.shell {
            *self.0.pty.lock().unwrap() = Some((cols as u16, rows as u16));
            session.channel_success(channel)?;
        } else {
            session.channel_failure(channel)?;
        }
        Ok(())
    }

    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
        if self.0.cfg.shell {
            self.0.open_shells.fetch_add(1, Ordering::SeqCst);
            self.0.shell_channels.lock().unwrap().push((channel, session.handle()));
            session.channel_success(channel)?;
        } else {
            session.channel_failure(channel)?;
        }
        Ok(())
    }

    async fn window_change_request(&mut self, _channel: ChannelId, cols: u32, rows: u32, _pw: u32, _ph: u32, _session: &mut Session) -> Result<(), Self::Error> {
        *self.0.pty.lock().unwrap() = Some((cols as u16, rows as u16));
        self.0.resizes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn data(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        if !self.0.cfg.shell {
            return Ok(());
        }
        if data.starts_with(b"exit") {
            session.exit_status_request(channel, 0)?;
            session.eof(channel)?;
            session.close(channel)?;
        } else {
            session.data(channel, data.to_vec())?;
        }
        Ok(())
    }

    async fn channel_close(&mut self, channel: ChannelId, _session: &mut Session) -> Result<(), Self::Error> {
        let mut chans = self.0.shell_channels.lock().unwrap();
        if let Some(i) = chans.iter().position(|(c, _)| *c == channel) {
            chans.remove(i);
            self.0.open_shells.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

pub struct TestSsh {
    pub addr: SocketAddr,
    shared: Shared,
    config: Arc<Mutex<Arc<server::Config>>>,
    fingerprint: Arc<Mutex<String>>,
    accept_loop: tokio::task::JoinHandle<()>,
}

fn new_config() -> (Arc<server::Config>, String) {
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let fp = key.public_key().fingerprint(HashAlg::Sha256).to_string();
    let cfg = server::Config {
        keys: vec![key],
        auth_rejection_time: Duration::from_millis(5),
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    };
    (Arc::new(cfg), fp)
}

impl TestSsh {
    pub async fn start(cfg: TestSshConfig) -> TestSsh {
        let shared = Shared {
            cfg: cfg.clone(),
            logins: Arc::new(AtomicUsize::new(0)),
            forwards: Arc::new(AtomicUsize::new(0)),
            pty: Arc::new(Mutex::new(None)),
            resizes: Arc::new(AtomicUsize::new(0)),
            open_shells: Arc::new(AtomicUsize::new(0)),
            shell_channels: Arc::new(Mutex::new(Vec::new())),
        };
        let (sc, fp) = new_config();
        let config = Arc::new(Mutex::new(sc));
        let fingerprint = Arc::new(Mutex::new(fp));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (cfg_slot, sh) = (config.clone(), shared.clone());
        let accept_loop = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { break };
                // Each connection reads the current config, so `rotate_host_key` affects new connections.
                let current = cfg_slot.lock().unwrap().clone();
                let (handler, delay) = (Conn(sh.clone()), sh.cfg.accept_delay);
                tokio::spawn(async move {
                    if let Some(d) = delay {
                        tokio::time::sleep(d).await;
                    }
                    if let Ok(running) = server::run_stream(current, stream, handler).await {
                        let _ = running.await;
                    }
                });
            }
        });
        TestSsh { addr, shared, config, fingerprint, accept_loop }
    }

    /// `SHA256:...` of the host key new connections will see.
    pub fn host_key_fingerprint(&self) -> String {
        self.fingerprint.lock().unwrap().clone()
    }

    /// Give the server a new host key (new connections only): simulates a changed or intercepted host.
    pub fn rotate_host_key(&self) {
        let (sc, fp) = new_config();
        *self.config.lock().unwrap() = sc;
        *self.fingerprint.lock().unwrap() = fp;
    }

    /// Successful logins so far.
    pub fn login_count(&self) -> usize {
        self.shared.logins.load(Ordering::SeqCst)
    }

    /// `direct-tcpip` channels this server accepted (traffic that really went through it).
    pub fn forward_count(&self) -> usize {
        self.shared.forwards.load(Ordering::SeqCst)
    }

    /// Size of the PTY as last requested (initial request or window change).
    pub fn pty_size(&self) -> Option<(u16, u16)> {
        *self.shared.pty.lock().unwrap()
    }

    /// `window-change` requests received.
    pub fn resize_count(&self) -> usize {
        self.shared.resizes.load(Ordering::SeqCst)
    }

    /// Shell channels currently open.
    pub fn open_shells(&self) -> usize {
        self.shared.open_shells.load(Ordering::SeqCst)
    }

    /// Close every shell channel from the server side (a dropped session).
    pub async fn close_shells(&self) {
        let chans: Vec<(ChannelId, server::Handle)> = self.shared.shell_channels.lock().unwrap().clone();
        for (c, h) in chans {
            let _ = h.close(c).await;
        }
    }

    pub fn stop(self) {
        self.accept_loop.abort();
    }
}

impl Drop for TestSsh {
    fn drop(&mut self) {
        self.accept_loop.abort();
    }
}

/// An unencrypted ed25519 key pair for tests: (private key in OpenSSH PEM, public key one-liner).
pub fn test_keypair(passphrase: Option<&str>) -> (String, String) {
    use russh::keys::ssh_key::LineEnding;
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let public = key.public_key().to_openssh().unwrap();
    let private = match passphrase {
        Some(p) => key.encrypt(&mut rand::rng(), p).unwrap().to_openssh(LineEnding::LF).unwrap().to_string(),
        None => key.to_openssh(LineEnding::LF).unwrap().to_string(),
    };
    (private, public)
}
