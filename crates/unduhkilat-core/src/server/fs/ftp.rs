//! FTP and FTPS (explicit `AUTH TLS` and implicit) behind `RemoteFs`.
//!
//! FTPS certificates are not checked against a certificate authority: like an SSH host key they are
//! shown for confirmation the first time (SHA-256 fingerprint), pinned in the `known_hosts` table
//! (key type `tls-cert`), and a changed certificate blocks the connection. The check happens right
//! after the TLS handshake and before the user name or password is sent.

use super::{join_remote, Entry, RemoteFs, RemoteReader, RemoteWriter};
use crate::db::Db;
use crate::error::{AppError, ErrorCode, Result};
use crate::server::hostkeys::{self, HostKeyStatus};
use crate::server::model::{Protocol, Server};
use crate::server::secrets::{self, SecretKind};
use crate::server::ssh::Interact;
use async_trait::async_trait;
use futures_util::future::BoxFuture;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, UNIX_EPOCH};
use suppaftp::list::File as ListFile;
use suppaftp::tokio::{AsyncRustlsConnector, AsyncRustlsFtpStream, AsyncRustlsStream, TransferStream};
use suppaftp::types::FileType;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const KEY_TYPE: &str = "tls-cert";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtpMode {
    Plain,
    /// `AUTH TLS` on the normal FTP port.
    Explicit,
    /// TLS from the first byte (port 990).
    Implicit,
}

/// FTP is plain, FTPS is explicit unless it uses the implicit-FTPS port 990.
pub fn ftp_mode(server: &Server) -> FtpMode {
    match (server.protocol, server.port) {
        (Protocol::Ftps, 990) => FtpMode::Implicit,
        (Protocol::Ftps, _) => FtpMode::Explicit,
        _ => FtpMode::Plain,
    }
}

