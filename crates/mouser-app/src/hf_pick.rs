//! `mouser model hf pick [QUERY]`: choose a Hugging Face GGUF on the
//! terminal (query → repository → quantization with its size), then the
//! usual `mouser model hf install` (consent, resumable download, checksums).
//! The screen logic ([`Picker`]) is plain state; [`pick`] drives it on a
//! raw-mode terminal and always puts the terminal back.

use anyhow::{Result, bail};
use mouser_core::models::hf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    Esc,
    Backspace,
    /// Ctrl-C, or Ctrl-D / end of input: leave.
    Quit,
    Char(char),
}

/// Keys in a chunk of terminal input (a lone ESC is Escape).
pub fn parse_keys(b: &[u8]) -> Vec<Key> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        i += 1;
        match c {
            0x1b if i == b.len() => out.push(Key::Esc),
            0x1b if b[i] == b'[' || b[i] == b'O' => {
                let start = i + 1;
                let mut j = start;
                while j < b.len() && !(0x40..=0x7e).contains(&b[j]) {
                    j += 1;
                }
                let Some(&fin) = b.get(j) else { break };
                let params = &b[start..j];
                i = j + 1;
                out.extend(match (fin, params) {
                    (b'A', _) => Some(Key::Up),
                    (b'B', _) => Some(Key::Down),
                    (b'D', _) => Some(Key::Left),
                    (b'H', _) | (b'~', b"1") | (b'~', b"7") => Some(Key::Home),
                    (b'F', _) | (b'~', b"4") | (b'~', b"8") => Some(Key::End),
                    (b'~', b"5") => Some(Key::PageUp),
                    (b'~', b"6") => Some(Key::PageDown),
                    _ => None,
                });
            }
            0x1b => out.push(Key::Esc),
            b'\r' | b'\n' => out.push(Key::Enter),
            0x03 | 0x04 => out.push(Key::Quit),
            0x7f | 0x08 => out.push(Key::Backspace),
            c if c < 0x20 => {}
            _ => {
                // One UTF-8 character.
                let len = match c {
                    0xf0.. => 4,
                    0xe0.. => 3,
                    0xc0.. => 2,
                    _ => 1,
                };
                let end = (i - 1 + len).min(b.len());
                if let Ok(s) = std::str::from_utf8(&b[i - 1..end]) {
                    out.extend(s.chars().next().map(Key::Char));
                }
                i = end;
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Results {
    pub query: String,
    pub hits: Vec<hf::Hit>,
    pub sizes: Vec<hf::Sizes>,
    pub sel: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quants {
    pub details: hf::Details,
    pub sel: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    Query(String),
    /// Waiting for the Hub; Escape goes back to `back`.
    Loading {
        what: String,
        back: Box<Screen>,
    },
    Results,
    Quants,
    Error {
        message: String,
        back: Box<Screen>,
    },
}

/// What the driver should do after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Search; answer with [`Picker::searched`] and this generation.
    Search(String, u64),
    /// Read a repository; answer with [`Picker::opened`].
    Open(String, u64),
    Install {
        id: String,
        quant: String,
    },
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    pub screen: Screen,
    pub results: Option<Results>,
    pub quants: Option<Quants>,
    /// Answers from an older request (left with Escape) are dropped.
    pub generation: u64,
}

impl Picker {
    /// Starts at the query line, or already searching for `query`.
    pub fn new(query: Option<&str>) -> (Picker, Effect) {
        let mut p = Picker { screen: Screen::Query(String::new()), results: None, quants: None, generation: 0 };
        let e = match query.map(str::trim).filter(|q| !q.is_empty()) {
            Some(q) => p.search(q.to_string()),
            None => Effect::None,
        };
        (p, e)
    }

    fn search(&mut self, q: String) -> Effect {
        self.generation += 1;
        let back = Box::new(Screen::Query(q.clone()));
        self.screen = Screen::Loading { what: format!("Searching Hugging Face for {q:?}…"), back };
        Effect::Search(q, self.generation)
    }

    pub fn key(&mut self, k: &Key, page: usize) -> Effect {
        if *k == Key::Quit {
            return Effect::Cancel;
        }
        let page = page.max(1);
        match &mut self.screen {
            Screen::Query(text) => match k {
                Key::Char(c) => {
                    text.push(*c);
                    Effect::None
                }
                Key::Backspace => {
                    text.pop();
                    Effect::None
                }
                Key::Enter if !text.trim().is_empty() => {
                    let q = text.trim().to_string();
                    self.search(q)
                }
                Key::Esc if self.results.is_some() => {
                    self.screen = Screen::Results;
                    Effect::None
                }
                Key::Esc => Effect::Cancel,
                _ => Effect::None,
            },
            Screen::Loading { .. } | Screen::Error { .. } => {
                let back = match &self.screen {
                    Screen::Loading { back, .. } if matches!(k, Key::Esc | Key::Backspace | Key::Left) => Some((**back).clone()),
                    Screen::Error { back, .. } => Some((**back).clone()),
                    _ => None,
                };
                if let Some(b) = back {
                    // A late answer to what was left is dropped.
                    self.generation += 1;
                    self.screen = b;
                }
                Effect::None
            }
            Screen::Results => {
                let r = self.results.as_mut().expect("results");
                let n = r.hits.len();
                match k {
                    Key::Enter if n > 0 => {
                        let id = r.hits[r.sel].id.clone();
                        self.generation += 1;
                        self.screen = Screen::Loading { what: format!("Reading {id}…"), back: Box::new(Screen::Results) };
                        Effect::Open(id, self.generation)
                    }
                    Key::Char('/') | Key::Char('s') => {
                        self.screen = Screen::Query(r.query.clone());
                        Effect::None
                    }
                    Key::Esc | Key::Char('q') => Effect::Cancel,
                    _ => {
                        r.sel = moved(r.sel, n, k, page);
                        Effect::None
                    }
                }
            }
            Screen::Quants => {
                let q = self.quants.as_mut().expect("quants");
                let n = q.details.quants.len();
                match k {
                    Key::Enter if n > 0 => Effect::Install { id: q.details.id.clone(), quant: q.details.quants[q.sel].label.clone() },
                    Key::Esc | Key::Backspace | Key::Left | Key::Char('h') => {
                        self.screen = Screen::Results;
                        Effect::None
                    }
                    Key::Char('q') => Effect::Cancel,
                    _ => {
                        q.sel = moved(q.sel, n, k, page);
                        Effect::None
                    }
                }
            }
        }
    }

    /// The answer to [`Effect::Search`].
    pub fn searched(&mut self, generation: u64, r: Result<(String, Vec<hf::Hit>, Vec<hf::Sizes>)>) {
        if generation != self.generation {
            return;
        }
        match r {
            Ok((query, hits, sizes)) => {
                self.results = Some(Results { query, hits, sizes, sel: 0 });
                self.screen = Screen::Results;
            }
            Err(e) => self.fail(e),
        }
    }

    /// The answer to [`Effect::Open`].
    pub fn opened(&mut self, generation: u64, r: Result<hf::Details>) {
        if generation != self.generation {
            return;
        }
        match r {
            Ok(details) => {
                let sel = hf::default_quant(&details).and_then(|d| details.quants.iter().position(|q| q == d)).unwrap_or(0);
                self.quants = Some(Quants { details, sel });
                self.screen = Screen::Quants;
            }
            Err(e) => self.fail(e),
        }
    }

    fn fail(&mut self, e: anyhow::Error) {
        let back = match &self.screen {
            Screen::Loading { back, .. } => back.clone(),
            s => Box::new(s.clone()),
        };
        self.screen = Screen::Error { message: format!("{e:#}"), back };
    }

    /// The screen as lines, and which one is selected (drawn inverted).
    pub fn render(&self, rows: usize) -> (Vec<String>, Option<usize>) {
        let rows = rows.max(6);
        let mut v = vec!["Hugging Face GGUF models (mouser model hf pick)".to_string(), String::new()];
        let mut sel_line = None;
        let list = |v: &mut Vec<String>, items: Vec<String>, sel: usize, room: usize, sel_line: &mut Option<usize>| {
            let room = room.max(1);
            let top = sel.saturating_sub(room - 1).min(items.len().saturating_sub(room));
            for (i, it) in items.iter().enumerate().skip(top).take(room) {
                if i == sel {
                    *sel_line = Some(v.len());
                }
                v.push(format!("{} {it}", if i == sel { "›" } else { " " }));
            }
        };
        let footer = match &self.screen {
            Screen::Query(t) => {
                v.push(format!("Search: {t}▏"));
                "Enter search · Esc back · Ctrl-C quit"
            }
            Screen::Loading { what, .. } => {
                v.push(what.clone());
                "Esc back · Ctrl-C quit"
            }
            Screen::Error { message, .. } => {
                v.push(format!("error: {message}"));
                "any key: back · Ctrl-C quit"
            }
            Screen::Results => {
                let r = self.results.as_ref().expect("results");
                if r.hits.is_empty() {
                    v.push(format!("no GGUF models match {:?}", r.query));
                } else {
                    v.push(format!("  {:<56} {:>16} {:>8} {:>7}  updated", "model", "download", "↓", "♥"));
                    let items = r.hits.iter().zip(&r.sizes).map(|(h, s)| hf::search_line(h, s)).collect();
                    list(&mut v, items, r.sel, rows - 5, &mut sel_line);
                }
                "↑↓ move · Enter quantizations · / new search · Esc quit"
            }
            Screen::Quants => {
                let q = self.quants.as_ref().expect("quants");
                let d = &q.details;
                v.push(format!(
                    "{}  ↓{} ♥{}{}",
                    d.id,
                    hf::count(d.downloads),
                    hf::count(d.likes),
                    if d.license.is_empty() { String::new() } else { format!("  license: {}", d.license) }
                ));
                if let hf::Support::No(why) = hf::support(d) {
                    v.push(format!("won't run: {why}"));
                }
                if d.gated {
                    v.push("gated: accept its terms on huggingface.co (gated downloads are not supported yet)".into());
                }
                if d.quants.is_empty() {
                    v.push("no GGUF quantizations".into());
                } else {
                    let def = hf::default_quant(d).map(|q| q.label.clone());
                    let items = d
                        .quants
                        .iter()
                        .map(|x| {
                            let size = x.known_size().map_or_else(|| "unknown".to_string(), mouser_core::models::local::human);
                            let parts = if x.files.len() > 1 { format!("  {} files", x.files.len()) } else { String::new() };
                            let mark = if def.as_deref() == Some(x.label.as_str()) { "  (default)" } else { "" };
                            format!("{:<12} {:>10}{parts}{mark}", x.label, size)
                        })
                        .collect();
                    let used = v.len();
                    list(&mut v, items, q.sel, rows.saturating_sub(used + 2), &mut sel_line);
                }
                "↑↓ move · Enter install (asks first) · Esc back · Ctrl-C quit"
            }
        };
        while v.len() < rows - 1 {
            v.push(String::new());
        }
        v.truncate(rows - 1);
        v.push(footer.to_string());
        (v, sel_line)
    }
}

fn moved(sel: usize, n: usize, k: &Key, page: usize) -> usize {
    if n == 0 {
        return 0;
    }
    match k {
        Key::Up | Key::Char('k') => sel.saturating_sub(1),
        Key::Down | Key::Char('j') => (sel + 1).min(n - 1),
        Key::PageUp => sel.saturating_sub(page),
        Key::PageDown => (sel + page).min(n - 1),
        Key::Home | Key::Char('g') => 0,
        Key::End | Key::Char('G') => n - 1,
        _ => sel,
    }
}

/// Cut to `cols` characters.
fn clip(s: &str, cols: usize) -> String {
    s.chars().take(cols).collect()
}

enum Fetched {
    Search(u64, Result<(String, Vec<hf::Hit>, Vec<hf::Sizes>)>),
    Open(u64, Result<hf::Details>),
}

/// Run the picker on this terminal. `Some((repo, quant))` when one was
/// chosen; `None` when left (Escape, `q`, Ctrl-C). Refuses without a terminal.
pub fn pick(query: Option<&str>) -> Result<Option<(String, String)>> {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        bail!("mouser model hf pick needs a terminal; in scripts use `mouser model hf search QUERY` and `mouser model hf install ID [QUANT]`");
    }
    let term = term::Raw::enter()?;
    let (mut p, mut effect) = Picker::new(query);
    let (tx, rx) = std::sync::mpsc::channel::<Fetched>();
    let mut out = std::io::stdout();
    let mut last_size = (0, 0);
    let mut dirty = true;
    loop {
        match std::mem::replace(&mut effect, Effect::None) {
            Effect::None => {}
            Effect::Cancel => return Ok(None),
            Effect::Install { id, quant } => {
                drop(term);
                return Ok(Some((id, quant)));
            }
            Effect::Search(q, g) => {
                let tx = tx.clone();
                // Detached: Escape or Ctrl-C doesn't wait for the Hub.
                std::thread::spawn(move || {
                    let r = hf::search(&q, hf::SEARCH_LIMIT).map(|hits| {
                        let ids: Vec<String> = hits.iter().map(|h| h.id.clone()).collect();
                        let sizes = hf::search_sizes(&ids, hf::SIZE_WORKERS, hf::SIZE_DEADLINE);
                        (q, hits, sizes)
                    });
                    let _ = tx.send(Fetched::Search(g, r));
                });
                dirty = true;
            }
            Effect::Open(id, g) => {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(Fetched::Open(g, hf::details(&id)));
                });
                dirty = true;
            }
        }
        let size = term::size();
        if dirty || size != last_size {
            last_size = size;
            let (rows, cols) = size;
            let (lines, sel) = p.render(rows);
            let mut s = String::from("\x1b[H");
            for (i, l) in lines.iter().enumerate() {
                let l = clip(l, cols);
                if Some(i) == sel {
                    s.push_str(&format!("\x1b[7m{l}\x1b[0m"));
                } else {
                    s.push_str(&l);
                }
                s.push_str("\x1b[K");
                if i + 1 < lines.len() {
                    s.push_str("\r\n");
                }
            }
            out.write_all(s.as_bytes())?;
            out.flush()?;
            dirty = false;
        }
        while let Ok(f) = rx.try_recv() {
            match f {
                Fetched::Search(g, r) => p.searched(g, r),
                Fetched::Open(g, r) => p.opened(g, r),
            }
            dirty = true;
        }
        let page = last_size.0.saturating_sub(6).max(1);
        for k in term::read_keys(100)? {
            effect = p.key(&k, page);
            dirty = true;
            if effect != Effect::None {
                break;
            }
        }
    }
}

