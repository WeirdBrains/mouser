//! Host → application encodings that depend on terminal modes: mouse reports,
//! focus events, bracketed paste and DECCKM cursor keys.

use crate::parser::{Modes, MouseMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    /// Motion with no button held.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Press,
    Release,
    Motion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseKind,
    pub button: MouseButton,
    /// 0-based cell.
    pub col: usize,
    pub row: usize,
    /// Pixel position inside the terminal area (for 1016).
    pub px: u32,
    pub py: u32,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// Bytes to send for `ev` under `modes`, or `None` when the app hasn't asked for it.
pub fn encode_mouse(ev: &MouseEvent, modes: &Modes) -> Option<Vec<u8>> {
    let wanted = match (modes.mouse, ev.kind) {
        (MouseMode::Off, _) => false,
        (_, MouseKind::Press | MouseKind::Release) => true,
        (MouseMode::Press, MouseKind::Motion) => false,
        (MouseMode::Drag, MouseKind::Motion) => ev.button != MouseButton::None,
        (MouseMode::Any, MouseKind::Motion) => true,
    };
    if !wanted {
        return None;
    }
    let mut cb: u32 = match ev.button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::None => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
    };
    if ev.kind == MouseKind::Release && matches!(ev.button, MouseButton::WheelUp | MouseButton::WheelDown) {
        return None;
    }
    if ev.shift {
        cb += 4;
    }
    if ev.alt {
        cb += 8;
    }
    if ev.ctrl {
        cb += 16;
    }
    if ev.kind == MouseKind::Motion {
        cb += 32;
    }
    let sgr = modes.mouse_sgr || modes.mouse_pixels;
    if sgr {
        let (x, y) = if modes.mouse_pixels { (ev.px as usize + 1, ev.py as usize + 1) } else { (ev.col + 1, ev.row + 1) };
        let end = if ev.kind == MouseKind::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{cb};{x};{y}{end}").into_bytes());
    }
    // Legacy X10 encoding: release is button 3, coordinates capped at 223.
    if ev.kind == MouseKind::Release {
        cb = (cb & !3) | 3;
    }
    let c = |v: usize| (v.min(222) + 33) as u8;
    Some(vec![0x1b, b'[', b'M', (cb + 32) as u8, c(ev.col), c(ev.row)])
}

pub fn encode_focus(focused: bool, modes: &Modes) -> Option<&'static [u8]> {
    modes.focus_events.then_some(if focused { b"\x1b[I" } else { b"\x1b[O" })
}

/// Wrap pasted text in 200~/201~ when bracketed paste is on. Embedded end
/// markers are stripped so pasted text can't break out of the bracket, and
/// control characters other than tab, newline and carriage return (C0, DEL,
/// C1) are dropped, as kitty and foot do, so a paste can't send escape
/// sequences or signals (Ctrl+C) to the program.
pub fn paste_bytes(text: &str, modes: &Modes) -> Vec<u8> {
    let clean: String = text.replace("\x1b[201~", "").chars().filter(|&c| matches!(c, '\t' | '\n' | '\r') || !(c.is_control())).collect();
    if modes.bracketed_paste {
        format!("\x1b[200~{clean}\x1b[201~").into_bytes()
    } else {
        clean.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// DECCKM: unmodified `CSI A..D/H/F` become `SS3 A..D/H/F`.
pub fn apply_cursor_mode(bytes: Vec<u8>, modes: &Modes) -> Vec<u8> {
    if modes.app_cursor && bytes.len() == 3 && bytes[0] == 0x1b && bytes[1] == b'[' && b"ABCDHF".contains(&bytes[2]) {
        return vec![0x1b, b'O', bytes[2]];
    }
    bytes
}
