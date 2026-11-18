//! Dialog "Terminal · <server>": sebelum tab SSH dibuka, pengguna memilih shell langsung, sesi tmux
//! yang sedang aktif di server, atau sesi tmux baru. Daftar sesi diambil lewat koneksi singkat yang
//! dimulai saat tombol Terminal ditekan (tidak ada yang terhubung sebelum itu).
//!
//! Child module dari `app` (lihat app.rs) supaya bisa memakai field privat `App`.

use super::servers::{ServersMsg, UiInteract};
use super::{App, Msg};
use crate::theme::{self, *};
use eframe::egui::{self, Color32, RichText};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use unduhkilat_core::server::tmux::{validate_name, ShellLaunch, TmuxProbe, TmuxSession};

/// Id di `shell_waiters` untuk koneksi pengecekan tmux (tidak bentrok dengan id tab, yang kecil).
pub const PROBE_WAITER: u64 = u64::MAX;

pub enum Phase {
    Probing,
    Ready(Vec<TmuxSession>),
    /// tmux tidak terpasang di server.
    Missing,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Direct,
    Attach,
    New,
}

pub struct TmuxDialog {
    pub server_id: String,
    server_name: String,
    host: String,
    phase: Phase,
    mode: Mode,
    /// Pengguna sudah memilih sendiri: hasil pengecekan tidak boleh mengubah pilihannya.
    user_chose: bool,
    pick: Option<String>,
    take_over: bool,
    name: String,
    cancel: CancellationToken,
    /// Nomor pengecekan yang sedang ditunggu; hasil dengan nomor lain sudah basi.
    pub generation: u64,
}

/// "baru saja", "5 mnt lalu", "3 jam lalu", "2 hari lalu".
pub fn ago(now: u64, then: u64) -> String {
    let d = now.saturating_sub(then);
    match d {
        0..=59 => "baru saja".to_string(),
        60..=3599 => format!("{} mnt lalu", d / 60),
        3600..=86399 => format!("{} jam lalu", d / 3600),
        _ => format!("{} hari lalu", d / 86400),
    }
}

/// Nama awal untuk sesi baru: kata pertama nama server, dibuat unik terhadap sesi yang ada.
pub fn suggest_name(server_name: &str, taken: &[TmuxSession]) -> String {
    let base: String = server_name.split_whitespace().next().unwrap_or("sesi").to_lowercase().chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
    let base = if base.is_empty() { "sesi".to_string() } else { base };
    let used = |n: &str| taken.iter().any(|s| s.name == n);
    if !used(&base) {
        return base;
    }
    (2..).map(|k| format!("{base}-{k}")).find(|n| !used(n)).expect("tak terhingga")
}

impl TmuxDialog {
    fn new(server_id: String, server_name: String, host: String, generation: u64) -> Self {
        TmuxDialog { server_id, server_name, host, phase: Phase::Probing, mode: Mode::Direct, user_chose: false, pick: None, take_over: false, name: String::new(), cancel: CancellationToken::new(), generation }
    }

    fn sessions(&self) -> &[TmuxSession] {
        match &self.phase {
            Phase::Ready(v) => v,
            _ => &[],
        }
    }

    fn tmux_available(&self) -> bool {
        matches!(self.phase, Phase::Ready(_))
    }

