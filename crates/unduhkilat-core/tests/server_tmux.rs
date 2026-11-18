//! tmux di server SSH: memeriksa sesi dan membuka shell di dalam sesi (server uji di loopback).

#[path = "common/ssh_server.rs"]
mod ssh_server;

use futures_util::future::BoxFuture;
use ssh_server::{TestCred, TestSsh, TestSshConfig};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use unduhkilat_core::db::Db;
use unduhkilat_core::engine::proxy::{ProxyConfig, ProxyMode};
use unduhkilat_core::server::model::{AuthMethod, NewServer, Protocol};
use unduhkilat_core::server::secrets::{self, SecretKind, SecretString};
use unduhkilat_core::server::shell::open_shell_launch;
use unduhkilat_core::server::ssh::{connect, ConnectOptions, Interact, SshSession};
use unduhkilat_core::server::store;
use unduhkilat_core::server::tmux::{probe, ShellLaunch, TmuxProbe};
use unduhkilat_core::term::source::{SourceEvent, SourceIo};

struct Yes;
impl Interact for Yes {
    fn confirm_host_key<'a>(&'a self, _h: &'a str, _p: u16, _t: &'a str, _f: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async { true })
    }
    fn secret<'a>(&'a self, _n: &'a str, _k: SecretKind) -> BoxFuture<'a, Option<SecretString>> {
        Box::pin(async { None })
    }
}

fn server() -> TestSshConfig {
    TestSshConfig { users: vec![("eko".into(), TestCred::Password("pw".into()))], allow_forwarding: false, accept_delay: None, shell: true }
}

async fn session(ssh: &TestSsh) -> (SshSession, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("uk-tmux-{}", uuid::Uuid::new_v4()));
    let db = Db::open(dir.join("unduhkilat.db")).unwrap();
    let n = NewServer { name: "web".into(), protocol: Protocol::Ssh, host: "127.0.0.1".into(), port: Some(ssh.addr.port()), username: "eko".into(), auth: AuthMethod::Password, key_path: None, save_secret: true, hops: vec![] };
    let s = db.write_blocking(move |c| store::insert_server(c, &n)).unwrap();
    let id = s.id.clone();
    db.write_blocking(move |c| secrets::store_secret(c, &id, SecretKind::Password, "pw")).unwrap();
    let opts = ConnectOptions { proxy: ProxyConfig { mode: ProxyMode::Direct, ..Default::default() }, ..Default::default() };
    let sess = connect(&db, &s.id, Arc::new(Yes), opts, CancellationToken::new()).await.expect("connects");
    (sess, dir)
}

async fn read_until(io: &mut SourceIo, needle: &str) {
    let mut text = String::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(5), io.rx.recv()).await {
            Ok(Some(SourceEvent::Data(b))) => {
                text.push_str(&String::from_utf8_lossy(&b));
                if text.contains(needle) {
                    return;
                }
            }
            other => panic!("menunggu {needle:?}; dapat {other:?}, teks {text:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_lists_sessions_newest_first() {
    let ssh = TestSsh::start(server()).await;
    ssh.enable_tmux(true, "3 1 1700000100 toko\n1 0 1700000900 api\n");
    let (sess, dir) = session(&ssh).await;
    let TmuxProbe::Sessions(list) = probe(&sess).await else { panic!("harus daftar sesi") };
    assert_eq!(list.iter().map(|s| (s.name.as_str(), s.windows, s.attached)).collect::<Vec<_>>(), [("api", 1, 0), ("toko", 3, 1)]);
    sess.close().await;
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_reports_no_server_and_missing_tmux() {
    let ssh = TestSsh::start(server()).await;
    ssh.enable_tmux(true, "");
    let (sess, dir) = session(&ssh).await;
    assert_eq!(probe(&sess).await, TmuxProbe::Sessions(vec![]), "tidak ada server tmux: daftar kosong, bukan galat");
    sess.close().await;
    std::fs::remove_dir_all(dir).ok();

    let ssh = TestSsh::start(server()).await;
    ssh.enable_tmux(false, "");
    let (sess, dir) = session(&ssh).await;
    assert_eq!(probe(&sess).await, TmuxProbe::Missing);
    sess.close().await;
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_fails_when_server_refuses_exec() {
    // Server tanpa tiruan tmux: `exec` ditolak, jadi pengecekan gagal dengan pesan, bukan hang.
    let ssh = TestSsh::start(server()).await;
    let (sess, dir) = session(&ssh).await;
    assert!(matches!(probe(&sess).await, TmuxProbe::Failed(_)));
    sess.close().await;
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_and_new_run_the_tmux_command_on_a_pty() {
    let ssh = TestSsh::start(server()).await;
    ssh.enable_tmux(true, "1 0 5 api\n");
    let (sess, dir) = session(&ssh).await;
    let launch = ShellLaunch::TmuxAttach { name: "api".into(), take_over: true };
    let (mut io, _) = open_shell_launch(sess, 100, 30, false, &launch).await.expect("shell tmux terbuka");
    read_until(&mut io, "[tmux]").await;
    io.control.write(b"halo\n").unwrap();
    read_until(&mut io, "halo").await;
    io.control.close();
    std::fs::remove_dir_all(dir).ok();

    let log = ssh.exec_log();
    assert_eq!(log.len(), 1);
    assert!(log[0].contains("attach-session -d -t") && log[0].contains("=api"), "{log:?}");
    assert_eq!(ssh.pty_size(), Some((100, 30)), "PTY diminta sebelum perintah");

    let (sess, dir) = session(&ssh).await;
    let launch = ShellLaunch::TmuxNew { name: "blog".into() };
    let (mut io, _) = open_shell_launch(sess, 100, 30, false, &launch).await.expect("sesi baru terbuka");
    read_until(&mut io, "[tmux]").await;
    io.control.close();
    std::fs::remove_dir_all(dir).ok();
    assert!(ssh.exec_log()[1].contains("new-session -A -s"), "{:?}", ssh.exec_log());
}
