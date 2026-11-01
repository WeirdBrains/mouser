//! winit key events -> toolkit-neutral kitty keyboard protocol input.

use mouser_core::kitty::{Func, KeyCode, KeyEventType, KeyInput, mods};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, KeyCode as Phys, KeyLocation, ModifiersState, NamedKey, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

/// Protocol modifier bits for winit's modifier state.
pub fn mod_bits(m: ModifiersState) -> u8 {
    let mut b = 0;
    if m.shift_key() {
        b |= mods::SHIFT;
    }
    if m.alt_key() {
        b |= mods::ALT;
    }
    if m.control_key() {
        b |= mods::CTRL;
    }
    if m.super_key() {
        b |= mods::SUPER;
    }
    b
}

/// The character this physical key produces on a US layout (for "base layout key").
pub fn us_layout_char(p: Phys) -> Option<char> {
    use Phys::*;
    let c = match p {
        KeyA => 'a',
        KeyB => 'b',
        KeyC => 'c',
        KeyD => 'd',
        KeyE => 'e',
        KeyF => 'f',
        KeyG => 'g',
        KeyH => 'h',
        KeyI => 'i',
        KeyJ => 'j',
        KeyK => 'k',
        KeyL => 'l',
        KeyM => 'm',
        KeyN => 'n',
        KeyO => 'o',
        KeyP => 'p',
        KeyQ => 'q',
        KeyR => 'r',
        KeyS => 's',
        KeyT => 't',
        KeyU => 'u',
        KeyV => 'v',
        KeyW => 'w',
        KeyX => 'x',
        KeyY => 'y',
        KeyZ => 'z',
        Digit0 => '0',
        Digit1 => '1',
        Digit2 => '2',
        Digit3 => '3',
        Digit4 => '4',
        Digit5 => '5',
        Digit6 => '6',
        Digit7 => '7',
        Digit8 => '8',
        Digit9 => '9',
        Minus => '-',
        Equal => '=',
        BracketLeft => '[',
        BracketRight => ']',
        Backslash => '\\',
        Semicolon => ';',
        Quote => '\'',
        Backquote => '`',
        Comma => ',',
        Period => '.',
        Slash => '/',
        Space => ' ',
        _ => return None,
    };
    Some(c)
}

fn keypad_code(p: Phys) -> Option<u32> {
    use Phys::*;
    Some(match p {
        Numpad0 => 57399,
        Numpad1 => 57400,
        Numpad2 => 57401,
        Numpad3 => 57402,
        Numpad4 => 57403,
        Numpad5 => 57404,
        Numpad6 => 57405,
        Numpad7 => 57406,
        Numpad8 => 57407,
        Numpad9 => 57408,
        NumpadDecimal => 57409,
        NumpadDivide => 57410,
        NumpadMultiply => 57411,
        NumpadSubtract => 57412,
        NumpadAdd => 57413,
        NumpadEnter => 57414,
        NumpadEqual => 57415,
        NumpadComma => 57416,
        _ => return None,
    })
}

/// Keypad keys with Num Lock off report KP_LEFT … KP_BEGIN (57417..=57427).
fn keypad_nav_code(k: &Key) -> Option<u32> {
    use NamedKey as N;
    Some(match k {
        Key::Named(N::ArrowLeft) => 57417,
        Key::Named(N::ArrowRight) => 57418,
        Key::Named(N::ArrowUp) => 57419,
        Key::Named(N::ArrowDown) => 57420,
        Key::Named(N::PageUp) => 57421,
        Key::Named(N::PageDown) => 57422,
        Key::Named(N::Home) => 57423,
        Key::Named(N::End) => 57424,
        Key::Named(N::Insert) => 57425,
        Key::Named(N::Delete) => 57426,
        Key::Named(N::Clear) => 57427,
        _ => return None,
    })
}

