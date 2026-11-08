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
    side_with(env, name, None).await
}

async fn side_with(env: &Env, name: &str, accept_delay: Option<Duration>) -> Side {
    let folder = env.dir.join(name);
    std::fs::create_dir_all(&folder).unwrap();
    let ssh = TestSsh::start_with_sftp(TestSshConfig { users: vec![("eko".into(), TestCred::Password("pw".into()))], allow_forwarding: false, accept_delay, shell: false }, folder.clone()).await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_destination_folders_are_created_for_copies_and_uploads() {
    let r = rig().await;
    std::fs::write(r.a.folder.join("f.txt"), b"hello").unwrap();
    let dl = r.env.manager.enqueue_remote_copy(&r.a.id, "/f.txt", &r.b.id, "/new/deeper/still", false).await.unwrap();
    r.env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_eq!(std::fs::read(r.b.folder.join("new/deeper/still/f.txt")).unwrap(), b"hello");

    let local = r.env.dir.join("up.txt");
    std::fs::write(&local, b"from here").unwrap();
    let dl = r.env.manager.enqueue_remote_upload(&r.b.id, &local, "/u1/u2", false).await.unwrap();
    r.env.manager.wait_for_state(&dl.id, DownloadState::Completed, T).await.unwrap();
    assert_eq!(std::fs::read(r.b.folder.join("u1/u2/up.txt")).unwrap(), b"from here");

    // A path component that is a file blocks the transfer instead of overwriting it.
    std::fs::write(r.b.folder.join("blocker"), b"i am a file").unwrap();
    let dl = r.env.manager.enqueue_remote_copy(&r.a.id, "/f.txt", &r.b.id, "/blocker/x", false).await.unwrap();
    r.env.manager.wait_for_state(&dl.id, DownloadState::Failed, T).await.unwrap();
    assert_eq!(std::fs::read(r.b.folder.join("blocker")).unwrap(), b"i am a file");
    r.a.ssh.stop();
    r.b.ssh.stop();
}

struct Nobody;

impl unduhkilat_core::server::ssh::Interact for Nobody {
    fn confirm_host_key<'a>(&'a self, _h: &'a str, _p: u16, _t: &'a str, _f: &'a str) -> futures_util::future::BoxFuture<'a, bool> {
        Box::pin(async { false })
    }
    fn secret<'a>(&'a self, _n: &'a str, _k: SecretKind) -> futures_util::future::BoxFuture<'a, Option<unduhkilat_core::server::secrets::SecretString>> {
        Box::pin(async { None })
    }
}

async fn open(r: &Rig, id: &str) -> Box<dyn unduhkilat_core::server::fs::RemoteFs> {
    r.env.manager.servers().open_fs(id, std::sync::Arc::new(Nobody), tokio_util::sync::CancellationToken::new()).await.unwrap()
}

fn list_all(r: &Rig) -> Vec<unduhkilat_core::models::Download> {
    r.env.manager.db().read(|c| unduhkilat_core::repo::list_downloads(c, None)).unwrap()
}

fn make_tree(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("sub/deep")).unwrap();
    std::fs::write(root.join("a.txt"), b"alpha").unwrap();
    std::fs::write(root.join("sub/b.txt"), b"bravo").unwrap();
    std::fs::write(root.join("sub/deep/c.bin"), data(70_000)).unwrap();
}

fn assert_tree(root: &std::path::Path) {
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(std::fs::read(root.join("sub/b.txt")).unwrap(), b"bravo");
    assert_eq!(std::fs::read(root.join("sub/deep/c.bin")).unwrap(), data(70_000));
}

