mod common;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use common::ftp_proxy::{FtpServer, ProxyServer};
use common::{Env, Mode, TestServer};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use unduhkilat_core::engine::manager::NewDownload;
use unduhkilat_core::engine::proxy::{ProxyConfig, ProxyMode};
use unduhkilat_core::models::DownloadState;
use unduhkilat_core::repo;
use zeroize::Zeroizing;

const T: Duration = Duration::from_secs(30);

fn req(url: String, name: &str, env: &Env, conns: u32) -> NewDownload {
    NewDownload { url, file_name: Some(name.into()), target_path: Some(env.path(name)), connection_count: Some(conns), start_immediately: true, ..Default::default() }
}

fn assert_file_eq(env: &Env, name: &str, expected: &[u8]) {
    let got = std::fs::read(env.path(name)).unwrap();
    assert_eq!(got.len(), expected.len(), "length");
    assert!(got == expected, "content differs");
}

fn manual(addr: std::net::SocketAddr, auth: Option<(&str, &str)>) -> ProxyConfig {
    ProxyConfig {
        mode: ProxyMode::Manual,
        host: addr.ip().to_string(),
        port: addr.port(),
        username: auth.map(|a| a.0.to_string()),
        password: auth.map(|a| Zeroizing::new(a.1.to_string())),
    }
}

async fn failed_with(env: &Env, id: &str) -> String {
    let d = env.manager.wait_for_state(id, DownloadState::Failed, T).await.unwrap();
    d.error_code.unwrap_or_default()
}

