//! Tabs: each one owns a PTY, its process, terminal state and scrollback.

use crate::SharedWriter;
use mouser_core::parser::{TermEvent, TerminalParser};
use mouser_core::scrollback::{Scrollback, ScrollbackConfig};
use mouser_core::types::Size;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub type Parser = TerminalParser<SharedWriter>;

/// What a tab runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// The user's login shell.
    Shell,
    /// A connection: argv run directly as the tab's process.
    Command(Vec<String>),
}

pub struct Session {
    pub id: u64,
    pub parser: Arc<Mutex<Parser>>,
    pub scrollback: Arc<Scrollback>,
    pub writer: SharedWriter,
    /// Title when the program sets none (connection name or "shell").
    pub name: String,
    pub launch: Launch,
    master: Mutex<Option<Box<dyn mouser_pty::MasterPty + Send>>>,
    child: Mutex<Option<Box<dyn mouser_pty::Child + Send>>>,
    /// Exit code once the process ended and the tab is kept open to show it.
    pub exited: Mutex<Option<u32>>,
    /// The tab should be removed.
    pub closed: AtomicBool,
    /// A timer is waiting to expire an abandoned Fuzzball upload.
    upload_timer: AtomicBool,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Whether a finished tab closes by itself: shells always, connections only
/// when they exit cleanly (so an ssh error stays readable).
pub fn closes_on_exit(launch: &Launch, code: u32) -> bool {
    matches!(launch, Launch::Shell) || code == 0
}

pub fn exit_banner(code: u32) -> String {
    format!("\r\n\x1b[0m[process exited {code}] press any key to close")
}

impl Session {
    pub fn new(name: impl Into<String>, launch: Launch, size: Size, sb: ScrollbackConfig) -> Arc<Self> {
        let scrollback = Arc::new(Scrollback::new(sb));
        let writer = SharedWriter(Arc::new(Mutex::new(None)));
        let parser = TerminalParser::new(size, scrollback.clone(), writer.clone());
        Arc::new(Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            parser: Arc::new(Mutex::new(parser)),
            scrollback,
            writer,
            name: name.into(),
            launch,
            master: Mutex::new(None),
            child: Mutex::new(None),
            exited: Mutex::new(None),
            closed: AtomicBool::new(false),
            upload_timer: AtomicBool::new(false),
        })
    }

    pub fn started(&self) -> bool {
        self.master.lock().unwrap().is_some()
    }

    pub fn title(&self) -> String {
        let t = self.parser.lock().unwrap().title.clone();
        if t.trim().is_empty() { self.name.clone() } else { t }
    }

