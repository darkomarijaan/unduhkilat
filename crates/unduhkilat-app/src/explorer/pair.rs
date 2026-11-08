//! The left panel can show another saved server instead of this computer, so files can be copied
//! between two servers without being stored here. The copy itself is a normal queue item
//! (`server-copy://`), so progress, pause, resume and speed limits come from the shared engine.

use super::*;

/// Questions from this panel's connection (host key, password) reach the UI under this bit.
pub const LEFT_BIT: u64 = 1 << 60;

/// One file to copy from `src_server` to `dst_server`/`dst_dir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyJob {
    pub src_server: String,
    pub src_path: String,
    pub dst_server: String,
    pub dst_dir: String,
    pub name: String,
    /// A folder: every file under it is copied, keeping the folder structure.
    pub is_dir: bool,
}

/// `chosen` as copy jobs into `dst_dir`, and the names of files that already exist there (replacing
/// those needs a yes). A folder that already exists is merged into: its files are checked one by one
/// when they are queued, and an existing one is reported, never replaced.
pub fn plan_copy(src_server: &str, chosen: &[Entry], dst_server: &str, dst_dir: &str, existing: &BTreeSet<String>) -> (Vec<CopyJob>, Vec<String>) {
    let jobs: Vec<CopyJob> = chosen
        .iter()
        .map(|e| CopyJob { src_server: src_server.into(), src_path: e.path.clone(), dst_server: dst_server.into(), dst_dir: dst_dir.into(), name: e.name.clone(), is_dir: e.is_dir })
        .collect();
    let conflicts = jobs.iter().filter(|j| !j.is_dir && existing.contains(&j.name)).map(|j| j.name.clone()).collect();
    (jobs, conflicts)
}

pub struct LeftServer {
    pub server_id: String,
    pub name: String,
    pub status: ExStatus,
    pub fs: Option<SharedFs>,
    pub cancel: Option<CancellationToken>,
    pub dir: String,
    pub entries: Vec<Entry>,
    /// Paths of the chosen files.
    pub selected: BTreeSet<String>,
    pub loading: bool,
    pub error: Option<String>,
    pub gen: u64,
    /// A file that just arrived here: select it and scroll to it once the folder is listed.
    pub reveal: Option<String>,
    pub scroll_row: Option<usize>,
}

impl LeftServer {
    pub fn new(server_id: &str, name: &str) -> Self {
        LeftServer {
            server_id: server_id.into(),
            name: name.into(),
            status: ExStatus::Idle,
            fs: None,
            cancel: None,
            dir: "/".into(),
            entries: Vec::new(),
            selected: BTreeSet::new(),
            loading: false,
            error: None,
            gen: 0,
            reveal: None,
            scroll_row: None,
        }
    }

