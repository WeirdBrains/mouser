use crate::types::{Point, Size};
use palette::Srgb;
use unicode_width::UnicodeWidthChar;

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

/// A cell colour as the application set it; resolved against the theme when drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    /// 0-15 ANSI (theme palette), 16-255 the xterm cube and gray ramp.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// SGR attribute bits.
pub mod attr {
    pub const BOLD: u16 = 1;
    pub const DIM: u16 = 2;
    pub const ITALIC: u16 = 4;
    pub const BLINK: u16 = 8;
    pub const INVERSE: u16 = 16;
    pub const HIDDEN: u16 = 32;
    pub const STRIKE: u16 = 64;
    pub const OVERLINE: u16 = 128;
    /// This cell holds a double-width character.
    pub const WIDE: u16 = 256;
    /// Right half of a double-width character (no glyph of its own).
    pub const WIDE_CONT: u16 = 512;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    /// SGR 58 underline colour (`Default` = same as the text).
    pub ul_color: Color,
    /// 0 none, 1 single, 2 double, 3 curly, 4 dotted, 5 dashed.
    pub underline: u8,
    pub attrs: u16,
    /// OSC 8 hyperlink id (0 = none).
    pub link: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    /// Up to three combining marks / ZWJ joiners attached to `ch` (`'\0'` = none);
    /// Kitty Unicode placeholders carry three diacritics.
    pub comb: [char; 3],
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Self { ch: ' ', comb: ['\0'; 3], style: Style::default() }
    }
}

impl Cell {
    fn blank(bg: Color) -> Self {
        Self { style: Style { bg, ..Style::default() }, ..Self::default() }
    }
}

/// Display width of `c` in cells: 0 (combining / zero-width), 1 or 2.
pub fn char_width(c: char) -> usize {
    if c == '\u{200d}' || c == '\u{fe0f}' || c == '\u{fe0e}' {
        return 0;
    }
    // Emoji modifiers and regional indicators are wide in modern terminals.
    if ('\u{1f1e6}'..='\u{1f1ff}').contains(&c) {
        return 2;
    }
    c.width().unwrap_or(0).min(2)
}

/// A block of rows moved by a scroll, recorded so attached objects (Fuzzball
/// placements) can follow their text. Rows are screen rows at the time of the scroll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollEvent {
    /// Lines already in `scrolled_off` when this scroll happened.
    pub pushed_before: usize,
    pub top: usize,
    pub bottom: usize,
    /// Rows moved: negative = up.
    pub delta: isize,
    /// The scrolled-out rows went to the scrollback (main screen, region at the top):
    /// rows in the region keep their absolute row, rows below it gain `n`.
    pub to_scrollback: bool,
}

#[derive(Debug, Clone)]
struct SavedScreen {
    cells: Vec<Cell>,
    cursor: Point,
}

/// DECSCUSR cursor shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
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
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,
    /// DECAWM (on by default).
    pub autowrap: bool,
    /// DECOM: cursor addressing relative to the scroll region.
    pub origin: bool,
    /// IRM: printing inserts instead of overwriting.
    pub insert: bool,
    /// Background used by erase operations (BCE).
    pub erase_bg: Color,
    pending_wrap: bool,
    saved_cursor: Point,
    saved_origin: bool,
    scroll_top: usize,
    scroll_bottom: usize,
    tabs: Vec<bool>,
    alt: Option<SavedScreen>,
    scrolled_off: Vec<String>,
    scroll_events: Vec<ScrollEvent>,
}

fn default_tabs(cols: usize) -> Vec<bool> {
    (0..cols).map(|x| x > 0 && x % 8 == 0).collect()
}

