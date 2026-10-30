use crate::types::{Point, Size};
use palette::Srgb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl From<Srgb<u8>> for Rgb {
    fn from(v: Srgb<u8>) -> Self {
        Self { r: v.red, g: v.green, b: v.blue }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self { fg: Rgb { r: 220, g: 220, b: 220 }, bg: Rgb { r: 16, g: 16, b: 16 }, bold: false, italic: false, underline: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Self { ch: ' ', style: Style::default() }
    }
}

#[derive(Debug, Clone)]
struct SavedScreen {
    cells: Vec<Cell>,
    cursor: Point,
}

/// The visible terminal grid with xterm-style cursor semantics.
///
/// Autowrap uses a pending-wrap flag (the cursor stays on the last column after
/// printing there, and the wrap happens on the next printable character), which
/// is what shells such as zsh rely on for `PROMPT_SP`.
#[derive(Debug)]
pub struct Screen {
    pub size: Size,
    pub cursor: Point,
    pub cells: Vec<Cell>,
    pub cursor_visible: bool,
    pending_wrap: bool,
    saved_cursor: Point,
    scroll_top: usize,
    scroll_bottom: usize,
    alt: Option<SavedScreen>,
    scrolled_off: Vec<String>,
}

impl Screen {
    pub fn new(size: Size) -> Self {
        let cells = vec![Cell::default(); size.cols * size.rows];
        Self {
            size,
            cursor: Point { x: 0, y: 0 },
            cells,
            cursor_visible: true,
            pending_wrap: false,
            saved_cursor: Point { x: 0, y: 0 },
            scroll_top: 0,
            scroll_bottom: size.rows.saturating_sub(1),
            alt: None,
            scrolled_off: Vec::new(),
        }
    }

    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.size.cols + x
    }

    fn row_text(&self, y: usize) -> String {
        let s: String = (0..self.size.cols).map(|x| self.cells[self.idx(x, y)].ch).collect();
        s.trim_end().to_string()
    }

    /// Lines that scrolled off the top of the main screen since the last call.
    pub fn take_scrolled_off(&mut self) -> Vec<String> {
        std::mem::take(&mut self.scrolled_off)
    }

    pub fn clear(&mut self) {
        self.cells.fill(Cell::default());
        self.cursor = Point { x: 0, y: 0 };
        self.pending_wrap = false;
    }

    pub fn put_char(&mut self, ch: char, style: Style) {
        if self.size.cols == 0 || self.size.rows == 0 {
            return;
        }
        if self.pending_wrap {
            self.pending_wrap = false;
            self.cursor.x = 0;
            self.linefeed();
        }
        let i = self.idx(self.cursor.x, self.cursor.y);
        self.cells[i] = Cell { ch, style };
        if self.cursor.x + 1 < self.size.cols {
            self.cursor.x += 1;
        } else {
            self.pending_wrap = true;
        }
    }

    /// BS: move left one column (non-destructive, like xterm).
    pub fn backspace(&mut self) {
        self.pending_wrap = false;
        self.cursor.x = self.cursor.x.saturating_sub(1);
    }

    pub fn tab(&mut self) {
        let next = (self.cursor.x / 8 + 1) * 8;
        self.cursor.x = next.min(self.size.cols.saturating_sub(1));
    }

    pub fn carriage_return(&mut self) {
        self.pending_wrap = false;
        self.cursor.x = 0;
    }

    pub fn linefeed(&mut self) {
        self.pending_wrap = false;
        if self.cursor.y == self.scroll_bottom {
            self.scroll_region_up(1, true);
        } else if self.cursor.y + 1 < self.size.rows {
            self.cursor.y += 1;
        }
    }

    /// ESC M: move up, scrolling the region down at the top margin.
    pub fn reverse_index(&mut self) {
        self.pending_wrap = false;
        if self.cursor.y == self.scroll_top {
            self.scroll_region_down(1);
        } else {
            self.cursor.y = self.cursor.y.saturating_sub(1);
        }
    }

    /// Scroll the whole screen up (used by tests and callers that want a full scroll).
    pub fn scroll_up(&mut self, lines: usize) {
        let (t, b) = (self.scroll_top, self.scroll_bottom);
        self.scroll_top = 0;
        self.scroll_bottom = self.size.rows.saturating_sub(1);
        self.scroll_region_up(lines, true);
        self.scroll_top = t;
        self.scroll_bottom = b;
        self.cursor.y = self.size.rows.saturating_sub(1);
    }

    fn scroll_region_up(&mut self, lines: usize, record: bool) {
        let cols = self.size.cols;
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let height = bottom + 1 - top;
        let n = lines.min(height);
        if n == 0 {
            return;
        }
        if record && top == 0 && self.alt.is_none() {
            for y in 0..n {
                let line = self.row_text(y);
                self.scrolled_off.push(line);
            }
        }
        self.cells.copy_within((top + n) * cols..(bottom + 1) * cols, top * cols);
        self.cells[(bottom + 1 - n) * cols..(bottom + 1) * cols].fill(Cell::default());
    }

    fn scroll_region_down(&mut self, lines: usize) {
        let cols = self.size.cols;
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let height = bottom + 1 - top;
        let n = lines.min(height);
        if n == 0 {
            return;
        }
        self.cells.copy_within(top * cols..(bottom + 1 - n) * cols, (top + n) * cols);
        self.cells[top * cols..(top + n) * cols].fill(Cell::default());
    }

    pub fn move_to(&mut self, x: usize, y: usize) {
        self.pending_wrap = false;
        self.cursor.x = x.min(self.size.cols.saturating_sub(1));
        self.cursor.y = y.min(self.size.rows.saturating_sub(1));
    }

    pub fn move_by(&mut self, dx: isize, dy: isize) {
        let x = (self.cursor.x as isize + dx).max(0) as usize;
        let y = (self.cursor.y as isize + dy).max(0) as usize;
        self.move_to(x, y);
    }

    /// ED: 0 = cursor to end, 1 = start to cursor, 2/3 = whole screen.
    pub fn erase_display(&mut self, mode: u16) {
        let i = self.idx(self.cursor.x, self.cursor.y);
        match mode {
            0 => self.cells[i..].fill(Cell::default()),
            1 => self.cells[..=i].fill(Cell::default()),
            _ => self.cells.fill(Cell::default()),
        }
    }

    /// EL: 0 = cursor to end of line, 1 = start to cursor, 2 = whole line.
    pub fn erase_line(&mut self, mode: u16) {
        let row = self.idx(0, self.cursor.y);
        let i = row + self.cursor.x;
        let end = row + self.size.cols;
        match mode {
            0 => self.cells[i..end].fill(Cell::default()),
            1 => self.cells[row..=i].fill(Cell::default()),
            _ => self.cells[row..end].fill(Cell::default()),
        }
    }

    /// ECH: blank `n` cells from the cursor without moving it.
    pub fn erase_chars(&mut self, n: usize) {
        let row = self.idx(0, self.cursor.y);
        let i = row + self.cursor.x;
        let end = (i + n).min(row + self.size.cols);
        self.cells[i..end].fill(Cell::default());
    }

    pub fn insert_chars(&mut self, n: usize) {
        let row = self.idx(0, self.cursor.y);
        let i = row + self.cursor.x;
        let end = row + self.size.cols;
        let n = n.min(end - i);
        self.cells.copy_within(i..end - n, i + n);
        self.cells[i..i + n].fill(Cell::default());
    }

    pub fn delete_chars(&mut self, n: usize) {
        let row = self.idx(0, self.cursor.y);
        let i = row + self.cursor.x;
        let end = row + self.size.cols;
        let n = n.min(end - i);
        self.cells.copy_within(i + n..end, i);
        self.cells[end - n..end].fill(Cell::default());
    }

    pub fn insert_lines(&mut self, n: usize) {
        if self.cursor.y < self.scroll_top || self.cursor.y > self.scroll_bottom {
            return;
        }
        let top = self.scroll_top;
        self.scroll_top = self.cursor.y;
        self.scroll_region_down(n);
        self.scroll_top = top;
        self.cursor.x = 0;
    }

    pub fn delete_lines(&mut self, n: usize) {
        if self.cursor.y < self.scroll_top || self.cursor.y > self.scroll_bottom {
            return;
        }
        let top = self.scroll_top;
        self.scroll_top = self.cursor.y;
        self.scroll_region_up(n, false);
        self.scroll_top = top;
        self.cursor.x = 0;
    }

    pub fn scroll_up_region(&mut self, n: usize) {
        self.scroll_region_up(n, true);
    }

    pub fn scroll_down_region(&mut self, n: usize) {
        self.scroll_region_down(n);
    }

    /// DECSTBM, 0-based inclusive rows. Moves the cursor home.
    pub fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        let last = self.size.rows.saturating_sub(1);
        let bottom = bottom.min(last);
        if top < bottom {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
        } else {
            self.scroll_top = 0;
            self.scroll_bottom = last;
        }
        self.move_to(0, 0);
    }

    pub fn save_cursor(&mut self) {
        self.saved_cursor = self.cursor;
    }

    pub fn restore_cursor(&mut self) {
        let p = self.saved_cursor;
        self.move_to(p.x, p.y);
    }

    pub fn enter_alt_screen(&mut self) {
        if self.alt.is_none() {
            self.alt = Some(SavedScreen { cells: self.cells.clone(), cursor: self.cursor });
            self.cells.fill(Cell::default());
        }
    }

    pub fn leave_alt_screen(&mut self) {
        if let Some(saved) = self.alt.take() {
            if saved.cells.len() == self.cells.len() {
                self.cells = saved.cells;
            }
            let p = saved.cursor;
            self.move_to(p.x, p.y);
        }
    }

    pub fn in_alt_screen(&self) -> bool {
        self.alt.is_some()
    }

    pub fn resize(&mut self, size: Size) {
        if size == self.size {
            return;
        }
        // Keep the cursor row on screen: push rows above it into scrollback.
        let mut first = 0;
        if self.cursor.y >= size.rows && size.rows > 0 {
            first = self.cursor.y + 1 - size.rows;
            if self.alt.is_none() {
                for y in 0..first {
                    let line = self.row_text(y);
                    self.scrolled_off.push(line);
                }
            }
        }
        let mut new_cells = vec![Cell::default(); size.cols * size.rows];
        let min_cols = size.cols.min(self.size.cols);
        for y in first..self.size.rows {
            let ny = y - first;
            if ny >= size.rows {
                break;
            }
            for x in 0..min_cols {
                new_cells[ny * size.cols + x] = self.cells[self.idx(x, y)];
            }
        }
        self.cells = new_cells;
        if let Some(alt) = &mut self.alt {
            alt.cells = vec![Cell::default(); size.cols * size.rows];
        }
        self.size = size;
        self.scroll_top = 0;
        self.scroll_bottom = size.rows.saturating_sub(1);
        self.pending_wrap = false;
        self.cursor.y = (self.cursor.y - first).min(size.rows.saturating_sub(1));
        self.cursor.x = self.cursor.x.min(size.cols.saturating_sub(1));
    }

    pub fn visible_lines(&self) -> Vec<String> {
        (0..self.size.rows).map(|y| (0..self.size.cols).map(|x| self.cells[self.idx(x, y)].ch).collect()).collect()
    }
}
