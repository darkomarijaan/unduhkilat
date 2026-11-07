//! Main window (docs/06_DESIGN.md §5, §6, §11, §17).

use crate::format;
use crate::theme::{self, *};
use crate::tray::{OpenSettings, Tray};
use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Stroke, Ui, Vec2};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};
use tokio::runtime::Handle;
use unduhkilat_core::config::{keys, EngineConfig};
use unduhkilat_core::engine::ftp;
use unduhkilat_core::engine::grabber::ScanResult;
use unduhkilat_core::engine::search::{Provider, SearchOutcome, Source};
use unduhkilat_core::engine::http::{self, RangeSupport};
use unduhkilat_core::engine::manager::{Manager, NewDownload};
use unduhkilat_core::engine::proxy::{ProxyConfig, ProxyMode};
use unduhkilat_core::engine::torrent::{self, TorrentInfo};
use unduhkilat_core::engine::EngineEvent;
use unduhkilat_core::logging::LogTail;
use unduhkilat_core::models::{Category, Download, DownloadState, Segment};
use unduhkilat_core::bridge::{Bridge, Proposal};
use unduhkilat_core::native;
use unduhkilat_core::paths::AppPaths;
use unduhkilat_core::update::{self, UpdateInfo};
use unduhkilat_core::{category, repo};

const DOCK_HEIGHT: f32 = 200.0;
/// Fixed height of one package card (DESIGN.md).
const CARD_H: f32 = 92.0;

/// PROVISIONAL priority scale (FR-018 leaves the scale undecided): Tinggi/Normal/Rendah.
const PRIORITIES: [(&str, i32); 3] = [("Tinggi", 1), ("Normal", 0), ("Rendah", -1)];

fn priority_label(p: i32) -> String {
    PRIORITIES.iter().find(|(_, v)| *v == p).map(|(n, _)| n.to_string()).unwrap_or_else(|| p.to_string())
}

#[derive(Clone, PartialEq)]
enum Filter {
    All,
    Active,
    Done,
    Paused,
    Category(String),
}

#[derive(Clone, Copy, PartialEq)]
enum SortKey {
    None,
    Name,
    Size,
    Status,
    Progress,
    Speed,
    Priority,
}

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Downloads,
    Settings,
    Diagnostics,
    Grabber,
    Search,
}

#[derive(Clone, Copy, PartialEq)]
enum SettingsTab {
    Engine,
    Categories,
    Speed,
    Behaviour,
    Browser,
    Network,
    Torrent,
}

struct AddDialog {
    urls: String,
    file_name: String,
    category: Option<String>,
    destination: String,
    connections: u32,
    priority: i32,
    referer: String,
    cookies: String,
    /// Captured from the browser; sent verbatim with every request (FR-017).
    user_agent: String,
    probe: String,
    err: Option<String>,
    /// Set when the dialog was opened by the browser extension (FR-036/039).
    origin: Option<String>,
    /// A `.torrent` chosen from disk (DEC-008); used instead of the URL list.
    torrent_file: Option<(String, Vec<u8>)>,
}

/// File search screen state (DEC-009).
#[derive(Default)]
struct SearchState {
    query: String,
    providers: Vec<(Provider, bool)>,
    busy: bool,
    result: Option<SearchOutcome>,
    checked: Vec<bool>,
    err: Option<String>,
}

/// Site Grabber screen state (FR-045).
#[derive(Default)]
struct GrabState {
    url: String,
    pattern: String,
    scanning: bool,
    result: Option<ScanResult>,
    checked: Vec<bool>,
    filter: String,
    err: Option<String>,
    note: Option<String>,
}

struct SettingsState {
    proxy_mode: ProxyMode,
    proxy_host: String,
    proxy_port: u32,
    proxy_user: String,
    proxy_pass: String,
    proxy_has_password: bool,
    net_note: Option<String>,
    cred_host: String,
    cred_port: u32,
    cred_protocol: usize,
    cred_user: String,
    cred_pass: String,
    browser: Vec<(native::Browser, native::Status)>,
    browser_note: Option<String>,
    tab: SettingsTab,
    concurrency: u32,
    connections: u32,
    global_kbps: u32,
    close_to_tray: bool,
    toast: bool,
    cats: Vec<Category>,
    new_cat: (String, String, String),
    clip: bool,
    upd: bool,
    t_seeding: bool,
    t_ratio: f32,
    t_dht: bool,
    t_upnp: bool,
    t_port: u32,
    t_net_orig: (bool, bool, u32),
}

enum Msg {
    Notice(String),
    Probe(String, Option<String>),
    Torrent(String, Option<TorrentInfo>),
    Scan(std::result::Result<ScanResult, String>),
    Search(std::result::Result<SearchOutcome, String>),
    /// Result of an update check; the flag says the user asked for it (so "up to date" is reported).
    UpdateFound(std::result::Result<Option<UpdateInfo>, String>, bool),
    /// The update was downloaded, verified and unpacked (or why not).
    UpdateStaged(std::result::Result<(), String>),
}

#[derive(Clone)]
enum UpdState {
    None,
    Available(UpdateInfo),
    Working(String),
}

pub struct App {
    rt: Handle,
    manager: Manager,
    paths: AppPaths,
    log_tail: LogTail,
    ctx: egui::Context,
    tray: Option<Tray>,
    open_settings: Option<OpenSettings>,
    bridge: Option<Bridge>,
    proposals_rx: Option<Receiver<Proposal>>,
    pending: VecDeque<Proposal>,

    downloads: Vec<Download>,
    categories: Vec<Category>,
    segments: Vec<Segment>,
    last_refresh: Instant,
    live: HashMap<String, (u64, u32)>,
    torrent: Option<(String, TorrentInfo)>,
    torrent_poll: Instant,
    grab: GrabState,
    search: SearchState,
    clip_rx: Receiver<String>,
    clip_enabled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    events: tokio::sync::broadcast::Receiver<EngineEvent>,

    selected: Option<String>,
    checked: HashSet<String>,
    filter: Filter,
    sort: (SortKey, bool),
    screen: Screen,
    dock_open: bool,
    dock_height: f32,
    add: Option<AddDialog>,
    settings: Option<SettingsState>,
    log_level: usize,
    close_to_tray: bool,
    sized: bool,

    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    notice: Option<(String, Instant)>,
    update: UpdState,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(ctx: egui::Context, rt: Handle, manager: Manager, paths: AppPaths, log_tail: LogTail, tray: Option<Tray>, bridge: Option<Bridge>, proposals_rx: Option<Receiver<Proposal>>) -> Self {
        theme::install(&ctx);
        let (tx, rx) = channel();
        let events = manager.subscribe();
        // FR-040: opt-in clipboard watcher; it only ever proposes a URL for confirmation.
        let clip_on = manager.db().read(|c| repo::get_setting(c, keys::CLIPBOARD_MONITOR)).ok().flatten().is_some_and(|v| v == "1");
        let clip_enabled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(clip_on));
        let (clip_tx, clip_rx) = channel();
        let wake_ctx = ctx.clone();
        #[cfg(windows)]
        unduhkilat_core::clipboard::start(clip_tx, clip_enabled.clone(), std::sync::Arc::new(move || crate::tray::show_window(&wake_ctx)));
        #[cfg(not(windows))]
        let _ = (clip_tx, wake_ctx);
        let open_settings = tray.as_ref().map(|t| t.open_settings.clone());
        let mut app = App {
            rt, manager, paths, log_tail, ctx, tray, open_settings, bridge, proposals_rx, pending: VecDeque::new(),
            downloads: Vec::new(), categories: Vec::new(), segments: Vec::new(),
            last_refresh: Instant::now() - Duration::from_secs(10), live: HashMap::new(), torrent: None, torrent_poll: Instant::now() - Duration::from_secs(10), grab: GrabState::default(), search: SearchState::default(), clip_rx, clip_enabled, events,
            selected: None, checked: HashSet::new(), filter: Filter::All, sort: (SortKey::None, true),
            screen: Screen::Downloads, dock_open: false, dock_height: DOCK_HEIGHT, add: None, settings: None,
            log_level: 0, close_to_tray: true, sized: false, tx, rx, notice: None, update: UpdState::None,
        };
        app.refresh();
        // FR-060: look for a newer signed release (setting is on by default; tests/demos can skip it).
        let check_on = app.manager.db().read(|c| repo::get_setting(c, keys::UPDATE_CHECK)).ok().flatten().map(|v| v != "0").unwrap_or(true);
        if check_on && std::env::var_os("UNDUHKILAT_NO_UPDATE_CHECK").is_none() {
            app.check_updates(false);
        }
        // Test aid: open straight on a screen (settings, search, grabber, diagnostics, add).
        match std::env::var("UNDUHKILAT_START_SCREEN").ok().as_deref() {
            Some("settings") => app.open_settings_screen(),
            Some("search") => app.screen = Screen::Search,
            Some("grabber") => app.screen = Screen::Grabber,
            Some("diagnostics") => app.screen = Screen::Diagnostics,
            Some("add") => app.open_add_dialog("https://releases.ubuntu.com/24.04.1/ubuntu-24.04.1-desktop-amd64.iso"),
            _ => {}
        }
        app
    }

    pub fn open_add_dialog(&mut self, url: &str) {
        let connections = self.manager.db().read(|c| repo::get_setting(c, keys::DEFAULT_CONNECTIONS)).ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(EngineConfig::default().default_connections);
        self.add = Some(AddDialog {
            urls: url.to_string(), file_name: String::new(), category: None, destination: String::new(), connections,
            priority: 0, referer: String::new(), cookies: String::new(), user_agent: String::new(), probe: String::new(), err: None,
            origin: None, torrent_file: None,
        });
    }

    /// Ask GitHub for a newer signed release (uses the app client, so the proxy applies).
    fn check_updates(&mut self, manual: bool) {
        let (tx, ctx, client) = (self.tx.clone(), self.ctx.clone(), self.manager.client());
        self.rt.spawn(async move {
            let r = update::check(&client, env!("CARGO_PKG_VERSION")).await.map_err(|e| e.message);
            let _ = tx.send(Msg::UpdateFound(r, manual));
            ctx.request_repaint();
        });
    }

    /// The user clicked "Perbarui": download, verify the signature, unpack. Nothing is replaced yet.
    fn start_update(&mut self, info: UpdateInfo) {
        self.update = UpdState::Working(info.version.clone());
        let (tx, ctx, client) = (self.tx.clone(), self.ctx.clone(), self.manager.client());
        let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
        self.rt.spawn(async move {
            let r = async {
                let dir = dir.ok_or_else(|| unduhkilat_core::AppError::internal("cannot locate the app folder"))?;
                let zip = update::download(&client, &info).await?;
                update::stage(&zip, &update::staging_dir(&dir))
            }
            .await
            .map_err(|e| format!("{}: {}", e.code, e.message));
            let _ = tx.send(Msg::UpdateStaged(r));
            ctx.request_repaint();
        });
    }

    /// Hand over to the updater helper and exit. Running downloads are paused first (resume on next start).
    fn apply_update(&mut self) {
        let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) else { return };
        if let Err(e) = update::spawn_apply(&update::staging_dir(&dir), &dir) {
            self.update = UpdState::None;
            self.notice = Some((format!("{}: {}", e.code, e.message), Instant::now()));
            return;
        }
        let _ = self.rt.block_on(self.manager.shutdown(Duration::from_secs(15)));
        std::process::exit(0);
    }

    /// FR-039: a download proposed by the browser. Nothing starts until the user confirms.
    fn open_proposal(&mut self, p: Proposal) {
        self.open_add_dialog(&p.url);
        if let Some(a) = self.add.as_mut() {
            a.file_name = p.file_name.clone().unwrap_or_default();
            a.referer = p.referer.clone().unwrap_or_default();
            a.cookies = p.cookies.clone().unwrap_or_default();
            a.user_agent = p.user_agent.clone().unwrap_or_default();
            if let Some(n) = p.total_bytes {
                a.probe = format!("{}  |  dari browser", crate::format::bytes(n));
            }
            a.origin = Some(p.request_id);
        }
    }

    /// Live swarm numbers for the selected torrent, fetched about once a second.
    fn poll_torrent(&mut self) {
        let Some(d) = self.selected.as_ref().and_then(|id| self.downloads.iter().find(|d| &d.id == id)) else {
            self.torrent = None;
            return;
        };
        if !torrent::is_magnet(&d.original_url) {
            self.torrent = None;
            return;
        }
        if self.torrent_poll.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.torrent_poll = Instant::now();
        let (m, tx, ctx, id) = (self.manager.clone(), self.tx.clone(), self.ctx.clone(), d.id.clone());
        self.rt.spawn(async move {
            let info = m.torrent_info(&id).await.ok().flatten();
            let _ = tx.send(Msg::Torrent(id, info));
            ctx.request_repaint();
        });
    }

    fn refresh(&mut self) {
        let db = self.manager.db().clone();
        if let Ok(d) = db.read(|c| repo::list_downloads(c, None)) {
            self.downloads = d;
        }
        if let Ok(c) = db.read(repo::list_categories) {
            self.categories = c;
        }
        // Show the segment map of something by default instead of an empty strip.
        if self.selected.as_ref().is_none_or(|id| !self.downloads.iter().any(|d| &d.id == id)) {
            self.selected = self.downloads.first().map(|d| d.id.clone());
        }
        self.close_to_tray = db.read(|c| repo::get_setting(c, keys::CLOSE_TO_TRAY)).ok().flatten().map(|v| v != "0").unwrap_or(true);
        self.segments = match &self.selected {
            Some(id) => db.read(|c| repo::get_segments(c, id)).unwrap_or_default(),
            None => Vec::new(),
        };
        self.last_refresh = Instant::now();
    }

    /// Run an engine command in the background; failures surface in the status strip.
    fn run<F>(&self, f: F)
    where
        F: std::future::Future<Output = unduhkilat_core::Result<()>> + Send + 'static,
    {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.rt.spawn(async move {
            if let Err(e) = f.await {
                let _ = tx.send(Msg::Notice(format!("{}: {}", e.code, e.message)));
            }
            ctx.request_repaint();
        });
    }

    fn drain(&mut self) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                EngineEvent::Progress { id, current_speed_bps, active_connections, .. } => {
                    self.live.insert(id, (current_speed_bps, active_connections));
                }
                EngineEvent::StateChanged { id, state, .. } => {
                    if state != DownloadState::Running {
                        self.live.remove(&id);
                    }
                    self.last_refresh = Instant::now() - Duration::from_secs(10);
                }
            }
        }
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Notice(s) => self.notice = Some((s, Instant::now())),
                Msg::UpdateFound(r, manual) => match r {
                    Ok(Some(info)) => {
                        if !matches!(self.update, UpdState::Working(_)) {
                            self.update = UpdState::Available(info);
                        }
                    }
                    Ok(None) if manual => self.notice = Some((format!("UnduhKilat {} sudah yang terbaru.", env!("CARGO_PKG_VERSION")), Instant::now())),
                    Ok(None) => {}
                    Err(e) if manual => self.notice = Some((e, Instant::now())),
                    Err(e) => tracing::info!(error = %e, "update check failed"),
                },
                Msg::UpdateStaged(r) => match r {
                    Ok(()) => self.apply_update(),
                    Err(e) => {
                        self.update = UpdState::None;
                        self.notice = Some((e, Instant::now()));
                    }
                },
                Msg::Search(r) => {
                    self.search.busy = false;
                    match r {
                        Ok(res) => {
                            self.search.checked = vec![false; res.hits.len()];
                            self.search.err = None;
                            self.search.result = Some(res);
                        }
                        Err(e) => {
                            self.search.result = None;
                            self.search.err = Some(e);
                        }
                    }
                }
                Msg::Scan(r) => {
                    self.grab.scanning = false;
                    match r {
                        Ok(res) => {
                            self.grab.checked = vec![true; res.candidates.len()];
                            self.grab.err = None;
                            self.grab.note = res.candidates.is_empty().then(|| "Tidak Ditemukan Berkas yang Cocok dengan Pola URL.".to_string());
                            self.grab.result = Some(res);
                        }
                        Err(e) => {
                            self.grab.result = None;
                            self.grab.err = Some(format!("Pemindaian dihentikan: {e}"));
                        }
                    }
                }
                Msg::Torrent(id, info) => {
                    if self.selected.as_deref() == Some(&id) {
                        self.torrent = info.map(|i| (id, i));
                    }
                }
                Msg::Probe(text, name) => {
                    if let Some(a) = self.add.as_mut() {
                        a.probe = text;
                        if a.file_name.is_empty() {
                            if let Some(n) = name {
                                a.file_name = n;
                            }
                        }
                    }
                }
            }
        }
    }

    fn speed_of(&self, id: &str) -> u64 {
        self.live.get(id).map(|l| l.0).unwrap_or(0)
    }

    fn total_speed(&self) -> u64 {
        self.live.values().map(|l| l.0).sum()
    }

    fn visible(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.downloads.len())
            .filter(|&i| {
                let d = &self.downloads[i];
                match &self.filter {
                    Filter::All => true,
                    Filter::Active => matches!(d.state, DownloadState::Running | DownloadState::Queued),
                    Filter::Done => d.state == DownloadState::Completed,
                    Filter::Paused => matches!(d.state, DownloadState::Paused | DownloadState::Failed),
                    Filter::Category(id) => d.category_id.as_deref() == Some(id),
                }
            })
            .collect();
        let (key, asc) = self.sort;
        if key != SortKey::None {
            let prio = |d: &Download| self.manager.db().read(|c| repo::get_queue_item(c, &d.id)).map(|q| q.priority).unwrap_or(0);
            v.sort_by(|&a, &b| {
                let (x, y) = (&self.downloads[a], &self.downloads[b]);
                let o = match key {
                    SortKey::Name => x.file_name.to_lowercase().cmp(&y.file_name.to_lowercase()),
                    SortKey::Size => x.total_bytes.cmp(&y.total_bytes),
                    SortKey::Status => x.state.as_str().cmp(y.state.as_str()),
                    SortKey::Progress => format::percent(x.downloaded_bytes, x.total_bytes).total_cmp(&format::percent(y.downloaded_bytes, y.total_bytes)),
                    SortKey::Speed => self.speed_of(&x.id).cmp(&self.speed_of(&y.id)),
                    SortKey::Priority => prio(x).cmp(&prio(y)),
                    SortKey::None => std::cmp::Ordering::Equal,
                };
                if asc { o } else { o.reverse() }
            });
        }
        v
    }

    fn count(&self, f: impl Fn(&Download) -> bool) -> usize {
        self.downloads.iter().filter(|d| f(d)).count()
    }
}

