//! SSH connections with jump host chains (design: spec §2.3, §7, §8; library notes:
//! docs/superpowers/spikes/2026-11-07-ssh-jump.md).
//!
//! The route to a server is explicit: `store::chain_for` gives the hops in order and then the
//! target. The first hop is reached over TCP (through the app's HTTP proxy when one is set); every
//! further hop rides a `direct-tcpip` channel opened on the previous one. Every hop is verified
//! against the pinned host keys and authenticated on its own.

use crate::db::Db;
use crate::engine::ftp::{connect_tcp, Timeouts};
use crate::engine::proxy::ProxyConfig;
use crate::error::{AppError, ErrorCode, Result};
use crate::server::hostkeys::{self, HostKeyStatus};
use crate::server::model::{AuthMethod, Protocol, Server};
use crate::server::secrets::{self, SecretKind, SecretString};
use crate::server::store::{self, MAX_HOPS};
use futures_util::future::BoxFuture;
use russh::client::{self, Handle};
use russh::keys::{decode_secret_key, Algorithm, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use serde_json::json;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

/// What the connection code needs from the user. The UI implements this; tests use a script.
pub trait Interact: Send + Sync {
    /// A host key never seen before. `true` pins it and goes on; `false` aborts the connection.
    fn confirm_host_key<'a>(&'a self, host: &'a str, port: u16, key_type: &'a str, fingerprint: &'a str) -> BoxFuture<'a, bool>;
    /// A password or key passphrase that is not saved (or could not be read). `None` = the user gave up.
    fn secret<'a>(&'a self, server_name: &'a str, kind: SecretKind) -> BoxFuture<'a, Option<SecretString>>;
}

#[derive(Clone)]
pub struct ConnectOptions {
    /// Per hop: TCP connect plus SSH handshake.
    pub connect_timeout: Duration,
    /// Per hop: authentication.
    pub auth_timeout: Duration,
    pub proxy: ProxyConfig,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        ConnectOptions { connect_timeout: Duration::from_secs(15), auth_timeout: Duration::from_secs(20), proxy: ProxyConfig::default() }
    }
}

/// An open chain of SSH connections. Dropping it (or `close`) ends every hop.
pub struct SshSession {
    pub target: Server,
    /// Time from the start of `connect` until the target accepted the login.
    pub rtt: Duration,
    handles: Vec<Handle<HostKeyHandler>>,
}

impl std::fmt::Debug for SshSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshSession").field("target", &self.target.name).field("hops", &(self.handles.len() - 1)).field("rtt", &self.rtt).finish()
    }
}

impl SshSession {
    /// The connection to the target (the last hop), for opening channels.
    pub fn handle(&self) -> &Handle<HostKeyHandler> {
        self.handles.last().expect("a session always has a target")
    }

    /// True once the target or any hop on the way has dropped the connection.
    pub fn is_closed(&self) -> bool {
        self.handles.iter().any(|h| h.is_closed())
    }

    pub async fn close(self) {
        for h in self.handles.iter().rev() {
            let _ = h.disconnect(russh::Disconnect::ByApplication, "bye", "en").await;
        }
    }
}

const CANCELLED: &str = "Sambungan dibatalkan.";

fn cancelled() -> AppError {
    AppError::new(ErrorCode::ErrConnectionDropped, CANCELLED)
}

enum Guard<T> {
    Done(T),
    TimedOut,
    Cancelled,
}

async fn guard<F: Future>(cancel: &CancellationToken, limit: Duration, fut: F) -> Guard<F::Output> {
    tokio::select! {
        _ = cancel.cancelled() => Guard::Cancelled,
        r = tokio::time::timeout(limit, fut) => match r {
            Ok(v) => Guard::Done(v),
            Err(_) => Guard::TimedOut,
        },
    }
}

/// Like `guard`, but the clock stands still while the user is being asked something (`prompting`):
/// reading a host-key fingerprint must not count against the handshake timeout.
async fn guard_handshake<F: Future>(cancel: &CancellationToken, limit: Duration, prompting: &AtomicBool, fut: F) -> Guard<F::Output> {
    tokio::pin!(fut);
    let mut deadline = tokio::time::Instant::now() + limit;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Guard::Cancelled,
            r = &mut fut => return Guard::Done(r),
            _ = tokio::time::sleep_until(deadline) => {
                if prompting.load(Ordering::SeqCst) {
                    deadline = tokio::time::Instant::now() + limit;
                } else {
                    return Guard::TimedOut;
                }
            }
        }
    }
}

fn timed_out(name: &str, limit: Duration) -> AppError {
    AppError::new(ErrorCode::ErrConnectionDropped, format!("Server \"{name}\" tidak menjawab dalam {} detik.", limit.as_secs().max(1)))
}

