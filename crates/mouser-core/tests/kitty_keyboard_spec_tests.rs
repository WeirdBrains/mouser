//! Kitty keyboard protocol conformance, one test per section of
//! https://sw.kovidgoyal.net/kitty/keyboard-protocol/ (fetched 2026-10-30).
//! Mouser implements only Kitty's keyboard protocol, as a fallback behind
//! Fuzzball keyboard mode.
use mouser_core::kitty::{Encoded, Func, KeyCode, KeyEventType, KeyInput, KeyboardProtocolState, encode_key, flags::*, mods::*};

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
use KeyEventType::*;

// "Key codes": always the un-shifted code point; alternates when requested.
#[test]
fn key_codes_and_alternates() {
    assert_eq!(enc(&ch('a', CTRL | SHIFT), DISAMBIGUATE), "\x1b[97;6u");
    let mut k = ch('a', CTRL | SHIFT);
    k.shifted = Some('A');
    assert_eq!(enc(&k, DISAMBIGUATE | REPORT_ALTERNATES), "\x1b[97:65;6u");
    // Shifted key only when shift is held.
    let mut k = ch('a', CTRL);
    k.shifted = Some('A');
    assert_eq!(enc(&k, DISAMBIGUATE | REPORT_ALTERNATES), "\x1b[97;5u");
    // Base layout key alone uses an empty shifted sub-field.
    let mut k = ch('с', CTRL);
    k.base = Some('c');
    assert_eq!(enc(&k, DISAMBIGUATE | REPORT_ALTERNATES), "\x1b[1089::99;5u");
    // Alternates never affect keys that are not already escapes.
    let mut k = ch('a', 0);
    k.base = Some('q');
    assert_eq!(enc(&k, REPORT_ALTERNATES), "a");
}

// "Modifiers": 1 + bits; locks included when active.
#[test]
fn modifier_encoding() {
    let all = SHIFT | ALT | CTRL | SUPER | HYPER | META;
    assert_eq!(enc(&ch('a', all), DISAMBIGUATE), "\x1b[97;64u");
    assert_eq!(enc(&ch('a', CTRL | CAPS_LOCK), REPORT_ALL_KEYS), "\x1b[97;69u");
    assert_eq!(enc(&ch('a', CTRL | NUM_LOCK), REPORT_ALL_KEYS), "\x1b[97;133u");
    // Modifier key events carry their own bit (set on press).
    assert_eq!(enc(&func(Func::Modifier(57442), CTRL), REPORT_ALL_KEYS), "\x1b[57442;5u");
    assert_eq!(enc(&ev(func(Func::Modifier(57442), 0), Release), REPORT_ALL_KEYS | REPORT_EVENTS), "\x1b[57442;1:3u");
}

// "Event types".
#[test]
fn event_types() {
    let f = DISAMBIGUATE | REPORT_EVENTS;
    assert_eq!(enc(&func(Func::Up, 0), f), "\x1b[A");
    assert_eq!(enc(&ev(func(Func::Up, 0), Repeat), f), "\x1b[1;1:2A");
    assert_eq!(enc(&ev(func(Func::Up, 0), Release), f), "\x1b[1;1:3A");
    assert_eq!(enc(&ev(func(Func::Escape, 0), Release), f), "\x1b[27;1:3u");
    // Text keys have no events without report-all.
    assert_eq!(enc(&ev(ch('a', 0), Release), f), "<nothing>");
    // Without the flag, releases are never sent.
    assert_eq!(enc(&ev(func(Func::Up, 0), Release), DISAMBIGUATE), "<nothing>");
}

// "Text as code points".
#[test]
fn associated_text() {
    let mut k = ch('a', SHIFT);
    k.text = Some("A".into());
    assert_eq!(enc(&k, REPORT_ALL_KEYS | REPORT_TEXT), "\x1b[97;2;65u");
    let mut k = ch('a', 0);
    k.text = Some("a".into());
    assert_eq!(enc(&k, REPORT_ALL_KEYS | REPORT_TEXT), "\x1b[97;;97u");
    // Control codes are never included.
    let mut k = func(Func::Enter, 0);
    k.text = Some("\r".into());
    assert_eq!(enc(&k, REPORT_ALL_KEYS | REPORT_TEXT), "\x1b[13u");
    // Text needs report-all; on its own it is undefined and ignored.
    assert_eq!(enc(&ch('a', 0), REPORT_TEXT), "a");
}