/// Accepts any certificate but remembers its fingerprint, so the caller can ask the user or compare
/// with the pinned one right after the handshake.
#[derive(Debug)]
struct Capture {
    seen: Arc<Mutex<Option<String>>>,
    /// Set once the user (or the pin table) accepted the certificate: every later handshake, the
    /// data connections included, must present exactly this one.
    pinned: Arc<Mutex<Option<String>>>,
    algs: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for Capture {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, _inter: &[CertificateDer<'_>], _name: &ServerName<'_>, _ocsp: &[u8], _now: UnixTime) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let hex: String = Sha256::digest(end_entity.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
        let fingerprint = format!("SHA256:{hex}");
        if let Some(p) = self.pinned.lock().unwrap().as_ref() {
            if *p != fingerprint {
                return Err(rustls::Error::General("certificate differs from the pinned one".into()));
            }
        }
        *self.seen.lock().unwrap() = Some(fingerprint);
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algs)
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

fn connector() -> (AsyncRustlsConnector, Arc<Mutex<Option<String>>>, Arc<Mutex<Option<String>>>) {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let seen = Arc::new(Mutex::new(None));
    let pinned = Arc::new(Mutex::new(None));
    let verifier = Capture { seen: seen.clone(), pinned: pinned.clone(), algs: provider.signature_verification_algorithms };
    let cfg = rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions().expect("default protocol versions").dangerous().with_custom_certificate_verifier(Arc::new(verifier)).with_no_client_auth();
    (AsyncRustlsConnector::from(tokio_rustls::TlsConnector::from(Arc::new(cfg))), seen, pinned)
}

fn ftp_err(e: suppaftp::FtpError) -> AppError {
    use suppaftp::FtpError::*;
    match e {
        ConnectionError(io) => AppError::new(ErrorCode::ErrConnectionDropped, format!("Sambungan FTP bermasalah: {io}")),
        SecureError(m) => AppError::new(ErrorCode::ErrConnectionDropped, format!("Gagal membuat sambungan aman (TLS): {m}")),
        UnexpectedResponse(r) => {
            let code = r.status as u32;
            if code == 550 {
                AppError::new(ErrorCode::ErrNotFound, "Berkas atau folder tidak ditemukan, atau akses ditolak oleh server.")
            } else {
                AppError::new(ErrorCode::ErrRemoteFs, format!("Server FTP menolak operasi (kode {code})."))
            }
        }
        other => AppError::new(ErrorCode::ErrRemoteFs, format!("Operasi FTP gagal: {other}")),
    }
}

/// Every FTP command is bounded: a half-open connection must fail, not hang the queue or the explorer.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

async fn within<T>(f: impl std::future::Future<Output = std::result::Result<T, suppaftp::FtpError>>) -> Result<T> {
    match tokio::time::timeout(COMMAND_TIMEOUT, f).await {
        Ok(r) => r.map_err(ftp_err),
        Err(_) => Err(AppError::new(ErrorCode::ErrConnectionDropped, "Server FTP tidak menjawab. Sambungan mungkin terputus.")),
    }
}

/// Connect, secure the channel if asked, pin the certificate, and log in.
pub async fn open_ftp(db: &Db, server: &Server, interact: Arc<dyn Interact>, cancel: CancellationToken) -> Result<Box<dyn RemoteFs>> {
    open_ftp_with(db, server, ftp_mode(server), interact, cancel).await
}

pub async fn open_ftp_with(db: &Db, server: &Server, mode: FtpMode, interact: Arc<dyn Interact>, cancel: CancellationToken) -> Result<Box<dyn RemoteFs>> {
    let work = open_inner(db, server, mode, interact);
    tokio::select! {
        _ = cancel.cancelled() => Err(AppError::new(ErrorCode::ErrConnectionDropped, "Sambungan dibatalkan.")),
        r = work => r,
    }
}

async fn open_inner(db: &Db, server: &Server, mode: FtpMode, interact: Arc<dyn Interact>) -> Result<Box<dyn RemoteFs>> {
    // The password is read first so a missing one is asked before anything leaves the machine.
    let id = server.id.clone();
    let saved = db.read(move |c| secrets::load_secret(c, &id, SecretKind::Password)).ok().flatten();
    let password = match saved {
        Some(p) => p.to_string(),
        None => interact.secret(&server.name, SecretKind::Password).await.map(|s| s.expose().to_string()).ok_or_else(|| AppError::new(ErrorCode::ErrSshAuthFailed, format!("Password untuk \"{}\" tidak diberikan.", server.name)))?,
    };

    let addr = format!("{}:{}", server.host, server.port);
    let (tls, seen, pinned) = connector();
    let mut tls = Some(tls);
    let mut ftp = match mode {
        FtpMode::Implicit => tokio::time::timeout(CONNECT_TIMEOUT, AsyncRustlsFtpStream::connect_secure_implicit(addr.as_str(), tls.take().expect("used once"), &server.host)).await,
        _ => tokio::time::timeout(CONNECT_TIMEOUT, async {
            let tcp = TcpStream::connect(addr.as_str()).await.map_err(suppaftp::FtpError::ConnectionError)?;
            AsyncRustlsFtpStream::connect_with_stream(tcp).await
        })
        .await,
    }
    .map_err(|_| AppError::new(ErrorCode::ErrConnectionDropped, format!("Server {} tidak menjawab.", server.host)))?
    .map_err(ftp_err)?;

    if mode == FtpMode::Explicit {
        ftp = tokio::time::timeout(CONNECT_TIMEOUT, ftp.into_secure(tls.take().expect("used once"), &server.host)).await.map_err(|_| AppError::new(ErrorCode::ErrConnectionDropped, "Negosiasi TLS dengan server kehabisan waktu."))?.map_err(ftp_err)?;
    }
    if mode != FtpMode::Plain {
        let fingerprint = seen.lock().unwrap().clone().ok_or_else(|| AppError::new(ErrorCode::ErrConnectionDropped, "Server tidak menyajikan sertifikat."))?;
        pin(db, server, &fingerprint, interact.as_ref()).await?;
        // From here on (data connections too) only this certificate is accepted.
        *pinned.lock().unwrap() = Some(fingerprint);
    }

    ftp.login(server.username.as_str(), password.as_str()).await.map_err(|e| match e {
        suppaftp::FtpError::UnexpectedResponse(_) => AppError::new(ErrorCode::ErrSshAuthFailed, format!("Login ke \"{}\" ditolak: pengguna atau password salah.", server.name)),
        other => ftp_err(other),
    })?;
    within(ftp.transfer_type(FileType::Binary)).await?;
    Ok(Box::new(FtpFs { ftp }))
}

/// Compare the certificate fingerprint with the pinned one, or ask the user the first time.
async fn pin(db: &Db, server: &Server, fingerprint: &str, interact: &dyn Interact) -> Result<()> {
    let (host, port, fp) = (server.host.clone(), server.port, fingerprint.to_string());
    let status = db.read(move |c| hostkeys::check(c, &host, port, KEY_TYPE, &fp))?;
    match status {
        HostKeyStatus::Trusted => Ok(()),
        HostKeyStatus::Changed { known } => {
            let mut e = AppError::new(
                ErrorCode::ErrHostKeyChanged,
                format!("Sertifikat server \"{}\" ({}:{}) BERUBAH. Tersimpan {known}, sekarang {fingerprint}. Sambungan diblokir karena bisa jadi disadap; ganti hanya bila Anda yakin server memang diganti.", server.name, server.host, server.port),
            );
            e.details = Some(json!({ "host": server.host, "port": server.port, "key_type": KEY_TYPE, "known": known, "presented": fingerprint }));
            Err(e)
        }
        HostKeyStatus::Unknown => {
            if !interact.confirm_host_key(&server.host, server.port, KEY_TYPE, fingerprint).await {
                return Err(AppError::new(ErrorCode::ErrHostKeyUnknown, format!("Sertifikat server \"{}\" belum dikonfirmasi.", server.name)));
            }
            let (host, port, fp) = (server.host.clone(), server.port, fingerprint.to_string());
            db.write(move |c| hostkeys::trust(c, &host, port, KEY_TYPE, &fp)).await
        }
    }
}

pub struct FtpFs {
    ftp: AsyncRustlsFtpStream,
}

fn to_entry(dir: &str, f: &ListFile) -> Entry {
    let modified = f.modified().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs() as i64);
    Entry { name: f.name().to_string(), path: join_remote(dir, f.name()), is_dir: f.is_directory(), size: f.size() as u64, modified, is_link: f.is_symlink(), is_special: false }
}

/// An upload: writing goes to the data connection; `shutdown` completes the transfer on the control
/// connection (the final `226`), so a failed upload is reported instead of silently truncated.
struct FtpUpload {
    stream: Option<TransferStream<AsyncRustlsStream>>,
    finishing: Option<BoxFuture<'static, io::Result<()>>>,
}

impl AsyncWrite for FtpUpload {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.stream.as_mut() {
            Some(s) => Pin::new(s).poll_write(cx, buf),
            None => Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "transfer is finished"))),
        }
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.stream.as_mut() {
            Some(s) => Pin::new(s).poll_flush(cx),
            None => Poll::Ready(Ok(())),
        }
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.finishing.is_none() {
            let Some(s) = self.stream.take() else { return Poll::Ready(Ok(())) };
            self.finishing = Some(Box::pin(async move { s.finish().await.map_err(|e| io::Error::other(e.to_string())) }));
        }
        self.finishing.as_mut().unwrap().as_mut().poll(cx)
    }
}