/// Verifies the host key of one hop. A rejection is recorded in `verdict` (the library only reports
/// "unknown server key") so the caller can return the precise error.
pub struct HostKeyHandler {
    db: Db,
    host: String,
    port: u16,
    name: String,
    interact: Arc<dyn Interact>,
    verdict: Arc<Mutex<Option<AppError>>>,
    /// Cancelled by `connect` when it gives up on this handshake. The library keeps running the
    /// key exchange in its own task, so a late host-key check must be refused, not prompted.
    abort: CancellationToken,
    /// True while the user is being asked (pauses the handshake timeout).
    prompting: Arc<AtomicBool>,
}

impl HostKeyHandler {
    fn reject(&self, e: AppError) -> std::result::Result<bool, russh::Error> {
        *self.verdict.lock().unwrap() = Some(e);
        Ok(false)
    }
}

impl client::Handler for HostKeyHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> std::result::Result<bool, Self::Error> {
        if self.abort.is_cancelled() {
            return Ok(false);
        }
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return self.reject(AppError::new(ErrorCode::ErrHostKeyUnknown, format!("Server \"{}\" memakai sertifikat host, yang belum didukung.", self.name)));
        };
        let key_type = key.algorithm().to_string();
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let (host, port, kt, fp) = (self.host.clone(), self.port, key_type.clone(), fingerprint.clone());
        let status = match self.db.read(|c| hostkeys::check(c, &host, port, &kt, &fp)) {
            Ok(s) => s,
            Err(e) => return self.reject(e),
        };
        match status {
            HostKeyStatus::Trusted => Ok(true),
            HostKeyStatus::Changed { known } => {
                let mut e = AppError::new(
                    ErrorCode::ErrHostKeyChanged,
                    format!(
                        "Kunci host server \"{}\" ({}:{}) BERUBAH. Tersimpan {known}, sekarang {fingerprint}. Sambungan diblokir karena bisa jadi disadap; ganti kunci hanya bila Anda yakin server memang diganti.",
                        self.name, self.host, self.port
                    ),
                );
                // Machine-readable: lets the UI forget exactly this pin, not every server on the route.
                e.details = Some(json!({ "host": self.host, "port": self.port, "key_type": key_type, "known": known, "presented": fingerprint }));
                self.reject(e)
            }
            HostKeyStatus::Unknown => {
                self.prompting.store(true, Ordering::SeqCst);
                let confirmed = tokio::select! {
                    _ = self.abort.cancelled() => false,
                    yes = self.interact.confirm_host_key(&self.host, self.port, &key_type, &fingerprint) => yes,
                };
                self.prompting.store(false, Ordering::SeqCst);
                if !confirmed || self.abort.is_cancelled() {
                    return self.reject(AppError::new(ErrorCode::ErrHostKeyUnknown, format!("Kunci host server \"{}\" belum dikonfirmasi.", self.name)));
                }
                let (host, port, kt, fp) = (self.host.clone(), self.port, key_type, fingerprint);
                match self.db.write(move |c| hostkeys::trust(c, &host, port, &kt, &fp)).await {
                    Ok(()) => Ok(true),
                    Err(e) => self.reject(e),
                }
            }
        }
    }
}

/// Open the whole chain to `target_id` and log in at every hop.
///
/// Errors: a failure on a hop before the target is `ErrJumpFailed` naming that hop, except host key
/// errors (`ErrHostKeyUnknown`/`ErrHostKeyChanged`), which keep their code and also name the server.
/// Nothing in an error message or log contains a secret.
pub async fn connect(db: &Db, target_id: &str, interact: Arc<dyn Interact>, opts: ConnectOptions, cancel: CancellationToken) -> Result<SshSession> {
    let started = Instant::now();
    let chain = db.read(|c| store::chain_for(c, target_id))?;
    let hop_count = chain.len() - 1;
    if hop_count > MAX_HOPS {
        return Err(AppError::new(ErrorCode::ErrTooManyHops, format!("Maksimal {MAX_HOPS} lompatan.")));
    }
    let target = chain[hop_count].clone();
    if !target.protocol.supports_jump() {
        return Err(AppError::invalid("Hanya server SSH dan SFTP yang bisa dihubungi lewat SSH."));
    }
    if chain[..hop_count].iter().any(|h| h.protocol != Protocol::Ssh) {
        return Err(AppError::invalid("Setiap jump host harus server SSH."));
    }
    let proxy = opts.proxy.ftp_proxy().map_err(|_| AppError::invalid("Proxy ini tidak bisa dipakai untuk SSH: SOCKS5 belum didukung; gunakan proxy HTTP atau mode langsung."))?;

    let mut handles: Vec<Handle<HostKeyHandler>> = Vec::with_capacity(chain.len());
    for (i, server) in chain.iter().enumerate() {
        let is_target = i == hop_count;
        match open_hop(db, server, handles.last(), proxy.as_ref(), &interact, &opts, &cancel).await {
            Ok(h) => handles.push(h),
            Err(e) => {
                for h in handles.iter().rev() {
                    let _ = h.disconnect(russh::Disconnect::ByApplication, "failed", "en").await;
                }
                return Err(wrap_hop_error(e, server, is_target));
            }
        }
    }
    tracing::info!(server = %target.name, hops = hop_count, "SSH connected");
    Ok(SshSession { target, rtt: started.elapsed(), handles })
}

