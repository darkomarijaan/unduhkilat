//! Integration tests for video downloads (DEC-015). Offline: instead of real yt-dlp,
//! it uses a fake `yt-dlp.cmd` whose behavior is set per test via a mode file.

mod common;

use common::Env;
use std::path::{Path, PathBuf};
use std::time::Duration;
use unduhkilat_core::engine::manager::NewDownload;
use unduhkilat_core::models::DownloadState;

/// The shared tools folder must be known BEFORE the env is created (the cfg is captured
/// in the manager). One folder per test process, cleaned up when the test finishes.
fn tools_dir() -> PathBuf {
    std::env::temp_dir().join(format!("uk-video-tools-{}", std::process::id()))
}

/// Full environment + fake yt-dlp. One script per mode (`video_ytdlp_name` differs per
/// test) so parallel tests share nothing mutable.
async fn video_env(mode: &str) -> (Env, PathBuf) {
    let tools = tools_dir();
    std::fs::create_dir_all(&tools).unwrap();
    write_fake(&tools, mode);
    let env = Env::with_cfg(|cfg| {
        cfg.tools_dir = Some(tools.clone());
        cfg.video_ytdlp_name = format!("fake-ytdlp-{mode}.cmd");
    })
    .await;
    (env, tools)
}

/// Common head: answer the `-J` metadata probe (meta.json wins when present).
const FAKE_HEAD: &str = "@echo off\r\nif \"%~1\"==\"-J\" (\r\n  if exist \"%~dp0meta.json\" (\r\n    type \"%~dp0meta.json\"\r\n    exit /b 0\r\n  )\r\n  echo {\"title\":\"Judul Video\",\"duration\":60,\"formats\":[{\"height\":1080},{\"height\":720}]}\r\n  exit /b 0\r\n)\r\n";

fn write_fake(tools: &Path, mode: &str) {
    let body = match mode {
        "login" => "echo ERROR: Sign in to confirm your age 1>&2\r\nexit /b 1\r\n".to_string(),
        "nooutput" => "echo UK 100 100 500\r\nexit /b 0\r\n".to_string(),
        "slow" => "echo UK 10 10000 100\r\nping -n 31 127.0.0.1 >nul\r\nexit /b 1\r\n".to_string(),
        // "ok": progress lines, then the result file at the -o stub.
        _ => "echo UK 500 1000 8000\r\necho UK 1000 1000 8000\r\nset STUB=\r\n:loop\r\nif \"%~1\"==\"-o\" set STUB=%~2\r\nshift\r\nif not \"%~1\"==\"\" goto loop\r\nif not \"%STUB%\"==\"\" type nul > \"%STUB%.mp4\"\r\nexit /b 0\r\n".to_string(),
    };
    std::fs::write(tools.join(format!("fake-ytdlp-{mode}.cmd")), format!("{FAKE_HEAD}{body}")).unwrap();
}

fn video_req(env: &Env) -> NewDownload {
    NewDownload {
        url: "video:v=best|https://example.com/watch?v=ok".into(),
        target_path: Some(env.path("videos").join("video.mp4")),
        start_immediately: true,
        ..Default::default()
    }
}

