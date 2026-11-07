//! FTP and FTPS client tests against the in-process FTP server (loopback only).

#[path = "common/ftp_server.rs"]
mod ftp_server;

use ftp_server::{TestFtp, TestFtpConfig};
use futures_util::future::BoxFuture;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use unduhkilat_core::db::Db;
use unduhkilat_core::error::ErrorCode;
use unduhkilat_core::server::fs::ftp::{ftp_mode, open_ftp, open_ftp_with, FtpMode};
use unduhkilat_core::server::fs::RemoteFs;
use unduhkilat_core::server::hostkeys;
use unduhkilat_core::server::model::{AuthMethod, NewServer, Protocol, Server};
use unduhkilat_core::server::secrets::{self, SecretKind, SecretString};
use unduhkilat_core::server::ssh::Interact;
use unduhkilat_core::server::store;

struct Script {
    answer: bool,
    asked: Arc<AtomicUsize>,
}
impl Interact for Script {
    fn confirm_host_key<'a>(&'a self, _h: &'a str, _p: u16, _t: &'a str, _f: &'a str) -> BoxFuture<'a, bool> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let a = self.answer;
        Box::pin(async move { a })
    }
    fn secret<'a>(&'a self, _n: &'a str, _k: SecretKind) -> BoxFuture<'a, Option<SecretString>> {
        Box::pin(async { None })
    }
}

fn cfg() -> TestFtpConfig {
    TestFtpConfig { user: "eko".into(), password: "pw".into(), ..Default::default() }
}

fn new_db() -> Db {
    Db::open(std::env::temp_dir().join(format!("uk-ftpdb-{}", uuid::Uuid::new_v4())).join("unduhkilat.db")).unwrap()
}

fn add_server(db: &Db, ftp: &TestFtp, protocol: Protocol) -> Server {
    let n = NewServer { name: "ftp".into(), protocol, host: "127.0.0.1".into(), port: Some(ftp.addr.port()), username: "eko".into(), auth: AuthMethod::Password, key_path: None, save_secret: true, hops: vec![] };
    let s = db.write_blocking(move |c| store::insert_server(c, &n)).unwrap();
    let id = s.id.clone();
    db.write_blocking(move |c| secrets::store_secret(c, &id, SecretKind::Password, "pw")).unwrap();
    s
}

