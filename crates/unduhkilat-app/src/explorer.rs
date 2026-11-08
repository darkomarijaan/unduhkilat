//! File explorer for saved servers (SFTP, FTP, FTPS): two panels (this computer / the server),
//! transfers through the queue, and file previews. Child module of `app` (shares its private state).

use super::{App, Msg};
use crate::format;
use crate::theme::{self, *};
use eframe::egui::{self, Align, Layout, RichText, Ui};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use unduhkilat_core::error::AppError;
use unduhkilat_core::preview::{self, Block, Kind, Limits, PageImage, Preview, Sheet};
use unduhkilat_core::server::fs::{join_remote, parent_remote, safe_local_name, sort_entries, Entry, RemoteFs, SortKey};

#[path = "explorer/pair.rs"]
mod pair;
use pair::{CopyJob, LeftServer, PairMsg};
pub use pair::LEFT_BIT;

pub type SharedFs = Arc<Mutex<Option<Box<dyn RemoteFs>>>>;

/// High bit marks an explorer connection in `ServersState::shell_waiters` (tab ids are small).
pub const EXPLORER_BIT: u64 = 1 << 62;
/// Marks a picture-paste upload in `shell_waiters`.
pub const PASTE_BIT: u64 = 1 << 61;

/// A random 64-bit number for file names (the standard library seeds its hasher from the OS).
fn random_nonce() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new().build_hasher().finish()
}

// ---- pure helpers (unit tested) ---------------------------------------------------------------

/// Breadcrumb parts of a `/`-separated server path: `("label", "path up to it")`.
pub fn crumbs(path: &str) -> Vec<(String, String)> {
    let mut out = vec![("/".to_string(), "/".to_string())];
    let mut acc = String::new();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        acc.push('/');
        acc.push_str(part);
        out.push((part.to_string(), acc.clone()));
    }
    out
}

/// Breadcrumb parts of a Windows folder path.
pub fn crumbs_local(path: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut acc = PathBuf::new();
    for c in path.components() {
        acc.push(c);
        let label = match c {
            std::path::Component::Prefix(p) => p.as_os_str().to_string_lossy().into_owned(),
            std::path::Component::RootDir => continue,
            other => other.as_os_str().to_string_lossy().into_owned(),
        };
        let target = if matches!(c, std::path::Component::Prefix(_)) { PathBuf::from(format!("{label}\\")) } else { acc.clone() };
        out.push((label, target));
    }
    out
}

/// What the list shows: filtered by name (case-insensitive) and sorted, folders first.
pub fn view_of(entries: &[Entry], key: SortKey, ascending: bool, filter: &str) -> Vec<Entry> {
    let f = filter.trim().to_lowercase();
    let mut v: Vec<Entry> = entries.iter().filter(|e| f.is_empty() || e.name.to_lowercase().contains(&f)).cloned().collect();
    sort_entries(&mut v, key, ascending);
    v
}

/// One line under a panel saying what its buttons will act on.
pub fn selection_summary(dirs: usize, files: usize) -> String {
    match (dirs, files) {
        (0, 0) => "Belum ada yang dipilih".to_string(),
        (0, f) => format!("{f} berkas dipilih"),
        (d, 0) => format!("{d} folder dipilih"),
        (d, f) => format!("{} dipilih: {d} folder, {f} berkas", d + f),
    }
}

/// How often the folder on this computer is read again while the explorer is open.
pub const LOCAL_REFRESH: std::time::Duration = std::time::Duration::from_secs(2);

pub fn local_is_stale(age: std::time::Duration) -> bool {
    age >= LOCAL_REFRESH
}

/// Names that already exist at the destination: the user must confirm replacing them.
pub fn overwrite_conflicts(names: &[String], existing: &BTreeSet<String>, case_insensitive: bool) -> Vec<String> {
    let lower: BTreeSet<String> = existing.iter().map(|e| e.to_lowercase()).collect();
    names.iter().filter(|n| if case_insensitive { lower.contains(&n.to_lowercase()) } else { existing.contains(*n) }).cloned().collect()
}

/// File types that must not be started straight from a server: programs, scripts, shortcuts, and
/// documents with macros. They can still be downloaded and opened on purpose.
pub fn is_unsafe_to_open(name: &str) -> bool {
    const RISKY: [&str; 36] = [
        "exe", "com", "bat", "cmd", "msi", "msp", "scr", "pif", "cpl", "lnk", "url", "hta", "js", "jse", "vbs", "vbe", "wsf", "wsh", "ps1", "psm1", "reg", "jar", "dll", "sys", "appx", "msix", "inf", "gadget", "docm", "xlsm", "pptm", "dotm", "xlam", "ppam", "iso", "chm",
    ];
    let ext = name.rsplit_once('.').map(|(_, e)| e.trim().to_ascii_lowercase()).unwrap_or_default();
    RISKY.contains(&ext.as_str())
}

/// Contents of the `Zone.Identifier` stream that marks a file as coming from the Internet, so Windows
/// opens Office documents in Protected View and warns before running anything.
pub const ZONE_IDENTIFIER: &str = "[ZoneTransfer]\r\nZoneId=3\r\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewChoice {
    /// Fetch and show it.
    Show,
    /// Too large: offer Download or Open in Windows app.
    TooLarge,
    /// Not previewable here: offer Open in Windows app and Download.
    External,
}

pub fn preview_choice(name: &str, size: u64, max_bytes: u64) -> PreviewChoice {
    match preview::kind_for(name) {
        Kind::Unknown | Kind::External => PreviewChoice::External,
        Kind::Text | Kind::Markdown => PreviewChoice::Show,
        _ if size > max_bytes => PreviewChoice::TooLarge,
        _ => PreviewChoice::Show,
    }
}

// ---- state ------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExStatus {
    Idle,
    Connecting,
    Ready,
    Failed(String),
}

pub enum ExDialog {
    NewFolder(String),
    Rename(Entry, String),
    ConfirmDelete(Vec<Entry>),
    /// Names that exist at the destination; replacing them needs a yes. `download` = server to computer.
    ConfirmOverwrite { download: bool, names: Vec<String>, all: Vec<String> },
    /// Copying between two servers: `names` already exist at the destination.
    ConfirmCopy { names: Vec<String>, jobs: Vec<CopyJob>, src_fs: Option<SharedFs> },
}

pub enum PvContent {
    Text { text: String, lines: Vec<(usize, usize)>, truncated: bool },
    Markdown { blocks: Vec<Block>, truncated: bool },
    Image(egui::TextureHandle),
    Pdf { pages: usize, page: usize, scale: f32, tex: Option<egui::TextureHandle>, bytes: Arc<Vec<u8>>, busy: bool },
    Doc(Vec<Block>),
    Sheets { sheets: Vec<Sheet>, active: usize },
}

pub enum PvState {
    Loading,
    /// `offer_open` = also offer "Buka di aplikasi Windows".
    Error { message: String, offer_open: bool },
    Ready(PvContent),
}

pub struct PreviewView {
    pub entry: Entry,
    pub state: PvState,
    pub text_bytes: usize,
}

pub struct ExplorerState {
    pub server_id: String,
    pub status: ExStatus,
    pub fs: Option<SharedFs>,
    pub cancel: Option<CancellationToken>,
    pub dir: String,
    pub entries: Vec<Entry>,
    pub loading: bool,
    pub error: Option<String>,
    pub gen: u64,
    pub sort: (SortKey, bool),
    pub filter: String,
    pub selected: BTreeSet<String>,
    pub local_dir: PathBuf,
    pub local_entries: Vec<Entry>,
    pub local_loaded: Option<PathBuf>,
    pub local_loaded_at: Option<std::time::Instant>,
    /// A file that just finished downloading into the folder on the left: select it and scroll to it.
    pub highlight: Option<(String, u8)>,
    pub local_scroll_to: Option<usize>,
    pub local_selected: BTreeSet<String>,
    pub dialog: Option<ExDialog>,
    pub preview: Option<PreviewView>,
    pub max_preview_mb: u64,
    /// Set when the connection was blocked because the pinned key or certificate changed.
    pub changed_key: Option<(String, u16)>,
    /// Cached sorted and filtered server listing: (entries gen, sort, filter) -> view.
    pub view_cache: Option<((u64, SortKey, bool, String), Vec<Entry>)>,
    /// The left panel shows this server instead of this computer.
    pub left: Option<LeftServer>,
    /// A file that just arrived in this folder: select it and scroll to it once listed.
    pub reveal: Option<String>,
    pub scroll_to: Option<String>,
}

