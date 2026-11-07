//! Keyboard input to the bytes a terminal program expects (xterm, plus the kitty "disambiguate" level).

use crate::term::engine::TermModes;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A character the key produces (after shift/layout), e.g. `'a'`, `'A'`, `'é'`, `'你'`.
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Mods {
    fn any(&self) -> bool {
        self.ctrl || self.alt || self.shift
    }
    /// xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl.
    fn param(&self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8
    }
}

fn csi_u(code: u32, mods: Mods) -> Vec<u8> {
    if mods.any() {
        format!("\x1b[{code};{}u", mods.param()).into_bytes()
    } else {
        format!("\x1b[{code}u").into_bytes()
    }
}

/// `CSI 1;m <letter>` when modified, otherwise the classic form (`SS3` in application-cursor mode).
fn letter_key(letter: char, mods: Mods, app_cursor: bool) -> Vec<u8> {
    if mods.any() {
        format!("\x1b[1;{}{letter}", mods.param()).into_bytes()
    } else if app_cursor {
        format!("\x1bO{letter}").into_bytes()
    } else {
        format!("\x1b[{letter}").into_bytes()
    }
}

fn tilde_key(code: u8, mods: Mods) -> Vec<u8> {
    if mods.any() {
        format!("\x1b[{code};{}~", mods.param()).into_bytes()
    } else {
        format!("\x1b[{code}~").into_bytes()
    }
}

fn with_alt(mods: Mods, mut bytes: Vec<u8>) -> Vec<u8> {
    if mods.alt {
        bytes.insert(0, 0x1b);
    }
    bytes
}