/// Path of the host binary that ships beside this executable.
fn host_exe() -> std::path::PathBuf {
    native::host_exe_next_to(&std::env::current_exe().unwrap_or_default())
}

pub fn browser_status(paths: &AppPaths) -> Vec<(native::Browser, native::Status)> {
    let manifest = paths.data_dir.join(format!("{}.json", native::HOST_NAME));
    native::status(&native::Target::default(), &manifest, &host_exe())
}

/// Idempotent per-user registration; repairs stale entries when the folder moved (FR-034).
pub fn register_host(paths: &AppPaths) -> unduhkilat_core::Result<Vec<(native::Browser, native::Status)>> {
    native::ensure_registered(&native::Target::default(), &paths.data_dir, &host_exe())
}

#[cfg(windows)]
fn primary_screen_px() -> Option<(f32, f32)> {
    unsafe extern "system" {
        fn GetSystemMetrics(index: i32) -> i32;
    }
    // SM_CXSCREEN = 0, SM_CYSCREEN = 1 (physical pixels for a DPI-aware process).
    let (w, h) = unsafe { (GetSystemMetrics(0), GetSystemMetrics(1)) };
    (w > 0 && h > 0).then_some((w as f32, h as f32))
}

#[cfg(not(windows))]
fn primary_screen_px() -> Option<(f32, f32)> {
    None
}

#[derive(Clone, Copy, PartialEq)]
enum Glyph {
    Bolt,
    Pause,
    Clock,
    Cross,
    Check,
}

fn paint_glyph(p: &egui::Painter, c: egui::Pos2, g: Glyph, col: Color32, r: f32) {
    let st = Stroke::new(2.0, col);
    match g {
        Glyph::Bolt => theme::paint_bolt(p, c, r * 2.0, col),
        Glyph::Pause => {
            p.line_segment([c + Vec2::new(-r * 0.35, -r * 0.5), c + Vec2::new(-r * 0.35, r * 0.5)], st);
            p.line_segment([c + Vec2::new(r * 0.35, -r * 0.5), c + Vec2::new(r * 0.35, r * 0.5)], st);
        }
        Glyph::Clock => {
            p.circle_stroke(c, r * 0.62, st);
            p.line_segment([c, c + Vec2::new(0.0, -r * 0.4)], st);
            p.line_segment([c, c + Vec2::new(r * 0.3, r * 0.15)], st);
        }
        Glyph::Cross => {
            p.line_segment([c + Vec2::new(-r * 0.4, -r * 0.4), c + Vec2::new(r * 0.4, r * 0.4)], st);
            p.line_segment([c + Vec2::new(r * 0.4, -r * 0.4), c + Vec2::new(-r * 0.4, r * 0.4)], st);
        }
        Glyph::Check => {
            p.add(egui::Shape::line(vec![c + Vec2::new(-r * 0.45, 0.0), c + Vec2::new(-r * 0.1, r * 0.35), c + Vec2::new(r * 0.5, -r * 0.35)], st));
        }
    }
}

/// Which of the five tracker steps (Antre, Terhubung, Mengunduh, Verifikasi, Selesai) a download is on.
fn step_of(d: &Download) -> usize {
    match d.state {
        DownloadState::Queued | DownloadState::Cancelled => 0,
        DownloadState::Running => if d.downloaded_bytes == 0 && d.total_bytes <= 0 { 1 } else { 2 },
        DownloadState::Paused => 2,
        DownloadState::Failed => if d.downloaded_bytes == 0 { 1 } else { 2 },
        DownloadState::Completed => 4,
    }
}

fn ext_tag(d: &Download) -> String {
    if torrent::is_magnet(&d.original_url) {
        return "BT".into();
    }
    match d.file_name.rsplit_once('.') {
        Some((_, e)) if !e.is_empty() && e.len() <= 4 => e.to_uppercase(),
        _ => "FILE".into(),
    }
}

fn host_of(u: &str) -> String {
    let r = u.split("://").nth(1).unwrap_or(u);
    let r = r.split(['/', '?', '#']).next().unwrap_or(r);
    r.rsplit('@').next().unwrap_or(r).to_string()
}

/// Waybill-style id, stable per download: UK-7F3A-0192.
fn waybill(id: &str) -> String {
    format!("UK-{}-{}", id.get(..4).unwrap_or("0000").to_uppercase(), id.get(24..28).unwrap_or("0000").to_uppercase())
}

/// Shorten `s` with an ellipsis so it fits `max` px.
fn fit_text(p: &egui::Painter, s: &str, font: egui::FontId, max: f32) -> String {
    if p.layout_no_wrap(s.to_string(), font.clone(), FG).size().x <= max {
        return s.to_string();
    }
    let mut t: Vec<char> = s.chars().collect();
    while !t.is_empty() {
        t.pop();
        let c = format!("{}\u{2026}", t.iter().collect::<String>());
        if p.layout_no_wrap(c.clone(), font.clone(), FG).size().x <= max {
            return c;
        }
    }
    "\u{2026}".into()
}

/// Result stamp ("SAMPAI", "TERTAHAN"): boxed caps, tilted a few degrees like a postmark.
fn stamp(p: &egui::Painter, right_x: f32, y: f32, text: &str, col: Color32) {
    let g = p.layout_no_wrap(text.to_string(), theme::bold(11.0), col);
    let (padx, pady) = (7.0, 2.0);
    let (w, h) = (g.size().x + 2.0 * padx, g.size().y + 2.0 * pady);
    let (sn, cs) = (-0.05f32).sin_cos();
    let o = egui::pos2(right_x - w, y);
    let rot = |dx: f32, dy: f32| egui::pos2(o.x + dx * cs - dy * sn, o.y + dx * sn + dy * cs);
    p.add(egui::Shape::closed_line(vec![rot(0.0, 0.0), rot(w, 0.0), rot(w, h), rot(0.0, h)], Stroke::new(1.5, col)));
    p.add(egui::epaint::TextShape::new(rot(padx, pady), g, col).with_angle(-0.05));
}