#[cfg(unix)]
mod term {
    use anyhow::{Result, bail};
    use std::io::Write;

    /// Raw mode on the alternate screen until dropped (also on a panic).
    pub struct Raw(libc::termios);

    impl Raw {
        pub fn enter() -> Result<Raw> {
            // SAFETY: plain termios calls on the process's own stdin.
            unsafe {
                let mut t: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(libc::STDIN_FILENO, &mut t) != 0 {
                    bail!("can't read the terminal settings");
                }
                let saved = t;
                libc::cfmakeraw(&mut t);
                t.c_cc[libc::VMIN] = 1;
                t.c_cc[libc::VTIME] = 0;
                if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t) != 0 {
                    bail!("can't set the terminal to raw mode");
                }
                let raw = Raw(saved);
                let _ = std::io::stdout().write_all(b"\x1b[?1049h\x1b[?25l\x1b[2J");
                let _ = std::io::stdout().flush();
                Ok(raw)
            }
        }
    }

    impl Drop for Raw {
        fn drop(&mut self) {
            let _ = std::io::stdout().write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l");
            let _ = std::io::stdout().flush();
            // SAFETY: restores the settings read in `enter`.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.0);
            }
        }
    }

    /// (rows, cols) of the terminal on stdout.
    pub fn size() -> (usize, usize) {
        // SAFETY: TIOCGWINSZ fills a winsize.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0;
        if ok && ws.ws_row > 0 && ws.ws_col > 0 { (ws.ws_row as usize, ws.ws_col as usize) } else { (24, 80) }
    }

    /// Keys typed within `ms` (none on a timeout); end of input is Quit.
    pub fn read_keys(ms: i32) -> Result<Vec<super::Key>> {
        let mut fds = libc::pollfd { fd: libc::STDIN_FILENO, events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd for stdin.
        let n = unsafe { libc::poll(&mut fds, 1, ms) };
        if n <= 0 {
            return Ok(Vec::new());
        }
        let mut b = [0u8; 256];
        // SAFETY: reads into a local buffer.
        let got = unsafe { libc::read(libc::STDIN_FILENO, b.as_mut_ptr().cast(), b.len()) };
        if got <= 0 {
            return Ok(vec![super::Key::Quit]);
        }
        Ok(super::parse_keys(&b[..got as usize]))
    }
}

