//! The "Server" screen: saved SSH/SFTP/FTP/FTPS servers, the add/edit dialog with the jump route,
//! host-key and password prompts, and "Uji sambungan".
//!
//! Compiled as a child module of `app` (`#[path]` in app.rs) so it can use `App`'s private fields.
//! Nothing here connects to a server until the user presses a button.

use super::{App, Msg};
use crate::theme::{self, *};
use eframe::egui::{self, Align, Color32, Layout, RichText, Stroke, Ui};
use futures_util::future::BoxFuture;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use unduhkilat_core::error::ErrorCode;
use unduhkilat_core::server::model::{AuthMethod, NewServer, Protocol, Server};
use unduhkilat_core::server::secrets::{NewServerSecrets, SecretKind, SecretString};
use unduhkilat_core::server::service::TestReport;
use unduhkilat_core::server::ssh::Interact;

const MAX_HOPS: usize = unduhkilat_core::server::store::MAX_HOPS;

fn protocol_color(p: Protocol) -> Color32 {
    match p {
        Protocol::Ssh => Color32::from_rgb(0x6D, 0x4A, 0xFF),
        Protocol::Sftp => Color32::from_rgb(0x0E, 0x7A, 0xE6),
        Protocol::Ftp => Color32::from_rgb(0x6B, 0x72, 0x80),
        Protocol::Ftps => Color32::from_rgb(0x12, 0x7A, 0x48),
    }
}

fn protocol_label(p: Protocol) -> &'static str {
    match p {
        Protocol::Ssh => "SSH",
        Protocol::Sftp => "SFTP",
        Protocol::Ftp => "FTP",
        Protocol::Ftps => "FTPS",
    }
}

// ---- messages from background tasks to the UI ----------------------------------------------------

pub enum ServersMsg {
    Loaded(Result<Vec<Server>, String>),
    Saved(Result<Server, String>),
    Deleted(Result<(), String>),
    TestDone { result: Result<TestReport, unduhkilat_core::AppError> },
    Forgotten(Result<(), String>),
    HostKey { host: String, port: u16, key_type: String, fingerprint: String, reply: oneshot::Sender<bool> },
    Secret { server_name: String, kind: SecretKind, reply: oneshot::Sender<Option<String>> },
}

/// Bridges the async connection code to dialogs: asks the UI and waits for the answer.
/// If the UI drops the question (window closed), the answer is "no".
pub struct UiInteract {
    pub tx: Sender<Msg>,
    pub ctx: egui::Context,
}

impl Interact for UiInteract {
    fn confirm_host_key<'a>(&'a self, host: &'a str, port: u16, key_type: &'a str, fingerprint: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let (reply, rx) = oneshot::channel();
            let msg = ServersMsg::HostKey { host: host.to_string(), port, key_type: key_type.to_string(), fingerprint: fingerprint.to_string(), reply };
            if self.tx.send(Msg::Servers(msg)).is_err() {
                return false;
            }
            self.ctx.request_repaint();
            rx.await.unwrap_or(false)
        })
    }

    fn secret<'a>(&'a self, server_name: &'a str, kind: SecretKind) -> BoxFuture<'a, Option<SecretString>> {
        Box::pin(async move {
            let (reply, rx) = oneshot::channel();
            if self.tx.send(Msg::Servers(ServersMsg::Secret { server_name: server_name.to_string(), kind, reply })).is_err() {
                return None;
            }
            self.ctx.request_repaint();
            rx.await.ok().flatten().map(SecretString::from)
        })
    }
}

// ---- state ----------------------------------------------------------------------------------------

#[derive(Default)]
pub struct ServersState {
    pub servers: Vec<Server>,
    pub loaded: bool,
    dialog: Option<ServerDialog>,
    testing: Option<Testing>,
    /// Last test outcome (text, is_error).
    result: Option<(String, bool)>,
    host_prompt: Option<HostPrompt>,
    secret_prompt: Option<SecretPrompt>,
    confirm_delete: Option<(String, String)>,
    /// Server whose route had a changed host key: offers the explicit "forget keys" action.
    changed_key: Option<ChangedKey>,
    confirm_forget: Option<ChangedKey>,
    /// Run a connection test right after the dialog's save succeeds.
    test_after_save: bool,
}

/// The one pinned host key that changed (taken from the error's details), so "forget" touches only it.
#[derive(Clone, Debug, PartialEq)]
struct ChangedKey {
    host: String,
    port: u16,
    known: String,
    presented: String,
}

