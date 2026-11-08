//! The pop-up that follows a copy between servers. The copy is a set of normal queue items, so the
//! numbers here are read from those items; nothing is counted separately.

use super::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use unduhkilat_core::models::{Download, DownloadState};

/// How long a copy that finished without a problem stays on screen.
const LINGER: Duration = Duration::from_secs(3);

/// One copy the user started: the queue items it made, and whether more are still being added
/// (a folder is read first, so its files show up in the queue a little later).
pub struct CopyRun {
    pub id: u64,
    pub title: String,
    pub ids: Vec<String>,
    pub queuing: bool,
    /// Items that could not even be queued (for example a file that already exists).
    pub failed: Vec<String>,
    pub finished_at: Option<Instant>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Tally {
    pub files: usize,
    pub done: usize,
    pub cancelled: usize,
    /// "name: reason" for items that failed while copying.
    pub errors: Vec<String>,
    pub bytes_done: i64,
    pub bytes_total: i64,
    /// Files still waiting or running whose size is not known yet.
    pub unknown: usize,
    pub speed: u64,
    /// The file being copied right now.
    pub current: Option<String>,
}

impl Tally {
    /// Nothing is waiting or running any more.
    pub fn settled(&self) -> bool {
        self.done + self.cancelled + self.errors.len() >= self.files
    }

    /// 0..=1 for the bar. By bytes once every size is known, otherwise by files, so it never
    /// jumps back when a new file's size turns up.
    pub fn fraction(&self) -> f32 {
        let live = self.files - self.cancelled - self.errors.len();
        if self.unknown == 0 && self.bytes_total > 0 {
            format::percent(self.bytes_done, self.bytes_total)
        } else if live == 0 {
            1.0
        } else {
            (self.done as f32 / live as f32).clamp(0.0, 1.0)
        }
    }
}

/// `live_bytes` and `speeds` are the engine's latest numbers per running item; the database copy of
/// the byte count is only saved now and then.
pub fn tally(ids: &[String], downloads: &[Download], live_bytes: &HashMap<String, i64>, speeds: &HashMap<String, (u64, u32)>) -> Tally {
    let by_id: HashMap<&str, &Download> = downloads.iter().map(|d| (d.id.as_str(), d)).collect();
    let mut t = Tally::default();
    for id in ids {
        // Removed from the list by the user: nothing left to follow.
        let Some(d) = by_id.get(id.as_str()) else { continue };
        t.files += 1;
        match d.state {
            DownloadState::Completed => {
                t.done += 1;
                let n = d.total_bytes.max(d.downloaded_bytes).max(0);
                t.bytes_done += n;
                t.bytes_total += n;
            }
            DownloadState::Cancelled => t.cancelled += 1,
            DownloadState::Failed => t.errors.push(format!("{}: {}", d.file_name, d.error_message.as_deref().unwrap_or("gagal"))),
            DownloadState::Queued | DownloadState::Paused | DownloadState::Running => {
                let got = live_bytes.get(id).copied().unwrap_or(d.downloaded_bytes).max(0);
                t.bytes_done += got;
                if d.total_bytes >= 0 {
                    t.bytes_total += d.total_bytes;
                } else {
                    t.unknown += 1;
                }
                if d.state == DownloadState::Running {
                    t.speed += speeds.get(id).map(|s| s.0).unwrap_or(0);
                    t.current.get_or_insert_with(|| d.file_name.clone());
                }
            }
        }
    }
    t
}

impl App {
    /// Start following a copy. Returns its id.
    pub(super) fn copy_run_start(&mut self, title: String) -> u64 {
        self.copy_seq += 1;
        self.copies.push(CopyRun { id: self.copy_seq, title, ids: Vec::new(), queuing: true, failed: Vec::new(), finished_at: None });
        self.copy_seq
    }