fn wrap_hop_error(e: AppError, server: &Server, is_target: bool) -> AppError {
    let keep = matches!(e.code, ErrorCode::ErrHostKeyUnknown | ErrorCode::ErrHostKeyChanged | ErrorCode::ErrTooManyHops) || e.message == CANCELLED;
    if is_target || keep {
        e
    } else {
        AppError::new(ErrorCode::ErrJumpFailed, format!("Lompatan \"{}\" gagal: {}", server.name, e.message))
    }
}

async fn open_hop(
    db: &Db,
    server: &Server,
    prev: Option<&Handle<HostKeyHandler>>,
    proxy: Option<&crate::engine::proxy::HttpProxy>,
    interact: &Arc<dyn Interact>,
    opts: &ConnectOptions,
    cancel: &CancellationToken,
) -> Result<Handle<HostKeyHandler>> {
    let verdict = Arc::new(Mutex::new(None));
    let abort = CancellationToken::new();
    let prompting = Arc::new(AtomicBool::new(false));
    let handler = HostKeyHandler {
        db: db.clone(),
        host: server.host.clone(),
        port: server.port,
        name: server.name.clone(),
        interact: interact.clone(),
        verdict: verdict.clone(),
        abort: abort.clone(),
        prompting: prompting.clone(),
    };
    let config = Arc::new(client::Config { keepalive_interval: Some(Duration::from_secs(30)), keepalive_max: 3, ..Default::default() });

    let result = match prev {
        None => {
            let t = Timeouts { connect: opts.connect_timeout, read: opts.connect_timeout };
            let stream = match guard(cancel, opts.connect_timeout, connect_tcp(&server.host, server.port, proxy, &t)).await {
                Guard::Done(Ok(s)) => s,
                Guard::Done(Err(e)) => return Err(AppError::new(ErrorCode::ErrConnectionDropped, format!("Tidak bisa menghubungi \"{}\" ({}:{}): {}", server.name, server.host, server.port, e.message))),
                Guard::TimedOut => return Err(timed_out(&server.name, opts.connect_timeout)),
                Guard::Cancelled => return Err(cancelled()),
            };
            guard_handshake(cancel, opts.connect_timeout, &prompting, client::connect_stream(config, stream, handler)).await
        }
        Some(p) => {
            let ch = match guard(cancel, opts.connect_timeout, p.channel_open_direct_tcpip(server.host.clone(), server.port as u32, "127.0.0.1", 0)).await {
                Guard::Done(Ok(ch)) => ch,
                Guard::Done(Err(_)) => return Err(AppError::new(ErrorCode::ErrConnectionDropped, format!("\"{}\" ({}:{}) tidak bisa dicapai lewat lompatan sebelumnya.", server.name, server.host, server.port))),
                Guard::TimedOut => return Err(timed_out(&server.name, opts.connect_timeout)),
                Guard::Cancelled => return Err(cancelled()),
            };
            guard_handshake(cancel, opts.connect_timeout, &prompting, client::connect_stream(config, ch.into_stream(), handler)).await
        }
    };
    let mut handle = match result {
        Guard::Done(Ok(h)) => h,
        Guard::Done(Err(e)) => {
            abort.cancel();
            // A host key rejection surfaces from the library as a generic error: report the real reason.
            if let Some(v) = verdict.lock().unwrap().take() {
                return Err(v);
            }
            return Err(AppError::new(ErrorCode::ErrConnectionDropped, format!("Jabat tangan SSH dengan \"{}\" gagal: {e}", server.name)));
        }
        Guard::TimedOut => {
            abort.cancel(); // the key exchange may still be running in the library's task: refuse anything it asks later
            return Err(timed_out(&server.name, opts.connect_timeout));
        }
        Guard::Cancelled => {
            abort.cancel();
            return Err(cancelled());
        }
    };
    authenticate(db, server, &mut handle, interact, opts, cancel).await?;
    Ok(handle)
}

fn auth_failed(server: &Server, what: &str) -> AppError {
    AppError::new(ErrorCode::ErrSshAuthFailed, format!("Autentikasi ke \"{}\" gagal: {what}.", server.name))
}