#[cfg(windows)]
mod term {
    use super::Key;
    use anyhow::{Result, bail};
    use std::io::Write;
    use windows_sys::Win32::System::Console::{
        CONSOLE_MODE, CONSOLE_SCREEN_BUFFER_INFO, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetConsoleScreenBufferInfo,
        GetNumberOfConsoleInputEvents, GetStdHandle, INPUT_RECORD, KEY_EVENT, ReadConsoleInputW, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode,
    };

    pub struct Raw {
        input: CONSOLE_MODE,
        output: CONSOLE_MODE,
    }

    impl Raw {
        pub fn enter() -> Result<Raw> {
            // SAFETY: console mode calls on the process's own handles.
            unsafe {
                let (i, o) = (GetStdHandle(STD_INPUT_HANDLE), GetStdHandle(STD_OUTPUT_HANDLE));
                let (mut im, mut om) = (0, 0);
                if GetConsoleMode(i, &mut im) == 0 || GetConsoleMode(o, &mut om) == 0 {
                    bail!("not a console");
                }
                // No line input, echo or Ctrl-C handling: keys come one by one.
                SetConsoleMode(i, 0);
                SetConsoleMode(o, om | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
                let _ = std::io::stdout().write_all(b"\x1b[?1049h\x1b[?25l\x1b[2J");
                let _ = std::io::stdout().flush();
                Ok(Raw { input: im, output: om })
            }
        }
    }

