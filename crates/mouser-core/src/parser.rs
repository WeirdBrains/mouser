use crate::graphics::{self, Graphics};
use crate::grid::{Screen, Style};
use crate::kitty::KeyboardProtocolState;
use crate::scrollback::Scrollback;
use crate::types::Size;
use log::trace;
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
    apc: Apc,
    /// Kitty graphics images and placements.
    pub graphics: Graphics,
    /// Lines scrolled off the top so far (monotonic; anchors image rows).
    pub lines_scrolled: i64,
    /// Cell size in pixels (for image placement), set by the front end.
    pub cell_px: (u32, u32),
    /// Bumped when images change so the front end can redraw.
    pub graphics_generation: u64,
}

/// APC strings (`ESC _ ... ESC \`) are cut out before vte, which ignores them.
#[derive(Default)]
enum Apc {
    #[default]
    Ground,
    Esc,
    Body(Vec<u8>),
    BodyEsc(Vec<u8>),
}

/// Largest APC body kept (chunked images arrive in many smaller ones).
const APC_MAX: usize = 16 * 1024 * 1024;

impl<W: Write> TerminalParser<W> {
    pub fn new(size: Size, scrollback: Arc<Scrollback>, writer: W) -> Self {
        Self {
            screen: Screen::new(size),
            scrollback,
            kitty_keyboard: KeyboardProtocolState::default(),
            writer,
            style: Style::default(),
            vte: Parser::new(),
            apc: Apc::Ground,
            graphics: Graphics::default(),
            lines_scrolled: 0,
            cell_px: (9, 18),
            graphics_generation: 0,
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
                    if buf.len() < APC_MAX {
                        buf.push(b);
                    }
                    Apc::Body(buf)
                }
                Apc::BodyEsc(buf) if b == b'\\' => {
                    self.apc_dispatch(&buf);
                    Apc::Ground
                }
                Apc::BodyEsc(mut buf) => {
                    buf.extend_from_slice(&[0x1b, b]);
                    Apc::Body(buf)
                }
            };
        }
        self.vte = vte;
        self.flush_scrollback();
    }

    pub fn resize(&mut self, size: Size) {
        self.screen.resize(size);
        self.flush_scrollback();
    }

    fn flush_scrollback(&mut self) {
        for line in self.screen.take_scrolled_off() {
            self.scrollback.push_line(line);
            self.lines_scrolled += 1;
        }
    }

    fn apc_dispatch(&mut self, body: &[u8]) {
        let Some(rest) = body.strip_prefix(b"G") else { return };
        self.flush_scrollback();
        let top = self.lines_scrolled;
        let cur = graphics::Cursor {
            top,
            abs_row: top + self.screen.cursor.y as i64,
            col: self.screen.cursor.x as u32,
            cell_w: self.cell_px.0,
            cell_h: self.cell_px.1,
        };
        let eff = self.graphics.command(rest, cur);
        self.graphics_generation += 1;
        if let Some(r) = eff.reply {
            self.reply(&r);
        }
        if let Some((dx, dy)) = eff.cursor {
            for _ in 0..dy {
                self.screen.linefeed();
            }
            self.flush_scrollback();
            let x = (cur.col as usize + dx as usize).min(self.screen.size.cols - 1);
            let y = self.screen.cursor.y;
            self.screen.move_to(x, y);
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
        self.screen.put_char(c, self.style);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => self.screen.linefeed(),
            b'\r' => self.screen.carriage_return(),
            0x08 => self.screen.backspace(),
            b'\t' => self.screen.tab(),
            _ => {}
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // Window titles, OSC 7 cwd, OSC 8 hyperlinks, OSC 133 marks: accepted and ignored.
        if let Some(first) = params.first() {
            trace!("OSC: {:?}", String::from_utf8_lossy(first));
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let private = intermediates.first().copied();
        let n = arg(params, 0, 1) as usize;
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
                self.screen.move_to(x, n - 1);
            }
            (None, 'H') | (None, 'f') => {
                let row = arg(params, 0, 1) as usize;
                let col = arg(params, 1, 1) as usize;
                self.screen.move_to(col - 1, row - 1);
            }
            (None, 'J') => self.screen.erase_display(params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0)),
            (None, 'K') => self.screen.erase_line(params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0)),
            (None, 'X') => self.screen.erase_chars(n),
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
            (None, 's') => self.screen.save_cursor(),
            (None, 'u') => self.screen.restore_cursor(),
            (None, 'n') => match arg(params, 0, 0) {
                5 => self.reply("\x1b[0n"),
                6 => {
                    let c = self.screen.cursor;
                    self.reply(&format!("\x1b[{};{}R", c.y + 1, c.x + 1));
                }
                _ => {}
            },
            (None, 'c') => self.reply("\x1b[?62;22c"),
            (None, 'm') => {} // SGR: colours are not rendered yet
            (Some(b'?'), 'h') | (Some(b'?'), 'l') => {
                let set = action == 'h';
                for p in params.iter() {
                    match p.first().copied().unwrap_or(0) {
                        25 => self.screen.cursor_visible = set,
                        47 | 1047 | 1049 => {
                            if set {
                                self.screen.save_cursor();
                                self.screen.enter_alt_screen();
                            } else {
                                self.screen.leave_alt_screen();
                            }
                        }
                        _ => {}
                    }
                }
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
            (None, b'7') => self.screen.save_cursor(),
            (None, b'8') => self.screen.restore_cursor(),
            (None, b'D') => self.screen.linefeed(),
            (None, b'E') => {
                self.screen.carriage_return();
                self.screen.linefeed();
            }
            (None, b'M') => self.screen.reverse_index(),
            (None, b'c') => {
                self.screen.clear();
                self.kitty_keyboard = KeyboardProtocolState::default();
                self.graphics = Graphics::default();
            }
            _ => {}
        }
    }
}
