//! Kitty keyboard protocol: flag stack and key encodings, checked against the
//! examples in https://sw.kovidgoyal.net/kitty/keyboard-protocol/.
use mouser_core::kitty::{Encoded, Func, KeyCode, KeyEventType, KeyInput, KeyboardProtocolState, encode_key, flags::*, mods::*};
use mouser_core::{Scrollback, Size, TerminalParser};
use std::sync::{Arc, Mutex};

fn enc(k: &KeyInput, f: u32) -> String {
    match encode_key(k, f) {
        Encoded::Bytes(b) => String::from_utf8(b).unwrap(),
        Encoded::Legacy => "<legacy>".into(),
        Encoded::Nothing => "<nothing>".into(),
    }
}
fn ch(c: char, m: u8) -> KeyInput {
    KeyInput::press(KeyCode::Char(c), m)
}
fn func(f: Func, m: u8) -> KeyInput {
    KeyInput::press(KeyCode::Func(f), m)
}
fn ev(mut k: KeyInput, e: KeyEventType) -> KeyInput {
    k.event = e;
    k
}

#[test]
fn flags_zero_is_legacy() {
    assert_eq!(enc(&ch('a', CTRL), 0), "<legacy>");
    assert_eq!(enc(&ev(ch('a', 0), KeyEventType::Release), 0), "<nothing>");
}

#[test]
fn disambiguate() {
    let f = DISAMBIGUATE;
    assert_eq!(enc(&ch('a', 0), f), "a");
    assert_eq!(enc(&ch('a', SHIFT), f).len(), 1); // text
    assert_eq!(enc(&ch('a', CTRL), f), "\x1b[97;5u");
    assert_eq!(enc(&ch('a', ALT), f), "\x1b[97;3u");
    assert_eq!(enc(&ch('a', CTRL | SHIFT), f), "\x1b[97;6u");
    assert_eq!(enc(&func(Func::Escape, 0), f), "\x1b[27u");
    assert_eq!(enc(&func(Func::Enter, 0), f), "<legacy>");
    assert_eq!(enc(&func(Func::Enter, CTRL), f), "\x1b[13;5u");
    assert_eq!(enc(&func(Func::Tab, SHIFT), f), "\x1b[9;2u");
    assert_eq!(enc(&func(Func::Backspace, ALT), f), "\x1b[127;3u");
    // Lock keys alone don't make a text key "modified".
    assert_eq!(enc(&ch('a', CAPS_LOCK), f), "a");
}

#[test]
fn functional_legacy_letter_and_tilde_forms() {
    let f = DISAMBIGUATE;
    assert_eq!(enc(&func(Func::Up, 0), f), "\x1b[A");
    assert_eq!(enc(&func(Func::Up, CTRL), f), "\x1b[1;5A");
    assert_eq!(enc(&func(Func::Home, SHIFT), f), "\x1b[1;2H");
    assert_eq!(enc(&func(Func::F(1), 0), f), "\x1b[P");
    assert_eq!(enc(&func(Func::F(3), 0), f), "\x1b[13~");
    assert_eq!(enc(&func(Func::F(5), SHIFT), f), "\x1b[15;2~");
    assert_eq!(enc(&func(Func::F(12), 0), f), "\x1b[24~");
    assert_eq!(enc(&func(Func::Delete, CTRL | ALT), f), "\x1b[3;7~");
    assert_eq!(enc(&func(Func::PageDown, 0), f), "\x1b[6~");
    // Keys that only exist under report-all.
    assert_eq!(enc(&func(Func::F(13), 0), f), "<legacy>");
    assert_eq!(enc(&func(Func::F(13), 0), REPORT_ALL_KEYS), "\x1b[57376u");
    assert_eq!(enc(&func(Func::F(35), 0), REPORT_ALL_KEYS), "\x1b[57398u");
    assert_eq!(enc(&func(Func::CapsLock, 0), REPORT_ALL_KEYS), "\x1b[57358u");
}

#[test]
fn event_types() {
    let f = DISAMBIGUATE | REPORT_EVENTS;
    assert_eq!(enc(&ev(ch('a', CTRL), KeyEventType::Repeat), f), "\x1b[97;5:2u");
    assert_eq!(enc(&ev(ch('a', CTRL), KeyEventType::Release), f), "\x1b[97;5:3u");
    assert_eq!(enc(&ev(func(Func::Up, 0), KeyEventType::Release), f), "\x1b[1;1:3A");
    assert_eq!(enc(&ev(func(Func::F(5), 0), KeyEventType::Repeat), f), "\x1b[15;1:2~");
    // Plain text keys: text on press, no release event without report-all.
    assert_eq!(enc(&ev(ch('a', 0), KeyEventType::Release), f), "<nothing>");
    assert_eq!(enc(&ev(func(Func::Enter, 0), KeyEventType::Release), f), "<nothing>");
    // Without the flag, releases are dropped.
    assert_eq!(enc(&ev(ch('a', CTRL), KeyEventType::Release), DISAMBIGUATE), "<nothing>");
    let all = REPORT_ALL_KEYS | REPORT_EVENTS;
    assert_eq!(enc(&ev(ch('a', 0), KeyEventType::Release), all), "\x1b[97;1:3u");
    assert_eq!(enc(&ev(func(Func::Enter, 0), KeyEventType::Release), all), "\x1b[13;1:3u");
}

