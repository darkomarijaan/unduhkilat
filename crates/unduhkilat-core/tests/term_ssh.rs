//! SSH shell tests (PTY channel) against the in-process test server on loopback.

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
use unduhkilat_core::server::shell::open_shell;
use unduhkilat_core::server::ssh::{connect, ConnectOptions, Interact, SshSession};
use unduhkilat_core::server::store;
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

fn shell_server(shell: bool) -> TestSshConfig {
    TestSshConfig { users: vec![("eko".into(), TestCred::Password("pw".into()))], allow_forwarding: false, accept_delay: None, shell }
}

async fn session(ssh: &TestSsh) -> (SshSession, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("uk-shell-{}", uuid::Uuid::new_v4()));
    let db = Db::open(dir.join("unduhkilat.db")).unwrap();
    let n = NewServer { name: "web".into(), protocol: Protocol::Ssh, host: "127.0.0.1".into(), port: Some(ssh.addr.port()), username: "eko".into(), auth: AuthMethod::Password, key_path: None, save_secret: true, hops: vec![] };
    let s = db.write_blocking(move |c| store::insert_server(c, &n)).unwrap();
    let id = s.id.clone();
    db.write_blocking(move |c| secrets::store_secret(c, &id, SecretKind::Password, "pw")).unwrap();
    let opts = ConnectOptions { proxy: ProxyConfig { mode: ProxyMode::Direct, ..Default::default() }, ..Default::default() };
    let sess = connect(&db, &s.id, Arc::new(Yes), opts, CancellationToken::new()).await.expect("connects");
    (sess, dir)
}

async fn read_until(io: &mut SourceIo, needle: &str) -> Option<Option<i32>> {
    let mut text = String::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(5), io.rx.recv()).await {
            Ok(Some(SourceEvent::Data(b))) => {
                text.push_str(&String::from_utf8_lossy(&b));
                if text.contains(needle) {
                    return None;
                }
            }
            Ok(Some(SourceEvent::Closed(c))) => return Some(c),
            _ => panic!("timed out waiting for {needle:?}; got {text:?}"),
        }
    }
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..60 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("never happened: {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_echoes_input() {
    let ssh = TestSsh::start(shell_server(true)).await;
    let (sess, dir) = session(&ssh).await;
    let mut io = open_shell(sess, 120, 40).await.expect("shell opens");
    io.control.write(b"halo\n").unwrap();
    assert_eq!(read_until(&mut io, "halo").await, None, "the server echoed our input back");
    io.control.close();
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pty_is_requested_with_the_initial_size() {
    let ssh = TestSsh::start(shell_server(true)).await;
    let (sess, dir) = session(&ssh).await;
    let io = open_shell(sess, 120, 40).await.unwrap();
    assert_eq!(ssh.pty_size(), Some((120, 40)));
    io.control.close();
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resize_sends_window_change() {
    let ssh = TestSsh::start(shell_server(true)).await;
    let (sess, dir) = session(&ssh).await;
    let io = open_shell(sess, 120, 40).await.unwrap();
    io.control.resize(80, 24).unwrap();
    eventually("the server saw 80x24", || ssh.pty_size() == Some((80, 24))).await;
    assert_eq!(ssh.resize_count(), 1);
    io.control.resize(0, 0).unwrap(); // clamped, never an error
    eventually("0x0 became 1x1", || ssh.pty_size() == Some((1, 1))).await;
    io.control.close();
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_close_ends_the_stream() {
    let ssh = TestSsh::start(shell_server(true)).await;
    let (sess, dir) = session(&ssh).await;
    let mut io = open_shell(sess, 80, 24).await.unwrap();
    ssh.close_shells().await;
    assert!(read_until(&mut io, "\u{0}never").await.is_some(), "the stream ends with Closed when the server closes the channel");
    assert!(io.control.write(b"x").is_err(), "writing to a closed shell is an error, not a hang");
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exit_status_is_reported() {
    let ssh = TestSsh::start(shell_server(true)).await;
    let (sess, dir) = session(&ssh).await;
    let mut io = open_shell(sess, 80, 24).await.unwrap();
    io.control.write(b"exit\n").unwrap();
    assert_eq!(read_until(&mut io, "\u{0}never").await, Some(Some(0)));
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_source_closes_the_channel() {
    let ssh = TestSsh::start(shell_server(true)).await;
    let (sess, dir) = session(&ssh).await;
    let io = open_shell(sess, 80, 24).await.unwrap();
    assert_eq!(ssh.open_shells(), 1);
    io.control.close();
    eventually("the server saw the channel close", || ssh.open_shells() == 0).await;
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_shell_server_gives_a_clear_error() {
    let ssh = TestSsh::start(shell_server(false)).await;
    let (sess, dir) = session(&ssh).await;
    let started = std::time::Instant::now();
    let e = open_shell(sess, 80, 24).await.err().expect("a server that refuses PTYs is an error");
    assert!(started.elapsed() < Duration::from_secs(5), "must not hang");
    assert!(e.message.contains("shell"), "the message is in Indonesian and says what failed: {}", e.message);
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_stats_are_read_over_the_shell_connection() {
    use unduhkilat_core::server::shell::open_shell_with_stats;
    use unduhkilat_core::server::stats::StatsState;
    let root = std::env::temp_dir().join(format!("uk-stats-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let ssh = TestSsh::start_with_sftp(shell_server(true), root.clone()).await;
    ssh.enable_tar();
    let (sess, dir) = session(&ssh).await;
    let (io, stats) = open_shell_with_stats(sess, 80, 24, true).await.unwrap();
    let mut rx = stats.expect("asked for stats");
    let got = tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|s| matches!(s, StatsState::Ready(_)))).await.expect("stats arrive").unwrap().clone();
    let StatsState::Ready(s) = got else { unreachable!() };
    assert_eq!((s.mem_total_kb, s.mem_used_kb, s.procs_total, s.user.as_str(), s.disk_root_pct), (2_000_000, 1_500_000, 99, "eko", Some(41)));
    assert_eq!(s.conns, vec![(22, 1)]);
    io.control.close();
    eventually("the shell channel closed", || ssh.open_shells() == 0).await;
    std::fs::remove_dir_all(dir).ok();
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_without_exec_hides_the_stats() {
    use unduhkilat_core::server::shell::open_shell_with_stats;
    use unduhkilat_core::server::stats::StatsState;
    let ssh = TestSsh::start(shell_server(true)).await;
    let (sess, dir) = session(&ssh).await;
    let (io, stats) = open_shell_with_stats(sess, 80, 24, true).await.unwrap();
    let mut rx = stats.unwrap();
    tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|s| *s == StatsState::Unsupported)).await.expect("gives up").unwrap();
    io.control.write(b"halo\n").unwrap();
    let mut io = io;
    assert_eq!(read_until(&mut io, "halo").await, None, "the shell still works");
    io.control.close();
    std::fs::remove_dir_all(dir).ok();
}
