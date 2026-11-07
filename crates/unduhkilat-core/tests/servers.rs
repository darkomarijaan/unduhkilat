//! Server (SSH) integration tests. Every SSH server runs in-process on loopback:
//! nothing here touches the internet.

mod common;

use common::ssh_server::{TestCred, TestSsh, TestSshConfig};
use russh::client;
use russh::keys::PublicKeyOrCertificate;
use std::sync::Arc;
use tokio::io::AsyncReadExt;

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
    tokio::time::timeout(std::time::Duration::from_secs(5), stream.read_exact(&mut banner)).await.expect("banner in time").unwrap();
    assert_eq!(&banner, b"SSH-2.0-");
    assert_eq!(first.forward_count(), 1);
    assert_eq!(second.login_count(), 0, "reading the banner is not a login");
}
