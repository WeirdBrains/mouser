//! Mouser's commander mode as a plugin program (`transport = "mode"`,
//! docs/commander.md). The view and model are the ones the built-in mode
//! used (`view.rs`, `commander.rs`); this file adapts them to the mode
//! protocol: Mouser's events become calls, every redraw is sent as a view,
//! and what the view takes from the keyboard is sent as `Wants`.
//!
//! Commander lists and previews the shell's folder itself (it runs as you,
//! like any program you start) and starts ffmpeg / ffprobe for video.
//! Typing into the shell goes through Mouser (`shell.type`).

// The modules are copies of what the built-in mode used (mouser-core's
// commander, fetch, remote, modes; mouser-render's view and player); parts
// only the host used are unused here until the built-in mode goes.
#![allow(dead_code)]

mod audio;
mod commander;
mod fetch;
mod host;
mod menu;
mod modes;
mod ops;
mod player;
mod process;
mod remote;
mod storage;
mod view;

use host::{Effect, Mode as _};
use mouser_mode_api::events::{Event, Listener};
use mouser_mode_api::protocol::{Accepted, ApiRange, BindingDecl, Handled, Hello, ImageFormat, ImageRef, Pixels, Provide, Wants};
use mouser_mode_api::{Context, Host, serve};
use serde_json::json;
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// The commander and what was last sent to Mouser.
struct State {
    mode: view::CommanderMode,
    keys: modes::Keymap,
    ctx: Option<Context>,
    /// Image keys in the last view: Mouser still has their pixels.
    sent: HashSet<u64>,
    wants: Option<Wants>,
    active: bool,
    /// Shared-memory slots Mouser hasn't copied yet (`frame.presented`
    /// frees one); a slot is written only when free.
    busy: Vec<bool>,
    /// Item rectangles in the last view (index), for a right-click.
    items: Vec<(mouser_mode_api::Rect, i32)>,
    /// File changes on Mouser's approval card, by request id.
    asked: Vec<(u64, ops::Op)>,
}

/// Images this big go through shared memory (player frames, large
/// previews); smaller ones inline.
const SHM_MIN: usize = 256 << 10;

impl State {
    fn new() -> Self {
        let (keys, _) = modes::Keymap::load("commander", commander::BINDINGS);
        Self {
            mode: view::CommanderMode::new(),
            keys,
            ctx: None,
            sent: HashSet::new(),
            wants: None,
            active: false,
            busy: Vec::new(),
            items: Vec::new(),
            asked: Vec::new(),
        }
    }

    /// Draw and send the view, then what the keyboard should bring.
    fn redraw(&mut self, host: &Host) {
        let Some(ctx) = self.ctx else { return };
        if !self.active {
            return;
        }
        let v = self.mode.view(&ctx, &self.keys);
        // Big new images take free shared-memory slots. When the slots are
        // all still waiting for Mouser, wait for it (`frame.presented` draws
        // again) rather than queue frames: the player drops, Mouser doesn't lag.
        let big = v.images.iter().filter(|i| !self.sent.contains(&i.key) && i.rgba.len() >= SHM_MIN).count();
        let slot_bytes = host.with_slot(0, |s| s.len()).unwrap_or(0);
        let ring = slot_bytes > 0 && !self.busy.is_empty();
        let free: Vec<u32> = (0..self.busy.len() as u32).filter(|&s| !self.busy[s as usize]).collect();
        if ring && big > 0 && big <= self.busy.len() && free.len() < big {
            return;
        }
        let mut free = free.into_iter();
        let mut sent = HashSet::new();
        let images = v
            .images
            .into_iter()
            .map(|i| {
                let n = i.rgba.len();
                let pixels = if self.sent.contains(&i.key) || sent.contains(&i.key) {
                    Pixels::Cached
                } else {
                    let slot = if ring && n >= SHM_MIN && n <= slot_bytes { free.next() } else { None };
                    match slot.and_then(|s| host.with_slot(s, |b| b[..n].copy_from_slice(&i.rgba)).map(|_| s)) {
                        Some(s) => {
                            self.busy[s as usize] = true;
                            Pixels::Slot(s)
                        }
                        None => Pixels::Inline { format: ImageFormat::Rgba, bytes: i.rgba.to_vec() },
                    }
                };
                sent.insert(i.key);
                ImageRef { key: i.key, width: i.width, height: i.height, src: i.src, rect: i.rect, pixels }
            })
            .collect();
        self.sent = sent;
        self.items = v
            .dock
            .iter()
            .flat_map(|d| &d.items)
            .filter_map(|i| match &i.action {
                mouser_mode_api::Action::Mode { action, arg } if action == "item" => Some((i.rect, *arg)),
                _ => None,
            })
            .collect();
        let _ = host.view(v.dock, v.panel, images, v.fullscreen);
        let w = Wants { text: self.mode.wants_text(), keys: self.mode.taken_keys(), escape: self.mode.escape_takes() };
        if self.wants.as_ref() != Some(&w) {
            let _ = host.wants(w.clone());
            self.wants = Some(w);
        }
    }