#[test]
fn report_all_keys_and_modifier_keys() {
    let f = REPORT_ALL_KEYS;
    assert_eq!(enc(&ch('a', 0), f), "\x1b[97u");
    assert_eq!(enc(&ch('a', SHIFT), f), "\x1b[97;2u");
    assert_eq!(enc(&func(Func::Enter, 0), f), "\x1b[13u");
    assert_eq!(enc(&func(Func::Tab, 0), f), "\x1b[9u");
    assert_eq!(enc(&func(Func::Backspace, 0), f), "\x1b[127u");
    // Left Shift press: the shift bit is already set.
    assert_eq!(enc(&func(Func::Modifier(57441), SHIFT), f), "\x1b[57441;2u");
    let fe = f | REPORT_EVENTS;
    assert_eq!(enc(&ev(func(Func::Modifier(57441), 0), KeyEventType::Release), fe), "\x1b[57441;1:3u");
    assert_eq!(enc(&func(Func::Keypad(57399), 0), f), "\x1b[57399u");
}

#[test]
fn alternate_keys() {
    let f = REPORT_ALL_KEYS | REPORT_ALTERNATES;
    let mut k = ch('a', SHIFT);
    k.shifted = Some('A');
    assert_eq!(enc(&k, f), "\x1b[97:65;2u");
    // Shifted key only when shift is held.
    let mut k = ch('a', 0);
    k.shifted = Some('A');
    assert_eq!(enc(&k, f), "\x1b[97u");
    // Base layout key (e.g. Cyrillic 'ф' on the US 'a' key) with ctrl.
    let mut k = ch('ф', CTRL);
    k.base = Some('a');
    assert_eq!(enc(&k, DISAMBIGUATE | REPORT_ALTERNATES), "\x1b[1092::97;5u");
    let mut k = ch('ф', CTRL | SHIFT);
    k.shifted = Some('Ф');
    k.base = Some('a');
    assert_eq!(enc(&k, DISAMBIGUATE | REPORT_ALTERNATES), "\x1b[1092:1060:97;6u");
}

#[test]
fn associated_text() {
    let f = REPORT_ALL_KEYS | REPORT_TEXT;
    assert_eq!(enc(&ch('a', 0), f), "\x1b[97;;97u");
    let mut k = ch('a', SHIFT);
    k.text = Some("A".into());
    assert_eq!(enc(&k, f), "\x1b[97;2;65u");
    // No text for keys that produce none, or with ctrl.
    assert_eq!(enc(&ch('a', CTRL), f), "\x1b[97;5u");
    // Text needs report-all.
    assert_eq!(enc(&ch('a', 0), DISAMBIGUATE | REPORT_TEXT), "a");
}

#[test]
fn flag_stack_push_pop_set_per_screen() {
    let mut s = KeyboardProtocolState::default();
    assert_eq!(s.current(false), 0);
    s.push(false, 1);
    s.push(false, 1 | 8);
    assert_eq!(s.current(false), 9);
    assert_eq!(s.current(true), 0);
    s.set(false, 2, 2);
    assert_eq!(s.current(false), 11);
    s.set(false, 8, 3);
    assert_eq!(s.current(false), 3);
    s.set(false, 31, 1);
    assert_eq!(s.query_response(false), "\x1b[?31u");
    s.pop(false, 1);
    assert_eq!(s.current(false), 1);
    s.pop(false, 5);
    assert_eq!(s.current(false), 0);
    for i in 0..100 {
        s.push(true, i % 32);
    }
    assert_eq!(s.alt.len(), 64);
}

#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn parser_handles_csi_u_sequences_per_screen() {
    let sink = Sink::default();
    let mut p = TerminalParser::new(
        Size { cols: 20, rows: 5 },
        Arc::new(Scrollback::new(mouser_core::scrollback::ScrollbackConfig::default())),
        sink.clone(),
    );
    p.feed(b"\x1b[?u");
    p.feed(b"\x1b[>5u\x1b[?u");
    p.feed(b"\x1b[?1049h\x1b[?u\x1b[>31u\x1b[?u");
    p.feed(b"\x1b[?1049l\x1b[?u\x1b[=1;3u\x1b[?u\x1b[<u\x1b[?u");
    let out = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
    assert_eq!(out, "\x1b[?0u\x1b[?5u\x1b[?0u\x1b[?31u\x1b[?5u\x1b[?4u\x1b[?0u");
}