impl ExplorerState {
    pub fn new(server_id: &str, local_dir: PathBuf, max_preview_mb: u64) -> Self {
        ExplorerState {
            server_id: server_id.to_string(),
            status: ExStatus::Idle,
            fs: None,
            cancel: None,
            dir: "/".into(),
            entries: Vec::new(),
            loading: false,
            error: None,
            gen: 0,
            sort: (SortKey::Name, true),
            filter: String::new(),
            selected: BTreeSet::new(),
            local_dir,
            local_entries: Vec::new(),
            local_loaded: None,
            local_loaded_at: None,
            highlight: None,
            local_scroll_to: None,
            local_selected: BTreeSet::new(),
            dialog: None,
            preview: None,
            max_preview_mb,
            changed_key: None,
            view_cache: None,
            left: None,
            reveal: None,
            scroll_to: None,
        }
    }

    pub fn close(&mut self, rt: &tokio::runtime::Handle) {
        if let Some(mut l) = self.left.take() {
            l.close(rt);
        }
        if let Some(c) = self.cancel.take() {
            c.cancel();
        }
        if let Some(fs) = self.fs.take() {
            rt.spawn(async move {
                if let Some(f) = fs.lock().await.take() {
                    f.close().await;
                }
            });
        }
    }
}

pub enum ExplorerMsg {
    Connected { tab_id: u64, result: Result<Box<dyn RemoteFs>, AppError> },
    Listed { tab_id: u64, gen: u64, dir: String, result: Result<Vec<Entry>, AppError> },
    OpDone { tab_id: u64, what: &'static str, result: Result<(), AppError> },
    Previewed { tab_id: u64, path: String, result: Result<Preview, AppError> },
    PdfPage { tab_id: u64, path: String, page: usize, result: Result<PageImage, AppError> },
    Opened(Result<PathBuf, AppError>),
    /// The server shown in the left panel (copying between servers).
    Pair(PairMsg),
    /// A pasted picture reached the server at this path.
    ImagePasted { tab_id: u64, result: Result<String, AppError> },
}

/// Answers every question with "no": used where nobody is there to ask.
struct NoAnswer;

impl unduhkilat_core::server::ssh::Interact for NoAnswer {
    fn confirm_host_key<'a>(&'a self, _h: &'a str, _p: u16, _t: &'a str, _f: &'a str) -> futures_util::future::BoxFuture<'a, bool> {
        Box::pin(async { false })
    }
    fn secret<'a>(&'a self, _n: &'a str, _k: unduhkilat_core::server::secrets::SecretKind) -> futures_util::future::BoxFuture<'a, Option<unduhkilat_core::server::secrets::SecretString>> {
        Box::pin(async { None })
    }
}

fn texture(ctx: &egui::Context, name: &str, img: &PageImage) -> egui::TextureHandle {
    ctx.load_texture(name, egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.rgba), egui::TextureOptions::LINEAR)
}

fn line_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            out.push((start, i));
            start = i + 1;
        }
    }
    if start < text.len() {
        out.push((start, text.len()));
    }
    out
}

fn read_local_dir(dir: &Path) -> Vec<Entry> {
    let mut out: Vec<Entry> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .take(5000)
                .filter_map(|e| {
                    let m = e.metadata().ok()?;
                    let name = e.file_name().to_string_lossy().into_owned();
                    let modified = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
                    Some(Entry { path: e.path().to_string_lossy().into_owned(), name, is_dir: m.is_dir(), size: m.len(), modified, is_link: false })
                })
                .collect()
        })
        .unwrap_or_default();
    sort_entries(&mut out, SortKey::Name, true);
    out
}

fn preview_temp_dir() -> PathBuf {
    std::env::temp_dir().join("UnduhKilat-pratinjau")
}

/// Remove files left by earlier previews (called when the explorer opens and when the app exits).
pub fn clean_preview_temp() {
    let _ = std::fs::remove_dir_all(preview_temp_dir());
}

// ---- actions on App ---------------------------------------------------------------------------

impl App {
    fn ex_send(&self, m: ExplorerMsg) {
        let _ = self.tx.send(Msg::Explorer(m));
        self.ctx.request_repaint();
    }

    /// A tab with only the file explorer of a saved server (the way to browse FTP and FTPS).
    pub fn open_files_tab(&mut self, server_id: &str) {
        let Some(server) = self.servers.servers.iter().find(|s| s.id == server_id).cloned() else { return };
        let id = self.terminal.next_tab_id();
        let tab = super::terminal::TermTab::files(id, &server.name, &format!("berkas · {}@{}", server.username, server.host), server_id);
        self.terminal.tabs.push(tab);
        self.terminal.active = self.terminal.tabs.len() - 1;
        self.screen = super::Screen::Terminal;
    }

    pub(super) fn new_explorer(&mut self, server_id: &str) -> ExplorerState {
        static CLEANED: std::sync::Once = std::sync::Once::new();
        CLEANED.call_once(clean_preview_temp);
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
        let local = super::terminal::start_dir(None, home.as_deref());
        let db = self.manager.db();
        let mb = db.read(|c| unduhkilat_core::repo::get_setting(c, unduhkilat_core::config::keys::PREVIEW_MAX_MB)).ok().flatten().and_then(|v| v.parse::<u64>().ok()).filter(|v| *v >= 1).unwrap_or(25);
        ExplorerState::new(server_id, local, mb)
    }

    /// A download finished: when it landed in the folder an explorer shows on the left, mark it there.
    pub(super) fn highlight_downloaded(&mut self, target: &str) {
        let target = PathBuf::from(target);
        for t in self.terminal.tabs.iter_mut() {
            if let Some(ex) = t.explorer.as_mut() {
                // The item directly inside the folder on the left: the file itself, or the folder it is in.
                let top = target.strip_prefix(&ex.local_dir).ok().and_then(|rel| rel.components().next()).map(|c| c.as_os_str().to_string_lossy().into_owned());
                if let Some(name) = top {
                    ex.highlight = Some((name, 0));
                    ex.local_loaded_at = None;
                }
            }
        }
    }

    pub(super) fn explorer_connect(&mut self, tab_id: u64, ex: &mut ExplorerState) {
        if let Some(c) = ex.cancel.take() {
            c.cancel();
        }
        let cancel = CancellationToken::new();
        ex.cancel = Some(cancel.clone());
        ex.status = ExStatus::Connecting;
        ex.error = None;
        self.servers.shell_waiters.insert(tab_id | EXPLORER_BIT);
        let (svc, tx, ctx, sid) = (self.manager.servers(), self.tx.clone(), self.ctx.clone(), ex.server_id.clone());
        let interact = Arc::new(super::servers::UiInteract { tx: tx.clone(), ctx: ctx.clone() });
        self.rt.spawn(async move {
            let result = svc.open_fs(&sid, interact, cancel).await;
            let _ = tx.send(Msg::Explorer(ExplorerMsg::Connected { tab_id, result }));
            ctx.request_repaint();
        });
    }

    pub(super) fn explorer_list(&mut self, tab_id: u64, ex: &mut ExplorerState, dir: &str) {
        let Some(fs) = ex.fs.clone() else { return };
        ex.gen += 1;
        ex.loading = true;
        ex.error = None;
        let (gen, dir) = (ex.gen, dir.to_string());
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.rt.spawn(async move {
            let result = match fs.lock().await.as_mut() {
                Some(f) => f.list(&dir).await,
                None => Err(AppError::new(unduhkilat_core::error::ErrorCode::ErrConnectionDropped, "Sambungan sudah ditutup.")),
            };
            let _ = tx.send(Msg::Explorer(ExplorerMsg::Listed { tab_id, gen, dir, result }));
            ctx.request_repaint();
        });
    }

    fn explorer_op<F>(&mut self, tab_id: u64, ex: &ExplorerState, what: &'static str, op: F)
    where
        F: for<'a> FnOnce(&'a mut dyn RemoteFs) -> futures_util::future::BoxFuture<'a, Result<(), AppError>> + Send + 'static,
    {
        let Some(fs) = ex.fs.clone() else { return };
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.rt.spawn(async move {
            let result = match fs.lock().await.as_mut() {
                Some(f) => op(f.as_mut()).await,
                None => Err(AppError::new(unduhkilat_core::error::ErrorCode::ErrConnectionDropped, "Sambungan sudah ditutup.")),
            };
            let _ = tx.send(Msg::Explorer(ExplorerMsg::OpDone { tab_id, what, result }));
            ctx.request_repaint();
        });
    }