/// Wait until the state settles (a completed transfer, or an error/pause).
async fn wait_state(env: &Env, id: &str, secs: u64) -> DownloadState {
    // Cancel/finish leaves queued/running; give the worker time to land elsewhere.
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let d = env.manager.get(id).unwrap();
        if !matches!(d.state, DownloadState::Queued | DownloadState::Running) || std::time::Instant::now() > deadline {
            return d.state;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn video_download_completes_and_lands_under_final_name() {
    const MODE: &str = "ok";
    let (env, tools) = video_env(MODE).await;
    std::fs::write(tools.join("meta.json"), serde_json::json!({"title": "Judul Video", "duration": 60, "formats": [{"height": 1080}, {"height": 720}]}).to_string()).unwrap();
    let dl = env.manager.create(video_req(&env)).await.unwrap();
    let st = wait_state(&env, &dl.id, 60).await;
    let done = env.manager.get(&dl.id).unwrap();
    assert_eq!(st, DownloadState::Completed, "err={:?} msg={:?}", done.error_code, done.error_message);
    // The final name comes from the fake -J title (placeholder "video.mp4" is replaced).
    let final_path = env.path("videos").join("Judul Video.mp4");
    assert!(final_path.exists(), "the result file exists under the final name");
    assert_eq!(done.file_name, "Judul Video.mp4");
    let work = env.path("videos").join(format!(".uk-video-{}", dl.id));
    assert!(!work.exists(), "the work folder is deleted");
    assert_eq!(done.total_bytes, std::fs::metadata(&final_path).unwrap().len() as i64);
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn video_without_tools_fails_with_tools_missing() {
    let env = Env::with_cfg(|cfg| {
        cfg.tools_dir = None;
        cfg.video_ytdlp_name = "fake-ytdlp.cmd".into();
    })
    .await;
    let dl = env.manager.create(NewDownload {
        url: "video:v=best|https://youtu.be/x".into(),
        target_path: Some(env.path("v.mp4")),
        start_immediately: true,
        ..Default::default()
    }).await.unwrap();
    let st = wait_state(&env, &dl.id, 30).await;
    assert_eq!(st, DownloadState::Failed);
    let done = env.manager.get(&dl.id).unwrap();
    assert_eq!(done.error_code.as_deref(), Some("ERR_VIDEO_TOOLS_MISSING"));
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn video_login_error_maps_and_fails() {
    const MODE: &str = "login";
    let (env, _tools) = video_env(MODE).await;
    let dl = env.manager.create(video_req(&env)).await.unwrap();
    let st = wait_state(&env, &dl.id, 30).await;
    assert_eq!(st, DownloadState::Failed);
    let done = env.manager.get(&dl.id).unwrap();
    assert_eq!(done.error_code.as_deref(), Some("ERR_VIDEO_LOGIN_REQUIRED"), "msg={:?}", done.error_message);
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_output_fails() {
    const MODE: &str = "nooutput";
    let (env, _tools) = video_env(MODE).await;
    let dl = env.manager.create(video_req(&env)).await.unwrap();
    let st = wait_state(&env, &dl.id, 30).await;
    assert_eq!(st, DownloadState::Failed);
    let done = env.manager.get(&dl.id).unwrap();
    assert_eq!(done.error_code.as_deref(), Some("ERR_VIDEO_EXTRACT_FAILED"), "msg={:?}", done.error_message);
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_stops_the_video_download() {
    const MODE: &str = "slow";
    let (env, _tools) = video_env(MODE).await;
    let dl = env.manager.create(video_req(&env)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    env.manager.cancel(&dl.id, false).await.unwrap();
    let st = wait_state(&env, &dl.id, 30).await;
    assert_eq!(st, DownloadState::Cancelled, "cancel really stops the process");
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn collision_at_final_name_refuses() {
    const MODE: &str = "ok";
    let (env, tools) = video_env(MODE).await;
    std::fs::write(tools.join("meta.json"), serde_json::json!({"title": "Judul Video", "duration": 60, "formats": []}).to_string()).unwrap();
    std::fs::create_dir_all(env.path("videos")).unwrap();
    // The file is in the way at the FINAL name (Judul Video.mp4) — the row is created
    // with the placeholder name, and the refusal comes from the transfer.
    std::fs::write(env.path("videos/Judul Video.mp4"), b"precious").unwrap();
    let dl = env.manager.create(video_req(&env)).await.unwrap();
    let st = wait_state(&env, &dl.id, 30).await;
    assert_eq!(st, DownloadState::Failed);
    let done = env.manager.get(&dl.id).unwrap();
    assert_eq!(done.error_code.as_deref(), Some("ERR_NAME_COLLISION"), "msg={:?}", done.error_message);
    assert_eq!(std::fs::read(env.path("videos/Judul Video.mp4")).unwrap(), b"precious", "the existing file is untouched");
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
}