fn changed_key_from(e: &unduhkilat_core::AppError) -> Option<ChangedKey> {
    if e.code != ErrorCode::ErrHostKeyChanged {
        return None;
    }
    let d = e.details.as_ref()?;
    Some(ChangedKey {
        host: d.get("host")?.as_str()?.to_string(),
        port: u16::try_from(d.get("port")?.as_u64()?).ok()?,
        known: d.get("known")?.as_str()?.to_string(),
        presented: d.get("presented")?.as_str()?.to_string(),
    })
}

impl ServersState {
    /// A host-key question from a connection. Only a running test is waiting for an answer: a question
    /// that arrives without one (the test was cancelled or timed out) is dropped, which the connection
    /// side reads as "no", so a stale dialog can never pin a key.
    fn offer_host_key(&mut self, host: String, port: u16, key_type: String, fingerprint: String, reply: oneshot::Sender<bool>) {
        if self.testing.is_some() {
            self.host_prompt = Some(HostPrompt { host, port, key_type, fingerprint, reply: Some(reply) });
        }
    }

    fn offer_secret(&mut self, server_name: String, kind: SecretKind, reply: oneshot::Sender<Option<String>>) {
        if self.testing.is_some() {
            self.secret_prompt = Some(SecretPrompt { server_name, kind, value: String::new(), reply: Some(reply) });
        }
    }
}

struct Testing {
    server_name: String,
    cancel: CancellationToken,
}

struct HostPrompt {
    host: String,
    port: u16,
    key_type: String,
    fingerprint: String,
    reply: Option<oneshot::Sender<bool>>,
}

struct SecretPrompt {
    server_name: String,
    kind: SecretKind,
    value: String,
    reply: Option<oneshot::Sender<Option<String>>>,
}

struct ServerDialog {
    id: Option<String>,
    name: String,
    protocol: Protocol,
    host: String,
    port: String,
    username: String,
    auth: AuthMethod,
    key_path: String,
    password: String,
    passphrase: String,
    save_secret: bool,
    had_saved_secret: bool,
    hops: Vec<Option<String>>,
    err: Option<String>,
    saving: bool,
}

impl ServerDialog {
    fn new() -> Self {
        ServerDialog { id: None, name: String::new(), protocol: Protocol::Ssh, host: String::new(), port: String::new(), username: String::new(), auth: AuthMethod::Password, key_path: String::new(), password: String::new(), passphrase: String::new(), save_secret: true, had_saved_secret: false, hops: Vec::new(), err: None, saving: false }
    }

    fn from(s: &Server) -> Self {
        ServerDialog {
            id: Some(s.id.clone()),
            name: s.name.clone(),
            protocol: s.protocol,
            host: s.host.clone(),
            port: s.port.to_string(),
            username: s.username.clone(),
            auth: s.auth,
            key_path: s.key_path.clone().unwrap_or_default(),
            password: String::new(),
            passphrase: String::new(),
            save_secret: s.save_secret,
            had_saved_secret: s.save_secret,
            hops: s.hops.iter().cloned().map(Some).collect(),
            err: None,
            saving: false,
        }
    }

    fn build(&self) -> Result<(NewServer, NewServerSecrets), String> {
        let port = match self.port.trim() {
            "" => None,
            p => Some(p.parse::<u16>().map_err(|_| "Port harus angka 1 sampai 65535.".to_string())?),
        };
        let hops = self.hops.iter().map(|h| h.clone().ok_or_else(|| "Pilih server untuk setiap lompatan, atau hapus lompatan yang kosong.".to_string())).collect::<Result<Vec<_>, _>>()?;
        let secret = |s: &str| (!s.is_empty()).then(|| SecretString::from(s));
        let new = NewServer {
            name: self.name.clone(),
            protocol: self.protocol,
            host: self.host.clone(),
            port,
            username: self.username.clone(),
            auth: self.auth,
            key_path: (self.auth == AuthMethod::Key).then(|| self.key_path.clone()),
            save_secret: self.save_secret,
            hops: if self.protocol.supports_jump() { hops } else { Vec::new() },
        };
        let secrets = NewServerSecrets {
            password: if self.auth == AuthMethod::Password { secret(&self.password) } else { None },
            passphrase: if self.auth == AuthMethod::Key { secret(&self.passphrase) } else { None },
        };
        Ok((new, secrets))
    }
}

fn node(ui: &mut Ui, text: &str, strong: bool) {
    let fill = if strong { ACCENT } else { SURFACE_RAISED };
    egui::Frame::new().fill(fill).stroke(Stroke::new(1.5, FG)).corner_radius(6).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
        let t = RichText::new(text).size(12.0);
        ui.label(if strong { t.font(theme::bold(12.0)) } else { t });
    });
}

impl App {
    fn svc(&self) -> unduhkilat_core::server::service::ServerService {
        self.manager.servers()
    }

