//! Downloads from and uploads to saved servers through the normal queue (loopback servers only).

mod common;
#[path = "common/ssh_server.rs"]
mod ssh_server;

use common::Env;
use ssh_server::{TestCred, TestSsh, TestSshConfig};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use unduhkilat_core::error::ErrorCode;
use unduhkilat_core::models::DownloadState;
use unduhkilat_core::server::hostkeys;
use unduhkilat_core::server::model::{AuthMethod, NewServer, Protocol};
use unduhkilat_core::server::secrets::{self, SecretKind};
use unduhkilat_core::server::store;

const T: Duration = Duration::from_secs(30);

struct Rig {
    env: Env,
    ssh: TestSsh,
    /// The server's folder (what the SFTP server serves as `/`).
    remote: PathBuf,
    server_id: String,
    local: PathBuf,
}

async fn rig(trusted: bool) -> Rig {
    let env = Env::new().await;
    let remote = env.dir.join("remote");
    let local = env.dir.join("local");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::create_dir_all(&local).unwrap();
    let ssh = TestSsh::start_with_sftp(TestSshConfig { users: vec![("eko".into(), TestCred::Password("pw".into()))], allow_forwarding: false, accept_delay: None, shell: false }, remote.clone()).await;
    let n = NewServer { name: "nas".into(), protocol: Protocol::Sftp, host: "127.0.0.1".into(), port: Some(ssh.addr.port()), username: "eko".into(), auth: AuthMethod::Password, key_path: None, save_secret: true, hops: vec![] };
    let db = env.manager.db().clone();
    let s = db.write_blocking(move |c| store::insert_server(c, &n)).unwrap();
    let (id, port, fp) = (s.id.clone(), ssh.addr.port(), ssh.host_key_fingerprint());
    db.write_blocking(move |c| {
        secrets::store_secret(c, &id, SecretKind::Password, "pw")?;
        if trusted {
            hostkeys::trust(c, "127.0.0.1", port, "ssh-ed25519", &fp)?;
        }
        Ok(())
    })
    .unwrap();
    Rig { env, ssh, remote, server_id: s.id, local }
}