/// One package: waybill block, five-step tracker, speed/ETA. Returns the click response
/// and any action chosen with the buttons that appear on the selected card.
#[allow(clippy::too_many_arguments)]
fn package_card(ui: &mut Ui, d: &Download, cat_name: &str, speed: u64, conns: u32, sel: bool, compact: bool) -> (egui::Response, Vec<Action>) {
    let w = (ui.available_width() - 6.0).max(300.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, CARD_H), Sense::click());
    let mut acts: Vec<(String, Action)> = Vec::new();
    let p = ui.painter().clone();
    if sel {
        p.rect_filled(rect.translate(Vec2::new(3.0, 3.0)), theme::RADIUS as f32, FG);
    }
    p.rect_filled(rect, theme::RADIUS as f32, SURFACE);
    p.rect_stroke(rect, theme::RADIUS as f32, Stroke::new(if sel { 1.5 } else { 1.0 }, if sel { FG } else { BORDER }), egui::StrokeKind::Inside);

    let left_w = if compact { 236.0 } else { 296.0 };
    let right_w = if compact { 150.0 } else { 184.0 };
    // Tear-off line between the waybill block and the tracker.
    let x_cut = rect.left() + left_w;
    let mut y = rect.top() + 10.0;
    while y < rect.bottom() - 10.0 {
        p.line_segment([egui::pos2(x_cut, y), egui::pos2(x_cut, (y + 4.0).min(rect.bottom() - 10.0))], Stroke::new(1.5, BORDER));
        y += 8.0;
    }

    // -- left: file tile + names
    let tile = egui::Rect::from_min_size(egui::pos2(rect.left() + 16.0, rect.center().y - 20.0), Vec2::splat(40.0));
    let cc = theme::category_color(cat_name);
    p.rect_filled(tile, 8.0, Color32::from_rgba_unmultiplied(cc.r(), cc.g(), cc.b(), 40));
    p.rect_stroke(tile, 8.0, Stroke::new(1.5, FG), egui::StrokeKind::Inside);
    p.text(tile.center(), egui::Align2::CENTER_CENTER, ext_tag(d), theme::mono(11.0), FG);
    let tx = tile.right() + 12.0;
    let tw = (x_cut - 12.0 - tx).max(40.0);
    p.text(egui::pos2(tx, rect.top() + 24.0), egui::Align2::LEFT_CENTER, fit_text(&p, &d.file_name, theme::bold(14.0), tw), theme::bold(14.0), FG);
    p.text(egui::pos2(tx, rect.top() + 44.0), egui::Align2::LEFT_CENTER, waybill(&d.id), theme::mono(11.5), MUTED);
    let proto = if torrent::is_magnet(&d.original_url) { "torrent".to_string() } else { d.original_url.split("://").next().unwrap_or("").to_uppercase() };
    let src = if torrent::is_magnet(&d.original_url) { "magnet · torrent".to_string() } else { format!("{} · {proto}", host_of(&d.effective_url)) };
    p.text(egui::pos2(tx, rect.top() + 63.0), egui::Align2::LEFT_CENTER, fit_text(&p, &src, egui::FontId::proportional(12.0), tw), egui::FontId::proportional(12.0), FG_SECONDARY);

    // -- middle: five-step tracker
    let k = step_of(d);
    let pct = format::percent(d.downloaded_bytes, d.total_bytes);
    let x0 = x_cut + 44.0;
    let x1 = rect.right() - right_w - 44.0;
    let xs: Vec<f32> = (0..5).map(|i| x0 + (x1 - x0) * i as f32 / 4.0).collect();
    let cy = rect.top() + 36.0;
    p.line_segment([egui::pos2(xs[0], cy), egui::pos2(xs[4], cy)], Stroke::new(2.5, theme::TRACK_OFF));
    let done_all = d.state == DownloadState::Completed;
    let reach = if done_all { xs[4] } else {
        let part = if d.state == DownloadState::Running && d.total_bytes > 0 { pct } else { 0.0 };
        xs[k] + if k < 4 { (xs[k + 1] - xs[k]) * part * 0.9 } else { 0.0 }
    };
    p.line_segment([egui::pos2(xs[0], cy), egui::pos2(reach, cy)], Stroke::new(2.5, FG));
    let names = ["Antre", "Terhubung", "Mengunduh", "Verifikasi", "Selesai"];
    for i in 0..5 {
        let c = egui::pos2(xs[i], cy);
        let is_cur = if done_all { i == 4 } else { i == k };
        let passed = if done_all { i < 4 } else { i < k };
        if passed {
            p.circle_filled(c, 9.0, FG);
            paint_glyph(&p, c, Glyph::Check, Color32::WHITE, 8.0);
        } else if is_cur {
            let (fill, glyph, gcol) = match d.state {
                DownloadState::Running => (ACCENT, Glyph::Bolt, FG),
                DownloadState::Paused => (ACCENT, Glyph::Pause, FG),
                DownloadState::Queued => (ACCENT, Glyph::Clock, FG),
                DownloadState::Failed => (DANGER, Glyph::Cross, Color32::WHITE),
                DownloadState::Cancelled => (SURFACE_RAISED, Glyph::Cross, FG_SECONDARY),
                DownloadState::Completed => (ACCENT, Glyph::Check, FG),
            };
            p.circle(c, 13.0, fill, Stroke::new(2.0, FG));
            paint_glyph(&p, c, glyph, gcol, if glyph == Glyph::Bolt { 7.5 } else { 8.0 });
        } else {
            p.circle(c, 8.0, SURFACE, Stroke::new(2.0, theme::TRACK_OFF));
        }
        let (label, font, col) = if is_cur {
            let l = match d.state {
                DownloadState::Running if d.total_bytes > 0 => format!("{:.0}%", pct * 100.0),
                DownloadState::Paused => "Terjeda".to_string(),
                DownloadState::Failed => "Gagal".to_string(),
                DownloadState::Cancelled => "Dibatalkan".to_string(),
                _ => names[i].to_string(),
            };
            (l, theme::bold(12.0), if d.state == DownloadState::Failed { DANGER } else { FG })
        } else {
            (names[i].to_string(), egui::FontId::proportional(11.5), FG_SECONDARY)
        };
        if !compact || is_cur {
            p.text(egui::pos2(xs[i], cy + 26.0), egui::Align2::CENTER_CENTER, label, font, col);
        }
    }

    // -- right: speed / size, caption, stamp
    let rx = rect.right() - 16.0;
    let shift = if sel { 20.0 } else { 0.0 };
    let remaining = d.total_bytes - d.downloaded_bytes;
    let (main, sub) = match d.state {
        DownloadState::Running => {
            let e = format::eta(remaining, speed);
            let tail = if torrent::is_magnet(&d.original_url) { format!("{conns} peer") } else { format!("{conns} koneksi") };
            (format::speed(speed), if e == "-" { tail } else { format!("{e} lagi · {tail}") })
        }
        DownloadState::Queued => (if d.total_bytes > 0 { format::bytes(d.total_bytes) } else { "\u{2014}".to_string() }, "menunggu giliran".to_string()),
        DownloadState::Paused => (format::bytes(d.total_bytes), format!("{} tersimpan", format::bytes(d.downloaded_bytes))),
        DownloadState::Failed => (format::bytes(d.total_bytes), d.error_code.clone().unwrap_or_default().trim_start_matches("ERR_").to_lowercase().replace('_', " ")),
        DownloadState::Completed => (format::bytes(d.total_bytes), "tersimpan".to_string()),
        DownloadState::Cancelled => (format::bytes(d.total_bytes), "dibatalkan".to_string()),
    };
    p.text(egui::pos2(rx, rect.top() + 30.0 + shift), egui::Align2::RIGHT_CENTER, main, theme::bold(17.0), FG);
    p.text(egui::pos2(rx, rect.top() + 49.0 + shift), egui::Align2::RIGHT_CENTER, fit_text(&p, &sub, egui::FontId::proportional(12.0), right_w - 8.0), egui::FontId::proportional(12.0), FG_SECONDARY);
    if !sel {
        match d.state {
            DownloadState::Completed => stamp(&p, rx, rect.top() + 62.0, "SAMPAI", SUCCESS),
            DownloadState::Failed => stamp(&p, rx, rect.top() + 62.0, "TERTAHAN", DANGER),
            _ => {}
        }
    }

    // -- actions on the selected card (also reachable from the context menu)
    if sel {
        let area = egui::Rect::from_min_max(egui::pos2(rect.right() - 196.0, rect.top() + 7.0), egui::pos2(rect.right() - 10.0, rect.top() + 33.0));
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(Layout::right_to_left(Align::Center)));
        c.spacing_mut().item_spacing.x = 4.0;
        let small = |t: &str| egui::Button::new(RichText::new(t.to_string()).size(12.0)).min_size(Vec2::new(0.0, 24.0));
        if c.add(small("Hapus")).on_hover_text("Hapus dari daftar (berkas tetap ada)").clicked() { acts.push((d.id.clone(), Action::Delete(false))); }
        if c.add(small("Folder")).clicked() { acts.push((d.id.clone(), Action::Reveal)); }
        match d.state {
            DownloadState::Running | DownloadState::Queued => { if c.add(small("Jeda")).clicked() { acts.push((d.id.clone(), Action::Pause)); } }
            DownloadState::Paused | DownloadState::Failed => { if c.add(small("Lanjutkan")).clicked() { acts.push((d.id.clone(), Action::Resume)); } }
            _ => {}
        }
    }
    let resp = resp.on_hover_text(&d.target_path).on_hover_cursor(egui::CursorIcon::PointingHand);
    (resp, acts.into_iter().map(|(_, a)| a).collect())
}

/// Signature "peta segmen": one slice per connection range, filled as bytes are saved.
fn segment_strip(ui: &mut Ui, d: &Download, segs: &[Segment]) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, SURFACE_RAISED);
    if d.total_bytes <= 0 || segs.is_empty() {
        let msg = if d.state == DownloadState::Running || d.state == DownloadState::Queued { "Menghubungi server dan memeriksa Range..." } else { "Tidak ada peta segmen" };
        p.text(rect.center(), egui::Align2::CENTER_CENTER, msg, theme::mono(11.0), FG_SECONDARY);
        return;
    }
    let total = d.total_bytes as f32;
    let done_all = d.state == DownloadState::Completed;
    let single_warn = !d.segmentation_supported;
    for s in segs {
        let x0 = rect.left() + rect.width() * (s.range_start as f32 / total);
        let w = rect.width() * ((s.range_end - s.range_start + 1) as f32 / total);
        let len = (s.range_end - s.range_start + 1) as f32;
        let got = ((s.highest_contiguous_offset - s.range_start) as f32 / len).clamp(0.0, 1.0);
        let colour = if done_all { SUCCESS } else if single_warn { WARNING } else { ACCENT };
        let slice = egui::Rect::from_min_size(egui::pos2(x0, rect.top()), Vec2::new(w, rect.height()));
        let mut fill = slice;
        fill.set_width(w * got);
        p.rect_filled(fill, 0.0, colour);
        if d.state == DownloadState::Failed && got < 1.0 {
            p.rect_stroke(slice, 0.0, Stroke::new(1.0, DANGER), egui::StrokeKind::Inside);
        }
        p.line_segment([slice.left_top(), slice.left_bottom()], Stroke::new(2.0, SURFACE)); // divider
        if w > 26.0 {
            p.text(slice.left_center() + Vec2::new(5.0, 0.0), egui::Align2::LEFT_CENTER, format!("#{}", s.segment_index + 1), theme::mono(10.0), if got > 0.15 { if done_all { Color32::WHITE } else { ACCENT_INK } } else { FG_SECONDARY });
        }
    }
    resp.on_hover_text(if single_warn { "Server tidak mendukung Range (satu koneksi)" } else { "Satu irisan per koneksi; terisi = byte yang sudah tersimpan" });
}

impl eframe::App for App {
    /// Runs even while the window is hidden in the tray (DEC-004), so state keeps updating.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain();
        // Browser proposals queue up and are shown one at a time (FR-039).
        if let Some(rx) = &self.proposals_rx {
            while let Ok(p) = rx.try_recv() {
                self.pending.push_back(p);
            }
        }
        if self.add.is_none() {
            if let Some(p) = self.pending.pop_front() {
                self.open_proposal(p);
            }
        }
        if self.last_refresh.elapsed() > Duration::from_millis(250) {
            self.refresh();
        }
        self.poll_torrent();
        while let Ok(url) = self.clip_rx.try_recv() {
            // Never stack dialogs or overwrite one the user is typing in.
            if self.add.is_none() {
                self.open_add_dialog(&url);
                self.screen = Screen::Downloads;
            }
        }
        if let Some(flag) = &self.open_settings {
            if flag.swap(false, Ordering::SeqCst) {
                self.screen = Screen::Settings;
            }
        }
        // DEC-004: closing the window hides it; transfers keep running.
        if ctx.input(|i| i.viewport().close_requested()) && self.close_to_tray && self.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        if !self.sized {
            // Default 1280x800 (§5), but never larger than the primary monitor, e.g. a 1536x864
            // laptop at 125% scaling would otherwise clip the dock and the status strip.
            if let Some((w, h)) = primary_screen_px() {
                let ppp = ctx.pixels_per_point().max(0.5);
                let size = egui::vec2(1280.0f32.min(w / ppp - 48.0), 800.0f32.min(h / ppp - 96.0));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(24.0, 24.0)));
            }
            self.sized = true;
        }
        ctx.request_repaint_after(Duration::from_millis(200));
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::N)) {
            self.open_add_dialog("");
        }
        if !ctx.egui_wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
                let ids: Vec<String> = if self.checked.is_empty() { self.selected.iter().cloned().collect() } else { self.checked.iter().cloned().collect() };
                for id in ids {
                    let m = self.manager.clone();
                    self.run(async move { m.delete(&id, false).await });
                }
                self.checked.clear();
            }
        }

        let width = ctx.content_rect().width();
        let compact = width < 1000.0;
        let flat = |fill: Color32, m: egui::Margin| egui::Frame::new().fill(fill).inner_margin(m);
        let ruled = |fill: Color32, m: egui::Margin| egui::Frame::new().fill(fill).stroke(Stroke::new(1.0, BORDER)).inner_margin(m);

        egui::Panel::top("head").exact_size(76.0).frame(flat(ACCENT, egui::Margin::symmetric(24, 8))).show(ui, |ui| self.header(ui, compact));
        if !matches!(self.update, UpdState::None) {
            let state = self.update.clone();
            egui::Panel::top("update").frame(flat(SURFACE, egui::Margin::symmetric(24, 8))).show(ui, |ui| {
                ui.horizontal(|ui| match state {
                    UpdState::Available(info) => {
                        ui.label(RichText::new(format!("Versi {} tersedia (Anda memakai {}).", info.version, env!("CARGO_PKG_VERSION"))).font(theme::bold(13.0)));
                        ui.label(RichText::new("Unduhan yang berjalan akan dijeda lalu dilanjutkan setelah pembaruan.").color(FG_SECONDARY));
                        if ui.button("Perbarui sekarang").clicked() {
                            self.start_update(info);
                        }
                        if ui.button("Nanti").clicked() {
                            self.update = UpdState::None;
                        }
                    }
                    UpdState::Working(v) => {
                        ui.label(RichText::new(format!("Mengunduh dan memverifikasi versi {v}... aplikasi akan dibuka ulang otomatis.")).font(theme::bold(13.0)));
                    }
                    UpdState::None => {}
                });
            });
        }
        egui::Panel::bottom("status").exact_size(30.0).frame(ruled(SURFACE, egui::Margin::symmetric(20, 4))).show(ui, |ui| self.status_strip(ui));

        if !compact {
            egui::Panel::left("sidebar").exact_size(208.0).resizable(false).frame(ruled(BG, egui::Margin::same(10))).show(ui, |ui| self.sidebar(ui));
        }

        if self.screen == Screen::Downloads {
            let panel = egui::Panel::bottom("dock").frame(ruled(SURFACE, egui::Margin::symmetric(20, 8)));
            let panel = if self.dock_open { panel.resizable(true).size_range(150.0..=420.0).default_size(self.dock_height) } else { panel.exact_size(68.0) };
            panel.show(ui, |ui| {
                if self.dock_open {
                    self.dock_height = ui.available_height();
                }
                self.dock(ui, width > 1600.0)
            });
        }

        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(20, 14))).show(ui, |ui| match self.screen {
            Screen::Downloads => self.table(ui, compact),
            Screen::Settings => self.settings_screen(ui),
            Screen::Diagnostics => self.diagnostics(ui),
            Screen::Grabber => self.grabber(ui),
            Screen::Search => self.search_screen(ui),
        });

        self.add_dialog(ctx);
        self.resize_edges(ctx);
    }
}