    impl Drop for Raw {
        fn drop(&mut self) {
            let _ = std::io::stdout().write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l");
            let _ = std::io::stdout().flush();
            // SAFETY: restores the modes read in `enter`.
            unsafe {
                SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), self.input);
                SetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), self.output);
            }
        }
    }

    pub fn size() -> (usize, usize) {
        // SAFETY: fills a plain struct for our own stdout.
        unsafe {
            let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
            if GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &mut info) != 0 {
                let r = (info.srWindow.Bottom - info.srWindow.Top + 1).max(1) as usize;
                let c = (info.srWindow.Right - info.srWindow.Left + 1).max(1) as usize;
                return (r, c);
            }
        }
        (24, 80)
    }

    pub fn read_keys(ms: i32) -> Result<Vec<Key>> {
        let end = std::time::Instant::now() + std::time::Duration::from_millis(ms.max(0) as u64);
        let mut keys = Vec::new();
        // SAFETY: reads console input records into a local buffer.
        unsafe {
            let h = GetStdHandle(STD_INPUT_HANDLE);
            loop {
                let mut n = 0u32;
                if GetNumberOfConsoleInputEvents(h, &mut n) == 0 {
                    return Ok(vec![Key::Quit]);
                }
                if n > 0 {
                    let mut recs: [INPUT_RECORD; 32] = std::mem::zeroed();
                    let mut got = 0u32;
                    if ReadConsoleInputW(h, recs.as_mut_ptr(), recs.len() as u32, &mut got) == 0 {
                        return Ok(vec![Key::Quit]);
                    }
                    for r in &recs[..got as usize] {
                        if r.EventType as u32 != KEY_EVENT as u32 {
                            continue;
                        }
                        let k = r.Event.KeyEvent;
                        if k.bKeyDown == 0 {
                            continue;
                        }
                        let ch = k.uChar.UnicodeChar;
                        keys.extend(match k.wVirtualKeyCode {
                            0x26 => Some(Key::Up),
                            0x28 => Some(Key::Down),
                            0x25 => Some(Key::Left),
                            0x21 => Some(Key::PageUp),
                            0x22 => Some(Key::PageDown),
                            0x24 => Some(Key::Home),
                            0x23 => Some(Key::End),
                            0x0d => Some(Key::Enter),
                            0x1b => Some(Key::Esc),
                            0x08 => Some(Key::Backspace),
                            _ if ch == 3 || ch == 4 => Some(Key::Quit),
                            _ if ch >= 0x20 => char::from_u32(ch as u32).map(Key::Char),
                            _ => None,
                        });
                    }
                    if !keys.is_empty() {
                        return Ok(keys);
                    }
                }
                if std::time::Instant::now() >= end {
                    return Ok(keys);
                }
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod term {
    pub struct Raw;
    impl Raw {
        pub fn enter() -> anyhow::Result<Raw> {
            anyhow::bail!("mouser model hf pick isn't supported on this platform")
        }
    }
    pub fn size() -> (usize, usize) {
        (24, 80)
    }
    pub fn read_keys(_: i32) -> anyhow::Result<Vec<super::Key>> {
        Ok(vec![super::Key::Quit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: &str) -> hf::Hit {
        hf::Hit { id: id.into(), downloads: 1500, likes: 7, pipeline: String::new(), updated: "2026-10-01".into() }
    }

    fn file(n: &str, size: u64) -> hf::HfFile {
        hf::HfFile { name: n.into(), size, sha256: Some("ab".repeat(32)) }
    }

    fn details() -> hf::Details {
        hf::Details {
            id: "acme/Tiny-GGUF".into(),
            architecture: "llama".into(),
            quants: hf::quants(&[
                file("t-Q8_0.gguf", 900_000_000),
                file("t-Q4_K_M.gguf", 500_000_000),
                file("t-F16-00001-of-00002.gguf", 1_000_000_000),
                file("t-F16-00002-of-00002.gguf", 1_000_000_000),
            ]),
            ..Default::default()
        }
    }

    #[test]
    fn keys_from_terminal_bytes() {
        use Key::*;
        assert_eq!(parse_keys(b"\x1b[A\x1b[B\x1bOA\x1b[5~\x1b[6~\x1b[H\x1b[F\x1b[D"), [Up, Down, Up, PageUp, PageDown, Home, End, Left]);
        assert_eq!(parse_keys(b"\x1b"), [Esc]);
        assert_eq!(parse_keys(b"\r\x03\x04\x7f"), [Enter, Quit, Quit, Backspace]);
        assert_eq!(parse_keys("qé/".as_bytes()), [Char('q'), Char('é'), Char('/')]);
        assert_eq!(parse_keys(b"\x1b[1;5C"), Vec::<Key>::new(), "unknown sequences are skipped");
    }

    #[test]
    fn query_to_results_to_quant_to_install() {
        let (mut p, e) = Picker::new(None);
        assert_eq!(e, Effect::None);
        for c in "tiny".chars() {
            p.key(&Key::Char(c), 10);
        }
        let Effect::Search(q, g) = p.key(&Key::Enter, 10) else { panic!() };
        assert_eq!(q, "tiny");
        let sizes = vec![hf::Sizes::Range { min: 500_000_000, max: 2_000_000_000, partial: false }, hf::Sizes::Unknown];
        p.searched(g, Ok((q, vec![hit("acme/Tiny-GGUF"), hit("acme/Other")], sizes)));
        assert_eq!(p.screen, Screen::Results);
        let (lines, sel) = p.render(20);
        let text = lines.join("\n");
        assert!(text.contains("0.50–2.00 GB") && text.contains("unknown"), "size column: {text}");
        assert!(lines[sel.unwrap()].contains("acme/Tiny-GGUF"));

        let Effect::Open(id, g) = p.key(&Key::Enter, 10) else { panic!() };
        assert_eq!(id, "acme/Tiny-GGUF");
        p.opened(g, Ok(details()));
        let (lines, sel) = p.render(20);
        assert!(lines[sel.unwrap()].contains("Q4_K_M") && lines[sel.unwrap()].contains("(default)"), "starts on the default: {lines:?}");
        assert!(lines.iter().any(|l| l.contains("F16") && l.contains("2.00 GB") && l.contains("2 files")), "split sizes summed: {lines:?}");
        p.key(&Key::Down, 10);
        assert_eq!(p.key(&Key::Enter, 10), Effect::Install { id: "acme/Tiny-GGUF".into(), quant: "Q8_0".into() });
    }

    #[test]
    fn escape_goes_back_and_ctrl_c_leaves_from_anywhere() {
        let (mut p, e) = Picker::new(Some("tiny"));
        let Effect::Search(q, g) = e else { panic!() };
        // Escape while searching: back to the query, and the late answer is dropped.
        p.key(&Key::Esc, 10);
        assert_eq!(p.screen, Screen::Query("tiny".into()));
        p.searched(g, Ok((q.clone(), vec![hit("a/b")], vec![hf::Sizes::Unknown])));
        assert_eq!(p.screen, Screen::Query("tiny".into()));
        assert_eq!(p.key(&Key::Esc, 10), Effect::Cancel, "Esc on the first query leaves");

        let (mut p, Effect::Search(q, g)) = Picker::new(Some("tiny")) else { panic!() };
        p.searched(g, Ok((q, vec![hit("acme/Tiny-GGUF")], vec![hf::Sizes::Unknown])));
        let Effect::Open(_, g) = p.key(&Key::Enter, 10) else { panic!() };
        p.opened(g, Ok(details()));
        assert_eq!(p.key(&Key::Esc, 10), Effect::None);
        assert_eq!(p.screen, Screen::Results, "Esc on quantizations: back to results");
        assert_eq!(p.key(&Key::Char('/'), 10), Effect::None);
        assert_eq!(p.screen, Screen::Query("tiny".into()));
        assert_eq!(p.key(&Key::Esc, 10), Effect::None);
        assert_eq!(p.screen, Screen::Results, "Esc on a new query: back to the results");
        assert_eq!(p.key(&Key::Quit, 10), Effect::Cancel);
    }

    #[test]
    fn errors_are_shown_and_any_key_goes_back() {
        let (mut p, Effect::Search(_, g)) = Picker::new(Some("x")) else { panic!() };
        p.searched(g, Err(anyhow::anyhow!("searching Hugging Face: offline")));
        assert!(p.render(10).0.iter().any(|l| l.contains("error: searching Hugging Face: offline")));
        p.key(&Key::Char('z'), 10);
        assert_eq!(p.screen, Screen::Query("x".into()));
    }

    #[test]
    fn long_lists_scroll_with_the_selection() {
        let (mut p, Effect::Search(q, g)) = Picker::new(Some("x")) else { panic!() };
        let hits: Vec<_> = (0..50).map(|i| hit(&format!("o/m{i}"))).collect();
        p.searched(g, Ok((q, hits, vec![hf::Sizes::Unknown; 50])));
        p.key(&Key::End, 10);
        let (lines, sel) = p.render(12);
        assert_eq!(lines.len(), 12);
        assert!(lines[sel.unwrap()].contains("o/m49"), "{lines:?}");
        p.key(&Key::PageUp, 10);
        p.key(&Key::Up, 10);
        assert_eq!(p.results.as_ref().unwrap().sel, 38);
        p.key(&Key::Home, 10);
        assert_eq!(p.results.as_ref().unwrap().sel, 0);
    }
}