    pub(super) fn explorer_msg(&mut self, m: ExplorerMsg) {
        match m {
            ExplorerMsg::Pair(m) => self.pair_msg(m),
            ExplorerMsg::Opened(Ok(path)) => {
                let _ = std::process::Command::new("explorer").arg(&path).spawn();
            }
            ExplorerMsg::Opened(Err(e)) => self.notice = Some((e.message, std::time::Instant::now())),
            ExplorerMsg::ImagePasted { tab_id, result: Ok(path) } => {
                self.servers.shell_waiters.remove(&(tab_id | PASTE_BIT));
                self.servers.clear_prompts_if_idle();
                self.terminal.pasted.entry(tab_id).or_default().push(path.clone());
                if let Some(s) = self.terminal.tabs.iter().find(|t| t.id == tab_id).and_then(|t| t.session.as_ref()) {
                    s.send(format!("{path} ").into_bytes());
                }
            }
            ExplorerMsg::ImagePasted { tab_id, result: Err(e) } => {
                self.servers.shell_waiters.remove(&(tab_id | PASTE_BIT));
                self.servers.clear_prompts_if_idle();
                self.notice = Some((e.message, std::time::Instant::now()));
            }
            ExplorerMsg::Connected { tab_id, result } => {
                self.servers.shell_waiters.remove(&(tab_id | EXPLORER_BIT));
                self.servers.clear_prompts_if_idle();
                let Some(mut ex) = self.take_explorer(tab_id) else {
                    if let Ok(fs) = result {
                        self.rt.spawn(fs.close());
                    }
                    return;
                };
                match result {
                    Ok(fs) => {
                        ex.fs = Some(Arc::new(Mutex::new(Some(fs))));
                        ex.status = ExStatus::Ready;
                        // Test aid: `UNDUHKILAT_LEFT=<server name>` shows that server on the left.
                        if let (Ok(name), None) = (std::env::var("UNDUHKILAT_LEFT"), ex.left.as_ref()) {
                            if let Some(s) = self.servers.servers.iter().find(|s| s.name == name) {
                                let (id, n) = (s.id.clone(), s.name.clone());
                                self.pair_choose(tab_id, &mut ex, Some((id, n)));
                            }
                        }
                        let dir = ex.dir.clone();
                        self.explorer_list_start(tab_id, &mut ex, dir, true);
                    }
                    Err(e) => {
                        ex.changed_key = e.details.as_ref().filter(|_| e.code == unduhkilat_core::error::ErrorCode::ErrHostKeyChanged).and_then(|d| Some((d.get("host")?.as_str()?.to_string(), d.get("port")?.as_u64()? as u16)));
                        ex.status = ExStatus::Failed(e.message);
                    }
                }
                self.put_explorer(tab_id, ex);
            }
            ExplorerMsg::Listed { tab_id, gen, dir, result } => {
                let Some(ex) = self.explorer_mut(tab_id) else { return };
                if gen != ex.gen {
                    return; // a newer listing was asked for
                }
                ex.loading = false;
                let mut auto_preview = None;
                match result {
                    Ok(v) => {
                        ex.dir = dir;
                        ex.entries = v;
                        // The cached view is keyed by the request number, which changes before the answer
                        // arrives: a frame in between would keep showing the old folder.
                        ex.view_cache = None;
                        if let Some(p) = ex.reveal.take() {
                            if ex.entries.iter().any(|e| e.path == p) {
                                ex.selected = [p.clone()].into();
                                ex.scroll_to = Some(p);
                            }
                        }
                        ex.selected.clear();
                        // Test aid: `UNDUHKILAT_PREVIEW=<file name>` previews that file once listed.
                        if let (Ok(name), None) = (std::env::var("UNDUHKILAT_PREVIEW"), ex.preview.as_ref()) {
                            auto_preview = ex.entries.iter().find(|e| e.name == name).cloned();
                        }
                    }
                    Err(e) => ex.error = Some(e.message),
                }
                if let Some(entry) = auto_preview {
                    if let Some(mut ex) = self.take_explorer(tab_id) {
                        self.explorer_start_preview(tab_id, &mut ex, entry, 1024 * 1024);
                        self.put_explorer(tab_id, ex);
                    }
                }
            }
            ExplorerMsg::OpDone { tab_id, what, result } => {
                match result {
                    Ok(()) => {
                        if let Some(mut ex) = self.take_explorer(tab_id) {
                            let dir = ex.dir.clone();
                            self.explorer_list_start(tab_id, &mut ex, dir, false);
                            self.put_explorer(tab_id, ex);
                        }
                    }
                    Err(e) => self.notice = Some((format!("{what}: {}", e.message), std::time::Instant::now())),
                }
            }
            ExplorerMsg::Previewed { tab_id, path, result } => {
                let ctx = self.ctx.clone();
                let Some(ex) = self.explorer_mut(tab_id) else { return };
                let Some(pv) = ex.preview.as_mut() else { return };
                if pv.entry.path != path {
                    return;
                }
                pv.state = match result {
                    Err(e) => PvState::Error { offer_open: matches!(e.code, unduhkilat_core::error::ErrorCode::ErrPreviewUnsupported | unduhkilat_core::error::ErrorCode::ErrPreviewTooLarge), message: e.message },
                    Ok(Preview::Text { text, truncated }) => PvState::Ready(PvContent::Text { lines: line_ranges(&text), text, truncated }),
                    Ok(Preview::Markdown { blocks, truncated }) => PvState::Ready(PvContent::Markdown { blocks, truncated }),
                    Ok(Preview::Image(img)) => PvState::Ready(PvContent::Image(texture(&ctx, &path, &img))),
                    Ok(Preview::Pdf { pages, first, bytes }) => PvState::Ready(PvContent::Pdf { pages, page: 0, scale: 1.5, tex: Some(texture(&ctx, &path, &first)), bytes: Arc::new(bytes), busy: false }),
                    Ok(Preview::Doc(b)) => PvState::Ready(PvContent::Doc(b)),
                    Ok(Preview::Sheets(s)) => PvState::Ready(PvContent::Sheets { sheets: s, active: 0 }),
                };
            }
            ExplorerMsg::PdfPage { tab_id, path, page, result } => {
                let ctx = self.ctx.clone();
                let Some(ex) = self.explorer_mut(tab_id) else { return };
                let Some(PreviewView { state: PvState::Ready(PvContent::Pdf { page: cur, tex, busy, .. }), entry, .. }) = ex.preview.as_mut() else { return };
                if entry.path != path {
                    return; // a page of a PDF that is no longer shown
                }
                *busy = false;
                if let Ok(img) = result {
                    *tex = Some(texture(&ctx, &format!("{}#{page}", entry.path), &img));
                    *cur = page;
                }
            }
        }
    }

    /// `first`: right after connecting, start in the server's home folder (falling back to `dir`).
    fn explorer_list_start(&mut self, tab_id: u64, ex: &mut ExplorerState, dir: String, first: bool) {
        if !first {
            self.explorer_list(tab_id, ex, &dir);
            return;
        }
        let Some(fs) = ex.fs.clone() else { return };
        ex.gen += 1;
        ex.loading = true;
        ex.error = None;
        let gen = ex.gen;
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.rt.spawn(async move {
            let (dir, result) = match fs.lock().await.as_mut() {
                Some(f) => {
                    let start = match f.home().await {
                        Ok(h) if h.starts_with('/') => h,
                        _ => dir,
                    };
                    let r = f.list(&start).await;
                    match r {
                        Ok(v) => (start, Ok(v)),
                        Err(_) if start != "/" => (String::from("/"), f.list("/").await),
                        Err(e) => (start, Err(e)),
                    }
                }
                None => (dir, Err(AppError::new(unduhkilat_core::error::ErrorCode::ErrConnectionDropped, "Sambungan sudah ditutup."))),
            };
            let _ = tx.send(Msg::Explorer(ExplorerMsg::Listed { tab_id, gen, dir, result }));
            ctx.request_repaint();
        });
    }

    fn explorer_mut(&mut self, tab_id: u64) -> Option<&mut ExplorerState> {
        self.terminal.tabs.iter_mut().find(|t| t.id == tab_id).and_then(|t| t.explorer.as_mut())
    }

    /// While a frame draws a tab the explorer is held outside the tab; messages arrive between frames.
    fn take_explorer(&mut self, tab_id: u64) -> Option<ExplorerState> {
        self.terminal.tabs.iter_mut().find(|t| t.id == tab_id).and_then(|t| t.explorer.take())
    }

    fn put_explorer(&mut self, tab_id: u64, ex: ExplorerState) {
        if let Some(t) = self.terminal.tabs.iter_mut().find(|t| t.id == tab_id) {
            t.explorer = Some(ex);
        }
    }