// ---- panels ---------------------------------------------------------------

impl App {
    /// Minimize / maximize / close, drawn in the band's top-right corner (Windows order).
    fn window_controls(&mut self, ctx: &egui::Context) {
        let full = ctx.content_rect();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let (w, h) = (46.0, 32.0);
        let top = full.top();
        let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("window-controls"));
        let painter = ctx.layer_painter(layer);
        for (i, kind) in ["min", "max", "close"].into_iter().enumerate() {
            let rect = egui::Rect::from_min_size(egui::pos2(full.right() - w * (3 - i) as f32, top), Vec2::new(w, h));
            let resp = egui::Area::new(egui::Id::new(("wc", kind))).order(egui::Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
                let (_, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
                resp
            }).inner;
            let hot = resp.hovered();
            let close = kind == "close";
            if hot {
                painter.rect_filled(rect, 0.0, if close { Color32::from_rgb(0xC4, 0x2B, 0x1C) } else { Color32::from_black_alpha(36) });
            }
            let ink = if close && hot { Color32::WHITE } else { FG };
            let c = rect.center();
            let st = Stroke::new(1.6, ink);
            match kind {
                "min" => { painter.line_segment([c + Vec2::new(-5.0, 0.0), c + Vec2::new(5.0, 0.0)], st); }
                "max" => {
                    if maximized {
                        painter.rect_stroke(egui::Rect::from_center_size(c + Vec2::new(-1.0, 1.0), Vec2::splat(8.0)), 0.0, st, egui::StrokeKind::Middle);
                        painter.line_segment([c + Vec2::new(-1.0, -4.5), c + Vec2::new(5.0, -4.5)], st);
                        painter.line_segment([c + Vec2::new(5.0, -4.5), c + Vec2::new(5.0, 3.0)], st);
                    } else {
                        painter.rect_stroke(egui::Rect::from_center_size(c, Vec2::splat(10.0)), 0.0, st, egui::StrokeKind::Middle);
                    }
                }
                _ => {
                    painter.line_segment([c + Vec2::new(-5.0, -5.0), c + Vec2::new(5.0, 5.0)], st);
                    painter.line_segment([c + Vec2::new(5.0, -5.0), c + Vec2::new(-5.0, 5.0)], st);
                }
            }
            if resp.clicked() {
                match kind {
                    "min" => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                    "max" => ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized)),
                    _ => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                }
            }
        }
    }

    /// An undecorated window has no native resize border; map the outer 5px to resize drags.
    fn resize_edges(&self, ctx: &egui::Context) {
        use egui::{CursorIcon as C, ResizeDirection as D};
        if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
            return;
        }
        let r = ctx.content_rect();
        let Some(p) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        let m = 5.0;
        let (l, rt, t, b) = (p.x - r.left() < m, r.right() - p.x < m, p.y - r.top() < m, r.bottom() - p.y < m);
        let hit = match (l, rt, t, b) {
            (true, _, true, _) => Some((D::NorthWest, C::ResizeNwSe)),
            (_, true, true, _) => Some((D::NorthEast, C::ResizeNeSw)),
            (true, _, _, true) => Some((D::SouthWest, C::ResizeNeSw)),
            (_, true, _, true) => Some((D::SouthEast, C::ResizeNwSe)),
            (true, ..) => Some((D::West, C::ResizeHorizontal)),
            (_, true, ..) => Some((D::East, C::ResizeHorizontal)),
            (_, _, true, _) => Some((D::North, C::ResizeVertical)),
            (_, _, _, true) => Some((D::South, C::ResizeVertical)),
            _ => None,
        };
        if let Some((dir, cursor)) = hit {
            ctx.set_cursor_icon(cursor);
            if ctx.input(|i| i.pointer.primary_pressed()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
            }
        }
    }

    fn limit_text(&self) -> String {
        let limit = self.manager.db().read(|c| repo::get_setting(c, keys::GLOBAL_SPEED_LIMIT_KBPS)).ok().flatten().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        if limit == 0 { "tak terbatas".to_string() } else { format!("{limit} KB/s") }
    }

    /// Orange band: brand, the one primary action, status tabs, total speed.
    fn header(&mut self, ui: &mut Ui, compact: bool) {
        // The band doubles as the title bar: drag to move, double-click to maximize.
        // Registered first so every widget added afterwards sits above it.
        let band = ui.max_rect().expand2(Vec2::new(24.0, 8.0));
        let drag = ui.interact(band, ui.id().with("titlebar"), Sense::click_and_drag());
        if drag.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
        }
        self.window_controls(ui.ctx());
        ui.horizontal_centered(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::new(30.0, 30.0), Sense::hover());
            theme::paint_bolt(ui.painter(), r.center(), 28.0, FG);
            ui.label(RichText::new("UnduhKilat").font(theme::bold(22.0)).color(FG));
            ui.add_space(12.0);
            let add = egui::Button::new(RichText::new("+  Tambah unduhan").color(Color32::WHITE).font(theme::bold(14.0))).fill(FG).stroke(Stroke::NONE).corner_radius(theme::RADIUS).min_size(Vec2::new(0.0, 44.0));
            if ui.add(add).on_hover_text("Ctrl+N").clicked() {
                self.open_add_dialog("");
            }
            ui.add_space(8.0);
            let tabs = [
                (Filter::All, "Semua", self.count(|_| true)),
                (Filter::Active, "Aktif", self.count(|d| matches!(d.state, DownloadState::Running | DownloadState::Queued))),
                (Filter::Done, "Selesai", self.count(|d| d.state == DownloadState::Completed)),
                (Filter::Paused, "Terjeda", self.count(|d| matches!(d.state, DownloadState::Paused | DownloadState::Failed))),
            ];
            ui.spacing_mut().item_spacing.x = 2.0;
            for (f, name, n) in tabs {
                let active = self.filter == f && self.screen == Screen::Downloads;
                let font = if active { theme::bold(14.0) } else { egui::FontId::proportional(14.0) };
                let text = format!("{name}  {n}");
                let g = ui.painter().layout_no_wrap(text.clone(), font.clone(), FG);
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 26.0, 40.0), Sense::click());
                if active {
                    ui.painter().rect_filled(rect, 8.0, Color32::from_black_alpha(38));
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 8.0, Color32::from_black_alpha(18));
                }
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, font, FG);
                if resp.clicked() {
                    self.filter = f;
                    self.screen = Screen::Downloads;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(if compact { 4.0 } else { 120.0 });
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.with_layout(Layout::top_down(Align::Max), |ui| {
                        ui.label(RichText::new(format::speed(self.total_speed())).font(theme::bold(26.0)).color(FG));
                        if !compact {
                            let conns: u32 = self.live.values().map(|l| l.1).sum();
                            ui.label(RichText::new(format!("{conns} koneksi · batas: {}", self.limit_text())).size(12.0).color(FG));
                        }
                    });
                });
            });
        });
    }

    fn open_settings_screen(&mut self) {
        if self.screen == Screen::Settings {
            self.screen = Screen::Downloads;
            return;
        }
        let db = self.manager.db();
        let get = |k: &str| db.read(|c| repo::get_setting(c, k)).ok().flatten();
        let cfg = EngineConfig::default();
        let px = self.manager.proxy_config();
        self.settings = Some(SettingsState {
            proxy_mode: px.mode,
            proxy_host: px.host.clone(),
            proxy_port: px.port as u32,
            proxy_user: px.username.clone().unwrap_or_default(),
            proxy_pass: String::new(),
            proxy_has_password: px.password.is_some(),
            net_note: None,
            cred_host: String::new(),
            cred_port: 0,
            cred_protocol: 0,
            cred_user: String::new(),
            cred_pass: String::new(),
            browser: browser_status(&self.paths),
            browser_note: None,
            tab: SettingsTab::Engine,
            concurrency: get(keys::CONCURRENCY_LIMIT).and_then(|v| v.parse().ok()).unwrap_or(cfg.default_concurrency as u32),
            connections: get(keys::DEFAULT_CONNECTIONS).and_then(|v| v.parse().ok()).unwrap_or(cfg.default_connections),
            global_kbps: get(keys::GLOBAL_SPEED_LIMIT_KBPS).and_then(|v| v.parse().ok()).unwrap_or(0),
            close_to_tray: get(keys::CLOSE_TO_TRAY).map(|v| v != "0").unwrap_or(true),
            toast: get(keys::COMPLETION_TOAST).map(|v| v != "0").unwrap_or(true),
            cats: self.categories.clone(),
            new_cat: (String::new(), String::new(), String::new()),
            clip: get(keys::CLIPBOARD_MONITOR).is_some_and(|v| v == "1"),
            upd: get(keys::UPDATE_CHECK).map(|v| v != "0").unwrap_or(true),
            t_seeding: get(keys::TORRENT_SEEDING).map(|v| v != "0").unwrap_or(true),
            t_ratio: get(keys::TORRENT_SEED_RATIO).and_then(|v| v.parse().ok()).unwrap_or(1.0),
            t_dht: get(keys::TORRENT_DHT).map(|v| v != "0").unwrap_or(true),
            t_upnp: get(keys::TORRENT_UPNP).map(|v| v == "1").unwrap_or(false),
            t_port: get(keys::TORRENT_PORT).and_then(|v| v.parse().ok()).unwrap_or(0),
            t_net_orig: (
                get(keys::TORRENT_DHT).map(|v| v != "0").unwrap_or(true),
                get(keys::TORRENT_UPNP).map(|v| v == "1").unwrap_or(false),
                get(keys::TORRENT_PORT).and_then(|v| v.parse().ok()).unwrap_or(0),
            ),
        });
        self.screen = Screen::Settings;
    }

    fn status_strip(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            if let Some((msg, at)) = &self.notice {
                if at.elapsed() < Duration::from_secs(10) {
                    ui.label(RichText::new(msg.clone()).color(if msg.contains("ERR_") { DANGER } else { FG }));
                } else {
                    self.notice = None;
                }
            } else {
                ui.label(RichText::new("Antrian: prioritas lalu urutan masuk").size(12.5).color(FG_SECONDARY));
                if self.paths.portable {
                    ui.separator();
                    ui.label(RichText::new("Mode portabel").size(12.5).color(FG_SECONDARY));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let active = self.count(|d| matches!(d.state, DownloadState::Running | DownloadState::Queued));
                ui.label(RichText::new(format!("{active} aktif · batas global {}", self.limit_text())).size(12.5).color(FG_SECONDARY));
            });
        });
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        let nav = |ui: &mut Ui, on: bool, label: &str, dot: Option<Color32>, count: Option<usize>| -> bool {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
            if on {
                ui.painter().rect_filled(rect, 6.0, FG);
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 6.0, Color32::from_black_alpha(14));
            }
            let tc = if on { Color32::WHITE } else { FG };
            let mut x = rect.left() + 12.0;
            if let Some(c) = dot {
                ui.painter().circle_filled(egui::pos2(x + 4.5, rect.center().y), 4.5, c);
                x += 18.0;
            }
            ui.painter().text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(13.5), tc);
            if let Some(n) = count {
                ui.painter().text(egui::pos2(rect.right() - 12.0, rect.center().y), egui::Align2::RIGHT_CENTER, n.to_string(), egui::FontId::proportional(12.0), if on { Color32::from_gray(0xDD) } else { FG_SECONDARY });
            }
            resp.clicked()
        };
        let section = |ui: &mut Ui, t: &str| {
            ui.add_space(12.0);
            ui.label(RichText::new(t).font(theme::bold(11.0)).color(FG_SECONDARY));
        };
        ui.spacing_mut().item_spacing.y = 2.0;
        if nav(ui, self.filter == Filter::All && self.screen == Screen::Downloads, "Semua paket", None, Some(self.count(|_| true))) {
            self.filter = Filter::All;
            self.screen = Screen::Downloads;
        }
        section(ui, "KATEGORI");
        for c in self.categories.clone() {
            let n = self.count(|d| d.category_id.as_deref() == Some(&c.id));
            if nav(ui, self.filter == Filter::Category(c.id.clone()) && self.screen == Screen::Downloads, &c.name, Some(theme::category_color(&c.name)), Some(n)) {
                self.filter = Filter::Category(c.id);
                self.screen = Screen::Downloads;
            }
        }
        section(ui, "ALAT");
        if nav(ui, self.screen == Screen::Grabber, "Site Grabber", None, None) {
            self.screen = if self.screen == Screen::Grabber { Screen::Downloads } else { Screen::Grabber };
        }
        if nav(ui, self.screen == Screen::Search, "Cari berkas", None, None) {
            self.screen = if self.screen == Screen::Search { Screen::Downloads } else { Screen::Search };
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            if ui.add(egui::Button::new(RichText::new("Keluar dari UnduhKilat").size(12.5).color(FG_SECONDARY)).fill(Color32::TRANSPARENT).stroke(Stroke::NONE)).clicked() {
                let (rt, m) = (self.rt.clone(), self.manager.clone());
                std::thread::spawn(move || crate::boot::exit_flow(rt, m));
            }
            if nav(ui, self.screen == Screen::Diagnostics, "Diagnostik", None, None) {
                self.screen = if self.screen == Screen::Diagnostics { Screen::Downloads } else { Screen::Diagnostics };
            }
            if nav(ui, self.screen == Screen::Settings, "Pengaturan", None, None) {
                self.open_settings_screen();
            }
        });
    }

    /// The list of packages (cards), with the bulk actions and sort above it.
    fn table(&mut self, ui: &mut Ui, compact: bool) {
        ui.horizontal(|ui| {
            if ui.button("Lanjutkan semua").clicked() {
                let m = self.manager.clone();
                self.run(async move { m.start_all().await.map(|_| ()) });
            }
            if ui.button("Jeda semua").clicked() {
                let m = self.manager.clone();
                self.run(async move { m.stop_all().await.map(|_| ()) });
            }
            if ui.button("Bersihkan selesai").clicked() {
                let m = self.manager.clone();
                self.run(async move { m.clear_finished().await.map(|_| ()) });
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = match self.sort.0 { SortKey::None => "Terbaru", SortKey::Name => "Nama", SortKey::Size => "Ukuran", SortKey::Status => "Status", SortKey::Progress => "Kemajuan", SortKey::Speed => "Kecepatan", SortKey::Priority => "Prioritas" };
                egui::ComboBox::from_id_salt("sort").selected_text(format!("Urutkan: {label}")).show_ui(ui, |ui| {
                    for (k, n, asc) in [(SortKey::None, "Terbaru", true), (SortKey::Name, "Nama", true), (SortKey::Size, "Ukuran", false), (SortKey::Status, "Status", true), (SortKey::Progress, "Kemajuan", false), (SortKey::Speed, "Kecepatan", false), (SortKey::Priority, "Prioritas", false)] {
                        if ui.selectable_label(self.sort.0 == k, n).clicked() {
                            self.sort = (k, asc);
                        }
                    }
                });
            });
        });
        ui.add_space(6.0);

        let rows = self.visible();
        if rows.is_empty() {
            let filtered = self.filter != Filter::All;
            let mut add = false;
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() / 4.0).max(24.0));
                let (r, _) = ui.allocate_exact_size(Vec2::splat(76.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 38.0, ACCENT);
                theme::paint_bolt(ui.painter(), r.center(), 40.0, FG);
                ui.add_space(10.0);
                ui.label(RichText::new(if filtered { "Tidak ada paket di sini" } else { "Belum ada paket di jalan" }).font(theme::bold(18.0)));
                ui.label(RichText::new(if filtered { "Pilih \"Semua\" di pita atas untuk melihat seluruh unduhan." } else { "Tambahkan tautan, magnet, atau berkas .torrent. UnduhKilat melacaknya dari antre sampai selesai." }).color(FG_SECONDARY));
                ui.add_space(8.0);
                if !filtered && ui.add(primary_button("+  Tambah unduhan").min_size(Vec2::new(0.0, 40.0))).clicked() {
                    add = true;
                }
                if !filtered {
                    ui.label(RichText::new("Tekan Ctrl+N, atau salin tautan di browser. Tidak ada yang dimulai tanpa konfirmasi Anda.").size(12.0).color(MUTED));
                }
            });
            if add {
                self.open_add_dialog("");
            }
            return;
        }

        let mut actions: Vec<(String, Action)> = Vec::new();
        let mut select: Option<String> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            for &i in &rows {
                let d = &self.downloads[i];
                let (speed, conns) = self.live.get(&d.id).copied().unwrap_or((0, 0));
                let sel = self.selected.as_deref() == Some(&d.id);
                let cat = self.categories.iter().find(|c| Some(&c.id) == d.category_id.as_ref()).map(|c| c.name.as_str()).unwrap_or("General");
                let (resp, mut acts) = package_card(ui, d, cat, speed, conns, sel, compact);
                if resp.clicked() { select = Some(d.id.clone()); }
                if resp.double_clicked() && d.state == DownloadState::Completed { acts.push(Action::Reveal); }
                resp.context_menu(|ui| {
                    let id = d.id.clone();
                    if matches!(d.state, DownloadState::Paused | DownloadState::Failed) && ui.button("Lanjutkan").clicked() { acts.push(Action::Resume); ui.close(); }
                    if matches!(d.state, DownloadState::Running | DownloadState::Queued) && ui.button("Jeda").clicked() { acts.push(Action::Pause); ui.close(); }
                    if d.state != DownloadState::Cancelled && ui.button("Mulai ulang dari awal").clicked() { acts.push(Action::Restart); ui.close(); }
                    ui.menu_button("Prioritas", |ui| {
                        for (name, v) in PRIORITIES {
                            if ui.button(name).clicked() { acts.push(Action::Priority(v)); ui.close(); }
                        }
                    });
                    if ui.button("Tampilkan di folder").clicked() { acts.push(Action::Reveal); ui.close(); }
                    ui.separator();
                    if ui.button("Hapus dari daftar").clicked() { acts.push(Action::Delete(false)); ui.close(); }
                    if ui.add(egui::Button::new(RichText::new("Hapus berkas & data").color(DANGER)).stroke(Stroke::new(1.0, DANGER))).clicked() { acts.push(Action::Delete(true)); ui.close(); }
                    let _ = id;
                });
                for a in acts { actions.push((d.id.clone(), a)); }
            }
            ui.add_space(8.0);
        });
        if let Some(id) = select {
            self.selected = Some(id);
            self.last_refresh = Instant::now() - Duration::from_secs(10);
        }
        for (id, a) in actions { self.apply(id, a); }
    }

    fn apply(&mut self, id: String, a: Action) {
        let m = self.manager.clone();
        match a {
            Action::Resume => self.run(async move { m.resume(&id).await.map(|_| ()) }),
            Action::Pause => self.run(async move { m.pause(&id).await.map(|_| ()) }),
            Action::Restart => self.run(async move { m.restart(&id).await.map(|_| ()) }),
            Action::Priority(p) => self.run(async move { m.set_priority(&id, p).await }),
            Action::Delete(f) => self.run(async move { m.delete(&id, f).await }),
            Action::Reveal => {
                if let Ok(d) = self.manager.get(&id) {
                    let _ = std::process::Command::new("explorer").arg(format!("/select,{}", d.target_path.replace('/', "\\"))).spawn();
                }
            }
        }
    }

    /// Bottom strip: segment map of the selected package; "Rincian" opens the full details.
    fn dock(&mut self, ui: &mut Ui, two_panels: bool) {
        let Some(d) = self.selected.as_ref().and_then(|id| self.downloads.iter().find(|d| &d.id == id)).cloned() else {
            ui.add_space(14.0);
            ui.label(RichText::new("Pilih sebuah paket untuk melihat peta segmennya.").color(FG_SECONDARY));
            return;
        };
        let conns = self.live.get(&d.id).map(|l| l.1).unwrap_or(0);
        let is_bt = torrent::is_magnet(&d.original_url);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&d.file_name).font(theme::bold(13.0)));
            ui.label(RichText::new(format!("·  peta segmen {}", if conns > 0 { format!("({conns} {})", if is_bt { "peer" } else { "koneksi" }) } else { String::new() })).size(12.0).color(FG_SECONDARY));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let t = if self.dock_open { "Sembunyikan rincian" } else { "Rincian" };
                if ui.add(egui::Button::new(RichText::new(t).size(12.0)).min_size(Vec2::new(0.0, 22.0))).clicked() {
                    self.dock_open = !self.dock_open;
                }
                ui.label(RichText::new(format!("{} dari {} · {:.0}%", format::bytes(d.downloaded_bytes), format::bytes(d.total_bytes), format::percent(d.downloaded_bytes, d.total_bytes) * 100.0)).font(theme::mono(12.0)).color(FG_SECONDARY));
            });
        });
        if is_bt {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
            ui.painter().rect_filled(rect, 3.0, SURFACE_RAISED);
            let mut f = rect;
            f.set_width(rect.width() * format::percent(d.downloaded_bytes, d.total_bytes));
            ui.painter().rect_filled(f, 3.0, if d.state == DownloadState::Completed { SUCCESS } else { ACCENT });
        } else {
            segment_strip(ui, &d, &self.segments);
        }
        if !self.dock_open {
            return;
        }
        ui.add_space(10.0);
        egui::Grid::new("dock_info").num_columns(2).spacing([16.0, 3.0]).show(ui, |ui| {
            ui.label(RichText::new("URL").color(FG_SECONDARY));
            ui.label(RichText::new(&d.effective_url).font(theme::mono(12.0)));
            ui.end_row();
            ui.label(RichText::new("Tujuan").color(FG_SECONDARY));
            ui.label(RichText::new(&d.target_path).font(theme::mono(12.0)));
            ui.end_row();
            ui.label(RichText::new("Koneksi aktif").color(FG_SECONDARY));
            ui.label(RichText::new(format!("{conns} dari {}", d.connection_count)).font(theme::mono(12.0)));
            ui.end_row();
            if let Some((_, t)) = self.torrent.as_ref().filter(|(id, _)| id == &d.id) {
                ui.label(RichText::new("Info hash").color(FG_SECONDARY));
                ui.label(RichText::new(&t.info_hash).font(theme::mono(12.0)));
                ui.end_row();
                ui.label(RichText::new("Peer").color(FG_SECONDARY));
                ui.label(RichText::new(format!("{} terhubung, {} terlihat", t.peers_connected, t.peers_seen)).font(theme::mono(12.0)));
                ui.end_row();
                ui.label(RichText::new("Unggah").color(FG_SECONDARY));
                ui.label(RichText::new(format!("{} total, rasio {:.2}, {}", format::bytes(t.uploaded_bytes as i64), t.ratio, format::speed(t.upload_bps))).font(theme::mono(12.0)));
                ui.end_row();
                ui.label(RichText::new("Seeding").color(FG_SECONDARY));
                let limit = match t.seed_ratio_limit { Some(r) if r > 0.0 => format!("target rasio {r:.2}"), Some(_) => "tanpa batas".into(), None => "target global".into() };
                let st = match t.seed_state.as_str() { "seeding" => "berjalan", "done" => "berhenti", _ => "belum" };
                ui.label(RichText::new(format!("{st} ({limit})")).font(theme::mono(12.0)));
                ui.end_row();
            }
            if let Some(code) = &d.error_code {
                ui.label(RichText::new("Galat").color(FG_SECONDARY));
                ui.label(RichText::new(format!("{code}: {}", d.error_message.clone().unwrap_or_default())).color(DANGER).font(theme::mono(12.0)));
                ui.end_row();
            }
        });
        if let Some((_, t)) = self.torrent.clone().filter(|(id, _)| id == &d.id) {
            if d.state == DownloadState::Completed {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let (m, id) = (self.manager.clone(), d.id.clone());
                    if t.seed_state == "seeding" {
                        if ui.button("Hentikan seeding").clicked() { self.run(async move { m.torrent_stop_seeding(&id).await }); }
                    } else if ui.button("Mulai seeding").clicked() {
                        self.run(async move { m.torrent_start_seeding(&id).await });
                    }
                });
            }
        }
        ui.add_space(8.0);
        egui::ScrollArea::vertical().id_salt("segs").show(ui, |ui| {
            for s in &self.segments {
                ui.label(RichText::new(format!("#{:<3} {:>12} - {:<12}  berikutnya {:>12}  {}", s.segment_index + 1, s.range_start, s.range_end, s.highest_contiguous_offset, match s.state { unduhkilat_core::models::SegmentState::Pending => "menunggu", unduhkilat_core::models::SegmentState::Downloading => "mengunduh", unduhkilat_core::models::SegmentState::Completed => "selesai" })).font(theme::mono(11.0)));
            }
        });
        let _ = two_panels;
    }
}

