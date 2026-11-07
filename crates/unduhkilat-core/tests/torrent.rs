//! DEC-008 BitTorrent integration tests. A librqbit seeder runs in-process on loopback,
//! so nothing touches the internet (DHT is switched off through the settings).

mod common;

use common::{pattern, Env};
use librqbit::limits::LimitsConfig;
use librqbit::{AddTorrent, AddTorrentOptions, ConnectionOptions, CreateTorrentOptions, ListenerMode, ListenerOptions, Session, SessionOptions};
use std::net::{Ipv4Addr, SocketAddr};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use unduhkilat_core::config::keys;
use unduhkilat_core::engine::manager::NewDownload;
use unduhkilat_core::engine::proxy::{ProxyConfig, ProxyMode};
use unduhkilat_core::engine::torrent;
use unduhkilat_core::error::ErrorCode;
use unduhkilat_core::models::DownloadState;
use unduhkilat_core::repo;

struct Seeder {
    session: Arc<Session>,
    addr: SocketAddr,
    torrent_bytes: Vec<u8>,
    info_hash: String,
    _dir: PathBuf,
}

impl Drop for Seeder {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self._dir).ok();
    }
}

/// Seed one file called `name` of `size` bytes, optionally capping upload speed.
async fn seeder(name: &str, size: usize, upload_kbps: Option<u32>) -> Seeder {
    let dir = std::env::temp_dir().join(format!("uk-seed-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(name);
    std::fs::write(&file, pattern(size)).unwrap();
    let spawner = librqbit::spawn_utils::BlockingSpawner::new(2);
    let created = librqbit::create_torrent(&file, CreateTorrentOptions { name: Some(name), trackers: vec![], piece_length: Some(32 * 1024) }, &spawner).await.unwrap();
    let torrent_bytes = created.as_bytes().unwrap().to_vec();
    let session = Session::new_with_opts(
        dir.clone(),
        SessionOptions {
            dht: None,
            persistence: None,
            disable_local_service_discovery: true,
            ipv4_only: true,
            listen: Some(ListenerOptions { mode: ListenerMode::TcpOnly, listen_addr: (Ipv4Addr::LOCALHOST, 0).into(), ipv4_only: true, ..Default::default() }),
            connect: Some(ConnectionOptions::default()),
            ratelimits: LimitsConfig { upload_bps: upload_kbps.and_then(|k| NonZeroU32::new(k * 1024)), download_bps: None },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    session
        .add_torrent(
            AddTorrent::from_bytes(torrent_bytes.clone()),
            Some(AddTorrentOptions { output_folder: Some(dir.to_string_lossy().into_owned()), overwrite: true, ..Default::default() }),
        )
        .await
        .unwrap();
    let addr = session.listen_addr().expect("seeder listens");
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, addr.port()));
    Seeder { session, addr, torrent_bytes, info_hash: created.info_hash().as_string(), _dir: dir }
}

/// Seed a multi-file torrent: a folder `name` holding `a.txt`, `Setup/b.bin` and `Crack/c.bin`.
async fn seeder_pack(name: &str) -> Seeder {
    let base = std::env::temp_dir().join(format!("uk-pack-{}", uuid::Uuid::new_v4()));
    let root = base.join(name);
    std::fs::create_dir_all(root.join("Setup")).unwrap();
    std::fs::create_dir_all(root.join("Crack")).unwrap();
    std::fs::write(root.join("a.txt"), pattern(1_000)).unwrap();
    std::fs::write(root.join("Setup").join("b.bin"), pattern(120_000)).unwrap();
    std::fs::write(root.join("Crack").join("c.bin"), pattern(80_000)).unwrap();
    let spawner = librqbit::spawn_utils::BlockingSpawner::new(2);
    let created = librqbit::create_torrent(&root, CreateTorrentOptions { name: Some(name), trackers: vec![], piece_length: Some(32 * 1024) }, &spawner).await.unwrap();
    let torrent_bytes = created.as_bytes().unwrap().to_vec();
    let session = Session::new_with_opts(
        base.clone(),
        SessionOptions {
            dht: None,
            persistence: None,
            disable_local_service_discovery: true,
            ipv4_only: true,
            listen: Some(ListenerOptions { mode: ListenerMode::TcpOnly, listen_addr: (Ipv4Addr::LOCALHOST, 0).into(), ipv4_only: true, ..Default::default() }),
            connect: Some(ConnectionOptions::default()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // For a multi-file torrent librqbit writes straight into `output_folder`: the folder itself.
    session
        .add_torrent(AddTorrent::from_bytes(torrent_bytes.clone()), Some(AddTorrentOptions { output_folder: Some(root.to_string_lossy().into_owned()), overwrite: true, ..Default::default() }))
        .await
        .unwrap();
    let addr = session.listen_addr().expect("seeder listens");
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, addr.port()));
    Seeder { session, addr, torrent_bytes, info_hash: created.info_hash().as_string(), _dir: base }
}

async fn env() -> Env {
    let env = Env::new().await;
    env.manager
        .db()
        .write(|c| {
            repo::set_setting(c, keys::TORRENT_DHT, "0")?;
            repo::set_setting(c, keys::TORRENT_SEED_RATIO, "0") // seed until stopped
        })
        .await
        .unwrap();
    env
}

fn magnet(s: &Seeder, name: &str) -> String {
    format!("magnet:?xt=urn:btih:{}&dn={name}&x.pe={}", s.info_hash, s.addr)
}

fn out_file(env: &Env, name: &str) -> PathBuf {
    // Default category directory chosen by the manager for an unclassified name.
    let cats = env.manager.db().read(repo::list_categories).unwrap();
    let dir = cats.iter().find(|c| c.is_default).map(|c| PathBuf::from(&c.target_directory)).unwrap();
    dir.join(name)
}

async fn wait(env: &Env, id: &str, state: DownloadState, secs: u64) -> unduhkilat_core::models::Download {
    env.manager.wait_for_state(id, state, Duration::from_secs(secs)).await.unwrap_or_else(|e| {
        let d = env.manager.get(id).unwrap();
        panic!("{e}; state={:?} err={:?} {:?}", d.state, d.error_code, d.error_message)
    })
}

fn same(path: &Path, size: usize) -> bool {
    std::fs::read(path).map(|b| b == pattern(size)).unwrap_or(false)
}

// ---- parsing ---------------------------------------------------------------

#[test]
fn magnet_parsing() {
    let m = torrent::parse_magnet("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Some%20File&x.pe=1.2.3.4:5").unwrap();
    assert_eq!(m.info_hash, "0123456789abcdef0123456789abcdef01234567");
    assert_eq!(m.name.as_deref(), Some("Some File"));
    assert!(torrent::parse_magnet("magnet:?dn=nohash").is_err());
    assert!(torrent::parse_magnet("magnet:?xt=urn:btih:zzzz").is_err());
    assert!(torrent::is_magnet("MAGNET:?xt=x") && !torrent::is_magnet("http://x/magnet:"));
    let peers = torrent::peers_from_magnet("magnet:?xt=urn:btih:00&x.pe=1.2.3.4:5&x.pe=bad&x.pe=[::1]:9");
    assert_eq!(peers.len(), 2);
}

#[test]
fn torrent_file_validation() {
    let e = torrent::parse_torrent_bytes(b"this is not bencode").unwrap_err();
    assert_eq!(e.code, ErrorCode::ErrPayloadCorrupt);
    let big = vec![b'd'; torrent::MAX_TORRENT_FILE_BYTES + 1];
    assert!(torrent::parse_torrent_bytes(&big).is_err());
}

#[test]
fn only_socks_proxies_carry_bittorrent() {
    let mk = |host: &str| ProxyConfig { mode: ProxyMode::Manual, host: host.into(), port: 1080, ..Default::default() };
    assert_eq!(torrent::socks_url(&mk("socks5://10.0.0.1")).unwrap().as_deref(), Some("socks5://10.0.0.1:1080"));
    assert!(torrent::socks_url(&mk("10.0.0.1")).is_err(), "HTTP proxy must be refused, not bypassed");
    assert!(torrent::socks_url(&mk("https://10.0.0.1")).is_err());
    assert_eq!(torrent::socks_url(&ProxyConfig { mode: ProxyMode::Direct, ..Default::default() }).unwrap(), None);
    let mut with_auth = mk("socks5://h");
    with_auth.username = Some("u@x".into());
    with_auth.password = Some(zeroize::Zeroizing::new("p:w".into()));
    assert_eq!(torrent::socks_url(&with_auth).unwrap().as_deref(), Some("socks5://u%40x:p%3Aw@h:1080"));
}

// ---- transfers -------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn magnet_download_completes_with_verified_content_and_seeds() {
    let size = 600_000;
    let s = seeder("ubuntu-test.bin", size, None).await;
    let env = env().await;
    let dl = env.manager.create(NewDownload { url: magnet(&s, "ubuntu-test.bin"), start_immediately: true, ..Default::default() }).await.unwrap();
    let done = wait(&env, &dl.id, DownloadState::Completed, 60).await;
    assert_eq!(done.downloaded_bytes, size as i64);
    assert_eq!(done.total_bytes, size as i64);
    assert!(same(&out_file(&env, "ubuntu-test.bin"), size), "piece-verified content must match the source");
    let info = env.manager.torrent_info(&dl.id).await.unwrap().unwrap();
    assert_eq!(info.info_hash, s.info_hash);
    assert_eq!(info.seed_state, "seeding", "ratio 0 = seed until stopped");
    env.manager.torrent_stop_seeding(&dl.id).await.unwrap();
    assert_eq!(env.manager.torrent_info(&dl.id).await.unwrap().unwrap().seed_state, "done");
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn torrent_file_download_and_duplicate_rejected() {
    let size = 300_000;
    let s = seeder("from-file.bin", size, None).await;
    let env = env().await;
    let hint = format!("magnet:?xt=urn:btih:{}&x.pe={}", s.info_hash, s.addr);
    let dl = env
        .manager
        .create_torrent_file(s.torrent_bytes.clone(), NewDownload { url: hint.clone(), start_immediately: true, ..Default::default() })
        .await
        .unwrap();
    assert_eq!(dl.file_name, "from-file.bin");
    assert_eq!(dl.total_bytes, size as i64, "size is known up front for a .torrent");
    wait(&env, &dl.id, DownloadState::Completed, 60).await;
    assert!(same(&out_file(&env, "from-file.bin"), size));
    let again = env.manager.create(NewDownload { url: magnet(&s, "from-file.bin"), start_immediately: true, ..Default::default() }).await.unwrap_err();
    assert_eq!(again.code, ErrorCode::ErrAlreadyExists);
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn existing_file_is_never_overwritten() {
    let size = 100_000;
    let s = seeder("collide.bin", size, None).await;
    let env = env().await;
    let target = out_file(&env, "collide.bin");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, b"precious").unwrap();
    let err = env
        .manager
        .create_torrent_file(s.torrent_bytes.clone(), NewDownload { url: format!("magnet:?xt=urn:btih:{}&x.pe={}", s.info_hash, s.addr), start_immediately: true, ..Default::default() })
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ErrNameCollision);
    assert_eq!(std::fs::read(&target).unwrap(), b"precious");

    // A magnet cannot know the name up front: the run itself must refuse.
    let dl = env.manager.create(NewDownload { url: magnet(&s, "collide.bin"), start_immediately: true, ..Default::default() }).await;
    match dl {
        Err(e) => assert_eq!(e.code, ErrorCode::ErrNameCollision),
        Ok(d) => {
            let failed = wait(&env, &d.id, DownloadState::Failed, 30).await;
            assert_eq!(failed.error_code.as_deref(), Some("ERR_NAME_COLLISION"));
        }
    }
    assert_eq!(std::fs::read(&target).unwrap(), b"precious");
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_and_resume_finishes_correctly() {
    let size = 1_500_000;
    let s = seeder("slow.bin", size, Some(400)).await;
    let env = env().await;
    let dl = env.manager.create(NewDownload { url: magnet(&s, "slow.bin"), start_immediately: true, ..Default::default() }).await.unwrap();
    // Let some pieces arrive, then pause.
    let mut waited = 0;
    while env.manager.get(&dl.id).unwrap().downloaded_bytes < 150_000 && waited < 200 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        waited += 1;
    }
    env.manager.pause(&dl.id).await.unwrap();
    let paused = wait(&env, &dl.id, DownloadState::Paused, 20).await;
    assert!(paused.downloaded_bytes < size as i64, "paused before the end");
    env.manager.resume(&dl.id).await.unwrap();
    wait(&env, &dl.id, DownloadState::Completed, 90).await;
    assert!(same(&out_file(&env, "slow.bin"), size), "resume must not corrupt the file");
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_proxy_refuses_torrents_instead_of_leaking() {
    let s = seeder("proxied.bin", 50_000, None).await;
    let env = env().await;
    env.manager.set_proxy(ProxyConfig { mode: ProxyMode::Manual, host: "127.0.0.1".into(), port: 9, ..Default::default() }).await.unwrap();
    let dl = env.manager.create(NewDownload { url: magnet(&s, "proxied.bin"), start_immediately: true, ..Default::default() }).await.unwrap();
    let failed = wait(&env, &dl.id, DownloadState::Failed, 20).await;
    assert_eq!(failed.error_code.as_deref(), Some("ERR_INVALID_ARGUMENT"));
    assert!(failed.error_message.unwrap().contains("SOCKS5"));
    assert!(!out_file(&env, "proxied.bin").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_with_files_removes_only_the_torrent_files() {
    let size = 80_000;
    let s = seeder("gone.bin", size, None).await;
    let env = env().await;
    let keep = out_file(&env, "unrelated.txt");
    std::fs::create_dir_all(keep.parent().unwrap()).unwrap();
    std::fs::write(&keep, b"mine").unwrap();
    let dl = env.manager.create(NewDownload { url: magnet(&s, "gone.bin"), start_immediately: true, ..Default::default() }).await.unwrap();
    wait(&env, &dl.id, DownloadState::Completed, 60).await;
    assert!(out_file(&env, "gone.bin").exists());
    env.manager.delete(&dl.id, true).await.unwrap();
    assert!(!out_file(&env, "gone.bin").exists(), "torrent file removed");
    assert_eq!(std::fs::read(&keep).unwrap(), b"mine", "unrelated files untouched");
    assert!(env.manager.get(&dl.id).is_err());
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seeding_resumes_after_restart() {
    let size = 120_000;
    let s = seeder("reseed.bin", size, None).await;
    let env = env().await;
    let dl = env.manager.create(NewDownload { url: magnet(&s, "reseed.bin"), start_immediately: true, ..Default::default() }).await.unwrap();
    wait(&env, &dl.id, DownloadState::Completed, 60).await;
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap(); // simulates closing the app
    let row = env.manager.db().read(|c| repo::get_torrent(c, &dl.id)).unwrap().unwrap();
    assert_eq!(row.seed_state, "seeding", "still marked as seeding for the next start");
    env.manager.restore_seeding().await;
    let info = env.manager.torrent_info(&dl.id).await.unwrap().unwrap();
    assert_eq!(info.seed_state, "seeding");
    assert!(info.live || env.manager.torrent_info(&dl.id).await.unwrap().is_some());
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
    drop(s.session.clone());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_file_torrent_lands_in_its_own_folder() {
    let s = seeder_pack("Cool Pack 1.0").await;
    let env = env().await;
    let dl = env.manager.create(NewDownload { url: magnet(&s, "Cool%20Pack%201.0"), start_immediately: true, ..Default::default() }).await.unwrap();
    wait(&env, &dl.id, DownloadState::Completed, 60).await;
    let folder = out_file(&env, "Cool Pack 1.0");
    assert!(same(&folder.join("a.txt"), 1_000), "files must be inside the torrent-named folder");
    assert!(same(&folder.join("Setup").join("b.bin"), 120_000));
    assert!(same(&folder.join("Crack").join("c.bin"), 80_000));
    let shared = folder.parent().unwrap();
    assert!(!shared.join("a.txt").exists() && !shared.join("Setup").exists() && !shared.join("Crack").exists(), "nothing may be scattered into the category folder");
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_folder_rules() {
    use std::path::Path;
    let single = {
        let d = std::env::temp_dir().join(format!("uk-of-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("f.bin"), pattern(40_000)).unwrap();
        d
    };
    let sp = librqbit::spawn_utils::BlockingSpawner::new(1);
    let one = librqbit::create_torrent(&single.join("f.bin"), CreateTorrentOptions { name: Some("f.bin"), trackers: vec![], piece_length: Some(32 * 1024) }, &sp).await.unwrap();
    let one = one.as_bytes().unwrap().to_vec();
    let root = single.join("pack");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("x.bin"), pattern(40_000)).unwrap();
    std::fs::write(root.join("y.bin"), pattern(40_000)).unwrap();
    let many = librqbit::create_torrent(&root, CreateTorrentOptions { name: Some("pack"), trackers: vec![], piece_length: Some(32 * 1024) }, &sp).await.unwrap().as_bytes().unwrap().to_vec();
    let (cat, target) = (Path::new("C:/dl/General"), Path::new("C:/dl/General/pack"));
    assert_eq!(torrent::output_folder(&one, Path::new("C:/dl/General/f.bin"), false), cat, "single file: the category folder");
    assert_eq!(torrent::output_folder(&many, target, false), target, "multi-file: its own folder");
    assert_eq!(torrent::output_folder(&many, target, true), cat, "started before the rule and no own folder yet: old place, no second copy");
    assert_eq!(torrent::output_folder(b"garbage", target, false), cat);
    let own = single.join("pack");
    assert_eq!(torrent::output_folder(&many, &own, true), own, "started and its own folder exists: use it");
    std::fs::remove_dir_all(single).ok();
}
