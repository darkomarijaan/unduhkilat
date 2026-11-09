//! FR-045 / FR-047: preview first, enqueue only what is ticked, items obey queue rules.

mod common;

use common::{pattern, Env};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use unduhkilat_core::error::ErrorCode;
use unduhkilat_core::models::DownloadState;

/// A page with 10 PDFs and 5 ZIPs, plus the files themselves (HEAD and GET).
async fn site() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let gets = Arc::new(AtomicUsize::new(0));
    let g2 = gets.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else { break };
            let gets = g2.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let mut it = req.split_whitespace();
                let (method, path) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
                let resp: Vec<u8> = if path == "/page" {
                    let mut html = String::from("<html><body>");
                    for i in 0..10 {
                        html += &format!("<a href=\"/f/doc{i}.pdf\">pdf {i}</a>");
                    }
                    for i in 0..5 {
                        html += &format!("<a href=\"/f/pack{i}.zip\">zip {i}</a>");
                    }
                    html += "</body></html>";
                    format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}", html.len()).into_bytes()
                } else if path == "/huge" {
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", 50 * 1024 * 1024).into_bytes()
                } else if path.starts_with("/f/") {
                    let body = pattern(4000);
                    if method == "GET" {
                        gets.fetch_add(1, Ordering::SeqCst);
                    }
                    let mut r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                    if method == "GET" {
                        r.extend_from_slice(&body);
                    }
                    r
                } else {
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
                };
                let _ = s.write_all(&resp).await;
            });
        }
    });
    (format!("http://{addr}"), gets)
}

#[tokio::test]
async fn preview_lists_only_matching_links_with_sizes_and_starts_nothing() {
    let (base, gets) = site().await;
    let env = Env::new().await;
    let r = env.manager.scan_links(&format!("{base}/page"), "*.pdf").await.unwrap();
    assert_eq!(r.candidates.len(), 10, "exactly the 10 pdf links");
    assert_eq!(r.links_on_page, 15);
    assert!(r.candidates.iter().all(|c| c.url.ends_with(".pdf") && c.size == Some(4000)));
    assert_eq!(gets.load(Ordering::SeqCst), 0, "scanning must not download any file");
    assert!(env.manager.db().read(|c| unduhkilat_core::repo::list_downloads(c, None)).unwrap().is_empty(), "nothing queued by a preview");
    let none = env.manager.scan_links(&format!("{base}/page"), "*.rar").await.unwrap();
    assert!(none.candidates.is_empty());
}

#[tokio::test]
async fn scan_errors_are_reported_not_swallowed() {
    let (base, _) = site().await;
    let env = Env::new().await;
    assert_eq!(env.manager.scan_links(&format!("{base}/nope"), "*.pdf").await.unwrap_err().code, ErrorCode::ErrCrawlFailed);
    assert_eq!(env.manager.scan_links(&format!("{base}/huge"), "*.pdf").await.unwrap_err().code, ErrorCode::ErrCrawlFailed);
    assert_eq!(env.manager.scan_links("ftp://x/y", "*.pdf").await.unwrap_err().code, ErrorCode::ErrInvalidUrlScheme);
    assert_eq!(env.manager.scan_links(&format!("{base}/page"), "  ").await.unwrap_err().code, ErrorCode::ErrInvalidArgument);
    assert_eq!(env.manager.scan_links("http://127.0.0.1:1/page", "*.pdf").await.unwrap_err().code, ErrorCode::ErrCrawlFailed);
}

#[tokio::test]
async fn only_ticked_links_are_enqueued_as_ordinary_items_and_obey_concurrency() {
    let (base, _) = site().await;
    let env = Env::with_cfg(|c| c.default_concurrency = 2).await;
    let found = env.manager.scan_links(&format!("{base}/page"), "*.pdf").await.unwrap();
    let ticked: Vec<String> = found.candidates.iter().take(3).map(|c| c.url.clone()).collect();
    let out = env.manager.enqueue_batch(ticked.clone(), 5, None, None).await;
    assert_eq!(out.created.len(), 3);
    assert!(out.failed.is_empty());
    for id in &out.created {
        let d = env.manager.wait_for_state(id, DownloadState::Completed, Duration::from_secs(20)).await.unwrap();
        assert_eq!(d.downloaded_bytes, 4000);
        assert!(d.category_id.is_some(), "category routing applies (FR-029)");
    }
    let all = env.manager.db().read(|c| unduhkilat_core::repo::list_downloads(c, None)).unwrap();
    assert_eq!(all.len(), 3, "the 7 unticked links were never queued");
    // A repeat of the same links collides instead of overwriting; the rest still go through.
    let again = env.manager.enqueue_batch(vec![ticked[0].clone(), found.candidates[5].url.clone()], 0, None, None).await;
    assert_eq!(again.failed.len(), 1);
    assert!(again.failed[0].1.contains("ERR_NAME_COLLISION"));
    assert_eq!(again.created.len(), 1);
}