    /// More items were queued, or the last of them was.
    pub(crate) fn copy_run_update(&mut self, run: u64, ids: Vec<String>, failed: Vec<String>, finished: bool) {
        if let Some(r) = self.copies.iter_mut().find(|r| r.id == run) {
            r.ids.extend(ids);
            r.failed.extend(failed);
            r.queuing &= !finished;
        }
        // The new items are in the database already; read them now rather than at the next tick.
        self.refresh();
    }

    /// Draw one pop-up per copy in progress. Called every frame from the main window.
    pub(crate) fn copy_windows(&mut self, ctx: &egui::Context) {
        if self.copies.is_empty() {
            return;
        }
        let frame = egui::Frame::window(&ctx.global_style()).fill(SURFACE).inner_margin(egui::Margin::same(20));
        let mut cancel: Vec<String> = Vec::new();
        let mut close: Vec<u64> = Vec::new();
        for (n, run) in self.copies.iter_mut().enumerate() {
            let t = tally(&run.ids, &self.downloads, &self.live_bytes, &self.live);
            let finished = !run.queuing && t.settled();
            if finished && run.finished_at.is_none() {
                run.finished_at = Some(Instant::now());
            }
            let clean = finished && t.errors.is_empty() && run.failed.is_empty() && t.cancelled == 0 && t.files > 0;
            if clean && run.finished_at.is_some_and(|at| at.elapsed() > LINGER) {
                close.push(run.id);
                continue;
            }
            egui::Window::new(format!("copy-run-{}", run.id))
                .title_bar(false)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 36.0 * n as f32])
                .fixed_size([460.0, 0.0])
                .frame(frame)
                .show(ctx, |ui| {
                    ui.label(RichText::new(&run.title).font(theme::bold(15.0)));
                    ui.add_space(6.0);
                    if run.queuing {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            let what = if t.files == 0 { "Membaca isi folder dan menyiapkan salinan...".to_string() } else { format!("Menyiapkan salinan... {} berkas sudah masuk antrean", t.files) };
                            ui.label(RichText::new(what).color(FG_SECONDARY));
                        });
                    }
                    if t.files > 0 {
                        let bar = egui::ProgressBar::new(t.fraction()).desired_width(f32::INFINITY).fill(if finished && t.errors.is_empty() { SUCCESS } else { ACCENT }).text(format!("{:.0}%", t.fraction() * 100.0));
                        ui.add(bar);
                        ui.add_space(4.0);
                        let size = if t.unknown == 0 { format!("{} dari {}", format::bytes(t.bytes_done), format::bytes(t.bytes_total)) } else { format::bytes(t.bytes_done) };
                        ui.label(RichText::new(format!("{} dari {} berkas selesai \u{b7} {size}", t.done, t.files)).color(FG_SECONDARY));
                        if !finished {
                            let eta = if t.unknown == 0 { format::eta(t.bytes_total - t.bytes_done, t.speed) } else { "-".into() };
                            ui.label(RichText::new(format!("{} \u{b7} sisa {eta}", format::speed(t.speed))).color(FG_SECONDARY));
                            if let Some(name) = &t.current {
                                ui.add(egui::Label::new(RichText::new(name).small().color(MUTED)).truncate());
                            }
                        }
                    }
                    if finished && t.errors.is_empty() && run.failed.is_empty() && t.cancelled == 0 && t.files > 0 {
                        ui.label(RichText::new("Selesai disalin.").color(SUCCESS).font(theme::bold(13.0)));
                    }
                    if t.cancelled > 0 {
                        ui.label(RichText::new(format!("{} berkas dibatalkan.", t.cancelled)).color(WARNING));
                    }
                    let problems: Vec<&String> = run.failed.iter().chain(t.errors.iter()).collect();
                    for p in problems.iter().take(4) {
                        ui.add(egui::Label::new(RichText::new(format!("\u{2022} {p}")).color(DANGER)).wrap());
                    }
                    if problems.len() > 4 {
                        ui.label(RichText::new(format!("dan {} lainnya", problems.len() - 4)).color(DANGER));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if finished {
                            if ui.button("Tutup").clicked() {
                                close.push(run.id);
                            }
                        } else {
                            if ui.add_enabled(!run.queuing, egui::Button::new("Batalkan salinan")).on_hover_text("Hentikan berkas yang belum selesai. Yang sudah selesai tetap ada di tujuan.").clicked() {
                                cancel.extend(run.ids.iter().cloned());
                                close.push(run.id);
                            }
                            if ui.button("Sembunyikan").on_hover_text("Salinan tetap berjalan; lihat di layar Unduhan.").clicked() {
                                close.push(run.id);
                            }
                        }
                    });
                });
        }
        self.copies.retain(|r| !close.contains(&r.id));
        for id in cancel {
            let m = self.manager.clone();
            // Already finished items refuse to be cancelled; that is fine.
            self.rt.spawn(async move {
                let _ = m.cancel(&id, false).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dl(id: &str, state: DownloadState, done: i64, total: i64) -> Download {
        Download {
            id: id.into(),
            original_url: String::new(),
            effective_url: String::new(),
            target_path: String::new(),
            file_name: format!("{id}.bin"),
            total_bytes: total,
            downloaded_bytes: done,
            state,
            segmentation_supported: false,
            connection_count: 1,
            speed_limit_kbps: 0,
            category_id: None,
            mime_type: None,
            referer: None,
            user_agent: None,
            cookies: None,
            error_code: None,
            error_message: None,
            created_at: String::new(),
            updated_at: String::new(),
            completed_at: None,
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn bytes_come_from_live_numbers_and_the_bar_follows_them() {
        let d = [dl("a", DownloadState::Completed, 100, 100), dl("b", DownloadState::Running, 0, 300)];
        let live: HashMap<String, i64> = [("b".to_string(), 100)].into();
        let speeds: HashMap<String, (u64, u32)> = [("b".to_string(), (2048, 1))].into();
        let t = tally(&ids(&["a", "b"]), &d, &live, &speeds);
        assert_eq!((t.files, t.done, t.bytes_done, t.bytes_total, t.speed), (2, 1, 200, 400, 2048));
        assert_eq!(t.current.as_deref(), Some("b.bin"));
        assert!(!t.settled());
        assert_eq!(t.fraction(), 0.5);
    }

    #[test]
    fn unknown_sizes_make_the_bar_count_files() {
        let d = [dl("a", DownloadState::Completed, 50, 50), dl("b", DownloadState::Queued, 0, -1)];
        let t = tally(&ids(&["a", "b"]), &d, &HashMap::new(), &HashMap::new());
        assert_eq!(t.unknown, 1);
        assert_eq!(t.fraction(), 0.5);
    }

    #[test]
    fn failed_and_cancelled_items_settle_the_copy_without_blocking_the_bar() {
        let mut bad = dl("b", DownloadState::Failed, 0, 10);
        bad.error_message = Some("sambungan putus".into());
        let d = [dl("a", DownloadState::Completed, 10, 10), bad, dl("c", DownloadState::Cancelled, 0, 10)];
        let t = tally(&ids(&["a", "b", "c"]), &d, &HashMap::new(), &HashMap::new());
        assert!(t.settled());
        assert_eq!((t.done, t.cancelled), (1, 1));
        assert_eq!(t.errors, ["b.bin: sambungan putus"]);
        assert_eq!(t.fraction(), 1.0);
    }

    #[test]
    fn items_removed_from_the_list_are_forgotten() {
        let d = [dl("a", DownloadState::Running, 5, 10)];
        let t = tally(&ids(&["a", "gone"]), &d, &HashMap::new(), &HashMap::new());
        assert_eq!(t.files, 1);
    }

    #[test]
    fn nothing_queued_counts_as_settled() {
        let t = tally(&[], &[], &HashMap::new(), &HashMap::new());
        assert!(t.settled());
        assert_eq!(t.fraction(), 1.0);
    }
}
