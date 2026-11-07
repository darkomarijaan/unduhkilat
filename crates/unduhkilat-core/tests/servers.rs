//! Server (SSH) integration tests. Every SSH server runs in-process on loopback:
//! nothing here touches the internet.

// Not part of `common/mod.rs`: other crates share that module and do not depend on russh.
#[path = "common/ssh_server.rs"]
mod ssh_server;

use ssh_server::{test_keypair, TestCred, TestSsh, TestSshConfig};
use futures_util::future::BoxFuture;
use russh::client;
use russh::keys::PublicKeyOrCertificate;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;
use unduhkilat_core::db::Db;
use unduhkilat_core::engine::proxy::{ProxyConfig, ProxyMode};
use unduhkilat_core::error::ErrorCode;
use unduhkilat_core::server::model::{AuthMethod, NewServer, Protocol, Server};
use unduhkilat_core::server::secrets::{self, SecretKind, SecretString};
use unduhkilat_core::server::secrets::NewServerSecrets;
use unduhkilat_core::server::service::ServerService;
use unduhkilat_core::server::ssh::{connect, ConnectOptions, Interact};
use unduhkilat_core::server::store;

/// Minimal client for harness checks: accepts any host key.
struct AnyKey;
impl client::Handler for AnyKey {
    type Error = russh::Error;
    async fn check_server_key(&mut self, _k: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

fn pw_server(user: &str, pw: &str, forwarding: bool) -> TestSshConfig {
    TestSshConfig { users: vec![(user.into(), TestCred::Password(pw.into()))], allow_forwarding: forwarding, accept_delay: None }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn harness_accepts_password_and_forwards_to_another_instance() {
    let first = TestSsh::start(pw_server("eko", "pw-1", true)).await;
    let second = TestSsh::start(pw_server("eko", "pw-2", false)).await;

    let tcp = tokio::net::TcpStream::connect(first.addr).await.unwrap();
    let mut h = client::connect_stream(Arc::new(client::Config::default()), tcp, AnyKey).await.unwrap();
    assert!(!h.authenticate_password("eko", "wrong").await.unwrap().success(), "wrong password is rejected");
    assert!(h.authenticate_password("eko", "pw-1").await.unwrap().success(), "right password is accepted");
    assert_eq!(first.login_count(), 1);

    // Forward to the second server and read its SSH banner through the first.
    let ch = h.channel_open_direct_tcpip(second.addr.ip().to_string(), second.addr.port() as u32, "127.0.0.1", 0).await.unwrap();
    let mut stream = ch.into_stream();
    let mut banner = [0u8; 8];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut banner)).await.expect("banner in time").unwrap();
    assert_eq!(&banner, b"SSH-2.0-");
    assert_eq!(first.forward_count(), 1);
    assert_eq!(second.login_count(), 0, "reading the banner is not a login");
}

// ---- test scaffolding for `server::ssh::connect` ------------------------------------------------

/// Stand-in for the UI: records what it was asked and answers from a script.
struct Fake {
    accept_keys: AtomicBool,
    key_asks: AtomicUsize,
    secret_asks: AtomicUsize,
    secrets: Mutex<HashMap<String, String>>,
}

impl Fake {
    fn new(accept_keys: bool) -> Arc<Fake> {
        Arc::new(Fake { accept_keys: AtomicBool::new(accept_keys), key_asks: AtomicUsize::new(0), secret_asks: AtomicUsize::new(0), secrets: Mutex::new(HashMap::new()) })
    }
    fn answer(self: &Arc<Self>, server: &str, kind: SecretKind, value: &str) -> Arc<Self> {
        self.secrets.lock().unwrap().insert(format!("{server}|{}", kind.as_str()), value.to_string());
        self.clone()
    }
}

impl Interact for Fake {
    fn confirm_host_key<'a>(&'a self, _host: &'a str, _port: u16, _key_type: &'a str, _fingerprint: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            self.key_asks.fetch_add(1, Ordering::SeqCst);
            self.accept_keys.load(Ordering::SeqCst)
        })
    }
    fn secret<'a>(&'a self, server_name: &'a str, kind: SecretKind) -> BoxFuture<'a, Option<SecretString>> {
        Box::pin(async move {
            self.secret_asks.fetch_add(1, Ordering::SeqCst);
            self.secrets.lock().unwrap().get(&format!("{server_name}|{}", kind.as_str())).map(|v| SecretString::from(v.as_str()))
        })
    }
}

