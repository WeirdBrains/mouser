//! Re-wrapping soft-wrapped lines for a new width (resize).
//!
//! Physical rows (scrollback lines as text, screen rows as cells) are joined into
//! logical lines at their soft-wrap flags and split again at the new width. Hard
//! line breaks stay. Points (the cursor, image anchors) are tracked by their
//! logical line and column offset so they land on the same character.

use crate::grid::{Cell, attr, char_width};

/// One physical row. `Str` rows are scrollback lines with their soft-wrap flag;
/// `Cells` rows are wrapped when their last cell carries [`attr::WRAPLINE`].
#[derive(Debug, Clone, PartialEq)]
pub enum Phys {
    Str(String, bool),
    Cells(Vec<Cell>),
}

impl Phys {
    pub fn wrapped(&self) -> bool {
        match self {
            Phys::Str(_, w) => *w,
            Phys::Cells(c) => c.last().is_some_and(|c| c.style.attrs & attr::WRAPLINE != 0),
        }
    }

    /// Scrollback text: wide characters once, padding skipped, trailing blanks
    /// trimmed unless the row is wrapped.
    pub fn into_string(self) -> (String, bool) {
        match self {
            Phys::Str(s, w) => (s, w),
            Phys::Cells(c) => {
                let w = c.last().is_some_and(|c| c.style.attrs & attr::WRAPLINE != 0);
                (crate::sbstyle::encode(cells_string(&c, w), &c), w)
            }
        }
    }

    /// A scrollback line (text, maybe with style runs) for re-wrapping: styled
    /// lines go through cells so their colours survive.
    pub fn from_scrollback(line: String, wrapped: bool) -> Self {
        if crate::sbstyle::is_styled(&line) { Phys::Cells(crate::sbstyle::to_cells(&line, wrapped)) } else { Phys::Str(line, wrapped) }
    }

    /// Screen cells `cols` wide (padded with blanks; the wrap flag on the last cell).
    pub fn into_cells(self, cols: usize) -> Vec<Cell> {
        let (mut c, w) = match self {
            Phys::Str(s, w) => (str_cells(&s), w),
            Phys::Cells(c) => {
                let w = c.last().is_some_and(|c| c.style.attrs & attr::WRAPLINE != 0);
                (c, w)
            }
        };
        if let Some(l) = c.last_mut() {
            l.style.attrs &= !attr::WRAPLINE;
        }
        let pad = Cell { style: crate::grid::Style { attrs: if w { attr::WRAP_PAD } else { 0 }, ..Default::default() }, ..Cell::default() };
        c.truncate(cols);
        c.resize(cols, pad);
        if w && cols > 0 {
            c[cols - 1].style.attrs |= attr::WRAPLINE;
        }
        c
    }
}

/// Text of cells: wide characters once, combining marks attached, wrap padding
/// skipped; trailing blanks trimmed unless `wrapped`.
pub fn cells_string(cells: &[Cell], wrapped: bool) -> String {
    let mut s = String::with_capacity(cells.len());
    for c in cells {
        if c.style.attrs & (attr::WIDE_CONT | attr::WRAP_PAD) != 0 {
            continue;
        }
        s.push(c.ch);
        s.extend(c.comb.iter().filter(|&&m| m != '\0'));
    }
    if wrapped { s } else { s.trim_end().to_string() }
}

/// Display width of a string in cells.
pub fn str_width(s: &str) -> usize {
    if s.is_ascii() { s.len() } else { s.chars().map(char_width).sum() }
}

/// Cells for plain text (default style).
pub fn str_cells(s: &str) -> Vec<Cell> {
    let mut v: Vec<Cell> = Vec::with_capacity(s.len());
    for ch in s.chars() {
        match char_width(ch) {
            0 => {
                if let Some(slot) =
                    v.iter_mut().rev().find(|c| c.style.attrs & attr::WIDE_CONT == 0).and_then(|c| c.comb.iter_mut().find(|m| **m == '\0'))
                {
                    *slot = ch;
                }
            }
            2 => {
                let mut c = Cell { ch, ..Cell::default() };
                c.style.attrs = attr::WIDE;
                v.push(c);
                let mut r = Cell::default();
                r.style.attrs = attr::WIDE_CONT;
                v.push(r);
            }
            _ => v.push(Cell { ch, ..Cell::default() }),
        }
    }
    v
}