    /// Spawn the process at the given size; output is fed on a reader thread.
    pub fn start(self: &Arc<Self>, cols: u16, rows: u16, pw: u16, ph: u16, wake: fn()) {
        let spawned = match &self.launch {
            Launch::Shell => mouser_pty::spawn_shell(cols, rows, pw, ph),
            Launch::Command(argv) => mouser_pty::spawn_argv(argv, cols, rows, pw, ph),
        };
        let handles = match spawned {
            Ok(h) => h,
            Err(e) => {
                log::error!("failed to start {:?}: {e}", self.launch);
                self.finish(127);
                wake();
                return;
            }
        };
        *self.writer.0.lock().unwrap() = Some(handles.writer);
        *self.child.lock().unwrap() = Some(handles.child);
        *self.master.lock().unwrap() = Some(handles.master);
        let me = self.clone();
        let mut reader = handles.reader;
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        me.feed(&buf[..n]);
                        me.watch_upload();
                        wake();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            // EOF / EIO: the process exited.
            let code = me.child.lock().unwrap().as_mut().and_then(|c| c.wait().ok()).map(|s| s.exit_code()).unwrap_or(0);
            me.finish(code);
            wake();
        });
    }

    /// While a chunked Fuzzball upload is pending, keep one timer thread that drops
    /// it once it has been idle for `UPLOAD_TIMEOUT` (frees its buffer).
    fn watch_upload(self: &Arc<Self>) {
        if !self.parser.lock().unwrap().fuzzball.upload_pending() || self.upload_timer.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            loop {
                let next = me.parser.lock().unwrap().fuzzball.expire_upload(std::time::Instant::now());
                match next {
                    Some(t) => std::thread::sleep(t.saturating_duration_since(std::time::Instant::now())),
                    None => break,
                }
            }
            me.upload_timer.store(false, Ordering::Release);
            // An upload may have started between the last check and clearing the flag.
            me.watch_upload();
        });
    }

    pub fn feed(&self, bytes: &[u8]) {
        let events = {
            let mut p = self.parser.lock().unwrap();
            p.feed(bytes);
            std::mem::take(&mut p.events)
        };
        self.scrollback.maybe_compress_idle();
        for ev in events {
            if let TermEvent::Clipboard(text) = ev {
                crate::set_clipboard(&text);
            }
        }
    }

    /// The process ended with `code`: close, or keep the tab with a banner.
    pub fn finish(&self, code: u32) {
        *self.writer.0.lock().unwrap() = None;
        if closes_on_exit(&self.launch, code) {
            self.closed.store(true, Ordering::SeqCst);
        } else {
            *self.exited.lock().unwrap() = Some(code);
            self.parser.lock().unwrap().feed(exit_banner(code).as_bytes());
        }
    }

    pub fn resize(&self, cols: u16, rows: u16, pw: u16, ph: u16) {
        self.parser.lock().unwrap().resize(Size { cols: cols as usize, rows: rows as usize });
        if let Some(m) = self.master.lock().unwrap().as_mut() {
            let _ = mouser_pty::set_master_winsize(&mut **m, cols, rows, pw, ph);
        }
    }

    /// Kill and reap the process (tab closed by the user or window closing).
    pub fn kill(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(mut c) = self.child.lock().unwrap().take()
            && c.try_wait().ok().flatten().is_none()
        {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// The open tabs and which one is shown.
#[derive(Default)]
pub struct Tabs {
    pub list: Vec<Arc<Session>>,
    pub active: usize,
    /// Last grid size (cols, rows, px w, px h); new tabs start at it.
    pub size: Option<(u16, u16, u16, u16)>,
}

impl Tabs {
    pub fn active(&self) -> Option<Arc<Session>> {
        self.list.get(self.active).cloned()
    }

    /// Add a tab and switch to it. It starts now if the grid size is known.
    pub fn open(&mut self, s: Arc<Session>, wake: fn()) {
        if let Some((c, r, pw, ph)) = self.size {
            s.parser.lock().unwrap().resize(Size { cols: c as usize, rows: r as usize });
            s.start(c, r, pw, ph, wake);
        }
        self.list.push(s);
        self.active = self.list.len() - 1;
    }

    pub fn select(&mut self, i: usize) {
        if i < self.list.len() {
            self.active = i;
        }
    }

    pub fn cycle(&mut self, d: i32) {
        let n = self.list.len() as i32;
        if n > 0 {
            self.active = (self.active as i32 + d).rem_euclid(n) as usize;
        }
    }

    pub fn close(&mut self, i: usize) {
        if let Some(s) = self.list.get(i) {
            s.kill();
        }
        self.reap();
    }

    /// Drop closed tabs, keeping the selection sensible. True when none are left.
    pub fn reap(&mut self) -> bool {
        let before = self.list.len();
        let active_id = self.list.get(self.active).map(|s| s.id);
        let removed_before_active = self.list.iter().take(self.active).filter(|s| s.closed.load(Ordering::SeqCst)).count();
        self.list.retain(|s| !s.closed.load(Ordering::SeqCst));
        if self.list.len() != before {
            let still = active_id.and_then(|id| self.list.iter().position(|s| s.id == id));
            self.active = match still {
                Some(i) => i,
                // The active tab closed: show its left neighbour (or the first).
                None => self.active.saturating_sub(removed_before_active).saturating_sub(1).min(self.list.len().saturating_sub(1)),
            };
        }
        self.list.is_empty()
    }

    pub fn resize(&mut self, cols: u16, rows: u16, pw: u16, ph: u16, wake: fn()) {
        self.size = Some((cols, rows, pw, ph));
        for s in &self.list {
            if s.started() || s.exited.lock().unwrap().is_some() {
                s.resize(cols, rows, pw, ph);
            } else {
                s.parser.lock().unwrap().resize(Size { cols: cols as usize, rows: rows as usize });
                s.start(cols, rows, pw, ph, wake);
            }
        }
    }

    pub fn bar(&self) -> mouser_render::ui::TabBar {
        mouser_render::ui::TabBar { titles: self.list.iter().map(|s| s.title()).collect(), active: self.active }
    }
}

/// Write to the active tab's PTY.
#[derive(Clone)]
pub struct ActiveWriter(pub Arc<Mutex<Tabs>>);

impl Write for ActiveWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let s = self.0.lock().unwrap().active();
        match s {
            Some(s) => s.writer.clone().write(buf),
            None => Ok(buf.len()),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
