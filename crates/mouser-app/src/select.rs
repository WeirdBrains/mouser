//! Mouse selection for a tab: turns renderer select events into a
//! [`Selection`], paints it into the view and copies it.

use crate::tabs::Parser;
use mouser_core::selection::{DEFAULT_WORD_CHARS, Mode, ParserRows, Pos, Row, Selection};
use mouser_core::themes::ThemeColors;
use mouser_render::frame::paint;
use mouser_render::{CellPaint, SelectEvent};
use std::sync::OnceLock;

static WORD_CHARS: OnceLock<String> = OnceLock::new();

/// Characters besides letters and digits that double-click treats as part of a word.
pub fn set_word_chars(s: Option<String>) {
    let _ = WORD_CHARS.set(s.unwrap_or_else(|| DEFAULT_WORD_CHARS.to_string()));
}

pub fn word_chars() -> &'static str {
    WORD_CHARS.get().map(String::as_str).unwrap_or(DEFAULT_WORD_CHARS)
}

/// A tab's selection and the text it covered when last updated, so a change
/// of that text (new output over it) drops the selection.
#[derive(Debug, Clone)]
pub struct SelState {
    pub sel: Selection,
    pub text: String,
}

/// Lines the view is scrolled up, as the renderer's fetch clamps it.
fn back(p: &Parser, scrolled_back: usize) -> usize {
    if p.screen.in_alt_screen() { 0 } else { scrolled_back.min(p.scrollback.total_lines()) }
}

/// Absolute line shown on view row `row`.
pub fn view_line(p: &Parser, scrolled_back: usize, row: usize) -> i64 {
    p.lines_scrolled - back(p, scrolled_back) as i64 + row as i64
}

/// Apply one event. Returns the selected text on release (for PRIMARY).
pub fn handle(p: &Parser, state: &mut Option<SelState>, ev: SelectEvent) -> Option<String> {
    let pr = ParserRows::new(p);
    let rows = |l: i64| pr.row(l);
    let wc = word_chars();
    match ev {
        SelectEvent::Press { col, row, scrolled_back, clicks, extend, block } => {
            let at = Pos::new(view_line(p, scrolled_back, row), col);
            let reporting = p.modes.mouse != mouser_core::parser::MouseMode::Off;
            match state {
                Some(s) if extend && !reporting => s.sel.extend(at),
                _ => {
                    let mode = match clicks {
                        2 => Mode::Word,
                        3 => Mode::Line,
                        _ if block => Mode::Block,
                        _ => Mode::Char,
                    };
                    *state = Some(SelState { sel: Selection::start(at, mode, &rows, wc), text: String::new() });
                }
            }
        }
        SelectEvent::Drag { col, row, scrolled_back } => {
            if let Some(s) = state {
                s.sel.extend(Pos::new(view_line(p, scrolled_back, row), col));
            }
        }
        SelectEvent::Release => {
            let s = state.as_ref()?;
            return (!s.sel.is_empty()).then(|| s.text.clone()).filter(|t| !t.is_empty());
        }
        SelectEvent::Clear => {
            *state = None;
            return None;
        }
    }
    if let Some(s) = state {
        match s.sel.text(&rows, wc) {
            Some(t) => s.text = t,
            None => *state = None,
        }
    }
    None
}

/// After new output: drop the selection if the text under it changed.
/// Returns true when it was dropped.
pub fn refresh(p: &Parser, state: &mut Option<SelState>) -> bool {
    let Some(s) = state else { return false };
    if s.sel.is_empty() {
        return false;
    }
    let pr = ParserRows::new(p);
    let rows = |l: i64| pr.row(l);
    if s.sel.text(&rows, word_chars()).as_deref() == Some(s.text.as_str()) {
        return false;
    }
    *state = None;
    true
}

/// The selected text, if anything is selected.
pub fn text(state: &Option<SelState>) -> Option<String> {
    state.as_ref().filter(|s| !s.sel.is_empty()).map(|s| s.text.clone()).filter(|t| !t.is_empty())
}

