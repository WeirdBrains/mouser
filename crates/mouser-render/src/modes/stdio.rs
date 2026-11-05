//! Mode plugins on the mode protocol (`transport = "mode"`,
//! docs/mode-protocol.md): a program Mouser starts once and keeps running,
//! supervised by `mouser_mode_api::host::Supervisor`. This adapter turns the
//! in-tree [`Mode`] calls into protocol events and the plugin's messages
//! back into views and effects:
//!
//! - bound keys and clicks become `ui.action` events, typing `input.text`,
//!   escape `input.escape`, activation `mode.activated` / `mode.deactivated`,
//!   the shell's folder `shell.cwd_changed` (only with `files.read`) and a new
//!   drawing area `view.resized`;
//! - the plugin's views are drawn as they come (its panels can only carry
//!   its own actions); images come from the shared-memory ring or inline;
//! - effects are checked against the granted permissions and handed to
//!   Mouser through [`Mode::take_effects`];
//! - emits come back to the plugin's own matching listeners (self-delivery),
//!   with the emit rules and hop limit of `mouser_mode_api::events`.
//!
//! The program only starts once `run` is allowed (`mouser plugin permissions
//! NAME run=allow`), like command plugins. If it keeps crashing the dock
//! shows why, with a Restart button.

use super::{Context, Effect, Mode, ModeImage, View};
use crate::Waker;
use crate::frame::PADDING;
use crate::ui::{Panel, PanelItem, PanelStyle, Rect, UiAction};
use mouser_core::modes::{Binding, Keymap};
use mouser_core::permissions::{Decision, Store};
use mouser_core::plugins::Installed;
use mouser_mode_api::events::{self, Event, Origin};
use mouser_mode_api::host::{Grants, Spec, State, Supervisor};
use mouser_mode_api::protocol::{In, LogLevel, Out, Pixels};
use mouser_mode_api::supervisor::Restarts;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The click on the "Restart" button of a plugin that gave up.
const RESTART: &str = "__mouser_restart";
/// Shared-memory ring: two 1080p RGBA slots (the file stays sparse until used).
const SHM: (u32, usize) = (2, 1920 * 1080 * 4);

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

pub struct StdioMode {
    plugin: Installed,
    name: &'static str,
    hotkey: char,
    bindings: &'static [Binding],
    max_restarts: u32,
    sup: Option<Supervisor>,
    waker: Option<Waker>,
    ctx: Option<Context>,
    seq: u64,
    view: View,
    images: HashMap<u64, Arc<Vec<u8>>>,
    effects: Vec<Effect>,
    error: Option<String>,
    last_status: Option<String>,
    /// The mode is current (replayed as `mode.activated` after a (re)start).
    active: bool,
    was_running: bool,
}

impl StdioMode {
    /// `None` if the manifest has no `[mode]` table (its check prevents that).
    pub fn new(plugin: Installed, max_restarts: u32) -> Option<Self> {
        let m = plugin.manifest.mode.clone()?;
        let hotkey = m.hotkey.chars().next()?;
        let bindings: Vec<Binding> = m
            .keys
            .iter()
            .map(|k| Binding { id: leak(&k.id), key: leak(&k.key), describe: leak(if k.describe.is_empty() { &k.id } else { &k.describe }) })
            .collect();
        Some(Self {
            name: leak(&plugin.manifest.name),
            plugin,
            hotkey,
            bindings: Box::leak(bindings.into_boxed_slice()),
            max_restarts,
            sup: None,
            waker: None,
            ctx: None,
            seq: 0,
            view: View::default(),
            images: HashMap::new(),
            effects: Vec::new(),
            error: None,
            last_status: None,
            active: false,
            was_running: false,
        })
    }

    fn allowed(&self, cap: &str) -> bool {
        Store::load().get(self.name, cap) == Decision::Allow
    }