#[derive(Debug)]
enum Line {
    Str(String),
    Cells(Vec<Cell>),
}

impl Line {
    fn width(&self) -> usize {
        match self {
            Line::Str(s) => str_width(s),
            Line::Cells(c) => c.len(),
        }
    }

    fn cells(&mut self) -> &mut Vec<Cell> {
        if let Line::Str(s) = self {
            *self = Line::Cells(str_cells(s));
        }
        match self {
            Line::Cells(c) => c,
            Line::Str(_) => unreachable!(),
        }
    }
}

fn is_blank(c: &Cell) -> bool {
    c.ch == ' ' && c.comb[0] == '\0' && c.style == crate::grid::Style::default()
}

/// Logical lines being re-wrapped, plus tracked points.
#[derive(Debug)]
pub struct Layout {
    lines: Vec<Line>,
    /// Per input row: logical line and the column offset where it starts.
    row_map: Vec<(usize, usize)>,
    /// Tracked points: logical line and column offset.
    pts: Vec<(usize, usize)>,
}

/// Where a point landed: row in the output and column. A point past the end of
/// its line's last row keeps its distance from the row start, so `col` can be
/// `cols` or more (the cursor clamps it to a pending wrap; an image anchor keeps
/// it as its requested column). `past_end` is `col >= cols`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Landed {
    pub row: usize,
    pub col: usize,
    pub past_end: bool,
}

impl Layout {
    pub fn build(rows: Vec<Phys>) -> Self {
        let mut lines = Vec::new();
        let mut row_map = Vec::with_capacity(rows.len());
        let mut cur: Option<Line> = None;
        let mut w = 0usize;
        let mut cont = false;
        for r in rows {
            if !cont && let Some(l) = cur.take() {
                lines.push(l);
                w = 0;
            }
            row_map.push((lines.len(), w));
            match r {
                Phys::Str(s, wr) => {
                    w += str_width(&s);
                    match &mut cur {
                        None => cur = Some(Line::Str(s)),
                        Some(Line::Str(t)) => t.push_str(&s),
                        Some(l @ Line::Cells(_)) => l.cells().extend(str_cells(&s)),
                    }
                    cont = wr;
                }
                Phys::Cells(mut c) => {
                    let wr = c.last().is_some_and(|c| c.style.attrs & attr::WRAPLINE != 0);
                    for x in c.iter_mut() {
                        x.style.attrs &= !attr::WRAPLINE;
                    }
                    c.retain(|x| x.style.attrs & attr::WRAP_PAD == 0);
                    if !wr {
                        while c.last().is_some_and(is_blank) {
                            c.pop();
                        }
                    }
                    w += c.len();
                    match &mut cur {
                        None => cur = Some(Line::Cells(c)),
                        Some(l) => l.cells().extend(c),
                    }
                    cont = wr;
                }
            }
        }
        if let Some(l) = cur {
            lines.push(l);
        }
        Self { lines, row_map, pts: Vec::new() }
    }

    /// Track input row `row`, column `col`. `extend` pads the line with blanks up to
    /// the point (the cursor may sit past the text). Returns the point's id.
    pub fn add_point(&mut self, row: usize, col: usize, extend: bool) -> Option<usize> {
        let &(line, start) = self.row_map.get(row)?;
        let off = start + col;
        if extend {
            let l = &mut self.lines[line];
            if l.width() < off {
                let c = l.cells();
                c.resize(off, Cell::default());
            }
        }
        self.pts.push((line, off));
        Some(self.pts.len() - 1)
    }

    /// Logical line and column offset of a point.
    pub fn point(&self, id: usize) -> (usize, usize) {
        self.pts[id]
    }