    /// Queue the named items of the server folder for download into the folder on the left. A folder
    /// brings every file under it; inside a folder an existing file is reported, never replaced.
    fn explorer_enqueue_downloads(&mut self, ex: &ExplorerState, names: &[String], overwrite: bool) {
        let (m, tx, ctx) = (self.manager.clone(), self.tx.clone(), self.ctx.clone());
        let (sid, dir, local, fs) = (ex.server_id.clone(), ex.dir.clone(), ex.local_dir.clone(), ex.fs.clone());
        let items: Vec<(String, bool)> = names.iter().map(|n| (n.clone(), ex.entries.iter().find(|e| &e.name == n).map(|e| e.is_dir).unwrap_or(false))).collect();
        self.rt.spawn(async move {
            let (mut ok, mut failed) = (0, Vec::new());
            for (n, is_dir) in items {
                let path = join_remote(&dir, &n);
                if is_dir {
                    let Some(fs) = &fs else { continue };
                    match fs.lock().await.as_mut() {
                        Some(f) => match m.enqueue_folder_download(f.as_mut(), &sid, &path, &local, false).await {
                            Ok(r) => {
                                ok += r.queued;
                                failed.extend(r.failed);
                            }
                            Err(e) => failed.push(format!("{n}: {}", e.message)),
                        },
                        None => failed.push(format!("{n}: sambungan sudah ditutup")),
                    }
                    continue;
                }
                match m.enqueue_remote_download(&sid, &path, &local, overwrite).await {
                    Ok(_) => ok += 1,
                    Err(e) => failed.push(format!("{n}: {}", e.message)),
                }
            }
            let msg = if failed.is_empty() { format!("{ok} unduhan ditambahkan ke antrean.") } else { format!("{ok} ditambahkan; gagal: {}", failed.iter().take(3).cloned().collect::<Vec<_>>().join("; ")) };
            let _ = tx.send(Msg::Notice(msg));
            ctx.request_repaint();
        });
    }

    /// Download one file by its own full path (the preview may stay open while the folder changes).
    fn explorer_enqueue_download_path(&mut self, ex: &ExplorerState, entry: &Entry) {
        let (m, tx, ctx) = (self.manager.clone(), self.tx.clone(), self.ctx.clone());
        let (sid, local, path) = (ex.server_id.clone(), ex.local_dir.clone(), entry.path.clone());
        self.rt.spawn(async move {
            let msg = match m.enqueue_remote_download(&sid, &path, &local, false).await {
                Ok(_) => "1 unduhan ditambahkan ke antrean.".to_string(),
                Err(e) => e.message,
            };
            let _ = tx.send(Msg::Notice(msg));
            ctx.request_repaint();
        });
    }

    /// Queue the named items of the folder on the left for upload to the server folder. A folder brings
    /// every file under it; inside a folder an existing file is reported, never replaced.
    fn explorer_enqueue_uploads(&mut self, ex: &ExplorerState, names: &[String], overwrite: bool) {
        let (m, tx, ctx) = (self.manager.clone(), self.tx.clone(), self.ctx.clone());
        let (sid, dir, local) = (ex.server_id.clone(), ex.dir.clone(), ex.local_dir.clone());
        let items: Vec<(String, bool)> = names.iter().map(|n| (n.clone(), ex.local_entries.iter().find(|e| &e.name == n).map(|e| e.is_dir).unwrap_or(false))).collect();
        self.rt.spawn(async move {
            let (mut ok, mut failed) = (0, Vec::new());
            for (n, is_dir) in items {
                if is_dir {
                    match m.enqueue_folder_upload(&sid, &local.join(&n), &dir, false).await {
                        Ok(r) => {
                            ok += r.queued;
                            failed.extend(r.failed);
                        }
                        Err(e) => failed.push(format!("{n}: {}", e.message)),
                    }
                    continue;
                }
                match m.enqueue_remote_upload(&sid, &local.join(&n), &dir, overwrite).await {
                    Ok(_) => ok += 1,
                    Err(e) => failed.push(format!("{n}: {}", e.message)),
                }
            }
            let msg = if failed.is_empty() { format!("{ok} unggahan ditambahkan ke antrean.") } else { format!("{ok} ditambahkan; gagal: {}", failed.iter().take(3).cloned().collect::<Vec<_>>().join("; ")) };
            let _ = tx.send(Msg::Notice(msg));
            ctx.request_repaint();
        });
    }

    fn explorer_start_preview(&mut self, tab_id: u64, ex: &mut ExplorerState, entry: Entry, text_bytes: usize) {
        let max = ex.max_preview_mb * 1024 * 1024;
        match preview_choice(&entry.name, entry.size, max) {
            PreviewChoice::TooLarge => {
                ex.preview = Some(PreviewView { text_bytes, state: PvState::Error { message: format!("Berkas {} melebihi batas pratinjau {} MB.", format::bytes(entry.size as i64), ex.max_preview_mb), offer_open: true }, entry });
                return;
            }
            PreviewChoice::External => {
                ex.preview = Some(PreviewView { text_bytes, state: PvState::Error { message: "Format ini tidak bisa dipratinjau di sini.".into(), offer_open: true }, entry });
                return;
            }
            PreviewChoice::Show => {}
        }
        let Some(fs) = ex.fs.clone() else { return };
        let (path, size) = (entry.path.clone(), entry.size);
        ex.preview = Some(PreviewView { entry, state: PvState::Loading, text_bytes });
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        let limits = Limits { max_bytes: max, text_bytes };
        self.rt.spawn(async move {
            let result = match fs.lock().await.as_mut() {
                Some(f) => preview::load(f.as_mut(), &path, size, &limits).await,
                None => Err(AppError::new(unduhkilat_core::error::ErrorCode::ErrConnectionDropped, "Sambungan sudah ditutup.")),
            };
            let _ = tx.send(Msg::Explorer(ExplorerMsg::Previewed { tab_id, path, result }));
            ctx.request_repaint();
        });
    }

    fn explorer_open_external(&mut self, ex: &ExplorerState, entry: &Entry) {
        let Some(fs) = ex.fs.clone() else { return };
        let Some(name) = safe_local_name(&entry.name) else { return };
        if is_unsafe_to_open(&name) {
            self.notice = Some(("Jenis berkas ini bisa menjalankan program atau makro, jadi tidak dibuka langsung dari server. Unduh dulu, lalu buka sendiri bila Anda yakin.".into(), std::time::Instant::now()));
            return;
        }
        let (tx, ctx, path, size) = (self.tx.clone(), self.ctx.clone(), entry.path.clone(), entry.size);
        let max = ex.max_preview_mb.max(25) * 1024 * 1024 * 4;
        self.rt.spawn(async move {
            let result = async {
                if size > max {
                    return Err(AppError::new(unduhkilat_core::error::ErrorCode::ErrPreviewTooLarge, "Berkas terlalu besar untuk dibuka sementara. Gunakan Unduh."));
                }
                let dir = preview_temp_dir();
                std::fs::create_dir_all(&dir).map_err(|e| AppError::internal(e.to_string()))?;
                let target = dir.join(&name);
                let mut guard = fs.lock().await;
                let f = guard.as_mut().ok_or_else(|| AppError::new(unduhkilat_core::error::ErrorCode::ErrConnectionDropped, "Sambungan sudah ditutup."))?;
                let mut reader = f.open_read(&path, 0).await?;
                let mut out = tokio::fs::File::create(&target).await.map_err(|e| AppError::internal(e.to_string()))?;
                // Never copy more than the size we were told (a server may lie).
                let mut limited = tokio::io::AsyncReadExt::take(&mut reader, size.min(max) + 1);
                let copied = tokio::io::copy(&mut limited, &mut out).await.map_err(|e| AppError::new(unduhkilat_core::error::ErrorCode::ErrConnectionDropped, e.to_string()))?;
                if copied > size.min(max) {
                    return Err(AppError::new(unduhkilat_core::error::ErrorCode::ErrPreviewTooLarge, "Berkas lebih besar dari yang dilaporkan server; dihentikan."));
                }
                drop(out);
                // Mark it as downloaded from the Internet (Protected View, SmartScreen).
                let _ = std::fs::write(format!("{}:Zone.Identifier", target.display()), ZONE_IDENTIFIER);
                Ok(target)
            }
            .await;
            let _ = tx.send(Msg::Explorer(ExplorerMsg::Opened(result)));
            ctx.request_repaint();
        });
    }