    /// Start the plugin on first use.
    fn ensure(&mut self) -> bool {
        if self.sup.is_some() {
            return true;
        }
        if !self.allowed("run") {
            self.error = Some(format!("{} needs permission to run: mouser plugin permissions {} run=allow", self.name, self.name));
            return false;
        }
        let Some(ctx) = self.ctx else { return false };
        let argv = self.plugin.argv();
        let Some((program, args)) = argv.split_first() else { return false };
        let spec = Spec { name: self.name.into(), program: program.into(), args: args.to_vec(), env: vec![], shm: Some(SHM) };
        let declared = self.plugin.manifest.permissions.clone();
        let name = self.name;
        let grant = move |h: &mouser_mode_api::protocol::Hello| {
            let store = Store::load();
            let granted: Vec<String> = declared.iter().filter(|c| store.get(name, c) == Decision::Allow).cloned().collect();
            let denied = h
                .listeners
                .iter()
                .filter(|l| needs(&l.event).is_some_and(|c| !granted.iter().any(|g| g == c)))
                .map(|l| (l.event.clone(), format!("needs {}", needs(&l.event).unwrap_or_default())))
                .collect();
            Grants { granted, denied }
        };
        let w = self.waker.clone();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if let Some(w) = &w {
                w()
            }
        });
        self.sup = Some(Supervisor::with_notify(spec, ctx, Restarts::new(self.max_restarts), notify, grant));
        self.error = None;
        true
    }

    fn granted(&self) -> Vec<String> {
        let store = Store::load();
        self.plugin.manifest.permissions.iter().filter(|c| store.get(self.name, c) == Decision::Allow).cloned().collect()
    }

    fn event(&mut self, name: &str, payload: Value) {
        if !self.ensure() {
            return;
        }
        self.seq += 1;
        let ts_us = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0);
        let ev = Event { seq: self.seq, ts_us, tab: None, name: name.into(), payload, origin: Origin::Host, cause: None, hops: 0, synthetic: false };
        if let Some(s) = &mut self.sup {
            s.send(&In::Event(ev), Instant::now());
        }
    }

    /// Hand a plugin's emit back to its own listeners.
    fn self_deliver(&mut self, name: String, payload: Value) {
        let Some(sup) = &mut self.sup else { return };
        let Some(hello) = sup.connection().map(|c| c.hello.clone()) else { return };
        let res = events::may_emit(&hello.name, &hello.emits, &name, None, &[]).map_err(|e| e.to_string());
        let result = match res {
            Err(e) => events::EmitResult::Denied(e),
            Ok(()) => {
                let hearers = hello.listeners.iter().filter(|l| l.matches(&name) && l.hears_self()).count().min(events::MAX_FANOUT);
                if hearers > 0 {
                    self.seq += 1;
                    let origin = Origin::Plugin { name: hello.name.clone(), session: String::new() };
                    let ev = Event { seq: self.seq, ts_us: 0, tab: None, name, payload, origin, cause: None, hops: 1, synthetic: true };
                    sup.send(&In::Event(ev), Instant::now());
                }
                events::EmitResult::Accepted { delivered: hearers as u32 }
            }
        };
        sup.send(&In::EmitResult(result), Instant::now());
    }

    fn handle(&mut self, m: Out) {
        match m {
            Out::View { dock, panel, images, fullscreen } => {
                let mut out = Vec::new();
                for i in images {
                    let rgba = match i.pixels {
                        Pixels::Slot(s) => {
                            let n = (i.width as usize) * (i.height as usize) * 4;
                            let ring = self.sup.as_ref().and_then(|s| s.connection()).and_then(|c| c.ring.as_ref());
                            match ring.and_then(|r| r.slot(s)).filter(|b| b.len() >= n) {
                                Some(b) => Arc::new(b[..n].to_vec()),
                                None => continue,
                            }
                        }
                        Pixels::Inline { format: mouser_mode_api::protocol::ImageFormat::Rgba, bytes } => Arc::new(bytes),
                        Pixels::Inline { .. } => continue,
                        Pixels::Cached => match self.images.get(&i.key) {
                            Some(b) => b.clone(),
                            None => continue,
                        },
                    };
                    self.images.insert(i.key, rgba.clone());
                    out.push(ModeImage { key: i.key, rgba, width: i.width, height: i.height, src: i.src, rect: i.rect });
                }
                let keys: Vec<u64> = out.iter().map(|i| i.key).collect();
                self.images.retain(|k, _| keys.contains(k));
                self.view = View { dock: dock.map(|p| p.map(UiAction::from)), panel: panel.map(|p| p.map(UiAction::from)), images: out, fullscreen };
            }
            Out::Effect(e) => {
                let granted = self.granted();
                match e.permission() {
                    Some(p) if !granted.iter().any(|g| g == p) => self.effects.push(Effect::Say(format!("{} needs {p} for that", self.name))),
                    _ => self.effects.push(e),
                }
            }
            Out::Emit { name, payload } => self.self_deliver(name, payload),
            Out::Request { id, .. } => {
                if let Some(s) = &mut self.sup {
                    s.send(&In::Response { id, result: Err("host services are not available yet".into()) }, Instant::now());
                }
            }
            Out::Log { level, text } => match level {
                LogLevel::Error => log::error!("{}: {text}", self.name),
                LogLevel::Warn => log::warn!("{}: {text}", self.name),
                LogLevel::Info => log::info!("{}: {text}", self.name),
                LogLevel::Debug => log::debug!("{}: {text}", self.name),
            },
            _ => {}
        }
    }

    fn notice(&self, ctx: &Context, text: &str, restart: bool) -> Panel {
        let vp = ctx.viewport;
        let (cw, ch) = (vp.cell_w.max(1), vp.cell_h.max(1));
        let w = ctx.area.w - 2 * PADDING;
        let rect = Rect { x: ctx.area.x + PADDING, y: ctx.area.y + PADDING, w, h: ch * 3 };
        let mut items = vec![PanelItem {
            rect: Rect { x: rect.x + cw, y: rect.y + ch / 2, w: w - cw * 14, h: ch },
            text: text.into(),
            style: PanelStyle::Error,
            action: UiAction::None,
        }];
        if restart {
            items.push(PanelItem {
                rect: Rect { x: rect.x + w - cw * 12, y: rect.y + ch / 2, w: cw * 11, h: ch * 2 },
                text: "Restart".into(),
                style: PanelStyle::Button { primary: true },
                action: UiAction::Mode { action: RESTART.into(), arg: 0 },
            });
        }
        Panel { rect, items }
    }
}

