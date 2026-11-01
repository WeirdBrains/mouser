//! Styled scrollback lines. A scrollback line is its text, optionally followed
//! by `\0` and the style runs of its cells (colours, underline, SGR attributes),
//! so the scrollback keeps colours when lines scroll off and when they are
//! re-wrapped. Readers that want text ([`text`], `Scrollback::get_lines`) never
//! see the runs. OSC 8 links are not kept in the scrollback.
//!
//! Runs: `col,len,fg,bg,ul,underline,attrs;...` with colours `d` (default),
//! `iN` (indexed) or `rRRGGBB`. Columns are cell columns.

use crate::grid::{Cell, Color, Style, attr};

/// Separates the text from its style runs.
pub const SEP: char = '\0';

/// Bits that describe layout, not style (kept out of the runs).
const LAYOUT: u16 = attr::WIDE | attr::WIDE_CONT | attr::WRAPLINE | attr::WRAP_PAD;

fn style_of(c: &Cell) -> Style {
    Style { attrs: c.style.attrs & !LAYOUT, link: 0, ..c.style }
}

fn color(c: Color, out: &mut String) {
    use std::fmt::Write;
    match c {
        Color::Default => out.push('d'),
        Color::Indexed(i) => {
            let _ = write!(out, "i{i}");
        }
        Color::Rgb(r, g, b) => {
            let _ = write!(out, "r{r:02x}{g:02x}{b:02x}");
        }
    }
}

fn parse_color(s: &str) -> Option<Color> {
    match s.as_bytes().first()? {
        b'd' => Some(Color::Default),
        b'i' => s[1..].parse().ok().map(Color::Indexed),
        b'r' if s.len() == 7 => {
            let h = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
            Some(Color::Rgb(h(1)?, h(3)?, h(5)?))
        }
        _ => None,
    }
}

/// `text` with the style runs of `cells` (the row it came from) appended;
/// unchanged when every cell has the default style.
pub fn encode(text: String, cells: &[Cell]) -> String {
    use std::fmt::Write;
    let mut runs = String::new();
    let mut i = 0;
    while i < cells.len() {
        let s = style_of(&cells[i]);
        let mut j = i + 1;
        while j < cells.len() && style_of(&cells[j]) == s {
            j += 1;
        }
        if s != Style::default() {
            let _ = write!(runs, "{i},{}", j - i);
            for c in [s.fg, s.bg, s.ul_color] {
                runs.push(',');
                color(c, &mut runs);
            }
            let _ = write!(runs, ",{},{};", s.underline, s.attrs);
        }
        i = j;
    }
    if runs.is_empty() {
        return text;
    }
    let mut t = text;
    t.push(SEP);
    t.push_str(&runs);
    t
}

/// The text of a scrollback line.
pub fn text(line: &str) -> &str {
    line.split_once(SEP).map_or(line, |(t, _)| t)
}

/// A scrollback line's text without its runs (owned).
pub fn strip(line: String) -> String {
    match line.find(SEP) {
        Some(i) => {
            let mut l = line;
            l.truncate(i);
            l
        }
        None => line,
    }
}

/// Whether a line carries style runs.
pub fn is_styled(line: &str) -> bool {
    line.contains(SEP)
}

/// The style runs of a line: (first cell, cells, style). Bad runs are skipped.
pub fn runs(line: &str) -> Vec<(usize, usize, Style)> {
    let Some((_, r)) = line.split_once(SEP) else { return vec![] };
    r.split(';')
        .filter_map(|run| {
            let f: Vec<&str> = run.split(',').collect();
            if f.len() != 7 {
                return None;
            }
            Some((
                f[0].parse().ok()?,
                f[1].parse().ok()?,
                Style {
                    fg: parse_color(f[2])?,
                    bg: parse_color(f[3])?,
                    ul_color: parse_color(f[4])?,
                    underline: f[5].parse().ok()?,
                    attrs: f[6].parse::<u16>().ok()? & !LAYOUT,
                    link: 0,
                },
            ))
        })
        .collect()
}

/// Per-cell styles for the first `n` cells (default where no run covers a cell).
pub fn cell_styles(line: &str, n: usize) -> Vec<Style> {
    let mut v = vec![Style::default(); n];
    for (a, len, s) in runs(line) {
        for st in v.iter_mut().skip(a).take(len) {
            *st = s;
        }
    }
    v
}

/// A scrollback line as cells with their styles (layout bits from the text; the
/// wrap flag on the last cell when `wrapped`), for re-wrapping.
pub fn to_cells(line: &str, wrapped: bool) -> Vec<Cell> {
    let mut cells = crate::reflow::str_cells(text(line));
    let styles = cell_styles(line, cells.len());
    for (c, s) in cells.iter_mut().zip(styles) {
        let layout = c.style.attrs & LAYOUT;
        c.style = Style { attrs: s.attrs | layout, ..s };
    }
    if wrapped && let Some(l) = cells.last_mut() {
        l.style.attrs |= attr::WRAPLINE;
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_runs_and_text() {
        let mut cells = crate::reflow::str_cells("ab 漢x");
        cells[0].style.fg = Color::Indexed(1);
        cells[1].style.fg = Color::Indexed(1);
        cells[1].style.attrs |= attr::BOLD;
        cells[3].style.bg = Color::Rgb(1, 2, 255);
        cells[4].style.bg = Color::Rgb(1, 2, 255);
        cells[5].style.underline = 3;
        cells[5].style.ul_color = Color::Indexed(200);
        cells[5].style.link = 7;
        let l = encode("ab 漢x".into(), &cells);
        assert_eq!(text(&l), "ab 漢x");
        assert!(is_styled(&l));
        let back = to_cells(&l, false);
        assert_eq!(back.len(), cells.len());
        for (a, b) in back.iter().zip(&cells) {
            assert_eq!(a.ch, b.ch);
            assert_eq!(a.style, Style { link: 0, ..b.style });
        }
        assert_eq!(strip(l.clone()), "ab 漢x");
        // Unstyled rows stay plain text.
        assert_eq!(encode("plain".into(), &crate::reflow::str_cells("plain")), "plain");
        assert!(to_cells("plain", true).last().unwrap().style.attrs & attr::WRAPLINE != 0);
        // Garbage runs are ignored, not fatal.
        assert_eq!(runs("x\0nope;1,2,d,d,d,0,1;"), vec![(1, 2, Style { attrs: 1, ..Style::default() })]);
    }
}