    pub fn close(&mut self, rt: &tokio::runtime::Handle) {
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

pub enum PairMsg {
    Connected { tab_id: u64, server_id: String, result: Result<Box<dyn RemoteFs>, AppError> },
    Listed { tab_id: u64, server_id: String, gen: u64, dir: String, result: Result<Vec<Entry>, AppError> },
}

impl App {
    /// The picker at the top of the left panel. `None` = no change; `Some(None)` = this computer;
    /// `Some(Some((id, name)))` = that server.
    pub(super) fn source_picker(&self, ui: &mut Ui, ex: &ExplorerState) -> Option<Option<(String, String)>> {
        let current = ex.left.as_ref().map(|l| l.name.clone()).unwrap_or_else(|| "Komputer ini".to_string());
        let mut choice = None;
        egui::ComboBox::from_id_salt("left-source").selected_text(RichText::new(current).font(theme::bold(13.0))).show_ui(ui, |ui| {
            if ui.selectable_label(ex.left.is_none(), "Komputer ini").clicked() {
                choice = Some(None);
            }
            for s in &self.servers.servers {
                let on = ex.left.as_ref().map(|l| l.server_id == s.id).unwrap_or(false);
                if ui.selectable_label(on, &s.name).on_hover_text("Tampilkan server ini di panel kiri untuk menyalin berkas antar server").clicked() && !on {
                    choice = Some(Some((s.id.clone(), s.name.clone())));
                }
            }
        });
        choice
    }

    pub(super) fn pair_choose(&mut self, tab_id: u64, ex: &mut ExplorerState, choice: Option<(String, String)>) {
        if let Some(mut old) = ex.left.take() {
            self.servers.shell_waiters.remove(&(tab_id | LEFT_BIT));
            self.servers.clear_prompts_if_idle();
            old.close(&self.rt);
        }
        if let Some((id, name)) = choice {
            ex.left = Some(LeftServer::new(&id, &name));
            self.pair_connect(tab_id, ex);
        }
    }

    pub(super) fn pair_connect(&mut self, tab_id: u64, ex: &mut ExplorerState) {
        let Some(left) = ex.left.as_mut() else { return };
        if let Some(c) = left.cancel.take() {
            c.cancel();
        }
        let cancel = CancellationToken::new();
        left.cancel = Some(cancel.clone());
        left.status = ExStatus::Connecting;
        left.error = None;
        self.servers.shell_waiters.insert(tab_id | LEFT_BIT);
        let (svc, tx, ctx, sid) = (self.manager.servers(), self.tx.clone(), self.ctx.clone(), left.server_id.clone());
        let interact = Arc::new(super::super::servers::UiInteract { tx: tx.clone(), ctx: ctx.clone() });
        self.rt.spawn(async move {
            let result = svc.open_fs(&sid, interact, cancel).await;
            let _ = tx.send(Msg::Explorer(ExplorerMsg::Pair(PairMsg::Connected { tab_id, server_id: sid, result })));
            ctx.request_repaint();
        });
    }

    /// List `dir` on the left server; `first` starts in its home folder.
    pub(super) fn pair_list(&mut self, tab_id: u64, ex: &mut ExplorerState, dir: &str, first: bool) {
        let Some(left) = ex.left.as_mut() else { return };
        let Some(fs) = left.fs.clone() else { return };
        left.gen += 1;
        left.loading = true;
        left.error = None;
        let (gen, dir, sid) = (left.gen, dir.to_string(), left.server_id.clone());
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.rt.spawn(async move {
            let (dir, result) = match fs.lock().await.as_mut() {
                Some(f) => {
                    let start = if first {
                        match f.home().await {
                            Ok(h) if h.starts_with('/') => h,
                            _ => dir,
                        }
                    } else {
                        dir
                    };
                    match f.list(&start).await {
                        Ok(v) => (start, Ok(v)),
                        Err(_) if first && start != "/" => ("/".to_string(), f.list("/").await),
                        Err(e) => (start, Err(e)),
                    }
                }
                None => (dir, Err(AppError::new(unduhkilat_core::error::ErrorCode::ErrConnectionDropped, "Sambungan sudah ditutup."))),
            };
            let _ = tx.send(Msg::Explorer(ExplorerMsg::Pair(PairMsg::Listed { tab_id, server_id: sid, gen, dir, result })));
            ctx.request_repaint();
        });
    }

    pub(super) fn pair_msg(&mut self, m: PairMsg) {
        match m {
            PairMsg::Connected { tab_id, server_id, result } => {
                self.servers.shell_waiters.remove(&(tab_id | LEFT_BIT));
                self.servers.clear_prompts_if_idle();
                let current = self.explorer_mut(tab_id).and_then(|ex| ex.left.as_ref()).map(|l| l.server_id == server_id).unwrap_or(false);
                if !current {
                    if let Ok(fs) = result {
                        self.rt.spawn(fs.close());
                    }
                    return;
                }
                let Some(mut ex) = self.take_explorer(tab_id) else { return };
                match result {
                    Ok(fs) => {
                        if let Some(l) = ex.left.as_mut() {
                            l.fs = Some(Arc::new(Mutex::new(Some(fs))));
                            l.status = ExStatus::Ready;
                        }
                        self.pair_list(tab_id, &mut ex, "/", true);
                    }
                    Err(e) => {
                        if let Some(l) = ex.left.as_mut() {
                            l.status = ExStatus::Failed(e.message);
                        }
                    }
                }
                self.put_explorer(tab_id, ex);
            }
            PairMsg::Listed { tab_id, server_id, gen, dir, result } => {
                let Some(left) = self.explorer_mut(tab_id).and_then(|ex| ex.left.as_mut()) else { return };
                if left.server_id != server_id || left.gen != gen {
                    return;
                }
                left.loading = false;
                match result {
                    Ok(mut v) => {
                        sort_entries(&mut v, SortKey::Name, true);
                        left.dir = dir;
                        left.entries = v;
                        left.selected.clear();
                        if let Some(p) = left.reveal.take() {
                            if let Some(i) = left.entries.iter().position(|e| e.path == p) {
                                left.selected.insert(p);
                                left.scroll_row = Some(i + usize::from(left.dir != "/"));
                            }
                        }
                    }
                    Err(e) => left.error = Some(e.message),
                }
            }
        }
    }

    /// The left panel when it shows a server.
    pub(super) fn pair_left_ui(&mut self, ui: &mut Ui, tab_id: u64, ex: &mut ExplorerState) {
        let mut choice = None;
        let mut refresh = false;
        ui.horizontal(|ui| {
            choice = self.source_picker(ui, ex);
            if ui.small_button("Muat ulang").clicked() {
                refresh = true;
            }
        });
        if choice.is_some() {
            self.pair_choose(tab_id, ex, choice.flatten());
            return;
        }
        let Some(status) = ex.left.as_ref().map(|l| l.status.clone()) else { return };
        match status {
            ExStatus::Idle | ExStatus::Connecting => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Menyambung...").color(FG_SECONDARY));
                });
                return;
            }
            ExStatus::Failed(msg) => {
                ui.label(RichText::new(msg).color(DANGER));
                if ui.button("Coba lagi").clicked() {
                    self.pair_connect(tab_id, ex);
                }
                return;
            }
            ExStatus::Ready => {}
        }
        let mut go: Option<String> = None;
        if let Some(left) = ex.left.as_mut() {
            ui.horizontal_wrapped(|ui| {
                for (label, path) in crumbs(&left.dir) {
                    if ui.small_button(&label).clicked() && path != left.dir {
                        go = Some(path);
                    }
                }
                if left.loading {
                    ui.spinner();
                }
            });
            if let Some(e) = &left.error {
                ui.label(RichText::new(e).color(DANGER));
            }
            let h = (ui.available_height() - 40.0).max(120.0);
            let up = usize::from(left.dir != "/");
            let mut area = egui::ScrollArea::vertical().id_salt("left-server-list").max_height(h).auto_shrink([false, false]);
            if let Some(row) = left.scroll_row.take() {
                let step = 22.0 + ui.spacing().item_spacing.y;
                area = area.vertical_scroll_offset((row as f32 * step - h / 2.0).max(0.0));
            }
            let rows = left.entries.len() + up;
            area.show_rows(ui, 22.0, rows, |ui, range| {
                if up == 1 && range.start == 0 && ui.selectable_label(false, "\u{2191}  ..").clicked() {
                    go = Some(parent_remote(&left.dir));
                }
                for e in left.entries.iter().skip(range.start.saturating_sub(up)).take(range.len()) {
                    let sel = left.selected.contains(&e.path);
                    let slash = if e.is_dir { "/" } else { "" };
                    let size = if e.is_dir { String::new() } else { format::bytes(e.size as i64) };
                    let r = ui.selectable_label(sel, format!("{}{slash}    {size}", e.name));
                    if r.double_clicked() && e.is_dir {
                        go = Some(e.path.clone());
                    } else if r.clicked() {
                        if ui.input(|i| i.modifiers.command) {
                            if !left.selected.remove(&e.path) {
                                left.selected.insert(e.path.clone());
                            }
                        } else {
                            left.selected = [e.path.clone()].into();
                        }
                    }
                }
                if left.entries.is_empty() && !left.loading {
                    ui.label(RichText::new("Folder kosong.").color(FG_SECONDARY));
                }
            });
        }
        if refresh && go.is_none() {
            go = ex.left.as_ref().map(|l| l.dir.clone());
        }
        if let Some(d) = go {
            self.pair_list(tab_id, ex, &d, false);
        }
    }

    /// Copy the chosen files across: `to_left` = from the server on the right to the one on the left.
    pub(super) fn pair_copy(&mut self, ex: &mut ExplorerState, to_left: bool) {
        let Some(left) = ex.left.as_ref() else { return };
        let src_fs = if to_left { ex.fs.clone() } else { left.fs.clone() };
        let (jobs, conflicts) = if to_left {
            let chosen: Vec<Entry> = ex.entries.iter().filter(|e| ex.selected.contains(&e.path)).cloned().collect();
            let existing: BTreeSet<String> = left.entries.iter().map(|e| e.name.clone()).collect();
            plan_copy(&ex.server_id, &chosen, &left.server_id, &left.dir, &existing)
        } else {
            let chosen: Vec<Entry> = left.entries.iter().filter(|e| left.selected.contains(&e.path)).cloned().collect();
            let existing: BTreeSet<String> = ex.entries.iter().map(|e| e.name.clone()).collect();
            plan_copy(&left.server_id, &chosen, &ex.server_id, &ex.dir, &existing)
        };
        if jobs.is_empty() {
            return;
        }
        if conflicts.is_empty() {
            self.pair_enqueue(jobs, Vec::new(), src_fs);
        } else {
            ex.dialog = Some(ExDialog::ConfirmCopy { names: conflicts, jobs, src_fs });
        }
    }

    /// Queue the jobs; the files named in `overwrite` replace what is already at the destination.
    /// `src_fs` is the connection to the source server, used to list the files of folders.
    pub(super) fn pair_enqueue(&mut self, jobs: Vec<CopyJob>, overwrite: Vec<String>, src_fs: Option<SharedFs>) {
        let (m, tx, ctx) = (self.manager.clone(), self.tx.clone(), self.ctx.clone());
        self.rt.spawn(async move {
            let (mut ok, mut failed) = (0, Vec::new());
            for j in jobs {
                if j.is_dir {
                    let Some(fs) = &src_fs else {
                        failed.push(format!("{}: sambungan sumber tidak tersedia", j.name));
                        continue;
                    };
                    match fs.lock().await.as_mut() {
                        Some(f) => match m.enqueue_folder_copy(f.as_mut(), &j.src_server, &j.src_path, &j.dst_server, &j.dst_dir, false).await {
                            Ok(r) => {
                                ok += r.queued;
                                failed.extend(r.failed);
                            }
                            Err(e) => failed.push(format!("{}: {}", j.name, e.message)),
                        },
                        None => failed.push(format!("{}: sambungan sumber sudah ditutup", j.name)),
                    }
                    continue;
                }
                match m.enqueue_remote_copy(&j.src_server, &j.src_path, &j.dst_server, &j.dst_dir, overwrite.contains(&j.name)).await {
                    Ok(_) => ok += 1,
                    Err(e) => failed.push(format!("{}: {}", j.name, e.message)),
                }
            }
            let msg = if failed.is_empty() { format!("{ok} salinan ditambahkan ke antrean.") } else { format!("{ok} ditambahkan; gagal: {}", failed.iter().take(3).cloned().collect::<Vec<_>>().join("; ")) };
            let _ = tx.send(Msg::Notice(msg));
            ctx.request_repaint();
        });
    }

    /// An upload or copy finished: show the new file where it landed, in whichever panel is looking
    /// at that folder.
    pub(crate) fn after_transfer(&mut self, url: &str) {
        use unduhkilat_core::engine::remote::{parse_url, Direction};
        let Some(r) = parse_url(url) else { return };
        let (server, path) = match r.direction {
            Direction::Upload => (r.server_id, r.path),
            Direction::Copy => match r.dest {
                Some(d) => d,
                None => return,
            },
            Direction::Download => return,
        };
        let ids: Vec<u64> = self.terminal.tabs.iter().filter(|t| t.explorer.is_some()).map(|t| t.id).collect();
        for id in ids {
            let Some(mut ex) = self.take_explorer(id) else { continue };
            if ex.server_id == server && ex.fs.is_some() && !ex.loading {
                if let Some(top) = top_under(&ex.dir, &path) {
                    ex.reveal = Some(top);
                    let d = ex.dir.clone();
                    self.explorer_list(id, &mut ex, &d);
                }
            }
            let left_top = ex.left.as_ref().filter(|l| l.server_id == server && l.fs.is_some() && !l.loading).and_then(|l| top_under(&l.dir, &path).map(|t| (t, l.dir.clone())));
            if let Some((top, ldir)) = left_top {
                if let Some(l) = ex.left.as_mut() {
                    l.reveal = Some(top);
                }
                self.pair_list(id, &mut ex, &ldir, false);
            }
            self.put_explorer(id, ex);
        }
    }
}