#[derive(Clone, Copy)]
enum Action {
    Resume,
    Pause,
    Restart,
    Priority(i32),
    Delete(bool),
    Reveal,
}

// ---- add URL dialog (FR-039, FR-049) -----------------------------------------

impl App {
    fn add_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut a) = self.add.take() else { return };
        let mut close = false;
        let mut submit: Option<bool> = None; // Some(start_immediately)
        let cats = self.categories.clone();
        egui::Window::new("Mulai Unduhan Baru")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size([560.0, 0.0])
            .frame(egui::Frame::window(&ctx.global_style()).fill(SURFACE).inner_margin(egui::Margin::same(24)).shadow(egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(150) }))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Mulai Unduhan Baru").font(theme::bold(17.0)));
                    if a.origin.is_some() {
                        // Badge from the design: this request came from the browser.
                        ui.label(RichText::new(" Tangkap Otomatis ").font(theme::mono(10.0)).color(ACCENT_INK).background_color(ACCENT));
                    }
                });
                ui.label(RichText::new("Tentukan parameter segmen koneksi, kategori berkas, dan folder tujuan.").color(FG_SECONDARY));
                ui.add_space(8.0);
                ui.label("Tautan Sumber (HTTP/HTTPS/FTP/magnet):");
                let urls_edit = ui.add(egui::TextEdit::multiline(&mut a.urls).desired_rows(3).desired_width(f32::INFINITY).font(theme::mono(12.0)).hint_text("Satu tautan per baris, atau tautan magnet:?xt=..."));
                // With nothing focused, Ctrl+V reached no widget: focus the link box so paste works at once.
                if ctx.memory(|m| m.focused().is_none()) {
                    urls_edit.request_focus();
                }
                let urls: Vec<String> = a.urls.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
                ui.horizontal(|ui| {
                    let probe_clicked = ui.add_enabled(urls.len() == 1 && a.torrent_file.is_none(), egui::Button::new("Periksa")).clicked();
                    if probe_clicked && torrent::is_magnet(&urls[0]) {
                        // A magnet needs no server round-trip: the swarm supplies metadata later.
                        a.probe = match torrent::parse_magnet(&urls[0]) {
                            Ok(m) => format!("Torrent  |  hash {}...  |  {}", &m.info_hash[..12], m.name.unwrap_or_else(|| "nama dari peer".into())),
                            Err(e) => format!("{}: {}", e.code, e.message),
                        };
                    } else if probe_clicked {
                        let (tx, ctx2, url) = (self.tx.clone(), self.ctx.clone(), urls[0].clone());
                        // The probe uses the same client as the download, so the proxy applies (FR-015).
                        let (client, proxy) = (self.manager.client(), self.manager.proxy_config());
                        a.probe = "Menghubungi server dan memeriksa Range...".into();
                        self.rt.spawn(async move {
                            let url = match http::normalize_drive_url(&url) {
                                Ok(Some(direct)) => direct,
                                Ok(None) => url,
                                Err(e) => {
                                    let _ = tx.send(Msg::Probe(format!("{}: {}", e.code, e.message), None));
                                    ctx2.request_repaint();
                                    return;
                                }
                            };
                            let probed = if url.to_ascii_lowercase().starts_with("ftp://") {
                                let cfg = EngineConfig::default();
                                let t = ftp::Timeouts { connect: cfg.connect_timeout, read: cfg.read_timeout };
                                match (ftp::parse(&url), proxy.ftp_proxy()) {
                                    (Ok(target), Ok(p)) => ftp::probe(&url, &target, p.as_ref(), &t).await,
                                    (Err(e), _) | (_, Err(e)) => Err(e),
                                }
                            } else {
                                http::probe(&client, &url, &http::RequestContext::default()).await
                            };
                            let msg = match probed {
                                Ok(p) => (format!("{}  |  {}", crate::format::bytes(p.total_bytes.unwrap_or(-1)), if p.range == RangeSupport::Supported { "Mendukung Range" } else { "Tanpa Range (satu koneksi)" }), p.file_name),
                                Err(e) => (format!("{}: {}", e.code, e.message), None),
                            };
                            let _ = tx.send(Msg::Probe(msg.0, msg.1));
                            ctx2.request_repaint();
                        });
                    }
                    ui.label(RichText::new(&a.probe).font(theme::mono(12.0)).color(FG_SECONDARY));
                });
                ui.horizontal(|ui| {
                    if ui.button("Pilih berkas .torrent...").clicked() {
                        if let Some(p) = rfd::FileDialog::new().add_filter("Torrent", &["torrent"]).pick_file() {
                            let too_big = std::fs::metadata(&p).map(|m| m.len() as usize > torrent::MAX_TORRENT_FILE_BYTES).unwrap_or(false);
                            match (too_big, std::fs::read(&p)) {
                                (true, _) => a.err = Some("ERR_INVALID_ARGUMENT: berkas torrent terlalu besar".into()),
                                (_, Err(e)) => a.err = Some(format!("ERR_FILE_NOT_FOUND: {e}")),
                                (_, Ok(bytes)) => match torrent::parse_torrent_bytes(&bytes) {
                                    Ok(m) => {
                                        a.probe = format!("Torrent  |  {}  |  hash {}...", m.total_bytes.map(crate::format::bytes).unwrap_or_default(), &m.info_hash[..12]);
                                        a.torrent_file = Some((p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), bytes));
                                        a.err = None;
                                    }
                                    Err(e) => a.err = Some(format!("{}: {}", e.code, e.message)),
                                },
                            }
                        }
                    }
                    if let Some((n, _)) = a.torrent_file.clone() {
                        ui.label(RichText::new(n).font(theme::mono(12.0)).color(ACCENT_TEXT));
                        if ui.small_button("Hapus").clicked() { a.torrent_file = None; a.probe.clear(); }
                    }
                });
                ui.add_space(8.0);
                egui::Grid::new("add_grid").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                    ui.label("Simpan Sebagai:");
                    ui.add_enabled(urls.len() <= 1, egui::TextEdit::singleline(&mut a.file_name).desired_width(f32::INFINITY));
                    ui.end_row();
                    ui.label("Kategori Otomatis:");
                    let sel = a.category.as_ref().and_then(|id| cats.iter().find(|c| &c.id == id)).map(|c| c.name.clone()).unwrap_or_else(|| "Otomatis".into());
                    egui::ComboBox::from_id_salt("cat").selected_text(sel).show_ui(ui, |ui| {
                        ui.selectable_value(&mut a.category, None, "Otomatis");
                        for c in &cats { ui.selectable_value(&mut a.category, Some(c.id.clone()), &c.name); }
                    });
                    ui.end_row();
                    ui.label("Folder Tujuan:");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut a.destination).desired_width(330.0).hint_text("Ikuti kategori"));
                        if ui.button("Telusuri").clicked() {
                            if let Some(p) = rfd::FileDialog::new().pick_folder() { a.destination = p.to_string_lossy().into_owned(); }
                        }
                    });
                    ui.end_row();
                    ui.label("Jumlah Koneksi Simultan:");
                    // DEC-001: any positive integer, no upper bound.
                    ui.add(egui::DragValue::new(&mut a.connections).range(1..=u32::MAX).speed(0.2));
                    ui.end_row();
                    ui.label("Prioritas Antrian:");
                    egui::ComboBox::from_id_salt("prio").selected_text(priority_label(a.priority)).show_ui(ui, |ui| {
                        for (n, v) in PRIORITIES { ui.selectable_value(&mut a.priority, v, n); }
                    });
                    ui.end_row();
                });
                ui.collapsing("Referer dan Cookie", |ui| {
                    ui.add(egui::TextEdit::singleline(&mut a.referer).desired_width(f32::INFINITY).hint_text("Referer"));
                    ui.add(egui::TextEdit::singleline(&mut a.cookies).desired_width(f32::INFINITY).hint_text("Cookie: nama=nilai; ..."));
                });
                if let Some(e) = &a.err { ui.label(RichText::new(e).color(DANGER)); }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!urls.is_empty() || a.torrent_file.is_some(), |ui| {
                        if ui.add(primary_button("Mulai Mengunduh")).clicked() { submit = Some(true); }
                        if ui.button("Antrikan Saja").clicked() { submit = Some(false); }
                    });
                    if ui.button("Batal").clicked() { close = true; }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) { close = true; }

        if let Some(start) = submit {
            let urls: Vec<String> = a.urls.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
            let single = urls.len() == 1;
            let mut first_err = None;
            let mut created_id: Option<String> = None;
            if let Some((fname, bytes)) = a.torrent_file.clone() {
                let target = (!a.destination.trim().is_empty()).then(|| {
                    let n = torrent::parse_torrent_bytes(&bytes).ok().and_then(|m| m.name).unwrap_or_else(|| fname.clone());
                    std::path::PathBuf::from(a.destination.trim()).join(n)
                });
                let req = NewDownload { target_path: target, category_id: a.category.clone(), priority: a.priority, start_immediately: start, ..Default::default() };
                match self.rt.block_on(self.manager.clone().create_torrent_file(bytes, req)) {
                    Ok(d) => { created_id.get_or_insert(d.id); }
                    Err(e) => { first_err.get_or_insert(format!("{}: {}", e.code, e.message)); }
                }
            }
            for url in urls {
                let name = (single && !a.file_name.trim().is_empty()).then(|| a.file_name.trim().to_string());
                let target = (!a.destination.trim().is_empty()).then(|| {
                    let n = name.clone().or_else(|| http::file_name_from_url(&url)).unwrap_or_else(|| "download".into());
                    std::path::PathBuf::from(a.destination.trim()).join(n)
                });
                let req = NewDownload {
                    url, file_name: name, target_path: target, category_id: a.category.clone(), connection_count: Some(a.connections.max(1)),
                    priority: a.priority, referer: Some(a.referer.trim().to_string()).filter(|s| !s.is_empty()),
                    cookies: Some(a.cookies.trim().to_string()).filter(|s| !s.is_empty()),
                    user_agent: Some(a.user_agent.clone()).filter(|s| !s.is_empty()), start_immediately: start, ..Default::default()
                };
                let m = self.manager.clone();
                // Validate synchronously so a rejected URL keeps the dialog open.
                let lower = req.url.to_ascii_lowercase();
                let remote_torrent = lower.starts_with("http") && lower.split(['?', '#']).next().is_some_and(|p| p.ends_with(".torrent"));
                let res = if remote_torrent {
                    // Bounded so a dead server cannot freeze the window.
                    let u = req.url.clone();
                    self.rt.block_on(async { tokio::time::timeout(Duration::from_secs(20), m.create_torrent_url(&u, req)).await.unwrap_or_else(|_| Err(unduhkilat_core::error::AppError::new(unduhkilat_core::error::ErrorCode::ErrConnectionDropped, "timed out fetching the .torrent file"))) })
                } else {
                    self.rt.block_on(m.create(req))
                };
                match res {
                    Ok(d) => { created_id.get_or_insert(d.id); }
                    Err(e) => { first_err.get_or_insert(format!("{}: {}", e.code, e.message)); }
                }
            }
            match first_err {
                Some(e) => { a.err = Some(e); }
                None => {
                    close = true;
                    // Tell the browser which download its request became (FR-038).
                    if let (Some(b), Some(req), Some(id)) = (&self.bridge, a.origin.take(), created_id) {
                        b.track(&req, &id);
                    }
                }
            }
        }
        if close {
            // Closed without starting anything: the browser must hear about it too.
            if let (Some(b), Some(req)) = (&self.bridge, a.origin.take()) {
                b.reject(&req);
            }
        }
        if !close { self.add = Some(a); }
        self.last_refresh = Instant::now() - Duration::from_secs(10);
    }
}