    /// Send what the view queued: file changes go to Mouser's approval
    /// card (they run on yes), text to the clipboard.
    fn flush(&mut self, host: &Host) {
        use mouser_mode_api::host_api::Call;
        for op in std::mem::take(&mut self.mode.ops) {
            let (title, detail) = op.describe();
            if let Ok(id) = host.request(Call::Approve { title, detail, options: vec!["once".into(), "deny".into()] }) {
                self.asked.push((id, op));
            }
        }
        for t in std::mem::take(&mut self.mode.clip) {
            let _ = host.request(Call::ClipboardSet(t));
            let _ = host.effect(Effect::Say("copied".into()));
        }
    }

    /// Mouser's answer to an approval card: run the change on yes.
    fn approved(&mut self, id: u64, r: &Result<serde_json::Value, String>, host: &Host) {
        let Some(i) = self.asked.iter().position(|(a, _)| *a == id) else { return };
        let (_, op) = self.asked.remove(i);
        let msg = match r {
            Ok(v) if v["decision"] == "once" => match op.run() {
                Ok(m) => m,
                Err(e) => format!("not done: {e}"),
            },
            Ok(_) => "cancelled".into(),
            Err(e) => format!("not done: {e}"),
        };
        self.mode.c.relist();
        let _ = host.effect(Effect::Say(msg));
    }

    /// The context menu at (`x`, `y`) (`< 0`: the selected item, from the keyboard).
    fn open_menu(&mut self, x: i32, y: i32, host: &Host) {
        let item = if x < 0 {
            let n = self.mode.c.visible().len();
            (self.mode.c.selected < n).then_some(self.mode.c.selected)
        } else {
            self.items.iter().find(|(r, _)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h).map(|(_, i)| *i as usize)
        };
        if let Some(i) = item {
            self.mode.c.selected = i;
        }
        let entries = self.mode.menu(item, &self.keys);
        let (x, y) = if x < 0 {
            let r = self.items.iter().find(|(_, i)| Some(*i as usize) == item).map(|(r, _)| *r);
            let a = self.ctx.map(|c| c.area).unwrap_or_default();
            r.map_or((a.x + a.w / 3, a.y + a.h / 3), |r| (r.x + r.w / 2, r.y + r.h))
        } else {
            (x, y)
        };
        let _ = host.menu(x, y, entries);
    }

    /// Carry out what the view asked. `_key`: the bound key this came from.
    fn apply(&mut self, e: Effect, _key: Option<&str>, host: &Host) {
        match e {
            Effect::None => {}
            // Not taken: the host has given the mode the keyboard, so the
            // key goes nowhere (never typed into the shell).
            Effect::Pass => {}
            e => {
                let _ = host.effect(e);
            }
        }
    }
}

/// Redraws asked for by the view's own threads (thumbnails decoded, the
/// player's clock): coalesced, at most one per 8 ms.
struct Redraw {
    want: Mutex<bool>,
    cv: Condvar,
}

struct Plugin {
    state: Arc<Mutex<State>>,
    redraw: Arc<Redraw>,
}

