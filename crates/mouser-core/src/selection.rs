//! Mouse text selection over the screen and the scrollback.
//!
//! Positions use absolute line numbers (the same numbering as graphics
//! anchors: screen row `y` is line `lines_scrolled + y`), so a selection stays
//! on its text while the view scrolls or new output pushes lines into the
//! scrollback. The front end supplies rows through a closure.

use crate::grid::{Cell, attr, char_width};
use crate::kitty_graphics::PLACEHOLDER;
use crate::parser::TerminalParser;
use std::cell::RefCell;
use std::collections::HashMap;

/// Characters that count as part of a word besides letters and digits
/// (kitty's default `select_by_word_characters` plus `:`, so URLs select whole).
pub const DEFAULT_WORD_CHARS: &str = "@-./_~?&=%+#:";

/// One cell of a row as selection sees it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SelCell {
    /// Base character plus combining marks; empty for the right half of a wide char.
    pub text: String,
    /// Right half of a wide character.
    pub cont: bool,
}

/// One row of text with its soft-wrap flag.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Row {
    pub cells: Vec<SelCell>,
    /// The row continues on the next line (autowrap), so no newline between them.
    pub wrapped: bool,
}

impl Row {
    /// A live screen row.
    pub fn from_cells(cells: &[Cell]) -> Self {
        let wrapped = cells.last().is_some_and(|c| c.style.attrs & attr::WRAPLINE != 0);
        let cells = cells
            .iter()
            .map(|c| {
                if c.style.attrs & attr::WIDE_CONT != 0 {
                    return SelCell { text: String::new(), cont: true };
                }
                let base = if c.ch == PLACEHOLDER || c.ch == '\0' { ' ' } else { c.ch };
                let mut text = base.to_string();
                text.extend(c.comb.iter().filter(|&&m| m != '\0'));
                SelCell { text, cont: false }
            })
            .collect();
        Self { cells, wrapped }
    }

    /// A scrollback line (wide characters stored once, combining marks after
    /// their base), laid out in cells the way the screen had them.
    pub fn from_scrollback(line: &str, wrapped: bool) -> Self {
        let mut cells: Vec<SelCell> = Vec::new();
        for c in line.chars() {
            let w = char_width(c);
            if w == 0
                && let Some(base) = cells.iter_mut().rev().find(|c| !c.cont)
            {
                base.text.push(c);
                continue;
            }
            cells.push(SelCell { text: c.to_string(), cont: false });
            if w == 2 {
                cells.push(SelCell { text: String::new(), cont: true });
            }
        }
        Self { cells, wrapped }
    }

    fn blank_at(&self, col: usize) -> bool {
        self.cells.get(col).is_none_or(|c| !c.cont && c.text.trim().is_empty())
    }
}

/// A point in the text: absolute line and column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: i64,
    pub col: usize,
}

impl Pos {
    pub fn new(line: i64, col: usize) -> Self {
        Self { line, col }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Character-wise (drag).
    Char,
    /// Whole words (double-click).
    Word,
    /// Whole logical lines, following soft wraps (triple-click).
    Line,
    /// Rectangle (Alt+drag).
    Block,
}

/// Row lookup: `None` when the line does not exist (evicted or never written).
pub type Rows<'a> = dyn Fn(i64) -> Option<Row> + 'a;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub mode: Mode,
    /// Range selected by the initial click (already expanded for Word/Line).
    anchor: (Pos, Pos),
    head: Pos,
    /// A Char selection only exists once the mouse moved off the anchor.
    moved: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Blank,
    Word,
    Other(char),
}

fn class(cell: Option<&SelCell>, word_chars: &str) -> Class {
    let Some(cell) = cell else { return Class::Blank };
    let c = cell.text.chars().next().unwrap_or(' ');
    if c.is_whitespace() {
        Class::Blank
    } else if c.is_alphanumeric() || word_chars.contains(c) || char_width(c) == 2 {
        Class::Word
    } else {
        Class::Other(c)
    }
}

impl Selection {
    /// Start a selection at `at` (click count 1/2/3 maps to Char/Word/Line).
    pub fn start(at: Pos, mode: Mode, rows: &Rows, word_chars: &str) -> Self {
        let at = snap_left(at, rows);
        let anchor = match mode {
            Mode::Char | Mode::Block => (at, at),
            Mode::Word => (word_start(at, rows, word_chars), word_end(at, rows, word_chars)),
            Mode::Line => (line_start(at, rows), line_end(at, rows)),
        };
        Self { mode, anchor, head: at, moved: matches!(mode, Mode::Word | Mode::Line) }
    }