struct FtpDownload(TransferStream<AsyncRustlsStream>);

impl AsyncRead for FtpDownload {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

#[async_trait]
impl RemoteFs for FtpFs {
    async fn list(&mut self, dir: &str) -> Result<Vec<Entry>> {
        let lines = within(self.ftp.list(Some(dir))).await?;
        Ok(lines.iter().filter_map(|l| ListFile::try_from(l.as_str()).ok()).filter(|f| f.name() != "." && f.name() != "..").map(|f| to_entry(dir, &f)).collect())
    }

    async fn stat(&mut self, path: &str) -> Result<Entry> {
        let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).to_string();
        if let Ok(Ok(size)) = tokio::time::timeout(COMMAND_TIMEOUT, self.ftp.size(path)).await {
            return Ok(Entry { name, path: path.to_string(), is_dir: false, size: size as u64, modified: None, is_link: false, is_special: false });
        }
        // Not a file with a size: a directory, if the server lets us enter it.
        if within(self.ftp.cwd(path)).await.is_ok() {
            return Ok(Entry { name, path: path.to_string(), is_dir: true, size: 0, modified: None, is_link: false, is_special: false });
        }
        // Servers without SIZE: look the name up in the parent folder's listing.
        let parent = super::parent_remote(path);
        self.list(&parent).await?.into_iter().find(|e| e.name == name).ok_or_else(|| AppError::new(ErrorCode::ErrNotFound, "Berkas atau folder tidak ditemukan di server."))
    }