impl Plugin {
    fn new() -> Self {
        Self { state: Arc::new(Mutex::new(State::new())), redraw: Arc::new(Redraw { want: Mutex::new(false), cv: Condvar::new() }) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn start_redraws(&self, host: &Host) {
        let r = self.redraw.clone();
        let waker: host::Waker = Arc::new(move || {
            *r.want.lock().unwrap_or_else(|e| e.into_inner()) = true;
            r.cv.notify_one();
        });
        self.lock().mode.set_waker(waker);
        let (r, state, host) = (self.redraw.clone(), self.state.clone(), host.clone());
        std::thread::spawn(move || {
            let mut last = Instant::now();
            loop {
                {
                    let mut w = r.want.lock().unwrap_or_else(|e| e.into_inner());
                    while !*w {
                        w = r.cv.wait(w).unwrap_or_else(|e| e.into_inner());
                    }
                    *w = false;
                }
                let gap = Duration::from_millis(8).saturating_sub(last.elapsed());
                std::thread::sleep(gap);
                last = Instant::now();
                state.lock().unwrap_or_else(|e| e.into_inner()).redraw(&host);
            }
        });
    }
}

impl mouser_mode_api::Mode for Plugin {
    fn hello(&self) -> Hello {
        Hello {
            api: ApiRange::CURRENT,
            name: "commander".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            provides: vec![Provide::Mode {
                hotkey: 'c',
                bindings: commander::BINDINGS
                    .iter()
                    .map(|b| BindingDecl { id: b.id.into(), key: b.key.into(), describe: b.describe.into() })
                    .collect(),
                activate_on: vec![],
            }],
            listeners: [
                "mode.*",
                "ui.action",
                "input.text",
                "input.escape",
                "view.resized",
                "menu.open",
                "shell.cwd_changed",
                "shell.remote_ssh",
                "shell.remote_changed",
                "frame.presented",
            ]
            .iter()
            .map(|e| Listener::on(e))
            .collect(),
            emits: vec![],
            wants_mcp: false,
            shm: true,
        }
    }

    fn welcome(&mut self, w: &Accepted, host: &Host) {
        let mut s = self.lock();
        s.ctx = Some(w.ctx);
        s.busy = vec![false; w.shm.as_ref().map_or(0, |r| r.slots as usize)];
        drop(s);
        self.start_redraws(host);
    }

    fn event(&mut self, e: &Event, host: &Host) -> Handled {
        let mut s = self.lock();
        match e.name.as_str() {
            "mode.activated" => {
                s.active = true;
                s.mode.activate();
            }
            "mode.deactivated" => {
                s.mode.deactivate();
                s.active = false;
                s.sent.clear();
            }
            "shell.cwd_changed" => {
                if let Some(d) = e.payload["dir"].as_str() {
                    s.mode.cwd_changed(Path::new(d));
                }
            }
            "frame.presented" => {
                let slot = e.payload["slot"].as_u64().unwrap_or(u64::MAX) as usize;
                if let Some(b) = s.busy.get_mut(slot) {
                    *b = false;
                }
            }
            "shell.remote_ssh" => {
                let t = e.payload.get("dest").and_then(|d| d.as_str()).map(|dest| remote::SshTarget {
                    opts: e.payload["opts"].as_array().into_iter().flatten().filter_map(|o| o.as_str().map(String::from)).collect(),
                    dest: dest.to_string(),
                });
                s.mode.remote_ssh(t.as_ref());
            }
            "shell.remote_changed" => {
                let p = &e.payload;
                let listing = match &p["listing"] {
                    v if v.is_null() => None,
                    v => match v["error"].as_str() {
                        Some(err) => Some(Err(err.to_string())),
                        None => Some(Ok(v["entries"].as_array().into_iter().flatten().map(entry).collect())),
                    },
                };
                let dir = p["dir"].as_str().map(std::path::PathBuf::from);
                s.mode.remote_changed(p["label"].as_str().unwrap_or(""), dir.as_deref(), listing);
            }
            "ui.action" => {
                let id = e.payload["id"].as_str().unwrap_or("").to_string();
                let arg = e.payload["arg"].as_i64().unwrap_or(0) as i32;
                let eff = s.mode.action(&id, arg);
                let key = s.keys.keys.iter().find(|k| k.0 == id).map(|k| k.1.clone());
                s.apply(eff, key.as_deref(), host);
            }
            "input.text" => {
                let t = e.payload["utf8"].as_str().unwrap_or("").to_string();
                match s.mode.text(t.as_bytes()) {
                    Some(eff) => s.apply(eff, None, host),
                    // Not taken after all: it goes on to the shell.
                    None => s.apply(Effect::Type(t), None, host),
                }
            }
            "input.escape" => {
                s.mode.escape();
            }
            "menu.open" => {
                let (x, y) = (e.payload["x"].as_i64().unwrap_or(-1) as i32, e.payload["y"].as_i64().unwrap_or(-1) as i32);
                s.open_menu(x, y, host);
            }
            "view.resized" => {
                s.mode.dock = mouser_mode_api::DockSize::from_payload(&e.payload);
                if let Ok(c) = serde_json::from_value::<Context>(json!({"viewport": e.payload["viewport"], "area": e.payload["area"]})) {
                    s.ctx = Some(c);
                }
            }
            _ => return Handled::Pass,
        }
        s.flush(host);
        s.redraw(host);
        Handled::Handled
    }

    fn response(&mut self, id: u64, result: Result<serde_json::Value, String>, host: &Host) {
        let mut s = self.lock();
        s.approved(id, &result, host);
        s.redraw(host);
    }
}

/// A remote listing entry as Mouser sends it.
fn entry(v: &serde_json::Value) -> commander::Entry {
    use commander::Kind;
    let kind = match v["kind"].as_str().unwrap_or("") {
        "dir" => Kind::Dir,
        "image" => Kind::Image,
        "video" => Kind::Video,
        "audio" => Kind::Audio,
        "text" => Kind::Text,
        _ => Kind::Other,
    };
    commander::Entry {
        name: v["name"].as_str().unwrap_or("").into(),
        path: v["path"].as_str().unwrap_or("").into(),
        kind,
        mode: v["mode"].as_str().unwrap_or("").into(),
        size: v["size"].as_u64().unwrap_or(0),
        modified: v["modified"].as_f64().map(|s| std::time::UNIX_EPOCH + Duration::from_secs_f64(s.max(0.0))),
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--manifest") {
        print!("{}", include_str!("../plugin.toml"));
        return;
    }
    if let Err(e) = serve(Plugin::new()) {
        eprintln!("mouser-commander: {e}");
        std::process::exit(1);
    }
}
