//! Real-network check of video downloads (DEC-015): installs the real yt-dlp + pinned ffmpeg
//! through `Tools::install`, then downloads a short public video as MP4 and as MP3.
//! `#[ignore]`d: run with
//! `cargo test -p unduhkilat-core --test video_live -- --ignored --nocapture`.

mod common;

use common::Env;
use std::time::Duration;
use unduhkilat_core::engine::manager::NewDownload;
use unduhkilat_core::models::DownloadState;
use unduhkilat_core::tools::Tools;

/// "Me at the zoo" — 19 s, the first YouTube video; small and long-lived.
const URL: &str = "https://www.youtube.com/watch?v=jNQXAC9IVRw";

async fn wait_done(env: &Env, id: &str) -> unduhkilat_core::models::Download {
    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    loop {
        let d = env.manager.get(id).unwrap();
        if !matches!(d.state, DownloadState::Queued | DownloadState::Running) || std::time::Instant::now() > deadline {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn real_video_and_mp3_download() {
    let tools_dir = std::env::temp_dir().join(format!("uk-live-tools-{}", uuid::Uuid::new_v4()));
    let tools = Tools::new(&tools_dir);
    let client = reqwest::Client::builder().build().unwrap();
    tools.install(&client, &|_| {}).await.expect("components install and verify");
    let (y, f) = tools.versions().await;
    println!("yt-dlp {y:?}, ffmpeg {f:?}");
    assert!(y.is_some() && f.is_some(), "both versions readable");

    let dir = tools_dir.clone();
    let env = Env::with_cfg(move |cfg| cfg.tools_dir = Some(dir)).await;

    // Auto-wrap: a plain YouTube link becomes a video item.
    let v = env.manager.create(NewDownload { url: URL.into(), target_path: Some(env.path("v").join("video.mp4")), start_immediately: true, ..Default::default() }).await.unwrap();
    assert!(v.original_url.starts_with("video:"), "{}", v.original_url);
    let vd = wait_done(&env, &v.id).await;
    assert_eq!(vd.state, DownloadState::Completed, "{:?} {:?}", vd.error_code, vd.error_message);
    let size = std::fs::metadata(&vd.target_path).unwrap().len();
    println!("video -> {} ({size} bytes)", vd.target_path);
    assert!(vd.file_name.ends_with(".mp4") && size > 50_000);

    let a = env.manager.create(NewDownload { url: format!("video:a=mp3-128|{URL}"), target_path: Some(env.path("a").join("audio.mp3")), start_immediately: true, ..Default::default() }).await.unwrap();
    let ad = wait_done(&env, &a.id).await;
    assert_eq!(ad.state, DownloadState::Completed, "{:?} {:?}", ad.error_code, ad.error_message);
    let head = std::fs::read(&ad.target_path).unwrap();
    println!("mp3 -> {} ({} bytes)", ad.target_path, head.len());
    assert!(head.starts_with(b"ID3") || head.first() == Some(&0xFF), "an MP3 file");

    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
    std::fs::remove_dir_all(&tools_dir).ok();
}