    async fn mkdir(&mut self, path: &str) -> Result<()> {
        within(self.ftp.mkdir(path)).await
    }

    async fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        within(self.ftp.rename(from, to)).await
    }

    async fn remove(&mut self, path: &str, is_dir: bool) -> Result<()> {
        if is_dir {
            within(self.ftp.rmdir(path)).await
        } else {
            within(self.ftp.rm(path)).await
        }
    }

    async fn read_at(&mut self, path: &str, offset: u64, len: usize) -> Result<Vec<u8>> {
        if offset > 0 {
            within(self.ftp.resume_transfer(offset as usize)).await?;
        }
        let mut stream = within(self.ftp.retr_as_stream(path)).await?;
        let mut buf = vec![0u8; len];
        let mut got = 0;
        while got < len {
            let n = tokio::time::timeout(Duration::from_secs(30), stream.read(&mut buf[got..]))
                .await
                .map_err(|_| AppError::new(ErrorCode::ErrConnectionDropped, "Server tidak menjawab."))?
                .map_err(|e| AppError::new(ErrorCode::ErrRemoteFs, format!("Gagal membaca dari server: {e}")))?;
            if n == 0 {
                break;
            }
            got += n;
        }
        buf.truncate(got);
        if got < len {
            let _ = stream.finish().await;
        } else {
            // Cut the data connection short; the server's reply is drained by the next command.
            drop(stream);
        }
        Ok(buf)
    }

    async fn open_read(&mut self, path: &str, offset: u64) -> Result<RemoteReader> {
        if offset > 0 {
            within(self.ftp.resume_transfer(offset as usize)).await?;
        }
        let stream = within(self.ftp.retr_as_stream(path)).await?;
        Ok(Box::new(FtpDownload(stream)))
    }

    async fn open_write(&mut self, path: &str, offset: u64, append: bool) -> Result<RemoteWriter> {
        let stream = if append {
            within(self.ftp.append_with_stream(path)).await
        } else {
            if offset > 0 {
                within(self.ftp.resume_transfer(offset as usize)).await?;
            }
            within(self.ftp.put_with_stream(path)).await
        }?;
        Ok(Box::new(FtpUpload { stream: Some(stream), finishing: None }))
    }

    async fn home(&mut self) -> Result<String> {
        within(self.ftp.pwd()).await
    }

    async fn close(mut self: Box<Self>) {
        let _ = self.ftp.quit().await;
    }

}

#[cfg(test)]
mod pin_tests {
    use super::*;

    #[test]
    fn once_pinned_every_later_handshake_must_present_the_same_certificate() {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let seen = Arc::new(Mutex::new(None));
        let pinned = Arc::new(Mutex::new(None));
        let v = Capture { seen: seen.clone(), pinned: pinned.clone(), algs: provider.signature_verification_algorithms };
        let name = ServerName::try_from("127.0.0.1").unwrap();
        let (a, b) = (CertificateDer::from(vec![1u8, 2, 3]), CertificateDer::from(vec![9u8, 9, 9]));
        assert!(v.verify_server_cert(&a, &[], &name, &[], UnixTime::now()).is_ok(), "the first handshake is recorded, not judged");
        let first = seen.lock().unwrap().clone().unwrap();
        *pinned.lock().unwrap() = Some(first);
        assert!(v.verify_server_cert(&a, &[], &name, &[], UnixTime::now()).is_ok(), "the pinned certificate keeps working (data connections)");
        assert!(v.verify_server_cert(&b, &[], &name, &[], UnixTime::now()).is_err(), "a different certificate on a later connection is refused");
    }
}