// ============================ FTP ============================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftp_segmented_download_uses_rest_per_segment_and_is_byte_identical() {
    let ftp = FtpServer::start(1_500_003).await;
    let env = Env::new().await;
    let dl = env.manager.create(req(ftp.url(), "f.bin", &env, 6)).await.unwrap();
    let done = env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();

    assert_file_eq(&env, "f.bin", &ftp.state.data);
    assert!(done.segmentation_supported);
    assert_eq!(done.total_bytes, 1_500_003);
    let segs = env.manager.db().read(|c| repo::get_segments(c, &dl.id)).unwrap();
    assert_eq!(segs.len(), 6, "connection count honoured exactly (DEC-001)");
    let cmds = ftp.commands();
    for s in segs.iter().filter(|s| s.range_start > 0) {
        assert!(cmds.contains(&format!("REST {}", s.range_start)), "segment {} must resume at its start; saw {cmds:?}", s.segment_index);
    }
    assert_eq!(cmds.iter().filter(|c| c.starts_with("RETR")).count(), 6, "one RETR per connection");
    assert_eq!(ftp.state.logins.load(Ordering::SeqCst), 7, "1 probe + 6 workers");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftp_server_without_rest_falls_back_to_a_single_connection() {
    let ftp = FtpServer::start(400_000).await;
    ftp.state.rest.store(false, Ordering::SeqCst);
    let env = Env::new().await;
    let dl = env.manager.create(req(ftp.url(), "nr.bin", &env, 8)).await.unwrap();
    let done = env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert!(!done.segmentation_supported, "list must show segmentation was unavailable (FR-005)");
    assert_file_eq(&env, "nr.bin", &ftp.state.data);
    let cmds = ftp.commands();
    assert_eq!(cmds.iter().filter(|c| c.starts_with("RETR")).count(), 1);
    assert!(!cmds.iter().any(|c| c.starts_with("REST ") && c != "REST 0"), "never asks for a non-zero restart point: {cmds:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftp_without_size_streams_until_the_data_connection_closes() {
    let ftp = FtpServer::start(250_000).await;
    ftp.state.size.store(false, Ordering::SeqCst);
    let env = Env::new().await;
    let dl = env.manager.create(req(ftp.url(), "u.bin", &env, 4)).await.unwrap();
    let done = env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_eq!(done.total_bytes, 250_000);
    assert_file_eq(&env, "u.bin", &ftp.state.data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftp_pasv_fallback_ignores_the_address_the_server_advertises() {
    // The mock answers PASV with 10.255.255.1 (unroutable). Connecting there would hang;
    // connecting to the control host (the safe behaviour) succeeds.
    let ftp = FtpServer::start(120_000).await;
    ftp.state.epsv.store(false, Ordering::SeqCst);
    let env = Env::new().await;
    let dl = env.manager.create(req(ftp.url(), "p.bin", &env, 2)).await.unwrap();
    env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_file_eq(&env, "p.bin", &ftp.state.data);
    assert!(ftp.commands().iter().any(|c| c == "PASV"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftp_login_from_the_url_and_failures_are_reported_as_auth_errors() {
    let ftp = FtpServer::start(100_000).await;
    *ftp.state.require.lock().unwrap() = Some(("alice".into(), "s3cret".into()));
    let env = Env::new().await;

    let ok = env.manager.create(req(ftp.url_with_login("alice", "s3cret"), "a.bin", &env, 2)).await.unwrap();
    env.manager.wait_for_state(&ok.id, DownloadState::Completed, T).await.unwrap();
    assert_file_eq(&env, "a.bin", &ftp.state.data);
    assert!(!ftp.commands().iter().any(|c| c.contains("s3cret")), "the password is never echoed into a recorded command");

    let bad = env.manager.create(req(ftp.url_with_login("alice", "wrong"), "b.bin", &env, 2)).await.unwrap();
    assert_eq!(failed_with(&env, &bad.id).await, "ERR_HTTP_AUTH_REQUIRED");

    let anon = env.manager.create(req(ftp.url(), "c.bin", &env, 2)).await.unwrap();
    assert_eq!(failed_with(&env, &anon.id).await, "ERR_HTTP_AUTH_REQUIRED");
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftp_uses_the_credential_vault_when_the_url_has_none() {
    let ftp = FtpServer::start(80_000).await;
    *ftp.state.require.lock().unwrap() = Some(("alice".into(), "vault-pw".into()));
    let env = Env::new().await;
    env.manager.save_credential("127.0.0.1", Some(ftp.addr.port()), "alice", "vault-pw", "ftp").await.unwrap();

    // Stored encrypted, never plaintext (DEC-005).
    let raw: String = env.manager.db().read(|c| Ok(c.query_row("SELECT password_encrypted FROM credentials", [], |r| r.get(0))?)).unwrap();
    assert!(!raw.contains("vault-pw"));

    let dl = env.manager.create(req(ftp.url(), "v.bin", &env, 2)).await.unwrap();
    env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_file_eq(&env, "v.bin", &ftp.state.data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftp_pause_then_resume_issues_rest_at_each_persisted_offset() {
    let ftp = FtpServer::start(2_000_000).await;
    ftp.state.delay_ms.store(8, Ordering::SeqCst);
    let env = Env::new().await;
    let dl = env.manager.create(req(ftp.url(), "r.bin", &env, 3)).await.unwrap();

    let t = Instant::now();
    while env.manager.get(&dl.id).unwrap().downloaded_bytes < 300_000 {
        assert!(t.elapsed() < T, "no progress");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    env.manager.pause(&dl.id).await.unwrap();
    env.manager.wait_for_state(&dl.id, DownloadState::Paused, T).await.unwrap();
    let segs = env.manager.db().read(|c| repo::get_segments(c, &dl.id)).unwrap();
    assert!(segs.iter().any(|s| s.highest_contiguous_offset > s.range_start));

    ftp.clear();
    ftp.state.delay_ms.store(0, Ordering::SeqCst);
    env.manager.resume(&dl.id).await.unwrap();
    env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_file_eq(&env, "r.bin", &ftp.state.data);
    let cmds = ftp.commands();
    for s in segs.iter().filter(|s| s.highest_contiguous_offset <= s.range_end && s.highest_contiguous_offset > 0) {
        assert!(cmds.contains(&format!("REST {}", s.highest_contiguous_offset)), "missing REST {}; saw {cmds:?}", s.highest_contiguous_offset);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftp_missing_file_reports_not_found() {
    let ftp = FtpServer::start(1000).await;
    let env = Env::new().await;
    let url = format!("ftp://{}/pub/nope.bin", ftp.addr);
    let dl = env.manager.create(req(url, "n.bin", &env, 2)).await.unwrap();
    assert_eq!(failed_with(&env, &dl.id).await, "ERR_FILE_NOT_FOUND");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftp_url_cannot_inject_extra_commands() {
    let ftp = FtpServer::start(1000).await;
    let env = Env::new().await;
    let url = format!("ftp://{}/pub/file.bin%0D%0ADELE%20important", ftp.addr);
    let dl = env.manager.create(req(url, "i.bin", &env, 1)).await.unwrap();
    assert_eq!(failed_with(&env, &dl.id).await, "ERR_INVALID_ARGUMENT");
    assert!(ftp.commands().iter().all(|c| !c.starts_with("DELE")), "no injected command reached the server: {:?}", ftp.commands());
}

// ============================ Proxy ============================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_download_goes_through_a_manual_proxy_with_basic_credentials() {
    let origin = TestServer::start(400_000, Mode::Ranges).await;
    let proxy = ProxyServer::start().await;
    proxy.set_auth(&format!("Basic {}", STANDARD.encode("user:pw")));
    let env = Env::new().await;
    env.manager.set_proxy(manual(proxy.addr, Some(("user", "pw")))).await.unwrap();

    let dl = env.manager.create(req(origin.url("/file"), "px.bin", &env, 4)).await.unwrap();
    env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_file_eq(&env, "px.bin", &origin.state.data);

    let reqs = proxy.requests();
    assert!(reqs.len() >= 5, "probe + 4 segments all went through the proxy: {reqs:?}");
    assert!(reqs.iter().all(|r| r.starts_with("GET http://127.0.0.1:")), "absolute-form requests: {reqs:?}");
    let seen = proxy.state.auth_seen.lock().unwrap().clone();
    let want = format!("Basic {}", STANDARD.encode("user:pw"));
    assert!(seen.iter().all(|a| a.as_deref() == Some(want.as_str())), "credentials on every request: {seen:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_proxy_credentials_fail_with_an_auth_error() {
    let origin = TestServer::start(10_000, Mode::Ranges).await;
    let proxy = ProxyServer::start().await;
    proxy.set_auth(&format!("Basic {}", STANDARD.encode("user:pw")));
    let env = Env::new().await;
    env.manager.set_proxy(manual(proxy.addr, Some(("user", "WRONG")))).await.unwrap();
    let dl = env.manager.create(req(origin.url("/file"), "bad.bin", &env, 2)).await.unwrap();
    assert_eq!(failed_with(&env, &dl.id).await, "ERR_HTTP_AUTH_REQUIRED");
    assert_eq!(origin.log().len(), 0, "the origin was never reached without valid proxy credentials");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn switching_to_direct_takes_effect_for_the_next_download_without_a_restart() {
    let origin = TestServer::start(50_000, Mode::Ranges).await;
    let proxy = ProxyServer::start().await;
    let env = Env::new().await;

    env.manager.set_proxy(manual(proxy.addr, None)).await.unwrap();
    let a = env.manager.create(req(origin.url("/file"), "via.bin", &env, 2)).await.unwrap();
    env.manager.wait_for_state(&a.id, DownloadState::Completed, T).await.unwrap();
    let after_first = proxy.requests().len();
    assert!(after_first >= 3);

    env.manager.set_proxy(ProxyConfig { mode: ProxyMode::Direct, ..Default::default() }).await.unwrap();
    let b = env.manager.create(req(origin.url("/file"), "direct.bin", &env, 2)).await.unwrap();
    env.manager.wait_for_state(&b.id, DownloadState::Completed, T).await.unwrap();
    assert_eq!(proxy.requests().len(), after_first, "direct mode must not touch the proxy");
    assert_file_eq(&env, "direct.bin", &origin.state.data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftp_is_tunnelled_through_an_http_proxy_with_connect() {
    let ftp = FtpServer::start(300_000).await;
    let proxy = ProxyServer::start().await;
    proxy.set_auth(&format!("Basic {}", STANDARD.encode("user:pw")));
    let env = Env::new().await;
    env.manager.set_proxy(manual(proxy.addr, Some(("user", "pw")))).await.unwrap();

    let dl = env.manager.create(req(ftp.url(), "tun.bin", &env, 3)).await.unwrap();
    env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_file_eq(&env, "tun.bin", &ftp.state.data);

    let connects: Vec<String> = proxy.requests().into_iter().filter(|r| r.starts_with("CONNECT ")).collect();
    // Control connections: 1 probe + 3 workers. Data connections: 3 (the probe only runs SIZE/REST).
    // Every one of them must be tunnelled, none may bypass the proxy.
    assert_eq!(connects.len(), 7, "{connects:?}");
    assert_eq!(ftp.state.logins.load(Ordering::SeqCst), 4);
    assert!(connects.iter().any(|c| c.contains(&format!("127.0.0.1:{}", ftp.addr.port()))), "control connection via CONNECT");
    assert!(proxy.state.auth_seen.lock().unwrap().iter().all(|a| a.is_some()), "proxy credentials sent on every CONNECT");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftp_over_a_socks_proxy_is_refused_loudly_not_bypassed() {
    let ftp = FtpServer::start(1000).await;
    let env = Env::new().await;
    env.manager.set_proxy(ProxyConfig { mode: ProxyMode::Manual, host: "socks5://127.0.0.1".into(), port: 9, ..Default::default() }).await.unwrap();
    let dl = env.manager.create(req(ftp.url(), "s.bin", &env, 1)).await.unwrap();
    assert_eq!(failed_with(&env, &dl.id).await, "ERR_INVALID_ARGUMENT");
    assert!(ftp.commands().is_empty(), "the FTP server must not be contacted directly behind the user's back");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_proxy_settings_are_rejected_and_the_old_ones_stay() {
    let env = Env::new().await;
    let before = env.manager.proxy_config().mode;
    let err = env.manager.set_proxy(ProxyConfig { mode: ProxyMode::Manual, host: String::new(), port: 0, ..Default::default() }).await.unwrap_err();
    assert_eq!(err.code.as_str(), "ERR_INVALID_ARGUMENT");
    assert_eq!(env.manager.proxy_config().mode, before);
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proxy_settings_persist_across_restarts_with_the_password_encrypted() {
    let env = Env::new().await;
    let proxy = ProxyServer::start().await;
    env.manager.set_proxy(manual(proxy.addr, Some(("user", "pw-secret")))).await.unwrap();

    let db2 = unduhkilat_core::db::Db::open(&env.db_path).unwrap();
    let m2 = unduhkilat_core::engine::manager::Manager::start(db2, Default::default()).await.unwrap();
    let cfg = m2.proxy_config();
    assert_eq!(cfg.mode, ProxyMode::Manual);
    assert_eq!(cfg.port, proxy.addr.port());
    assert_eq!(cfg.username.as_deref(), Some("user"));
    assert_eq!(cfg.password.as_deref().map(String::as_str), Some("pw-secret"));
    let raw: String = m2.db().read(|c| Ok(c.query_row("SELECT value FROM settings WHERE key='proxy_password_encrypted'", [], |r| r.get(0))?)).unwrap();
    assert!(!raw.contains("pw-secret") && !raw.is_empty());
}