/// The permission a listener needs (EVENTS.md section 6).
fn needs(event: &str) -> Option<&'static str> {
    if event.starts_with("shell.cwd") || event.starts_with("files.") {
        Some("files.read")
    } else if event.starts_with("term.") || event.starts_with("shell.command") {
        Some("scrollback.read")
    } else if event.starts_with("shell.remote") {
        Some("remote.read")
    } else if event.starts_with("agent.") || event.starts_with("models.") {
        Some("agent.use")
    } else {
        None
    }
}

impl Mode for StdioMode {
    fn name(&self) -> &'static str {
        self.name
    }

    fn hotkey(&self) -> char {
        self.hotkey
    }

    fn bindings(&self) -> &'static [Binding] {
        self.bindings
    }

    fn activate(&mut self) {
        self.active = true;
        if self.was_running {
            self.event("mode.activated", json!({"via": "hotkey"}));
        }
    }

    fn restore(&mut self) {
        self.activate()
    }

    fn deactivate(&mut self) {
        self.active = false;
        if self.was_running {
            self.event("mode.deactivated", json!({}));
        }
    }

    fn cwd_changed(&mut self, dir: &Path) {
        if self.granted().iter().any(|g| g == "files.read") {
            self.event("shell.cwd_changed", json!({"dir": dir.display().to_string()}));
        }
    }

    fn action(&mut self, id: &str, arg: i32) -> Effect {
        if id == RESTART {
            if let Some(s) = &mut self.sup {
                s.restart(Instant::now());
            }
            return Effect::None;
        }
        self.event("ui.action", json!({"id": id, "arg": arg}));
        Effect::None
    }

    fn text(&mut self, bytes: &[u8]) -> Option<Effect> {
        self.event("input.text", json!({"utf8": String::from_utf8_lossy(bytes)}));
        None
    }

    fn escape(&mut self) -> bool {
        self.event("input.escape", json!({}));
        false
    }

    fn view(&mut self, ctx: &Context, _keys: &Keymap) -> View {
        if self.ctx != Some(*ctx) {
            let first = self.ctx.is_none();
            self.ctx = Some(*ctx);
            if let Some(s) = &mut self.sup {
                s.set_ctx(*ctx);
            }
            if first {
                self.ensure();
            } else {
                self.event("view.resized", json!({"viewport": ctx.viewport, "area": ctx.area}));
            }
        }
        let now = Instant::now();
        let msgs = self.sup.as_mut().map(|s| s.poll(now)).unwrap_or_default();
        for m in msgs {
            self.handle(m);
        }
        let running = self.sup.as_ref().is_some_and(|s| *s.state() == State::Running);
        if running && !self.was_running && self.active {
            // Started, or back after a crash: tell it it's the current mode.
            self.was_running = true;
            self.event("mode.activated", json!({"via": "start"}));
        }
        self.was_running = running;
        if let Some(e) = &self.error {
            return View { dock: Some(self.notice(ctx, e, false)), ..View::default() };
        }
        let status = self.sup.as_ref().and_then(|s| s.status(now));
        if status != self.last_status {
            if let Some(t) = &status {
                self.effects.push(Effect::Say(t.clone()));
            }
            self.last_status = status.clone();
        }
        match self.sup.as_ref().map(|s| s.state().clone()) {
            Some(State::Running) | None => self.view.clone(),
            Some(State::Waiting { until, .. }) => {
                if let Some(w) = &self.waker {
                    // Poll again when the restart is due.
                    let (w, d) = (w.clone(), until.saturating_duration_since(now));
                    std::thread::spawn(move || {
                        std::thread::sleep(d);
                        w();
                    });
                }
                View { dock: Some(self.notice(ctx, &status.unwrap_or_default(), false)), ..View::default() }
            }
            Some(State::Disabled { reason }) => View { dock: Some(self.notice(ctx, &reason, true)), ..View::default() },
        }
    }

    fn set_waker(&mut self, w: Waker) {
        self.waker = Some(w);
    }

    fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }
}

impl Drop for StdioMode {
    fn drop(&mut self) {
        if let Some(s) = &mut self.sup {
            s.stop("quit");
        }
    }
}