    /// Alt+V in an SSH tab: take the picture from the Windows clipboard, upload it with SFTP to a
    /// temporary folder on the server, and type its path (what Claude Code reads a picture from).
    pub(super) fn paste_clipboard_image(&mut self, tab_id: u64, server_id: String) {
        let (svc, tx, ctx) = (self.manager.servers(), self.tx.clone(), self.ctx.clone());
        let interact = Arc::new(super::servers::UiInteract { tx: tx.clone(), ctx: ctx.clone() });
        self.notice = Some(("Mengunggah gambar dari papan klip...".into(), std::time::Instant::now()));
        // Questions (host key, password) from this connection must reach the user.
        self.servers.shell_waiters.insert(tab_id | PASTE_BIT);
        self.rt.spawn(async move {
            let result: Result<String, AppError> = async {
                let png = tokio::task::spawn_blocking(|| {
                    let img = arboard::Clipboard::new().and_then(|mut c| c.get_image()).map_err(|_| AppError::new(unduhkilat_core::error::ErrorCode::ErrInvalidArgument, "Papan klip tidak berisi gambar."))?;
                    unduhkilat_core::server::fs::encode_png_rgba(img.width as u32, img.height as u32, &img.bytes)
                })
                .await
                .map_err(|_| AppError::internal("pembacaan papan klip berhenti"))??;
                let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
                let name = unduhkilat_core::server::fs::temp_image_name(millis, random_nonce());
                let mut fs = svc.open_fs(&server_id, interact, CancellationToken::new()).await?;
                // Inside the login's own home: not readable or plantable by other users of the server.
                let home = fs.home().await?;
                let dir = join_remote(&home, unduhkilat_core::server::fs::PASTE_SUBDIR);
                let _ = fs.mkdir(&dir).await; // exists already, usually
                let path = join_remote(&dir, &name);
                let outcome: Result<(), AppError> = async {
                    use tokio::io::AsyncWriteExt;
                    let mut w = fs.open_write(&path, 0, false).await?;
                    w.write_all(&png).await.map_err(|e| AppError::new(unduhkilat_core::error::ErrorCode::ErrConnectionDropped, e.to_string()))?;
                    w.shutdown().await.map_err(|e| AppError::new(unduhkilat_core::error::ErrorCode::ErrConnectionDropped, e.to_string()))?;
                    Ok(())
                }
                .await;
                fs.close().await;
                outcome.map(|_| path)
            }
            .await;
            let _ = tx.send(Msg::Explorer(ExplorerMsg::ImagePasted { tab_id, result }));
            ctx.request_repaint();
        });
    }

    /// Delete the pictures pasted from a tab when it closes (best effort; they live in /tmp anyway).
    pub(super) fn cleanup_pasted_images(&mut self, tab_id: u64) {
        let Some(paths) = self.terminal.pasted.remove(&tab_id) else { return };
        let Some(sid) = self.terminal.tabs.iter().find(|t| t.id == tab_id).and_then(|t| if let super::terminal::TabKind::Ssh { server_id } = &t.kind { Some(server_id.clone()) } else { None }) else { return };
        let svc = self.manager.servers();
        self.rt.spawn(async move {
            // Nobody is there to answer a question during cleanup: it never prompts.
            let quiet: Arc<dyn unduhkilat_core::server::ssh::Interact> = Arc::new(NoAnswer);
            if let Ok(mut fs) = svc.open_fs(&sid, quiet, CancellationToken::new()).await {
                for p in paths {
                    let _ = fs.remove(&p, false).await;
                }
                fs.close().await;
            }
        });
    }

    fn explorer_pdf_page(&mut self, tab_id: u64, ex: &mut ExplorerState, page: usize, scale: f32) {
        let Some(PreviewView { state: PvState::Ready(PvContent::Pdf { bytes, busy, scale: sc, .. }), .. }) = ex.preview.as_mut() else { return };
        if *busy {
            return;
        }
        *busy = true;
        *sc = scale;
        let (bytes, tx, ctx) = (bytes.clone(), self.tx.clone(), self.ctx.clone());
        let path = ex.preview.as_ref().map(|p| p.entry.path.clone()).unwrap_or_default();
        self.rt.spawn(async move {
            let result = tokio::task::spawn_blocking(move || preview::pdf::render_page(&bytes, page, scale)).await.unwrap_or_else(|_| Err(AppError::internal("render PDF berhenti")));
            let _ = tx.send(Msg::Explorer(ExplorerMsg::PdfPage { tab_id, path, page, result }));
            ctx.request_repaint();
        });
    }

    // ---- drawing ----------------------------------------------------------------------------------