    /// Make room below line `line` for `delta` more rows (insert empty lines) or
    /// give back up to `-delta` empty lines that directly follow it (never a line
    /// another point is on, so images below and the cursor keep their rows).
    pub fn adjust_after(&mut self, line: usize, delta: i64) {
        if delta > 0 {
            let n = delta as usize;
            let at = (line + 1).min(self.lines.len());
            self.lines.splice(at..at, (0..n).map(|_| Line::Cells(Vec::new())));
            for p in self.pts.iter_mut().filter(|p| p.0 > line) {
                p.0 += n;
            }
        } else if delta < 0 {
            // Only empty lines nothing is anchored to (another image, the cursor).
            let pts = &self.pts;
            let empty =
                self.lines[line + 1..].iter().enumerate().take_while(|(i, l)| l.width() == 0 && !pts.iter().any(|p| p.0 == line + 1 + i)).count();
            let n = empty.min(delta.unsigned_abs() as usize);
            if n == 0 {
                return;
            }
            self.lines.drain(line + 1..line + 1 + n);
            for p in self.pts.iter_mut().filter(|p| p.0 > line) {
                p.0 = if p.0 <= line + n { line + 1 } else { p.0 - n };
                if p.0 == line + 1 && p.0 >= self.lines.len() {
                    p.0 = line;
                }
            }
        }
    }

    /// Split every line at `cols` columns. Returns the rows and where each point landed.
    pub fn split(self, cols: usize) -> (Vec<Phys>, Vec<Landed>) {
        let cols = cols.max(1);
        let mut by_line: Vec<(usize, usize, usize)> = self.pts.iter().enumerate().map(|(i, &(l, o))| (l, o, i)).collect();
        by_line.sort_unstable();
        let mut landed = vec![Landed { row: 0, col: 0, past_end: false }; self.pts.len()];
        let mut out = Vec::with_capacity(self.row_map.len());
        let mut k = 0;
        for (li, line) in self.lines.into_iter().enumerate() {
            let base = out.len();
            let starts = match line {
                Line::Str(s) => split_str(s, cols, &mut out),
                Line::Cells(c) => split_cells(c, cols, &mut out),
            };
            while k < by_line.len() && by_line[k].0 == li {
                let (_, off, id) = by_line[k];
                let r = starts.partition_point(|&s| s <= off).saturating_sub(1);
                let col = off - starts[r];
                landed[id] = Landed { row: base + r, col, past_end: col >= cols };
                k += 1;
            }
        }
        (out, landed)
    }
}

fn split_str(s: String, cols: usize, out: &mut Vec<Phys>) -> Vec<usize> {
    if str_width(&s) <= cols {
        out.push(Phys::Str(s, false));
        return vec![0];
    }
    let mut starts = vec![0];
    let (mut cur, mut w, mut off) = (String::new(), 0usize, 0usize);
    for ch in s.chars() {
        let cw = char_width(ch);
        if cw > 0 && w + cw > cols && w > 0 {
            out.push(Phys::Str(std::mem::take(&mut cur), true));
            starts.push(off);
            w = 0;
        }
        cur.push(ch);
        w += cw;
        off += cw;
    }
    out.push(Phys::Str(cur, false));
    starts
}

fn split_cells(c: Vec<Cell>, cols: usize, out: &mut Vec<Phys>) -> Vec<usize> {
    if c.len() <= cols {
        out.push(Phys::Cells(c));
        return vec![0];
    }
    let mut starts = vec![0];
    let mut row: Vec<Cell> = Vec::with_capacity(cols);
    let mut i = 0;
    while i < c.len() {
        let uw = if c[i].style.attrs & attr::WIDE != 0 && i + 1 < c.len() { 2 } else { 1 };
        if row.len() + uw > cols && !row.is_empty() {
            let mut pad = Cell::default();
            pad.style.attrs = attr::WRAP_PAD;
            row.resize(cols, pad);
            row[cols - 1].style.attrs |= attr::WRAPLINE;
            out.push(Phys::Cells(std::mem::replace(&mut row, Vec::with_capacity(cols))));
            starts.push(i);
        }
        row.extend_from_slice(&c[i..i + uw]);
        i += uw;
    }
    out.push(Phys::Cells(row));
    starts
}