/// The saved secret if there is a readable one, otherwise ask. The `bool` says it was asked for.
async fn obtain_secret(db: &Db, server: &Server, kind: SecretKind, interact: &Arc<dyn Interact>, cancel: &CancellationToken) -> Result<(Zeroizing<String>, bool)> {
    let id = server.id.clone();
    match db.read(|c| secrets::load_secret(c, &id, kind)) {
        Ok(Some(s)) => return Ok((s, false)),
        Ok(None) => {}
        // Moved to another Windows account: fall back to asking instead of failing.
        Err(e) if e.code == ErrorCode::ErrCredentialUnreadable => {}
        Err(e) => return Err(e),
    }
    let answer = tokio::select! {
        _ = cancel.cancelled() => return Err(cancelled()),
        a = interact.secret(&server.name, kind) => a,
    };
    match answer {
        Some(s) => Ok((Zeroizing::new(s.expose().to_string()), true)),
        None if cancel.is_cancelled() => Err(cancelled()),
        None => Err(auth_failed(server, "kredensial tidak diberikan")),
    }
}

async fn remember_if_asked(db: &Db, server: &Server, kind: SecretKind, secret: &Zeroizing<String>, asked: bool) {
    if asked && server.save_secret {
        let (id, s) = (server.id.clone(), secret.clone());
        if let Err(e) = db.write(move |c| secrets::store_secret(c, &id, kind, &s)).await {
            tracing::warn!(server = %server.name, error = %e.code, "could not save the credential");
        }
    }
}

async fn authenticate(db: &Db, server: &Server, handle: &mut Handle<HostKeyHandler>, interact: &Arc<dyn Interact>, opts: &ConnectOptions, cancel: &CancellationToken) -> Result<()> {
    let accepted = |r: Guard<std::result::Result<russh::client::AuthResult, russh::Error>>| -> Result<bool> {
        match r {
            Guard::Done(Ok(a)) => Ok(a.success()),
            Guard::Done(Err(_)) => Err(auth_failed(server, "sambungan terputus saat masuk")),
            Guard::TimedOut => Err(timed_out(&server.name, opts.auth_timeout)),
            Guard::Cancelled => Err(cancelled()),
        }
    };
    match server.auth {
        AuthMethod::Password => {
            let (password, asked) = obtain_secret(db, server, SecretKind::Password, interact, cancel).await?;
            let r = guard(cancel, opts.auth_timeout, handle.authenticate_password(server.username.clone(), password.as_str())).await;
            if !accepted(r)? {
                return Err(auth_failed(server, "nama pengguna atau password salah"));
            }
            remember_if_asked(db, server, SecretKind::Password, &password, asked).await;
            Ok(())
        }
        AuthMethod::Key => {
            let path = server.key_path.clone().unwrap_or_default();
            let pem = std::fs::read_to_string(&path).map_err(|_| AppError::new(ErrorCode::ErrFileNotFound, format!("Berkas kunci tidak ditemukan atau tidak bisa dibaca: {path}")))?;
            let mut asked_phrase = None;
            let key = match decode_secret_key(&pem, None) {
                Ok(k) => k,
                Err(russh::keys::Error::KeyIsEncrypted) => {
                    let (phrase, asked) = obtain_secret(db, server, SecretKind::Passphrase, interact, cancel).await?;
                    let k = decode_secret_key(&pem, Some(phrase.as_str())).map_err(|_| auth_failed(server, "frasa sandi kunci salah"))?;
                    asked_phrase = Some((phrase, asked));
                    k
                }
                Err(_) => return Err(AppError::invalid(format!("Berkas kunci tidak bisa dibaca (format tidak dikenali): {path}"))),
            };
            let hash = if matches!(key.algorithm(), Algorithm::Rsa { .. }) {
                match guard(cancel, opts.auth_timeout, handle.best_supported_rsa_hash()).await {
                    Guard::Done(r) => r.ok().flatten().flatten(),
                    Guard::TimedOut => return Err(timed_out(&server.name, opts.auth_timeout)),
                    Guard::Cancelled => return Err(cancelled()),
                }
            } else {
                None
            };
            let r = guard(cancel, opts.auth_timeout, handle.authenticate_publickey(server.username.clone(), PrivateKeyWithHashAlg::new(Arc::new(key), hash))).await;
            if !accepted(r)? {
                return Err(auth_failed(server, "kunci ditolak server"));
            }
            if let Some((phrase, asked)) = asked_phrase {
                remember_if_asked(db, server, SecretKind::Passphrase, &phrase, asked).await;
            }
            Ok(())
        }
        AuthMethod::Agent => Err(AppError::invalid("Masuk lewat SSH agent belum didukung.")),
    }
}
