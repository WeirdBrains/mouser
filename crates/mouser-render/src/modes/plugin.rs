//! External mode plugins (P6, docs/modes.md): a `plugin.toml` with
//! `kind = "mode"`, `transport = "command"` and a `[mode]` table. Mouser runs
//! the plugin's program once per event (JSON in on stdin, JSON out on stdout,
//! the same contract as command plugins) and draws its reply in the dock:
//!
//! - request: `{"api":1,"command":"mode","args":[EVENT, ID],"cwd":...}` where
//!   EVENT is `activate`, `key` (ID = a `[[mode.keys]]` id) or `cwd`;
//! - reply: `{"title":"...","lines":["..."],"text":"status message"}`.
//!
//! The program runs only after `run` is allowed (`mouser plugin permissions
//! NAME run=allow`); the working directory is sent only with `files.read`. A
//! mode plugin can show text and react to keys; it can't type into the shell.

use super::{Context, Effect, Mode, View};
use crate::Waker;
use crate::frame::PADDING;
use crate::ui::{Panel, PanelItem, PanelStyle, Rect, UiAction};
use mouser_core::modes::{Binding, Keymap};
use mouser_core::permissions::{Decision, Store};
use mouser_core::plugins::{self, CommandRequest, Installed};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Most lines a reply may put in the dock.
pub const MAX_LINES: usize = 40;

#[derive(Default)]
struct Shared {
    title: Option<String>,
    lines: Vec<String>,
    status: Option<String>,
    error: Option<String>,
    busy: bool,
    /// An event that came in while the program was running (latest wins),
    /// with the directory to send for it.
    queued: Option<(String, String, Option<String>)>,
}

pub struct PluginMode {
    plugin: Installed,
    name: &'static str,
    hotkey: char,
    bindings: &'static [Binding],
    cwd: Option<PathBuf>,
    shared: Arc<Mutex<Shared>>,
    waker: Option<Waker>,
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

impl PluginMode {
    /// `None` if the plugin has no `[mode]` table (the manifest check prevents that).
    pub fn new(plugin: Installed) -> Option<Self> {
        let m = plugin.manifest.mode.clone()?;
        let hotkey = m.hotkey.chars().next()?;
        // Mode names and bindings are 'static in the Mode trait; a handful of
        // short strings per plugin, made once at startup.
        let bindings: Vec<Binding> = m
            .keys
            .iter()
            .map(|k| Binding { id: leak(&k.id), key: leak(&k.key), describe: leak(if k.describe.is_empty() { &k.id } else { &k.describe }) })
            .collect();
        Some(PluginMode {
            name: leak(&plugin.manifest.name),
            plugin,
            hotkey,
            bindings: Box::leak(bindings.into_boxed_slice()),
            cwd: None,
            shared: Arc::new(Mutex::new(Shared::default())),
            waker: None,
        })
    }

    fn allowed(&self, cap: &str) -> bool {
        Store::load().get(self.name, cap) == Decision::Allow
    }

    /// Run the program for one event in the background.
    fn send(&mut self, event: &str, id: &str) {
        if !self.allowed("run") {
            let mut s = self.shared.lock().unwrap();
            s.error = Some(format!("{} needs permission to run: mouser plugin permissions {} run=allow", self.name, self.name));
            return;
        }
        let cwd = if self.allowed("files.read") { self.cwd.as_ref().map(|d| d.display().to_string()) } else { None };
        {
            let mut s = self.shared.lock().unwrap();
            if s.busy {
                s.queued = Some((event.into(), id.into(), cwd));
                return;
            }
            s.busy = true;
        }
        let (p, shared, waker) = (self.plugin.clone(), self.shared.clone(), self.waker.clone());
        let (mut event, mut id, mut cwd) = (event.to_string(), id.to_string(), cwd);
        std::thread::spawn(move || {
            loop {
                let req = CommandRequest { api: plugins::API, command: "mode".into(), args: vec![event.clone(), id.clone()], cwd: cwd.clone() };
                let r = plugins::run_command(&p, &req);
                let mut s = shared.lock().unwrap();
                match r {
                    Ok(r) if r.error.is_some() => s.error = r.error,
                    Ok(r) => {
                        s.error = None;
                        if r.title.is_some() {
                            s.title = r.title;
                        }
                        if !r.lines.is_empty() {
                            s.lines = r.lines.into_iter().take(MAX_LINES).collect();
                        }
                        s.status = (!r.text.is_empty()).then_some(r.text);
                    }
                    Err(e) => s.error = Some(format!("{e:#}")),
                }
                match s.queued.take() {
                    Some((e, i, d)) => {
                        (event, id, cwd) = (e, i, d);
                        drop(s);
                    }
                    None => {
                        s.busy = false;
                        break;
                    }
                }
            }
            if let Some(w) = waker {
                w();
            }
        });
    }
}

fn row(rect: Rect, text: impl Into<String>, style: PanelStyle) -> PanelItem {
    PanelItem { rect, text: text.into(), style, action: UiAction::None }
}

impl Mode for PluginMode {
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
        self.send("activate", "");
    }

