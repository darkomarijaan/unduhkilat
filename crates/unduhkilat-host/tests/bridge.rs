//! End-to-end: the real `unduhkilat-host.exe` process <-> named pipe <-> Bridge <-> Manager.
#![cfg(windows)]

#[path = "../../unduhkilat-core/tests/common/mod.rs"]
mod common;

use common::{Mode, TestServer};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use unduhkilat_core::bridge::{pipe_name, Bridge, Proposal};
use unduhkilat_core::config::EngineConfig;
use unduhkilat_core::db::Db;
use unduhkilat_core::engine::manager::{Manager, NewDownload};
use unduhkilat_core::error::ErrorCode;
use unduhkilat_core::ipc::{read_frame_async, write_frame_async, Request, Response};
use unduhkilat_core::native::allowed_origin;
use unduhkilat_core::paths::AppPaths;

const T: Duration = Duration::from_secs(20);

struct Rig {
    dir: PathBuf,
    paths: AppPaths,
    manager: Manager,
    bridge: Bridge,
    proposals: Receiver<Proposal>,
}

impl Rig {
    async fn new() -> Rig {
        let dir = std::env::temp_dir().join(format!("uk-bridge-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths { data_dir: dir.clone(), state_dir: dir.join("state"), logs_dir: dir.join("logs"), portable: true };
        paths.create_dirs().unwrap();
        let db = Db::open(paths.db_path()).unwrap();
        unduhkilat_core::category::seed_defaults(&db, &dir.join("dl")).unwrap();
        let cfg = EngineConfig { checkpoint_interval: Duration::from_millis(40), ..Default::default() };
        let manager = Manager::start(db, cfg).await.unwrap();
        let (tx, proposals) = channel();
        let bridge = Bridge::start(&tokio::runtime::Handle::current(), manager.clone(), &paths, tx, Arc::new(|| {})).unwrap();
        Rig { dir, paths, manager, bridge, proposals }
    }

    fn spawn_host(&self, origin: &str) -> Host {
        spawn_host_in(&self.dir, origin)
    }
}

fn spawn_host_in(dir: &PathBuf, origin: &str) -> Host {
    let mut child = Command::new(env!("CARGO_BIN_EXE_unduhkilat-host"))
        .arg(origin)
        .env("UNDUHKILAT_DATA_DIR", dir)
        .env("UNDUHKILAT_NO_AUTOLAUNCH", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    Host { stdin: child.stdin.take().unwrap(), stdout: child.stdout.take().unwrap(), child }
}

impl Drop for Rig {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

struct Host {
    stdin: ChildStdin,
    stdout: ChildStdout,
    child: Child,
}

impl Host {
    async fn call(&mut self, id: &str, command: &str, payload: Value) -> Response {
        write_frame_async(&mut self.stdin, &Request { id: id.into(), command: command.into(), payload }).await.unwrap();
        loop {
            let v: Value = self.next().await;
            // Skip pushed events; wait for our response.
            if v.get("id").and_then(Value::as_str) == Some(id) {
                return serde_json::from_value(v).unwrap();
            }
        }
    }
    async fn next(&mut self) -> Value {
        tokio::time::timeout(T, read_frame_async::<_, Value>(&mut self.stdout)).await.expect("timed out").unwrap().expect("host closed stdout")
    }
}

fn err_code(r: &Response) -> ErrorCode {
    match r {
        Response::Error { error, .. } => error.code,
        other => panic!("expected error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn caller_with_the_wrong_origin_is_refused_and_the_host_exits() {
    let rig = Rig::new().await;
    let mut h = rig.spawn_host("chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/");
    let v = h.next().await;
    assert_eq!(v["status"], "error");
    assert_eq!(v["error"]["code"], "ERR_UNAUTHORIZED");
    let status = tokio::time::timeout(T, h.child.wait()).await.unwrap().unwrap();
    assert_eq!(status.code(), Some(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ping_round_trips_through_the_pipe() {
    let rig = Rig::new().await;
    let mut h = rig.spawn_host(&allowed_origin());
    match h.call("r1", "ping", json!({})).await {
        Response::Ok { id, data } => {
            assert_eq!(id, "r1");
            assert_eq!(data["pong"], true);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn intercept_becomes_a_proposal_and_starts_nothing() {
    let rig = Rig::new().await;
    let mut h = rig.spawn_host(&allowed_origin());
    let r = h
        .call(
            "r2",
            "browser.intercept",
            json!({
                "url": "https://files.example.test/a.zip", "file_name": "a.zip", "mime_type": "application/zip", "total_bytes": 1234,
                "referer": "https://files.example.test/page", "user_agent": "UA/1", "cookies": "sid=abc"
            }),
        )
        .await;
    let Response::Ok { data, .. } = r else { panic!("{r:?}") };
    assert_eq!(data["state"], "pending_confirmation");

    let p = rig.proposals.recv_timeout(T).unwrap();
    assert_eq!(p.url, "https://files.example.test/a.zip");
    assert_eq!(p.source, "browser");
    assert_eq!(p.cookies.as_deref(), Some("sid=abc"));
    assert_eq!(p.referer.as_deref(), Some("https://files.example.test/page"));
    assert_eq!(p.total_bytes, Some(1234));
    assert_eq!(data["request_id"], p.request_id);

    // FR-039 / KPI-011: no download, no network request, until the user confirms.
    let all = rig.manager.db().read(|c| unduhkilat_core::repo::list_downloads(c, None)).unwrap();
    assert!(all.is_empty(), "interception must not create a download");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn media_requests_use_the_same_confirmation_path() {
    let rig = Rig::new().await;
    let mut h = rig.spawn_host(&allowed_origin());
    let r = h.call("m1", "media.send_to_app", json!({ "url": "https://cdn.example.test/v.mp4", "mime_type": "video/mp4" })).await;
    assert!(matches!(r, Response::Ok { .. }));
    assert_eq!(rig.proposals.recv_timeout(T).unwrap().source, "media");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bad_payloads_get_canonical_errors() {
    let rig = Rig::new().await;
    let mut h = rig.spawn_host(&allowed_origin());
    assert_eq!(err_code(&h.call("e1", "browser.intercept", json!({ "url": "file:///C:/Windows/system32/config/SAM" })).await), ErrorCode::ErrInvalidUrlScheme);
    assert_eq!(err_code(&h.call("e2", "browser.intercept", json!({ "url": "javascript:alert(1)" })).await), ErrorCode::ErrInvalidUrlScheme);
    assert_eq!(err_code(&h.call("e3", "browser.intercept", json!({ "nope": 1 })).await), ErrorCode::ErrPayloadCorrupt);
    assert_eq!(err_code(&h.call("e4", "does.not.exist", json!({})).await), ErrorCode::ErrInvalidArgument);
    assert!(rig.proposals.try_recv().is_err(), "rejected requests never reach the UI");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn confirmed_download_pushes_status_back_until_completed() {
    let srv = TestServer::start(300_000, Mode::Ranges).await;
    srv.set_delay_ms(5);
    let rig = Rig::new().await;
    let mut h = rig.spawn_host(&allowed_origin());
    let r = h.call("s1", "browser.intercept", json!({ "url": srv.url("/file"), "file_name": "pushed.bin" })).await;
    let Response::Ok { data, .. } = r else { panic!("{r:?}") };
    let request_id = data["request_id"].as_str().unwrap().to_string();
    let _ = rig.proposals.recv_timeout(T).unwrap();

    // "User clicks Mulai Mengunduh" in the dialog.
    let dl = rig
        .manager
        .create(NewDownload {
            url: srv.url("/file"),
            file_name: Some("pushed.bin".into()),
            target_path: Some(rig.dir.join("out").join("pushed.bin")),
            connection_count: Some(2),
            start_immediately: true,
            ..Default::default()
        })
        .await
        .unwrap();
    rig.bridge.track(&request_id, &dl.id);

    let (mut saw_result, mut saw_running, mut done) = (false, false, false);
    while !done {
        let v = h.next().await;
        match v["event"].as_str() {
            Some("browser.proposal_result") => {
                saw_result = true;
                assert_eq!(v["payload"]["accepted"], true);
                assert_eq!(v["payload"]["request_id"], request_id.as_str());
            }
            Some("browser.push_status") => {
                assert_eq!(v["payload"]["request_id"], request_id.as_str());
                assert!(v["payload"].get("cookies").is_none());
                match v["payload"]["state"].as_str() {
                    Some("running") => saw_running = true,
                    Some("completed") => done = true,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    assert!(saw_result && saw_running);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn declining_the_dialog_is_reported_to_the_browser() {
    let rig = Rig::new().await;
    let mut h = rig.spawn_host(&allowed_origin());
    let r = h.call("d1", "browser.intercept", json!({ "url": "https://x.example.test/f.bin" })).await;
    let Response::Ok { data, .. } = r else { panic!("{r:?}") };
    let rid = data["request_id"].as_str().unwrap().to_string();
    rig.bridge.reject(&rid);
    let v = h.next().await;
    assert_eq!(v["event"], "browser.proposal_result");
    assert_eq!(v["payload"]["accepted"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_get_never_returns_cookies() {
    let srv = TestServer::start(1000, Mode::Ranges).await;
    let rig = Rig::new().await;
    let dl = rig
        .manager
        .create(NewDownload {
            url: srv.url("/file"),
            target_path: Some(rig.dir.join("o").join("g.bin")),
            cookies: Some("sid=secret".into()),
            start_immediately: false,
            ..Default::default()
        })
        .await
        .unwrap();
    let mut h = rig.spawn_host(&allowed_origin());
    let r = h.call("g1", "download.get", json!({ "id": dl.id })).await;
    let Response::Ok { data, .. } = r else { panic!("{r:?}") };
    assert_eq!(data["id"], dl.id.as_str());
    assert!(!data.to_string().contains("secret"), "{data}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_reports_app_not_running_without_launching_it() {
    let dir = std::env::temp_dir().join(format!("uk-nopipe-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = spawn_host_in(&dir, &allowed_origin());
    // A heartbeat must not start the app; it just reports that it is not there.
    assert_eq!(err_code(&h.call("n1", "ping", json!({})).await), ErrorCode::ErrNativeHostNotFound);
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pipe_client_with_the_wrong_token_is_rejected() {
    let rig = Rig::new().await;
    let pipe = tokio::net::windows::named_pipe::ClientOptions::new().open(pipe_name(&rig.paths)).unwrap();
    let (mut r, mut w) = tokio::io::split(pipe);
    write_frame_async(&mut w, &Request { id: "x".into(), command: "host.hello".into(), payload: json!({ "token": "wrong" }) }).await.unwrap();
    let resp: Response = read_frame_async(&mut r).await.unwrap().unwrap();
    assert_eq!(err_code(&resp), ErrorCode::ErrUnauthorized);
    // ...and the server then hangs up.
    assert!(read_frame_async::<_, Value>(&mut r).await.unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commands_before_hello_are_refused() {
    let rig = Rig::new().await;
    let pipe = tokio::net::windows::named_pipe::ClientOptions::new().open(pipe_name(&rig.paths)).unwrap();
    let (mut r, mut w) = tokio::io::split(pipe);
    write_frame_async(&mut w, &Request { id: "y".into(), command: "browser.intercept".into(), payload: json!({ "url": "https://a.test/f" }) }).await.unwrap();
    let resp: Response = read_frame_async(&mut r).await.unwrap().unwrap();
    assert_eq!(err_code(&resp), ErrorCode::ErrUnauthorized);
    assert!(rig.proposals.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_bridge_on_the_same_pipe_is_refused() {
    let rig = Rig::new().await;
    let (tx, _rx) = channel();
    let err = Bridge::start(&tokio::runtime::Handle::current(), rig.manager.clone(), &rig.paths, tx, Arc::new(|| {})).err().expect("second bridge must fail");
    assert_eq!(err.code, ErrorCode::ErrStorageInitFailed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn excluded_file_types_stay_in_the_browser() {
    let rig = Rig::new().await;
    rig.manager
        .db()
        .write(|c| unduhkilat_core::repo::set_setting(c, unduhkilat_core::config::keys::BROWSER_SKIP_EXTENSIONS, "pdf, jpg"))
        .await
        .unwrap();
    let mut h = rig.spawn_host(&allowed_origin());

    // The extension learns the list from ping so it can skip without asking the app.
    let Response::Ok { data, .. } = h.call("p1", "ping", json!({})).await else { panic!() };
    assert_eq!(data["skip_extensions"], json!(["pdf", "jpg"]));

    // Link without an extension, recognised by MIME (the Tokopedia invoice case).
    let r = h.call("x1", "browser.intercept", json!({ "url": "https://www.tokopedia.com/digital/invoice?id=1", "mime_type": "application/pdf" })).await;
    assert_eq!(err_code(&r), ErrorCode::ErrCaptureSkipped);
    let r = h.call("x2", "browser.intercept", json!({ "url": "https://a.test/get", "file_name": "Foto.JPG" })).await;
    assert_eq!(err_code(&r), ErrorCode::ErrCaptureSkipped);
    assert!(rig.proposals.try_recv().is_err(), "skipped downloads never reach the UI");

    // Other types are still captured, and an explicit "send to app" ignores the list.
    assert!(matches!(h.call("x3", "browser.intercept", json!({ "url": "https://a.test/setup.exe" })).await, Response::Ok { .. }));
    assert!(matches!(h.call("x4", "media.send_to_app", json!({ "url": "https://a.test/a.pdf", "mime_type": "application/pdf" })).await, Response::Ok { .. }));
}
