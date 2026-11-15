//! Integration tests for video downloads (DEC-015). Offline: instead of real yt-dlp,
//! it uses a fake `yt-dlp.cmd` whose behavior is set per test via a mode file.

mod common;

use common::Env;
use std::path::{Path, PathBuf};
use std::time::Duration;
use unduhkilat_core::engine::manager::NewDownload;
use unduhkilat_core::models::DownloadState;

/// Each test gets its own tools folder (known BEFORE the env is created, since the cfg is
/// captured by the manager), so marker files and meta.json never leak between tests.
fn tools_dir() -> PathBuf {
    std::env::temp_dir().join(format!("uk-video-tools-{}", uuid::Uuid::new_v4()))
}

/// Full environment + fake yt-dlp for one mode.
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

/// Common head: answer the `-J` metadata probe (meta.json wins when present), then for a
/// download walk the arguments: remember the `-o` stub and the URL after `--`. Like real
/// yt-dlp, a download without a URL is an error.
const FAKE_HEAD: &str = "@echo off\r\n\
if \"%~1\"==\"-J\" (\r\n  if exist \"%~dp0meta.json\" (\r\n    type \"%~dp0meta.json\"\r\n    exit /b 0\r\n  )\r\n  echo {\"title\":\"Judul Video\",\"duration\":60,\"formats\":[{\"height\":1080},{\"height\":720}]}\r\n  exit /b 0\r\n)\r\n\
set \"HERE=%~dp0\"\r\nset STUB=\r\nset URL=\r\nset /a N=0\r\n\
:loop\r\nif \"%~1\"==\"--\" (\r\n  set \"URL=%~2\"\r\n  goto done\r\n)\r\nif \"%~1\"==\"-o\" set \"STUB=%~2\"\r\nshift\r\nset /a N+=1\r\nif %N% LSS 60 goto loop\r\n:done\r\n\
if \"%URL%\"==\"\" (\r\n  echo ERROR: You must provide at least one URL. 1>&2\r\n  exit /b 2\r\n)\r\n\
for %%D in (\"%STUB%\") do set \"WORK=%%~dpD\"\r\n";

/// Shared body of a successful run: progress lines, then the merged result at the stub.
const FAKE_OK: &str = "echo UK 500 1000 8000\r\necho UK 1000 1000 8000\r\n> \"%WORK%out.mp4\" echo merged\r\nexit /b 0\r\n";

fn write_fake(tools: &Path, mode: &str) {
    let body = match mode {
        "login" => "echo ERROR: Sign in to confirm your age 1>&2\r\nexit /b 1\r\n".to_string(),
        "nooutput" => "echo UK 100 100 500\r\nexit /b 0\r\n".to_string(),
        // Leaves a .part behind (as yt-dlp does), then hangs until killed.
        "slow" => "echo UK 10 10000 100\r\n> \"%WORK%out.mp4.part\" echo partial\r\nping -n 31 127.0.0.1 >nul\r\nexit /b 1\r\n".to_string(),
        // First run: the bot check (a stale-version sign). After the update (marker file), succeed.
        "stale" => format!("if not exist \"%HERE%stale-ran.txt\" (\r\n  > \"%HERE%stale-ran.txt\" echo 1\r\n  echo ERROR: Sign in to confirm you're not a bot 1>&2\r\n  exit /b 1\r\n)\r\n{FAKE_OK}"),
        // Records whether a previous run's .part was still there when it started.
        "resume" => format!("if exist \"%WORK%out.mp4.part\" (> \"%HERE%resume-saw-part.txt\" echo 1)\r\nif not exist \"%HERE%resume-ran.txt\" (\r\n  > \"%HERE%resume-ran.txt\" echo 1\r\n  echo UK 10 10000 100\r\n  > \"%WORK%out.mp4.part\" echo partial\r\n  ping -n 31 127.0.0.1 >nul\r\n  exit /b 1\r\n)\r\n{FAKE_OK}"),
        // "ok"
        _ => FAKE_OK.to_string(),
    };
    std::fs::write(tools.join(format!("fake-ytdlp-{mode}.cmd")), format!("{FAKE_HEAD}{body}")).unwrap();
}

/// A video item without a chosen name: the target folder is fixed, the file name is the
/// engine's placeholder (replaced by the title when the run starts).
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
async fn two_unnamed_videos_in_one_folder_are_both_queued() {
    const MODE: &str = "slow";
    let (env, _tools) = video_env(MODE).await;
    let a = env.manager.create(video_req(&env)).await.unwrap();
    let b = env.manager.create(video_req(&env)).await.expect("a second placeholder must not clash with the first");
    assert_ne!(a.target_path, b.target_path);
    env.manager.cancel(&a.id, false).await.unwrap();
    env.manager.cancel(&b.id, false).await.unwrap();
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_bot_check_updates_once_then_succeeds() {
    const MODE: &str = "stale";
    let (env, tools) = video_env(MODE).await;
    let dl = env.manager.create(video_req(&env)).await.unwrap();
    let st = wait_state(&env, &dl.id, 60).await;
    let done = env.manager.get(&dl.id).unwrap();
    assert_eq!(st, DownloadState::Completed, "err={:?} msg={:?}", done.error_code, done.error_message);
    assert!(tools.join("stale-ran.txt").exists(), "files: {:?}", std::fs::read_dir(&tools).unwrap().flatten().map(|e| e.file_name()).collect::<Vec<_>>());
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_keeps_the_partial_download_for_resume() {
    const MODE: &str = "resume";
    let (env, tools) = video_env(MODE).await;
    let dl = env.manager.create(video_req(&env)).await.unwrap();
    // Wait until the first run has written its .part.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !tools.join("resume-ran.txt").exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(tools.join("resume-ran.txt").exists(), "first run started; state={:?}", env.manager.get(&dl.id).unwrap());
    tokio::time::sleep(Duration::from_millis(500)).await;
    env.manager.pause(&dl.id).await.unwrap();
    let paused = wait_state(&env, &dl.id, 30).await;
    assert_eq!(paused, DownloadState::Paused, "{:?}", env.manager.get(&dl.id).unwrap());
    env.manager.resume(&dl.id).await.unwrap();
    let st = wait_state(&env, &dl.id, 60).await;
    let done = env.manager.get(&dl.id).unwrap();
    assert_eq!(st, DownloadState::Completed, "err={:?} msg={:?}", done.error_code, done.error_message);
    assert!(tools.join("resume-saw-part.txt").exists(), "the resumed run found the previous .part (yt-dlp --continue)");
    env.manager.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restarting_a_finished_video_never_empties_it() {
    const MODE: &str = "ok";
    let (env, tools) = video_env(MODE).await;
    std::fs::write(tools.join("meta.json"), serde_json::json!({"title": "Sekali", "duration": 5, "formats": []}).to_string()).unwrap();
    let dl = env.manager.create(video_req(&env)).await.unwrap();
    assert_eq!(wait_state(&env, &dl.id, 60).await, DownloadState::Completed);
    let file = env.path("videos").join("Sekali.mp4");
    let before = std::fs::read(&file).unwrap();
    assert!(!before.is_empty());
    let _ = env.manager.restart(&dl.id).await;
    let _ = wait_state(&env, &dl.id, 30).await;
    assert_eq!(std::fs::read(&file).unwrap(), before, "the finished file is untouched by restart");
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