    /// The explorer of one tab. The state is held outside the tab while it is drawn.
    pub(super) fn explorer_ui(&mut self, ui: &mut Ui, tab_id: u64, ex: &mut ExplorerState) {
        match ex.status.clone() {
            ExStatus::Idle => {
                self.explorer_connect(tab_id, ex);
            }
            ExStatus::Connecting => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Menghubungkan ke server...").color(FG_SECONDARY));
                    if ui.button("Batal").clicked() {
                        ex.close(&self.rt);
                        ex.status = ExStatus::Failed("Dibatalkan.".into());
                    }
                });
                return;
            }
            ExStatus::Failed(msg) => {
                ui.label(RichText::new(&msg).color(DANGER));
                if ui.button("Coba lagi").clicked() {
                    self.explorer_connect(tab_id, ex);
                }
                if let Some((host, port)) = ex.changed_key.clone() {
                    ui.label(RichText::new("Bila Anda yakin server ini memang diganti (mis. sertifikat diperbarui), percayai yang baru:").color(FG_SECONDARY));
                    if ui.add(egui::Button::new(RichText::new("Lupakan yang lama dan percayai yang baru").color(egui::Color32::WHITE)).fill(DANGER)).clicked() {
                        let (svc, rt) = (self.manager.servers(), self.rt.clone());
                        // The forgetting finishes before the next connection attempt asks about the new one.
                        let _ = rt.block_on(svc.forget_host_key(&host, port));
                        ex.changed_key = None;
                        self.explorer_connect(tab_id, ex);
                    }
                }
                return;
            }
            ExStatus::Ready => {}
        }
        self.explorer_dialogs(ui.ctx(), tab_id, ex);
        let wide = ui.available_width() > 760.0;
        let preview_open = ex.preview.is_some();
        if preview_open {
            let h = (ui.available_height() * 0.5).max(200.0);
            egui::Frame::new().fill(SURFACE).stroke(egui::Stroke::new(1.0, BORDER)).corner_radius(6).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_height(h);
                self.preview_ui(ui, tab_id, ex);
            });
            ui.add_space(6.0);
        }
        if wide {
            ui.columns(2, |cols| {
                self.local_panel(&mut cols[0], tab_id, ex);
                self.server_panel(&mut cols[1], tab_id, ex);
            });
        } else {
            self.server_panel(ui, tab_id, ex);
        }
    }

    fn local_panel(&mut self, ui: &mut Ui, tab_id: u64, ex: &mut ExplorerState) {
        if ex.left.is_some() {
            self.pair_left_ui(ui, tab_id, ex);
            return;
        }
        let mut choice = None;
        ui.horizontal(|ui| {
            choice = self.source_picker(ui, ex);
            if ui.small_button("Muat ulang").clicked() {
                ex.local_loaded_at = None;
            }
        });
        if let Some(c) = choice {
            self.pair_choose(tab_id, ex, c);
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for (label, path) in crumbs_local(&ex.local_dir) {
                if ui.small_button(&label).clicked() {
                    ex.local_dir = path;
                    ex.local_selected.clear();
                }
            }
        });
        let reload = ex.local_loaded.as_deref() != Some(ex.local_dir.as_path()) || ex.local_loaded_at.map(|t| local_is_stale(t.elapsed())).unwrap_or(true);
        if reload {
            ex.local_entries = read_local_dir(&ex.local_dir);
            ex.local_loaded = Some(ex.local_dir.clone());
            ex.local_loaded_at = Some(std::time::Instant::now());
            let names: BTreeSet<&String> = ex.local_entries.iter().map(|e| &e.name).collect();
            ex.local_selected.retain(|n| names.contains(n));
        }
        if let Some((name, tries)) = ex.highlight.take() {
            if let Some(i) = ex.local_entries.iter().position(|e| e.name == name) {
                ex.local_selected = [name].into();
                ex.local_scroll_to = Some(i + 1);
            } else if tries < 5 {
                ex.highlight = Some((name, tries + 1));
                ex.local_loaded_at = None;
            }
        }
        // New files (a finished download) show up without clicking anything.
        ui.ctx().request_repaint_after(LOCAL_REFRESH);
        let h = (ui.available_height() - 74.0).max(120.0);
        let rows = ex.local_entries.len() + 1;
        let mut area = egui::ScrollArea::vertical().id_salt("local-list").max_height(h).auto_shrink([false, false]);
        if let Some(row) = ex.local_scroll_to.take() {
            let step = 22.0 + ui.spacing().item_spacing.y;
            area = area.vertical_scroll_offset((row as f32 * step - h / 2.0).max(0.0));
        }
        area.show_rows(ui, 22.0, rows, |ui, range| {
            if range.start == 0 && ui.selectable_label(false, "\u{2191}  ..").clicked() {
                if let Some(p) = ex.local_dir.parent() {
                    ex.local_dir = p.to_path_buf();
                    ex.local_selected.clear();
                }
            }
            for i in range.start.saturating_sub(1)..range.end.saturating_sub(1).min(ex.local_entries.len()) {
                let e = ex.local_entries[i].clone();
                let sel = ex.local_selected.contains(&e.name);
                let slash = if e.is_dir { "/" } else { "" };
                let size = if e.is_dir { String::new() } else { format::bytes(e.size as i64) };
                let r = ui.selectable_label(sel, format!("{}{slash}    {size}", e.name));
                if r.double_clicked() && e.is_dir {
                    ex.local_dir = PathBuf::from(&e.path);
                    ex.local_selected.clear();
                } else if r.clicked() {
                    if ui.input(|i| i.modifiers.command) {
                        if !ex.local_selected.remove(&e.name) {
                            ex.local_selected.insert(e.name.clone());
                        }
                    } else {
                        ex.local_selected = [e.name.clone()].into();
                    }
                }
            }
        });
        // This panel's own buttons: they act on what is chosen here.
        let server_name = self.servers.servers.iter().find(|s| s.id == ex.server_id).map(|s| s.name.clone()).unwrap_or_else(|| "server".to_string());
        let names: Vec<String> = ex.local_selected.iter().cloned().collect();
        let dirs = names.iter().filter(|n| ex.local_entries.iter().any(|e| &e.name == *n && e.is_dir)).count();
        ui.label(RichText::new(selection_summary(dirs, names.len() - dirs)).color(FG_SECONDARY).small());
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(!names.is_empty(), egui::Button::new(format!("Unggah ke {server_name} \u{2192}"))).on_hover_text("Unggah berkas dan folder yang dipilih di panel ini ke folder yang terbuka di server (panel kanan)").clicked() {
                let existing: BTreeSet<String> = ex.entries.iter().map(|e| e.name.clone()).collect();
                let plain: Vec<String> = names.iter().filter(|n| !ex.local_entries.iter().any(|e| &e.name == *n && e.is_dir)).cloned().collect();
                let conflicts = overwrite_conflicts(&plain, &existing, false);
                if conflicts.is_empty() {
                    self.explorer_enqueue_uploads(ex, &names, false);
                } else {
                    ex.dialog = Some(ExDialog::ConfirmOverwrite { download: false, names: conflicts, all: names.clone() });
                }
            }
        });
    }

    fn server_panel(&mut self, ui: &mut Ui, tab_id: u64, ex: &mut ExplorerState) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Server").font(theme::bold(13.0)));
            if ui.small_button("Muat ulang").clicked() {
                let d = ex.dir.clone();
                self.explorer_list(tab_id, ex, &d);
            }
            if ui.small_button("Folder baru").clicked() {
                ex.dialog = Some(ExDialog::NewFolder(String::new()));
            }
            ui.add(egui::TextEdit::singleline(&mut ex.filter).hint_text("Saring nama...").desired_width(140.0));
        });
        ui.horizontal_wrapped(|ui| {
            for (label, path) in crumbs(&ex.dir) {
                if ui.small_button(&label).clicked() && path != ex.dir {
                    self.explorer_list(tab_id, ex, &path);
                }
            }
        });
        ui.horizontal(|ui| {
            for (label, key) in [("Nama", SortKey::Name), ("Ukuran", SortKey::Size), ("Diubah", SortKey::Modified)] {
                let on = ex.sort.0 == key;
                let arrow = if on { if ex.sort.1 { " \u{25B2}" } else { " \u{25BC}" } } else { "" };
                if ui.selectable_label(on, format!("{label}{arrow}")).clicked() {
                    ex.sort = (key, if on { !ex.sort.1 } else { true });
                }
            }
            if ex.loading {
                ui.spinner();
            }
        });
        if let Some(e) = &ex.error {
            ui.label(RichText::new(e).color(DANGER));
        }
        let h = (ui.available_height() - 74.0).max(120.0);
        let key = (ex.gen, ex.sort.0, ex.sort.1, ex.filter.clone());
        if ex.view_cache.as_ref().map(|(k, _)| k != &key).unwrap_or(true) {
            ex.view_cache = Some((key, view_of(&ex.entries, ex.sort.0, ex.sort.1, &ex.filter)));
        }
        let view: Vec<Entry> = ex.view_cache.as_ref().map(|(_, v)| v.clone()).unwrap_or_default();
        let up_row = usize::from(ex.dir != "/");
        let mut area = egui::ScrollArea::vertical().id_salt("server-list").max_height(h).auto_shrink([false, false]);
        if let Some(p) = ex.scroll_to.take() {
            if let Some(i) = view.iter().position(|e| e.path == p) {
                let step = 22.0 + ui.spacing().item_spacing.y;
                area = area.vertical_scroll_offset((((i + up_row) as f32) * step - h / 2.0).max(0.0));
            }
        }
        area.show_rows(ui, 22.0, view.len() + up_row, |ui, range| {
            if up_row == 1 && range.start == 0 && ui.selectable_label(false, "\u{2191}  ..").clicked() {
                let p = parent_remote(&ex.dir);
                self.explorer_list(tab_id, ex, &p);
            }
            for e in view.iter().skip(range.start.saturating_sub(up_row)).take(range.len()) {
                let sel = ex.selected.contains(&e.path);
                let slash = if e.is_dir { "/" } else { "" };
                let size = if e.is_dir { String::new() } else { format::bytes(e.size as i64) };
                let r = ui.selectable_label(sel, format!("{}{slash}    {size}", e.name));
                if r.double_clicked() {
                    if e.is_dir {
                        self.explorer_list(tab_id, ex, &e.path);
                    } else {
                        self.explorer_start_preview(tab_id, ex, e.clone(), 1024 * 1024);
                    }
                } else if r.clicked() {
                    if ui.input(|i| i.modifiers.command) {
                        if !ex.selected.remove(&e.path) {
                            ex.selected.insert(e.path.clone());
                        }
                    } else {
                        ex.selected = [e.path.clone()].into();
                    }
                }
            }
            if view.is_empty() && !ex.loading {
                ui.label(RichText::new("Folder kosong.").color(FG_SECONDARY));
            }
        });
        let chosen: Vec<Entry> = ex.entries.iter().filter(|e| ex.selected.contains(&e.path)).cloned().collect();
        let files: Vec<&Entry> = chosen.iter().filter(|e| !e.is_dir).collect();
        let left_state = ex.left.as_ref().map(|l| (l.name.clone(), l.status == ExStatus::Ready));
        // This panel's own buttons: they act on what is chosen here.
        ui.label(RichText::new(selection_summary(chosen.len() - files.len(), files.len())).color(FG_SECONDARY).small());
        ui.horizontal_wrapped(|ui| {
            match &left_state {
                Some((name, true)) => {
                    if ui.add_enabled(!chosen.is_empty(), egui::Button::new(format!("\u{2190} Salin ke {name}"))).on_hover_text("Salin berkas dan folder yang dipilih di panel ini ke folder yang terbuka di panel kiri, langsung antar server").clicked() {
                        self.pair_copy(ex, true);
                    }
                }
                Some((_, false)) => {}
                None => {
                    if ui.add_enabled(!chosen.is_empty(), egui::Button::new("\u{2190} Unduh ke komputer ini")).on_hover_text("Unduh berkas dan folder yang dipilih di panel ini ke folder yang terbuka di panel kiri").clicked() {
                        let names: Vec<String> = chosen.iter().map(|e| e.name.clone()).collect();
                        let existing: BTreeSet<String> = ex.local_entries.iter().map(|e| e.name.clone()).collect();
                        let safe: Vec<String> = files.iter().filter_map(|e| safe_local_name(&e.name)).collect();
                        let conflicts = overwrite_conflicts(&safe, &existing, true);
                        if conflicts.is_empty() {
                            self.explorer_enqueue_downloads(ex, &names, false);
                        } else {
                            ex.dialog = Some(ExDialog::ConfirmOverwrite { download: true, names: conflicts, all: names });
                        }
                    }
                }
            }
            if ui.add_enabled(files.len() == 1 && chosen.len() == 1, egui::Button::new("Pratinjau")).clicked() {
                if let Some(e) = files.first() {
                    self.explorer_start_preview(tab_id, ex, (*e).clone(), 1024 * 1024);
                }
            }
            if ui.add_enabled(chosen.len() == 1, egui::Button::new("Ganti nama")).clicked() {
                if let Some(e) = chosen.first() {
                    ex.dialog = Some(ExDialog::Rename(e.clone(), e.name.clone()));
                }
            }
            if ui.add_enabled(!chosen.is_empty(), egui::Button::new(RichText::new("Hapus").color(DANGER))).clicked() {
                ex.dialog = Some(ExDialog::ConfirmDelete(chosen.clone()));
            }
        });
    }

    fn explorer_dialogs(&mut self, ctx: &egui::Context, tab_id: u64, ex: &mut ExplorerState) {
        let Some(dialog) = ex.dialog.take() else { return };
        let mut keep: Option<ExDialog> = None;
        let frame = egui::Frame::window(&ctx.global_style()).fill(SURFACE).inner_margin(egui::Margin::same(20));
        let win = |title: &str| egui::Window::new(title.to_string()).title_bar(false).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).fixed_size([440.0, 0.0]);
        match dialog {
            ExDialog::NewFolder(mut name) => {
                let mut act: Option<bool> = None;
                win("Folder baru").frame(frame).show(ctx, |ui| {
                    ui.label(RichText::new("Folder baru di server").font(theme::bold(15.0)));
                    let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY));
                    r.request_focus();
                    ui.horizontal(|ui| {
                        if ui.add(primary_button("Buat")).clicked() {
                            act = Some(true);
                        }
                        if ui.button("Batal").clicked() {
                            act = Some(false);
                        }
                    });
                });
                match act {
                    Some(true) if safe_local_name(&name).is_some() => {
                        let path = join_remote(&ex.dir, name.trim());
                        self.explorer_op(tab_id, ex, "Gagal membuat folder", move |f| Box::pin(async move { f.mkdir(&path).await }));
                    }
                    Some(true) => keep = Some(ExDialog::NewFolder(name)),
                    Some(false) => {}
                    None => keep = Some(ExDialog::NewFolder(name)),
                }
            }
            ExDialog::Rename(entry, mut name) => {
                let mut act: Option<bool> = None;
                win("Ganti nama").frame(frame).show(ctx, |ui| {
                    ui.label(RichText::new(format!("Ganti nama \"{}\"", entry.name)).font(theme::bold(15.0)));
                    ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY));
                    ui.horizontal(|ui| {
                        if ui.add(primary_button("Ganti")).clicked() {
                            act = Some(true);
                        }
                        if ui.button("Batal").clicked() {
                            act = Some(false);
                        }
                    });
                });
                match act {
                    Some(true) if safe_local_name(&name).is_some() && name != entry.name => {
                        let name = name.trim().to_string();
                        if ex.entries.iter().any(|e| e.name == name) {
                            self.notice = Some(("Nama itu sudah dipakai di folder ini.".into(), std::time::Instant::now()));
                            keep = Some(ExDialog::Rename(entry, name));
                        } else {
                            let (from, to) = (entry.path.clone(), join_remote(&parent_remote(&entry.path), &name));
                            self.explorer_op(tab_id, ex, "Gagal mengganti nama", move |f| Box::pin(async move { f.rename(&from, &to).await }));
                        }
                    }
                    Some(true) => keep = Some(ExDialog::Rename(entry, name)),
                    Some(false) => {}
                    None => keep = Some(ExDialog::Rename(entry, name)),
                }
            }
            ExDialog::ConfirmDelete(items) => {
                let mut act: Option<bool> = None;
                win("Hapus").frame(frame).show(ctx, |ui| {
                    ui.label(RichText::new(format!("Hapus {} item dari server?", items.len())).font(theme::bold(15.0)));
                    ui.label(RichText::new("Tindakan ini tidak bisa dibatalkan. Folder hanya bisa dihapus bila kosong.").color(FG_SECONDARY));
                    for e in items.iter().take(6) {
                        ui.label(format!("\u{2022} {}", e.name));
                    }
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new("Hapus").color(egui::Color32::WHITE)).fill(DANGER)).clicked() {
                            act = Some(true);
                        }
                        if ui.button("Batal").clicked() {
                            act = Some(false);
                        }
                    });
                });
                match act {
                    Some(true) => {
                        let list: Vec<(String, bool)> = items.iter().map(|e| (e.path.clone(), e.is_dir)).collect();
                        self.explorer_op(tab_id, ex, "Gagal menghapus", move |f| {
                            Box::pin(async move {
                                for (p, d) in list {
                                    f.remove(&p, d).await?;
                                }
                                Ok(())
                            })
                        });
                    }
                    Some(false) => {}
                    None => keep = Some(ExDialog::ConfirmDelete(items)),
                }
            }
            ExDialog::ConfirmCopy { names, jobs, src_fs } => {
                let mut act: Option<bool> = None;
                win("Timpa berkas").frame(frame).show(ctx, |ui| {
                    ui.label(RichText::new("Berkas ini sudah ada di server tujuan").font(theme::bold(15.0)));
                    for n in names.iter().take(8) {
                        ui.label(format!("\u{2022} {n}"));
                    }
                    ui.label(RichText::new("Menimpa akan mengganti isinya dan tidak bisa dibatalkan.").color(FG_SECONDARY));
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new("Timpa").color(egui::Color32::WHITE)).fill(DANGER)).clicked() {
                            act = Some(true);
                        }
                        if ui.button("Lewati yang sudah ada").clicked() {
                            act = Some(false);
                        }
                    });
                });
                match act {
                    Some(true) => self.pair_enqueue(jobs, names, src_fs),
                    Some(false) => {
                        let rest: Vec<CopyJob> = jobs.into_iter().filter(|j| !names.contains(&j.name)).collect();
                        if !rest.is_empty() {
                            self.pair_enqueue(rest, Vec::new(), src_fs);
                        }
                    }
                    None => keep = Some(ExDialog::ConfirmCopy { names, jobs, src_fs }),
                }
            }
            ExDialog::ConfirmOverwrite { download, names, all } => {
                let mut act: Option<bool> = None;
                win("Timpa berkas").frame(frame).show(ctx, |ui| {
                    ui.label(RichText::new(if download { "Berkas ini sudah ada di komputer ini" } else { "Berkas ini sudah ada di server" }).font(theme::bold(15.0)));
                    for n in names.iter().take(8) {
                        ui.label(format!("\u{2022} {n}"));
                    }
                    ui.label(RichText::new("Menimpa akan mengganti isinya dan tidak bisa dibatalkan.").color(FG_SECONDARY));
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new("Timpa").color(egui::Color32::WHITE)).fill(DANGER)).clicked() {
                            act = Some(true);
                        }
                        if ui.button("Lewati yang sudah ada").clicked() {
                            act = Some(false);
                        }
                    });
                });
                match act {
                    Some(true) => {
                        if download {
                            self.explorer_enqueue_downloads(ex, &all, true);
                        } else {
                            self.explorer_enqueue_uploads(ex, &all, true);
                        }
                    }
                    Some(false) => {
                        let rest: Vec<String> = all.iter().filter(|n| !names.contains(n)).cloned().collect();
                        if !rest.is_empty() {
                            if download {
                                self.explorer_enqueue_downloads(ex, &rest, false);
                            } else {
                                self.explorer_enqueue_uploads(ex, &rest, false);
                            }
                        }
                    }
                    None => keep = Some(ExDialog::ConfirmOverwrite { download, names, all }),
                }
            }
        }
        ex.dialog = keep;
    }

    fn preview_ui(&mut self, ui: &mut Ui, tab_id: u64, ex: &mut ExplorerState) {
        let Some(pv) = ex.preview.as_mut() else { return };
        let entry = pv.entry.clone();
        let mut close = false;
        let (mut want_more, mut want_pdf, mut want_open, mut want_download) = (false, None::<(usize, f32)>, false, false);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{}  ({})", entry.name, format::bytes(entry.size as i64))).font(theme::bold(13.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Tutup").clicked() {
                    close = true;
                }
                if ui.button("Unduh").clicked() {
                    want_download = true;
                }
                if ui.button("Buka di aplikasi Windows").clicked() {
                    want_open = true;
                }
            });
        });
        ui.separator();
        match &mut pv.state {
            PvState::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Memuat pratinjau...");
                });
            }
            PvState::Error { message, offer_open } => {
                ui.label(RichText::new(message.as_str()).color(DANGER));
                if *offer_open {
                    ui.label(RichText::new("Pakai \"Buka di aplikasi Windows\" atau Unduh.").color(FG_SECONDARY));
                }
            }
            PvState::Ready(content) => match content {
                PvContent::Text { text, lines, truncated } => {
                    if *truncated {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Hanya bagian awal yang dimuat.").color(FG_SECONDARY));
                            if ui.button("Muat lebih banyak").clicked() {
                                want_more = true;
                            }
                        });
                    }
                    let row_h = 15.0;
                    egui::ScrollArea::both().auto_shrink([false, false]).show_rows(ui, row_h, lines.len(), |ui, range| {
                        for i in range {
                            let (a, b) = lines[i];
                            ui.label(RichText::new(format!("{:>5}  {}", i + 1, text[a..b].trim_end_matches('\r'))).font(theme::mono(12.0)));
                        }
                    });
                }
                PvContent::Markdown { blocks, truncated } => {
                    if *truncated {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Hanya bagian awal yang dimuat.").color(FG_SECONDARY));
                            if ui.button("Muat lebih banyak").clicked() {
                                want_more = true;
                            }
                        });
                    }
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| blocks_ui(ui, blocks));
                }
                PvContent::Doc(blocks) => {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| blocks_ui(ui, blocks));
                }
                PvContent::Image(tex) => {
                    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                        ui.image((tex.id(), tex.size_vec2()));
                    });
                }
                PvContent::Pdf { pages, page, scale, tex, busy, .. } => {
                    ui.horizontal(|ui| {
                        if ui.add_enabled(*page > 0 && !*busy, egui::Button::new("\u{25C0}")).clicked() {
                            want_pdf = Some((*page - 1, *scale));
                        }
                        ui.label(format!("Halaman {} dari {}", *page + 1, pages));
                        if ui.add_enabled(*page + 1 < *pages && !*busy, egui::Button::new("\u{25B6}")).clicked() {
                            want_pdf = Some((*page + 1, *scale));
                        }
                        if ui.add_enabled(!*busy, egui::Button::new("\u{2212}")).clicked() {
                            want_pdf = Some((*page, (*scale / 1.25).max(0.5)));
                        }
                        if ui.add_enabled(!*busy, egui::Button::new("+")).clicked() {
                            want_pdf = Some((*page, (*scale * 1.25).min(4.0)));
                        }
                        if *busy {
                            ui.spinner();
                        }
                    });
                    if let Some(t) = tex {
                        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                            ui.image((t.id(), t.size_vec2()));
                        });
                    }
                }
                PvContent::Sheets { sheets, active } => {
                    ui.horizontal(|ui| {
                        for (i, s) in sheets.iter().enumerate() {
                            if ui.selectable_label(*active == i, &s.name).clicked() {
                                *active = i;
                            }
                        }
                    });
                    if let Some(s) = sheets.get(*active) {
                        if s.truncated {
                            ui.label(RichText::new("Hanya sebagian baris/kolom yang ditampilkan.").color(FG_SECONDARY));
                        }
                        let cols = s.rows.iter().map(|r| r.len()).max().unwrap_or(0).max(1);
                        egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
                            egui_extras::TableBuilder::new(ui).striped(true).columns(egui_extras::Column::auto().at_least(60.0).clip(true), cols).body(|body| {
                                body.rows(18.0, s.rows.len(), |mut row| {
                                    let r = &s.rows[row.index()];
                                    for c in 0..cols {
                                        row.col(|ui| {
                                            ui.label(RichText::new(r.get(c).map(|s| s.as_str()).unwrap_or("")).font(theme::mono(12.0)));
                                        });
                                    }
                                });
                            });
                        });
                    }
                }
            },
        }
        if close {
            ex.preview = None;
        }
        if want_more {
            let tb = ex.preview.as_ref().map(|p| p.text_bytes * 4).unwrap_or(4 * 1024 * 1024).min(64 * 1024 * 1024);
            self.explorer_start_preview(tab_id, ex, entry.clone(), tb);
        }
        if let Some((p, s)) = want_pdf {
            self.explorer_pdf_page(tab_id, ex, p, s);
        }
        if want_open {
            self.explorer_open_external(ex, &entry);
        }
        if want_download {
            self.explorer_enqueue_download_path(ex, &entry);
        }
    }
}