    fn cwd_changed(&mut self, dir: &Path) {
        if self.cwd.as_deref() != Some(dir) {
            self.cwd = Some(dir.to_path_buf());
            self.send("cwd", "");
        }
    }

    fn action(&mut self, id: &str, _arg: i32) -> Effect {
        if self.bindings.iter().any(|b| b.id == id) {
            self.send("key", id);
        }
        Effect::None
    }

    fn view(&mut self, ctx: &Context, keys: &Keymap) -> View {
        let vp = ctx.viewport;
        let (cw, ch) = (vp.cell_w.max(1), vp.cell_h.max(1));
        let a = ctx.area;
        let row_h = ch + 4;
        let s = self.shared.lock().unwrap();
        let mut body: Vec<(String, PanelStyle)> = Vec::new();
        if let Some(e) = &s.error {
            body.push((e.clone(), PanelStyle::Error));
        }
        let cols = ((a.w - 2 * PADDING - 20) / cw).max(10) as usize;
        for l in &s.lines {
            body.push((super::chat::wrap(l, cols).join("\n"), PanelStyle::Label));
        }
        if s.lines.is_empty() && s.error.is_none() {
            body.push((if s.busy { "…".into() } else { "(nothing yet)".into() }, PanelStyle::Hint));
        }
        let lines: Vec<(String, PanelStyle)> =
            body.into_iter().flat_map(|(t, st)| t.split('\n').map(|l| (l.to_string(), st)).collect::<Vec<_>>()).take(MAX_LINES).collect();
        let hint = {
            let k: Vec<String> = self.bindings.iter().map(|b| format!("{} {}", keys.label(b.id), b.describe)).collect();
            let mut h = format!("plugin mode {} · esc :{} off", self.name, self.hotkey);
            if !k.is_empty() {
                h = format!("{h} · {}", k.join(" · "));
            }
            if let Some(st) = &s.status {
                h = format!("{st}  ·  {h}");
            }
            h
        };
        let h = ((lines.len() as i32 + 2) * row_h + 16).min((a.h / 2).max(row_h * 4));
        let rect = Rect { x: a.x + PADDING, y: a.y, w: a.w - 2 * PADDING, h };
        let (x0, w) = (rect.x + 10, rect.w - 20);
        let mut items =
            vec![row(Rect { x: x0, y: rect.y + 6, w, h: row_h }, s.title.clone().unwrap_or_else(|| self.name.to_string()), PanelStyle::Title)];
        let fit = ((h - 2 * row_h - 16) / row_h).max(0) as usize;
        for (i, (t, st)) in lines.into_iter().take(fit).enumerate() {
            items.push(row(Rect { x: x0, y: rect.y + 6 + (i as i32 + 1) * row_h, w, h: row_h }, t, st));
        }
        items.push(row(Rect { x: x0, y: rect.y + h - row_h - 6, w, h: row_h }, hint, PanelStyle::Hint));
        View { dock: Some(Panel { rect, items }), panel: None, images: vec![] }
    }

    fn set_waker(&mut self, w: Waker) {
        self.waker = Some(w);
    }
}
