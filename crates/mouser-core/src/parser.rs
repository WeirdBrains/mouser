use crate::fuzzball::{self, Fuzzball};
use crate::graphics;
use crate::grid::{Color, CursorShape, Rgb, Screen, Style, attr};
use crate::kitty::KeyboardProtocolState;
use crate::kitty_graphics::{self, KittyGraphics};
use crate::scrollback::Scrollback;
use crate::types::Size;
use log::trace;
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use vte::{Params, Parser, Perform};

pub struct TerminalParser<W: Write> {
    pub screen: Screen,
    pub scrollback: Arc<Scrollback>,
    pub kitty_keyboard: KeyboardProtocolState,
    writer: W, // for responses (DSR, DA, kitty negotiation)
    style: Style,
    /// vte state persists across reads so escape sequences split between
    /// PTY chunks are still recognised.
    vte: Parser,
    /// Fuzzball protocol state: images, placements, keyboard mode.
    pub fuzzball: Fuzzball,
    /// Kitty graphics (fallback; Fuzzball takes precedence).
    pub kitty_gfx: KittyGraphics,
    /// Lines scrolled off the top so far (monotonic; anchors image rows).
    pub lines_scrolled: i64,
    /// Cell size in pixels (for image placement), set by the front end.
    pub cell_px: (u32, u32),
    /// Bumped when images change so the front end can redraw.
    pub graphics_generation: u64,
    /// DEC private / ANSI modes the front end needs.
    pub modes: Modes,
    /// Side effects for the front end (title, clipboard, bell…), drained by it.
    pub events: Vec<TermEvent>,
    /// Theme colours used to answer OSC 4/10/11/12 queries (set by the front end).
    pub theme: ThemeColors,
    /// OSC 4/10/11/12 overrides: 0-255 palette, 256 fg, 257 bg, 258 cursor.
    pub color_overrides: HashMap<u16, Rgb>,
    /// OSC 8 hyperlink targets, indexed by `Style::link - 1`.
    pub links: Vec<String>,
    /// OSC 7 working directory.
    pub cwd: Option<String>,
    /// OSC 133 prompt marks: (absolute row, kind A/B/C/D).
    pub prompt_marks: Vec<(i64, char)>,
    pub title: String,
    /// `$TERM` given to the child (answers XTGETTCAP `TN`).
    pub term_name: String,
    saved_style: Style,
    /// G0/G1 designations and which one is invoked into GL (SI/SO).
    charsets: [Charset; 2],
    gl: usize,
    saved_charsets: ([Charset; 2], usize),
    last_char: Option<char>,
    dcs: Option<Dcs>,
    apc: Apc,
}

/// Character sets that can be designated into G0/G1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charset {
    Ascii,
    /// `ESC ( A`: `#` is the pound sign.
    British,
    /// `ESC ( 0`: DEC Special Graphics (line drawing).
    DecSpecial,
}

/// Translate `c` through a designated character set.
pub fn translate(c: char, set: Charset) -> char {
    match set {
        Charset::Ascii => c,
        Charset::British => {
            if c == '#' {
                '£'
            } else {
                c
            }
        }
        Charset::DecSpecial => {
            const T: [char; 32] = [
                '\u{a0}', '◆', '▒', '␉', '␌', '␍', '␊', '°', '±', '␤', '␋', '┘', '┐', '┌', '└', '┼', '⎺', '⎻', '─', '⎼', '⎽', '├', '┤', '┴', '┬',
                '│', '≤', '≥', 'π', '≠', '£', '·',
            ];
            match c as u32 {
                0x5f..=0x7e => T[c as usize - 0x5f],
                _ => c,
            }
        }
    }
}

/// Terminal modes tracked for the front end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modes {
    /// DECCKM: cursor keys send `ESC O x`.
    pub app_cursor: bool,
    /// DECKPAM / DECKPNM.
    pub app_keypad: bool,
    pub bracketed_paste: bool,
    pub focus_events: bool,
    pub mouse: MouseMode,
    /// 1006 SGR encoding.
    pub mouse_sgr: bool,
    /// 1016 SGR with pixel coordinates.
    pub mouse_pixels: bool,
    /// 2026 synchronized output in progress.
    pub synchronized: bool,
    /// 2027 grapheme clustering (always on; reported as permanently set).
    pub grapheme: bool,
    /// DECSCNM: whole-screen reverse video.
    pub reverse_video: bool,
    /// LNM (ANSI mode 20): LF also returns the carriage; Return sends CR LF.
    pub newline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseMode {
    #[default]
    Off,
    /// 9 / 1000: button press and release.
    Press,
    /// 1002: plus motion while a button is held.
    Drag,
    /// 1003: all motion.
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    Title(String),
    /// OSC 52 set-clipboard (decoded text).
    Clipboard(String),
    Bell,
}

#[derive(Debug, Clone, Copy)]
pub struct ThemeColors {
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Rgb,
    pub ansi: [Rgb; 16],
}

impl Default for ThemeColors {
    fn default() -> Self {
        let c = |r, g, b| Rgb { r, g, b };
        Self {
            fg: c(220, 220, 220),
            bg: c(16, 16, 16),
            cursor: c(220, 220, 220),
            ansi: [
                c(0, 0, 0),
                c(205, 0, 0),
                c(0, 205, 0),
                c(205, 205, 0),
                c(0, 0, 238),
                c(205, 0, 205),
                c(0, 205, 205),
                c(229, 229, 229),
                c(127, 127, 127),
                c(255, 0, 0),
                c(0, 255, 0),
                c(255, 255, 0),
                c(92, 92, 255),
                c(255, 0, 255),
                c(0, 255, 255),
                c(255, 255, 255),
            ],
        }
    }
}