// ---- settings (§6 Primary Screen 3) ------------------------------------------

impl App {
    fn settings_screen(&mut self, ui: &mut Ui) {
        let Some(mut s) = self.settings.take() else { self.open_settings_screen(); return };
        let mut save = false;
        let mut check_now = false;
        ui.label(RichText::new("Pengaturan").font(theme::bold(17.0)));
        ui.add_space(6.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(180.0);
                for (t, name) in [(SettingsTab::Engine, "Mesin Unduhan"), (SettingsTab::Categories, "Kategori & Folder"), (SettingsTab::Speed, "Batas Kecepatan"), (SettingsTab::Behaviour, "Jendela & Notifikasi"), (SettingsTab::Network, "Jaringan & Proxy"), (SettingsTab::Torrent, "Torrent"), (SettingsTab::Browser, "Integrasi Browser")] {
                    if ui.selectable_label(s.tab == t, name).clicked() { s.tab = t; }
                }
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.set_max_width(640.0);
                match s.tab {
                    SettingsTab::Engine => {
                        egui::Grid::new("g1").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                            ui.label("Unduhan berjalan bersamaan:");
                            ui.add(egui::DragValue::new(&mut s.concurrency).range(1..=u32::MAX).speed(0.2));
                            ui.end_row();
                            ui.label("Koneksi per unduhan (baru):");
                            ui.add(egui::DragValue::new(&mut s.connections).range(1..=u32::MAX).speed(0.2));
                            ui.end_row();
                        });
                        ui.label(RichText::new("Tanpa batas atas; nilai apa pun mulai 1 dipatuhi persis. Lihat Diagnostik untuk jumlah koneksi aktif.").color(FG_SECONDARY));
                    }
                    SettingsTab::Speed => {
                        egui::Grid::new("g2").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                            ui.label("Batas kecepatan global (KB/s):");
                            ui.add(egui::DragValue::new(&mut s.global_kbps).range(0..=u32::MAX).speed(10.0));
                            ui.end_row();
                        });
                        ui.label(RichText::new("0 berarti tak terbatas. Berlaku langsung pada unduhan yang sedang berjalan.").color(FG_SECONDARY));
                    }
                    SettingsTab::Network => {
                        ui.label(RichText::new("Proxy").font(theme::bold(14.0)));
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut s.proxy_mode, ProxyMode::Direct, "Langsung");
                            ui.selectable_value(&mut s.proxy_mode, ProxyMode::System, "Proxy sistem");
                            ui.selectable_value(&mut s.proxy_mode, ProxyMode::Manual, "Manual");
                        });
                        if s.proxy_mode == ProxyMode::Manual {
                            egui::Grid::new("proxy_grid").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                                ui.label("Host:");
                                ui.add(egui::TextEdit::singleline(&mut s.proxy_host).desired_width(300.0).hint_text("proxy.contoh.com  atau  socks5://host"));
                                ui.end_row();
                                ui.label("Port:");
                                ui.add(egui::DragValue::new(&mut s.proxy_port).range(0..=65535));
                                ui.end_row();
                                ui.label("Pengguna:");
                                ui.add(egui::TextEdit::singleline(&mut s.proxy_user).desired_width(300.0));
                                ui.end_row();
                                ui.label("Kata sandi:");
                                ui.add(egui::TextEdit::singleline(&mut s.proxy_pass).password(true).desired_width(300.0).hint_text(if s.proxy_has_password { "(tersimpan; kosongkan untuk mempertahankan)" } else { "" }));
                                ui.end_row();
                            });
                            ui.label(RichText::new("FTP hanya dapat melalui proxy HTTP (tunnel CONNECT); proxy SOCKS berlaku untuk HTTP/HTTPS.").color(FG_SECONDARY));
                        }
                        if ui.add(primary_button("Terapkan Proxy")).clicked() {
                            let keep = self.manager.proxy_config().password;
                            let cfg = ProxyConfig {
                                mode: s.proxy_mode,
                                host: s.proxy_host.trim().to_string(),
                                port: s.proxy_port.min(65535) as u16,
                                username: Some(s.proxy_user.trim().to_string()).filter(|u| !u.is_empty()),
                                password: if s.proxy_pass.is_empty() { keep } else { Some(zeroize::Zeroizing::new(s.proxy_pass.clone())) },
                            };
                            s.net_note = Some(match self.rt.block_on(self.manager.set_proxy(cfg)) {
                                Ok(()) => { s.proxy_pass.clear(); s.proxy_has_password = self.manager.proxy_config().password.is_some(); "Proxy diterapkan untuk unduhan berikutnya.".to_string() }
                                Err(e) => format!("{}: {}", e.code, e.message),
                            });
                        }
                        ui.add_space(16.0);
                        ui.separator();
                        ui.label(RichText::new("Kredensial host (disimpan terenkripsi)").font(theme::bold(14.0)));
                        egui::Grid::new("cred_grid").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                            ui.label("Host:");
                            ui.add(egui::TextEdit::singleline(&mut s.cred_host).desired_width(300.0).hint_text("ftp.contoh.com"));
                            ui.end_row();
                            ui.label("Port (0 = bawaan):");
                            ui.add(egui::DragValue::new(&mut s.cred_port).range(0..=65535));
                            ui.end_row();
                            ui.label("Protokol:");
                            egui::ComboBox::from_id_salt("cred_proto").selected_text(["ftp", "http", "https"][s.cred_protocol]).show_ui(ui, |ui| {
                                for (i, n) in ["ftp", "http", "https"].iter().enumerate() { ui.selectable_value(&mut s.cred_protocol, i, *n); }
                            });
                            ui.end_row();
                            ui.label("Pengguna:");
                            ui.add(egui::TextEdit::singleline(&mut s.cred_user).desired_width(300.0));
                            ui.end_row();
                            ui.label("Kata sandi:");
                            ui.add(egui::TextEdit::singleline(&mut s.cred_pass).password(true).desired_width(300.0));
                            ui.end_row();
                        });
                        if ui.button("Simpan Kredensial").clicked() {
                            let port = (s.cred_port > 0).then_some(s.cred_port.min(65535) as u16);
                            let proto = ["ftp", "http", "https"][s.cred_protocol];
                            s.net_note = Some(match self.rt.block_on(self.manager.save_credential(&s.cred_host, port, &s.cred_user, &s.cred_pass, proto)) {
                                Ok(()) => { s.cred_pass.clear(); "Kredensial disimpan.".to_string() }
                                Err(e) => format!("{}: {}", e.code, e.message),
                            });
                        }
                        if let Some(n) = &s.net_note { ui.label(RichText::new(n).color(FG_SECONDARY)); }
                    }
                    SettingsTab::Browser => {
                        // DEC-006: only the three Chromium-class browsers. Status is text, never colour alone.
                        for (b, st) in &s.browser {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(b.name()).font(theme::bold(14.0)));
                                let (t, c) = match st {
                                    native::Status::Registered => ("Terdaftar", SUCCESS),
                                    native::Status::Missing => ("Belum terdaftar", WARNING),
                                    native::Status::Stale => ("Perlu diperbaiki (folder dipindah?)", DANGER),
                                };
                                ui.label(RichText::new(t).color(c));
                            });
                        }
                        if ui.button("Daftarkan Ulang").clicked() {
                            match register_host(&self.paths) {
                                Ok(st) => { s.browser = st; s.browser_note = Some("Pendaftaran native host HKCU diperbarui.".into()); }
                                Err(e) => { s.browser_note = Some(format!("{}: {}", e.code, e.message)); }
                            }
                        }
                        if let Some(n) = &s.browser_note { ui.label(RichText::new(n).color(FG_SECONDARY)); }
                        ui.add_space(8.0);
                        let ext_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("extension"))).map(|p| p.display().to_string()).unwrap_or_default();
                        ui.label(RichText::new("Pasang ekstensi: buka chrome://extensions (atau edge://extensions, brave://extensions), aktifkan Mode pengembang, lalu Muat yang belum dipaketkan dan pilih folder:").color(FG_SECONDARY));
                        ui.label(RichText::new(ext_dir).font(theme::mono(12.0)));
                    }
                    SettingsTab::Torrent => {
                        ui.checkbox(&mut s.t_seeding, "Lanjutkan seeding setelah unduhan selesai");
                        egui::Grid::new("g_torrent").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                            ui.label("Target rasio berbagi (0 = tanpa batas):");
                            ui.add(egui::DragValue::new(&mut s.t_ratio).range(0.0..=1000.0).speed(0.05).fixed_decimals(2));
                            ui.end_row();
                            ui.label("Port masuk (0 = acak):");
                            ui.add(egui::DragValue::new(&mut s.t_port).range(0..=65535).speed(1.0));
                            ui.end_row();
                        });
                        ui.checkbox(&mut s.t_dht, "Gunakan DHT (cari peer tanpa tracker)");
                        ui.checkbox(&mut s.t_upnp, "Buka port otomatis lewat UPnP");
                        ui.add_space(4.0);
                        ui.label(RichText::new("Nilai bawaan ini masih sementara (PROVISIONAL). Sesi torrent baru dibuat saat torrent pertama dijalankan, bukan saat aplikasi dibuka. Mengubah DHT, UPnP, atau port menjeda torrent yang sedang berjalan; lanjutkan secara manual.").color(FG_SECONDARY));
                        ui.label(RichText::new("Dengan proxy SOCKS5, DHT, UPnP, dan port masuk dimatikan otomatis. Proxy HTTP tidak bisa membawa BitTorrent, sehingga torrent ditolak (bukan dialihkan langsung).").color(FG_SECONDARY));
                    }
                    SettingsTab::Behaviour => {
                        ui.checkbox(&mut s.close_to_tray, "Tutup jendela ke tray (unduhan tetap berjalan)");
                        ui.checkbox(&mut s.toast, "Tampilkan notifikasi saat unduhan selesai");
                        ui.checkbox(&mut s.clip, "Pantau clipboard: tawarkan tautan HTTP/HTTPS/FTP yang disalin");
                        ui.checkbox(&mut s.upd, "Cek versi baru saat aplikasi dibuka (menghubungi github.com)");
                        if ui.button("Periksa pembaruan sekarang").clicked() {
                            check_now = true;
                        }
                        ui.label(RichText::new("Hanya teks yang seluruhnya satu tautan yang ditawarkan, dan tidak ada yang diunduh sebelum Anda mengonfirmasi.").color(FG_SECONDARY));
                    }
                    SettingsTab::Categories => {
                        let mut del: Option<String> = None;
                        let mut upd: Option<Category> = None;
                        for c in s.cats.iter_mut() {
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut c.name).desired_width(110.0));
                                ui.add(egui::TextEdit::singleline(&mut c.target_directory).desired_width(250.0).font(theme::mono(12.0)));
                                ui.add(egui::TextEdit::singleline(&mut c.file_extensions).desired_width(150.0).font(theme::mono(12.0)));
                                if ui.checkbox(&mut c.is_default, "Bawaan").changed() { upd = Some(c.clone()); }
                                if ui.button("Simpan").clicked() { upd = Some(c.clone()); }
                                if !c.is_default && ui.button("Hapus").clicked() { del = Some(c.id.clone()); }
                            });
                        }
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut s.new_cat.0).desired_width(110.0).hint_text("Nama"));
                            ui.add(egui::TextEdit::singleline(&mut s.new_cat.1).desired_width(250.0).hint_text("C:\\Folder"));
                            ui.add(egui::TextEdit::singleline(&mut s.new_cat.2).desired_width(150.0).hint_text("ext1,ext2"));
                            if ui.button("Tambah").clicked() {
                                match category::create(self.manager.db(), &s.new_cat.0, &s.new_cat.1, &s.new_cat.2, false) {
                                    Ok(_) => { s.new_cat = Default::default(); s.cats = self.manager.db().read(repo::list_categories).unwrap_or_default(); }
                                    Err(e) => self.notice = Some((format!("{}: {}", e.code, e.message), Instant::now())),
                                }
                            }
                        });
                        if let Some(c) = upd {
                            match category::update(self.manager.db(), c) {
                                Ok(()) => s.cats = self.manager.db().read(repo::list_categories).unwrap_or_default(),
                                Err(e) => self.notice = Some((format!("{}: {}", e.code, e.message), Instant::now())),
                            }
                        }
                        if let Some(id) = del {
                            match category::delete(self.manager.db(), &id) {
                                Ok(()) => s.cats = self.manager.db().read(repo::list_categories).unwrap_or_default(),
                                Err(e) => self.notice = Some((format!("{}: {}", e.code, e.message), Instant::now())),
                            }
                        }
                        ui.label(RichText::new("Perubahan hanya berlaku untuk unduhan berikutnya; berkas yang sudah tersimpan tidak dipindah.").color(FG_SECONDARY));
                    }
                }
                if s.tab != SettingsTab::Categories {
                    ui.add_space(16.0);
                    if ui.add(primary_button("Simpan Pengaturan")).clicked() { save = true; }
                }
            });
        });
        if save {
            let m = self.manager.clone();
            let (c, n, g, tray, toast) = (s.concurrency as usize, s.connections, s.global_kbps as u64, s.close_to_tray, s.toast);
            self.clip_enabled.store(s.clip, Ordering::Relaxed);
            let clip = s.clip;
            let upd = s.upd;
            let (t_seed, t_ratio, t_dht, t_upnp, t_port) = (s.t_seeding, s.t_ratio.max(0.0), s.t_dht, s.t_upnp, s.t_port.min(65535));
            let net_changed = (t_dht, t_upnp, t_port) != s.t_net_orig;
            s.t_net_orig = (t_dht, t_upnp, t_port);
            self.run(async move {
                m.set_global_speed_limit(g).await?;
                m.db().write(move |conn| {
                    repo::set_setting(conn, keys::DEFAULT_CONNECTIONS, &n.to_string())?;
                    repo::set_setting(conn, keys::CLOSE_TO_TRAY, if tray { "1" } else { "0" })?;
                    repo::set_setting(conn, keys::CLIPBOARD_MONITOR, if clip { "1" } else { "0" })?;
                    repo::set_setting(conn, keys::UPDATE_CHECK, if upd { "1" } else { "0" })?;
                    repo::set_setting(conn, keys::TORRENT_SEEDING, if t_seed { "1" } else { "0" })?;
                    repo::set_setting(conn, keys::TORRENT_SEED_RATIO, &t_ratio.to_string())?;
                    repo::set_setting(conn, keys::TORRENT_DHT, if t_dht { "1" } else { "0" })?;
                    repo::set_setting(conn, keys::TORRENT_UPNP, if t_upnp { "1" } else { "0" })?;
                    repo::set_setting(conn, keys::TORRENT_PORT, &t_port.to_string())?;
                    repo::set_setting(conn, keys::COMPLETION_TOAST, if toast { "1" } else { "0" })
                }).await?;
                if net_changed {
                    m.reset_torrents().await;
                }
                m.set_concurrency_limit(c).await
            });
            self.notice = Some(("Pengaturan disimpan.".into(), Instant::now()));
        }
        if check_now {
            self.check_updates(true);
        }
        self.settings = Some(s);
    }

    // ---- diagnostics (§6 Primary Screen 4, FR-056) ---------------------------

    fn diagnostics(&mut self, ui: &mut Ui) {
        let conns: u32 = self.live.values().map(|l| l.1).sum();
        ui.label(RichText::new("Diagnostik").font(theme::bold(17.0)));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Unduhan aktif: {}", self.manager.running_count())).font(theme::mono(13.0)));
            ui.separator();
            ui.label(RichText::new(format!("Koneksi aktif: {conns}")).font(theme::mono(13.0)));
        });
        ui.add_space(4.0);
        let failed: Vec<&Download> = self.downloads.iter().filter(|d| d.error_code.is_some()).collect();
        if !failed.is_empty() {
            ui.label(RichText::new("Galat terakhir per unduhan").color(FG_SECONDARY));
            for d in failed {
                ui.label(RichText::new(format!("{}  {}  {}", d.file_name, d.error_code.clone().unwrap_or_default(), d.error_message.clone().unwrap_or_default())).font(theme::mono(11.0)).color(DANGER));
            }
            ui.add_space(4.0);
        }
        let levels = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"];
        ui.horizontal(|ui| {
            for (i, l) in levels.iter().enumerate() {
                if ui.selectable_label(self.log_level == i, *l).clicked() { self.log_level = i; }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(primary_button("Ekspor Berkas Log")).clicked() {
                    if let Some(dest) = rfd::FileDialog::new().pick_folder() {
                        match export_logs(&self.paths.logs_dir, &dest) {
                            Ok(n) => self.notice = Some((format!("{n} berkas log diekspor."), Instant::now())),
                            Err(e) => self.notice = Some((format!("ERR_EXPORT_FAILED: {e}"), Instant::now())),
                        }
                    }
                }
            });
        });
        let min = self.log_level;
        egui::Frame::new().fill(BG).stroke(Stroke::new(1.0, BORDER)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                for line in self.log_tail.tail() {
                    let rank = levels.iter().position(|l| line.contains(&format!(" {l} "))).unwrap_or(2);
                    if rank < min { continue; }
                    let col = match rank { 4 => DANGER, 3 => WARNING, _ => FG };
                    ui.label(RichText::new(line).font(theme::mono(11.0)).color(col));
                }
            });
        });
    }
}

