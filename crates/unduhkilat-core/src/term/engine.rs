//! Terminal emulator wrapper (library choice: docs/superpowers/spikes/2026-11-07-terminal-egui.md).

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape as AlaShape, NamedColor, Processor, Rgb as AlaRgb};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Default colours (the dark look of the mockups).
pub const DEFAULT_FG: Rgb = Rgb(0xE8, 0xE3, 0xDC);
pub const DEFAULT_BG: Rgb = Rgb(0x14, 0x12, 0x0F);

/// Standard xterm colours 0-15.
const ANSI16: [Rgb; 16] = [
    Rgb(0, 0, 0), Rgb(205, 0, 0), Rgb(0, 205, 0), Rgb(205, 205, 0), Rgb(0, 0, 238), Rgb(205, 0, 205), Rgb(0, 205, 205), Rgb(229, 229, 229),
    Rgb(127, 127, 127), Rgb(255, 0, 0), Rgb(0, 255, 0), Rgb(255, 255, 0), Rgb(92, 92, 255), Rgb(255, 0, 255), Rgb(0, 255, 255), Rgb(255, 255, 255),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellView {
    pub ch: char,
    pub fg: Rgb,
    pub bg: Rgb,
    /// The background is the terminal default: no need to paint it.
    pub bg_default: bool,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub dim: bool,
    /// First half of a double-width character.
    pub wide: bool,
    /// Second half of a double-width character (draws nothing).
    pub spacer: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    /// OSC 52: the program wants this text in the clipboard.
    ClipboardStore(String),
    /// The emulator's answer to a query (device attributes, colours, size): send it to the program.
    PtyWrite(Vec<u8>),
    Title(Option<String>),
    Bell,
    /// The shell reported its current folder (OSC 9;9 or OSC 7), as a Windows path.
    Cwd(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseMode {
    Off,
    Click,
    Drag,
    Motion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermModes {
    pub alt_screen: bool,
    pub bracketed_paste: bool,
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub mouse: MouseMode,
    pub sgr_mouse: bool,
    pub focus_events: bool,
    pub kitty_disambiguate: bool,
    pub alternate_scroll: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectKind {
    Simple,
    Word,
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorView {
    pub row: u16,
    pub col: u16,
    pub shape: CursorShape,
}

struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Clone)]
struct Collector {
    events: Arc<Mutex<Vec<TermEvent>>>,
    size: Arc<Mutex<(u16, u16)>>,
}

impl EventListener for Collector {
    fn send_event(&self, event: Event) {
        let ev = match event {
            Event::ClipboardStore(_, text) => TermEvent::ClipboardStore(text),
            Event::PtyWrite(text) => TermEvent::PtyWrite(text.into_bytes()),
            Event::Title(t) => TermEvent::Title(Some(t)),
            Event::ResetTitle => TermEvent::Title(None),
            Event::Bell => TermEvent::Bell,
            Event::ColorRequest(index, format) => {
                let c = match index {
                    256 | 258 => DEFAULT_FG,
                    257 => DEFAULT_BG,
                    i if i < 256 => indexed(i as u8),
                    _ => DEFAULT_FG,
                };
                TermEvent::PtyWrite(format(AlaRgb { r: c.0, g: c.1, b: c.2 }).into_bytes())
            }
            Event::TextAreaSizeRequest(format) => {
                let (cols, rows) = *self.size.lock().unwrap();
                TermEvent::PtyWrite(format(alacritty_terminal::event::WindowSize { num_lines: rows, num_cols: cols, cell_width: 8, cell_height: 16 }).into_bytes())
            }
            // Never let a program read the clipboard.
            _ => return,
        };
        self.events.lock().unwrap().push(ev);
    }
}

fn indexed(i: u8) -> Rgb {
    match i {
        0..=15 => ANSI16[i as usize],
        16..=231 => {
            let n = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            Rgb(level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        _ => {
            let g = 8 + 10 * (i - 232);
            Rgb(g, g, g)
        }
    }
}

fn dimmed(c: Rgb) -> Rgb {
    Rgb((c.0 as u16 * 2 / 3) as u8, (c.1 as u16 * 2 / 3) as u8, (c.2 as u16 * 2 / 3) as u8)
}

fn resolve(c: Color) -> Rgb {
    match c {
        Color::Spec(r) => Rgb(r.r, r.g, r.b),
        Color::Indexed(i) => indexed(i),
        Color::Named(n) => match n {
            NamedColor::Foreground | NamedColor::Cursor | NamedColor::BrightForeground => DEFAULT_FG,
            NamedColor::DimForeground => dimmed(DEFAULT_FG),
            NamedColor::Background => DEFAULT_BG,
            other => {
                let v = other as usize;
                if v < 16 {
                    ANSI16[v]
                } else {
                    // DimBlack..DimWhite (259..=266)
                    dimmed(ANSI16[(v - 259).min(7)])
                }
            }
        },
    }
}

pub struct TermEngine {
    term: Term<Collector>,
    parser: Processor,
    events: Arc<Mutex<Vec<TermEvent>>>,
    size: Arc<Mutex<(u16, u16)>>,
    cwd_scan: CwdScan,
}

/// Longest OSC payload kept while looking for a folder report; longer ones are ignored.
const OSC_MAX: usize = 4096;

/// Finds folder reports (OSC 9;9 and OSC 7) in the byte stream. The emulator ignores both, so they
/// are picked out here; a report may be split across `feed` calls.
#[derive(Default)]
struct CwdScan {
    state: ScanState,
    buf: Vec<u8>,
    too_long: bool,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum ScanState {
    #[default]
    Ground,
    Esc,
    Osc,
    OscEsc,
}

impl CwdScan {
    fn feed(&mut self, bytes: &[u8], out: &mut Vec<TermEvent>) {
        for &b in bytes {
            match self.state {
                ScanState::Ground => {
                    if b == 0x1b {
                        self.state = ScanState::Esc;
                    }
                }
                ScanState::Esc => self.after_esc(b),
                ScanState::Osc => match b {
                    0x07 => self.finish(out),
                    0x1b => self.state = ScanState::OscEsc,
                    // CAN and SUB cancel the sequence.
                    0x18 | 0x1a => self.state = ScanState::Ground,
                    _ => self.push(b),
                },
                ScanState::OscEsc => {
                    if b == b'\\' {
                        self.finish(out);
                    } else {
                        // ESC that is not ST: the OSC was cut off and this ESC starts something new.
                        self.after_esc(b);
                    }
                }
            }
        }
    }

    fn after_esc(&mut self, b: u8) {
        self.state = match b {
            b']' => {
                self.buf.clear();
                self.too_long = false;
                ScanState::Osc
            }
            0x1b => ScanState::Esc,
            _ => ScanState::Ground,
        };
    }

    fn push(&mut self, b: u8) {
        if self.buf.len() < OSC_MAX {
            self.buf.push(b);
        } else {
            self.too_long = true;
        }
    }

    fn finish(&mut self, out: &mut Vec<TermEvent>) {
        self.state = ScanState::Ground;
        if self.too_long {
            return;
        }
        if let Some(dir) = std::str::from_utf8(&self.buf).ok().and_then(cwd_from_osc) {
            out.push(TermEvent::Cwd(dir));
        }
    }
}

/// The Windows folder in an OSC payload (without `ESC ]` and the terminator), if it is a folder report.
pub fn cwd_from_osc(payload: &str) -> Option<String> {
    let dir = if let Some(rest) = payload.strip_prefix("9;9;") {
        let rest = rest.trim();
        rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')).unwrap_or(rest).to_string()
    } else {
        // OSC 7: file://host/C:/path, percent-encoded.
        let rest = payload.strip_prefix("7;")?.trim().strip_prefix("file://")?;
        let path = percent_decode(&rest[rest.find('/')?..])?;
        let path = path.strip_prefix('/').filter(|p| is_drive_path(p)).map(str::to_string).unwrap_or(path);
        if !is_drive_path(&path) {
            return None; // a Linux path (WSL): not a folder on this computer
        }
        path.replace('/', "\\")
    };
    let ok = (is_drive_path(&dir) || dir.starts_with(r"\\")) && !dir.chars().any(char::is_control);
    ok.then_some(dir)
}

fn is_drive_path(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

impl TermEngine {
    pub fn new(cols: u16, rows: u16, scrollback: usize) -> TermEngine {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let events = Arc::new(Mutex::new(Vec::new()));
        let size = Arc::new(Mutex::new((cols, rows)));
        let mut cfg = Config::default();
        cfg.scrolling_history = scrollback;
        cfg.kitty_keyboard = true;
        let term = Term::new(cfg, &Size { cols: cols as usize, rows: rows as usize }, Collector { events: events.clone(), size: size.clone() });
        TermEngine { term, parser: Processor::default(), events, size, cwd_scan: CwdScan::default() }
    }

    /// Feed bytes from the program. Returns what the emulator wants done about them.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<TermEvent> {
        self.parser.advance(&mut self.term, bytes);
        let mut out = std::mem::take(&mut *self.events.lock().unwrap());
        self.cwd_scan.feed(bytes, &mut out);
        out
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        self.term.resize(Size { cols: cols as usize, rows: rows as usize });
        *self.size.lock().unwrap() = (cols, rows);
    }

    pub fn size(&self) -> (u16, u16) {
        *self.size.lock().unwrap()
    }

    pub fn modes(&self) -> TermModes {
        let m = *self.term.mode();
        TermModes {
            alt_screen: m.contains(TermMode::ALT_SCREEN),
            bracketed_paste: m.contains(TermMode::BRACKETED_PASTE),
            app_cursor: m.contains(TermMode::APP_CURSOR),
            app_keypad: m.contains(TermMode::APP_KEYPAD),
            mouse: if m.contains(TermMode::MOUSE_MOTION) {
                MouseMode::Motion
            } else if m.contains(TermMode::MOUSE_DRAG) {
                MouseMode::Drag
            } else if m.contains(TermMode::MOUSE_REPORT_CLICK) {
                MouseMode::Click
            } else {
                MouseMode::Off
            },
            sgr_mouse: m.contains(TermMode::SGR_MOUSE),
            focus_events: m.contains(TermMode::FOCUS_IN_OUT),
            kitty_disambiguate: m.contains(TermMode::DISAMBIGUATE_ESC_CODES),
            alternate_scroll: m.contains(TermMode::ALTERNATE_SCROLL),
        }
    }

    fn point(&self, row: u16, col: u16) -> Point {
        Point::new(Line(row as i32 - self.term.grid().display_offset() as i32), Column(col as usize))
    }

    /// One visible cell (the viewport, honouring scroll-back).
    pub fn cell(&self, row: u16, col: u16) -> CellView {
        let (cols, rows) = self.size();
        let blank = CellView { ch: ' ', fg: DEFAULT_FG, bg: DEFAULT_BG, bg_default: true, bold: false, italic: false, underline: false, strike: false, dim: false, wide: false, spacer: false };
        if row >= rows || col >= cols {
            return blank;
        }
        let cell = &self.term.grid()[self.point(row, col)];
        let f = cell.flags;
        let mut fg = resolve(cell.fg);
        let mut bg = resolve(cell.bg);
        let mut bg_default = matches!(cell.bg, Color::Named(NamedColor::Background));
        if f.contains(Flags::DIM) {
            fg = dimmed(fg);
        }
        if f.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
            bg_default = false;
        }
        CellView {
            ch: if f.contains(Flags::HIDDEN) { ' ' } else { cell.c },
            fg,
            bg,
            bg_default,
            bold: f.contains(Flags::BOLD),
            italic: f.contains(Flags::ITALIC),
            underline: f.intersects(Flags::ALL_UNDERLINES),
            strike: f.contains(Flags::STRIKEOUT),
            dim: f.contains(Flags::DIM),
            wide: f.contains(Flags::WIDE_CHAR),
            spacer: f.contains(Flags::WIDE_CHAR_SPACER),
        }
    }

    pub fn cursor(&self) -> Option<CursorView> {
        if self.term.grid().display_offset() != 0 || !self.term.mode().contains(TermMode::SHOW_CURSOR) {
            return None;
        }
        let p = self.term.grid().cursor.point;
        let shape = match self.term.cursor_style().shape {
            AlaShape::Underline => CursorShape::Underline,
            AlaShape::Beam => CursorShape::Bar,
            _ => CursorShape::Block,
        };
        Some(CursorView { row: p.line.0.max(0) as u16, col: p.column.0 as u16, shape })
    }

    /// Positive scrolls toward older output.
    pub fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    pub fn history_len(&self) -> usize {
        self.term.grid().total_lines().saturating_sub(self.term.grid().screen_lines())
    }

    pub fn select_start(&mut self, kind: SelectKind, row: u16, col: u16) {
        let ty = match kind {
            SelectKind::Simple => SelectionType::Simple,
            SelectKind::Word => SelectionType::Semantic,
            SelectKind::Line => SelectionType::Lines,
        };
        let p = self.point(row, col);
        self.term.selection = Some(Selection::new(ty, p, Side::Left));
    }

    pub fn select_update(&mut self, row: u16, col: u16) {
        let p = self.point(row, col);
        if let Some(sel) = self.term.selection.as_mut() {
            sel.update(p, Side::Right);
        }
    }

    pub fn select_clear(&mut self) {
        self.term.selection = None;
    }

    /// Select everything currently visible.
    pub fn select_all(&mut self) {
        let (cols, rows) = self.size();
        let start = self.point(0, 0);
        let end = self.point(rows - 1, cols - 1);
        let mut sel = Selection::new(SelectionType::Simple, start, Side::Left);
        sel.update(end, Side::Right);
        self.term.selection = Some(sel);
    }

    pub fn selected_text(&self) -> Option<String> {
        self.term.selection_to_string().filter(|s| !s.is_empty())
    }

    /// The selection as ((row, col), (row, col)) from first to last cell, in viewport rows (may be off
    /// screen when scrolled; clamp when drawing).
    pub fn selection_viewport(&self) -> Option<((i32, u16), (i32, u16))> {
        let range = self.term.selection.as_ref().and_then(|s| s.to_range(&self.term))?;
        let off = self.term.grid().display_offset() as i32;
        Some(((range.start.line.0 + off, range.start.column.0 as u16), (range.end.line.0 + off, range.end.column.0 as u16)))
    }

    pub fn clear_screen_and_history(&mut self) {
        self.feed(b"\x1b[H\x1b[2J\x1b[3J");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_text(e: &TermEngine, row: u16) -> String {
        let (cols, _) = e.size();
        (0..cols).filter_map(|c| {
            let cell = e.cell(row, c);
            (!cell.spacer).then_some(cell.ch)
        }).collect::<String>().trim_end().to_string()
    }

    #[test]
    fn modes_follow_private_mode_sequences() {
        let mut e = TermEngine::new(80, 24, 1000);
        e.feed(b"\x1b[?1049h\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[?1004h\x1b[>1u");
        let m = e.modes();
        assert!(m.alt_screen && m.bracketed_paste && m.sgr_mouse && m.focus_events && m.kitty_disambiguate);
        assert_eq!(m.mouse, MouseMode::Click);
        e.feed(b"\x1b[?1002h");
        assert_eq!(e.modes().mouse, MouseMode::Drag);
        e.feed(b"\x1b[?1003h");
        assert_eq!(e.modes().mouse, MouseMode::Motion);
        e.feed(b"\x1b[?1049l\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l\x1b[<u");
        let m = e.modes();
        assert!(!m.alt_screen && !m.bracketed_paste && !m.sgr_mouse && !m.focus_events && !m.kitty_disambiguate);
        assert_eq!(m.mouse, MouseMode::Off);
    }

    #[test]
    fn colors_256_and_truecolor_resolve_to_rgb() {
        let mut e = TermEngine::new(80, 24, 100);
        e.feed(b"\x1b[38;5;208mA\x1b[0m\x1b[38;2;18;165;148mB\x1b[0m\x1b[31mC\x1b[0m\x1b[91mD");
        assert_eq!(e.cell(0, 0).fg, Rgb(255, 135, 0), "xterm 208 from the 6x6x6 cube");
        assert_eq!(e.cell(0, 1).fg, Rgb(18, 165, 148));
        assert_eq!(e.cell(0, 2).fg, Rgb(205, 0, 0), "ANSI red");
        assert_eq!(e.cell(0, 3).fg, Rgb(255, 0, 0), "bright red");
        assert!(e.cell(0, 0).bg_default);
        e.feed(b"\r\n\x1b[0m\x1b[38;2;10;20;30m\x1b[48;2;200;210;220m\x1b[7mX");
        let x = e.cell(1, 0);
        assert_eq!((x.fg, x.bg), (Rgb(200, 210, 220), Rgb(10, 20, 30)), "inverse swaps foreground and background");
        assert!(!x.bg_default);
    }

    #[test]
    fn text_attributes_are_reported() {
        let mut e = TermEngine::new(80, 24, 100);
        e.feed(b"\x1b[1mB\x1b[0m\x1b[3mI\x1b[0m\x1b[4mU\x1b[0m\x1b[9mS\x1b[0m\x1b[2mD");
        assert!(e.cell(0, 0).bold && e.cell(0, 1).italic && e.cell(0, 2).underline && e.cell(0, 3).strike && e.cell(0, 4).dim);
        assert!(!e.cell(0, 0).italic);
    }

    #[test]
    fn wide_chars_use_two_cells() {
        let mut e = TermEngine::new(80, 24, 100);
        e.feed("\u{4F60}\u{1F600}x".as_bytes());
        assert!(e.cell(0, 0).wide && e.cell(0, 0).ch == '\u{4F60}');
        assert!(e.cell(0, 1).spacer);
        assert!(e.cell(0, 2).wide && e.cell(0, 2).ch == '\u{1F600}');
        assert!(e.cell(0, 3).spacer);
        assert_eq!(e.cell(0, 4).ch, 'x');
    }

    #[test]
    fn osc52_becomes_a_clipboard_event() {
        let mut e = TermEngine::new(80, 24, 100);
        let events = e.feed(b"\x1b]52;c;SGVsbG8=\x07");
        assert!(events.contains(&TermEvent::ClipboardStore("Hello".into())), "{events:?}");
    }

    #[test]
    fn folder_reports_become_cwd_events() {
        let mut e = TermEngine::new(80, 24, 100);
        let ev = e.feed("\x1b]9;9;C:\\Users\\x y\x1b\\C:\\Users\\x y>".as_bytes());
        assert!(ev.contains(&TermEvent::Cwd(r"C:\Users\x y".into())), "{ev:?}");
        assert_eq!(row_text(&e, 0).trim_end(), r"C:\Users\x y>", "the report itself is not drawn");
        let ev = e.feed(b"\x1b]7;file://host/D:/Rust%20Projects\x07");
        assert!(ev.contains(&TermEvent::Cwd(r"D:\Rust Projects".into())), "OSC 7 ended by BEL: {ev:?}");
    }

    #[test]
    fn a_folder_report_split_across_feeds_is_found() {
        let mut e = TermEngine::new(80, 24, 100);
        let all = "\x1b]9;9;D:\\\u{fc}ber\x1b\\".as_bytes();
        let mut ev = Vec::new();
        for b in all {
            ev.extend(e.feed(std::slice::from_ref(b)));
        }
        assert_eq!(ev, vec![TermEvent::Cwd("D:\\\u{fc}ber".into())]);
    }

    #[test]
    fn other_or_broken_osc_is_not_a_folder() {
        assert_eq!(cwd_from_osc("0;title"), None);
        assert_eq!(cwd_from_osc("9;4;1;50"), None, "progress report");
        assert_eq!(cwd_from_osc(r"9;9;relative\path"), None);
        assert_eq!(cwd_from_osc("7;file://host/home/me"), None, "Linux path from WSL");
        assert_eq!(cwd_from_osc(r#"9;9;"C:\a b""#), Some(r"C:\a b".into()), "quotes are removed");
        assert_eq!(cwd_from_osc(r"9;9;\\srv\share"), Some(r"\\srv\share".into()), "UNC");
        let mut e = TermEngine::new(80, 24, 100);
        // Cut off by a new escape sequence, then an oversized one: no event, no panic.
        assert!(e.feed(b"\x1b]9;9;C:\\x\x1b[31m").is_empty());
        let mut big = b"\x1b]9;9;C:\\".to_vec();
        big.extend(std::iter::repeat_n(b'a', OSC_MAX + 10));
        big.push(0x07);
        assert!(e.feed(&big).iter().all(|x| !matches!(x, TermEvent::Cwd(_))));
    }

    #[test]
    fn device_query_gets_a_reply() {
        let mut e = TermEngine::new(80, 24, 100);
        let events = e.feed(b"\x1b[c");
        let reply = events.iter().find_map(|ev| match ev { TermEvent::PtyWrite(b) => Some(b.clone()), _ => None }).expect("a reply");
        assert!(reply.starts_with(b"\x1b[?"), "{reply:?}");
    }

    #[test]
    fn scrollback_is_capped() {
        let mut e = TermEngine::new(80, 24, 10_000);
        let mut lines = String::new();
        for i in 0..20_000 {
            lines.push_str(&format!("line {i}\r\n"));
        }
        e.feed(lines.as_bytes());
        assert_eq!(e.history_len(), 10_000);
    }

    #[test]
    fn scroll_and_display_offset() {
        let mut e = TermEngine::new(80, 5, 1000);
        let mut lines = String::new();
        for i in 0..100 {
            lines.push_str(&format!("L{i}\r\n"));
        }
        e.feed(lines.as_bytes());
        assert_eq!(e.display_offset(), 0);
        assert!(row_text(&e, 0).starts_with("L96"), "{}", row_text(&e, 0));
        e.scroll(10);
        assert_eq!(e.display_offset(), 10);
        assert!(row_text(&e, 0).starts_with("L86"), "{}", row_text(&e, 0));
        assert!(e.cursor().is_none(), "the cursor is hidden while scrolled back");
        e.scroll_to_bottom();
        assert_eq!(e.display_offset(), 0);
        assert!(e.cursor().is_some());
    }

    #[test]
    fn resize_reflows_and_never_panics() {
        let mut e = TermEngine::new(80, 24, 1000);
        e.feed(b"hello world\r\n");
        e.resize(100, 30);
        assert_eq!(e.size(), (100, 30));
        for (c, r) in [(0, 0), (1, 1), (500, 200), (2, 50), (80, 24)] {
            e.resize(c, r);
            e.feed(b"after resize\r\n");
        }
        for i in 0..50u16 {
            e.resize(10 + i, 5 + i % 7);
        }
        e.resize(0, 0);
        assert_eq!(e.size(), (1, 1), "sizes are clamped to at least 1x1");
        let _ = e.cell(0, 0);
    }

    #[test]
    fn selection_text_word_line_and_all() {
        let mut e = TermEngine::new(80, 24, 100);
        e.feed(b"halo dunia\r\nbaris dua");
        e.select_start(SelectKind::Word, 0, 6);
        assert_eq!(e.selected_text().as_deref(), Some("dunia"));
        e.select_clear();
        assert_eq!(e.selected_text(), None);
        e.select_start(SelectKind::Line, 0, 2);
        assert!(e.selected_text().unwrap().starts_with("halo dunia"));
        e.select_clear();
        e.select_start(SelectKind::Simple, 0, 0);
        e.select_update(0, 3);
        assert_eq!(e.selected_text().as_deref(), Some("halo"));
        e.select_all();
        let all = e.selected_text().unwrap();
        assert!(all.contains("halo dunia") && all.contains("baris dua"), "{all:?}");
    }

    #[test]
    fn selection_viewport_reports_the_selected_cells() {
        let mut e = TermEngine::new(80, 5, 100);
        e.feed(b"halo dunia
baris dua");
        assert_eq!(e.selection_viewport(), None);
        e.select_start(SelectKind::Simple, 0, 0);
        e.select_update(0, 3);
        assert_eq!(e.selection_viewport(), Some(((0, 0), (0, 3))));
        e.feed("
x".repeat(10).as_bytes());
        e.scroll(2);
        let ((r0, _), (r1, _)) = e.selection_viewport().expect("still selected while scrolled");
        assert!(r0 != 0 && r0 == r1, "rows are reported relative to the scrolled viewport: {r0}");
    }

    #[test]
    fn clear_screen_and_history_empties_everything() {
        let mut e = TermEngine::new(80, 5, 100);
        e.feed("x\r\n".repeat(50).as_bytes());
        assert!(e.history_len() > 0);
        e.clear_screen_and_history();
        assert_eq!(e.history_len(), 0);
        assert_eq!(row_text(&e, 0), "");
    }

    #[test]
    fn hostile_input_is_bounded() {
        let mut e = TermEngine::new(80, 24, 2000);
        let mut x: u64 = 0x2545F4914F6CDD1D;
        let noise: Vec<u8> = (0..1_000_000).map(|_| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; (x & 0xff) as u8 }).collect();
        e.feed(&noise);
        e.feed(&vec![b'x'; 1_000_000]); // one enormous line without a newline
        e.feed("\x1b[1;1H\x1b[2J".repeat(10_000).as_bytes());
        assert!(e.history_len() <= 2000);
        let _ = e.cell(0, 0);
    }
}