/// The bytes to send to the program for one key press.
pub fn encode_key(key: Key, mods: Mods, modes: &TermModes) -> Vec<u8> {
    if modes.kitty_disambiguate {
        match key {
            Key::Escape => return csi_u(27, mods),
            Key::Enter if mods.any() => return csi_u(13, mods),
            Key::Tab if mods.any() && !(mods.shift && !mods.ctrl && !mods.alt) => return csi_u(9, mods),
            Key::Backspace if mods.any() => return csi_u(127, mods),
            Key::Char(c) if mods.ctrl || mods.alt => {
                let base = c.to_lowercase().next().unwrap_or(c);
                return csi_u(base as u32, mods);
            }
            _ => {}
        }
    }
    match key {
        Key::Char(c) => {
            let mut buf = [0u8; 4];
            let text = c.encode_utf8(&mut buf).as_bytes().to_vec();
            if mods.ctrl {
                let control = match c {
                    'a'..='z' => Some(c as u8 - b'a' + 1),
                    'A'..='Z' => Some(c as u8 - b'A' + 1),
                    '[' => Some(0x1b),
                    '\\' => Some(0x1c),
                    ']' => Some(0x1d),
                    '^' => Some(0x1e),
                    '_' => Some(0x1f),
                    ' ' | '@' => Some(0x00),
                    '?' => Some(0x7f),
                    _ => None,
                };
                if let Some(b) = control {
                    return with_alt(mods, vec![b]);
                }
            }
            with_alt(mods, text)
        }
        Key::Enter => with_alt(mods, b"\r".to_vec()),
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => with_alt(mods, b"\t".to_vec()),
        Key::Backspace if mods.ctrl => with_alt(mods, vec![0x08]),
        Key::Backspace => with_alt(mods, vec![0x7f]),
        Key::Escape => with_alt(mods, vec![0x1b]),
        Key::Up => letter_key('A', mods, modes.app_cursor),
        Key::Down => letter_key('B', mods, modes.app_cursor),
        Key::Right => letter_key('C', mods, modes.app_cursor),
        Key::Left => letter_key('D', mods, modes.app_cursor),
        Key::Home => letter_key('H', mods, modes.app_cursor),
        Key::End => letter_key('F', mods, modes.app_cursor),
        Key::PageUp => tilde_key(5, mods),
        Key::PageDown => tilde_key(6, mods),
        Key::Insert => tilde_key(2, mods),
        Key::Delete => tilde_key(3, mods),
        Key::F(n @ 1..=4) => {
            let letter = ['P', 'Q', 'R', 'S'][n as usize - 1];
            if mods.any() {
                format!("\x1b[1;{}{letter}", mods.param()).into_bytes()
            } else {
                format!("\x1bO{letter}").into_bytes()
            }
        }
        Key::F(n) => match n {
            5 => tilde_key(15, mods),
            6 => tilde_key(17, mods),
            7 => tilde_key(18, mods),
            8 => tilde_key(19, mods),
            9 => tilde_key(20, mods),
            10 => tilde_key(21, mods),
            11 => tilde_key(23, mods),
            12 => tilde_key(24, mods),
            _ => Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::engine::{MouseMode, TermModes};

    fn modes() -> TermModes {
        TermModes { alt_screen: false, bracketed_paste: false, app_cursor: false, app_keypad: false, mouse: MouseMode::Off, sgr_mouse: false, focus_events: false, kitty_disambiguate: false, alternate_scroll: false }
    }
    fn kitty() -> TermModes {
        TermModes { kitty_disambiguate: true, ..modes() }
    }
    fn app_cursor() -> TermModes {
        TermModes { app_cursor: true, ..modes() }
    }
    const NONE: Mods = Mods { ctrl: false, alt: false, shift: false };
    const CTRL: Mods = Mods { ctrl: true, alt: false, shift: false };
    const ALT: Mods = Mods { ctrl: false, alt: true, shift: false };
    const SHIFT: Mods = Mods { ctrl: false, alt: false, shift: true };

    fn enc(k: Key, m: Mods) -> Vec<u8> {
        encode_key(k, m, &modes())
    }

    #[test]
    fn plain_keys() {
        assert_eq!(enc(Key::Enter, NONE), b"\r");
        assert_eq!(enc(Key::Tab, NONE), b"\t");
        assert_eq!(enc(Key::Tab, SHIFT), b"\x1b[Z");
        assert_eq!(enc(Key::Backspace, NONE), b"\x7f");
        assert_eq!(enc(Key::Escape, NONE), b"\x1b");
        assert_eq!(enc(Key::Char('a'), NONE), b"a");
    }

    #[test]
    fn ctrl_letters() {
        assert_eq!(enc(Key::Char('c'), CTRL), [0x03]);
        assert_eq!(enc(Key::Char('a'), CTRL), [0x01]);
        assert_eq!(enc(Key::Char('Z'), CTRL), [0x1a]);
        assert_eq!(enc(Key::Char('['), CTRL), [0x1b]);
        assert_eq!(enc(Key::Char(' '), CTRL), [0x00]);
        assert_eq!(enc(Key::Backspace, CTRL), [0x08]);
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(enc(Key::Char('x'), ALT), b"\x1bx");
        assert_eq!(enc(Key::Enter, ALT), b"\x1b\r");
        assert_eq!(enc(Key::Backspace, ALT), b"\x1b\x7f");
    }

    #[test]
    fn arrows_normal_and_application() {
        assert_eq!(enc(Key::Up, NONE), b"\x1b[A");
        assert_eq!(encode_key(Key::Up, NONE, &app_cursor()), b"\x1bOA");
        assert_eq!(enc(Key::Left, CTRL), b"\x1b[1;5D");
        assert_eq!(enc(Key::Up, SHIFT), b"\x1b[1;2A");
        assert_eq!(enc(Key::Right, ALT), b"\x1b[1;3C");
        assert_eq!(encode_key(Key::Left, CTRL, &app_cursor()), b"\x1b[1;5D", "modifiers always use the CSI form");
    }

    #[test]
    fn editing_keys() {
        assert_eq!(enc(Key::Home, NONE), b"\x1b[H");
        assert_eq!(enc(Key::End, NONE), b"\x1b[F");
        assert_eq!(enc(Key::PageUp, NONE), b"\x1b[5~");
        assert_eq!(enc(Key::PageDown, NONE), b"\x1b[6~");
        assert_eq!(enc(Key::Delete, NONE), b"\x1b[3~");
        assert_eq!(enc(Key::Insert, NONE), b"\x1b[2~");
        assert_eq!(enc(Key::Delete, CTRL), b"\x1b[3;5~");
    }

    #[test]
    fn function_keys() {
        assert_eq!(enc(Key::F(1), NONE), b"\x1bOP");
        assert_eq!(enc(Key::F(4), NONE), b"\x1bOS");
        assert_eq!(enc(Key::F(5), NONE), b"\x1b[15~");
        assert_eq!(enc(Key::F(12), NONE), b"\x1b[24~");
        assert_eq!(enc(Key::F(5), SHIFT), b"\x1b[15;2~");
        assert_eq!(enc(Key::F(1), CTRL), b"\x1b[1;5P");
        assert!(enc(Key::F(40), NONE).is_empty(), "unknown function keys send nothing");
    }

    #[test]
    fn kitty_disambiguates() {
        let k = |key, m| encode_key(key, m, &kitty());
        assert_eq!(k(Key::Enter, SHIFT), b"\x1b[13;2u");
        assert_eq!(k(Key::Escape, NONE), b"\x1b[27u");
        assert_eq!(k(Key::Char('c'), CTRL), b"\x1b[99;5u");
        assert_eq!(k(Key::Char('x'), ALT), b"\x1b[120;3u");
        assert_eq!(k(Key::Enter, NONE), b"\r", "an unmodified Enter stays classic");
        assert_eq!(k(Key::Char('a'), NONE), b"a", "plain text stays text");
        assert_eq!(k(Key::Char('A'), SHIFT), b"A", "shifted text stays text");
        assert_eq!(k(Key::Up, CTRL), b"\x1b[1;5A", "arrows keep their classic form");
        assert_eq!(enc(Key::Enter, SHIFT), b"\r", "without kitty, Shift+Enter is a plain Enter");
    }

    #[test]
    fn unicode_text() {
        assert_eq!(enc(Key::Char('\u{e9}'), NONE), "\u{e9}".as_bytes());
        assert_eq!(enc(Key::Char('\u{4F60}'), NONE), "\u{4F60}".as_bytes());
        assert_eq!(enc(Key::Char('\u{1F600}'), NONE), "\u{1F600}".as_bytes());
        assert_eq!(enc(Key::Char('\u{e9}'), ALT), "\x1b\u{e9}".as_bytes());
        assert_eq!(enc(Key::Char('\u{e9}'), CTRL), "\u{e9}".as_bytes(), "Ctrl on a non-ASCII letter is just the letter");
    }
}
