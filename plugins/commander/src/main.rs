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

mod commander;
mod fetch;
mod host;
mod modes;
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
}

impl State {
    fn new() -> Self {
        let (keys, _) = modes::Keymap::load("commander", commander::BINDINGS);
        Self { mode: view::CommanderMode::new(), keys, ctx: None, sent: HashSet::new(), wants: None, active: false }
    }

    /// Draw and send the view, then what the keyboard should bring.
    fn redraw(&mut self, host: &Host) {
        let Some(ctx) = self.ctx else { return };
        if !self.active {
            return;
        }
        let v = self.mode.view(&ctx, &self.keys);
        let mut sent = HashSet::new();
        let images = v
            .images
            .into_iter()
            .map(|i| {
                let pixels = if self.sent.contains(&i.key) || sent.contains(&i.key) {
                    Pixels::Cached
                } else {
                    Pixels::Inline { format: ImageFormat::Rgba, bytes: i.rgba.to_vec() }
                };
                sent.insert(i.key);
                ImageRef { key: i.key, width: i.width, height: i.height, src: i.src, rect: i.rect, pixels }
            })
            .collect();
        self.sent = sent;
        let _ = host.view(v.dock, v.panel, images, v.fullscreen);
        let w = Wants { text: self.mode.wants_text(), keys: self.mode.taken_keys(), escape: self.mode.escape_takes() };
        if self.wants.as_ref() != Some(&w) {
            let _ = host.wants(w.clone());
            self.wants = Some(w);
        }
    }

    /// Carry out what the view asked. `key`: the bound key this came from,
    /// typed into the shell when the mode passes on it (as the built-in
    /// mode let it through).
    fn apply(&mut self, e: Effect, key: Option<&str>, host: &Host) {
        match e {
            Effect::None => {}
            Effect::Pass => {
                // What alt+key sends (its first encoding).
                if let Some(b) = key.and_then(modes::key_bytes).and_then(|v| v.into_iter().next()) {
                    let _ = host.effect(Effect::Type(String::from_utf8_lossy(&b).into_owned()));
                }
            }
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
            listeners: ["mode.*", "ui.action", "input.text", "input.escape", "view.resized", "shell.cwd_changed"]
                .iter()
                .map(|e| Listener::on(e))
                .collect(),
            emits: vec![],
            wants_mcp: false,
            shm: false,
        }
    }

    fn welcome(&mut self, w: &Accepted, host: &Host) {
        self.lock().ctx = Some(w.ctx);
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
            "view.resized" => {
                if let Ok(c) = serde_json::from_value::<Context>(json!({"viewport": e.payload["viewport"], "area": e.payload["area"]})) {
                    s.ctx = Some(c);
                }
            }
            _ => return Handled::Pass,
        }
        s.redraw(host);
        Handled::Handled
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
