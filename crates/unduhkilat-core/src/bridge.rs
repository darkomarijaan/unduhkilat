//! App-side bridge for the native messaging host (ARCHITECTURE.md §3: "Local Loopback
//! IPC / Pipe"; API.md §1.1 layer 2). Windows only (DEC-005).
//!
//! `unduhkilat-host.exe` is launched by the browser and relays frames to this named
//! pipe. The pipe rejects remote clients and every connection must open with
//! `host.hello` carrying a per-run secret read from the user's data directory.
//! Nothing here starts a download: browser and media requests become *proposals*
//! that the UI shows in the confirmation dialog (FR-039).

use crate::engine::manager::Manager;
use crate::engine::EngineEvent;
use crate::error::{AppError, ErrorCode, Result};
use crate::ipc::{read_frame_async, write_frame_async, Event, Request, Response};
use crate::models::Download;
use crate::paths::AppPaths;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::runtime::Handle;
use tokio::sync::broadcast;

/// A download proposed by the browser, awaiting explicit user confirmation.
#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    pub request_id: String,
    /// `browser` (download interception) or `media` (floating panel).
    pub source: String,
    pub url: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub total_bytes: Option<i64>,
    pub referer: Option<String>,
    pub user_agent: Option<String>,
    pub cookies: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProposalPayload {
    url: String,
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    total_bytes: Option<i64>,
    #[serde(default)]
    referer: Option<String>,
    #[serde(default)]
    user_agent: Option<String>,
    #[serde(default)]
    cookies: Option<String>,
}

/// Max proposals waiting for the user. A runaway page cannot flood the UI.
const MAX_PENDING: usize = 100;
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

pub fn pipe_name(paths: &AppPaths) -> String {
    format!(r"\\.\pipe\unduhkilat-{}", paths.instance_key())
}

pub fn token_path(state_dir: &Path) -> PathBuf {
    state_dir.join("ipc.token")
}

fn new_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

struct Inner {
    manager: Manager,
    token: String,
    proposals: Sender<Proposal>,
    wake: Arc<dyn Fn() + Send + Sync>,
    out: broadcast::Sender<Event>,
    /// download id -> browser request id, for status push-back (FR-038).
    tracked: Mutex<HashMap<String, String>>,
    pending: AtomicUsize,
}

#[derive(Clone)]
pub struct Bridge {
    inner: Arc<Inner>,
}

impl Bridge {
    /// Create the pipe, write a fresh token, and start serving. Fails if another process
    /// already owns the pipe (a second instance).
    pub fn start(
        handle: &Handle,
        manager: Manager,
        paths: &AppPaths,
        proposals: Sender<Proposal>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Bridge> {
        let _guard = handle.enter();
        let name = pipe_name(paths);
        let first = ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create(&name)
            .map_err(|e| AppError::new(ErrorCode::ErrStorageInitFailed, format!("cannot create pipe {name}: {e}")))?;

        let token = new_token();
        std::fs::create_dir_all(&paths.state_dir)?;
        std::fs::write(token_path(&paths.state_dir), &token)?;

        let (out, _) = broadcast::channel(256);
        let inner = Arc::new(Inner { manager: manager.clone(), token, proposals, wake, out, tracked: Mutex::new(HashMap::new()), pending: AtomicUsize::new(0) });
        let bridge = Bridge { inner: inner.clone() };

        // Accept loop.
        let inner2 = inner.clone();
        handle.spawn(async move {
            let mut server = first;
            loop {
                if let Err(e) = server.connect().await {
                    tracing::error!(error = %e, "pipe accept failed");
                    break;
                }
                let connected = server;
                server = match ServerOptions::new().reject_remote_clients(true).create(&name) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!(error = %e, "cannot create next pipe instance");
                        break;
                    }
                };
                tokio::spawn(serve(connected, inner2.clone()));
            }
        });