    /// Alasan nama sesi baru ditolak, atau `None` bila boleh.
    fn name_error(&self) -> Option<&'static str> {
        if let Err(e) = validate_name(&self.name) {
            return Some(e);
        }
        self.sessions().iter().any(|s| s.name == self.name).then_some("Nama sudah dipakai sesi lain.")
    }

    /// Apa yang dibuka bila "Buka" ditekan, atau `None` bila pilihan belum lengkap.
    pub fn launch(&self) -> Option<ShellLaunch> {
        match self.mode {
            Mode::Direct => Some(ShellLaunch::Direct),
            Mode::Attach if self.tmux_available() => {
                let s = self.sessions().iter().find(|s| Some(&s.name) == self.pick.as_ref())?;
                Some(ShellLaunch::TmuxAttach { name: s.name.clone(), take_over: self.take_over && s.attached > 0 })
            }
            Mode::New if self.tmux_available() && self.name_error().is_none() => Some(ShellLaunch::TmuxNew { name: self.name.clone() }),
            _ => None,
        }
    }

    /// Hasil pengecekan tiba: isi daftar dan pilih mode awal yang masuk akal.
    fn apply_probe(&mut self, result: Result<TmuxProbe, unduhkilat_core::AppError>) {
        self.phase = match result {
            Ok(TmuxProbe::Sessions(v)) => Phase::Ready(v),
            Ok(TmuxProbe::Missing) => Phase::Missing,
            Ok(TmuxProbe::Failed(m)) => Phase::Failed(m),
            Err(e) => Phase::Failed(e.message),
        };
        let first_free = self.sessions().iter().find(|s| s.attached == 0).or_else(|| self.sessions().first()).map(|s| s.name.clone());
        // Sesi yang tadinya dipilih hilang dari server: pilih ulang.
        if !self.sessions().iter().any(|s| Some(&s.name) == self.pick.as_ref()) {
            self.pick = first_free;
        }
        if self.name.is_empty() {
            self.name = suggest_name(&self.server_name, self.sessions());
        }
        if !self.tmux_available() {
            self.mode = Mode::Direct;
        } else if !self.user_chose {
            self.mode = if self.sessions().is_empty() { Mode::New } else { Mode::Attach };
        } else if self.mode == Mode::Attach && self.sessions().is_empty() {
            self.mode = Mode::New;
        }
    }

    fn command_preview(&self) -> String {
        match self.launch() {
            Some(ShellLaunch::Direct) => "Perintah: shell login biasa".to_string(),
            Some(ShellLaunch::TmuxAttach { name, take_over }) => format!("Perintah: tmux attach-session{} -t ={name}", if take_over { " -d" } else { "" }),
            Some(ShellLaunch::TmuxNew { name }) => format!("Perintah: tmux new-session -A -s {name}"),
            None => String::new(),
        }
    }
}

enum Action {
    Open,
    Cancel,
    Refresh,
}

fn window(title: &str, width: f32) -> egui::Window<'static> {
    egui::Window::new(title.to_string()).title_bar(false).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).fixed_size([width, 0.0])
}