    /// Test aid (`UNDUHKILAT_START_SCREEN=servers-new`): open the empty dialog.
    pub(super) fn servers_open_new_dialog(&mut self) {
        self.servers.dialog = Some(ServerDialog::new());
    }

    /// Reload the saved list from the local database (no network).
    pub(super) fn servers_reload(&mut self) {
        let (svc, tx, ctx) = (self.svc(), self.tx.clone(), self.ctx.clone());
        self.rt.spawn(async move {
            let r = svc.list().await.map_err(|e| e.message);
            let _ = tx.send(Msg::Servers(ServersMsg::Loaded(r)));
            ctx.request_repaint();
        });
    }

    fn start_server_test(&mut self, id: &str) {
        if self.servers.testing.is_some() {
            return;
        }
        let Some(server) = self.servers.servers.iter().find(|s| s.id == id).cloned() else { return };
        let cancel = CancellationToken::new();
        self.servers.testing = Some(Testing { server_name: server.name.clone(), cancel: cancel.clone() });
        self.servers.result = None;
        self.servers.changed_key = None;
        let (svc, tx, ctx) = (self.svc(), self.tx.clone(), self.ctx.clone());
        let interact = Arc::new(UiInteract { tx: tx.clone(), ctx: ctx.clone() });
        self.rt.spawn(async move {
            let result = svc.test_connection(&server.id, interact, cancel).await;
            let _ = tx.send(Msg::Servers(ServersMsg::TestDone { result }));
            ctx.request_repaint();
        });
    }

    pub(super) fn servers_msg(&mut self, m: ServersMsg) {
        match m {
            ServersMsg::Loaded(Ok(list)) => {
                self.servers.servers = list;
                self.servers.loaded = true;
            }
            ServersMsg::Loaded(Err(e)) => {
                self.servers.loaded = true;
                self.servers.result = Some((e, true));
            }
            ServersMsg::Saved(Ok(saved)) => {
                self.servers.dialog = None;
                self.servers_reload();
                if std::mem::take(&mut self.servers.test_after_save) {
                    // The list refresh is async; the test only needs the saved id and name.
                    if !self.servers.servers.iter().any(|s| s.id == saved.id) {
                        self.servers.servers.push(saved.clone());
                    }
                    self.start_server_test(&saved.id);
                }
            }
            ServersMsg::Saved(Err(e)) => {
                self.servers.test_after_save = false;
                if let Some(d) = self.servers.dialog.as_mut() {
                    d.saving = false;
                    d.err = Some(e);
                }
            }
            ServersMsg::Deleted(Ok(())) => self.servers_reload(),
            ServersMsg::Deleted(Err(e)) => self.servers.result = Some((e, true)),
            ServersMsg::Forgotten(Ok(())) => {
                self.servers.changed_key = None;
                self.servers.result = Some(("Kunci host dilupakan. Ujilah lagi untuk mengonfirmasi kunci yang baru.".into(), false));
            }
            ServersMsg::Forgotten(Err(e)) => self.servers.result = Some((e, true)),
            ServersMsg::TestDone { result } => {
                self.servers.testing = None;
                self.servers.host_prompt = None;
                self.servers.secret_prompt = None;
                match result {
                    Ok(r) => self.servers.result = Some((format!("Terhubung lewat {} dalam {} ms.", r.hop_names.join(" → "), r.rtt_ms), false)),
                    Err(e) => {
                        self.servers.changed_key = changed_key_from(&e);
                        self.servers.result = Some((e.message, true));
                    }
                }
            }
            ServersMsg::HostKey { host, port, key_type, fingerprint, reply } => self.servers.offer_host_key(host, port, key_type, fingerprint, reply),
            ServersMsg::Secret { server_name, kind, reply } => self.servers.offer_secret(server_name, kind, reply),
        }
    }

    fn server_subtitle(&self, s: &Server) -> String {
        let auth = match s.auth {
            AuthMethod::Password => if s.save_secret { "password tersimpan".to_string() } else { "password (ditanya saat masuk)".to_string() },
            AuthMethod::Key => format!("kunci {}", std::path::Path::new(s.key_path.as_deref().unwrap_or("")).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
            AuthMethod::Agent => "SSH agent".to_string(),
        };
        let via: Vec<String> = s.hops.iter().map(|id| self.servers.servers.iter().find(|x| &x.id == id).map(|x| x.name.clone()).unwrap_or_else(|| "?".into())).collect();
        let mut line = format!("{}@{}:{} · {}", s.username, s.host, s.port, auth);
        if !via.is_empty() {
            line.push_str(&format!(" · via {}", via.join(" → ")));
        }
        line
    }

    pub(super) fn servers_screen(&mut self, ui: &mut Ui) {
        if !self.servers.loaded {
            self.servers_reload();
        }
        ui.label(RichText::new("Server").font(theme::bold(17.0)));
        ui.label(RichText::new("SSH, SFTP, FTP, dan FTPS yang Anda simpan. Tidak ada yang terhubung sampai Anda menekan Uji sambungan.").color(FG_SECONDARY));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.add(primary_button("+ Server baru")).clicked() {
                self.servers.dialog = Some(ServerDialog::new());
            }
        });
        ui.add_space(10.0);

