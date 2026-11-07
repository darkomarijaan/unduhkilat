//! `UnduhKilat.exe cli <url> [<url> ...] [--connections N] [--out DIR]`
//! A `magnet:` link, a `.torrent` URL or a local `.torrent` path starts a BitTorrent download.
//! Headless engine driver, handy for scripting and for verifying the engine.

use crate::boot::boot;
use std::path::PathBuf;
use std::time::Duration;
use unduhkilat_core::engine::manager::NewDownload;
use unduhkilat_core::engine::EngineEvent;
use unduhkilat_core::models::DownloadState;

fn arg_value(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let i = args.iter().position(|a| a == flag)?;
    args.remove(i);
    (i < args.len()).then(|| args.remove(i))
}

pub async fn run(mut args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let connections = arg_value(&mut args, "--connections").map(|v| v.parse::<u32>()).transpose()?;
    let out_dir = arg_value(&mut args, "--out").map(PathBuf::from);
    let b = boot().await?;
    let manager = b.manager;

    if args.is_empty() {
        println!("usage: UnduhKilat.exe cli <url> [<url> ...] [--connections N] [--out DIR]");
        return Ok(());
    }
    for url in args {
        let lower = url.to_ascii_lowercase();
        let path_part = lower.split(['?', '#']).next().unwrap_or("").to_string();
        let is_torrent_ref = unduhkilat_core::engine::torrent::is_magnet(&url) || path_part.ends_with(".torrent");
        let target_path = out_dir.as_ref().map(|d| d.join(if is_torrent_ref { "torrent".to_string() } else { unduhkilat_core::engine::http::file_name_from_url(&url).unwrap_or_else(|| "download".into()) }));
        let req = NewDownload { url: url.clone(), target_path, connection_count: connections, start_immediately: true, ..Default::default() };
        let d = if lower.starts_with("http") && path_part.ends_with(".torrent") {
            manager.create_torrent_url(&url, req).await?
        } else if !lower.contains("://") && path_part.ends_with(".torrent") {
            // Local .torrent file (DEC-008).
            manager.create_torrent_file(std::fs::read(&url)?, NewDownload { url: String::new(), ..req }).await?
        } else {
            manager.create(req).await?
        };
        println!("queued {} -> {}", d.original_url, d.target_path);
    }
    let mut rx = manager.subscribe();
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        tokio::select! {
            _ = &mut ctrl_c => {
                println!("\nstopping: checkpointing running downloads...");
                manager.shutdown(Duration::from_secs(15)).await?;
                return Ok(());
            }
            ev = rx.recv() => {
                if let Ok(EngineEvent::StateChanged { id, state, error_code }) = ev {
                    println!("{id}: {}{}", state.as_str(), error_code.map(|c| format!(" ({c})")).unwrap_or_default());
                }
            }
            _ = tick.tick() => {
                let all = manager.db().read(|c| unduhkilat_core::repo::list_downloads(c, None))?;
                if !all.iter().any(|d| matches!(d.state, DownloadState::Queued | DownloadState::Running)) { return Ok(()); }
            }
        }
    }
}