fn named(n: NamedKey, phys: Option<Phys>) -> Option<Func> {
    use NamedKey as N;
    let right = |l: Phys| phys.is_some_and(|p| p == l);
    Some(match n {
        N::Escape => Func::Escape,
        N::Enter => Func::Enter,
        N::Tab => Func::Tab,
        N::Backspace => Func::Backspace,
        N::Insert => Func::Insert,
        N::Delete => Func::Delete,
        N::ArrowLeft => Func::Left,
        N::ArrowRight => Func::Right,
        N::ArrowUp => Func::Up,
        N::ArrowDown => Func::Down,
        N::PageUp => Func::PageUp,
        N::PageDown => Func::PageDown,
        N::Home => Func::Home,
        N::End => Func::End,
        N::CapsLock => Func::CapsLock,
        N::ScrollLock => Func::ScrollLock,
        N::NumLock => Func::NumLock,
        N::PrintScreen => Func::PrintScreen,
        N::Pause => Func::Pause,
        N::ContextMenu => Func::Menu,
        N::MediaPlay => Func::Media(57428),
        N::MediaPause => Func::Media(57429),
        N::MediaPlayPause => Func::Media(57430),
        N::MediaStop => Func::Media(57432),
        N::MediaFastForward => Func::Media(57433),
        N::MediaRewind => Func::Media(57434),
        N::MediaTrackNext => Func::Media(57435),
        N::MediaTrackPrevious => Func::Media(57436),
        N::MediaRecord => Func::Media(57437),
        N::AudioVolumeDown => Func::Media(57438),
        N::AudioVolumeUp => Func::Media(57439),
        N::AudioVolumeMute => Func::Media(57440),
        N::Shift => Func::Modifier(if right(Phys::ShiftRight) { 57447 } else { 57441 }),
        N::Control => Func::Modifier(if right(Phys::ControlRight) { 57448 } else { 57442 }),
        N::Alt => Func::Modifier(if right(Phys::AltRight) { 57449 } else { 57443 }),
        N::Super | N::Meta => Func::Modifier(if right(Phys::SuperRight) { 57450 } else { 57444 }),
        N::Hyper => Func::Modifier(57445),
        N::AltGraph => Func::Modifier(57453),
        N::F1 => Func::F(1),
        N::F2 => Func::F(2),
        N::F3 => Func::F(3),
        N::F4 => Func::F(4),
        N::F5 => Func::F(5),
        N::F6 => Func::F(6),
        N::F7 => Func::F(7),
        N::F8 => Func::F(8),
        N::F9 => Func::F(9),
        N::F10 => Func::F(10),
        N::F11 => Func::F(11),
        N::F12 => Func::F(12),
        N::F13 => Func::F(13),
        N::F14 => Func::F(14),
        N::F15 => Func::F(15),
        N::F16 => Func::F(16),
        N::F17 => Func::F(17),
        N::F18 => Func::F(18),
        N::F19 => Func::F(19),
        N::F20 => Func::F(20),
        N::F21 => Func::F(21),
        N::F22 => Func::F(22),
        N::F23 => Func::F(23),
        N::F24 => Func::F(24),
        N::F25 => Func::F(25),
        N::F26 => Func::F(26),
        N::F27 => Func::F(27),
        N::F28 => Func::F(28),
        N::F29 => Func::F(29),
        N::F30 => Func::F(30),
        N::F31 => Func::F(31),
        N::F32 => Func::F(32),
        N::F33 => Func::F(33),
        N::F34 => Func::F(34),
        N::F35 => Func::F(35),
        _ => return None,
    })
}

/// Describe a winit key event for the kitty keyboard protocol.
pub fn kitty_input(ev: &KeyEvent, m: ModifiersState) -> Option<KeyInput> {
    let phys = match ev.physical_key {
        PhysicalKey::Code(c) => Some(c),
        _ => None,
    };
    let event = match (ev.state, ev.repeat) {
        (ElementState::Released, _) => KeyEventType::Release,
        (_, true) => KeyEventType::Repeat,
        _ => KeyEventType::Press,
    };
    let mut bits = mod_bits(m);
    let key = if ev.location == KeyLocation::Numpad
        && let Some(code) = keypad_nav_code(&ev.logical_key).or_else(|| phys.and_then(keypad_code))
    {
        KeyCode::Func(Func::Keypad(code))
    } else {
        match &ev.logical_key {
            Key::Named(NamedKey::Space) => KeyCode::Char(' '),
            Key::Named(n) => {
                let f = named(*n, phys)?;
                // A modifier key's own press already counts as held; its release doesn't.
                if let Func::Modifier(code) = f {
                    let bit = match code {
                        57441 | 57447 => mods::SHIFT,
                        57442 | 57448 => mods::CTRL,
                        57443 | 57449 => mods::ALT,
                        57444 | 57450 => mods::SUPER,
                        57445 => mods::HYPER,
                        _ => 0,
                    };
                    if event == KeyEventType::Release { bits &= !bit } else { bits |= bit }
                }
                KeyCode::Func(f)
            }
            Key::Character(_) => {
                let base = match ev.key_without_modifiers() {
                    Key::Character(s) => s.chars().next(),
                    _ => None,
                }?;
                KeyCode::Char(base.to_lowercase().next().unwrap_or(base))
            }
            _ => return None,
        }
    };
    let (shifted, base) = match &key {
        KeyCode::Char(c) => {
            let shifted = match &ev.logical_key {
                Key::Character(s) if m.shift_key() => s.chars().next().filter(|s| s != c),
                _ => None,
            };
            (shifted, phys.and_then(us_layout_char).filter(|b| b != c))
        }
        _ => (None, None),
    };
    // Ctrl/Alt/Super combinations carry no associated text (some keymaps, e.g. virtual
    // keyboards, still report the plain character).
    let text = text_for(ev.text.as_deref(), bits);
    Some(KeyInput { key, shifted, base, mods: bits, event, text })
}

