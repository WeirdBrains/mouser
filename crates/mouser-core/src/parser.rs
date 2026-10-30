use crate::grid::{Screen, Style};
use crate::kitty::{KeyboardProtocolState, KittyGraphicsEvent, parse_kitty_graphics_params};
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
}

impl<W: Write> TerminalParser<W> {
    pub fn new(size: Size, scrollback: Arc<Scrollback>, writer: W) -> Self {
        Self {
            screen: Screen::new(size),
            scrollback,
            kitty_keyboard: KeyboardProtocolState::default(),
            writer,
            style: Style::default(),
            vte: Parser::new(),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        let mut vte = std::mem::take(&mut self.vte);
        for &b in bytes {
            vte.advance(self, b);
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
            (Some(b'?'), 'u') => {
                // Kitty keyboard protocol query
                let resp = self.kitty_keyboard.negotiation_response();
                self.reply(resp);
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
            (None, b'c') => self.screen.clear(),
            (Some(b'_'), b'G') => {
                // Kitty graphics placeholder (needs a real APC handler).
                let _event = KittyGraphicsEvent::Begin { id: None, width: None, height: None };
                let _ = parse_kitty_graphics_params("");
            }
            _ => {}
        }
    }
}