fn data(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

async fn wait_bytes(r: &Rig, id: &str, at_least: i64) {
    let deadline = Instant::now() + T;
    loop {
        let d = r.env.manager.get(id).unwrap();
        if d.downloaded_bytes >= at_least {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {at_least} bytes; at {}", d.downloaded_bytes);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_file_completes_with_correct_bytes() {
    let r = rig(true).await;
    let bytes = data(1_200_000);
    std::fs::write(r.remote.join("big.bin"), &bytes).unwrap();
    let dl = r.env.manager.enqueue_remote_download(&r.server_id, "/big.bin", &r.local, false).await.unwrap();
    let done = r.env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_eq!(std::fs::read(r.local.join("big.bin")).unwrap(), bytes);
    assert_eq!((done.total_bytes, done.downloaded_bytes), (1_200_000, 1_200_000));
    r.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_resumes_after_pause() {
    let r = rig(true).await;
    let bytes = data(1_500_000);
    std::fs::write(r.remote.join("slow.bin"), &bytes).unwrap();
    let dl = r.env.manager.enqueue_remote_download(&r.server_id, "/slow.bin", &r.local, false).await.unwrap();
    r.env.manager.set_download_speed_limit(&dl.id, 400).await.unwrap();
    wait_bytes(&r, &dl.id, 150_000).await;
    r.env.manager.pause(&dl.id).await.unwrap();
    let paused = r.env.manager.wait_for_state(&dl.id, DownloadState::Paused, T).await.unwrap();
    assert!(paused.downloaded_bytes > 0 && paused.downloaded_bytes < 1_500_000);
    r.env.manager.set_download_speed_limit(&dl.id, 0).await.unwrap();
    r.env.manager.resume(&dl.id).await.unwrap();
    r.env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_eq!(std::fs::read(r.local.join("slow.bin")).unwrap(), bytes, "no gap, no duplicate bytes");
    r.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upload_resumes_after_pause_without_corruption() {
    let r = rig(true).await;
    let bytes = data(1_500_000);
    let src = r.local.join("up.bin");
    std::fs::write(&src, &bytes).unwrap();
    let dl = r.env.manager.enqueue_remote_upload(&r.server_id, &src, "/", false).await.unwrap();
    r.env.manager.set_download_speed_limit(&dl.id, 400).await.unwrap();
    wait_bytes(&r, &dl.id, 150_000).await;
    r.env.manager.pause(&dl.id).await.unwrap();
    r.env.manager.wait_for_state(&dl.id, DownloadState::Paused, T).await.unwrap();
    r.env.manager.set_download_speed_limit(&dl.id, 0).await.unwrap();
    r.env.manager.resume(&dl.id).await.unwrap();
    r.env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_eq!(std::fs::read(r.remote.join("up.bin")).unwrap(), bytes);
    assert_eq!(std::fs::read(&src).unwrap(), bytes, "the local source is untouched");
    r.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upload_collision_stops_with_err_name_collision() {
    let r = rig(true).await;
    std::fs::write(r.remote.join("same.txt"), b"server copy").unwrap();
    let src = r.local.join("same.txt");
    std::fs::write(&src, b"local copy").unwrap();
    let dl = r.env.manager.enqueue_remote_upload(&r.server_id, &src, "/", false).await.unwrap();
    let failed = r.env.manager.wait_for_state(&dl.id, DownloadState::Failed, T).await.unwrap();
    assert_eq!(failed.error_code.as_deref(), Some("ERR_NAME_COLLISION"));
    assert_eq!(std::fs::read(r.remote.join("same.txt")).unwrap(), b"server copy", "never overwritten silently");
    // With the user's explicit confirmation the same upload replaces it.
    let dl2 = r.env.manager.enqueue_remote_upload(&r.server_id, &src, "/", true).await.unwrap();
    r.env.manager.wait_for_state(&dl2.id, DownloadState::Completed, T).await.unwrap();
    assert_eq!(std::fs::read(r.remote.join("same.txt")).unwrap(), b"local copy");
    r.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_name_is_sanitised_inside_the_target_folder() {
    let r = rig(true).await;
    std::fs::write(r.remote.join("con.txt"), b"x").unwrap();
    let dl = r.env.manager.enqueue_remote_download(&r.server_id, "/con.txt", &r.local, false).await.unwrap();
    r.env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert!(r.local.join("_con.txt").is_file(), "a Windows device name is made safe");
    for bad in ["/..", "/a/..", "/", ""] {
        let e = r.env.manager.enqueue_remote_download(&r.server_id, bad, &r.local, false).await.err().unwrap_or_else(|| panic!("{bad:?} must be refused"));
        assert_eq!(e.code, ErrorCode::ErrInvalidPath, "{bad:?}");
    }
    assert_eq!(std::fs::read_dir(&r.local).unwrap().count(), 1, "nothing was created outside or inside for the refused names");
    r.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn global_speed_limit_applies_to_uploads() {
    let r = rig(true).await;
    r.env.manager.set_global_speed_limit(100).await.unwrap();
    let src = r.local.join("limited.bin");
    std::fs::write(&src, data(300_000)).unwrap();
    let t0 = Instant::now();
    let dl = r.env.manager.enqueue_remote_upload(&r.server_id, &src, "/", false).await.unwrap();
    r.env.manager.wait_for_state(&dl.id, DownloadState::Completed, Duration::from_secs(60)).await.unwrap();
    assert!(t0.elapsed() > Duration::from_millis(1500), "300 KB at 100 KB/s cannot finish in {:?}", t0.elapsed());
    r.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn zero_byte_files_transfer_both_ways() {
    let r = rig(true).await;
    std::fs::write(r.remote.join("empty.bin"), b"").unwrap();
    let d = r.env.manager.enqueue_remote_download(&r.server_id, "/empty.bin", &r.local, false).await.unwrap();
    r.env.manager.wait_for_state(&d.id, DownloadState::Completed, T).await.unwrap();
    assert_eq!(std::fs::metadata(r.local.join("empty.bin")).unwrap().len(), 0);
    let src = r.local.join("zero-up.bin");
    std::fs::write(&src, b"").unwrap();
    let u = r.env.manager.enqueue_remote_upload(&r.server_id, &src, "/", false).await.unwrap();
    r.env.manager.wait_for_state(&u.id, DownloadState::Completed, T).await.unwrap();
    assert_eq!(std::fs::metadata(r.remote.join("zero-up.bin")).unwrap().len(), 0);
    r.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_an_upload_never_deletes_the_local_file() {
    let r = rig(true).await;
    let src = r.local.join("keep.bin");
    std::fs::write(&src, data(800_000)).unwrap();
    let dl = r.env.manager.enqueue_remote_upload(&r.server_id, &src, "/", false).await.unwrap();
    r.env.manager.set_download_speed_limit(&dl.id, 100).await.unwrap();
    wait_bytes(&r, &dl.id, 50_000).await;
    r.env.manager.delete(&dl.id, true).await.unwrap();
    assert!(src.is_file(), "\"delete the file\" on an upload must not delete the user's own source file");
    r.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unconfirmed_host_key_fails_with_a_hint_instead_of_prompting() {
    let r = rig(false).await;
    std::fs::write(r.remote.join("a.txt"), b"x").unwrap();
    let dl = r.env.manager.enqueue_remote_download(&r.server_id, "/a.txt", &r.local, false).await.unwrap();
    let failed = r.env.manager.wait_for_state(&dl.id, DownloadState::Failed, T).await.unwrap();
    assert_eq!(failed.error_code.as_deref(), Some("ERR_HOST_KEY_UNKNOWN"));
    assert!(failed.error_message.unwrap_or_default().contains("Uji sambungan"), "the message says what to do");
    r.ssh.stop();
}