/// A scrollback line laid out in cells like the screen (`'\0'` for the right
/// half of a wide char, combining marks in the paint), so wide text and the
/// selection line up with the columns.
pub fn scrollback_view(line: &str, fg: u32) -> (String, Vec<CellPaint>) {
    let row = Row::from_scrollback(line, false);
    let mut s = String::with_capacity(row.cells.len());
    let mut v = Vec::with_capacity(row.cells.len());
    for (i, c) in row.cells.iter().enumerate() {
        let mut p = CellPaint::plain(fg);
        if c.cont {
            s.push('\0');
            p.flags |= paint::NO_GLYPH;
        } else {
            let mut ch = c.text.chars();
            s.push(ch.next().unwrap_or(' '));
            for (slot, m) in p.comb.iter_mut().zip(ch) {
                *slot = m;
            }
            if row.cells.get(i + 1).is_some_and(|n| n.cont) {
                p.flags |= paint::WIDE;
            }
        }
        v.push(p);
    }
    (s, v)
}

/// Paint the selection into the view rows (`lines`/`paint` as fetched for
/// `scrolled_back`), padding rows that are selected past their text.
pub fn decorate(
    p: &Parser,
    state: &Option<SelState>,
    scrolled_back: usize,
    lines: &mut [String],
    paints: &mut [Vec<CellPaint>],
    theme: &ThemeColors,
) {
    let Some(s) = state else { return };
    if s.sel.is_empty() {
        return;
    }
    let cols = p.screen.size.cols;
    let pr = ParserRows::new(p);
    let rows = |l: i64| pr.row(l);
    let bounds = s.sel.bounds(&rows, word_chars());
    for (r, (line, pt)) in lines.iter_mut().zip(paints.iter_mut()).enumerate() {
        let Some((c0, c1)) = s.sel.span(view_line(p, scrolled_back, r), cols, bounds) else { continue };
        let n = line.chars().count();
        if n <= c1 {
            line.extend(std::iter::repeat_n(' ', c1 + 1 - n));
        }
        while pt.len() <= c1 {
            pt.push(CellPaint::plain(theme.foreground));
        }
        for cell in &mut pt[c0..=c1] {
            match theme.selection_bg {
                Some(bg) => {
                    cell.bg = Some(bg);
                    if let Some(fg) = theme.selection_fg {
                        cell.fg = fg;
                    }
                }
                None => {
                    let bg = cell.bg.unwrap_or(theme.background);
                    cell.bg = Some(cell.fg);
                    cell.fg = bg;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mouser_core::scrollback::{Scrollback, ScrollbackConfig};
    use mouser_core::types::Size;
    use std::sync::{Arc, Mutex};

    fn parser(cols: usize, rows: usize, input: &str) -> Parser {
        let sb = Arc::new(Scrollback::new(ScrollbackConfig::default()));
        let mut p = Parser::new(Size { cols, rows }, sb, crate::SharedWriter(Arc::new(Mutex::new(None))));
        p.feed(input.as_bytes());
        p
    }

    fn press(col: usize, row: usize, clicks: u8) -> SelectEvent {
        SelectEvent::Press { col, row, scrolled_back: 0, clicks, extend: false, block: false }
    }

    fn theme() -> ThemeColors {
        mouser_core::themes::builtins().remove(0).1.colors()
    }

    #[test]
    fn press_drag_release_and_clear() {
        let p = parser(20, 3, "hello world\r\nnext");
        let mut st = None;
        assert_eq!(handle(&p, &mut st, press(0, 0, 1)), None);
        assert_eq!(text(&st), None, "a click alone selects nothing");
        handle(&p, &mut st, SelectEvent::Drag { col: 4, row: 0, scrolled_back: 0 });
        assert_eq!(handle(&p, &mut st, SelectEvent::Release).as_deref(), Some("hello"));
        // Shift+click extends the existing selection.
        handle(&p, &mut st, SelectEvent::Press { col: 3, row: 1, scrolled_back: 0, clicks: 1, extend: true, block: false });
        assert_eq!(text(&st).as_deref(), Some("hello world\nnext"));
        handle(&p, &mut st, SelectEvent::Clear);
        assert!(st.is_none());
        // Double and triple click.
        handle(&p, &mut st, press(7, 0, 2));
        assert_eq!(text(&st).as_deref(), Some("world"));
        handle(&p, &mut st, press(7, 0, 3));
        assert_eq!(text(&st).as_deref(), Some("hello world"));
    }

    #[test]
    fn shift_click_starts_fresh_when_the_app_reports_the_mouse() {
        let mut p = parser(20, 3, "hello world");
        let mut st = None;
        handle(&p, &mut st, press(6, 0, 2));
        p.feed(b"\x1b[?1000h");
        handle(&p, &mut st, SelectEvent::Press { col: 0, row: 0, scrolled_back: 0, clicks: 1, extend: true, block: false });
        assert_eq!(text(&st), None);
    }

    #[test]
    fn scrolled_view_rows_map_to_scrollback_lines() {
        let p = parser(10, 2, "a0\r\na1\r\na2\r\na3");
        assert_eq!(view_line(&p, 0, 0), 2);
        assert_eq!(view_line(&p, 2, 0), 0);
        assert_eq!(view_line(&p, 99, 0), 0, "clamped like the view");
        let mut st = None;
        handle(&p, &mut st, SelectEvent::Press { col: 0, row: 0, scrolled_back: 2, clicks: 1, extend: false, block: false });
        handle(&p, &mut st, SelectEvent::Drag { col: 1, row: 1, scrolled_back: 0 });
        assert_eq!(text(&st).as_deref(), Some("a0\na1\na2\na3"));
    }

    #[test]
    fn new_output_over_the_selection_clears_it() {
        let mut p = parser(20, 3, "keep this");
        let mut st = None;
        handle(&p, &mut st, press(0, 0, 3));
        p.feed(b"\r\nmore");
        assert!(!refresh(&p, &mut st), "output elsewhere keeps it");
        p.feed(b"\x1b[1;1Hxx");
        assert!(refresh(&p, &mut st));
        assert!(st.is_none());
    }

    #[test]
    fn decorate_inverts_or_uses_theme_colours() {
        let mut p = parser(10, 2, "abc");
        let mut t = theme();
        crate::term_view::sync_theme(&mut p, &t);
        let mut st = None;
        handle(&p, &mut st, press(1, 0, 1));
        handle(&p, &mut st, SelectEvent::Drag { col: 1, row: 1, scrolled_back: 0 });
        let (mut lines, mut paint) = (p.screen.visible_lines(), crate::term_view::screen_paint(&p));
        decorate(&p, &st, 0, &mut lines, &mut paint, &t);
        assert_eq!(paint[0][0].bg, None);
        assert_eq!((paint[0][1].fg, paint[0][1].bg), (t.background, Some(t.foreground)));
        assert_eq!(paint[1][1].bg, Some(t.foreground));
        assert_eq!(paint[1][2].bg, None);
        t.selection_bg = Some(0x123456);
        t.selection_fg = Some(0xabcdef);
        let (mut lines, mut paint) = (p.screen.visible_lines(), crate::term_view::screen_paint(&p));
        decorate(&p, &st, 0, &mut lines, &mut paint, &t);
        assert_eq!((paint[0][2].fg, paint[0][2].bg), (0xabcdef, Some(0x123456)));
    }

    #[test]
    fn scrollback_lines_keep_their_columns() {
        let (s, p) = scrollback_view("a日e\u{301}", 7);
        assert_eq!(s, "a日\0e");
        assert_ne!(p[1].flags & paint::WIDE, 0);
        assert_ne!(p[2].flags & paint::NO_GLYPH, 0);
        assert_eq!(p[3].comb[0], '\u{301}');
    }
}