    /// Move the free end to `to` (drag, or Shift+click to extend).
    pub fn extend(&mut self, to: Pos) {
        if to != self.anchor.0 {
            self.moved = true;
        }
        self.head = to;
    }

    pub fn is_empty(&self) -> bool {
        !self.moved
    }

    /// Inclusive start and end, in order.
    pub fn bounds(&self, rows: &Rows, word_chars: &str) -> (Pos, Pos) {
        let h = self.head;
        match self.mode {
            Mode::Block => {
                let (a, b) = (self.anchor.0, h);
                (Pos::new(a.line.min(b.line), a.col.min(b.col)), Pos::new(a.line.max(b.line), a.col.max(b.col)))
            }
            mode => {
                let (a0, a1) = self.anchor;
                if h < a0 {
                    let s = match mode {
                        Mode::Word => word_start(h, rows, word_chars),
                        Mode::Line => line_start(h, rows),
                        _ => snap_left(h, rows),
                    };
                    (s, snap_right(a1, rows))
                } else {
                    let e = match mode {
                        Mode::Word => word_end(h, rows, word_chars),
                        Mode::Line => line_end(h, rows),
                        _ => h,
                    };
                    (a0, snap_right(e.max(a1), rows))
                }
            }
        }
    }

    /// Selected columns `(first, last)` of `line`, for highlighting.
    pub fn span(&self, line: i64, cols: usize, bounds: (Pos, Pos)) -> Option<(usize, usize)> {
        let (s, e) = bounds;
        if self.is_empty() || line < s.line || line > e.line || cols == 0 {
            return None;
        }
        if self.mode == Mode::Block {
            return Some((s.col.min(cols - 1), e.col.min(cols - 1)));
        }
        let c0 = if line == s.line { s.col } else { 0 };
        let c1 = if line == e.line { e.col } else { cols - 1 };
        (c0 <= c1).then_some((c0.min(cols - 1), c1.min(cols - 1)))
    }

    /// The selected text: soft-wrapped rows are joined, hard line ends become
    /// `\n`, trailing blanks of each line are dropped. `None` when a selected
    /// line is gone (e.g. evicted from the scrollback).
    pub fn text(&self, rows: &Rows, word_chars: &str) -> Option<String> {
        if self.is_empty() {
            return Some(String::new());
        }
        let (s, e) = self.bounds(rows, word_chars);
        let mut out = String::new();
        let mut line_buf = String::new();
        for line in s.line..=e.line {
            let row = rows(line)?;
            let n = row.cells.len();
            let (c0, c1) = match self.mode {
                Mode::Block => (s.col, e.col),
                _ => (if line == s.line { s.col } else { 0 }, if line == e.line { e.col } else { usize::MAX }),
            };
            if n > 0 && c0 < n {
                for cell in &row.cells[c0..=c1.min(n - 1)] {
                    line_buf.push_str(&cell.text);
                }
            }
            let joined = self.mode != Mode::Block && line < e.line && row.wrapped && c1 >= n.saturating_sub(1);
            if !joined {
                out.push_str(line_buf.trim_end());
                line_buf.clear();
                if line < e.line {
                    out.push('\n');
                }
            }
        }
        out.push_str(line_buf.trim_end());
        Some(out)
    }
}

/// A position on the right half of a wide char moves to its left half.
fn snap_left(p: Pos, rows: &Rows) -> Pos {
    match rows(p.line) {
        Some(r) if p.col > 0 && r.cells.get(p.col).is_some_and(|c| c.cont) => Pos::new(p.line, p.col - 1),
        _ => p,
    }
}

/// An end on the left half of a wide char takes its right half too.
fn snap_right(p: Pos, rows: &Rows) -> Pos {
    match rows(p.line) {
        Some(r) if r.cells.get(p.col + 1).is_some_and(|c| c.cont) => Pos::new(p.line, p.col + 1),
        _ => p,
    }
}