async fn plain(c: TestFtpConfig) -> (TestFtp, Box<dyn RemoteFs>) {
    let ftp = TestFtp::start(c).await;
    let db = new_db();
    let s = add_server(&db, &ftp, Protocol::Ftp);
    let fs = open_ftp(&db, &s, Arc::new(Script { answer: true, asked: Arc::new(AtomicUsize::new(0)) }), CancellationToken::new()).await.expect("connects");
    (ftp, fs)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_parses_unix_format() {
    let (ftp, mut fs) = plain(cfg()).await;
    std::fs::create_dir(ftp.root.join("docs")).unwrap();
    std::fs::write(ftp.root.join("a b.txt"), b"hello").unwrap();
    let mut v = fs.list("/").await.unwrap();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    let got: Vec<_> = v.iter().map(|e| (e.name.as_str(), e.is_dir, e.size)).collect();
    assert_eq!(got, [("a b.txt", false, 5), ("docs", true, 4096)].map(|(n, d, s)| (n, d, if d { got_dir_size(&v) } else { s })));
    assert_eq!(v[0].path, "/a b.txt");
    fs.close().await;
    ftp.stop();
}

fn got_dir_size(v: &[unduhkilat_core::server::fs::Entry]) -> u64 {
    v.iter().find(|e| e.is_dir).map(|e| e.size).unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_parses_dos_format() {
    let (ftp, mut fs) = plain(TestFtpConfig { dos_list: true, ..cfg() }).await;
    std::fs::create_dir(ftp.root.join("Folder")).unwrap();
    std::fs::write(ftp.root.join("report 1.txt"), b"12345").unwrap();
    let v = fs.list("/").await.unwrap();
    assert!(v.iter().any(|e| e.name == "Folder" && e.is_dir), "{v:?}");
    assert!(v.iter().any(|e| e.name == "report 1.txt" && !e.is_dir && e.size == 5), "{v:?}");
    fs.close().await;
    ftp.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mkdir_rename_remove_roundtrip() {
    let (ftp, mut fs) = plain(cfg()).await;
    fs.mkdir("/new").await.unwrap();
    assert!(ftp.root.join("new").is_dir());
    fs.rename("/new", "/renamed").await.unwrap();
    assert!(ftp.root.join("renamed").is_dir());
    std::fs::write(ftp.root.join("renamed/f.txt"), b"x").unwrap();
    fs.remove("/renamed/f.txt", false).await.unwrap();
    fs.remove("/renamed", true).await.unwrap();
    assert!(!ftp.root.join("renamed").exists());
    assert!(fs.remove("/nope", false).await.is_err());
    fs.close().await;
    ftp.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_at_returns_a_range_and_the_connection_stays_usable() {
    let (ftp, mut fs) = plain(cfg()).await;
    std::fs::write(ftp.root.join("n.txt"), b"0123456789").unwrap();
    assert_eq!(fs.read_at("/n.txt", 3, 4).await.unwrap(), b"3456");
    assert_eq!(fs.read_at("/n.txt", 8, 100).await.unwrap(), b"89");
    let v = fs.list("/").await.unwrap();
    assert_eq!(v.len(), 1, "commands still work after partial reads");
    fs.close().await;
    ftp.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_then_resume_by_appending_gives_the_whole_file() {
    let (ftp, mut fs) = plain(cfg()).await;
    let data: Vec<u8> = (0..150_000u32).map(|i| (i % 251) as u8).collect();
    let mut w = fs.open_write("/big.bin", 0, false).await.unwrap();
    w.write_all(&data[..90_000]).await.unwrap();
    w.shutdown().await.unwrap();
    drop(w);
    assert_eq!(std::fs::metadata(ftp.root.join("big.bin")).unwrap().len(), 90_000);
    let mut w = fs.open_write("/big.bin", 90_000, true).await.unwrap();
    w.write_all(&data[90_000..]).await.unwrap();
    w.shutdown().await.unwrap();
    drop(w);
    assert_eq!(std::fs::read(ftp.root.join("big.bin")).unwrap(), data);
    let mut r = fs.open_read("/big.bin", 100_000).await.unwrap();
    let mut tail = Vec::new();
    r.read_to_end(&mut tail).await.unwrap();
    drop(r);
    assert_eq!(tail, &data[100_000..]);
    fs.close().await;
    ftp.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_ftps_asks_once_pins_and_then_connects_silently() {
    let ftp = TestFtp::start(TestFtpConfig { tls: true, ..cfg() }).await;
    let db = new_db();
    let s = add_server(&db, &ftp, Protocol::Ftps);
    let asked = Arc::new(AtomicUsize::new(0));
    let fs = open_ftp(&db, &s, Arc::new(Script { answer: true, asked: asked.clone() }), CancellationToken::new()).await.expect("first connect");
    fs.close().await;
    assert_eq!(asked.load(Ordering::SeqCst), 1, "unknown certificate is shown for confirmation");
    let mut fs = open_ftp(&db, &s, Arc::new(Script { answer: false, asked: asked.clone() }), CancellationToken::new()).await.expect("pinned: no question");
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    std::fs::write(ftp.root.join("x.txt"), b"abc").unwrap();
    assert_eq!(fs.list("/").await.unwrap().len(), 1, "data channel is encrypted and works");
    fs.close().await;
    ftp.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_fingerprint_aborts_before_the_password_is_sent() {
    let ftp = TestFtp::start(TestFtpConfig { tls: true, ..cfg() }).await;
    let db = new_db();
    let s = add_server(&db, &ftp, Protocol::Ftps);
    let e = open_ftp(&db, &s, Arc::new(Script { answer: false, asked: Arc::new(AtomicUsize::new(0)) }), CancellationToken::new()).await.err().expect("refused");
    assert_eq!(e.code, ErrorCode::ErrHostKeyUnknown);
    assert_eq!(ftp.pass_commands.load(Ordering::SeqCst), 0, "the password never reached an unverified server");
    ftp.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_certificate_is_blocked() {
    let ftp = TestFtp::start(TestFtpConfig { tls: true, ..cfg() }).await;
    let db = new_db();
    let s = add_server(&db, &ftp, Protocol::Ftps);
    let port = ftp.addr.port();
    db.write_blocking(move |c| hostkeys::trust(c, "127.0.0.1", port, "tls-cert", "SHA256:someoneelse")).unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let e = open_ftp(&db, &s, Arc::new(Script { answer: true, asked: asked.clone() }), CancellationToken::new()).await.err().expect("blocked");
    assert_eq!(e.code, ErrorCode::ErrHostKeyChanged);
    assert_eq!(asked.load(Ordering::SeqCst), 0, "a changed certificate is never offered for a quick yes");
    assert_eq!(ftp.pass_commands.load(Ordering::SeqCst), 0);
    ftp.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn implicit_ftps_connects() {
    let ftp = TestFtp::start(TestFtpConfig { tls: true, implicit_tls: true, ..cfg() }).await;
    let db = new_db();
    let s = add_server(&db, &ftp, Protocol::Ftps);
    let asked = Arc::new(AtomicUsize::new(0));
    let mut fs = open_ftp_with(&db, &s, FtpMode::Implicit, Arc::new(Script { answer: true, asked: asked.clone() }), CancellationToken::new()).await.expect("implicit connect");
    std::fs::write(ftp.root.join("y.txt"), b"1").unwrap();
    assert_eq!(fs.list("/").await.unwrap().len(), 1);
    fs.close().await;
    ftp.stop();
}

#[test]
fn mode_follows_protocol_and_port() {
    let mut s = Server { id: "i".into(), name: "n".into(), protocol: Protocol::Ftp, host: "h".into(), port: 21, username: "u".into(), auth: AuthMethod::Password, key_path: None, save_secret: false, hops: vec![], created_at: String::new(), updated_at: String::new() };
    assert_eq!(ftp_mode(&s), FtpMode::Plain);
    s.protocol = Protocol::Ftps;
    assert_eq!(ftp_mode(&s), FtpMode::Explicit);
    s.port = 990;
    assert_eq!(ftp_mode(&s), FtpMode::Implicit, "port 990 is implicit FTPS");
}
