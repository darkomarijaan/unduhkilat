//! DEC-009 file search against a local mock of the four providers (no internet).

mod common;

use common::{pattern, Env};
use librqbit::CreateTorrentOptions;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use unduhkilat_core::config::keys;
use unduhkilat_core::engine::search::{Endpoints, Hit, Provider, Source, ALL_PROVIDERS};
use unduhkilat_core::error::ErrorCode;
use unduhkilat_core::repo;

const HASH_A: &str = "43F4001DE4AB25D521C63684E2B69804193ED9D9";

struct Mock {
    ep: Endpoints,
    requests: Arc<AtomicUsize>,
    torrent_bytes: Vec<u8>,
}

async fn make_torrent_bytes() -> Vec<u8> {
    let dir = std::env::temp_dir().join(format!("uk-srch-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("item.bin");
    std::fs::write(&f, pattern(70_000)).unwrap();
    let sp = librqbit::spawn_utils::BlockingSpawner::new(2);
    let c = librqbit::create_torrent(&f, CreateTorrentOptions { name: Some("item.bin"), trackers: vec![], piece_length: Some(32 * 1024) }, &sp).await.unwrap();
    std::fs::remove_dir_all(dir).ok();
    c.as_bytes().unwrap().to_vec()
}

async fn mock(nyaa_fails: bool) -> Mock {
    let torrent_bytes = make_torrent_bytes().await;
    let tb = torrent_bytes.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let r2 = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else { break };
            let (tb, r2) = (tb.clone(), r2.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                r2.fetch_add(1, Ordering::SeqCst);
                let (status, ctype, body): (&str, &str, Vec<u8>) = if path.starts_with("/ia") {
                    ("200 OK", "application/json", br#"{"response":{"docs":[{"identifier":"item1","title":"Public Domain Film","item_size":123456}]}}"#.to_vec())
                } else if path.starts_with("/apibay") {
                    ("200 OK", "application/json", format!(r#"[{{"id":"1","name":"Same Torrent","info_hash":"{HASH_A}","seeders":"40","leechers":"2","size":"999"}}]"#).into_bytes())
                } else if path.starts_with("/nyaa") {
                    if nyaa_fails {
                        ("500 Internal Server Error", "text/plain", b"boom".to_vec())
                    } else {
                        ("200 OK", "application/rss+xml", format!("<rss><channel><item><title>Same Torrent</title><nyaa:seeders>3</nyaa:seeders><nyaa:leechers>0</nyaa:leechers><nyaa:infoHash>{}</nyaa:infoHash><nyaa:size>1 KiB</nyaa:size></item></channel></rss>", HASH_A.to_lowercase()).into_bytes())
                    }
                } else if path.starts_with("/at") {
                    ("200 OK", "application/rss+xml", b"<channel><item><title>Film dataset</title><infohash>447df5f09ca5c48b0cf2844cfffc1425ae18aaa4</infohash><description>x</description><size>77</size></item></channel>".to_vec())
                } else if path.starts_with("/dl/item1/item1_archive.torrent") {
                    ("200 OK", "application/x-bittorrent", tb)
                } else {
                    ("404 Not Found", "text/plain", b"nope".to_vec())
                };
                let head = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(&body).await;
            });
        }
    });
    let b = format!("http://{addr}");
    Mock {
        ep: Endpoints { internet_archive: format!("{b}/ia"), academic_rss: format!("{b}/at"), apibay: format!("{b}/apibay"), nyaa: format!("{b}/nyaa"), archive_download: format!("{b}/dl") },
        requests,
        torrent_bytes,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn merges_providers_dedupes_and_orders_by_seeders() {
    let m = mock(false).await;
    let env = Env::new().await;
    let out = env.manager.search_files_with("film", &ALL_PROVIDERS, &m.ep).await.unwrap();
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    // apibay and Nyaa list the same torrent: one row, the better-seeded one wins.
    let same: Vec<&Hit> = out.hits.iter().filter(|h| h.title == "Same Torrent").collect();
    assert_eq!(same.len(), 1);
    assert_eq!(same[0].provider, Provider::Apibay);
    assert_eq!(out.hits[0].title, "Same Torrent", "highest seeders first");
    assert!(out.hits.iter().any(|h| h.provider == Provider::InternetArchive && matches!(&h.source, Source::TorrentUrl(u) if u.ends_with("/item1/item1_archive.torrent"))));
    assert!(out.hits.iter().any(|h| h.provider == Provider::AcademicTorrents && h.title == "Film dataset"));
    assert_eq!(m.requests.load(Ordering::SeqCst), 4, "exactly one request per provider");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_provider_does_not_hide_the_others() {
    let m = mock(true).await;
    let env = Env::new().await;
    let out = env.manager.search_files_with("film", &[Provider::Nyaa, Provider::InternetArchive], &m.ep).await.unwrap();
    assert_eq!(out.hits.len(), 1);
    assert_eq!(out.errors.len(), 1);
    assert_eq!(out.errors[0].0, Provider::Nyaa);
    assert!(out.errors[0].1.contains("500"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_is_sent_for_invalid_input_and_defaults_are_conservative() {
    let m = mock(false).await;
    let env = Env::new().await;
    assert_eq!(env.manager.search_files_with("a", &ALL_PROVIDERS, &m.ep).await.unwrap_err().code, ErrorCode::ErrInvalidArgument);
    assert_eq!(env.manager.search_files_with("film", &[], &m.ep).await.unwrap_err().code, ErrorCode::ErrInvalidArgument);
    assert_eq!(m.requests.load(Ordering::SeqCst), 0, "rejected before any network traffic");
    // Legal catalogues on, general indexes off until the user ticks them.
    assert_eq!(env.manager.search_providers(), vec![Provider::InternetArchive, Provider::AcademicTorrents]);
    env.manager.set_search_provider(Provider::Nyaa, true).await.unwrap();
    env.manager.set_search_provider(Provider::InternetArchive, false).await.unwrap();
    assert_eq!(env.manager.search_providers(), vec![Provider::AcademicTorrents, Provider::Nyaa]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ticked_hits_become_ordinary_queue_items() {
    let m = mock(false).await;
    let env = Env::new().await;
    env.manager.db().write(|c| Ok(repo::set_setting(c, keys::TORRENT_DHT, "0")?)).await.unwrap();
    let out = env.manager.search_files_with("film", &[Provider::InternetArchive, Provider::Apibay], &m.ep).await.unwrap();
    assert_eq!(out.hits.len(), 2);
    let res = env.manager.enqueue_hits(out.hits.clone(), 3).await;
    assert!(res.failed.is_empty(), "{:?}", res.failed);
    assert_eq!(res.created.len(), 2);
    let all = env.manager.db().read(|c| repo::list_downloads(c, None)).unwrap();
    assert_eq!(all.len(), 2);
    // The .torrent URL hit was fetched, parsed and recorded with its real name and size.
    let ia = all.iter().find(|d| d.file_name == "item.bin").expect(".torrent-based item");
    assert_eq!(ia.total_bytes, 70_000);
    // Queuing the same hit again is refused as a duplicate, not silently doubled.
    let again = env.manager.enqueue_hits(vec![out.hits[0].clone()], 0).await;
    assert_eq!(again.failed.len(), 1);
    assert!(again.failed[0].1.contains("ERR_ALREADY_EXISTS"));
    for id in res.created {
        env.manager.delete(&id, false).await.unwrap();
    }
    env.manager.shutdown(std::time::Duration::from_secs(5)).await.unwrap();
    let _ = m.torrent_bytes;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_torrent_url_is_reported() {
    let m = mock(false).await;
    let env = Env::new().await;
    let h = Hit { provider: Provider::InternetArchive, title: "gone".into(), size: None, seeders: None, leechers: None, source: Source::TorrentUrl(format!("{}/missing.torrent", m.ep.archive_download)), page: None };
    let r = env.manager.enqueue_hits(vec![h], 0).await;
    assert_eq!(r.failed.len(), 1);
    assert!(r.created.is_empty());
}

/// Real providers. Run on demand: `cargo test -p unduhkilat-core --test search -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs internet"]
async fn live_providers_return_parseable_results() {
    let env = Env::new().await;
    let out = env.manager.search_files("sintel", &ALL_PROVIDERS).await.unwrap();
    for p in ALL_PROVIDERS {
        let n = out.hits.iter().filter(|h| h.provider == p).count();
        println!("{:<18} {n} hits", p.name());
    }
    for (p, e) in &out.errors {
        println!("ERROR {}: {e}", p.name());
    }
    for h in out.hits.iter().take(5) {
        println!("{:?} | {} | {:?} | seeds {:?}", h.provider, h.title, h.size, h.seeders);
    }
    assert!(out.hits.iter().any(|h| h.provider == Provider::InternetArchive), "Internet Archive");
    assert!(out.hits.iter().any(|h| h.provider == Provider::Apibay), "apibay");
    assert!(out.hits.iter().any(|h| h.provider == Provider::Nyaa), "Nyaa");
    let at = env.manager.search_files("dataset", &[Provider::AcademicTorrents]).await.unwrap();
    println!("Academic Torrents (dataset): {} hits, errors {:?}", at.hits.len(), at.errors);
    assert!(at.errors.is_empty());
}
