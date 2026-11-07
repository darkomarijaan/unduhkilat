//! `ServerService::open_fs` picks the right client for each protocol (loopback servers only).

#[path = "common/ftp_server.rs"]
mod ftp_server;
#[path = "common/ssh_server.rs"]
mod ssh_server;

use ftp_server::{TestFtp, TestFtpConfig};
use futures_util::future::BoxFuture;
use ssh_server::{TestCred, TestSsh, TestSshConfig};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use unduhkilat_core::db::Db;
use unduhkilat_core::engine::proxy::{ProxyConfig, ProxyMode};
use unduhkilat_core::error::ErrorCode;
use unduhkilat_core::server::model::{AuthMethod, NewServer, Protocol};
use unduhkilat_core::server::secrets::{self, SecretKind, SecretString};
use unduhkilat_core::server::service::ServerService;
use unduhkilat_core::server::ssh::Interact;
use unduhkilat_core::server::store;

struct Script {
    host_keys: Arc<AtomicUsize>,
    secrets: Arc<AtomicUsize>,
    password: Option<&'static str>,
}
impl Interact for Script {
    fn confirm_host_key<'a>(&'a self, _h: &'a str, _p: u16, _t: &'a str, _f: &'a str) -> BoxFuture<'a, bool> {
        self.host_keys.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { true })
    }
    fn secret<'a>(&'a self, _n: &'a str, _k: SecretKind) -> BoxFuture<'a, Option<SecretString>> {
        self.secrets.fetch_add(1, Ordering::SeqCst);
        let p = self.password;
        Box::pin(async move { p.map(SecretString::from) })
    }
}

fn counters(password: Option<&'static str>) -> (Arc<Script>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let (h, s) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    (Arc::new(Script { host_keys: h.clone(), secrets: s.clone(), password }), h, s)
}

fn service() -> (ServerService, Db) {
    let db = Db::open(std::env::temp_dir().join(format!("uk-openfs-{}", uuid::Uuid::new_v4())).join("unduhkilat.db")).unwrap();
    (ServerService::new(db.clone(), Arc::new(|| ProxyConfig { mode: ProxyMode::Direct, ..Default::default() })), db)
}

fn add(db: &Db, protocol: Protocol, port: u16, save: bool) -> String {
    let n = NewServer { name: "srv".into(), protocol, host: "127.0.0.1".into(), port: Some(port), username: "eko".into(), auth: AuthMethod::Password, key_path: None, save_secret: save, hops: vec![] };
    let s = db.write_blocking(move |c| store::insert_server(c, &n)).unwrap();
    if save {
        let id = s.id.clone();
        db.write_blocking(move |c| secrets::store_secret(c, &id, SecretKind::Password, "pw")).unwrap();
    }
    s.id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_fs_uses_sftp_for_ssh_servers_and_pins_host_keys() {
    let root = std::env::temp_dir().join(format!("uk-openfs-root-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("hello.txt"), b"hi").unwrap();
    let ssh = TestSsh::start_with_sftp(TestSshConfig { users: vec![("eko".into(), TestCred::Password("pw".into()))], allow_forwarding: false, accept_delay: None, shell: false }, root).await;
    let (svc, db) = service();
    let id = add(&db, Protocol::Ssh, ssh.addr.port(), true);
    let (script, hk, _) = counters(None);
    let mut fs = svc.open_fs(&id, script.clone(), CancellationToken::new()).await.expect("opens");
    assert_eq!(fs.list("/").await.unwrap().len(), 1);
    fs.close().await;
    assert_eq!(hk.load(Ordering::SeqCst), 1, "unknown host key is asked once");
    let fs = svc.open_fs(&id, script, CancellationToken::new()).await.expect("opens again");
    fs.close().await;
    assert_eq!(hk.load(Ordering::SeqCst), 1, "pinned now: no second question");
    ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_fs_ftp_without_saved_password_asks_the_user() {
    let ftp = TestFtp::start(TestFtpConfig { user: "eko".into(), password: "pw".into(), ..Default::default() }).await;
    std::fs::write(ftp.root.join("a.txt"), b"x").unwrap();
    let (svc, db) = service();
    let id = add(&db, Protocol::Ftp, ftp.addr.port(), false);
    let (script, _, asked) = counters(Some("pw"));
    let mut fs = svc.open_fs(&id, script, CancellationToken::new()).await.expect("opens with the typed password");
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    assert_eq!(fs.list("/").await.unwrap().len(), 1);
    fs.close().await;
    let (give_up, _, _) = counters(None);
    let e = svc.open_fs(&id, give_up, CancellationToken::new()).await.err().expect("no password, no connection");
    assert_eq!(e.code, ErrorCode::ErrSshAuthFailed);
    ftp.stop();
}