async fn all_done(r: &Rig, expected: usize) {
    let deadline = Instant::now() + T;
    loop {
        let all = list_all(&r);
        let done = all.iter().filter(|d| d.state == DownloadState::Completed).count();
        assert!(all.iter().all(|d| d.state != DownloadState::Failed), "a transfer failed: {:?}", all.iter().map(|d| (&d.file_name, &d.error_message)).collect::<Vec<_>>());
        if done >= expected {
            return;
        }
        assert!(Instant::now() < deadline, "timed out: {done} of {expected} done");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_whole_folder_is_copied_between_servers_keeping_its_structure() {
    let r = rig().await;
    make_tree(&r.a.folder.join("Proyek"));
    std::fs::create_dir_all(r.b.folder.join("Masuk")).unwrap();
    let mut src = open(&r, &r.a.id).await;
    let res = r.env.manager.enqueue_folder_copy(src.as_mut(), &r.a.id, "/Proyek", &r.b.id, "/Masuk", false).await.unwrap();
    assert_eq!((res.queued, res.failed.len()), (3, 0));
    all_done(&r, 3).await;
    assert_tree(&r.b.folder.join("Masuk/Proyek"));
    r.a.ssh.stop();
    r.b.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_whole_folder_is_downloaded_and_uploaded_keeping_its_structure() {
    let r = rig().await;
    make_tree(&r.a.folder.join("Proyek"));
    let local = r.env.dir.join("unduhan");
    std::fs::create_dir_all(&local).unwrap();
    let mut fs = open(&r, &r.a.id).await;
    let res = r.env.manager.enqueue_folder_download(fs.as_mut(), &r.a.id, "/Proyek", &local, false).await.unwrap();
    assert_eq!((res.queued, res.failed.len()), (3, 0));
    all_done(&r, 3).await;
    assert_tree(&local.join("Proyek"));

    let res = r.env.manager.enqueue_folder_upload(&r.b.id, &local.join("Proyek"), "/Naik", false).await.unwrap();
    assert_eq!((res.queued, res.failed.len()), (3, 0));
    all_done(&r, 6).await;
    assert_tree(&r.b.folder.join("Naik/Proyek"));
    // The local folder is untouched by the upload.
    assert_tree(&local.join("Proyek"));
    r.a.ssh.stop();
    r.b.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn existing_files_inside_a_folder_are_reported_not_overwritten_and_self_nesting_is_refused() {
    let r = rig().await;
    make_tree(&r.a.folder.join("Proyek"));
    std::fs::create_dir_all(r.b.folder.join("Proyek")).unwrap();
    std::fs::write(r.b.folder.join("Proyek/a.txt"), b"keep").unwrap();
    let mut src = open(&r, &r.a.id).await;
    let res = r.env.manager.enqueue_folder_copy(src.as_mut(), &r.a.id, "/Proyek", &r.b.id, "/", false).await.unwrap();
    assert_eq!(res.queued, 3, "the collision shows up as a failed item, not as a silent overwrite");
    let deadline = Instant::now() + T;
    while list_all(&r).iter().filter(|d| matches!(d.state, DownloadState::Completed | DownloadState::Failed)).count() < 3 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert_eq!(std::fs::read(r.b.folder.join("Proyek/a.txt")).unwrap(), b"keep");
    assert_eq!(std::fs::read(r.b.folder.join("Proyek/sub/b.txt")).unwrap(), b"bravo");

    let e = r.env.manager.enqueue_folder_copy(src.as_mut(), &r.a.id, "/Proyek", &r.a.id, "/Proyek/sub", false).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ErrInvalidArgument, "a folder cannot be copied into itself");
    r.a.ssh.stop();
    r.b.ssh.stop();
}

fn make_many(root: &std::path::Path, n: usize) {
    std::fs::create_dir_all(root).unwrap();
    for i in 0..n {
        std::fs::write(root.join(format!("f{i:03}.txt")), format!("isi berkas {i}")).unwrap();
    }
}

fn assert_many(root: &std::path::Path, n: usize) {
    for i in 0..n {
        assert_eq!(std::fs::read_to_string(root.join(format!("f{i:03}.txt"))).unwrap(), format!("isi berkas {i}"));
    }
}

/// Thousands of small files must not mean thousands of SSH logins: connections are reused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_small_files_reuse_connections_instead_of_logging_in_per_file() {
    let r = rig().await;
    make_many(&r.a.folder.join("Banyak"), 30);
    let mut src = open(&r, &r.a.id).await;
    let (a0, b0) = (r.a.ssh.login_count(), r.b.ssh.login_count());
    let res = r.env.manager.enqueue_folder_copy(src.as_mut(), &r.a.id, "/Banyak", &r.b.id, "/", false).await.unwrap();
    src.close().await;
    assert_eq!(res.queued, 30);
    all_done(&r, 30).await;
    assert_many(&r.b.folder.join("Banyak"), 30);
    let cap = unduhkilat_core::server::pool::PER_SERVER;
    let (a, b) = (r.a.ssh.login_count() - a0, r.b.ssh.login_count() - b0);
    assert!(a <= cap && b <= cap, "logins: source {a}, destination {b}; at most {cap} each expected");
    r.a.ssh.stop();
    r.b.ssh.stop();
}

/// Copying many files within ONE server needs two connections per file from the same server; this
/// must neither deadlock nor exceed the per-server limit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_files_copied_within_one_server_finish_without_deadlock() {
    let r = rig().await;
    make_many(&r.a.folder.join("Banyak"), 20);
    std::fs::create_dir_all(r.a.folder.join("Salinan")).unwrap();
    let mut src = open(&r, &r.a.id).await;
    let a0 = r.a.ssh.login_count();
    let res = r.env.manager.enqueue_folder_copy(src.as_mut(), &r.a.id, "/Banyak", &r.a.id, "/Salinan", false).await.unwrap();
    src.close().await;
    assert_eq!(res.queued, 20);
    all_done(&r, 20).await;
    assert_many(&r.a.folder.join("Salinan/Banyak"), 20);
    let cap = unduhkilat_core::server::pool::PER_SERVER;
    assert!(r.a.ssh.login_count() - a0 <= cap, "logins: {}", r.a.ssh.login_count() - a0);
    r.a.ssh.stop();
    r.b.ssh.stop();
}

/// A kept connection the server dropped in the meantime is replaced, not reported as a failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kept_connection_that_died_is_replaced_transparently() {
    let r = rig().await;
    std::fs::write(r.a.folder.join("satu.txt"), b"1").unwrap();
    std::fs::write(r.a.folder.join("dua.txt"), b"2").unwrap();
    let d1 = r.env.manager.enqueue_remote_copy(&r.a.id, "/satu.txt", &r.b.id, "/", false).await.unwrap();
    r.env.manager.wait_for_state(&d1.id, DownloadState::Completed, T).await.unwrap();
    r.a.ssh.kill_connections().await;
    r.b.ssh.kill_connections().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let d2 = r.env.manager.enqueue_remote_copy(&r.a.id, "/dua.txt", &r.b.id, "/", false).await.unwrap();
    let done = r.env.manager.wait_for_state(&d2.id, DownloadState::Completed, T).await;
    assert!(done.is_ok(), "second copy: {:?}", r.env.manager.get(&d2.id).unwrap().error_message);
    assert_eq!(std::fs::read(r.b.folder.join("dua.txt")).unwrap(), b"2");
    r.a.ssh.stop();
    r.b.ssh.stop();
}

