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
}

/// Keyboard state that the egui events alone do not carry.
#[derive(Debug, Clone, Copy)]
pub struct KeyContext {
    pub mods: egui::Modifiers,
    /// The Insert key is down (distinguishes Ctrl+Insert from Ctrl+C, Shift+Insert from Ctrl+V).
    pub insert_down: bool,
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
}

impl TerminalView {
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
        let (events, mods, insert_down) = ui.input(|i| (i.events.clone(), i.modifiers, i.key_down(egui::Key::Insert)));
        let inputs = translate_events(&events, &snap.modes, snap.has_selection, ctrl_v_pastes, &KeyContext { mods, insert_down });
        if inputs.is_empty() {
            return None;
        }
        // These events are ours now: nothing else on screen may react to them.
        ui.input_mut(|i| {
            i.events.retain(|e| !matches!(e, egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Copy | egui::Event::Cut | egui::Event::Key { pressed: true, .. }));
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
        for ev in &events {
            match ev {
                egui::Event::PointerButton { pos, button: b, pressed, .. } => {
                    let Some(mb) = button(*b) else { continue };
                    let (r, c) = cell_at(*pos);
                    if *pressed && rect.contains(*pos) {
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
                    if self.reporting_drag.is_some() || (report && rect.contains(*pos)) {
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
        KeyContext { mods, insert_down: false }
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
        assert_eq!(translate_events(&[Event::Copy], &modes(), false, true, &KeyContext { mods: CTRL, insert_down: true }), vec![TermInput::Copy]);
    }

    #[test]
    fn paste_event_becomes_paste() {
        assert_eq!(tr(&[Event::Paste("hi".into())], CTRL), vec![TermInput::Paste("hi".into())]);
        assert_eq!(tr(&[Event::Paste("hi".into())], SHIFT), vec![TermInput::Paste("hi".into())], "Shift+Insert");
    }

    #[test]
    fn ctrl_v_can_be_given_to_the_program() {
        let off = |m, insert| translate_events(&[Event::Paste("x".into())], &modes(), false, false, &KeyContext { mods: m, insert_down: insert });
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
}
