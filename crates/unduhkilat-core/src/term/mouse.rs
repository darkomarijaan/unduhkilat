//! Mouse reports for programs that ask for them (xterm modes 1000/1002/1003, SGR 1006 or legacy X10).

use crate::term::engine::{MouseMode, TermModes};
use crate::term::keys::Mods;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Press(MouseButton),
    Release(MouseButton),
    Move { held: Option<MouseButton> },
    WheelUp,
    WheelDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseKind,
    /// Zero-based cell column and row.
    pub col: u16,
    pub row: u16,
    pub mods: Mods,
}

fn button_code(b: MouseButton) -> u8 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

/// The report for a mouse event, or `None` when the program's mouse mode does not ask for it.
pub fn encode_mouse(ev: &MouseEvent, modes: &TermModes) -> Option<Vec<u8>> {
    if modes.mouse == MouseMode::Off {
        return None;
    }
    let (mut code, release) = match ev.kind {
        MouseKind::Press(b) => (button_code(b), false),
        MouseKind::Release(b) => (if modes.sgr_mouse { button_code(b) } else { 3 }, true),
        MouseKind::WheelUp => (64, false),
        MouseKind::WheelDown => (65, false),
        MouseKind::Move { held } => {
            match (modes.mouse, held) {
                (MouseMode::Motion, Some(b)) => (button_code(b) + 32, false),
                (MouseMode::Motion, None) => (3 + 32, false),
                (MouseMode::Drag, Some(b)) => (button_code(b) + 32, false),
                _ => return None, // Click mode and drag-without-button do not report motion
            }
        }
    };
    if ev.mods.shift {
        code += 4;
    }
    if ev.mods.alt {
        code += 8;
    }
    if ev.mods.ctrl {
        code += 16;
    }
    let (x, y) = (ev.col as u32 + 1, ev.row as u32 + 1);
    if modes.sgr_mouse {
        Some(format!("\x1b[<{code};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes())
    } else {
        // X10 encoding: one byte per value, so coordinates beyond 223 cannot be expressed.
        if x > 223 || y > 223 {
            return None;
        }
        Some(vec![0x1b, b'[', b'M', 32 + code, 32 + x as u8, 32 + y as u8])
    }
}

/// On the alternate screen, programs that did not take the mouse still expect the wheel to move
/// through their content: send arrow keys (xterm "alternate scroll").
pub fn wheel_as_arrows(up: bool, lines: u32, modes: &TermModes) -> Option<Vec<u8>> {
    if !modes.alt_screen || !modes.alternate_scroll || modes.mouse != MouseMode::Off {
        return None;
    }
    let letter = if up { 'A' } else { 'B' };
    let one = if modes.app_cursor { format!("\x1bO{letter}") } else { format!("\x1b[{letter}") };
    Some(one.repeat(lines as usize).into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::engine::{MouseMode, TermModes};
    use crate::term::keys::Mods;

    fn modes(mouse: MouseMode, sgr: bool) -> TermModes {
        TermModes { alt_screen: false, bracketed_paste: false, app_cursor: false, app_keypad: false, mouse, sgr_mouse: sgr, focus_events: false, kitty_disambiguate: false, alternate_scroll: false }
    }
    const NONE: Mods = Mods { ctrl: false, alt: false, shift: false };
    fn ev(kind: MouseKind, col: u16, row: u16, mods: Mods) -> MouseEvent {
        MouseEvent { kind, col, row, mods }
    }

    #[test]
    fn sgr_press_release() {
        let m = modes(MouseMode::Click, true);
        assert_eq!(encode_mouse(&ev(MouseKind::Press(MouseButton::Left), 10, 5, NONE), &m).unwrap(), b"\x1b[<0;11;6M");
        assert_eq!(encode_mouse(&ev(MouseKind::Release(MouseButton::Left), 10, 5, NONE), &m).unwrap(), b"\x1b[<0;11;6m");
        assert_eq!(encode_mouse(&ev(MouseKind::Press(MouseButton::Right), 0, 0, NONE), &m).unwrap(), b"\x1b[<2;1;1M");
        assert_eq!(encode_mouse(&ev(MouseKind::Press(MouseButton::Middle), 0, 0, NONE), &m).unwrap(), b"\x1b[<1;1;1M");
    }

    #[test]
    fn wheel_buttons() {
        let m = modes(MouseMode::Click, true);
        assert_eq!(encode_mouse(&ev(MouseKind::WheelUp, 0, 0, NONE), &m).unwrap(), b"\x1b[<64;1;1M");
        assert_eq!(encode_mouse(&ev(MouseKind::WheelDown, 0, 0, NONE), &m).unwrap(), b"\x1b[<65;1;1M");
    }

    #[test]
    fn modifiers_add_bits() {
        let m = modes(MouseMode::Click, true);
        let p = |mods| encode_mouse(&ev(MouseKind::Press(MouseButton::Left), 0, 0, mods), &m).unwrap();
        assert_eq!(p(Mods { shift: true, ..NONE }), b"\x1b[<4;1;1M");
        assert_eq!(p(Mods { alt: true, ..NONE }), b"\x1b[<8;1;1M");
        assert_eq!(p(Mods { ctrl: true, ..NONE }), b"\x1b[<16;1;1M");
        assert_eq!(p(Mods { ctrl: true, alt: true, shift: true }), b"\x1b[<28;1;1M");
    }

    #[test]
    fn motion_is_reported_only_in_drag_or_motion_mode() {
        let held = MouseKind::Move { held: Some(MouseButton::Left) };
        let free = MouseKind::Move { held: None };
        assert_eq!(encode_mouse(&ev(held, 0, 0, NONE), &modes(MouseMode::Click, true)), None);
        assert_eq!(encode_mouse(&ev(held, 0, 0, NONE), &modes(MouseMode::Drag, true)).unwrap(), b"\x1b[<32;1;1M");
        assert_eq!(encode_mouse(&ev(free, 0, 0, NONE), &modes(MouseMode::Drag, true)), None, "drag mode ignores a mouse with no button down");
        assert_eq!(encode_mouse(&ev(free, 0, 0, NONE), &modes(MouseMode::Motion, true)).unwrap(), b"\x1b[<35;1;1M");
        assert_eq!(encode_mouse(&ev(held, 0, 0, NONE), &modes(MouseMode::Motion, true)).unwrap(), b"\x1b[<32;1;1M");
    }

    #[test]
    fn off_mode_reports_nothing() {
        let m = modes(MouseMode::Off, true);
        assert_eq!(encode_mouse(&ev(MouseKind::Press(MouseButton::Left), 0, 0, NONE), &m), None);
        assert_eq!(encode_mouse(&ev(MouseKind::WheelUp, 0, 0, NONE), &m), None);
    }

    #[test]
    fn legacy_encoding_when_sgr_is_off() {
        let m = modes(MouseMode::Click, false);
        assert_eq!(encode_mouse(&ev(MouseKind::Press(MouseButton::Left), 0, 0, NONE), &m).unwrap(), [0x1b, b'[', b'M', 32, 33, 33]);
        assert_eq!(encode_mouse(&ev(MouseKind::Release(MouseButton::Left), 0, 0, NONE), &m).unwrap(), [0x1b, b'[', b'M', 35, 33, 33], "legacy release is button 3");
        assert_eq!(encode_mouse(&ev(MouseKind::Press(MouseButton::Left), 222, 0, NONE), &m).unwrap().len(), 6);
        assert_eq!(encode_mouse(&ev(MouseKind::Press(MouseButton::Left), 223, 0, NONE), &m), None, "legacy cannot address column 224 or beyond");
        assert_eq!(encode_mouse(&ev(MouseKind::Press(MouseButton::Left), 0, 223, NONE), &m), None);
    }

    #[test]
    fn alternate_scroll_arrows() {
        let alt = TermModes { alt_screen: true, alternate_scroll: true, ..modes(MouseMode::Off, false) };
        assert_eq!(wheel_as_arrows(true, 3, &alt).unwrap(), b"\x1b[A\x1b[A\x1b[A");
        assert_eq!(wheel_as_arrows(false, 2, &alt).unwrap(), b"\x1b[B\x1b[B");
        let app = TermModes { app_cursor: true, ..alt };
        assert_eq!(wheel_as_arrows(true, 1, &app).unwrap(), b"\x1bOA");
        assert_eq!(wheel_as_arrows(true, 1, &TermModes { alt_screen: false, ..alt }), None, "only on the alternate screen");
        assert_eq!(wheel_as_arrows(true, 1, &TermModes { mouse: MouseMode::Click, ..alt }), None, "a program that takes the mouse gets wheel reports instead");
        assert_eq!(wheel_as_arrows(true, 1, &TermModes { alternate_scroll: false, ..alt }), None);
    }
}