/// Server transfers have their own parallel lanes: the download limit (here 1) does not make a
/// folder of thousands of files crawl through one file at a time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_transfers_run_in_parallel_even_when_the_download_limit_is_one() {
    let r = rig().await;
    r.env.manager.set_concurrency_limit(1).await.unwrap();
    r.env.manager.set_global_speed_limit(1500).await.unwrap();
    let root = r.a.folder.join("Paralel");
    std::fs::create_dir_all(&root).unwrap();
    for i in 0..8 {
        std::fs::write(root.join(format!("p{i}.bin")), data(400_000)).unwrap();
    }
    let mut src = open(&r, &r.a.id).await;
    let res = r.env.manager.enqueue_folder_copy(src.as_mut(), &r.a.id, "/Paralel", &r.b.id, "/", false).await.unwrap();
    src.close().await;
    assert_eq!(res.queued, 8);
    let mut most = 0;
    let deadline = Instant::now() + T;
    while list_all(&r).iter().filter(|d| d.state == DownloadState::Completed).count() < 8 {
        most = most.max(r.env.manager.running_count());
        assert!(Instant::now() < deadline, "timed out");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(most > 1, "at most {most} server transfer ran at once");
    r.env.manager.set_global_speed_limit(0).await.unwrap();
    r.a.ssh.stop();
    r.b.ssh.stop();
}

