//! The plugin side: implement [`Mode`] and call [`serve`] from `main`. It
//! sends the `Hello`, waits for the `Welcome`, maps the shared-memory ring
//! and then feeds every event to the mode on one thread, in order. The
//! [`Host`] handle sends views, effects, emits and requests; it can be
//! cloned into worker threads (a worker that emits an event gets it back
//! through the mode's own listener, in order with input).

use crate::codec::{self, Wire};
use crate::events::{Delivery, Event, Listener};
use crate::host_api::Call;
use crate::mode::Effect;
use crate::protocol::{Accepted, Handled, Hello, ImageRef, In, LogLevel, Out, Welcome};
use crate::shm::Ring;
use serde_json::Value;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A mode plugin. Only [`Mode::hello`] is required.
pub trait Mode {
    /// The first message: name, version, what it provides, its listeners.
    fn hello(&self) -> Hello;
    /// Accepted by Mouser: the granted permissions, drawing area and, after
    /// a crash, the state it saved ([`Host::save_state`]).
    fn welcome(&mut self, _w: &Accepted, _host: &Host) {}
    /// An event for one of its listeners. The answer only matters for sync
    /// listeners (`Pass` lets the key go to the shell).
    fn event(&mut self, _ev: &Event, _host: &Host) -> Handled {
        Handled::Pass
    }
    /// The answer to [`Host::request`] number `id`.
    fn response(&mut self, _id: u64, _result: Result<Value, String>, _host: &Host) {}
    /// Mouser is about to stop it: 500 ms to save state.
    fn stopping(&mut self, _reason: &str, _host: &Host) {}
}

struct Shared {
    out: Mutex<Box<dyn Write + Send>>,
    wire: Wire,
    next_id: AtomicU64,
    ring: Mutex<Option<Ring>>,
}

/// The connection to Mouser, cheap to clone.
#[derive(Clone)]
pub struct Host(Arc<Shared>);

impl Host {
    fn send(&self, m: &Out) -> io::Result<()> {
        let mut w = self.0.out.lock().unwrap_or_else(|e| e.into_inner());
        codec::write(&mut *w, self.0.wire, m)
    }

    /// Show this view (the latest one wins). Panels use the mode's own
    /// [`crate::ui::Action`]s.
    pub fn view(&self, dock: Option<crate::ui::Panel>, panel: Option<crate::ui::Panel>, images: Vec<ImageRef>, fullscreen: bool) -> io::Result<()> {
        self.send(&Out::View { dock, panel, images, fullscreen })
    }

    /// What the mode takes from the keyboard now ([`crate::protocol::Wants`]).
    /// Show a context menu at (`x`, `y`) (API 0.2; answer to `menu.open`).
    pub fn menu(&self, x: i32, y: i32, entries: Vec<crate::ui::MenuEntry>) -> io::Result<()> {
        self.send(&Out::Menu { x, y, entries })
    }

    /// The views and players Mouser runs for this mode (latest wins); only
    /// when the `Welcome` offered them ([`Accepted::has`], mode-api 0.4).
    pub fn hosted(&self, views: Vec<crate::hosted::HostedView>) -> io::Result<()> {
        self.send(&Out::Hosted(views))
    }

    pub fn wants(&self, w: crate::protocol::Wants) -> io::Result<()> {
        self.send(&Out::Wants(w))
    }

    pub fn effect(&self, e: Effect) -> io::Result<()> {
        self.send(&Out::Effect(e))
    }

    /// Emit one of the plugin's declared events.
    pub fn emit(&self, name: &str, payload: Value) -> io::Result<()> {
        self.send(&Out::Emit { name: name.into(), payload })
    }

    /// Call the host; the answer comes to [`Mode::response`] with this id.
    pub fn request(&self, call: Call) -> io::Result<u64> {
        let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(&Out::Request { id, call })?;
        Ok(id)
    }

    /// Kept by Mouser and handed back in the `Welcome` after a crash.
    pub fn save_state(&self, blob: Vec<u8>) -> io::Result<()> {
        self.send(&Out::StateSave(blob))
    }

    pub fn log(&self, level: LogLevel, text: impl Into<String>) -> io::Result<()> {
        self.send(&Out::Log { level, text: text.into() })
    }

    pub fn subscribe(&self, l: Listener) -> io::Result<()> {
        self.send(&Out::Subscribe(l))
    }

    /// Write into shared-memory slot `slot` (`None`: there's no ring, send
    /// pixels inline). Returns what `f` returned.
    pub fn with_slot<R>(&self, slot: u32, f: impl FnOnce(&mut [u8]) -> R) -> Option<R> {
        let mut r = self.0.ring.lock().unwrap_or_else(|e| e.into_inner());
        r.as_mut().and_then(|r| r.slot_mut(slot)).map(f)
    }

    pub fn has_ring(&self) -> bool {
        self.0.ring.lock().map(|r| r.is_some()).unwrap_or(false)
    }
}

/// Run `mode` on this process's stdin and stdout until Mouser stops it.
pub fn serve(mode: impl Mode) -> io::Result<()> {
    serve_on(mode, BufReader::new(io::stdin()), BufWriter::new(io::stdout()), Wire::from_env())
}

/// [`serve`] over any streams (tests, sockets).
pub fn serve_on(mut mode: impl Mode, mut input: impl Read, output: impl Write + Send + 'static, wire: Wire) -> io::Result<()> {
    let hello = mode.hello();
    let sync: Vec<Listener> = hello.listeners.iter().filter(|l| matches!(l.delivery, Delivery::Sync { .. })).cloned().collect();
    let host = Host(Arc::new(Shared { out: Mutex::new(Box::new(output)), wire, next_id: AtomicU64::new(1), ring: Mutex::new(None) }));
    host.send(&Out::Hello(hello))?;
    let mut buf = Vec::new();
    let accepted = match codec::read::<Welcome>(&mut input, wire, &mut buf)? {
        Some(Welcome::Accepted(a)) => a,
        Some(Welcome::Rejected { reason }) => return Err(io::Error::new(io::ErrorKind::ConnectionRefused, reason)),
        None => return Ok(()),
    };
    if let Some(spec) = &accepted.shm {
        *host.0.ring.lock().unwrap_or_else(|e| e.into_inner()) = Ring::open(spec).ok();
    }
    mode.welcome(&accepted, &host);
    while let Some(m) = codec::read::<In>(&mut input, wire, &mut buf)? {
        match m {
            In::Event(ev) => {
                let answer = mode.event(&ev, &host);
                if sync.iter().any(|l| l.matches(&ev.name)) {
                    host.send(&Out::Reply { seq: ev.seq, result: answer })?;
                }
            }
            In::Response { id, result } => mode.response(id, result, &host),
            In::Stopping { reason } => {
                mode.stopping(&reason, &host);
                break;
            }
            _ => {}
        }
    }
    Ok(())
}