fn export_logs(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<usize> {
    let mut n = 0;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        if e.file_type()?.is_file() {
            std::fs::copy(e.path(), to.join(e.file_name()))?;
            n += 1;
        }
    }
    Ok(n)
}


// ---- Site Grabber & Batch Downloader (§6 Primary Screen 2, FR-045/047) ------------

impl App {
    fn grabber(&mut self, ui: &mut Ui) {
        let mut scan = false;
        let mut enqueue = false;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(300.0);
                ui.label(RichText::new("Site Grabber").font(theme::bold(17.0)));
                ui.label(RichText::new("Pindai satu halaman, pratinjau tautan yang cocok, lalu antrikan yang Anda centang. Tidak ada yang diunduh sebelum Anda mengonfirmasi.").color(FG_SECONDARY));
                ui.add_space(8.0);
                ui.label("URL Halaman:");
                ui.add(egui::TextEdit::singleline(&mut self.grab.url).desired_width(f32::INFINITY).font(theme::mono(12.0)).hint_text("https://contoh.com/unduhan/"));
                ui.label("Pola (wildcard):");
                ui.add(egui::TextEdit::singleline(&mut self.grab.pattern).desired_width(f32::INFINITY).font(theme::mono(12.0)).hint_text("*.pdf   atau   */a/*.zip   atau   foto_???.jpg"));
                ui.label(RichText::new("* = urutan apa pun, ? = satu karakter. Pola tanpa / dicocokkan ke nama berkas; pola dengan / ke URL penuh.").color(FG_SECONDARY).size(11.0));
                ui.add_space(8.0);
                let ready = !self.grab.url.trim().is_empty() && !self.grab.pattern.trim().is_empty() && !self.grab.scanning;
                ui.add_enabled_ui(ready, |ui| {
                    if ui.add(primary_button("Pindai Halaman")).clicked() { scan = true; }
                });
                if self.grab.scanning {
                    ui.label(RichText::new("Memindai tautan halaman...").font(theme::mono(12.0)).color(ACCENT_TEXT));
                }
                if let Some(e) = &self.grab.err { ui.label(RichText::new(e).color(DANGER)); }
                ui.add_space(8.0);
                ui.label(RichText::new("Penelusuran berkedalaman (Site Walk) belum tersedia: kedalaman bawaannya masih menunggu keputusan pemilik (TBD-017).").color(FG_SECONDARY).size(11.0));
            });
            ui.separator();
            ui.vertical(|ui| {
                let Some(res) = self.grab.result.as_ref() else {
                    ui.add_space(ui.available_height() / 3.0);
                    ui.vertical_centered(|ui| { ui.label(RichText::new("Belum ada pratinjau").color(FG_SECONDARY)); });
                    return;
                };
                let total = res.candidates.len();
                let truncated = res.truncated;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Ditemukan: {total} berkas dari {} tautan", res.links_on_page)).font(theme::mono(12.0)));
                    ui.add(egui::TextEdit::singleline(&mut self.grab.filter).desired_width(160.0).hint_text("Saring daftar"));
                    if ui.button("Centang Semua").clicked() { for c in self.grab.checked.iter_mut() { *c = true; } }
                    if ui.button("Kosongkan").clicked() { for c in self.grab.checked.iter_mut() { *c = false; } }
                });
                if truncated { ui.label(RichText::new("Terlalu banyak tautan cocok; hanya 5000 pertama ditampilkan. Persempit polanya.").color(WARNING)); }
                if let Some(n) = &self.grab.note { ui.label(RichText::new(n).color(FG_SECONDARY)); }
                let needle = self.grab.filter.to_lowercase();
                egui::ScrollArea::vertical().id_salt("grab_rows").auto_shrink([false, false]).max_height((ui.available_height() - 44.0).max(80.0)).show(ui, |ui| {
                    for (i, c) in res.candidates.iter().enumerate() {
                        if !needle.is_empty() && !c.url.to_lowercase().contains(&needle) { continue; }
                        ui.horizontal(|ui| {
                            if let Some(b) = self.grab.checked.get_mut(i) { ui.checkbox(b, ""); }
                            ui.label(RichText::new(&c.file_name).font(theme::bold(13.0)));
                            let size = c.size.map(crate::format::bytes).unwrap_or_else(|| "Ukuran: Tidak Diketahui".into());
                            ui.label(RichText::new(size).font(theme::mono(11.0)).color(FG_SECONDARY));
                        });
                        ui.label(RichText::new(&c.url).font(theme::mono(10.0)).color(MUTED));
                    }
                });
                let picked = self.grab.checked.iter().filter(|c| **c).count();
                ui.add_space(6.0);
                ui.add_enabled_ui(picked > 0, |ui| {
                    if ui.add(primary_button(&format!("Antrikan File Terpilih ({picked})"))).clicked() { enqueue = true; }
                });
            });
        });

        if scan {
            self.grab.scanning = true;
            self.grab.err = None;
            self.grab.note = None;
            let (m, tx, ctx) = (self.manager.clone(), self.tx.clone(), self.ctx.clone());
            let (url, pat) = (self.grab.url.trim().to_string(), self.grab.pattern.trim().to_string());
            self.rt.spawn(async move {
                let r = m.scan_links(&url, &pat).await.map_err(|e| format!("{}: {}", e.code, e.message));
                let _ = tx.send(Msg::Scan(r));
                ctx.request_repaint();
            });
        }
        if enqueue {
            let urls: Vec<String> = match &self.grab.result {
                Some(r) => r.candidates.iter().zip(&self.grab.checked).filter(|(_, on)| **on).map(|(c, _)| c.url.clone()).collect(),
                None => Vec::new(),
            };
            let out = self.rt.block_on(self.manager.clone().enqueue_batch(urls, 0, None));
            let mut msg = format!("{} berkas diantrikan.", out.created.len());
            if let Some((u, e)) = out.failed.first() {
                msg += &format!(" {} gagal (mis. {u}: {e}).", out.failed.len());
            }
            self.notice = Some((msg, Instant::now()));
            self.last_refresh = Instant::now() - Duration::from_secs(10);
            if out.failed.is_empty() {
                self.grab.result = None;
                self.grab.checked.clear();
                self.screen = Screen::Downloads;
            }
        }
    }
}