/// The item directly inside `dir` that `path` is, or is inside of: `/a` and `/a/b/c.txt` give `/a/b`.
pub fn top_under(dir: &str, path: &str) -> Option<String> {
    let base = dir.trim_end_matches('/');
    let rest = path.strip_prefix(base)?.strip_prefix('/')?;
    let first = rest.split('/').next().filter(|s| !s.is_empty())?;
    Some(format!("{base}/{first}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nested_file_reveals_the_folder_it_is_in() {
        assert_eq!(top_under("/home", "/home/Proyek/src/a.txt").as_deref(), Some("/home/Proyek"));
        assert_eq!(top_under("/home", "/home/a.txt").as_deref(), Some("/home/a.txt"));
        assert_eq!(top_under("/", "/Proyek/a.txt").as_deref(), Some("/Proyek"));
        assert_eq!(top_under("/home", "/homework/a.txt"), None, "a sibling with the same prefix is not inside");
        assert_eq!(top_under("/home/x", "/home/a.txt"), None);
    }

    #[test]
    fn folders_are_copied_whole_and_only_files_can_collide() {
        let existing: BTreeSet<String> = ["Proyek".to_string(), "b.txt".to_string()].into();
        let (jobs, conflicts) = plan_copy("A", &[e("Proyek", true), e("b.txt", false)], "B", "/in", &existing);
        assert_eq!(jobs.iter().map(|j| (j.name.as_str(), j.is_dir)).collect::<Vec<_>>(), [("Proyek", true), ("b.txt", false)]);
        assert_eq!(conflicts, ["b.txt"], "an existing folder is merged into, not a conflict");
    }

    fn e(name: &str, dir: bool) -> Entry {
        Entry { name: name.into(), path: format!("/src/{name}"), is_dir: dir, size: 1, modified: None, is_link: false }
    }

    #[test]
    fn existing_file_names_are_reported() {
        let existing: BTreeSet<String> = ["b.txt".to_string()].into();
        let (jobs, conflicts) = plan_copy("A", &[e("a.txt", false), e("b.txt", false), e("sub", true)], "B", "/in", &existing);
        assert_eq!(jobs.iter().map(|j| j.name.as_str()).collect::<Vec<_>>(), ["a.txt", "b.txt", "sub"]);
        assert_eq!(conflicts, ["b.txt"]);
        assert!(jobs.iter().all(|j| j.src_server == "A" && j.dst_server == "B" && j.dst_dir == "/in" && j.src_path.starts_with("/src/")));
    }

    #[test]
    fn names_differing_only_by_case_are_different_files_on_a_server() {
        let existing: BTreeSet<String> = ["A.TXT".to_string()].into();
        let (_, conflicts) = plan_copy("A", &[e("a.txt", false)], "B", "/", &existing);
        assert!(conflicts.is_empty());
    }
}