        let list = self.servers.servers.clone();
        if list.is_empty() && self.servers.loaded {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Belum ada server").font(theme::bold(16.0)));
                ui.label(RichText::new("Simpan server SSH atau FTP beserta kredensialnya, termasuk server yang hanya bisa dicapai lewat jump host.").color(FG_SECONDARY));
            });
            return;
        }

        let mut test: Option<String> = None;
        let mut edit: Option<Server> = None;
        let mut delete: Option<(String, String)> = None;
        let mut cancel_test = false;
        let busy = self.servers.testing.is_some();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for s in &list {
                let subtitle = self.server_subtitle(s);
                let testing_this = self.servers.testing.as_ref().is_some_and(|t| t.server_name == s.name);
                egui::Frame::new()
                    .fill(SURFACE)
                    .stroke(Stroke::new(2.0, FG))
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .shadow(egui::epaint::Shadow { offset: [2, 2], blur: 0, spread: 0, color: FG })
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!(" {} ", protocol_label(s.protocol))).font(theme::mono(11.0)).color(Color32::WHITE).background_color(protocol_color(s.protocol)));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&s.name).font(theme::bold(14.0)));
                                ui.label(RichText::new(subtitle).font(theme::mono(11.5)).color(FG_SECONDARY));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Hapus").clicked() {
                                    delete = Some((s.id.clone(), s.name.clone()));
                                }
                                if ui.button("Ubah").clicked() {
                                    edit = Some(s.clone());
                                }
                                if testing_this {
                                    if ui.button("Batal").clicked() {
                                        cancel_test = true;
                                    }
                                    ui.label(RichText::new("Menghubungkan...").color(FG_SECONDARY));
                                } else if ui.add_enabled(!busy, egui::Button::new("Uji sambungan")).clicked() {
                                    test = Some(s.id.clone());
                                }
                                ui.add_enabled(false, egui::Button::new("Berkas")).on_disabled_hover_text("Segera hadir");
                                if s.protocol.supports_jump() {
                                    ui.add_enabled(false, egui::Button::new("Terminal")).on_disabled_hover_text("Segera hadir");
                                }
                            });
                        });
                    });
                ui.add_space(8.0);
            }
            if let Some((text, is_err)) = &self.servers.result {
                ui.add_space(4.0);
                ui.label(RichText::new(text).color(if *is_err { DANGER } else { SUCCESS }));
                if self.servers.changed_key.is_some() && ui.button("Ganti kunci host...").clicked() {
                    self.servers.confirm_forget = self.servers.changed_key.clone();
                }
            }
        });
        if let Some(id) = test {
            self.start_server_test(&id);
        }
        if cancel_test {
            if let Some(t) = &self.servers.testing {
                t.cancel.cancel();
            }
            self.servers.host_prompt = None;
            self.servers.secret_prompt = None;
        }
        if let Some(s) = edit {
            self.servers.dialog = Some(ServerDialog::from(&s));
        }
        if delete.is_some() {
            self.servers.confirm_delete = delete;
        }
    }

    /// Every window of this screen: the server dialog and the prompts that interrupt a connection.
    pub(super) fn servers_dialogs(&mut self, ctx: &egui::Context) {
        self.host_prompt_window(ctx);
        self.secret_prompt_window(ctx);
        self.confirm_windows(ctx);
        self.server_dialog(ctx);
    }

    fn window(title: &str, width: f32) -> egui::Window<'static> {
        egui::Window::new(title.to_string()).title_bar(false).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).fixed_size([width, 0.0])
    }

    fn frame(ctx: &egui::Context) -> egui::Frame {
        egui::Frame::window(&ctx.global_style()).fill(SURFACE).inner_margin(egui::Margin::same(22)).shadow(egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(150) })
    }

    fn host_prompt_window(&mut self, ctx: &egui::Context) {
        let Some(mut p) = self.servers.host_prompt.take() else { return };
        let mut answer: Option<bool> = None;
        Self::window("Percayai server ini?", 520.0).frame(Self::frame(ctx)).show(ctx, |ui| {
            ui.label(RichText::new("Percayai server ini?").font(theme::bold(16.0)));
            ui.label(RichText::new(format!("Pertama kali terhubung ke {}:{}.", p.host, p.port)).color(FG_SECONDARY));
            ui.add_space(6.0);
            ui.label(format!("Jenis kunci: {}", p.key_type));
            ui.label("Sidik jari:");
            ui.add(egui::TextEdit::singleline(&mut p.fingerprint.clone()).font(theme::mono(12.0)).desired_width(f32::INFINITY));
            ui.add_space(4.0);
            ui.label(RichText::new("Bandingkan dengan sidik jari yang Anda ketahui dari server ini. Bila berbeda, jangan lanjut: bisa jadi sambungan Anda disadap.").color(FG_SECONDARY).size(12.0));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(primary_button("Percayai dan lanjut")).clicked() {
                    answer = Some(true);
                }
                if ui.button("Batal").clicked() {
                    answer = Some(false);
                }
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = Some(false);
        }
        match answer {
            Some(a) => {
                if let Some(r) = p.reply.take() {
                    let _ = r.send(a);
                }
            }
            None => self.servers.host_prompt = Some(p),
        }
    }

    fn secret_prompt_window(&mut self, ctx: &egui::Context) {
        let Some(mut p) = self.servers.secret_prompt.take() else { return };
        let mut answer: Option<Option<String>> = None;
        let what = match p.kind {
            SecretKind::Password => "password",
            SecretKind::Passphrase => "frasa sandi kunci",
        };
        Self::window("Kredensial", 440.0).frame(Self::frame(ctx)).show(ctx, |ui| {
            ui.label(RichText::new(format!("Masukkan {what}")).font(theme::bold(16.0)));
            ui.label(RichText::new(format!("untuk server \"{}\". Disimpan hanya bila \"Simpan kredensial\" aktif pada server itu.", p.server_name)).color(FG_SECONDARY));
            ui.add_space(6.0);
            let r = ui.add(egui::TextEdit::singleline(&mut p.value).password(true).desired_width(f32::INFINITY));
            r.request_focus();
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(primary_button("Masuk")).clicked() || enter {
                    answer = Some(Some(p.value.clone()));
                }
                if ui.button("Batal").clicked() {
                    answer = Some(None);
                }
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = Some(None);
        }
        match answer {
            Some(a) => {
                if let Some(r) = p.reply.take() {
                    let _ = r.send(a);
                }
            }
            None => self.servers.secret_prompt = Some(p),
        }
    }

    fn confirm_windows(&mut self, ctx: &egui::Context) {
        if let Some((id, name)) = self.servers.confirm_delete.clone() {
            let mut done: Option<bool> = None;
            Self::window("Hapus server", 440.0).frame(Self::frame(ctx)).show(ctx, |ui| {
                ui.label(RichText::new(format!("Hapus server \"{name}\"?")).font(theme::bold(16.0)));
                ui.label(RichText::new("Kredensial yang tersimpan untuk server ini ikut terhapus. Berkas di server tidak disentuh.").color(FG_SECONDARY));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Hapus").color(Color32::WHITE).font(theme::bold(13.5))).fill(DANGER).stroke(Stroke::NONE)).clicked() {
                        done = Some(true);
                    }
                    if ui.button("Batal").clicked() {
                        done = Some(false);
                    }
                });
            });
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(false);
            }
            match done {
                Some(true) => {
                    self.servers.confirm_delete = None;
                    let (svc, tx, ctx2) = (self.svc(), self.tx.clone(), self.ctx.clone());
                    self.rt.spawn(async move {
                        let r = svc.delete(&id).await.map_err(|e| e.message);
                        let _ = tx.send(Msg::Servers(ServersMsg::Deleted(r)));
                        ctx2.request_repaint();
                    });
                }
                Some(false) => self.servers.confirm_delete = None,
                None => {}
            }
        }
        if let Some(k) = self.servers.confirm_forget.clone() {
            let mut done: Option<bool> = None;
            Self::window("Ganti kunci host", 520.0).frame(Self::frame(ctx)).show(ctx, |ui| {
                ui.label(RichText::new("Kunci host berubah").font(theme::bold(16.0)).color(DANGER));
                ui.label(format!("Server {}:{} memakai kunci yang berbeda dari yang tersimpan. Itu normal bila server baru dipasang ulang, tetapi juga tanda sambungan yang disadap.", k.host, k.port));
                ui.label("Tersimpan:");
                ui.label(RichText::new(&k.known).font(theme::mono(12.0)));
                ui.label("Sekarang:");
                ui.label(RichText::new(&k.presented).font(theme::mono(12.0)));
                ui.label(RichText::new("Melupakan kunci akan membuat sambungan berikutnya meminta konfirmasi sidik jari yang baru. Lanjutkan hanya bila Anda yakin.").color(FG_SECONDARY));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Lupakan kunci lama").color(Color32::WHITE).font(theme::bold(13.5))).fill(DANGER).stroke(Stroke::NONE)).clicked() {
                        done = Some(true);
                    }
                    if ui.button("Batal").clicked() {
                        done = Some(false);
                    }
                });
            });
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(false);
            }
            match done {
                Some(true) => {
                    self.servers.confirm_forget = None;
                    // Only the hop whose key changed: the other pins on the route stay.
                    let (svc, tx, ctx2) = (self.svc(), self.tx.clone(), self.ctx.clone());
                    self.rt.spawn(async move {
                        let r = svc.forget_host_key(&k.host, k.port).await.map_err(|e| e.message);
                        let _ = tx.send(Msg::Servers(ServersMsg::Forgotten(r)));
                        ctx2.request_repaint();
                    });
                }
                Some(false) => self.servers.confirm_forget = None,
                None => {}
            }
        }
    }

    fn server_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.servers.dialog.take() else { return };
        let others: Vec<Server> = self.servers.servers.iter().filter(|s| s.protocol == Protocol::Ssh && Some(&s.id) != d.id.as_ref()).cloned().collect();
        let name_of = |id: &str| self.servers.servers.iter().find(|s| s.id == id).map(|s| s.name.clone());
        let mut close = false;
        let mut submit: Option<bool> = None; // Some(also_test)
        let prompt_open = self.servers.host_prompt.is_some() || self.servers.secret_prompt.is_some();
        let title = if d.id.is_some() { "Ubah server" } else { "Server baru" };
        Self::window(title, 580.0).frame(Self::frame(ctx)).show(ctx, |ui| {
            ui.label(RichText::new(title).font(theme::bold(17.0)));
            ui.label(RichText::new("Nama, alamat, dan cara masuk. Tidak ada yang terhubung sampai Anda menekan Simpan dan uji.").color(FG_SECONDARY));
            ui.add_space(8.0);
            egui::Grid::new("server-form").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
                ui.label("Nama:");
                ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY));
                ui.end_row();

                ui.label("Protokol:");
                ui.horizontal(|ui| {
                    for p in [Protocol::Ssh, Protocol::Sftp, Protocol::Ftp, Protocol::Ftps] {
                        if ui.selectable_label(d.protocol == p, protocol_label(p)).clicked() && d.protocol != p {
                            d.protocol = p;
                            if !p.supports_jump() {
                                d.auth = AuthMethod::Password;
                                d.hops.clear();
                            }
                        }
                    }
                });
                ui.end_row();

                ui.label("Alamat / port:");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.host).desired_width(300.0).font(theme::mono(12.0)).hint_text("contoh.com atau 10.0.1.20"));
                    ui.add(egui::TextEdit::singleline(&mut d.port).desired_width(64.0).hint_text(d.protocol.default_port().to_string()));
                });
                ui.end_row();

                ui.label("Pengguna:");
                ui.add(egui::TextEdit::singleline(&mut d.username).desired_width(240.0));
                ui.end_row();

                ui.label("Cara masuk:");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut d.auth, AuthMethod::Password, "Password");
                    if d.protocol.supports_jump() {
                        ui.selectable_value(&mut d.auth, AuthMethod::Key, "Kunci pribadi");
                        ui.add_enabled(false, egui::Button::new("Agent")).on_disabled_hover_text("Segera hadir");
                    }
                });
                ui.end_row();

                match d.auth {
                    AuthMethod::Password => {
                        ui.label("Password:");
                        let hint = if d.had_saved_secret { "(tersimpan; kosongkan untuk mempertahankan)" } else { "" };
                        ui.add(egui::TextEdit::singleline(&mut d.password).password(true).desired_width(300.0).hint_text(hint));
                        ui.end_row();
                    }
                    AuthMethod::Key => {
                        ui.label("Berkas kunci:");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut d.key_path).desired_width(300.0).font(theme::mono(12.0)).hint_text("C:\\Users\\...\\.ssh\\id_ed25519"));
                            if ui.button("Telusuri").clicked() {
                                if let Some(p) = rfd::FileDialog::new().pick_file() {
                                    d.key_path = p.to_string_lossy().into_owned();
                                }
                            }
                        });
                        ui.end_row();
                        ui.label("Frasa sandi:");
                        let hint = if d.had_saved_secret { "(tersimpan; kosongkan untuk mempertahankan)" } else { "kosongkan bila kunci tanpa frasa sandi" };
                        ui.add(egui::TextEdit::singleline(&mut d.passphrase).password(true).desired_width(300.0).hint_text(hint));
                        ui.end_row();
                    }
                    AuthMethod::Agent => {}
                }

                ui.label("");
                ui.checkbox(&mut d.save_secret, "Simpan kredensial (dienkripsi Windows)");
                ui.end_row();
            });

            if d.protocol.supports_jump() {
                ui.add_space(6.0);
                ui.separator();
                ui.label(RichText::new("Lewat jump host").font(theme::bold(13.5)));
                ui.label(RichText::new("Untuk server yang hanya bisa dicapai lewat server lain, tanpa VPN. Urutan dari atas ke bawah.").color(FG_SECONDARY).size(12.0));
                let mut remove: Option<usize> = None;
                for i in 0..d.hops.len() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{}.", i + 1));
                        let shown = d.hops[i].as_deref().and_then(&name_of).unwrap_or_else(|| "(pilih server SSH)".into());
                        egui::ComboBox::from_id_salt(("hop", i)).selected_text(shown).width(260.0).show_ui(ui, |ui| {
                            for o in &others {
                                if ui.selectable_label(d.hops[i].as_deref() == Some(o.id.as_str()), &o.name).clicked() {
                                    d.hops[i] = Some(o.id.clone());
                                }
                            }
                        });
                        if ui.small_button("Hapus").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    d.hops.remove(i);
                }
                if d.hops.len() < MAX_HOPS && ui.add_enabled(!others.is_empty(), egui::Button::new("+ lompatan")).on_disabled_hover_text("Simpan dulu server SSH lain untuk dipakai sebagai jump host").clicked() {
                    d.hops.push(None);
                }
                if !d.hops.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        node(ui, "Komputer ini", false);
                        for h in &d.hops {
                            ui.label("→");
                            node(ui, &h.as_deref().and_then(&name_of).unwrap_or_else(|| "?".into()), false);
                        }
                        ui.label("→");
                        node(ui, if d.name.trim().is_empty() { "server ini" } else { d.name.trim() }, true);
                    });
                }
            }

            if let Some(e) = &d.err {
                ui.add_space(6.0);
                ui.label(RichText::new(e).color(DANGER));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!d.saving, |ui| {
                    if ui.add(primary_button("Simpan")).clicked() {
                        submit = Some(false);
                    }
                    if ui.button("Simpan dan uji").clicked() {
                        submit = Some(true);
                    }
                });
                if ui.button("Batal").clicked() {
                    close = true;
                }
            });
        });
        if !prompt_open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }

        if let Some(also_test) = submit {
            match d.build() {
                Ok((new, secrets)) => {
                    d.saving = true;
                    d.err = None;
                    self.servers.test_after_save = also_test;
                    let (svc, tx, ctx2, id) = (self.svc(), self.tx.clone(), self.ctx.clone(), d.id.clone());
                    self.rt.spawn(async move {
                        let r = svc.save(id, new, secrets).await.map_err(|e| e.message);
                        let _ = tx.send(Msg::Servers(ServersMsg::Saved(r)));
                        ctx2.request_repaint();
                    });
                }
                Err(e) => d.err = Some(e),
            }
        }
        if !close {
            self.servers.dialog = Some(d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn interact() -> (UiInteract, std::sync::mpsc::Receiver<Msg>) {
        let (tx, rx) = channel();
        (UiInteract { tx, ctx: egui::Context::default() }, rx)
    }

    #[tokio::test]
    async fn host_key_question_reaches_the_ui_and_waits_for_the_answer() {
        let (ui, rx) = interact();
        let ask = tokio::spawn(async move { ui.confirm_host_key("example.com", 22, "ssh-ed25519", "SHA256:abc").await });
        let msg = loop {
            match rx.try_recv() {
                Ok(m) => break m,
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
            }
        };
        let Msg::Servers(ServersMsg::HostKey { host, port, key_type, fingerprint, reply }) = msg else { panic!("expected a host key prompt") };
        assert_eq!((host.as_str(), port, key_type.as_str(), fingerprint.as_str()), ("example.com", 22, "ssh-ed25519", "SHA256:abc"));
        assert!(!ask.is_finished(), "the connection waits for the user");
        reply.send(true).unwrap();
        assert!(ask.await.unwrap(), "the user's yes is passed back");
    }

    #[tokio::test]
    async fn a_dropped_prompt_counts_as_no() {
        let (ui, rx) = interact();
        let ask = tokio::spawn(async move { (ui.confirm_host_key("h", 22, "t", "f").await, ui.secret("web", SecretKind::Password).await.map(|s| s.expose().to_string())) });
        // Drop the replies without answering (the dialog was closed or the app is exiting).
        let mut seen = 0;
        while seen < 2 {
            match rx.try_recv() {
                Ok(Msg::Servers(ServersMsg::HostKey { reply, .. })) => {
                    drop(reply);
                    seen += 1;
                }
                Ok(Msg::Servers(ServersMsg::Secret { reply, .. })) => {
                    drop(reply);
                    seen += 1;
                }
                Ok(_) => {}
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
            }
        }
        assert_eq!(ask.await.unwrap(), (false, None));
    }

    #[tokio::test]
    async fn a_typed_secret_comes_back_as_a_secret() {
        let (ui, rx) = interact();
        let ask = tokio::spawn(async move { ui.secret("web", SecretKind::Passphrase).await.map(|s| s.expose().to_string()) });
        let msg = loop {
            match rx.try_recv() {
                Ok(m) => break m,
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
            }
        };
        let Msg::Servers(ServersMsg::Secret { server_name, kind, reply }) = msg else { panic!("expected a secret prompt") };
        assert_eq!((server_name.as_str(), kind), ("web", SecretKind::Passphrase));
        reply.send(Some("frasa".into())).unwrap();
        assert_eq!(ask.await.unwrap().as_deref(), Some("frasa"));
    }

    #[test]
    fn a_prompt_arriving_without_a_running_test_is_answered_no() {
        let mut st = ServersState::default(); // no connection test is running
        let (reply, mut rx) = oneshot::channel();
        st.offer_host_key("h".into(), 22, "ssh-ed25519".into(), "SHA256:x".into(), reply);
        assert!(st.host_prompt.is_none(), "no window for a question nobody is waiting on");
        assert!(rx.try_recv().is_err(), "the reply was dropped, which the connection side reads as 'no'");
        let (reply, mut rx) = oneshot::channel();
        st.offer_secret("web".into(), SecretKind::Password, reply);
        assert!(st.secret_prompt.is_none());
        assert!(rx.try_recv().is_err());
        st.testing = Some(Testing { server_name: "web".into(), cancel: CancellationToken::new() });
        let (reply, _rx) = oneshot::channel();
        st.offer_host_key("h".into(), 22, "ssh-ed25519".into(), "SHA256:x".into(), reply);
        assert!(st.host_prompt.is_some(), "while a test runs the question is shown");
    }

    #[test]
    fn changed_key_details_name_only_the_hop_that_changed() {
        let mut e = unduhkilat_core::AppError::new(ErrorCode::ErrHostKeyChanged, "berubah");
        assert!(changed_key_from(&e).is_none(), "no details, nothing to forget");
        e.details = Some(serde_json::json!({ "host": "bastion.example.com", "port": 2222, "key_type": "ssh-ed25519", "known": "SHA256:old", "presented": "SHA256:new" }));
        let c = changed_key_from(&e).expect("parsed");
        assert_eq!((c.host.as_str(), c.port, c.known.as_str(), c.presented.as_str()), ("bastion.example.com", 2222, "SHA256:old", "SHA256:new"));
    }

    #[test]
    fn dialog_build_checks_port_hops_and_keeps_only_the_matching_secret() {
        let mut d = ServerDialog::new();
        d.name = "web".into();
        d.host = "h".into();
        d.username = "u".into();
        d.port = "abc".into();
        assert!(d.build().unwrap_err().contains("Port"), "a non-numeric port is explained");
        d.port = "70000".into();
        assert!(d.build().is_err(), "a port above 65535 is refused");
        d.port = String::new();
        d.hops = vec![None];
        assert!(d.build().unwrap_err().contains("lompatan"), "an empty hop row is explained");
        d.hops = vec![Some("hop-id".into())];
        d.password = "pw".into();
        d.passphrase = "phrase".into();
        let (new, secrets) = d.build().unwrap();
        assert_eq!((new.port, new.hops.clone()), (None, vec!["hop-id".to_string()]), "empty port = default; hops kept for SSH");
        assert!(secrets.password.is_some() && secrets.passphrase.is_none(), "password auth sends only the password");
        d.auth = AuthMethod::Key;
        d.key_path = "k".into();
        let (new, secrets) = d.build().unwrap();
        assert_eq!(new.key_path.as_deref(), Some("k"));
        assert!(secrets.password.is_none() && secrets.passphrase.is_some(), "key auth sends only the passphrase");
        d.protocol = Protocol::Ftp;
        d.auth = AuthMethod::Password;
        let (new, _) = d.build().unwrap();
        assert!(new.hops.is_empty(), "FTP never carries a jump route");
    }
}