// "Progressive enhancement": set / push / pop / query, per screen.
#[test]
fn flag_stack() {
    let mut s = KeyboardProtocolState::default();
    s.set(false, 0b11, 1);
    assert_eq!(s.current(false), 3);
    s.set(false, 0b100, 2);
    assert_eq!(s.current(false), 7);
    s.set(false, 0b1, 3);
    assert_eq!(s.current(false), 6);
    assert_eq!(s.query_response(false), "\x1b[?6u");
    s.push(false, 1);
    s.push(false, 31);
    assert_eq!(s.current(false), 31);
    s.pop(false, 1);
    assert_eq!(s.current(false), 1);
    s.pop(false, 99);
    assert_eq!(s.current(false), 0, "emptying the stack resets all flags");
    // Separate main and alternate stacks.
    s.push(true, 8);
    assert_eq!((s.current(false), s.current(true)), (0, 8));
    // Full stack evicts the oldest entry.
    for i in 0..200 {
        s.push(false, i % 32);
    }
    assert!(s.main.len() <= 64);
    assert_eq!(s.current(false), 199 % 32);
    // Unknown bits are dropped.
    s.push(false, 0xFFFF);
    assert_eq!(s.current(false), ALL);
}

// "Disambiguate escape codes".
#[test]
fn disambiguate() {
    let f = DISAMBIGUATE;
    assert_eq!(enc(&func(Func::Escape, 0), f), "\x1b[27u");
    assert_eq!(enc(&ch('a', ALT), f), "\x1b[97;3u");
    assert_eq!(enc(&ch('c', CTRL), f), "\x1b[99;5u");
    assert_eq!(enc(&ch('a', CTRL | ALT), f), "\x1b[97;7u");
    assert_eq!(enc(&ch('a', SHIFT | ALT), f), "\x1b[97;4u");
    // Plain and shifted text stays text.
    assert_eq!(enc(&ch('a', SHIFT), f), "a");
    // Enter / Tab / Backspace keep legacy bytes when unmodified.
    for k in [Func::Enter, Func::Tab, Func::Backspace] {
        assert_eq!(enc(&func(k, 0), f), "<legacy>");
    }
    // Lock modifiers are not reported for text keys.
    assert_eq!(enc(&ch('a', CTRL | CAPS_LOCK | NUM_LOCK), f), "\x1b[97;5u");
    // Non-text keypad keys get their own codes; text keypad keys stay text.
    assert_eq!(enc(&func(Func::Keypad(57414), 0), f), "\x1b[57414u");
    assert_eq!(enc(&func(Func::Keypad(57417), 0), f), "\x1b[57417u");
    let mut k = func(Func::Keypad(57400), 0);
    k.text = Some("1".into());
    assert_eq!(enc(&k, f), "<legacy>");
}

// "Report event types": no Enter/Tab/Backspace releases without report-all.
#[test]
fn enter_tab_backspace_releases() {
    let f = DISAMBIGUATE | REPORT_EVENTS;
    for k in [Func::Enter, Func::Tab, Func::Backspace] {
        assert_eq!(enc(&ev(func(k, CTRL), Release), f), "<nothing>");
        assert_ne!(enc(&ev(func(k, 0), Release), f | REPORT_ALL_KEYS), "<nothing>");
    }
}

// "Report all keys as escape codes".
#[test]
fn report_all_keys() {
    let f = REPORT_ALL_KEYS;
    assert_eq!(enc(&ch('a', 0), f), "\x1b[97u");
    assert_eq!(enc(&func(Func::Enter, 0), f), "\x1b[13u");
    assert_eq!(enc(&func(Func::Tab, 0), f), "\x1b[9u");
    assert_eq!(enc(&func(Func::Backspace, 0), f), "\x1b[127u");
    assert_eq!(enc(&func(Func::Modifier(57441), SHIFT), f), "\x1b[57441;2u");
    assert_eq!(enc(&func(Func::CapsLock, CAPS_LOCK), f), "\x1b[57358;65u");
    // Modifier keys are not reported without it.
    assert_eq!(enc(&func(Func::Modifier(57441), SHIFT), DISAMBIGUATE), "<legacy>");
}

