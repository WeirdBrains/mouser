use serde::{Deserialize, Serialize};

/// Kitty keyboard protocol progressive-enhancement flags.
pub mod flags {
    pub const DISAMBIGUATE: u32 = 1;
    pub const REPORT_EVENTS: u32 = 2;
    pub const REPORT_ALTERNATES: u32 = 4;
    pub const REPORT_ALL_KEYS: u32 = 8;
    pub const REPORT_TEXT: u32 = 16;
    pub const ALL: u32 = 31;
}

/// Most entries kept per stack; pushing more drops the oldest (as kitty does).
const STACK_MAX: usize = 64;

/// Per-screen flag stacks (`CSI > u` push, `CSI < u` pop, `CSI = u` set,
/// `CSI ? u` query). The main and alternate screens keep separate stacks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeyboardProtocolState {
    pub main: Vec<u32>,
    pub alt: Vec<u32>,
}

impl KeyboardProtocolState {
    fn stack(&mut self, alt: bool) -> &mut Vec<u32> {
        if alt { &mut self.alt } else { &mut self.main }
    }

    /// Active flags on the main (`alt == false`) or alternate screen.
    pub fn current(&self, alt: bool) -> u32 {
        let s = if alt { &self.alt } else { &self.main };
        s.last().copied().unwrap_or(0)
    }

    pub fn push(&mut self, alt: bool, f: u32) {
        let s = self.stack(alt);
        if s.len() >= STACK_MAX {
            s.remove(0);
        }
        s.push(f & flags::ALL);
    }

    /// Pop `n` entries (at least one); popping everything resets to 0.
    pub fn pop(&mut self, alt: bool, n: usize) {
        let s = self.stack(alt);
        let keep = s.len().saturating_sub(n.max(1));
        s.truncate(keep);
    }

    /// `CSI = flags ; mode u`: 1 sets, 2 adds bits, 3 removes bits.
    pub fn set(&mut self, alt: bool, f: u32, mode: u32) {
        let cur = self.current(alt);
        let new = match mode {
            2 => cur | f,
            3 => cur & !f,
            _ => f,
        } & flags::ALL;
        let s = self.stack(alt);
        match s.last_mut() {
            Some(top) => *top = new,
            None => s.push(new),
        }
    }

    /// Reply to `CSI ? u`.
    pub fn query_response(&self, alt: bool) -> String {
        format!("\x1b[?{}u", self.current(alt))
    }
}

/// Modifier bits as in the protocol (the wire value is `1 + bits`).
pub mod mods {
    pub const SHIFT: u8 = 1;
    pub const ALT: u8 = 2;
    pub const CTRL: u8 = 4;
    pub const SUPER: u8 = 8;
    pub const HYPER: u8 = 16;
    pub const META: u8 = 32;
    pub const CAPS_LOCK: u8 = 64;
    pub const NUM_LOCK: u8 = 128;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventType {
    Press = 1,
    Repeat = 2,
    Release = 3,
}

/// Non-text keys, with the protocol's functional key codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Func {
    Escape,
    Enter,
    Tab,
    Backspace,
    Insert,
    Delete,
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    CapsLock,
    ScrollLock,
    NumLock,
    PrintScreen,
    Pause,
    Menu,
    /// F1..=F35
    F(u8),
    /// Keypad key by its kitty code (57399..=57427).
    Keypad(u32),
    /// Media keys by kitty code (57428..=57440).
    Media(u32),
    /// Modifier keys by kitty code (57441..=57454).
    Modifier(u32),
}

/// How a functional key is written in CSI form.
enum Form {
    /// `CSI 1;mods X` (or `CSI X` when unmodified).
    Letter(u8),
    /// `CSI n;mods ~`
    Tilde(u32),
    /// `CSI code;mods u`
    U(u32),
}

impl Func {
    fn form(self) -> Form {
        use Form::*;
        match self {
            Func::Escape => U(27),
            Func::Enter => U(13),
            Func::Tab => U(9),
            Func::Backspace => U(127),
            Func::Insert => Tilde(2),
            Func::Delete => Tilde(3),
            Func::Left => Letter(b'D'),
            Func::Right => Letter(b'C'),
            Func::Up => Letter(b'A'),
            Func::Down => Letter(b'B'),
            Func::PageUp => Tilde(5),
            Func::PageDown => Tilde(6),
            Func::Home => Letter(b'H'),
            Func::End => Letter(b'F'),
            Func::CapsLock => U(57358),
            Func::ScrollLock => U(57359),
            Func::NumLock => U(57360),
            Func::PrintScreen => U(57361),
            Func::Pause => U(57362),
            Func::Menu => U(57363),
            Func::F(1) => Letter(b'P'),
            Func::F(2) => Letter(b'Q'),
            Func::F(3) => Tilde(13),
            Func::F(4) => Letter(b'S'),
            Func::F(5) => Tilde(15),
            Func::F(n @ 6..=10) => Tilde(17 + (n as u32 - 6)),
            Func::F(11) => Tilde(23),
            Func::F(12) => Tilde(24),
            Func::F(n) => U(57376 + (n.clamp(13, 35) as u32 - 13)),
            Func::Keypad(c) | Func::Media(c) | Func::Modifier(c) => U(c),
        }
    }