// ---- File search (DEC-009) --------------------------------------------------------

impl App {
    fn search_screen(&mut self, ui: &mut Ui) {
        if self.search.providers.is_empty() {
            let on = self.manager.search_providers();
            self.search.providers = unduhkilat_core::engine::search::ALL_PROVIDERS.iter().map(|p| (*p, on.contains(p))).collect();
        }
        let mut go = false;
        let mut enqueue = false;
        ui.label(RichText::new("Cari Berkas").font(theme::bold(17.0)));
        ui.label(RichText::new("Kata kunci dikirim ke sumber yang dicentang di bawah, hanya saat Anda menekan Cari. Tidak ada yang diunduh sebelum Anda memilih hasil dan menekan Antrikan.").color(FG_SECONDARY));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.search.query).desired_width(420.0).font(theme::mono(13.0)).hint_text("Nama berkas, film, dataset..."));
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let any = self.search.providers.iter().any(|(_, on)| *on);
            let ready = self.search.query.trim().chars().count() >= 2 && any && !self.search.busy;
            ui.add_enabled_ui(ready, |ui| {
                if ui.add(primary_button("Cari")).clicked() || (enter && ready) { go = true; }
            });
            if self.search.busy { ui.label(RichText::new("Mencari...").font(theme::mono(12.0)).color(ACCENT_TEXT)); }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Sumber:").color(FG_SECONDARY));
            let mut changed: Option<(Provider, bool)> = None;
            for (p, on) in self.search.providers.iter_mut() {
                let label = match p {
                    Provider::Apibay | Provider::Nyaa => format!("{} (indeks umum)", p.name()),
                    _ => p.name().to_string(),
                };
                if ui.checkbox(on, label).changed() { changed = Some((*p, *on)); }
            }
            if let Some((p, on)) = changed {
                let m = self.manager.clone();
                self.run(async move { m.set_search_provider(p, on).await });
            }
        });
        ui.horizontal(|ui| {
            let ok = self.search.query.trim().chars().count() >= 2;
            ui.add_enabled_ui(ok, |ui| {
                if ui.button("Cari file Drive publik di browser").on_hover_text("Membuka Google dengan site:drive.google.com di browser Anda. UnduhKilat sendiri tidak mengirim apa pun.").clicked() {
                    let q = format!("site:drive.google.com {}", self.search.query.trim());
                    open_in_browser(&format!("https://www.google.com/search?q={}", percent_encode(&q)));
                }
            });
            ui.label(RichText::new("Salin tautan Drive yang ketemu, lalu tempel di Tambah unduhan (atau aktifkan pemantau clipboard di Pengaturan).").size(11.5).color(FG_SECONDARY));
        });
        ui.label(RichText::new("Indeks umum dapat memuat materi berhak cipta; pastikan Anda berhak mengunduh apa pun yang dipilih. Academic Torrents hanya mencari entri terbarunya.").color(FG_SECONDARY).size(11.0));
        if let Some(e) = &self.search.err { ui.label(RichText::new(e).color(DANGER)); }
        ui.separator();

        if let Some(res) = self.search.result.as_ref() {
            for (p, e) in &res.errors {
                ui.label(RichText::new(format!("{}: {e}", p.name())).color(WARNING).font(theme::mono(11.0)));
            }
            if res.hits.is_empty() && res.errors.is_empty() {
                ui.label(RichText::new("Tidak ada hasil.").color(FG_SECONDARY));
            }
            ui.label(RichText::new(format!("{} hasil", res.hits.len())).font(theme::mono(12.0)));
            egui::ScrollArea::vertical().id_salt("search_rows").auto_shrink([false, false]).max_height((ui.available_height() - 44.0).max(80.0)).show(ui, |ui| {
                for (i, h) in res.hits.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if let Some(b) = self.search.checked.get_mut(i) { ui.checkbox(b, ""); }
                        ui.label(RichText::new(&h.title).font(theme::bold(13.0)));
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(26.0);
                        let kind = match h.source { Source::Magnet(_) => "magnet", Source::TorrentUrl(_) => ".torrent" };
                        let size = h.size.map(crate::format::bytes).unwrap_or_else(|| "Ukuran: Tidak Diketahui".into());
                        let seeds = match (h.seeders, h.leechers) { (Some(s), Some(l)) => format!("  |  seed {s} / leech {l}"), (Some(s), None) => format!("  |  seed {s}"), _ => String::new() };
                        ui.label(RichText::new(format!("{}  |  {kind}  |  {size}{seeds}", h.provider.name())).font(theme::mono(10.5)).color(FG_SECONDARY));
                    });
                }
            });
            let picked = self.search.checked.iter().filter(|c| **c).count();
            ui.add_space(6.0);
            ui.add_enabled_ui(picked > 0, |ui| {
                if ui.add(primary_button(&format!("Antrikan Terpilih ({picked})"))).clicked() { enqueue = true; }
            });
        } else if !self.search.busy {
            ui.add_space(ui.available_height() / 4.0);
            ui.vertical_centered(|ui| { ui.label(RichText::new("Belum ada hasil pencarian").color(FG_SECONDARY)); });
        }

        if go {
            self.search.busy = true;
            self.search.err = None;
            let chosen: Vec<Provider> = self.search.providers.iter().filter(|(_, on)| *on).map(|(p, _)| *p).collect();
            let (m, tx, ctx, q) = (self.manager.clone(), self.tx.clone(), self.ctx.clone(), self.search.query.clone());
            self.rt.spawn(async move {
                let r = m.search_files(&q, &chosen).await.map_err(|e| format!("{}: {}", e.code, e.message));
                let _ = tx.send(Msg::Search(r));
                ctx.request_repaint();
            });
        }
        if enqueue {
            let hits = match &self.search.result {
                Some(r) => r.hits.iter().zip(&self.search.checked).filter(|(_, on)| **on).map(|(h, _)| h.clone()).collect(),
                None => Vec::new(),
            };
            // Fetching a .torrent can take a moment; the dialog-style block_on is bounded by the HTTP timeouts.
            let out = self.rt.block_on(self.manager.clone().enqueue_hits(hits, 0));
            let mut msg = format!("{} berkas diantrikan.", out.created.len());
            if let Some((t, e)) = out.failed.first() {
                msg += &format!(" {} gagal (mis. {t}: {e}).", out.failed.len());
            }
            self.notice = Some((msg, Instant::now()));
            self.last_refresh = Instant::now() - Duration::from_secs(10);
            if out.failed.is_empty() {
                self.search.checked.iter_mut().for_each(|c| *c = false);
                self.screen = Screen::Downloads;
            }
        }
    }
}


/// Percent-encode everything but unreserved characters (RFC 3986), for a query value.
fn percent_encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Open an https URL in the default browser. No shell parsing: the URL is one argument.
fn open_in_browser(url: &str) {
    if url.starts_with("https://") {
        let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    }
}