/// The xterm 256-colour palette entry for `i` (16-255; 0-15 come from the theme).
pub fn xterm_color(i: u8, theme: &ThemeColors) -> Rgb {
    match i {
        0..=15 => theme.ansi[i as usize],
        16..=231 => {
            let i = i - 16;
            let v = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
            Rgb { r: v(i / 36), g: v((i / 6) % 6), b: v(i % 6) }
        }
        _ => {
            let g = 8 + (i - 232) * 10;
            Rgb { r: g, g, b: g }
        }
    }
}

/// A DCS string being collected.
struct Dcs {
    kind: DcsKind,
    data: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DcsKind {
    /// `DCS + q` XTGETTCAP
    GetTcap,
    /// `DCS $ q` DECRQSS
    Rqss,
    /// `DCS = F z …` Fuzzball
    Fuzzball,
    /// A Fuzzball message longer than `fuzzball::MAX_MESSAGE` (dropped).
    Overflow,
}

/// APC strings (`ESC _ ... ESC \\`) are cut out before vte, which ignores them.
#[derive(Default)]
enum Apc {
    #[default]
    Ground,
    Esc,
    Body(Vec<u8>),
    BodyEsc(Vec<u8>),
    /// Over [`APC_MAX`]: skipped up to its terminator.
    Skip,
    SkipEsc,
}

/// Largest APC body kept (chunked Kitty images arrive in many smaller ones).
const APC_MAX: usize = 16 * 1024 * 1024;

/// Name the terminal identifies as (XTVERSION, terminfo TN).
pub const NAME: &str = "Mouser";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

impl<W: Write> TerminalParser<W> {
    pub fn new(size: Size, scrollback: Arc<Scrollback>, writer: W) -> Self {
        Self {
            screen: Screen::new(size),
            scrollback,
            kitty_keyboard: KeyboardProtocolState::default(),
            writer,
            style: Style::default(),
            vte: Parser::new(),
            fuzzball: Fuzzball::default(),
            kitty_gfx: KittyGraphics::default(),
            lines_scrolled: 0,
            cell_px: (9, 18),
            graphics_generation: 0,
            modes: Modes { grapheme: true, ..Modes::default() },
            events: Vec::new(),
            theme: ThemeColors::default(),
            color_overrides: HashMap::new(),
            links: Vec::new(),
            cwd: None,
            prompt_marks: Vec::new(),
            title: String::new(),
            term_name: "xterm-256color".into(),
            saved_style: Style::default(),
            charsets: [Charset::Ascii; 2],
            gl: 0,
            saved_charsets: ([Charset::Ascii; 2], 0),
            last_char: None,
            dcs: None,
            apc: Apc::Ground,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        let mut vte = std::mem::take(&mut self.vte);
        for &b in bytes {
            self.apc = match std::mem::take(&mut self.apc) {
                Apc::Ground if b == 0x1b => Apc::Esc,
                Apc::Ground => {
                    vte.advance(self, b);
                    Apc::Ground
                }
                Apc::Esc if b == b'_' => Apc::Body(Vec::new()),
                Apc::Esc => {
                    vte.advance(self, 0x1b);
                    if b == 0x1b {
                        Apc::Esc
                    } else {
                        vte.advance(self, b);
                        Apc::Ground
                    }
                }
                Apc::Body(buf) if b == 0x1b => Apc::BodyEsc(buf),
                Apc::Body(mut buf) => {
                    if buf.len() >= APC_MAX {
                        Apc::Skip
                    } else {
                        buf.push(b);
                        Apc::Body(buf)
                    }
                }
                Apc::BodyEsc(buf) if b == b'\\' => {
                    self.apc_dispatch(&buf);
                    Apc::Ground
                }
                Apc::BodyEsc(mut buf) => {
                    buf.extend_from_slice(&[0x1b, b]);
                    Apc::Body(buf)
                }
                Apc::Skip if b == 0x1b => Apc::SkipEsc,
                Apc::Skip => Apc::Skip,
                Apc::SkipEsc if b == b'\\' => Apc::Ground,
                Apc::SkipEsc => Apc::Skip,
            };
        }
        self.vte = vte;
        self.flush_scrollback();
    }

    /// Whether either graphics protocol has placements.
    fn has_placements(&self) -> bool {
        !self.fuzzball.gfx.placements.is_empty() || !self.kitty_gfx.gfx.placements.is_empty()
    }

    /// Palette for Kitty's indexed image formats.
    fn kitty_palette(&self) -> kitty_graphics::Palette {
        let rgb = |c: Rgb| (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
        let mut p = [0u32; 256];
        for (i, v) in p.iter_mut().enumerate().take(248) {
            *v = rgb(self.color_for(i as u16));
        }
        let (fg, bg, cur) = (rgb(self.color_for(256)), rgb(self.color_for(257)), rgb(self.color_for(258)));
        p[248..].copy_from_slice(&[bg, fg, fg, bg, cur, bg, fg, fg]);
        p
    }

    /// A Kitty graphics command (`ESC _ G ... ESC \\`); other APC strings are ignored.
    fn apc_dispatch(&mut self, body: &[u8]) {
        if body.first() != Some(&b'G') {
            return;
        }
        self.flush_scrollback();
        let alt = self.screen.in_alt_screen();
        let col = self.screen.cursor.x as u32;
        let palette = self.kitty_palette();
        let ctx = kitty_graphics::Ctx {
            abs_row: self.lines_scrolled + self.screen.cursor.y as i64,
            col,
            top: self.lines_scrolled,
            rows: self.screen.size.rows,
            cols: self.screen.size.cols,
            cell: self.cell_px,
            alt,
            other_mem: self.fuzzball.gfx.mem_used(),
            palette: &palette,
            now: std::time::Instant::now(),
        };
        let eff = self.kitty_gfx.handle(body, ctx);
        if eff.changed {
            self.graphics_generation += 1;
        }
        if let Some(r) = eff.reply {
            self.reply(&r);
        }
        if let Some((dx, dy)) = eff.cursor {
            self.move_after_image(col, dx, dy);
        }
    }

    fn move_after_image(&mut self, col: u32, dx: u32, dy: u32) {
        for _ in 0..dy {
            self.screen.linefeed();
        }
        self.flush_scrollback();
        let x = (col as usize + dx as usize).min(self.screen.size.cols.saturating_sub(1));
        let y = self.screen.cursor.y;
        self.screen.move_to(x, y);
    }

    pub fn resize(&mut self, size: Size) {
        if size == self.screen.size {
            return;
        }
        let alt = self.screen.in_alt_screen();
        let shift = if size.rows > 0 && self.screen.cursor.y >= size.rows { self.screen.cursor.y + 1 - size.rows } else { 0 };
        self.screen.resize(size);
        self.flush_scrollback();
        let shift = if alt { shift as i64 } else { 0 };
        let mut changed = false;
        for g in [&mut self.fuzzball.gfx, &mut self.kitty_gfx.gfx] {
            if !g.placements.is_empty() {
                changed |= g.resize(alt, self.lines_scrolled, shift, size.rows, size.cols);
            }
        }
        if changed {
            self.graphics_generation += 1;
        }
    }

    fn flush_scrollback(&mut self) {
        let events = self.screen.take_scroll_events();
        if self.has_placements() {
            let alt = self.screen.in_alt_screen();
            let rows = self.screen.size.rows;
            let mut changed = false;
            for ev in events {
                let base = self.lines_scrolled + ev.pushed_before as i64;
                for g in [&mut self.fuzzball.gfx, &mut self.kitty_gfx.gfx] {
                    changed |= g.scrolled(alt, base, ev, rows);
                }
            }
            if changed {
                self.graphics_generation += 1;
            }
        }
        let mut pushed = false;
        for line in self.screen.take_scrolled_off() {
            self.scrollback.push_line(line);
            self.lines_scrolled += 1;
            pushed = true;
        }
        if pushed {
            self.evict_scrolled_placements();
        }
    }

    /// Placements whose anchor row left the scrollback are removed (graphics.md §4).
    fn evict_scrolled_placements(&mut self) {
        if !self.has_placements() {
            return;
        }
        let first = self.lines_scrolled - self.scrollback.total_lines() as i64;
        let a = self.fuzzball.gfx.evict_scrollback(first);
        if self.kitty_gfx.gfx.evict_scrollback(first) | a {
            self.graphics_generation += 1;
        }
    }

    fn fuzzball_dispatch(&mut self, body: &[u8]) {
        self.flush_scrollback();
        let alt = self.screen.in_alt_screen();
        let col = self.screen.cursor.x as u32;
        let ctx = fuzzball::Ctx {
            abs_row: self.lines_scrolled + self.screen.cursor.y as i64,
            col,
            cell_w: self.cell_px.0,
            cell_h: self.cell_px.1,
            alt,
            screen_cols: self.screen.size.cols,
        };
        self.fuzzball.other_mem = self.kitty_gfx.mem_used();
        let eff = self.fuzzball.handle(body, ctx);
        if eff.changed {
            self.graphics_generation += 1;
        }
        if let Some(r) = eff.reply {
            self.reply(&r);
        }
        if let Some((dx, dy)) = eff.cursor {
            self.move_after_image(col, dx, dy);
        }
    }

    /// Erasing cells removes Fuzzball placements that cover them (graphics.md §4).
    /// Kitty placements only go with a whole-screen clear (`clear`), as the
    /// Kitty protocol asks.
    fn erase_placements(&mut self, rows: std::ops::Range<usize>, cols: Option<(usize, usize)>, clear: bool) {
        if !self.has_placements() {
            return;
        }
        self.flush_scrollback();
        let alt = self.screen.in_alt_screen();
        let base = self.lines_scrolled;
        let mut changed = false;
        match cols {
            None => changed |= self.fuzzball.gfx.erase_rows(alt, base + rows.start as i64, base + rows.end as i64),
            Some((c0, c1)) => {
                for r in rows.clone() {
                    changed |= self.fuzzball.gfx.erase_cells(alt, base + r as i64, c0 as u32, c1 as u32);
                }
            }
        }
        if clear {
            changed |= self.kitty_gfx.gfx.erase_rows(alt, base + rows.start as i64, base + rows.end as i64);
        }
        if changed {
            self.graphics_generation += 1;
        }
    }

    /// Current SGR as a DECRQSS-style parameter string (e.g. `0;1;38;2;1;2;3`).
    pub fn sgr_string(&self) -> String {
        let st = &self.style;
        let mut v = vec!["0".to_string()];
        let bits = [
            (attr::BOLD, "1"),
            (attr::DIM, "2"),
            (attr::ITALIC, "3"),
            (attr::BLINK, "5"),
            (attr::INVERSE, "7"),
            (attr::HIDDEN, "8"),
            (attr::STRIKE, "9"),
            (attr::OVERLINE, "53"),
        ];
        for (b, n) in bits {
            if st.attrs & b != 0 {
                v.push(n.into());
            }
        }
        match st.underline {
            0 => {}
            1 => v.push("4".into()),
            n => v.push(format!("4:{n}")),
        }
        let col = |c: Color, base: u8, ext: u8| -> Option<String> {
            match c {
                Color::Default => None,
                Color::Indexed(i) if i < 8 && base > 0 => Some(format!("{}", base + i)),
                Color::Indexed(i) if i < 16 && base > 0 => Some(format!("{}", base + 60 + i - 8)),
                Color::Indexed(i) => Some(format!("{ext};5;{i}")),
                Color::Rgb(r, g, b) => Some(format!("{ext};2;{r};{g};{b}")),
            }
        };
        v.extend(col(st.fg, 30, 38));
        v.extend(col(st.bg, 40, 48));
        v.extend(col(st.ul_color, 0, 58));
        v.join(";")
    }

    /// The colour a query for `slot` reports (palette 0-255, 256 fg, 257 bg, 258 cursor).
    pub fn color_for(&self, slot: u16) -> Rgb {
        if let Some(c) = self.color_overrides.get(&slot) {
            return *c;
        }
        match slot {
            256 => self.theme.fg,
            257 => self.theme.bg,
            258 => self.theme.cursor,
            i => xterm_color(i as u8, &self.theme),
        }
    }

    /// URI of the hyperlink on a visible cell, if any.
    pub fn link_at(&self, x: usize, y: usize) -> Option<&str> {
        if x >= self.screen.size.cols || y >= self.screen.size.rows {
            return None;
        }
        let id = self.screen.row(y)[x].style.link;
        (id > 0).then(|| self.links.get(id as usize - 1).map(String::as_str)).flatten()
    }

    fn sgr(&mut self, params: &Params) {
        let list: Vec<&[u16]> = params.iter().collect();
        if list.is_empty() {
            self.style = Style { link: self.style.link, ..Style::default() };
            return;
        }
        let mut i = 0;
        while i < list.len() {
            let p = list[i];
            let code = p.first().copied().unwrap_or(0);
            let st = &mut self.style;
            match code {
                0 => *st = Style { link: st.link, ..Style::default() },
                1 => st.attrs |= attr::BOLD,
                2 => st.attrs |= attr::DIM,
                3 => st.attrs |= attr::ITALIC,
                4 => st.underline = if p.len() > 1 { p[1].min(5) as u8 } else { 1 },
                5 | 6 => st.attrs |= attr::BLINK,
                7 => st.attrs |= attr::INVERSE,
                8 => st.attrs |= attr::HIDDEN,
                9 => st.attrs |= attr::STRIKE,
                21 => st.underline = 2,
                22 => st.attrs &= !(attr::BOLD | attr::DIM),
                23 => st.attrs &= !attr::ITALIC,
                24 => st.underline = 0,
                25 => st.attrs &= !attr::BLINK,
                27 => st.attrs &= !attr::INVERSE,
                28 => st.attrs &= !attr::HIDDEN,
                29 => st.attrs &= !attr::STRIKE,
                30..=37 => st.fg = Color::Indexed((code - 30) as u8),
                39 => st.fg = Color::Default,
                40..=47 => st.bg = Color::Indexed((code - 40) as u8),
                49 => st.bg = Color::Default,
                53 => st.attrs |= attr::OVERLINE,
                55 => st.attrs &= !attr::OVERLINE,
                59 => st.ul_color = Color::Default,
                90..=97 => st.fg = Color::Indexed((code - 90 + 8) as u8),
                100..=107 => st.bg = Color::Indexed((code - 100 + 8) as u8),
                38 | 48 | 58 => {
                    // Colon form carries everything in one param; semicolon form spans several.
                    let (c, used) = if p.len() > 1 { (ext_color(&p[1..], true), 0) } else { ext_color_semis(&list[i + 1..]) };
                    i += used;
                    if let Some(c) = c {
                        match code {
                            38 => st.fg = c,
                            48 => st.bg = c,
                            _ => st.ul_color = c,
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn mode(&mut self, private: bool, n: u16, set: bool) {
        if !private {
            match n {
                4 => self.screen.insert = set,
                20 => self.modes.newline = set,
                _ => {}
            }
            return;
        }
        let m = &mut self.modes;
        match n {
            1 => m.app_cursor = set,
            5 => {
                m.reverse_video = set;
                self.graphics_generation += 1;
            }
            6 => {
                self.screen.origin = set;
                self.screen.move_to_origin(0, 0);
            }
            7 => self.screen.autowrap = set,
            12 => self.screen.cursor_blink = set,
            25 => self.screen.cursor_visible = set,
            9 | 1000 => m.mouse = if set { MouseMode::Press } else { MouseMode::Off },
            1002 => m.mouse = if set { MouseMode::Drag } else { MouseMode::Off },
            1003 => m.mouse = if set { MouseMode::Any } else { MouseMode::Off },
            1004 => m.focus_events = set,
            1006 => m.mouse_sgr = set,
            1016 => m.mouse_pixels = set,
            2004 => m.bracketed_paste = set,
            2026 => m.synchronized = set,
            2027 => {}
            47 | 1047 => {
                if set {
                    self.flush_scrollback();
                    self.screen.enter_alt_screen();
                    if self.kitty_gfx.gfx.leave_alt() {
                        self.graphics_generation += 1;
                    }
                } else {
                    self.flush_scrollback();
                    self.screen.leave_alt_screen();
                    self.fuzzball.leave_alt();
                    if self.kitty_gfx.gfx.leave_alt() {
                        self.graphics_generation += 1;
                    }
                }
            }
            1048 => {
                if set {
                    self.save_cursor();
                } else {
                    self.restore_cursor();
                }
            }
            1049 => {
                if set {
                    self.save_cursor();
                    self.flush_scrollback();
                    self.screen.enter_alt_screen();
                    if self.kitty_gfx.gfx.leave_alt() {
                        self.graphics_generation += 1;
                    }
                } else {
                    self.flush_scrollback();
                    self.screen.leave_alt_screen();
                    self.fuzzball.leave_alt();
                    if self.kitty_gfx.gfx.leave_alt() {
                        self.graphics_generation += 1;
                    }
                    self.restore_cursor();
                }
            }
            _ => trace!("unhandled mode ?{n}"),
        }
    }

    /// DECRQM answer: 1 set, 2 reset, 3 permanently set, 4 permanently reset, 0 unknown.
    pub fn mode_state(&self, private: bool, n: u16) -> u8 {
        let b = |v: bool| if v { 1 } else { 2 };
        let m = &self.modes;
        if !private {
            return match n {
                4 => b(self.screen.insert),
                20 => b(self.modes.newline),
                _ => 0,
            };
        }
        match n {
            1 => b(m.app_cursor),
            5 => b(m.reverse_video),
            6 => b(self.screen.origin),
            7 => b(self.screen.autowrap),
            12 => b(self.screen.cursor_blink),
            25 => b(self.screen.cursor_visible),
            9 | 1000 => b(m.mouse == MouseMode::Press),
            1002 => b(m.mouse == MouseMode::Drag),
            1003 => b(m.mouse == MouseMode::Any),
            1004 => b(m.focus_events),
            1006 => b(m.mouse_sgr),
            1016 => b(m.mouse_pixels),
            2004 => b(m.bracketed_paste),
            2026 => b(m.synchronized),
            2027 => 3,
            47 | 1047 | 1049 => b(self.screen.in_alt_screen()),
            _ => 0,
        }
    }

    fn save_cursor(&mut self) {
        self.screen.save_cursor();
        self.saved_style = self.style;
        self.saved_charsets = (self.charsets, self.gl);
    }

    fn restore_cursor(&mut self) {
        self.screen.restore_cursor();
        self.style = self.saved_style;
        (self.charsets, self.gl) = self.saved_charsets;
    }

    fn reset(&mut self) {
        self.screen.clear();
        self.style = Style::default();
        self.saved_style = Style::default();
        self.kitty_keyboard = KeyboardProtocolState::default();
        self.fuzzball = Fuzzball::default();
        self.kitty_gfx = KittyGraphics::default();
        self.graphics_generation += 1;
        self.modes = Modes { grapheme: true, ..Modes::default() };
        self.color_overrides.clear();
        self.charsets = [Charset::Ascii; 2];
        self.gl = 0;
    }

    fn osc_color(&mut self, slot: u16, spec: &[u8], st: &str, prefix: &str) {
        if spec == b"?" {
            let c = self.color_for(slot);
            let h = |v: u8| format!("{v:02x}{v:02x}");
            self.reply(&format!("\x1b]{prefix};rgb:{}/{}/{}{st}", h(c.r), h(c.g), h(c.b)));
        } else if let Some(c) = parse_color(&String::from_utf8_lossy(spec)) {
            self.color_overrides.insert(slot, c);
            self.graphics_generation += 1;
        }
    }

    fn dcs_finish(&mut self, d: Dcs) {
        let text = String::from_utf8_lossy(&d.data).to_string();
        match d.kind {
            DcsKind::GetTcap => {
                for name_hex in text.split(';') {
                    let name = hex::decode(name_hex).ok().map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
                    let v = if name == "TN" || name == "name" { Some(self.term_name.clone()) } else { tcap(&name) };
                    match v {
                        Some(v) => {
                            let r = format!("\x1bP1+r{}={}\x1b\\", name_hex, hex::encode(v));
                            self.reply(&r);
                        }
                        None => self.reply(&format!("\x1bP0+r{name_hex}\x1b\\")),
                    }
                }
            }
            DcsKind::Rqss => {
                let (t, b) = self.screen.scroll_region();
                let v = match text.as_str() {
                    "m" => Some(format!("{}m", self.sgr_string())),
                    "r" => Some(format!("{};{}r", t + 1, b + 1)),
                    " q" => {
                        let base = match self.screen.cursor_shape {
                            CursorShape::Block => 1,
                            CursorShape::Underline => 3,
                            CursorShape::Bar => 5,
                        };
                        Some(format!("{} q", base + u8::from(!self.screen.cursor_blink)))
                    }
                    "\"p" => Some("62;1\"p".into()),
                    _ => None,
                };
                match v {
                    Some(v) => self.reply(&format!("\x1bP1$r{v}\x1b\\")),
                    None => self.reply("\x1bP0$r\x1b\\"),
                }
            }
            DcsKind::Fuzzball => self.fuzzball_dispatch(&d.data),
            DcsKind::Overflow => {}
        }
    }

    fn reply(&mut self, s: &str) {
        let _ = self.writer.write_all(s.as_bytes());
        let _ = self.writer.flush();
    }
}

/// First parameter, with 0/missing mapped to `default`.
fn arg(params: &Params, i: usize, default: u16) -> u16 {
    match params.iter().nth(i).and_then(|p| p.first().copied()) {
        Some(0) | None => default,
        Some(v) => v,
    }
}

impl<W: Write> Perform for TerminalParser<W> {
    fn print(&mut self, c: char) {
        let c = translate(c, self.charsets[self.gl]);
        self.screen.put_char(c, self.style);
        self.last_char = Some(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => {
                if self.modes.newline {
                    self.screen.carriage_return();
                }
                self.screen.linefeed();
            }
            b'\r' => self.screen.carriage_return(),
            0x08 => self.screen.backspace(),
            b'\t' => self.screen.tab(),
            0x07 => self.events.push(TermEvent::Bell),
            0x0e => self.gl = 1,
            0x0f => self.gl = 0,
            _ => {}
        }
    }

    fn hook(&mut self, _params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let kind = match (intermediates, action) {
            (b"+", 'q') => Some(DcsKind::GetTcap),
            (b"$", 'q') => Some(DcsKind::Rqss),
            (b"=", 'F') => Some(DcsKind::Fuzzball),
            _ => None,
        };
        self.dcs = kind.map(|kind| Dcs { kind, data: Vec::new() });
    }

    fn put(&mut self, byte: u8) {
        let Some(d) = &mut self.dcs else { return };
        let cap = if d.kind == DcsKind::Fuzzball { fuzzball::MAX_MESSAGE } else { 4096 };
        if d.data.len() < cap {
            d.data.push(byte);
        } else if d.kind == DcsKind::Fuzzball {
            d.kind = DcsKind::Overflow;
            d.data = Vec::new();
        }
    }

    fn unhook(&mut self) {
        if let Some(d) = self.dcs.take() {
            self.dcs_finish(d);
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        let st = if bell_terminated { "\x07" } else { "\x1b\\" };
        let Some(first) = params.first() else { return };
        let text = |i: usize| params.get(i).map(|p| String::from_utf8_lossy(p).to_string()).unwrap_or_default();
        // Everything after the first `;` (payloads may contain `;`).
        let rest = |from: usize| {
            params.get(from..).map(|ps| ps.iter().map(|p| String::from_utf8_lossy(p)).collect::<Vec<_>>().join(";")).unwrap_or_default()
        };
        match *first {
            b"0" | b"2" => {
                let t = rest(1);
                self.title = t.clone();
                self.events.push(TermEvent::Title(t));
            }
            b"1" => {}
            b"4" => {
                let mut i = 1;
                while i + 1 < params.len() {
                    if let Ok(idx) = text(i).parse::<u16>()
                        && idx < 256
                    {
                        let spec = params[i + 1];
                        self.osc_color(idx, spec, st, &format!("4;{idx}"));
                    }
                    i += 2;
                }
            }
            b"10" | b"11" | b"12" => {
                let base: u16 = text(0).parse().unwrap_or(10);
                for (k, spec) in params[1..].iter().enumerate() {
                    let n = base + k as u16;
                    if n <= 12 {
                        self.osc_color(256 + n - 10, spec, st, &n.to_string());
                    }
                }
            }
            b"104" => {
                if params.len() == 1 {
                    self.color_overrides.retain(|k, _| *k >= 256);
                } else {
                    for p in &params[1..] {
                        if let Ok(i) = String::from_utf8_lossy(p).parse::<u16>() {
                            self.color_overrides.remove(&i);
                        }
                    }
                }
                self.graphics_generation += 1;
            }
            b"110" | b"111" | b"112" => {
                let n: u16 = text(0).parse().unwrap_or(110);
                self.color_overrides.remove(&(256 + n - 110));
                self.graphics_generation += 1;
            }
            b"7" => self.cwd = Some(rest(1)),
            b"8" => {
                let uri = rest(2);
                self.style.link = if uri.is_empty() {
                    0
                } else {
                    let pos = self.links.iter().position(|u| *u == uri).unwrap_or_else(|| {
                        self.links.push(uri);
                        self.links.len() - 1
                    });
                    pos as u32 + 1
                };
            }
            b"52" => {
                let data = params.get(2).copied().unwrap_or(b"");
                // Reading the clipboard is off by default (privacy); only writes are honoured.
                if data != b"?"
                    && let Some(bytes) = graphics::base64_decode(data)
                {
                    self.events.push(TermEvent::Clipboard(String::from_utf8_lossy(&bytes).to_string()));
                }
            }
            b"133" => {
                if let Some(k) = text(1).chars().next() {
                    self.flush_scrollback();
                    let row = self.lines_scrolled + self.screen.cursor.y as i64;
                    self.prompt_marks.push((row, k));
                    if self.prompt_marks.len() > 1000 {
                        self.prompt_marks.remove(0);
                    }
                }
            }
            _ => trace!("OSC: {:?}", String::from_utf8_lossy(first)),
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let private = intermediates.first().copied();
        let n = arg(params, 0, 1) as usize;
        self.screen.erase_bg = self.style.bg;
        match (private, action) {
            (None, 'A') => self.screen.move_by(0, -(n as isize)),
            (None, 'B') | (None, 'e') => self.screen.move_by(0, n as isize),
            (None, 'C') | (None, 'a') => self.screen.move_by(n as isize, 0),
            (None, 'D') => self.screen.move_by(-(n as isize), 0),
            (None, 'E') => {
                self.screen.move_by(0, n as isize);
                self.screen.carriage_return();
            }
            (None, 'F') => {
                self.screen.move_by(0, -(n as isize));
                self.screen.carriage_return();
            }
            (None, 'G') | (None, '`') => {
                let y = self.screen.cursor.y;
                self.screen.move_to(n - 1, y);
            }
            (None, 'd') => {
                let x = self.screen.cursor.x;
                self.screen.move_to_origin(x, n - 1);
            }
            (None, 'H') | (None, 'f') => {
                let row = arg(params, 0, 1) as usize;
                let col = arg(params, 1, 1) as usize;
                self.screen.move_to_origin(col - 1, row - 1);
            }
            (None, 'I') => self.screen.tab_forward(n),
            (None, 'Z') => self.screen.tab_backward(n),
            (None, 'g') => self.screen.clear_tab(params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0)),
            (None, 'b') => {
                if let Some(c) = self.last_char {
                    for _ in 0..n.min(65535) {
                        self.screen.put_char(c, self.style);
                    }
                }
            }
            (Some(b' '), 'q') => {
                let v = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0);
                self.screen.cursor_shape = match v {
                    3 | 4 => CursorShape::Underline,
                    5 | 6 => CursorShape::Bar,
                    _ => CursorShape::Block,
                };
                self.screen.cursor_blink = v == 0 || v % 2 == 1;
            }
            (None, 'J') => {
                let mode = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0);
                let (y, rows) = (self.screen.cursor.y, self.screen.size.rows);
                match mode {
                    0 => self.erase_placements(y..rows, None, false),
                    1 => self.erase_placements(0..y + 1, None, false),
                    // ED 3 (xterm): clear the scrollback only, not the screen.
                    3 => {
                        self.flush_scrollback();
                        self.scrollback.clear();
                        self.evict_scrolled_placements();
                        return;
                    }
                    _ => self.erase_placements(0..rows, None, true),
                }
                self.screen.erase_display(mode)
            }
            (None, 'K') => {
                let mode = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0);
                let (x, y, cols) = (self.screen.cursor.x, self.screen.cursor.y, self.screen.size.cols);
                let span = match mode {
                    0 => (x, cols),
                    1 => (0, x + 1),
                    _ => (0, cols),
                };
                self.erase_placements(y..y + 1, Some(span), false);
                self.screen.erase_line(mode)
            }
            (None, 'X') => {
                let (x, y) = (self.screen.cursor.x, self.screen.cursor.y);
                self.erase_placements(y..y + 1, Some((x, x + n)), false);
                self.screen.erase_chars(n)
            }

            (None, '@') => self.screen.insert_chars(n),
            (None, 'P') => self.screen.delete_chars(n),
            (None, 'L') => self.screen.insert_lines(n),
            (None, 'M') => self.screen.delete_lines(n),
            (None, 'S') => self.screen.scroll_up_region(n),
            (None, 'T') => self.screen.scroll_down_region(n),
            (None, 'r') => {
                let top = arg(params, 0, 1) as usize;
                let bottom = arg(params, 1, self.screen.size.rows as u16) as usize;
                self.screen.set_scroll_region(top - 1, bottom - 1);
            }
            (None, 's') => self.save_cursor(),
            (None, 'u') => self.restore_cursor(),
            (None, 'n') => match arg(params, 0, 0) {
                5 => self.reply("\x1b[0n"),
                6 => {
                    let c = self.screen.cursor;
                    self.reply(&format!("\x1b[{};{}R", c.y + 1, c.x + 1));
                }
                _ => {}
            },
            (Some(b'?'), 'n') if arg(params, 0, 0) == 6 => {
                let c = self.screen.cursor;
                self.reply(&format!("\x1b[?{};{}R", c.y + 1, c.x + 1));
            }
            // DA1: VT220-class with ANSI colour (22) and clipboard (52).
            (None, 'c') if arg(params, 0, 0) == 0 => self.reply("\x1b[?62;22c"),
            (Some(b'>'), 'c') if arg(params, 0, 0) == 0 => {
                let v: Vec<u32> = VERSION.split('.').filter_map(|x| x.parse().ok()).collect();
                let num = v.first().unwrap_or(&0) * 10000 + v.get(1).unwrap_or(&0) * 100 + v.get(2).unwrap_or(&0);
                self.reply(&format!("\x1b[>1;{num};0c"));
            }
            (Some(b'='), 'c') if arg(params, 0, 0) == 0 => self.reply("\x1bP!|4D4F5553\x1b\\"),
            // DECREQTPARM: no parity, 8 bits, 38400 baud, clock multiplier 1.
            (None, 'x') if arg(params, 0, 0) <= 1 => {
                let kind = arg(params, 0, 0) + 2;
                self.reply(&format!("\x1b[{kind};1;1;128;128;1;0x"));
            }
            (Some(b'>'), 'q') if arg(params, 0, 0) == 0 => self.reply(&format!("\x1bP>|{NAME}({VERSION})\x1b\\")),
            (None, 'm') => self.sgr(params),
            (Some(b'?'), 'h') | (Some(b'?'), 'l') | (None, 'h') | (None, 'l') => {
                let set = action == 'h';
                for p in params.iter() {
                    self.mode(private.is_some(), p.first().copied().unwrap_or(0), set);
                }
            }
            (_, 'p') if intermediates.last() == Some(&b'$') => {
                let is_private = private == Some(b'?');
                let m = arg(params, 0, 0);
                let state = self.mode_state(is_private, m);
                let q = if is_private { "?" } else { "" };
                self.reply(&format!("\x1b[{q}{m};{state}$y"));
            }
            (Some(b'!'), 'p') => self.reset_soft(),
            // Window state is always "not iconified"; Wayland has no global
            // window position, so 13t reports the origin.
            (None, 't') if arg(params, 0, 0) == 11 => self.reply("\x1b[1t"),
            (None, 't') if arg(params, 0, 0) == 13 => self.reply("\x1b[3;0;0t"),
            (None, 't') if arg(params, 0, 0) == 20 => self.reply("\x1b]L\x1b\\"),
            (None, 't') if arg(params, 0, 0) == 21 => {
                let t = self.title.replace(['\x1b', '\x07'], "");
                self.reply(&format!("\x1b]l{t}\x1b\\"));
            }
            (None, 't') if arg(params, 0, 0) == 18 => {
                let s = self.screen.size;
                self.reply(&format!("\x1b[8;{};{}t", s.rows, s.cols));
            }
            (None, 't') if arg(params, 0, 0) == 16 => {
                let (w, h) = self.cell_px;
                self.reply(&format!("\x1b[6;{h};{w}t"));
            }
            (None, 't') if arg(params, 0, 0) == 14 => {
                let (w, h) = self.cell_px;
                let s = self.screen.size;
                self.reply(&format!("\x1b[4;{};{}t", s.rows as u32 * h, s.cols as u32 * w));
            }
            // Kitty keyboard protocol: query / push / pop / set, per screen.
            (Some(b'?'), 'u') => {
                let resp = self.kitty_keyboard.query_response(self.screen.in_alt_screen());
                self.reply(&resp);
            }
            (Some(b'>'), 'u') => {
                let f = arg(params, 0, 0) as u32;
                self.kitty_keyboard.push(self.screen.in_alt_screen(), f);
            }
            (Some(b'<'), 'u') => {
                let n = arg(params, 0, 1) as usize;
                self.kitty_keyboard.pop(self.screen.in_alt_screen(), n);
            }
            (Some(b'='), 'u') => {
                let (f, mode) = (arg(params, 0, 0) as u32, arg(params, 1, 1) as u32);
                self.kitty_keyboard.set(self.screen.in_alt_screen(), f, mode);
            }
            _ => trace!("unhandled CSI {:?} {:?}", intermediates, action),
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (intermediates.first().copied(), byte) {
            (None, b'7') => self.save_cursor(),
            (None, b'8') => self.restore_cursor(),
            (None, b'H') => self.screen.set_tab(),
            (Some(g @ (b'(' | b')')), set) => {
                let slot = usize::from(g == b')');
                self.charsets[slot] = match set {
                    b'0' => Charset::DecSpecial,
                    b'A' => Charset::British,
                    _ => Charset::Ascii,
                };
            }
            (None, b'=') => self.modes.app_keypad = true,
            (None, b'>') => self.modes.app_keypad = false,
            (Some(b'#'), b'8') => {
                // DECALN: fill with E.
                let (cols, rows) = (self.screen.size.cols, self.screen.size.rows);
                self.screen.move_to(0, 0);
                for c in self.screen.cells.iter_mut() {
                    *c = crate::grid::Cell { ch: 'E', ..Default::default() };
                }
                let _ = (cols, rows);
            }
            (None, b'D') => self.screen.linefeed(),
            (None, b'E') => {
                self.screen.carriage_return();
                self.screen.linefeed();
            }
            (None, b'M') => self.screen.reverse_index(),
            (None, b'c') => self.reset(),
            _ => {}
        }
    }
}

impl<W: Write> TerminalParser<W> {
    /// DECSTR soft reset: modes and attributes, not the screen contents.
    fn reset_soft(&mut self) {
        self.style = Style::default();
        self.saved_style = Style::default();
        self.screen.cursor_visible = true;
        self.screen.autowrap = true;
        self.screen.origin = false;
        self.screen.insert = false;
        let rows = self.screen.size.rows;
        let (x, y) = (self.screen.cursor.x, self.screen.cursor.y);
        self.screen.set_scroll_region(0, rows.saturating_sub(1));
        self.screen.move_to(x, y);
        self.modes.app_cursor = false;
        self.modes.app_keypad = false;
    }
}

/// `38:2::r:g:b`, `38:2:r:g:b`, `38:5:n` (colon sub-parameters after the 38).
fn ext_color(sub: &[u16], colon: bool) -> Option<Color> {
    match sub.first()? {
        5 => Some(Color::Indexed(*sub.get(1)? as u8)),
        2 => {
            // With colons, ITU T.416 puts a colour-space id before r:g:b.
            let rgb = if colon && sub.len() >= 5 { &sub[2..5] } else { sub.get(1..4)? };
            Some(Color::Rgb(rgb[0] as u8, rgb[1] as u8, rgb[2] as u8))
        }
        _ => None,
    }
}

/// `38;5;n` / `38;2;r;g;b` spread over separate parameters. Returns params used.
fn ext_color_semis(rest: &[&[u16]]) -> (Option<Color>, usize) {
    let v = |i: usize| rest.get(i).and_then(|p| p.first().copied());
    match v(0) {
        Some(5) => (v(1).map(|n| Color::Indexed(n as u8)), 2),
        Some(2) => match (v(1), v(2), v(3)) {
            (Some(r), Some(g), Some(b)) => (Some(Color::Rgb(r as u8, g as u8, b as u8)), 4),
            _ => (None, rest.len()),
        },
        _ => (None, 1),
    }
}

/// `rgb:r/g/b` (1-4 hex digits each) or `#rrggbb`.
pub fn parse_color(s: &str) -> Option<Rgb> {
    if let Some(h) = s.strip_prefix('#') {
        if h.len() == 6 {
            let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
            return Some(Rgb { r: p(0)?, g: p(2)?, b: p(4)? });
        }
        return None;
    }
    let body = s.strip_prefix("rgb:")?;
    let parts: Vec<&str> = body.split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let ch = |p: &str| -> Option<u8> {
        if p.is_empty() || p.len() > 4 {
            return None;
        }
        let v = u32::from_str_radix(p, 16).ok()?;
        let max = (1u32 << (4 * p.len())) - 1;
        Some((v * 255 / max) as u8)
    };
    Some(Rgb { r: ch(parts[0])?, g: ch(parts[1])?, b: ch(parts[2])? })
}

/// XTGETTCAP answers (terminfo names and a few tmux-style extensions).
pub fn tcap(name: &str) -> Option<String> {
    Some(
        match name {
            "TN" | "name" => "xterm-256color",
            "Co" | "colors" => "256",
            "RGB" => "8/8/8",
            "Tc" => "",
            "setrgbf" => "\x1b[38:2::%p1%d:%p2%d:%p3%dm",
            "setrgbb" => "\x1b[48:2::%p1%d:%p2%d:%p3%dm",
            "Smulx" => "\x1b[4:%p1%dm",
            "Setulc" => "\x1b[58:2::%p1%{65536}%/%d:%p1%{256}%/%{255}%&%d:%p1%{255}%&%dm",
            "Ss" => "\x1b[%p1%d q",
            "Se" => "\x1b[2 q",
            "Sync" => "\x1b[?2026%?%p1%{1}%-%tl%eh%;",
            "BD" => "\x1b[?2004l",
            "BE" => "\x1b[?2004h",
            "PS" => "\x1b[200~",
            "PE" => "\x1b[201~",
            "fd" => "\x1b[?1004l",
            "fe" => "\x1b[?1004h",
            "smxx" => "\x1b[9m",
            // Fuzzball protocol version (docs/protocol/README.md §4).
            "Fz" => "0",
            "rmxx" => "\x1b[29m",
            _ => return None,
        }
        .to_string(),
    )
}