    /// Modifier and lock keys: reported only under "report all keys".
    fn all_keys_only(self) -> bool {
        matches!(self, Func::CapsLock | Func::ScrollLock | Func::NumLock | Func::Modifier(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyCode {
    /// A text key, as its unshifted character.
    Char(char),
    Func(Func),
}

/// A key event in toolkit-neutral form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyInput {
    pub key: KeyCode,
    /// The shifted character, when shift is held and it differs.
    pub shifted: Option<char>,
    /// The key at this position on a US (PC-101) layout, if different.
    pub base: Option<char>,
    pub mods: u8,
    pub event: KeyEventType,
    /// Text the key produces, if any.
    pub text: Option<String>,
}

impl KeyInput {
    pub fn press(key: KeyCode, mods: u8) -> Self {
        let text = match &key {
            KeyCode::Char(c) if mods & !(mods::SHIFT | mods::CAPS_LOCK | mods::NUM_LOCK) == 0 => Some(c.to_string()),
            _ => None,
        };
        Self { key, shifted: None, base: None, mods, event: KeyEventType::Press, text }
    }
}

/// Result of encoding a key for the protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Encoded {
    /// Send these bytes.
    Bytes(Vec<u8>),
    /// Use the legacy (non-kitty) encoding.
    Legacy,
    /// Send nothing (e.g. a release event that isn't reported).
    Nothing,
}

const LOCKS: u8 = mods::CAPS_LOCK | mods::NUM_LOCK;

/// Encode `k` under the active progressive-enhancement `flags`.
pub fn encode_key(k: &KeyInput, f: u32) -> Encoded {
    if f == 0 {
        return if k.event == KeyEventType::Release { Encoded::Nothing } else { Encoded::Legacy };
    }
    let all = f & flags::REPORT_ALL_KEYS != 0;
    let events = f & flags::REPORT_EVENTS != 0;
    let release = k.event == KeyEventType::Release;
    if release && !events {
        return Encoded::Nothing;
    }
    let active = k.mods & !LOCKS;
    match &k.key {
        KeyCode::Char(c) => {
            if !all && active & !mods::SHIFT == 0 {
                // Plain text keys keep sending text.
                return if release { Encoded::Nothing } else { Encoded::Bytes(k.text.clone().unwrap_or_else(|| c.to_string()).into_bytes()) };
            }
            if !all && f & flags::DISAMBIGUATE == 0 {
                return if release { Encoded::Nothing } else { Encoded::Legacy };
            }
            // Lock modifiers are not reported for text keys unless all keys are escapes.
            let k = if all { k.clone() } else { KeyInput { mods: active, ..k.clone() } };
            Encoded::Bytes(csi_u(&k, lower(*c) as u32, f).into_bytes())
        }
        KeyCode::Func(func) => {
            let func = *func;
            if !all {
                if func.all_keys_only() {
                    return if release { Encoded::Nothing } else { Encoded::Legacy };
                }
                if matches!(func, Func::Enter | Func::Tab | Func::Backspace) {
                    // No release events for these without "report all keys", so
                    // `reset` stays typeable after a crashed program.
                    if release {
                        return Encoded::Nothing;
                    }
                    if active == 0 || f & flags::DISAMBIGUATE == 0 {
                        return Encoded::Legacy;
                    }
                }
                // Keypad keys that produce text stay text unless all keys are escapes.
                if matches!(func, Func::Keypad(_)) && (f & flags::DISAMBIGUATE == 0 || (k.text.is_some() && active & !mods::SHIFT == 0)) {
                    return if release { Encoded::Nothing } else { Encoded::Legacy };
                }
                if func == Func::Escape && f & flags::DISAMBIGUATE == 0 {
                    return if release { Encoded::Nothing } else { Encoded::Legacy };
                }
            }
            let s = match func.form() {
                Form::U(code) => csi_u(k, code, f),
                Form::Letter(l) => {
                    let m = mods_field(k, f);
                    if m.is_empty() { format!("\x1b[{}", l as char) } else { format!("\x1b[1;{m}{}", l as char) }
                }
                Form::Tilde(n) => {
                    let m = mods_field(k, f);
                    if m.is_empty() { format!("\x1b[{n}~") } else { format!("\x1b[{n};{m}~") }
                }
            };
            Encoded::Bytes(s.into_bytes())
        }
    }
}

fn lower(c: char) -> char {
    let mut l = c.to_lowercase();
    match (l.next(), l.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

/// `mods[:event]`, or empty when both are default.
fn mods_field(k: &KeyInput, f: u32) -> String {
    let ev = if f & flags::REPORT_EVENTS != 0 { k.event as u8 } else { 1 };
    let m = 1 + k.mods as u32;
    match (m, ev) {
        (1, 1) => String::new(),
        (m, 1) => m.to_string(),
        (m, e) => format!("{m}:{e}"),
    }
}

fn csi_u(k: &KeyInput, code: u32, f: u32) -> String {
    let mut s = format!("\x1b[{code}");
    if f & flags::REPORT_ALTERNATES != 0 {
        let shifted = k.shifted.filter(|c| k.mods & mods::SHIFT != 0 && *c as u32 != code);
        let base = k.base.filter(|c| *c as u32 != code);
        match (shifted, base) {
            (Some(sh), Some(b)) => s += &format!(":{}:{}", sh as u32, b as u32),
            (Some(sh), None) => s += &format!(":{}", sh as u32),
            (None, Some(b)) => s += &format!("::{}", b as u32),
            (None, None) => {}
        }
    }
    let m = mods_field(k, f);
    let text = (f & flags::REPORT_TEXT != 0 && f & flags::REPORT_ALL_KEYS != 0 && k.event != KeyEventType::Release)
        .then(|| k.text.as_deref().filter(|t| !t.is_empty() && !t.chars().any(char::is_control)))
        .flatten()
        .map(|t| t.chars().map(|c| (c as u32).to_string()).collect::<Vec<_>>().join(":"));
    match (m.is_empty(), text) {
        (true, None) => {}
        (_, None) => s += &format!(";{m}"),
        (_, Some(t)) => s += &format!(";{m};{t}"),
    }
    s.push('u');
    s
}