        // Engine events -> status push for browser-originated downloads.
        let (b2, mut rx) = (bridge.clone(), manager.subscribe());
        handle.spawn(async move {
            while let Ok(ev) = rx.recv().await {
                b2.on_engine_event(ev);
            }
        });
        Ok(bridge)
    }

    /// A proposal was accepted in the UI and became download `download_id`.
    pub fn track(&self, request_id: &str, download_id: &str) {
        self.inner.tracked.lock().unwrap().insert(download_id.to_string(), request_id.to_string());
        self.inner.pending.fetch_sub(1, Ordering::SeqCst);
        let _ = self.inner.out.send(Event::new("browser.proposal_result", json!({ "request_id": request_id, "accepted": true, "download_id": download_id })));
        if let Ok(d) = self.inner.manager.get(download_id) {
            self.push_state(request_id, &d);
        }
    }

    /// The user closed the dialog without starting anything.
    pub fn reject(&self, request_id: &str) {
        self.inner.pending.fetch_sub(1, Ordering::SeqCst);
        let _ = self.inner.out.send(Event::new("browser.proposal_result", json!({ "request_id": request_id, "accepted": false })));
    }

    fn push_state(&self, request_id: &str, d: &Download) {
        let _ = self.inner.out.send(Event::new(
            "browser.push_status",
            json!({
                "request_id": request_id, "download_id": d.id, "state": d.state,
                "error_code": d.error_code, "downloaded_bytes": d.downloaded_bytes, "total_bytes": d.total_bytes,
                "file_name": d.file_name,
            }),
        ));
    }

    fn on_engine_event(&self, ev: EngineEvent) {
        let id = match &ev {
            EngineEvent::Progress { id, .. } | EngineEvent::StateChanged { id, .. } => id.clone(),
        };
        let Some(req) = self.inner.tracked.lock().unwrap().get(&id).cloned() else { return };
        match ev {
            EngineEvent::StateChanged { .. } => {
                if let Ok(d) = self.inner.manager.get(&id) {
                    self.push_state(&req, &d);
                }
            }
            EngineEvent::Progress { downloaded_bytes, total_bytes, current_speed_bps, .. } => {
                let _ = self.inner.out.send(Event::new(
                    "browser.push_status",
                    json!({ "request_id": req, "download_id": id, "state": "running", "downloaded_bytes": downloaded_bytes, "total_bytes": total_bytes, "speed_bps": current_speed_bps }),
                ));
            }
        }
    }
}