fn frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::window(&ctx.global_style()).fill(SURFACE).inner_margin(egui::Margin::same(22)).shadow(egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(150) })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl App {
    /// Tombol Terminal: buka dialog dan mulai memeriksa tmux di server.
    pub(super) fn open_tmux_dialog(&mut self, server_id: &str) {
        if self.servers.tmux.is_some() {
            return;
        }
        let Some(server) = self.servers.servers.iter().find(|s| s.id == server_id).cloned() else { return };
        let host = format!("{}@{}:{}", server.username, server.host, server.port);
        self.servers.tmux_generation += 1;
        self.servers.tmux = Some(TmuxDialog::new(server.id.clone(), server.name.clone(), host, self.servers.tmux_generation));
        self.start_tmux_probe();
    }

    fn start_tmux_probe(&mut self) {
        let Some(d) = self.servers.tmux.as_mut() else { return };
        d.cancel.cancel();
        d.cancel = CancellationToken::new();
        d.phase = Phase::Probing;
        self.servers.tmux_generation += 1;
        d.generation = self.servers.tmux_generation;
        let (id, generation, cancel) = (d.server_id.clone(), d.generation, d.cancel.clone());
        self.servers.shell_waiters.insert(PROBE_WAITER);
        let (svc, tx, ctx) = (self.manager.servers(), self.tx.clone(), self.ctx.clone());
        let interact = Arc::new(UiInteract { tx: tx.clone(), ctx: ctx.clone() });
        self.rt.spawn(async move {
            let result = svc.probe_tmux(&id, interact, cancel).await;
            let _ = tx.send(Msg::Servers(ServersMsg::TmuxProbed { generation, result }));
            ctx.request_repaint();
        });
    }

    /// Hasil pengecekan tmux dari latar belakang.
    pub(super) fn tmux_probed(&mut self, generation: u64, result: Result<TmuxProbe, unduhkilat_core::AppError>) {
        let Some(d) = self.servers.tmux.as_mut() else { return };
        if d.generation != generation {
            return; // pengecekan lama (sudah dimuat ulang atau dibatalkan)
        }
        d.apply_probe(result);
        self.servers.shell_waiters.remove(&PROBE_WAITER);
        self.servers.clear_prompts_if_idle();
    }

    fn close_tmux_dialog(&mut self) -> Option<TmuxDialog> {
        let d = self.servers.tmux.take()?;
        d.cancel.cancel();
        self.servers.shell_waiters.remove(&PROBE_WAITER);
        self.servers.clear_prompts_if_idle();
        Some(d)
    }

    pub(super) fn tmux_dialog_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.servers.tmux.take() else { return };
        let mut action: Option<Action> = None;
        window("Terminal", 560.0).frame(frame(ctx)).show(ctx, |ui| {
            ui.label(RichText::new(format!("Terminal · {}", d.server_name)).font(theme::bold(16.0)));
            ui.label(RichText::new(&d.host).font(theme::mono(12.0)).color(FG_SECONDARY));
            ui.add_space(10.0);

            if d.mode != Mode::Direct && matches!(d.phase, Phase::Probing) {
                // Pilihan tmux belum bisa dipakai sebelum daftar tiba.
                d.mode = Mode::Direct;
            }
            let before = d.mode;
            ui.radio_value(&mut d.mode, Mode::Direct, RichText::new("Langsung, tanpa tmux").font(theme::bold(13.5)));
            ui.label(RichText::new("Shell biasa. Sesi berakhir kalau koneksi putus.").color(FG_SECONDARY).size(12.0));
            ui.add_space(8.0);

            let avail = d.tmux_available();
            let count = d.sessions().len();
            ui.horizontal(|ui| {
                ui.add_enabled_ui(avail, |ui| ui.radio_value(&mut d.mode, Mode::Attach, RichText::new("Sambung ke sesi tmux").font(theme::bold(13.5))));
                let note = match &d.phase {
                    Phase::Probing => "memeriksa tmux…".to_string(),
                    Phase::Ready(_) if count == 0 => "belum ada sesi".to_string(),
                    Phase::Ready(_) => format!("{count} sesi"),
                    _ => "tidak tersedia".to_string(),
                };
                ui.label(RichText::new(note).font(theme::mono(11.5)).color(FG_SECONDARY));
            });
            match &d.phase {
                Phase::Missing => {
                    ui.label(RichText::new("tmux tidak ditemukan di server ini. Pasang dengan: sudo apt install tmux").color(DANGER).size(12.0));
                }
                Phase::Failed(m) => {
                    ui.label(RichText::new(format!("Tidak bisa memeriksa tmux: {m} Opsi Langsung tetap bisa dipakai.")).color(DANGER).size(12.0));
                }
                _ => {}
            }
            if d.mode == Mode::Attach && avail && count > 0 {
                let now = now_secs();
                let rows: Vec<(String, String)> = d
                    .sessions()
                    .iter()
                    .map(|s| {
                        let use_ = if s.attached > 0 { format!("dipakai {} klien", s.attached) } else { "bebas".to_string() };
                        (s.name.clone(), format!("{:<14} {} jendela · {} · aktif {}", s.name, s.windows, use_, ago(now, s.activity)))
                    })
                    .collect();
                egui::Frame::new().inner_margin(egui::Margin { left: 22, right: 0, top: 2, bottom: 2 }).show(ui, |ui| {
                    for (name, line) in rows {
                        let selected = d.pick.as_ref() == Some(&name);
                        if ui.selectable_label(selected, RichText::new(line).font(theme::mono(12.0))).clicked() {
                            d.pick = Some(name);
                            d.take_over = false;
                        }
                    }
                    let busy = d.sessions().iter().find(|s| Some(&s.name) == d.pick.as_ref()).is_some_and(|s| s.attached > 0);
                    if busy {
                        ui.checkbox(&mut d.take_over, "Ambil alih (lepas klien lain dari sesi ini)");
                    }
                });
            }
            if avail && ui.small_button("↻ Muat ulang daftar").clicked() {
                action = Some(Action::Refresh);
            }
            ui.add_space(8.0);

            ui.add_enabled_ui(avail, |ui| ui.radio_value(&mut d.mode, Mode::New, RichText::new("Buat sesi tmux baru").font(theme::bold(13.5))));
            if d.mode == Mode::New && avail {
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    ui.label("Nama");
                    ui.add(egui::TextEdit::singleline(&mut d.name).font(theme::mono(12.5)).desired_width(240.0));
                });
                if let Some(err) = d.name_error() {
                    ui.horizontal(|ui| {
                        ui.add_space(22.0);
                        ui.label(RichText::new(err).color(DANGER).size(12.0));
                    });
                }
            }
            if d.mode != before {
                d.user_chose = true;
            }

            ui.add_space(10.0);
            let preview = d.command_preview();
            if !preview.is_empty() {
                ui.label(RichText::new(preview).font(theme::mono(11.0)).color(FG_SECONDARY));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(d.launch().is_some(), primary_button("Buka ▸")).clicked() {
                    action = Some(Action::Open);
                }
                if ui.button("Batal").clicked() {
                    action = Some(Action::Cancel);
                }
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            action = Some(Action::Cancel);
        }
        self.servers.tmux = Some(d);
        match action {
            Some(Action::Cancel) => {
                self.close_tmux_dialog();
            }
            Some(Action::Refresh) => self.start_tmux_probe(),
            Some(Action::Open) => {
                if let Some(d) = self.close_tmux_dialog() {
                    if let Some(launch) = d.launch() {
                        self.open_ssh_terminal(&d.server_id, launch);
                    }
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(name: &str, attached: u32) -> TmuxSession {
        TmuxSession { name: name.into(), windows: 1, attached, activity: 100 }
    }

    fn dialog() -> TmuxDialog {
        TmuxDialog::new("id".into(), "Tunnel NAS2".into(), "eko@nas".into(), 1)
    }

    #[test]
    fn formats_relative_time() {
        assert_eq!(ago(1000, 990), "baru saja");
        assert_eq!(ago(1000, 1000 - 120), "2 mnt lalu");
        assert_eq!(ago(100_000, 100_000 - 7300), "2 jam lalu");
        assert_eq!(ago(1_000_000, 1_000_000 - 200_000), "2 hari lalu");
        assert_eq!(ago(5, 10), "baru saja", "jam server lebih maju dari jam lokal");
    }

    #[test]
    fn suggests_a_free_name() {
        assert_eq!(suggest_name("Tunnel NAS2", &[]), "tunnel");
        assert_eq!(suggest_name("Tunnel NAS2", &[s("tunnel", 0), s("tunnel-2", 0)]), "tunnel-3");
        assert_eq!(suggest_name("!!!", &[]), "sesi");
    }

    #[test]
    fn picks_a_sensible_mode_after_the_probe() {
        let mut d = dialog();
        d.apply_probe(Ok(TmuxProbe::Sessions(vec![s("toko", 1), s("api", 0)])));
        assert_eq!(d.mode, Mode::Attach);
        assert_eq!(d.pick.as_deref(), Some("api"), "sesi bebas lebih dulu dipilih");

        let mut d = dialog();
        d.apply_probe(Ok(TmuxProbe::Sessions(vec![])));
        assert_eq!(d.mode, Mode::New);
        assert_eq!(d.name, "tunnel");

        for r in [Ok(TmuxProbe::Missing), Ok(TmuxProbe::Failed("x".into())), Err(unduhkilat_core::AppError::new(unduhkilat_core::error::ErrorCode::ErrConnectionDropped, "putus"))] {
            let mut d = dialog();
            d.apply_probe(r);
            assert_eq!(d.mode, Mode::Direct);
            assert_eq!(d.launch(), Some(ShellLaunch::Direct), "Langsung selalu bisa dipakai");
        }
    }

    #[test]
    fn probe_result_never_overrides_the_users_choice() {
        let mut d = dialog();
        d.user_chose = true;
        d.mode = Mode::Direct;
        d.apply_probe(Ok(TmuxProbe::Sessions(vec![s("api", 0)])));
        assert_eq!(d.mode, Mode::Direct);
    }

    #[test]
    fn launch_follows_the_selection() {
        let mut d = dialog();
        d.apply_probe(Ok(TmuxProbe::Sessions(vec![s("toko", 1), s("api", 0)])));
        d.pick = Some("toko".into());
        d.take_over = true;
        assert_eq!(d.launch(), Some(ShellLaunch::TmuxAttach { name: "toko".into(), take_over: true }));
        d.pick = Some("api".into());
        assert_eq!(d.launch(), Some(ShellLaunch::TmuxAttach { name: "api".into(), take_over: false }), "ambil alih hanya untuk sesi yang dipakai");

        d.mode = Mode::New;
        d.name = "toko".into();
        assert_eq!(d.launch(), None, "nama bentrok");
        assert_eq!(d.name_error(), Some("Nama sudah dipakai sesi lain."));
        d.name = "a b".into();
        assert_eq!(d.launch(), None);
        d.name = "blog".into();
        assert_eq!(d.launch(), Some(ShellLaunch::TmuxNew { name: "blog".into() }));
    }

    #[test]
    fn refresh_keeps_choice_and_repicks_a_vanished_session() {
        let mut d = dialog();
        d.apply_probe(Ok(TmuxProbe::Sessions(vec![s("a", 0), s("b", 0)])));
        d.pick = Some("b".into());
        d.apply_probe(Ok(TmuxProbe::Sessions(vec![s("a", 0), s("b", 0)])));
        assert_eq!(d.pick.as_deref(), Some("b"));
        d.apply_probe(Ok(TmuxProbe::Sessions(vec![s("a", 0)])));
        assert_eq!(d.pick.as_deref(), Some("a"));
    }
}