struct Env {
    db: Db,
    dir: PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn env() -> Env {
    let dir = std::env::temp_dir().join(format!("uk-ssh-{}", uuid::Uuid::new_v4()));
    let db = Db::open(dir.join("unduhkilat.db")).unwrap();
    Env { db, dir }
}

fn opts() -> ConnectOptions {
    ConnectOptions { connect_timeout: Duration::from_secs(3), auth_timeout: Duration::from_secs(5), proxy: ProxyConfig { mode: ProxyMode::Direct, ..Default::default() } }
}

impl Env {
    /// Register `ssh` as a saved server called `name`.
    fn add(&self, name: &str, ssh: &TestSsh, user: &str, auth: AuthMethod, key_path: Option<String>, save_secret: bool, hops: Vec<String>) -> Server {
        let n = NewServer { name: name.into(), protocol: Protocol::Ssh, host: "127.0.0.1".into(), port: Some(ssh.addr.port()), username: user.into(), auth, key_path, save_secret, hops };
        self.db.write_blocking(move |c| store::insert_server(c, &n)).unwrap()
    }
    fn save_secret(&self, server: &Server, kind: SecretKind, value: &str) {
        let (id, v) = (server.id.clone(), value.to_string());
        self.db.write_blocking(move |c| secrets::store_secret(c, &id, kind, &v)).unwrap();
    }
    fn host_key_rows(&self) -> Vec<String> {
        self.db
            .read(|c| {
                let mut st = c.prepare("SELECT fingerprint FROM host_keys ORDER BY host, port")?;
                let rows = st.query_map([], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .unwrap()
    }
    fn secret_rows(&self) -> usize {
        self.db.read(|c| Ok(c.query_row("SELECT COUNT(*) FROM server_secrets", [], |r| r.get::<_, i64>(0))? as usize)).unwrap()
    }
}

async fn go(env: &Env, id: &str, fake: &Arc<Fake>, opts: ConnectOptions) -> unduhkilat_core::Result<unduhkilat_core::server::ssh::SshSession> {
    connect(&env.db, id, fake.clone(), opts, CancellationToken::new()).await
}

// ---- connect: authentication ------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn password_login_reaches_the_target() {
    let (env, ssh, fake) = (env(), TestSsh::start(pw_server("eko", "pw-t", false)).await, Fake::new(true));
    let web = env.add("web", &ssh, "eko", AuthMethod::Password, None, true, vec![]);
    env.save_secret(&web, SecretKind::Password, "pw-t");
    let session = go(&env, &web.id, &fake, opts()).await.expect("connects");
    assert_eq!(session.target.name, "web");
    assert_eq!(ssh.login_count(), 1);
    assert_eq!(fake.secret_asks.load(Ordering::SeqCst), 0, "a saved secret is used without asking");
    session.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn key_login_with_passphrase_asks_interact_when_not_saved() {
    let env = env();
    let (private, public) = test_keypair(Some("frasa"));
    let key_file = env.dir.join("id_ed25519");
    std::fs::create_dir_all(&env.dir).unwrap();
    std::fs::write(&key_file, private).unwrap();
    let ssh = TestSsh::start(TestSshConfig { users: vec![("eko".into(), TestCred::Key(public))], ..Default::default() }).await;
    let web = env.add("web", &ssh, "eko", AuthMethod::Key, Some(key_file.to_string_lossy().into_owned()), false, vec![]);

    let right = Fake::new(true).answer("web", SecretKind::Passphrase, "frasa");
    let session = go(&env, &web.id, &right, opts()).await.expect("key login works");
    assert_eq!(right.secret_asks.load(Ordering::SeqCst), 1, "the passphrase was asked once");
    assert_eq!(env.secret_rows(), 0, "an unsaved passphrase is never persisted");
    session.close().await;

    let wrong = Fake::new(true).answer("web", SecretKind::Passphrase, "salah");
    let e = go(&env, &web.id, &wrong, opts()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ErrSshAuthFailed, "a wrong passphrase is an auth failure: {e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_key_file_is_file_not_found() {
    let (env, ssh, fake) = (env(), TestSsh::start(pw_server("eko", "x", false)).await, Fake::new(true));
    let web = env.add("web", &ssh, "eko", AuthMethod::Key, Some(env.dir.join("nope").to_string_lossy().into_owned()), false, vec![]);
    assert_eq!(go(&env, &web.id, &fake, opts()).await.unwrap_err().code, ErrorCode::ErrFileNotFound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_ssh_target_is_refused() {
    let env = env();
    let n = NewServer { name: "old".into(), protocol: Protocol::Ftp, host: "127.0.0.1".into(), port: Some(21), username: "u".into(), auth: AuthMethod::Password, key_path: None, save_secret: false, hops: vec![] };
    let s = env.db.write_blocking(move |c| store::insert_server(c, &n)).unwrap();
    assert_eq!(go(&env, &s.id, &Fake::new(true), opts()).await.unwrap_err().code, ErrorCode::ErrInvalidArgument);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asked_secret_is_saved_only_when_save_secret_is_on() {
    let env = env();
    let (a, b) = (TestSsh::start(pw_server("eko", "pw", false)).await, TestSsh::start(pw_server("eko", "pw", false)).await);
    let keep = env.add("keep", &a, "eko", AuthMethod::Password, None, true, vec![]);
    let drop_ = env.add("drop", &b, "eko", AuthMethod::Password, None, false, vec![]);
    let fake = Fake::new(true).answer("keep", SecretKind::Password, "pw").answer("drop", SecretKind::Password, "pw");
    go(&env, &keep.id, &fake, opts()).await.unwrap().close().await;
    assert_eq!(env.secret_rows(), 1, "typed secret kept because save_secret is on");
    go(&env, &drop_.id, &fake, opts()).await.unwrap().close().await;
    assert_eq!(env.secret_rows(), 1, "typed secret not kept when save_secret is off");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreadable_saved_secret_falls_back_to_asking() {
    let (env, ssh) = (env(), TestSsh::start(pw_server("eko", "pw", false)).await);
    let web = env.add("web", &ssh, "eko", AuthMethod::Password, None, true, vec![]);
    let id = web.id.clone();
    env.db.write_blocking(move |c| Ok(c.execute("INSERT INTO server_secrets(server_id,kind,ciphertext) VALUES(?1,'password','AAECAwQFBgcICQ==')", [&id])?)).unwrap();
    let fake = Fake::new(true).answer("web", SecretKind::Password, "pw");
    go(&env, &web.id, &fake, opts()).await.expect("falls back to asking").close().await;
    assert_eq!(fake.secret_asks.load(Ordering::SeqCst), 1);
}

// ---- connect: jump chains ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_hop_chain_connects_through_both() {
    let env = env();
    let b = TestSsh::start(pw_server("eko", "pb", true)).await;
    let a = TestSsh::start(pw_server("eko", "pa", false)).await;
    let bastion = env.add("Bastion", &b, "eko", AuthMethod::Password, None, true, vec![]);
    let prod = env.add("Prod", &a, "eko", AuthMethod::Password, None, true, vec![bastion.id.clone()]);
    env.save_secret(&bastion, SecretKind::Password, "pb");
    env.save_secret(&prod, SecretKind::Password, "pa");
    let session = go(&env, &prod.id, &Fake::new(true), opts()).await.expect("chain connects");
    assert_eq!((b.login_count(), a.login_count()), (1, 1), "both hops authenticated");
    assert_eq!(b.forward_count(), 1, "the target really was reached through the bastion");
    session.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failing_hop_error_names_the_hop() {
    let env = env();
    let b = TestSsh::start(pw_server("eko", "right", true)).await;
    let a = TestSsh::start(pw_server("eko", "pa", false)).await;
    let bastion = env.add("Bastion", &b, "eko", AuthMethod::Password, None, true, vec![]);
    let prod = env.add("Prod", &a, "eko", AuthMethod::Password, None, true, vec![bastion.id.clone()]);
    env.save_secret(&bastion, SecretKind::Password, "wrong");
    env.save_secret(&prod, SecretKind::Password, "pa");
    let e = go(&env, &prod.id, &Fake::new(true), opts()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ErrJumpFailed, "{e}");
    assert!(e.message.contains("Bastion"), "message names the failing hop: {}", e.message);
    assert_eq!(a.login_count(), 0, "the target was never contacted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn max_five_hops_is_enforced_on_connect() {
    let env = env();
    let ssh = TestSsh::start(pw_server("eko", "pw", true)).await;
    let hops: Vec<Server> = (1..=6).map(|i| env.add(&format!("hop{i}"), &ssh, "eko", AuthMethod::Password, None, false, vec![])).collect();
    let target = env.add("Target", &ssh, "eko", AuthMethod::Password, None, false, vec![]);
    // The store refuses six hops, so write the chain by hand to prove `connect` checks too.
    let (tid, ids): (String, Vec<String>) = (target.id.clone(), hops.iter().map(|h| h.id.clone()).collect());
    env.db
        .write_blocking(move |c| {
            for (i, h) in ids.iter().enumerate() {
                c.execute("INSERT INTO server_hops(server_id,position,hop_id) VALUES(?1,?2,?3)", rusqlite::params![tid, i as i64, h])?;
            }
            Ok(())
        })
        .unwrap();
    let e = go(&env, &target.id, &Fake::new(true), opts()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ErrTooManyHops);
    assert_eq!(ssh.login_count() + ssh.forward_count(), 0, "no socket work happened before the check");
}

// ---- connect: host keys -----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_host_key_asks_once_and_rejection_aborts_without_saving() {
    let (env, ssh, fake) = (env(), TestSsh::start(pw_server("eko", "pw", false)).await, Fake::new(false));
    let web = env.add("web", &ssh, "eko", AuthMethod::Password, None, true, vec![]);
    env.save_secret(&web, SecretKind::Password, "pw");
    let e = go(&env, &web.id, &fake, opts()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ErrHostKeyUnknown, "{e}");
    assert_eq!(fake.key_asks.load(Ordering::SeqCst), 1);
    assert!(env.host_key_rows().is_empty(), "a rejected key is not saved");
    assert_eq!(ssh.login_count(), 0, "no login is attempted on an unconfirmed host");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_host_key_is_saved_and_not_asked_again() {
    let (env, ssh, fake) = (env(), TestSsh::start(pw_server("eko", "pw", false)).await, Fake::new(true));
    let web = env.add("web", &ssh, "eko", AuthMethod::Password, None, true, vec![]);
    env.save_secret(&web, SecretKind::Password, "pw");
    go(&env, &web.id, &fake, opts()).await.unwrap().close().await;
    assert_eq!(fake.key_asks.load(Ordering::SeqCst), 1);
    assert_eq!(env.host_key_rows(), vec![ssh.host_key_fingerprint()], "the saved fingerprint equals what the server presented (SHA256:...)");
    go(&env, &web.id, &fake, opts()).await.unwrap().close().await;
    assert_eq!(fake.key_asks.load(Ordering::SeqCst), 1, "a known key is not asked about again");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changed_host_key_on_the_middle_hop_is_blocked() {
    let env = env();
    let b = TestSsh::start(pw_server("eko", "pb", true)).await;
    let a = TestSsh::start(pw_server("eko", "pa", false)).await;
    let bastion = env.add("Bastion", &b, "eko", AuthMethod::Password, None, true, vec![]);
    let prod = env.add("Prod", &a, "eko", AuthMethod::Password, None, true, vec![bastion.id.clone()]);
    env.save_secret(&bastion, SecretKind::Password, "pb");
    env.save_secret(&prod, SecretKind::Password, "pa");
    go(&env, &prod.id, &Fake::new(true), opts()).await.unwrap().close().await; // pins both keys
    let (logins_a, forwards_b) = (a.login_count(), b.forward_count());

    b.rotate_host_key();
    let fake = Fake::new(true);
    let e = go(&env, &prod.id, &fake, opts()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ErrHostKeyChanged, "{e}");
    assert!(e.message.contains("Bastion"), "message names the hop whose key changed: {}", e.message);
    assert_eq!(fake.key_asks.load(Ordering::SeqCst), 0, "a changed key is never offered for one-click approval");
    assert_eq!((a.login_count(), b.forward_count()), (logins_a, forwards_b), "nothing further was contacted");
}

// ---- connect: slow hops, cancellation, proxy --------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dead_hop_times_out() {
    let env = env();
    let dead = TestSsh::start(TestSshConfig { accept_delay: Some(Duration::from_secs(10)), ..pw_server("eko", "pw", true) }).await;
    let live = TestSsh::start(pw_server("eko", "pw", false)).await;
    let o = ConnectOptions { connect_timeout: Duration::from_secs(1), ..opts() };

    let target = env.add("Dead", &dead, "eko", AuthMethod::Password, None, false, vec![]);
    let t = Instant::now();
    let e = go(&env, &target.id, &Fake::new(true), o.clone()).await.unwrap_err();
    assert!(t.elapsed() < Duration::from_secs(3), "gave up in {:?}", t.elapsed());
    assert_eq!(e.code, ErrorCode::ErrConnectionDropped, "{e}");

    let behind = env.add("Behind", &live, "eko", AuthMethod::Password, None, false, vec![target.id.clone()]);
    let t = Instant::now();
    let e = go(&env, &behind.id, &Fake::new(true), o).await.unwrap_err();
    assert!(t.elapsed() < Duration::from_secs(3));
    assert_eq!(e.code, ErrorCode::ErrJumpFailed, "{e}");
    assert!(e.message.contains("Dead"), "message names the slow hop: {}", e.message);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_aborts_a_pending_connect() {
    let env = env();
    let dead = TestSsh::start(TestSshConfig { accept_delay: Some(Duration::from_secs(10)), ..pw_server("eko", "pw", false) }).await;
    let web = env.add("web", &dead, "eko", AuthMethod::Password, None, false, vec![]);
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        c2.cancel();
    });
    let o = ConnectOptions { connect_timeout: Duration::from_secs(30), ..opts() };
    let t = Instant::now();
    let e = connect(&env.db, &web.id, Fake::new(true), o, cancel).await.unwrap_err();
    assert!(t.elapsed() < Duration::from_secs(1), "returned after {:?}", t.elapsed());
    assert!(e.message.contains("dibatalkan"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn socks5_proxy_is_refused_clearly() {
    let (env, ssh) = (env(), TestSsh::start(pw_server("eko", "pw", false)).await);
    let web = env.add("web", &ssh, "eko", AuthMethod::Password, None, false, vec![]);
    let o = ConnectOptions { proxy: ProxyConfig { mode: ProxyMode::Manual, host: "socks5://127.0.0.1".into(), port: 1080, ..Default::default() }, ..opts() };
    let e = go(&env, &web.id, &Fake::new(true), o).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ErrInvalidArgument);
    assert!(e.message.contains("SOCKS5"), "{e}");
    assert_eq!(ssh.login_count(), 0);
}

// ---- secrets never leak -----------------------------------------------------------------------

#[derive(Clone, Default)]
struct LogBuf(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for LogBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
    type Writer = LogBuf;
    fn make_writer(&'a self) -> LogBuf {
        self.clone()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn wrong_password_error_never_contains_the_password() {
    let buf = LogBuf::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::fmt().with_max_level(tracing::Level::TRACE).with_ansi(false).with_writer(buf.clone()).finish());
    let (env, ssh) = (env(), TestSsh::start(pw_server("eko", "the-right-one", false)).await);
    let web = env.add("web", &ssh, "eko", AuthMethod::Password, None, true, vec![]);
    env.save_secret(&web, SecretKind::Password, "s3cr3t-XYZ");
    let e = go(&env, &web.id, &Fake::new(true), opts()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ErrSshAuthFailed, "{e}");
    let everything = format!("{e} {e:?} {}", String::from_utf8_lossy(&buf.0.lock().unwrap()));
    assert!(!everything.contains("s3cr3t-XYZ"), "the password leaked into the error or the log: {everything}");
}

// ---- ServerService ----------------------------------------------------------------------------

fn service(env: &Env) -> ServerService {
    ServerService::new(env.db.clone(), Arc::new(|| ProxyConfig { mode: ProxyMode::Direct, ..Default::default() }))
}

fn new_server(name: &str, ssh: &TestSsh, save_secret: bool, hops: Vec<String>) -> NewServer {
    NewServer { name: name.into(), protocol: Protocol::Ssh, host: "127.0.0.1".into(), port: Some(ssh.addr.port()), username: "eko".into(), auth: AuthMethod::Password, key_path: None, save_secret, hops }
}

fn password(p: &str) -> NewServerSecrets {
    NewServerSecrets { password: Some(SecretString::from(p)), passphrase: None }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn service_saves_lists_tests_and_deletes() {
    let env = env();
    let svc = service(&env);
    let b = TestSsh::start(pw_server("eko", "pb", true)).await;
    let a = TestSsh::start(pw_server("eko", "pa", false)).await;
    let bastion = svc.save(None, new_server("B", &b, true, vec![]), password("pb")).await.unwrap();
    let prod = svc.save(None, new_server("A", &a, true, vec![bastion.id.clone()]), password("pa")).await.unwrap();

    let names: Vec<String> = svc.list().await.unwrap().into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["A", "B"]);

    let report = svc.test_connection(&prod.id, Fake::new(true), CancellationToken::new()).await.expect("test connection works");
    assert_eq!(report.hop_names, ["B", "A"]);
    assert_eq!((b.login_count(), a.login_count()), (1, 1));

    assert_eq!(svc.delete(&bastion.id).await.unwrap_err().code, ErrorCode::ErrServerInUse, "B is still a hop of A");
    svc.delete(&prod.id).await.unwrap();
    svc.delete(&bastion.id).await.unwrap();
    assert!(svc.list().await.unwrap().is_empty());
    assert_eq!(env.secret_rows(), 0, "deleting a server removes its saved secrets");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secrets_follow_the_save_secret_switch() {
    let env = env();
    let svc = service(&env);
    let ssh = TestSsh::start(pw_server("eko", "pw", false)).await;

    let s = svc.save(None, new_server("web", &ssh, false, vec![]), password("pw")).await.unwrap();
    assert_eq!(env.secret_rows(), 0, "save_secret off: the typed password is not persisted");

    let s = svc.save(Some(s.id.clone()), new_server("web", &ssh, true, vec![]), password("pw")).await.unwrap();
    assert_eq!(env.secret_rows(), 1, "save_secret on: it is persisted");

    svc.save(Some(s.id.clone()), new_server("web", &ssh, true, vec![]), NewServerSecrets::default()).await.unwrap();
    assert_eq!(env.secret_rows(), 1, "no new secret typed: the saved one is kept");

    svc.save(Some(s.id.clone()), new_server("web", &ssh, false, vec![]), NewServerSecrets::default()).await.unwrap();
    assert_eq!(env.secret_rows(), 0, "turning save_secret off wipes the saved secrets");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forgetting_a_host_key_makes_the_next_connection_ask_again() {
    let env = env();
    let svc = service(&env);
    let ssh = TestSsh::start(pw_server("eko", "pw", false)).await;
    let s = svc.save(None, new_server("web", &ssh, true, vec![]), password("pw")).await.unwrap();
    let fake = Fake::new(true);
    svc.test_connection(&s.id, fake.clone(), CancellationToken::new()).await.unwrap();
    svc.test_connection(&s.id, fake.clone(), CancellationToken::new()).await.unwrap();
    assert_eq!(fake.key_asks.load(Ordering::SeqCst), 1);
    svc.forget_host_key("127.0.0.1", ssh.addr.port()).await.unwrap();
    svc.test_connection(&s.id, fake.clone(), CancellationToken::new()).await.unwrap();
    assert_eq!(fake.key_asks.load(Ordering::SeqCst), 2, "asked again after forgetting");
}