async fn serve(stream: NamedPipeServer, inner: Arc<Inner>) {
    let (mut r, mut w) = tokio::io::split(stream);

    // 1. Authenticate: the first frame must be host.hello with the right token.
    let hello = tokio::time::timeout(HELLO_TIMEOUT, read_frame_async::<_, Request>(&mut r)).await;
    let authed = match &hello {
        Ok(Ok(Some(req))) if req.command == "host.hello" => req.payload.get("token").and_then(Value::as_str) == Some(inner.token.as_str()),
        _ => false,
    };
    let hello_id = match &hello {
        Ok(Ok(Some(req))) => req.id.clone(),
        _ => String::new(),
    };
    if !authed {
        let e = AppError::new(ErrorCode::ErrUnauthorized, "missing or wrong host token");
        let _ = write_frame_async(&mut w, &Response::error(hello_id, &e)).await;
        tracing::warn!("rejected a pipe client that failed authentication");
        return;
    }
    if write_frame_async(&mut w, &Response::ok(hello_id, json!({ "app": "UnduhKilat" }))).await.is_err() {
        return;
    }

    // 2. Serve requests and push events.
    let mut events = inner.out.subscribe();
    loop {
        tokio::select! {
            frame = read_frame_async::<_, Request>(&mut r) => {
                let req = match frame {
                    Ok(Some(r)) => r,
                    Ok(None) => break,
                    Err(e) => {
                        let _ = write_frame_async(&mut w, &Response::error("", &e)).await;
                        break;
                    }
                };
                let id = req.id.clone();
                let resp = match dispatch(&inner, req).await {
                    Ok(v) => Response::ok(id, v),
                    Err(e) => Response::error(id, &e),
                };
                if write_frame_async(&mut w, &resp).await.is_err() { break; }
            }
            ev = events.recv() => match ev {
                Ok(ev) => { if write_frame_async(&mut w, &ev).await.is_err() { break; } }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    }
}

fn trimmed(d: &Download) -> Value {
    // Never hand cookies back to the browser side.
    json!({
        "id": d.id, "file_name": d.file_name, "state": d.state, "total_bytes": d.total_bytes,
        "downloaded_bytes": d.downloaded_bytes, "error_code": d.error_code, "target_path": d.target_path,
    })
}

fn skip_list(inner: &Inner) -> Vec<String> {
    let text = inner.manager.db().read(|c| crate::repo::get_setting(c, crate::config::keys::BROWSER_SKIP_EXTENSIONS)).ok().flatten().unwrap_or_default();
    crate::capture_filter::parse_list(&text)
}

async fn dispatch(inner: &Arc<Inner>, req: Request) -> Result<Value> {
    match req.command.as_str() {
        // The extension caches `skip_extensions` so excluded types never reach (or start) the app.
        "ping" => Ok(json!({ "pong": true, "skip_extensions": skip_list(inner) })),
        "browser.intercept" | "media.send_to_app" => {
            let p: ProposalPayload = serde_json::from_value(req.payload).map_err(|e| AppError::new(ErrorCode::ErrPayloadCorrupt, e.to_string()))?;
            let scheme = url::Url::parse(&p.url).map(|u| u.scheme().to_string()).map_err(|_| AppError::new(ErrorCode::ErrInvalidUrlScheme, format!("not a valid URL: {}", p.url)))?;
            if !matches!(scheme.as_str(), "http" | "https" | "ftp") {
                return Err(AppError::new(ErrorCode::ErrInvalidUrlScheme, format!("{scheme} is not supported")));
            }
            // Only automatic capture honours the list; "send to app" from the media panel is explicit.
            if req.command == "browser.intercept" && crate::capture_filter::should_skip(&skip_list(inner), p.file_name.as_deref(), &p.url, p.mime_type.as_deref()) {
                return Err(AppError::new(ErrorCode::ErrCaptureSkipped, "jenis berkas ini dibiarkan diunduh browser"));
            }
            if inner.pending.load(Ordering::SeqCst) >= MAX_PENDING {
                return Err(AppError::internal("too many proposals are waiting for confirmation"));
            }
            let request_id = format!("prop_{}", uuid::Uuid::new_v4().simple());
            let proposal = Proposal {
                request_id: request_id.clone(),
                source: if req.command == "media.send_to_app" { "media" } else { "browser" }.into(),
                url: p.url,
                file_name: p.file_name.filter(|s| !s.trim().is_empty()),
                mime_type: p.mime_type,
                total_bytes: p.total_bytes.filter(|n| *n >= 0),
                referer: p.referer.filter(|s| !s.is_empty()),
                user_agent: p.user_agent.filter(|s| !s.is_empty()),
                cookies: p.cookies.filter(|s| !s.is_empty()),
            };
            inner.pending.fetch_add(1, Ordering::SeqCst);
            if inner.proposals.send(proposal).is_err() {
                inner.pending.fetch_sub(1, Ordering::SeqCst);
                return Err(AppError::internal("the UI is not accepting proposals"));
            }
            (inner.wake)();
            // Nothing has started: the user still has to confirm (FR-039).
            Ok(json!({ "request_id": request_id, "state": "pending_confirmation" }))
        }
        "app.open_link" => {
            // A magnet/link clicked in any program (OS protocol handler); still only a proposal.
            let url = req.payload.get("url").and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
            let scheme = url::Url::parse(&url).map(|u| u.scheme().to_string()).map_err(|_| AppError::new(ErrorCode::ErrInvalidUrlScheme, "not a valid link"))?;
            if !matches!(scheme.as_str(), "http" | "https" | "ftp" | "magnet") {
                return Err(AppError::new(ErrorCode::ErrInvalidUrlScheme, format!("{scheme} is not supported")));
            }
            if inner.pending.load(Ordering::SeqCst) >= MAX_PENDING {
                return Err(AppError::internal("too many proposals are waiting for confirmation"));
            }
            let request_id = format!("prop_{}", uuid::Uuid::new_v4().simple());
            let proposal = Proposal { request_id: request_id.clone(), source: "link".into(), url, file_name: None, mime_type: None, total_bytes: None, referer: None, user_agent: None, cookies: None };
            inner.pending.fetch_add(1, Ordering::SeqCst);
            if inner.proposals.send(proposal).is_err() {
                inner.pending.fetch_sub(1, Ordering::SeqCst);
                return Err(AppError::internal("the UI is not accepting proposals"));
            }
            (inner.wake)();
            Ok(json!({ "request_id": request_id, "state": "pending_confirmation" }))
        }
        "download.get" => {
            let id = req.payload.get("id").and_then(Value::as_str).ok_or_else(|| AppError::invalid("id is required"))?;
            Ok(trimmed(&inner.manager.get(id)?))
        }
        "download.list" => {
            let limit = req.payload.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
            let all = inner.manager.db().read(|c| crate::repo::list_downloads(c, None))?;
            Ok(json!({ "items": all.iter().take(limit.max(1)).map(trimmed).collect::<Vec<_>>() }))
        }
        other => Err(AppError::invalid(format!("unknown command {other}"))),
    }
}