/// Not a pass/fail test: how long 200 small files take when every login costs 300 ms, as it does
/// against a real server. Run with `--ignored --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn bench_many_small_files_with_slow_logins() {
    let env = Env::new().await;
    let a = side_with(&env, "server-a", Some(Duration::from_millis(300))).await;
    let b = side_with(&env, "server-b", Some(Duration::from_millis(300))).await;
    let r = Rig { env, a, b };
    make_many(&r.a.folder.join("Banyak"), 200);
    let mut src = open(&r, &r.a.id).await;
    let started = Instant::now();
    r.env.manager.enqueue_folder_copy(src.as_mut(), &r.a.id, "/Banyak", &r.b.id, "/", false).await.unwrap();
    src.close().await;
    all_done(&r, 200).await;
    let took = started.elapsed();
    let logins = r.a.ssh.login_count() + r.b.ssh.login_count();
    println!("200 berkas: {took:.1?}, {logins} login (tanpa pool: 400 login, sekitar 200 x 0.6 s / 3 jalur = 40 s)");
    assert_many(&r.b.folder.join("Banyak"), 200);
}

// ---- fast folder copy: one tar stream (folder_stream) ------------------------------------------

fn make_big_tree(root: &std::path::Path) {
    make_many(root, 60);
    std::fs::create_dir_all(root.join("sub/deep")).unwrap();
    std::fs::write(root.join("sub/deep/c.bin"), data(70_000)).unwrap();
    std::fs::write(root.join("sub/it's ok.txt"), b"kutip").unwrap();
}

