//! Copying a file from one saved server to another through the queue, streaming through this
//! computer without touching its disk (loopback servers only).

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

struct Side {
    ssh: TestSsh,
    folder: PathBuf,
    id: String,
}

struct Rig {
    env: Env,
    a: Side,
    b: Side,
}

async fn side(env: &Env, name: &str) -> Side {
    let folder = env.dir.join(name);
    std::fs::create_dir_all(&folder).unwrap();
    let ssh = TestSsh::start_with_sftp(TestSshConfig { users: vec![("eko".into(), TestCred::Password("pw".into()))], allow_forwarding: false, accept_delay: None, shell: false }, folder.clone()).await;
    let n = NewServer { name: name.into(), protocol: Protocol::Sftp, host: "127.0.0.1".into(), port: Some(ssh.addr.port()), username: "eko".into(), auth: AuthMethod::Password, key_path: None, save_secret: true, hops: vec![] };
    let db = env.manager.db().clone();
    let s = db.write_blocking(move |c| store::insert_server(c, &n)).unwrap();
    let (id, port, fp) = (s.id.clone(), ssh.addr.port(), ssh.host_key_fingerprint());
    db.write_blocking(move |c| {
        secrets::store_secret(c, &id, SecretKind::Password, "pw")?;
        hostkeys::trust(c, "127.0.0.1", port, "ssh-ed25519", &fp)?;
        Ok(())
    })
    .unwrap();
    Side { ssh, folder, id: s.id }
}

async fn rig() -> Rig {
    let env = Env::new().await;
    let a = side(&env, "server-a").await;
    let b = side(&env, "server-b").await;
    Rig { env, a, b }
}

fn data(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

async fn wait_bytes(r: &Rig, id: &str, at_least: i64) {
    let deadline = Instant::now() + T;
    loop {
        if r.env.manager.get(id).unwrap().downloaded_bytes >= at_least {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {at_least} bytes");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_is_copied_from_one_server_to_another() {
    let r = rig().await;
    let bytes = data(1_300_000);
    std::fs::write(r.a.folder.join("big.bin"), &bytes).unwrap();
    std::fs::create_dir_all(r.b.folder.join("in")).unwrap();
    let dl = r.env.manager.enqueue_remote_copy(&r.a.id, "/big.bin", &r.b.id, "/in", false).await.unwrap();
    let done = r.env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_eq!(std::fs::read(r.b.folder.join("in").join("big.bin")).unwrap(), bytes);
    assert_eq!((done.total_bytes, done.downloaded_bytes), (1_300_000, 1_300_000));
    assert!(std::fs::read(r.a.folder.join("big.bin")).unwrap() == bytes, "a copy leaves the original alone");
    r.a.ssh.stop();
    r.b.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_existing_file_at_the_destination_is_kept_unless_confirmed() {
    let r = rig().await;
    std::fs::write(r.a.folder.join("x.txt"), b"new").unwrap();
    std::fs::write(r.b.folder.join("x.txt"), b"precious old data").unwrap();
    let dl = r.env.manager.enqueue_remote_copy(&r.a.id, "/x.txt", &r.b.id, "/", false).await.unwrap();
    let failed = r.env.manager.wait_for_state(&dl.id, DownloadState::Failed, T).await.unwrap();
    assert_eq!(failed.error_code.as_deref(), Some("ERR_NAME_COLLISION"));
    assert_eq!(std::fs::read(r.b.folder.join("x.txt")).unwrap(), b"precious old data");

    let dl = r.env.manager.enqueue_remote_copy(&r.a.id, "/x.txt", &r.b.id, "/", true).await.unwrap();
    r.env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_eq!(std::fs::read(r.b.folder.join("x.txt")).unwrap(), b"new");
    r.a.ssh.stop();
    r.b.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn copying_a_file_onto_itself_is_refused_before_anything_is_queued() {
    let r = rig().await;
    std::fs::write(r.a.folder.join("same.txt"), b"keep me").unwrap();
    for overwrite in [false, true] {
        let e = r.env.manager.enqueue_remote_copy(&r.a.id, "/same.txt", &r.a.id, "/", overwrite).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::ErrInvalidArgument);
    }
    assert_eq!(std::fs::read(r.a.folder.join("same.txt")).unwrap(), b"keep me");
    r.a.ssh.stop();
    r.b.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_copy_resumes_after_pause_and_restart_or_delete_never_touch_files() {
    let r = rig().await;
    let bytes = data(1_500_000);
    std::fs::write(r.a.folder.join("slow.bin"), &bytes).unwrap();
    let dl = r.env.manager.enqueue_remote_copy(&r.a.id, "/slow.bin", &r.b.id, "/", false).await.unwrap();
    r.env.manager.set_download_speed_limit(&dl.id, 400).await.unwrap();
    wait_bytes(&r, &dl.id, 150_000).await;
    r.env.manager.pause(&dl.id).await.unwrap();
    let paused = r.env.manager.wait_for_state(&dl.id, DownloadState::Paused, T).await.unwrap();
    assert!(paused.downloaded_bytes > 0 && paused.downloaded_bytes < 1_500_000);
    r.env.manager.set_download_speed_limit(&dl.id, 0).await.unwrap();
    r.env.manager.resume(&dl.id).await.unwrap();
    r.env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_eq!(std::fs::read(r.b.folder.join("slow.bin")).unwrap(), bytes);

    // Deleting the finished item, even with "delete file", must not delete anything on a server.
    r.env.manager.delete(&dl.id, true).await.unwrap();
    assert!(r.b.folder.join("slow.bin").exists() && r.a.folder.join("slow.bin").exists());
    r.a.ssh.stop();
    r.b.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_source_fails_cleanly() {
    let r = rig().await;
    let dl = r.env.manager.enqueue_remote_copy(&r.a.id, "/nope.bin", &r.b.id, "/", false).await.unwrap();
    let failed = r.env.manager.wait_for_state(&dl.id, DownloadState::Failed, T).await.unwrap();
    assert!(failed.error_code.is_some());
    assert!(!r.b.folder.join("nope.bin").exists());
    r.a.ssh.stop();
    r.b.ssh.stop();
}