fn prev(p: Pos, rows: &Rows) -> Option<Pos> {
    if p.col > 0 {
        return Some(Pos::new(p.line, p.col - 1));
    }
    let r = rows(p.line - 1)?;
    (r.wrapped && !r.cells.is_empty()).then(|| Pos::new(p.line - 1, r.cells.len() - 1))
}

fn next(p: Pos, rows: &Rows) -> Option<Pos> {
    let r = rows(p.line)?;
    if p.col + 1 < r.cells.len() {
        return Some(Pos::new(p.line, p.col + 1));
    }
    (r.wrapped && rows(p.line + 1).is_some()).then(|| Pos::new(p.line + 1, 0))
}

fn class_at(p: Pos, rows: &Rows, wc: &str) -> Class {
    let r = rows(p.line);
    let r = r.as_ref();
    let mut col = p.col;
    // The right half of a wide char belongs to its left half.
    if col > 0 && r.and_then(|r| r.cells.get(col)).is_some_and(|c| c.cont) {
        col -= 1;
    }
    class(r.and_then(|r| r.cells.get(col)), wc)
}

fn word_start(p: Pos, rows: &Rows, wc: &str) -> Pos {
    let k = class_at(p, rows, wc);
    let mut cur = p;
    // Blanks and punctuation: only that run on this row; words cross soft wraps.
    while let Some(q) = prev(cur, rows) {
        if (k != Class::Word && q.line != p.line) || class_at(q, rows, wc) != k {
            break;
        }
        cur = q;
    }
    snap_left(cur, rows)
}

fn word_end(p: Pos, rows: &Rows, wc: &str) -> Pos {
    let k = class_at(p, rows, wc);
    let mut cur = p;
    while let Some(q) = next(cur, rows) {
        if (k != Class::Word && q.line != p.line) || class_at(q, rows, wc) != k {
            break;
        }
        cur = q;
    }
    // A blank run ending a row (the empty rest of a line) stays a blank run.
    if k == Class::Blank && rows(cur.line).is_some_and(|r| r.blank_at(cur.col)) {
        return cur;
    }
    snap_right(cur, rows)
}

fn line_start(p: Pos, rows: &Rows) -> Pos {
    let mut line = p.line;
    while rows(line - 1).is_some_and(|r| r.wrapped) {
        line -= 1;
    }
    Pos::new(line, 0)
}

fn line_end(p: Pos, rows: &Rows) -> Pos {
    let mut line = p.line;
    while rows(line).is_some_and(|r| r.wrapped) && rows(line + 1).is_some() {
        line += 1;
    }
    let cols = rows(line).map(|r| r.cells.len()).unwrap_or(1);
    Pos::new(line, cols.saturating_sub(1))
}

/// Row lookup over a parser's screen and scrollback. Scrollback lines are
/// fetched in batches and cached, since each fetch may decompress a chunk.
pub struct ParserRows<'a, W: std::io::Write> {
    p: &'a TerminalParser<W>,
    cache: RefCell<HashMap<i64, Option<Row>>>,
}

const BATCH: i64 = 64;

impl<'a, W: std::io::Write> ParserRows<'a, W> {
    pub fn new(p: &'a TerminalParser<W>) -> Self {
        Self { p, cache: RefCell::new(HashMap::new()) }
    }

    /// First line still stored (oldest scrollback line).
    pub fn first_line(&self) -> i64 {
        self.p.lines_scrolled - self.p.scrollback.total_lines() as i64
    }

    pub fn row(&self, line: i64) -> Option<Row> {
        let top = self.p.lines_scrolled;
        let n = self.p.screen.size.rows as i64;
        if line >= top {
            return (line < top + n).then(|| Row::from_cells(self.p.screen.row((line - top) as usize)));
        }
        let first = self.first_line();
        if line < first {
            return None;
        }
        if let Some(r) = self.cache.borrow().get(&line) {
            return r.clone();
        }
        let start = (line - BATCH / 2).max(first);
        let end = (start + BATCH).min(top);
        let lines = self.p.scrollback.get_lines((start - first) as usize, (end - start) as usize);
        let mut cache = self.cache.borrow_mut();
        for (k, l) in lines.iter().enumerate() {
            let i = start + k as i64;
            let wrapped = self.p.scrollback.is_wrapped((i - first) as usize);
            cache.insert(i, Some(Row::from_scrollback(l, wrapped)));
        }
        cache.get(&line).cloned().flatten()
    }
}