fn blocks_ui(ui: &mut Ui, blocks: &[Block]) {
    for b in blocks {
        match b {
            Block::Heading { level, text } => {
                ui.add_space(6.0);
                ui.label(RichText::new(text).font(theme::bold(match level { 1 => 22.0, 2 => 19.0, 3 => 17.0, _ => 15.0 })));
            }
            Block::Paragraph(t) => {
                ui.label(t);
            }
            Block::ListItem { ordered, depth, text } => {
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(14.0 * (*depth as f32 + 1.0));
                    ui.label(if *ordered { "\u{2022}" } else { "\u{2022}" });
                    ui.label(text);
                });
            }
            Block::Code(t) => {
                egui::Frame::new().fill(SURFACE_RAISED).corner_radius(4).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                    ui.label(RichText::new(t).font(theme::mono(12.0)));
                });
            }
            Block::Quote(t) => {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new(t).italics().color(FG_SECONDARY));
                });
            }
            Block::Table(rows) => {
                egui::Grid::new(ui.next_auto_id()).striped(true).show(ui, |ui| {
                    for r in rows {
                        for c in r {
                            ui.label(c);
                        }
                        ui.end_row();
                    }
                });
            }
            Block::Rule => {
                ui.separator();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_says_what_the_buttons_act_on() {
        assert_eq!(selection_summary(0, 0), "Belum ada yang dipilih");
        assert_eq!(selection_summary(0, 1), "1 berkas dipilih");
        assert_eq!(selection_summary(2, 0), "2 folder dipilih");
        assert_eq!(selection_summary(1, 2), "3 dipilih: 1 folder, 2 berkas");
    }

    #[test]
    fn local_listing_refreshes_after_a_short_while() {
        assert!(!local_is_stale(std::time::Duration::from_millis(500)));
        assert!(local_is_stale(std::time::Duration::from_secs(2)));
    }

    fn e(name: &str, dir: bool, size: u64) -> Entry {
        Entry { name: name.into(), path: format!("/{name}"), is_dir: dir, size, modified: None, is_link: false }
    }

    #[test]
    fn crumbs_split_unix_and_windows_paths() {
        assert_eq!(crumbs("/"), vec![("/".to_string(), "/".to_string())]);
        assert_eq!(crumbs("/home/eko/"), vec![("/".into(), "/".into()), ("home".into(), "/home".into()), ("eko".into(), "/home/eko".into())]);
        let c = crumbs_local(Path::new(r"C:\Users\eko\Downloads"));
        assert_eq!(c.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), ["C:", "Users", "eko", "Downloads"]);
        assert_eq!(c[0].1, PathBuf::from(r"C:\"));
        assert_eq!(c[2].1, PathBuf::from(r"C:\Users\eko"));
    }

    #[test]
    fn sorted_view_filters_and_orders() {
        let v = vec![e("b.txt", false, 5), e("Docs", true, 0), e("a.TXT", false, 9), e("photo.png", false, 1)];
        let names = |x: Vec<Entry>| x.into_iter().map(|e| e.name).collect::<Vec<_>>();
        assert_eq!(names(view_of(&v, SortKey::Name, true, "")), ["Docs", "a.TXT", "b.txt", "photo.png"]);
        assert_eq!(names(view_of(&v, SortKey::Size, false, "")), ["Docs", "a.TXT", "b.txt", "photo.png"]);
        assert_eq!(names(view_of(&v, SortKey::Name, true, "  txt ")), ["a.TXT", "b.txt"], "case-insensitive substring");
        assert!(view_of(&v, SortKey::Name, true, "zzz").is_empty());
    }

    #[test]
    fn overwrite_check_lists_existing_names() {
        let existing: BTreeSet<String> = ["a.txt".to_string(), "B.TXT".to_string()].into();
        let names = vec!["a.txt".to_string(), "b.txt".to_string(), "c.txt".to_string()];
        assert_eq!(overwrite_conflicts(&names, &existing, false), ["a.txt"], "servers are case-sensitive");
        assert_eq!(overwrite_conflicts(&names, &existing, true), ["a.txt", "b.txt"], "Windows is not");
        assert!(overwrite_conflicts(&[], &existing, true).is_empty());
    }

    #[test]
    fn programs_and_macro_documents_are_not_opened_from_a_server() {
        for bad in ["setup.exe", "run.BAT", "x.js", "a.hta", "link.lnk", "i.msi", "s.ps1", "m.docm", "m.XLSM", "t.scr"] {
            assert!(is_unsafe_to_open(bad), "{bad}");
        }
        for ok in ["report.pdf", "notes.txt", "photo.png", "sheet.xlsx", "doc.docx", "noext", "archive.zip"] {
            assert!(!is_unsafe_to_open(ok), "{ok}");
        }
        assert!(ZONE_IDENTIFIER.contains("ZoneId=3"));
    }

    #[test]
    fn preview_choice_follows_kind_and_limit() {
        let max = 25 * 1024 * 1024;
        assert_eq!(preview_choice("notes.txt", 900 * 1024 * 1024, max), PreviewChoice::Show, "text is read partially, any size");
        assert_eq!(preview_choice("big.pdf", 30 * 1024 * 1024, max), PreviewChoice::TooLarge);
        assert_eq!(preview_choice("small.pdf", 1024, max), PreviewChoice::Show);
        assert_eq!(preview_choice("old.xls", 10, max), PreviewChoice::External);
        assert_eq!(preview_choice("mystery.bin", 10, max), PreviewChoice::External);
        assert_eq!(preview_choice("pic.PNG", 1024, max), PreviewChoice::Show);
    }
}
