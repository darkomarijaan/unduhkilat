//! SFTP client tests against the in-process SSH server serving a temporary folder (loopback only).

#[path = "common/ssh_server.rs"]
mod ssh_server;

use futures_util::future::BoxFuture;
use ssh_server::{TestCred, TestSsh, TestSshConfig};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use unduhkilat_core::db::Db;
use unduhkilat_core::engine::proxy::{ProxyConfig, ProxyMode};
use unduhkilat_core::server::fs::sftp::open_sftp;
use unduhkilat_core::server::fs::RemoteFs;
use unduhkilat_core::server::model::{AuthMethod, NewServer, Protocol};
use unduhkilat_core::server::secrets::{self, SecretKind, SecretString};
use unduhkilat_core::server::ssh::{connect, ConnectOptions, Interact};
use unduhkilat_core::server::store;

struct Yes;
impl Interact for Yes {
    fn confirm_host_key<'a>(&'a self, _h: &'a str, _p: u16, _t: &'a str, _f: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async { true })
    }
    fn secret<'a>(&'a self, _n: &'a str, _k: SecretKind) -> BoxFuture<'a, Option<SecretString>> {
        Box::pin(async { None })
    }
}

struct Rig {
    ssh: TestSsh,
    root: PathBuf,
    fs: Box<dyn RemoteFs>,
}

async fn rig() -> Rig {
    let root = std::env::temp_dir().join(format!("uk-sftp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let cfg = TestSshConfig { users: vec![("eko".into(), TestCred::Password("pw".into()))], allow_forwarding: false, accept_delay: None, shell: false };
    let ssh = TestSsh::start_with_sftp(cfg, root.clone()).await;
    let db = Db::open(root.join("..").join(format!("uk-db-{}.db", uuid::Uuid::new_v4()))).unwrap();
    let n = NewServer { name: "nas".into(), protocol: Protocol::Sftp, host: "127.0.0.1".into(), port: Some(ssh.addr.port()), username: "eko".into(), auth: AuthMethod::Password, key_path: None, save_secret: true, hops: vec![] };
    let s = db.write_blocking(move |c| store::insert_server(c, &n)).unwrap();
    let id = s.id.clone();
    db.write_blocking(move |c| secrets::store_secret(c, &id, SecretKind::Password, "pw")).unwrap();
    let opts = ConnectOptions { proxy: ProxyConfig { mode: ProxyMode::Direct, ..Default::default() }, ..Default::default() };
    let session = connect(&db, &s.id, Arc::new(Yes), opts, CancellationToken::new()).await.expect("connects");
    let fs = open_sftp(session).await.expect("sftp opens");
    Rig { ssh, root, fs }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_returns_files_and_folders_with_sizes() {
    let mut r = rig().await;
    std::fs::create_dir(r.root.join("docs")).unwrap();
    std::fs::write(r.root.join("a.txt"), b"hello").unwrap();
    std::fs::write(r.root.join("empty.bin"), b"").unwrap();
    let mut v = r.fs.list("/").await.unwrap();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<_> = v.iter().map(|e| (e.name.as_str(), e.is_dir, e.size)).collect();
    assert_eq!(names, [("a.txt", false, 5), ("docs", true, 0), ("empty.bin", false, 0)]);
    assert_eq!(v[0].path, "/a.txt");
    r.fs.close().await;
    r.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mkdir_rename_remove_roundtrip() {
    let mut r = rig().await;
    r.fs.mkdir("/new").await.unwrap();
    assert!(r.root.join("new").is_dir());
    r.fs.rename("/new", "/renamed").await.unwrap();
    assert!(r.root.join("renamed").is_dir() && !r.root.join("new").exists());
    std::fs::write(r.root.join("renamed/f.txt"), b"x").unwrap();
    r.fs.remove("/renamed/f.txt", false).await.unwrap();
    r.fs.remove("/renamed", true).await.unwrap();
    assert!(!r.root.join("renamed").exists());
    let e = r.fs.remove("/nope", false).await.unwrap_err();
    assert!(!e.message.is_empty());
    r.fs.close().await;
    r.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_at_returns_a_range() {
    let mut r = rig().await;
    std::fs::write(r.root.join("n.txt"), b"0123456789").unwrap();
    assert_eq!(r.fs.read_at("/n.txt", 3, 4).await.unwrap(), b"3456");
    assert_eq!(r.fs.read_at("/n.txt", 8, 100).await.unwrap(), b"89", "fewer bytes at the end");
    assert!(r.fs.read_at("/n.txt", 50, 10).await.unwrap().is_empty(), "past the end is empty, not an error");
    r.fs.close().await;
    r.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_then_resume_from_offset_gives_the_whole_file() {
    let mut r = rig().await;
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let mut w = r.fs.open_write("/big.bin", 0, false).await.unwrap();
    w.write_all(&data[..120_000]).await.unwrap();
    w.shutdown().await.unwrap();
    drop(w);
    assert_eq!(std::fs::metadata(r.root.join("big.bin")).unwrap().len(), 120_000);
    let mut w = r.fs.open_write("/big.bin", 120_000, true).await.unwrap();
    w.write_all(&data[120_000..]).await.unwrap();
    w.shutdown().await.unwrap();
    drop(w);
    assert_eq!(std::fs::read(r.root.join("big.bin")).unwrap(), data);
    let mut rd = r.fs.open_read("/big.bin", 150_000).await.unwrap();
    let mut tail = Vec::new();
    rd.read_to_end(&mut tail).await.unwrap();
    assert_eq!(tail, &data[150_000..]);
    r.fs.close().await;
    r.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unicode_and_space_names_roundtrip() {
    let mut r = rig().await;
    let name = "laporan \u{4F60}\u{597D} 2026.txt";
    let mut w = r.fs.open_write(&format!("/{name}"), 0, false).await.unwrap();
    w.write_all(b"ok").await.unwrap();
    w.shutdown().await.unwrap();
    drop(w);
    let v = r.fs.list("/").await.unwrap();
    assert!(v.iter().any(|e| e.name == name && e.size == 2), "{v:?}");
    r.fs.close().await;
    r.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_server_fails_calls_instead_of_hanging() {
    let mut r = rig().await;
    r.ssh.kill_connections().await;
    let res = tokio::time::timeout(std::time::Duration::from_secs(20), r.fs.list("/")).await;
    assert!(matches!(res, Ok(Err(_))), "must fail, not hang: {res:?}");
}