impl Screen {
    pub fn new(size: Size) -> Self {
        let cells = vec![Cell::default(); size.cols * size.rows];
        Self {
            size,
            cursor: Point { x: 0, y: 0 },
            cells,
            cursor_visible: true,
            cursor_shape: CursorShape::Block,
            cursor_blink: false,
            autowrap: true,
            origin: false,
            insert: false,
            erase_bg: Color::Default,
            pending_wrap: false,
            saved_cursor: Point { x: 0, y: 0 },
            saved_origin: false,
            scroll_top: 0,
            scroll_bottom: size.rows.saturating_sub(1),
            tabs: default_tabs(size.cols),
            alt: None,
            scrolled_off: Vec::new(),
            scroll_events: Vec::new(),
        }
    }

    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.size.cols + x
    }

    fn blank(&self) -> Cell {
        Cell::blank(self.erase_bg)
    }

    /// Text of one row: wide characters once, combining marks attached.
    pub fn row_string(&self, y: usize) -> String {
        let mut s = String::new();
        for c in self.row(y) {
            if c.style.attrs & attr::WIDE_CONT != 0 {
                continue;
            }
            s.push(c.ch);
            s.extend(c.comb.iter().filter(|&&m| m != '\0'));
        }
        s.trim_end().to_string()
    }

    pub fn row(&self, y: usize) -> &[Cell] {
        let i = self.idx(0, y);
        &self.cells[i..i + self.size.cols]
    }

    /// Lines that scrolled off the top of the main screen since the last call.
    /// Scrolls since the last call, oldest first.
    pub fn take_scroll_events(&mut self) -> Vec<ScrollEvent> {
        std::mem::take(&mut self.scroll_events)
    }

    pub fn take_scrolled_off(&mut self) -> Vec<String> {
        std::mem::take(&mut self.scrolled_off)
    }

    pub fn clear(&mut self) {
        self.cells.fill(Cell::default());
        self.cursor = Point { x: 0, y: 0 };
        self.pending_wrap = false;
        self.autowrap = true;
        self.origin = false;
        self.insert = false;
        self.cursor_visible = true;
        self.cursor_shape = CursorShape::Block;
        self.erase_bg = Color::Default;
        self.scroll_top = 0;
        self.scroll_bottom = self.size.rows.saturating_sub(1);
        self.tabs = default_tabs(self.size.cols);
    }

    /// Attach a zero-width character to the previous cell.
    fn combine(&mut self, ch: char) {
        let (mut x, y) = (self.cursor.x, self.cursor.y);
        if !self.pending_wrap {
            if x == 0 {
                return;
            }
            x -= 1;
        }
        let mut i = self.idx(x, y);
        if self.cells[i].style.attrs & attr::WIDE_CONT != 0 && x > 0 {
            i -= 1;
        }
        if let Some(slot) = self.cells[i].comb.iter_mut().find(|m| **m == '\0') {
            *slot = ch;
        }
    }

    pub fn put_char(&mut self, ch: char, style: Style) {
        if self.size.cols == 0 || self.size.rows == 0 {
            return;
        }
        let w = char_width(ch);
        if w == 0 {
            self.combine(ch);
            return;
        }
        if self.pending_wrap {
            self.pending_wrap = false;
            if self.autowrap {
                self.cursor.x = 0;
                self.linefeed();
            }
        }
        let cols = self.size.cols;
        if w == 2 && self.cursor.x + 1 >= cols {
            if cols < 2 {
                return;
            }
            if self.autowrap {
                let i = self.idx(self.cursor.x, self.cursor.y);
                self.cells[i] = self.blank();
                self.cursor.x = 0;
                self.linefeed();
            } else {
                self.cursor.x = cols - 2;
            }
        }
        if self.insert {
            self.insert_chars(w);
        }
        self.clear_wide_at(self.cursor.x);
        let i = self.idx(self.cursor.x, self.cursor.y);
        let mut st = style;
        st.attrs &= !(attr::WIDE | attr::WIDE_CONT);
        if w == 2 {
            self.clear_wide_at(self.cursor.x + 1);
            self.cells[i] = Cell { ch, comb: ['\0'; 3], style: Style { attrs: st.attrs | attr::WIDE, ..st } };
            self.cells[i + 1] = Cell { ch: ' ', comb: ['\0'; 3], style: Style { attrs: st.attrs | attr::WIDE_CONT, ..st } };
        } else {
            self.cells[i] = Cell { ch, comb: ['\0'; 3], style: st };
        }
        if self.cursor.x + w < cols {
            self.cursor.x += w;
        } else {
            self.cursor.x = cols - 1;
            self.pending_wrap = true;
        }
    }

    /// Overwriting half of a wide character blanks the other half.
    fn clear_wide_at(&mut self, x: usize) {
        if x >= self.size.cols {
            return;
        }
        let i = self.idx(x, self.cursor.y);
        let a = self.cells[i].style.attrs;
        if a & attr::WIDE_CONT != 0 && x > 0 {
            self.cells[i - 1] = self.blank();
        } else if a & attr::WIDE != 0 && x + 1 < self.size.cols {
            self.cells[i + 1] = self.blank();
        }
    }

    /// BS: move left one column (non-destructive, like xterm).
    pub fn backspace(&mut self) {
        self.pending_wrap = false;
        self.cursor.x = self.cursor.x.saturating_sub(1);
    }

    /// HT / CHT: forward to the `n`th next tab stop (or the last column).
    pub fn tab_forward(&mut self, n: usize) {
        let last = self.size.cols.saturating_sub(1);
        for _ in 0..n.max(1) {
            let mut x = self.cursor.x + 1;
            while x < last && !self.tabs.get(x).copied().unwrap_or(false) {
                x += 1;
            }
            self.cursor.x = x.min(last);
        }
    }

    pub fn tab(&mut self) {
        self.tab_forward(1);
    }

    /// CBT: back to the `n`th previous tab stop (or column 0).
    pub fn tab_backward(&mut self, n: usize) {
        self.pending_wrap = false;
        for _ in 0..n.max(1) {
            let mut x = self.cursor.x;
            while x > 0 {
                x -= 1;
                if self.tabs[x] {
                    break;
                }
            }
            self.cursor.x = x;
        }
    }

    /// HTS
    pub fn set_tab(&mut self) {
        let x = self.cursor.x;
        if let Some(t) = self.tabs.get_mut(x) {
            *t = true;
        }
    }

    /// TBC: 0 = at cursor, 3 = all.
    pub fn clear_tab(&mut self, mode: u16) {
        match mode {
            0 => {
                let x = self.cursor.x;
                if let Some(t) = self.tabs.get_mut(x) {
                    *t = false;
                }
            }
            3 => self.tabs.fill(false),
            _ => {}
        }
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
        let to_scrollback = record && top == 0 && self.alt.is_none();
        self.scroll_events.push(ScrollEvent { pushed_before: self.scrolled_off.len(), top, bottom, delta: -(n as isize), to_scrollback });
        if to_scrollback {
            for y in 0..n {
                let line = self.row_string(y);
                self.scrolled_off.push(line);
            }
        }
        let blank = self.blank();
        self.cells.copy_within((top + n) * cols..(bottom + 1) * cols, top * cols);
        self.cells[(bottom + 1 - n) * cols..(bottom + 1) * cols].fill(blank);
    }

    fn scroll_region_down(&mut self, lines: usize) {
        let cols = self.size.cols;
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let height = bottom + 1 - top;
        let n = lines.min(height);
        if n == 0 {
            return;
        }
        self.scroll_events.push(ScrollEvent { pushed_before: self.scrolled_off.len(), top, bottom, delta: n as isize, to_scrollback: false });
        let blank = self.blank();
        self.cells.copy_within(top * cols..(bottom + 1 - n) * cols, (top + n) * cols);
        self.cells[top * cols..(top + n) * cols].fill(blank);
    }

    /// Absolute move (0-based), clamped to the screen.
    pub fn move_to(&mut self, x: usize, y: usize) {
        self.pending_wrap = false;
        self.cursor.x = x.min(self.size.cols.saturating_sub(1));
        self.cursor.y = y.min(self.size.rows.saturating_sub(1));
    }

    /// CUP/HVP/VPA target: relative to the scroll region in origin mode.
    pub fn move_to_origin(&mut self, x: usize, y: usize) {
        if self.origin {
            let y = (self.scroll_top + y).min(self.scroll_bottom);
            self.move_to(x, y);
        } else {
            self.move_to(x, y);
        }
    }

    pub fn move_by(&mut self, dx: isize, dy: isize) {
        let x = (self.cursor.x as isize + dx).max(0) as usize;
        let mut y = (self.cursor.y as isize + dy).max(0) as usize;
        // CUU/CUD stop at the margins when starting inside them.
        if dy < 0 && self.cursor.y >= self.scroll_top {
            y = y.max(self.scroll_top);
        }
        if dy > 0 && self.cursor.y <= self.scroll_bottom {
            y = y.min(self.scroll_bottom);
        }
        self.move_to(x, y);
    }

    /// ED: 0 = cursor to end, 1 = start to cursor, 2/3 = whole screen.
    pub fn erase_display(&mut self, mode: u16) {
        let i = self.idx(self.cursor.x, self.cursor.y);
        let blank = self.blank();
        match mode {
            0 => self.cells[i..].fill(blank),
            1 => self.cells[..=i].fill(blank),
            _ => self.cells.fill(blank),
        }
    }

    /// EL: 0 = cursor to end of line, 1 = start to cursor, 2 = whole line.
    pub fn erase_line(&mut self, mode: u16) {
        let row = self.idx(0, self.cursor.y);
        let i = row + self.cursor.x;
        let end = row + self.size.cols;
        let blank = self.blank();
        match mode {
            0 => self.cells[i..end].fill(blank),
            1 => self.cells[row..=i].fill(blank),
            _ => self.cells[row..end].fill(blank),
        }
    }

    /// ECH: blank `n` cells from the cursor without moving it.
    pub fn erase_chars(&mut self, n: usize) {
        let row = self.idx(0, self.cursor.y);
        let i = row + self.cursor.x;
        let end = (i + n.max(1)).min(row + self.size.cols);
        let blank = self.blank();
        self.cells[i..end].fill(blank);
    }

    pub fn insert_chars(&mut self, n: usize) {
        let row = self.idx(0, self.cursor.y);
        let i = row + self.cursor.x;
        let end = row + self.size.cols;
        let n = n.min(end - i);
        let blank = self.blank();
        self.cells.copy_within(i..end - n, i + n);
        self.cells[i..i + n].fill(blank);
    }

    pub fn delete_chars(&mut self, n: usize) {
        let row = self.idx(0, self.cursor.y);
        let i = row + self.cursor.x;
        let end = row + self.size.cols;
        let n = n.min(end - i);
        let blank = self.blank();
        self.cells.copy_within(i + n..end, i);
        self.cells[end - n..end].fill(blank);
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
        self.move_to_origin(0, 0);
    }

    pub fn scroll_region(&self) -> (usize, usize) {
        (self.scroll_top, self.scroll_bottom)
    }

    pub fn save_cursor(&mut self) {
        self.saved_cursor = self.cursor;
        self.saved_origin = self.origin;
    }

    pub fn restore_cursor(&mut self) {
        let p = self.saved_cursor;
        self.origin = self.saved_origin;
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
                    let line = self.row_string(y);
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
            // A wide char cut in half at the new right edge becomes a blank.
            if min_cols > 0
                && new_cells[ny * size.cols + min_cols - 1].style.attrs & attr::WIDE != 0
                && min_cols < self.size.cols
                && size.cols == min_cols
            {
                new_cells[ny * size.cols + min_cols - 1] = Cell::default();
            }
        }
        self.cells = new_cells;
        if let Some(alt) = &mut self.alt {
            alt.cells = vec![Cell::default(); size.cols * size.rows];
        }
        let mut tabs = default_tabs(size.cols);
        for (x, t) in self.tabs.iter().enumerate().take(size.cols) {
            tabs[x] = *t;
        }
        self.tabs = tabs;
        self.size = size;
        self.scroll_top = 0;
        self.scroll_bottom = size.rows.saturating_sub(1);
        self.pending_wrap = false;
        self.cursor.y = (self.cursor.y - first).min(size.rows.saturating_sub(1));
        self.cursor.x = self.cursor.x.min(size.cols.saturating_sub(1));
    }

    /// One string per row with one char per cell (`'\0'` for the right half of
    /// a wide character), so string index == column.
    pub fn visible_lines(&self) -> Vec<String> {
        (0..self.size.rows).map(|y| self.row(y).iter().map(|c| if c.style.attrs & attr::WIDE_CONT != 0 { '\0' } else { c.ch }).collect()).collect()
    }
}
