//! Emoji clusters: ZWJ sequences, skin tones, flags and VS16 occupy one 2-cell slot.

use mouser_core::grid::attr;
use mouser_core::parser::TerminalParser;
use mouser_core::scrollback::{Scrollback, ScrollbackConfig};
use mouser_core::types::Size;
use std::sync::Arc;

fn term(cols: usize) -> TerminalParser<std::io::Sink> {
    TerminalParser::new(Size { cols, rows: 1 }, Arc::new(Scrollback::new(ScrollbackConfig::default())), std::io::sink())
}

fn c(t: &TerminalParser<std::io::Sink>, x: usize) -> mouser_core::Cell {
    t.screen.row(0)[x]
}

fn wide(t: &TerminalParser<std::io::Sink>, x: usize) -> bool {
    c(t, x).style.attrs & attr::WIDE != 0
}

#[test]
fn skin_tone_joins_the_base() {
    let mut t = term(10);
    t.feed("👍🏽x".as_bytes());
    assert!(wide(&t, 0));
    assert_eq!(c(&t, 0).comb[0], '\u{1f3fd}');
    assert_eq!(c(&t, 2).ch, 'x');
    assert_eq!(t.screen.cursor.x, 3);
}

#[test]
fn zwj_sequence_is_one_cluster() {
    let mut t = term(10);
    t.feed("👩\u{200d}💻x".as_bytes());
    assert_eq!(c(&t, 0).comb[..2], ['\u{200d}', '💻']);
    assert_eq!(c(&t, 2).ch, 'x');
    let mut t = term(10);
    t.feed("👩🏽\u{200d}💻x".as_bytes());
    assert_eq!(c(&t, 0).comb, ['\u{1f3fd}', '\u{200d}', '💻']);
    assert_eq!(t.screen.cursor.x, 3);
}

#[test]
fn vs16_widens_text_symbols() {
    let mut t = term(10);
    t.feed("❤\u{fe0f}x".as_bytes());
    assert!(wide(&t, 0));
    assert!(c(&t, 1).style.attrs & attr::WIDE_CONT != 0);
    assert_eq!(c(&t, 2).ch, 'x');
    // Without VS16 it stays one cell; VS15 never widens.
    let mut t = term(10);
    t.feed("❤x\u{2764}\u{fe0e}y".as_bytes());
    assert_eq!(c(&t, 1).ch, 'x');
    assert_eq!(c(&t, 3).ch, 'y');
    // Already-wide emoji are unchanged.
    let mut t = term(10);
    t.feed("✅\u{fe0f}x".as_bytes());
    assert_eq!(c(&t, 2).ch, 'x');
}

#[test]
fn flags_pair_up() {
    let mut t = term(10);
    t.feed("🇺🇸🇯🇵x".as_bytes());
    assert_eq!(c(&t, 0).comb[0], '🇸');
    assert_eq!(c(&t, 2).ch, '🇯');
    assert_eq!(c(&t, 2).comb[0], '🇵');
    assert_eq!(c(&t, 4).ch, 'x');
}