/// Describe a winit key event for Fuzzball keyboard mode (docs/protocol/keyboard.md).
pub fn fuzzball_event(ev: &KeyEvent, m: ModifiersState) -> Option<mouser_core::fuzzball::KeyEvent> {
    use mouser_core::fuzzball::{KeyAction, KeyName, mods as fm};
    let PhysicalKey::Code(phys) = ev.physical_key else { return None };
    // winit's physical key names are the W3C UI Events `code` values.
    let code = format!("{phys:?}");
    let action = match (ev.state, ev.repeat) {
        (ElementState::Released, _) => KeyAction::Release,
        (_, true) => KeyAction::Repeat,
        _ => KeyAction::Press,
    };
    let mut bits = 0;
    if m.shift_key() {
        bits |= fm::SHIFT;
    }
    if m.alt_key() {
        bits |= fm::ALT;
    }
    if m.control_key() {
        bits |= fm::CTRL;
    }
    if m.super_key() {
        bits |= fm::SUPER;
    }
    let key = match ev.key_without_modifiers() {
        Key::Character(s) => KeyName::Char(s.chars().next()?),
        Key::Named(NamedKey::Space) => KeyName::Char(' '),
        Key::Named(n) => {
            // `mods` reflects the state after a modifier key's own event.
            let bit = match n {
                NamedKey::Shift => fm::SHIFT,
                NamedKey::Control => fm::CTRL,
                NamedKey::Alt => fm::ALT,
                NamedKey::Super | NamedKey::Meta => fm::SUPER,
                NamedKey::Hyper => fm::HYPER,
                NamedKey::AltGraph => fm::ALTGR,
                _ => 0,
            };
            if action == KeyAction::Release {
                bits &= !bit
            } else {
                bits |= bit
            }
            KeyName::Named(format!("{n:?}"))
        }
        _ => return None,
    };
    // Text only for presses/repeats without Ctrl/Alt/Super (those are shortcuts).
    let text = ev.text.as_ref().map(|t| t.to_string()).filter(|t| !t.is_empty() && bits & (fm::CTRL | fm::ALT | fm::SUPER) == 0);
    Some(mouser_core::fuzzball::KeyEvent { action, code, key, mods: bits, locks: 0, text })
}

/// Kitty associated text: none when Ctrl, Alt or Super is held, or for control characters.
pub fn text_for(text: Option<&str>, mods: u8) -> Option<String> {
    use mouser_core::kitty::mods as km;
    if mods & (km::CTRL | km::ALT | km::SUPER) != 0 {
        return None;
    }
    text.map(str::to_string).filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
}

#[cfg(test)]
mod text_tests {
    use super::text_for;
    use mouser_core::kitty::mods;

    #[test]
    fn modified_keys_have_no_text() {
        assert_eq!(text_for(Some("x"), mods::CTRL), None);
        assert_eq!(text_for(Some("x"), mods::ALT), None);
        assert_eq!(text_for(Some("x"), mods::SUPER), None);
        assert_eq!(text_for(Some("\u{18}"), 0), None);
        assert_eq!(text_for(Some("A"), mods::SHIFT).as_deref(), Some("A"));
        assert_eq!(text_for(Some("a"), mods::CAPS_LOCK).as_deref(), Some("a"));
    }
}