fn assert_big_tree(root: &std::path::Path) {
    assert_many(root, 60);
    assert_eq!(std::fs::read(root.join("sub/deep/c.bin")).unwrap(), data(70_000));
    assert_eq!(std::fs::read(root.join("sub/it's ok.txt")).unwrap(), b"kutip");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_folder_of_many_small_files_is_copied_as_one_stream() {
    let r = rig().await;
    r.a.ssh.enable_tar();
    r.b.ssh.enable_tar();
    make_big_tree(&r.a.folder.join("Banyak"));
    std::fs::create_dir_all(r.b.folder.join("Masuk")).unwrap();
    let mut src = open(&r, &r.a.id).await;
    let res = r.env.manager.enqueue_folder_copy(src.as_mut(), &r.a.id, "/Banyak", &r.b.id, "/Masuk", false).await.unwrap();
    src.close().await;
    assert_eq!((res.queued, res.streamed_files, res.failed.len()), (1, 62, 0), "one queue item for the whole folder");
    let done = r.env.manager.wait_for_state(&res.ids[0], DownloadState::Completed, T).await;
    let d = r.env.manager.get(&res.ids[0]).unwrap();
    assert!(done.is_ok(), "stream copy: {:?}", d.error_message);
    assert_big_tree(&r.b.folder.join("Masuk/Banyak"));
    assert_eq!(d.downloaded_bytes, d.total_bytes, "progress ends at 100%");
    r.a.ssh.stop();
    r.b.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stream_copy_within_one_server_works() {
    let r = rig().await;
    r.a.ssh.enable_tar();
    make_big_tree(&r.a.folder.join("Banyak"));
    std::fs::create_dir_all(r.a.folder.join("Salinan")).unwrap();
    let mut src = open(&r, &r.a.id).await;
    let res = r.env.manager.enqueue_folder_copy(src.as_mut(), &r.a.id, "/Banyak", &r.a.id, "/Salinan", false).await.unwrap();
    src.close().await;
    assert_eq!(res.streamed_files, 62);
    r.env.manager.wait_for_state(&res.ids[0], DownloadState::Completed, T).await.unwrap();
    assert_big_tree(&r.a.folder.join("Salinan/Banyak"));
    r.a.ssh.stop();
    r.b.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_tar_on_a_server_the_folder_is_copied_file_by_file() {
    let r = rig().await;
    r.a.ssh.enable_tar(); // the destination has no shell access
    make_big_tree(&r.a.folder.join("Banyak"));
    let mut src = open(&r, &r.a.id).await;
    let res = r.env.manager.enqueue_folder_copy(src.as_mut(), &r.a.id, "/Banyak", &r.b.id, "/", false).await.unwrap();
    src.close().await;
    assert_eq!((res.queued, res.streamed_files), (62, 0));
    all_done(&r, 62).await;
    assert_big_tree(&r.b.folder.join("Banyak"));
    r.a.ssh.stop();
    r.b.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_existing_destination_folder_is_never_overwritten_by_a_stream() {
    let r = rig().await;
    r.a.ssh.enable_tar();
    r.b.ssh.enable_tar();
    make_big_tree(&r.a.folder.join("Banyak"));
    std::fs::create_dir_all(r.b.folder.join("Banyak")).unwrap();
    std::fs::write(r.b.folder.join("Banyak/f000.txt"), b"keep").unwrap();
    let mut src = open(&r, &r.a.id).await;
    let res = r.env.manager.enqueue_folder_copy(src.as_mut(), &r.a.id, "/Banyak", &r.b.id, "/", false).await.unwrap();
    src.close().await;
    assert_eq!(res.streamed_files, 0, "falls back to per-file copying, which reports each existing file");
    let deadline = Instant::now() + T;
    while list_all(&r).iter().filter(|d| matches!(d.state, DownloadState::Completed | DownloadState::Failed)).count() < 62 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert_eq!(std::fs::read(r.b.folder.join("Banyak/f000.txt")).unwrap(), b"keep");
    assert_eq!(std::fs::read_to_string(r.b.folder.join("Banyak/f001.txt")).unwrap(), "isi berkas 1");
    r.a.ssh.stop();
    r.b.ssh.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paused_stream_copy_starts_again_and_completes() {
    let r = rig().await;
    r.a.ssh.enable_tar();
    r.b.ssh.enable_tar();
    let root = r.a.folder.join("Banyak");
    make_many(&root, 60);
    std::fs::write(root.join("besar.bin"), data(1_500_000)).unwrap();
    r.env.manager.set_global_speed_limit(400).await.unwrap();
    let mut src = open(&r, &r.a.id).await;
    let res = r.env.manager.enqueue_folder_copy(src.as_mut(), &r.a.id, "/Banyak", &r.b.id, "/", false).await.unwrap();
    src.close().await;
    let id = res.ids[0].clone();
    wait_bytes(&r, &id, 200_000).await;
    r.env.manager.pause(&id).await.unwrap();
    r.env.manager.wait_for_state(&id, DownloadState::Paused, T).await.unwrap();
    r.env.manager.set_global_speed_limit(0).await.unwrap();
    r.env.manager.resume(&id).await.unwrap();
    let done = r.env.manager.wait_for_state(&id, DownloadState::Completed, T).await;
    assert!(done.is_ok(), "resumed stream: {:?}", r.env.manager.get(&id).unwrap().error_message);
    assert_many(&r.b.folder.join("Banyak"), 60);
    assert_eq!(std::fs::read(r.b.folder.join("Banyak/besar.bin")).unwrap(), data(1_500_000));
    r.a.ssh.stop();
    r.b.ssh.stop();
}
