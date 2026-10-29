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

#[derive(Debug)]
pub struct Screen {
    pub size: Size,
    pub cursor: Point,
    pub cells: Vec<Cell>,
}

impl Screen {
    pub fn new(size: Size) -> Self {
        let cells = vec![Cell::default(); size.cols * size.rows];
        Self { size, cursor: Point { x: 0, y: 0 }, cells }
    }

    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.size.cols + x
    }

    pub fn clear(&mut self) {
        self.cells.fill(Cell::default());
        self.cursor = Point { x: 0, y: 0 };
    }

    pub fn put_char(&mut self, ch: char, style: Style) {
        if self.cursor.y >= self.size.rows || self.cursor.x >= self.size.cols {
            return;
        }
        let i = self.idx(self.cursor.x, self.cursor.y);
        self.cells[i] = Cell { ch, style };
        if self.cursor.x + 1 < self.size.cols {
            self.cursor.x += 1;
        }
    }

    pub fn backspace(&mut self) {
        if self.cursor.x > 0 {
            self.cursor.x -= 1;
            let i = self.idx(self.cursor.x, self.cursor.y);
            self.cells[i] = Cell::default();
        }
    }

    pub fn carriage_return(&mut self) {
        self.cursor.x = 0;
    }

    pub fn linefeed(&mut self) {
        if self.cursor.y + 1 < self.size.rows {
            self.cursor.y += 1;
        } else {
            self.scroll_up(1);
        }
    }

    pub fn scroll_up(&mut self, lines: usize) {
        let cols = self.size.cols;
        let rows = self.size.rows;
        let shift = lines.min(rows);
        if shift == 0 {
            return;
        }
        // Move rows up
        self.cells.copy_within(shift * cols..rows * cols, 0);
        // Clear the bottom lines
        for idx in (rows - shift) * cols..rows * cols {
            self.cells[idx] = Cell::default();
        }
        self.cursor.y = rows - 1;
    }

    pub fn resize(&mut self, size: Size) {
        if size == self.size {
            return;
        }
        let mut new_cells = vec![Cell::default(); size.cols * size.rows];
        let min_rows = size.rows.min(self.size.rows);
        let min_cols = size.cols.min(self.size.cols);
        for y in 0..min_rows {
            for x in 0..min_cols {
                let old = self.cells[self.idx(x, y)];
                new_cells[y * size.cols + x] = old;
            }
        }
        self.cells = new_cells;
        self.size = size;
        self.cursor.x = self.cursor.x.min(size.cols.saturating_sub(1));
        self.cursor.y = self.cursor.y.min(size.rows.saturating_sub(1));
    }

    pub fn visible_lines(&self) -> Vec<String> {
        let mut lines = Vec::with_capacity(self.size.rows);
        for y in 0..self.size.rows {
            let mut s = String::with_capacity(self.size.cols);
            for x in 0..self.size.cols {
                s.push(self.cells[self.idx(x, y)].ch);
            }
            lines.push(s);
        }
        lines
    }
}
