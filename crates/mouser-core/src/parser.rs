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
    writer: W, // for responses (DSR, kitty negotiation)
    style: Style,
    cur_line: String,
}

impl<W: Write> TerminalParser<W> {
    pub fn new(size: Size, scrollback: Arc<Scrollback>, writer: W) -> Self {
        Self {
            screen: Screen::new(size),
            scrollback,
            kitty_keyboard: KeyboardProtocolState::default(),
            writer,
            style: Style::default(),
            cur_line: String::new(),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        let mut parser = Parser::new();
        for &b in bytes {
            parser.advance(self, b);
        }
    }

    pub fn resize(&mut self, size: Size) {
        self.screen.resize(size);
    }
}

impl<W: Write> Perform for TerminalParser<W> {
    fn print(&mut self, c: char) {
        if c == '\n' {
            self.screen.linefeed();
            self.scrollback.push_line(std::mem::take(&mut self.cur_line));
        } else {
            self.cur_line.push(c);
            self.screen.put_char(c, self.style);
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.screen.linefeed();
                self.scrollback.push_line(std::mem::take(&mut self.cur_line));
            }
            b'\r' => self.screen.carriage_return(),
            0x08 => self.screen.backspace(), // BS
            _ => {
                // ignore others for now
            }
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // OSC 52 clipboard and OSC 8 hyperlinks could be parsed here.
        // For now we only trace.
        if let Some(first) = params.first() {
            trace!("OSC: {:?}", String::from_utf8_lossy(first));
        }
    }

    fn csi_dispatch(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, action: char) {
        match action {
            'A' => {
                // CUU
                let n = 1usize;
                if self.screen.cursor.y >= n {
                    self.screen.cursor.y -= n;
                } else {
                    self.screen.cursor.y = 0;
                }
            }
            'B' => {
                // CUD
                let n = 1usize;
                self.screen.cursor.y = (self.screen.cursor.y + n).min(self.screen.size.rows - 1);
            }
            'C' => {
                // CUF
                let n = 1usize;
                self.screen.cursor.x = (self.screen.cursor.x + n).min(self.screen.size.cols - 1);
            }
            'D' => {
                // CUB
                let n = 1usize;
                if self.screen.cursor.x >= n {
                    self.screen.cursor.x -= n;
                } else {
                    self.screen.cursor.x = 0;
                }
            }
            'u' => {
                // Kitty keyboard negotiation or key reports; for now acknowledge negotiation
                let _ = self.writer.write_all(self.kitty_keyboard.negotiation_response().as_bytes());
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        // Kitty graphics sequences use ESC_G ... ST/BEL form (terminator handled upstream).
        if intermediates == b"_" && (byte as char) == 'G' {
            // For simplicity assume we receive "ESC_G<params>;" followed by data
            // Real implementations need a DCS handler; here we only demonstrate param parsing.
            let _event = KittyGraphicsEvent::Begin { id: None, width: None, height: None };
            let _ = parse_kitty_graphics_params("");
        }
    }
}