// "Functional key definitions" table.
#[test]
fn functional_key_table() {
    let f = DISAMBIGUATE;
    let cases: &[(Func, &str)] = &[
        (Func::Escape, "\x1b[27u"),
        (Func::Insert, "\x1b[2~"),
        (Func::Delete, "\x1b[3~"),
        (Func::Left, "\x1b[D"),
        (Func::Right, "\x1b[C"),
        (Func::Up, "\x1b[A"),
        (Func::Down, "\x1b[B"),
        (Func::PageUp, "\x1b[5~"),
        (Func::PageDown, "\x1b[6~"),
        (Func::Home, "\x1b[H"),
        (Func::End, "\x1b[F"),
        (Func::PrintScreen, "\x1b[57361u"),
        (Func::Pause, "\x1b[57362u"),
        (Func::Menu, "\x1b[57363u"),
        (Func::F(1), "\x1b[P"),
        (Func::F(2), "\x1b[Q"),
        (Func::F(3), "\x1b[13~"),
        (Func::F(4), "\x1b[S"),
        (Func::F(5), "\x1b[15~"),
        (Func::F(6), "\x1b[17~"),
        (Func::F(7), "\x1b[18~"),
        (Func::F(8), "\x1b[19~"),
        (Func::F(9), "\x1b[20~"),
        (Func::F(10), "\x1b[21~"),
        (Func::F(11), "\x1b[23~"),
        (Func::F(12), "\x1b[24~"),
        (Func::F(13), "\x1b[57376u"),
        (Func::F(24), "\x1b[57387u"),
        (Func::F(35), "\x1b[57398u"),
        (Func::Media(57430), "\x1b[57430u"),
        (Func::Media(57440), "\x1b[57440u"),
        (Func::Keypad(57427), "\x1b[57427u"),
    ];
    for (k, want) in cases {
        assert_eq!(enc(&func(*k, 0), f), *want, "{k:?}");
    }
    let r = REPORT_ALL_KEYS;
    assert_eq!(enc(&func(Func::CapsLock, 0), r), "\x1b[57358u");
    assert_eq!(enc(&func(Func::ScrollLock, 0), r), "\x1b[57359u");
    assert_eq!(enc(&func(Func::NumLock, 0), r), "\x1b[57360u");
    assert_eq!(enc(&func(Func::Modifier(57454), 0), r), "\x1b[57454u");
    // With modifiers the letter forms gain the `1;`.
    assert_eq!(enc(&func(Func::F(1), CTRL), f), "\x1b[1;5P");
    assert_eq!(enc(&func(Func::End, SHIFT), f), "\x1b[1;2F");
}

// "Detection of support for this protocol": CSI ? u answers before DA1.
#[test]
fn detection_query_before_da1() {
    use mouser_core::{Scrollback, Size, TerminalParser};
    use std::sync::{Arc, Mutex};
    #[derive(Clone, Default)]
    struct Out(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Out {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let out = Out::default();
    let sb = Arc::new(Scrollback::new(Default::default()));
    let mut t = TerminalParser::new(Size { cols: 10, rows: 2 }, sb, out.clone());
    t.feed(b"\x1b[>5u\x1b[?u\x1b[c");
    let r = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    assert!(r.starts_with("\x1b[?5u\x1b[?62;"), "{r:?}");
    // Alternate screen starts with its own empty stack.
    t.feed(b"\x1b[?1049h\x1b[?u");
    assert!(String::from_utf8(out.0.lock().unwrap().clone()).unwrap().ends_with("\x1b[?0u"));
    // RIS resets.
    t.feed(b"\x1b[?1049l\x1bc");
    assert_eq!(t.kitty_keyboard.current(false), 0);
}
