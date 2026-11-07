//! `unduhkilat-host.exe`: Chromium native messaging host (DEC-006, TASK-37).
//!
//! Browser <-> host: 32-bit little-endian length-prefixed JSON on stdin/stdout.
//! Host <-> app: the same frames over a per-user named pipe, authenticated with a
//! secret from the data directory. The host is a dumb relay: it launches the app
//! only when a download is actually being handed over, so a user who exited the
//! app is never surprised by it coming back.

#[cfg(not(windows))]
fn main() {
    eprintln!("unduhkilat-host is Windows-only (DEC-005)");
}

#[cfg(windows)]
fn main() {
    imp::main();
}

#[cfg(windows)]
mod imp {
    use serde_json::Value;
    use std::io::{stdin, stdout};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tokio::io::{split, WriteHalf};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
    use unduhkilat_core::bridge::{pipe_name, token_path};
    use unduhkilat_core::error::{AppError, ErrorCode};
    use unduhkilat_core::ipc::{read_frame, read_frame_async, write_frame, write_frame_async, Request, Response};
    use unduhkilat_core::native;
    use unduhkilat_core::paths::AppPaths;

    const ERROR_PIPE_BUSY: i32 = 231;

    type Out = Arc<Mutex<std::io::Stdout>>;

    fn send(out: &Out, v: &impl serde::Serialize) {
        let mut g = out.lock().unwrap();
        let _ = write_frame(&mut *g, v);
    }

    fn send_error(out: &Out, id: &str, code: ErrorCode, msg: &str) {
        send(out, &Response::error(id, &AppError::new(code, msg)));
    }

    pub fn main() {
        let out: Out = Arc::new(Mutex::new(stdout()));

        // Chromium passes the caller's origin as the first argument (API.md §1.1).
        let origin = std::env::args().nth(1).unwrap_or_default();
        if origin != native::allowed_origin() {
            send_error(&out, "", ErrorCode::ErrUnauthorized, "caller is not the UnduhKilat extension");
            std::process::exit(2);
        }
        let paths = match AppPaths::from_current_exe() {
            Ok(p) => p,
            Err(e) => {
                send_error(&out, "", ErrorCode::ErrInternal, &e.to_string());
                std::process::exit(1);
            }
        };
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        rt.block_on(run(paths, out));
    }

    /// Requests that express user intent to download; only these may start the app.
    fn is_intent(cmd: &str) -> bool {
        matches!(cmd, "browser.intercept" | "media.send_to_app")
    }

    async fn run(paths: AppPaths, out: Out) {
        let (in_tx, mut in_rx) = tokio::sync::mpsc::channel::<Value>(64);
        std::thread::spawn(move || {
            let mut input = stdin().lock();
            loop {
                match read_frame::<_, Value>(&mut input) {
                    Ok(Some(v)) => {
                        if in_tx.blocking_send(v).is_err() {
                            break;
                        }
                    }
                    _ => break, // browser closed the pipe
                }
            }
        });

        let connected = Arc::new(AtomicBool::new(false));
        let mut writer: Option<WriteHalf<NamedPipeClient>> = None;

        while let Some(msg) = in_rx.recv().await {
            let id = msg.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            let cmd = msg.get("command").and_then(Value::as_str).unwrap_or("").to_string();

            if !connected.load(Ordering::SeqCst) {
                writer = None;
            }
            if writer.is_none() {
                writer = connect(&paths, is_intent(&cmd), &out, &connected).await;
            }
            let Some(w) = writer.as_mut() else {
                send_error(&out, &id, ErrorCode::ErrNativeHostNotFound, "UnduhKilat tidak aktif. Jalankan aplikasi portabel.");
                continue;
            };
            if write_frame_async(w, &msg).await.is_err() {
                connected.store(false, Ordering::SeqCst);
                writer = None;
                send_error(&out, &id, ErrorCode::ErrNativeHostNotFound, "Koneksi ke UnduhKilat terputus.");
            }
        }
    }

    fn launch_app() {
        use std::os::windows::process::CommandExt;
        // Safety valve for automated tests: never start a real GUI from a test run.
        if std::env::var_os("UNDUHKILAT_NO_AUTOLAUNCH").is_some() {
            return;
        }
        let Ok(me) = std::env::current_exe() else { return };
        let app = me.with_file_name("UnduhKilat.exe");
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: the app must outlive the browser.
        let _ = std::process::Command::new(app).creation_flags(0x0000_0008 | 0x0000_0200).spawn();
    }

    async fn open_pipe(name: &str, launch: bool) -> Option<NamedPipeClient> {
        let start = Instant::now();
        let mut launched = false;
        loop {
            match ClientOptions::new().open(name) {
                Ok(c) => return Some(c),
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    if start.elapsed() > Duration::from_secs(3) {
                        return None;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if !launch || start.elapsed() > Duration::from_secs(12) {
                        return None;
                    }
                    if !launched {
                        launch_app();
                        launched = true;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                Err(_) => return None,
            }
        }
    }

    fn read_token(paths: &AppPaths) -> Option<String> {
        std::fs::read_to_string(token_path(&paths.state_dir)).ok().map(|s| s.trim().to_string())
    }

    /// Open the pipe, authenticate, and start forwarding pushed frames to the browser.
    async fn connect(paths: &AppPaths, launch: bool, out: &Out, connected: &Arc<AtomicBool>) -> Option<WriteHalf<NamedPipeClient>> {
        let name = pipe_name(paths);
        // The app writes a fresh token just after creating the pipe; retry briefly so a
        // stale token from a previous run cannot win the race.
        for attempt in 0..4 {
            let pipe = open_pipe(&name, launch).await?;
            let token = read_token(paths).unwrap_or_default();
            let (mut r, mut w) = split(pipe);
            let hello = Request { id: "__hello__".into(), command: "host.hello".into(), payload: serde_json::json!({ "token": token }) };
            if write_frame_async(&mut w, &hello).await.is_err() {
                return None;
            }
            match tokio::time::timeout(Duration::from_secs(3), read_frame_async::<_, Response>(&mut r)).await {
                Ok(Ok(Some(Response::Ok { .. }))) => {
                    connected.store(true, Ordering::SeqCst);
                    let (out, connected) = (out.clone(), connected.clone());
                    tokio::spawn(async move {
                        while let Ok(Some(v)) = read_frame_async::<_, Value>(&mut r).await {
                            send(&out, &v);
                        }
                        connected.store(false, Ordering::SeqCst);
                    });
                    return Some(w);
                }
                Ok(Ok(Some(Response::Error { .. }))) if attempt < 3 => {
                    tokio::time::sleep(Duration::from_millis(300)).await; // token may still be rotating
                }
                _ => return None,
            }
        }
        None
    }
}
