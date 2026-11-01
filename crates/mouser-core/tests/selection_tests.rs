use mouser_core::parser::TerminalParser;
use mouser_core::scrollback::{Scrollback, ScrollbackConfig};
use mouser_core::selection::{DEFAULT_WORD_CHARS as WC, Mode, ParserRows, Pos, Row, Selection};
use mouser_core::types::Size;
use std::sync::Arc;

fn term(cols: usize, rows: usize, input: &str) -> TerminalParser<Vec<u8>> {
    let sb = Arc::new(Scrollback::new(ScrollbackConfig::default()));
    let mut t = TerminalParser::new(Size { cols, rows }, sb, Vec::new());
    t.feed(input.as_bytes());
    t
}

fn sel_text(t: &TerminalParser<Vec<u8>>, a: (i64, usize), b: (i64, usize), mode: Mode) -> String {
    let pr = ParserRows::new(t);
    let rows = |l: i64| pr.row(l);
    let mut s = Selection::start(Pos::new(a.0, a.1), mode, &rows, WC);
    s.extend(Pos::new(b.0, b.1));
    s.text(&rows, WC).unwrap()
}

fn click(t: &TerminalParser<Vec<u8>>, at: (i64, usize), mode: Mode) -> String {
    let pr = ParserRows::new(t);
    let rows = |l: i64| pr.row(l);
    Selection::start(Pos::new(at.0, at.1), mode, &rows, WC).text(&rows, WC).unwrap()
}

#[test]
fn char_selection_trims_and_breaks_lines() {
    let t = term(20, 4, "hello world\r\nsecond line\r\n");
    assert_eq!(sel_text(&t, (0, 6), (0, 19), Mode::Char), "world");
    assert_eq!(sel_text(&t, (0, 6), (1, 5), Mode::Char), "world\nsecond");
    // Backwards drag gives the same text.
    assert_eq!(sel_text(&t, (1, 5), (0, 6), Mode::Char), "world\nsecond");
}

#[test]
fn plain_click_selects_nothing() {
    let t = term(20, 2, "abc");
    let pr = ParserRows::new(&t);
    let rows = |l: i64| pr.row(l);
    let s = Selection::start(Pos::new(0, 1), Mode::Char, &rows, WC);
    assert!(s.is_empty());
    assert_eq!(s.text(&rows, WC).unwrap(), "");
}

#[test]
fn soft_wraps_join_and_hard_breaks_do_not() {
    // 10 columns: "0123456789abc" wraps after '9'.
    let t = term(10, 4, "0123456789abc\r\nnext");
    assert_eq!(sel_text(&t, (0, 0), (2, 3), Mode::Char), "0123456789abc\nnext");
    // A wrapped row keeps its trailing spaces: "hello" + 5 blanks then "world".
    let t = term(10, 4, "hello     world");
    assert_eq!(sel_text(&t, (0, 0), (1, 4), Mode::Char), "hello     world");
}

#[test]
fn word_selection_uses_word_chars_and_crosses_wraps() {
    let t = term(30, 2, "see https://x.io/a_b?c=1 now");
    assert_eq!(click(&t, (0, 10), Mode::Word), "https://x.io/a_b?c=1");
    assert_eq!(click(&t, (0, 1), Mode::Word), "see");
    // Punctuation not in the set is a word of its own.
    let t = term(20, 2, "foo(bar)");
    assert_eq!(click(&t, (0, 3), Mode::Word), "(");
    assert_eq!(click(&t, (0, 5), Mode::Word), "bar");
    // A word broken by autowrap is one word.
    let t = term(8, 3, "aa bbbbbbbbb cc");
    assert_eq!(click(&t, (1, 1), Mode::Word), "bbbbbbbbb");
    let pr = ParserRows::new(&t);
    let rows = |l: i64| pr.row(l);
    let s = Selection::start(Pos::new(0, 5), Mode::Word, &rows, "");
    assert_eq!(s.text(&rows, "").unwrap(), "bbbbbbbbb");
}

#[test]
fn word_drag_extends_by_words() {
    let t = term(30, 2, "one two three four");
    let pr = ParserRows::new(&t);
    let rows = |l: i64| pr.row(l);
    let mut s = Selection::start(Pos::new(0, 5), Mode::Word, &rows, WC);
    s.extend(Pos::new(0, 9));
    assert_eq!(s.text(&rows, WC).unwrap(), "two three");
    s.extend(Pos::new(0, 1));
    assert_eq!(s.text(&rows, WC).unwrap(), "one two");
}

#[test]
fn line_selection_follows_soft_wraps() {
    let t = term(5, 4, "abcdefgh\r\nxyz");
    assert_eq!(click(&t, (1, 1), Mode::Line), "abcdefgh");
    assert_eq!(click(&t, (2, 0), Mode::Line), "xyz");
}

#[test]
fn wide_chars_and_combining_marks() {
    let t = term(10, 2, "a日本b e\u{301}x");
    // Clicking the right half of 日 still starts at 日; ending on 本's left half takes it whole.
    assert_eq!(sel_text(&t, (0, 2), (0, 3), Mode::Char), "日本");
    assert_eq!(sel_text(&t, (0, 0), (0, 9), Mode::Char), "a日本b e\u{301}x");
    assert_eq!(click(&t, (0, 3), Mode::Word), "a日本b");
}

#[test]
fn block_selection_is_rectangular() {
    let t = term(10, 3, "abcdef\r\nghijkl\r\nmnopqr");
    assert_eq!(sel_text(&t, (0, 1), (2, 3), Mode::Block), "bcd\nhij\nnop");
}

#[test]
fn selection_reaches_into_scrollback() {
    // 3 rows: lines 0..=4 printed, so lines 0..2 are in the scrollback.
    let t = term(10, 3, "l0\r\nl1\r\nl2\r\nl3\r\nl4");
    assert_eq!(t.lines_scrolled, 2);
    assert_eq!(sel_text(&t, (0, 0), (3, 1), Mode::Char), "l0\nl1\nl2\nl3");
    // Soft wraps survive the trip into the scrollback.
    let t = term(4, 2, "abcdefgh\r\nz\r\nq");
    assert_eq!(click(&t, (0, 0), Mode::Line), "abcdefgh");
}

#[test]
fn scrollback_rows_lay_out_wide_and_combining() {
    let r = Row::from_scrollback("日e\u{301}", false);
    assert_eq!(r.cells.len(), 3);
    assert!(r.cells[1].cont);
    assert_eq!(r.cells[2].text, "e\u{301}");
}

#[test]
fn missing_lines_give_none() {
    let t = term(10, 2, "x");
    let pr = ParserRows::new(&t);
    let rows = |l: i64| pr.row(l);
    let mut s = Selection::start(Pos::new(-5, 0), Mode::Char, &rows, WC);
    s.extend(Pos::new(0, 0));
    assert_eq!(s.text(&rows, WC), None);
}

#[test]
fn span_for_highlight() {
    let t = term(10, 3, "abc\r\ndef\r\nghi");
    let pr = ParserRows::new(&t);
    let rows = |l: i64| pr.row(l);
    let mut s = Selection::start(Pos::new(0, 1), Mode::Char, &rows, WC);
    s.extend(Pos::new(2, 1));
    let b = s.bounds(&rows, WC);
    assert_eq!(s.span(0, 10, b), Some((1, 9)));
    assert_eq!(s.span(1, 10, b), Some((0, 9)));
    assert_eq!(s.span(2, 10, b), Some((0, 1)));
    assert_eq!(s.span(3, 10, b), None);
}
