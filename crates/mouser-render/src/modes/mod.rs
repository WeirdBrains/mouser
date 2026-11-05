//! Mode plugins (docs/modes.md). A mode changes how you work with the files
//! under the shell: while it has the keyboard (the focus toggle, or a click
//! in its view) it gets plain keys through its keymap, and otherwise none:
//! every key goes to the shell untouched. It follows the shell's
//! working directory and draws a view above the prompt. Exactly one mode is
//! active at a time (or none); Mouser starts in the default one, commander.
//!
//! Modes are compiled in for now ([`Host::builtin`]) while they move to
//! plugin executables on `mouser-mode-api` (docs/protocol.md); the shared
//! types ([`Context`], [`Effect`], `Rect`, `PanelStyle`, `UiViewport`)
//! already come from that crate.

pub mod chat;
pub mod commander;
pub mod player;
pub mod plugin;
pub mod prefs;
pub mod prompt;
pub mod services;
pub mod stdio;

use crate::ui::{Panel, PanelItem, PanelStyle, Rect, UiAction};
use crate::{UiViewport, Waker};
use mouser_core::modes::{Binding, Keymap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use mouser_mode_api::{Context, Effect};

/// Pixels to show inside `rect` (fitted, aspect kept).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeImage {
    pub key: u64,
    pub rgba: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    /// Part of the image shown, in image pixels (x, y, w, h).
    pub src: (u32, u32, u32, u32),
    pub rect: Rect,
}

/// A mode's contribution to the frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    /// Docked above the prompt.
    pub dock: Option<Panel>,
    /// A floating window (options, manual ...).
    pub panel: Option<Panel>,
    pub images: Vec<ModeImage>,
    /// The dock fills the whole window (commander's fullscreen media): no
    /// grid, tab bar or sidebar is drawn, and the terminal keeps its size.
    pub fullscreen: bool,
}

/// A mode plugin.
pub trait Mode: Send {
    fn name(&self) -> &'static str;
    /// Typed after `esc :` to switch to it.
    fn hotkey(&self) -> char;
    /// Default key bindings; users remap them in `modes.toml`.
    fn bindings(&self) -> &'static [Binding];
    fn activate(&mut self) {}
    /// Back in this mode at launch (the last mode, `Host::restore`): like
    /// `activate`, but nothing that needs the user (no prompts, no downloads).
    fn restore(&mut self) {
        self.activate()
    }
    /// Leaving the mode: drop transient state (search, windows).
    fn deactivate(&mut self) {}
    fn cwd_changed(&mut self, _dir: &Path) {}
    /// The shell is on another host (`label` = user@host) in `dir` (`None`:
    /// unknown), listed over ssh (`None`: loading). Called every frame.
    fn remote_changed(&mut self, _label: &str, _dir: Option<&Path>, _listing: Option<Result<Vec<mouser_core::commander::Entry>, String>>) {}
    /// The ssh side channel to the remote shell's host (`None`: none / local).
    fn remote_ssh(&mut self, _ssh: Option<&mouser_core::remote::SshTarget>) {}
    /// A bound key (`id` from [`Mode::bindings`]) or a click in the mode's
    /// view or window (`id`, `arg` from its `UiAction::Mode` items).
    fn action(&mut self, id: &str, arg: i32) -> Effect;
    /// Typed text while the mode has an input focused; `None` = not taken.
    fn text(&mut self, _bytes: &[u8]) -> Option<Effect> {
        None
    }
    /// An input of the mode has focus: plain typing goes to [`Mode::text`].
    /// An escape sequence (arrows, Shift+Tab) this mode takes while it
    /// wants text, e.g. a list in its view. Default: none.
    fn wants_key(&self, _bytes: &[u8]) -> bool {
        false
    }
    fn wants_text(&self) -> bool {
        false
    }
    /// Escape: close the mode's window or input; false if nothing was open.
    fn escape(&mut self) -> bool {
        false
    }
    fn view(&mut self, ctx: &Context, keys: &Keymap) -> View;
    fn set_waker(&mut self, _w: Waker) {}
    /// Effects that came in off the event path (a plugin process's messages);
    /// Mouser applies them after drawing. Default: none.
    fn take_effects(&mut self) -> Vec<Effect> {
        Vec::new()
    }
    /// Questions for Mouser's own popup ([`prompt`]). Default: none.
    fn take_prompts(&mut self) -> Vec<prompt::Prompt> {
        Vec::new()
    }
    /// How to start an agent session for a plugin with `agent.use` (the
    /// app's tools and models; keys stay in Mouser). Default: ignored.
    fn set_agent(&mut self, _f: AgentFactory) {}
}

/// Makes an agent session (`mouser_core::agent::Chat`) on the app's host.
pub type AgentFactory = Arc<dyn Fn() -> mouser_core::agent::Chat + Send + Sync>;

/// Host-owned windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Window {
    Keys,
    Modes,
}

/// The registered modes and the single current one.
pub struct Host {
    modes: Vec<(Box<dyn Mode>, Keymap)>,
    current: Option<usize>,
    window: Option<Window>,
    cwd: Option<PathBuf>,
    /// The current mode has the keyboard.
    focused: bool,
    /// The focus toggle (`[focus] key` in `modes.toml`).
    focus_key: String,
    /// Problems loading `modes.toml`, shown once.
    pub notes: Vec<String>,
}

impl Host {
    /// Modes in order; the first is the default and starts active.
    pub fn new(modes: Vec<Box<dyn Mode>>) -> Self {
        let mut notes = Vec::new();
        let modes: Vec<_> = modes
            .into_iter()
            .map(|m| {
                let (k, note) = Keymap::load(m.name(), m.bindings());
                notes.extend(note);
                (m, k)
            })
            .collect();
        let (focus_key, note) = mouser_core::modes::load_focus_key();
        notes.extend(note);
        let mut h = Self { current: None, window: None, cwd: None, focused: false, focus_key, notes, modes };
        if !h.modes.is_empty() {
            h.current = Some(0);
            h.modes[0].0.activate();
        }
        h
    }

    /// The modes built into Mouser: commander (default).
    pub fn builtin() -> Self {
        Self::new(vec![Box::new(commander::CommanderMode::new())])
    }

    /// The mode switcher from the plugin registry (P6): enabled modes in
    /// priority order, the first one active. Commander and command-transport
    /// mode plugins are made here; `make` builds the rest (the app's chat mode)
    /// or returns `None` for modes this build lacks. A plugin whose hotkey is
    /// already taken is left out with a note. No enabled mode: an empty host,
    /// and Mouser is a plain terminal.
    pub fn from_registry(r: &mouser_core::plugins::Registry, mut make: impl FnMut(&mouser_core::plugins::Entry) -> Option<Box<dyn Mode>>) -> Self {
        use mouser_core::plugins::Transport;
        let mut modes: Vec<Box<dyn Mode>> = Vec::new();
        let mut notes = Vec::new();
        for e in r.modes() {
            let m: Option<Box<dyn Mode>> = match (e.transport, e.name.as_str()) {
                (Transport::Builtin, "commander") => Some(Box::new(commander::CommanderMode::new())),
                (Transport::Builtin, "preferences") => Some(Box::new(prefs::PrefsMode::new())),
                (Transport::Mode, _) => mouser_core::plugins::find(&e.name)
                    .and_then(|p| stdio::StdioMode::new(p, mouser_core::storage::max_restarts(&mouser_core::storage::load_preferences())))
                    .map(|m| Box::new(m) as Box<dyn Mode>),
                (Transport::Command, _) => {
                    mouser_core::plugins::find(&e.name).and_then(plugin::PluginMode::new).map(|m| Box::new(m) as Box<dyn Mode>)
                }
                _ => make(e),
            };
            let Some(m) = m else {
                notes.push(format!("mode {} is not available in this build", e.name));
                continue;
            };
            if let Some(o) = modes.iter().find(|o| o.hotkey() == m.hotkey()) {
                notes.push(format!("mode {} left out: hotkey {} is {}'s", e.name, m.hotkey(), o.name()));
                continue;
            }
            modes.push(m);
        }
        let mut h = Self::new(modes);
        h.notes.extend(notes);
        h
    }

    /// Start in the mode a window last closed in (`load_last_mode`): a
    /// name, or `None` for no mode. A mode that is gone or disabled keeps no
    /// mode, quietly. Only activates: nothing is sent, nothing downloads.
    pub fn restore(&mut self, last: Option<&str>) {
        if let Some(c) = self.current.take() {
            self.modes[c].0.deactivate();
        }
        let Some(i) = last.and_then(|n| self.modes.iter().position(|(m, _)| m.name() == n)) else { return };
        self.current = Some(i);
        self.modes[i].0.restore();
        if let Some(d) = &self.cwd {
            self.modes[i].0.cwd_changed(d);
        }
    }

    /// Names of the registered modes, in switcher order.
    pub fn names(&self) -> Vec<&'static str> {
        self.modes.iter().map(|(m, _)| m.name()).collect()
    }

    /// Register another mode (not active until switched to).
    pub fn push(&mut self, m: Box<dyn Mode>) {
        let (k, note) = Keymap::load(m.name(), m.bindings());
        self.notes.extend(note);
        self.modes.push((m, k));
    }

    pub fn set_waker(&mut self, w: Waker) {
        for (m, _) in &mut self.modes {
            m.set_waker(w.clone());
        }
    }

    pub fn current(&self) -> Option<&'static str> {
        self.current.map(|i| self.modes[i].0.name())
    }

    /// Switch to the mode with `hotkey` (or `name`), replacing the current
    /// one, and give it the keyboard; the current mode's own hotkey turns
    /// modes off.
    pub fn switch(&mut self, key: &str) -> Result<String, String> {
        let key = key.trim().trim_start_matches(':');
        let Some(i) = self.modes.iter().position(|(m, _)| key == m.name() || key.chars().eq([m.hotkey()])) else {
            return Err(format!("no mode {key:?} (ctrl+shift+m lists them)"));
        };
        self.focused = false;
        if let Some(c) = self.current.take() {
            self.modes[c].0.deactivate();
            if c == i {
                return Ok(format!("{} mode off", self.modes[i].0.name()));
            }
        }
        self.current = Some(i);
        // Switched to on purpose: it gets the keyboard (Esc gives it back).
        self.focused = true;
        let m = &mut self.modes[i].0;
        m.activate();
        if let Some(d) = &self.cwd {
            m.cwd_changed(d);
        }
        Ok(format!("{} mode", m.name()))
    }

    fn mode(&mut self) -> Option<&mut (Box<dyn Mode>, Keymap)> {
        self.current.map(|i| &mut self.modes[i])
    }

    fn is_focus_key(&self, bytes: &[u8]) -> bool {
        mouser_core::modes::key_bytes(&self.focus_key).is_some_and(|v| v.iter().any(|b| b.as_slice() == bytes))
    }

    /// The current mode has the keyboard.
    pub fn focused(&self) -> bool {
        self.focused && self.current.is_some()
    }

    /// Give the current mode the keyboard (a click in its view) or take it back.
    pub fn focus(&mut self, on: bool) {
        self.focused = on && self.current.is_some();
    }

    /// The focus toggle as typed (`ctrl+alt+k`).
    pub fn focus_key(&self) -> &str {
        &self.focus_key
    }

    /// A key from the keyboard. Without focus only the focus toggle is the
    /// mode's (`None`: send it to the shell). With focus every key is: the
    /// mode's input first, then its bindings; anything else is dropped so
    /// nothing reaches the shell. Mouser's own F10 and Alt+Esc pass through.
    pub fn key(&mut self, bytes: &[u8]) -> Option<Effect> {
        self.current?;
        if self.is_focus_key(bytes) {
            self.focused = !self.focused;
            return Some(Effect::None);
        }
        if !self.focused || bytes == b"\x1b[21~" || bytes == b"\x1b\x1b" {
            return None;
        }
        let (m, k) = self.mode()?;
        if let Some(e) = m.text(bytes) {
            return Some(e);
        }
        let Some(id) = k.lookup(bytes).map(str::to_string) else { return Some(Effect::None) };
        let e = m.action(&id, 0);
        if e == Effect::Pass {
            // `close` with nothing left to close hands the keyboard back.
            if id == "close" {
                self.focused = false;
            }
            return Some(Effect::None);
        }
        Some(e)
    }

    /// Whether `key` (a hotkey or name) is a registered mode.
    pub fn has(&self, key: &str) -> bool {
        let key = key.trim().trim_start_matches(':');
        !key.is_empty() && self.modes.iter().any(|(m, _)| key == m.name() || key.chars().eq([m.hotkey()]))
    }

    /// Would [`Host::key`] take these bytes (no side effects)?
    pub fn takes(&self, bytes: &[u8]) -> bool {
        self.current.is_some() && (self.is_focus_key(bytes) || (self.focused && bytes != b"\x1b[21~" && bytes != b"\x1b\x1b"))
    }

    /// A click in the mode's view or window (or a host action): it gives
    /// the mode the keyboard, except the wheel. [`FOCUS_ACTION`] toggles.
    pub fn action(&mut self, id: &str, arg: i32) -> Effect {
        if id == FOCUS_ACTION {
            self.focused = !self.focused && self.current.is_some();
            return Effect::None;
        }
        if id != "scroll" {
            self.focus(true);
        }
        self.mode().map_or(Effect::None, |(m, _)| m.action(id, arg))
    }

    /// Escape: closes a host window, then the mode's, then gives the
    /// keyboard back to the shell; false if none of that happened.
    pub fn escape(&mut self) -> bool {
        if self.window.take().is_some() {
            return true;
        }
        if self.focused() && self.mode().is_some_and(|(m, _)| m.escape()) {
            return true;
        }
        std::mem::take(&mut self.focused) && self.current.is_some()
    }

    pub fn has_window(&self) -> bool {
        self.window.is_some()
    }

    pub fn show_keys(&mut self) {
        self.window = if self.window == Some(Window::Keys) { None } else { Some(Window::Keys) };
    }

    pub fn show_modes(&mut self) {
        self.window = if self.window == Some(Window::Modes) { None } else { Some(Window::Modes) };
    }

    pub fn close_window(&mut self) -> bool {
        self.window.take().is_some()
    }

    /// The shell is remote (see [`Mode::remote_changed`]).
    pub fn set_remote(
        &mut self,
        label: &str,
        dir: Option<&Path>,
        listing: Option<Result<Vec<mouser_core::commander::Entry>, String>>,
        ssh: Option<&mouser_core::remote::SshTarget>,
    ) {
        self.cwd = None;
        if let Some((m, _)) = self.mode() {
            m.remote_ssh(ssh);
            m.remote_changed(label, dir, listing);
        }
    }

    /// The shell's working directory (called every frame; cheap when unchanged).
    pub fn set_cwd(&mut self, dir: Option<PathBuf>) {
        let Some(d) = dir else { return };
        if self.cwd.as_ref() == Some(&d) {
            // The mode re-checks the directory's mtime itself.
            if let Some((m, _)) = self.mode() {
                m.cwd_changed(&d);
            }
            return;
        }
        self.cwd = Some(d.clone());
        if let Some((m, _)) = self.mode() {
            m.cwd_changed(&d);
        }
    }

    /// The current mode's view, with a host window (keys / modes) on top.
    pub fn view(&mut self, ctx: &Context) -> View {
        let window = self.window;
        let mut v = match self.mode() {
            Some((m, k)) => {
                let k = k.clone();
                m.view(ctx, &k)
            }
            None => View::default(),
        };
        if let (Some(d), Some(i)) = (v.dock.as_mut(), self.current) {
            focus_marks(d, self.focused, &self.focus_key, self.modes[i].0.name(), &self.modes[i].1, ctx.viewport);
        }
        match window {
            Some(Window::Keys) => v.panel = Some(self.keys_panel(ctx.viewport)),
            Some(Window::Modes) => v.panel = Some(self.modes_panel(ctx.viewport)),
            None => {}
        }
        v
    }

    /// Effects the modes produced off the event path, to apply now.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        self.modes.iter_mut().flat_map(|(m, _)| m.take_effects()).collect()
    }

    /// Questions for Mouser's popup from every mode ([`prompt`]).
    pub fn take_prompts(&mut self) -> Vec<prompt::Prompt> {
        self.modes.iter_mut().flat_map(|(m, _)| m.take_prompts()).collect()
    }

    /// Give every mode the agent factory (mode plugins with `agent.use`).
    pub fn set_agent(&mut self, f: AgentFactory) {
        for (m, _) in &mut self.modes {
            m.set_agent(f.clone());
        }
    }

    /// Ctrl+Shift+K: the current mode's mappings plus the global keys.
    pub fn keys_panel(&self, vp: UiViewport) -> Panel {
        let mut lines: Vec<(String, String)> = Vec::new();
        match self.current {
            Some(i) => {
                let (m, k) = &self.modes[i];
                lines.push((format!("{} mode", m.name()), "these keys work while it has the keyboard".into()));
                lines.push((self.focus_key.clone(), format!("give {} the keyboard / back to the shell (also esc)", m.name())));
                for (id, _, d) in &k.keys {
                    lines.push((k.label(id), d.clone()));
                }
            }
            None => lines.push(("no mode".into(), String::new())),
        }
        lines.push((String::new(), String::new()));
        lines.push(("mouse".into(), "click selects, double-click opens, wheel moves".into()));
        lines.push(("esc : <key> Enter".into(), "switch mode (its own key again: modes off)".into()));
        lines.push(("ctrl+shift+k".into(), "this window".into()));
        lines.push(("ctrl+shift+m".into(), "modes".into()));
        lines.push(("esc".into(), "close".into()));
        list_panel(vp, "Key mappings", &lines, |_| UiAction::None)
    }

    /// Ctrl+Shift+M: the registered modes; click one to switch.
    pub fn modes_panel(&self, vp: UiViewport) -> Panel {
        let lines: Vec<(String, String)> = self
            .modes
            .iter()
            .enumerate()
            .map(|(i, (m, _))| {
                let on = if self.current == Some(i) { "  (active)" } else { "" };
                (format!("esc :{}", m.hotkey()), format!("{}{on}", m.name()))
            })
            .collect();
        let keys: Vec<char> = self.modes.iter().map(|(m, _)| m.hotkey()).collect();
        list_panel(vp, "Modes", &lines, |i| keys.get(i).map_or(UiAction::None, |c| UiAction::SwitchMode(*c)))
    }
}

/// The host action a click on the focus hint sends.
pub const FOCUS_ACTION: &str = "mouser.focus";

/// Show who has the keyboard: with focus an accent ring round the dock and
/// a hint with a few of the mode's keys; without, a hint naming the toggle.
fn focus_marks(d: &mut Panel, focused: bool, toggle: &str, name: &str, k: &Keymap, vp: UiViewport) {
    let r = d.rect;
    if r.w <= 0 || r.h <= 0 {
        return;
    }
    let text = if focused {
        let mut parts: Vec<String> = ["sync", "search", "switch_view", "open", "focus", "man"]
            .iter()
            .filter(|id| k.keys.iter().any(|(i, _, _)| i == *id))
            .take(4)
            .map(|id| format!("{} {}", k.label(id), id.replace('_', " ")))
            .collect();
        parts.push(format!("esc / {toggle} shell"));
        format!("{name} has the keys · {}", parts.join(" · "))
    } else {
        format!("{toggle}: keys to {name}")
    };
    if focused {
        let t = 2;
        for (x, y, w, h) in [(r.x, r.y, r.w, t), (r.x, r.y + r.h - t, r.w, t), (r.x, r.y, t, r.h), (r.x + r.w - t, r.y, t, r.h)] {
            d.items.push(PanelItem { rect: Rect { x, y, w, h }, text: String::new(), style: PanelStyle::MdQuote, action: UiAction::None });
        }
    }
    let w = ((text.chars().count() as i32 + 2) * vp.cell_w).min(r.w - 8).max(0);
    let h = vp.cell_h + 6;
    let rect = Rect { x: r.x + r.w - w - 6, y: r.y + r.h - h - 6, w, h };
    let action = UiAction::Mode { action: FOCUS_ACTION.into(), arg: 0 };
    d.items.push(PanelItem { rect, text, style: PanelStyle::FloatLabel, action });
}

/// A centred window with two columns of text.
pub fn list_panel(vp: UiViewport, title: &str, lines: &[(String, String)], action: impl Fn(usize) -> UiAction) -> Panel {
    let (cw, ch) = (vp.cell_w, vp.cell_h);
    let row_h = ch + 6;
    let left = lines.iter().map(|l| l.0.chars().count()).max().unwrap_or(0) as i32 + 2;
    let right = lines.iter().map(|l| l.1.chars().count()).max().unwrap_or(0) as i32;
    let pad = 14;
    let w = ((left + right) * cw + 2 * pad).max(cw * 30).min(vp.width - 16);
    let h = (row_h * (lines.len() as i32 + 2) + 2 * pad).min(vp.height - 16);
    let rect = Rect { x: (vp.width - w) / 2, y: (vp.height - h) / 2, w, h };
    let (x, mut y) = (rect.x + pad, rect.y + pad);
    let mut items =
        vec![PanelItem { rect: Rect { x, y, w: w - 2 * pad, h: row_h }, text: title.into(), style: PanelStyle::Title, action: UiAction::None }];
    y += row_h + 6;
    for (i, (a, b)) in lines.iter().enumerate() {
        let act = action(i);
        let style = if act == UiAction::None { PanelStyle::Label } else { PanelStyle::Row { selected: false } };
        items.push(PanelItem { rect: Rect { x, y, w: left * cw, h: row_h }, text: a.clone(), style, action: act.clone() });
        items.push(PanelItem {
            rect: Rect { x: x + left * cw, y, w: (w - 2 * pad - left * cw).max(0), h: row_h },
            text: b.clone(),
            style: PanelStyle::Hint,
            action: act,
        });
        y += row_h;
        if y > rect.y + h - row_h {
            break;
        }
    }
    Panel { rect, items }
}

/// The largest `w` x `h` that fits in `r` with the image's aspect, centred.
pub fn fit(r: Rect, w: u32, h: u32) -> Rect {
    if w == 0 || h == 0 || r.w <= 0 || r.h <= 0 {
        return Rect { w: 0, h: 0, ..r };
    }
    let s = (r.w as f64 / w as f64).min(r.h as f64 / h as f64);
    let (fw, fh) = (((w as f64 * s) as i32).max(1), ((h as f64 * s) as i32).max(1));
    Rect { x: r.x + (r.w - fw) / 2, y: r.y + (r.h - fh) / 2, w: fw, h: fh }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        on: bool,
        got: Vec<String>,
    }

    impl Mode for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn hotkey(&self) -> char {
            'f'
        }
        fn bindings(&self) -> &'static [Binding] {
            &[Binding { id: "go", key: "g", describe: "go" }]
        }
        fn activate(&mut self) {
            self.on = true;
        }
        fn deactivate(&mut self) {
            self.on = false;
        }
        fn action(&mut self, id: &str, _: i32) -> Effect {
            self.got.push(id.into());
            Effect::Say(id.into())
        }
        fn view(&mut self, _: &Context, _: &Keymap) -> View {
            View::default()
        }
    }

    #[test]
    fn one_mode_routes_only_its_keys() {
        let mut h = Host::new(vec![Box::new(Fake { on: false, got: vec![] }), Box::new(commander::CommanderMode::new())]);
        assert_eq!(h.current(), Some("fake"), "the first mode is the default and starts active");
        assert_eq!(h.key(b"g"), None, "no focus: the key is the shell's");
        assert_eq!(h.key(b"\x1b\x0b"), Some(Effect::None), "ctrl+alt+k gives the mode the keyboard");
        assert!(h.focused());
        assert_eq!(h.key(b"g"), Some(Effect::Say("go".into())));
        assert_eq!(h.key(b"v"), Some(Effect::None), "commander's key is not routed while fake is on; dropped, not typed");
        assert!(h.takes(b"b") && !h.takes(b"\x1b[21~"), "F10 stays Mouser's");
        assert!(h.escape() && !h.focused(), "Esc gives the keyboard back");
        assert!(!h.escape());
        assert_eq!(h.switch("c").unwrap(), "commander mode");
        assert_eq!(h.current(), Some("commander"), "switching replaces the mode");
        assert!(h.focused(), "switched to on purpose: it has the keyboard");
        assert!(h.key(b"v").is_some());
        h.focus(false);
        assert_eq!(h.key(b"v"), None);
        assert_eq!(h.switch(":c").unwrap(), "commander mode off");
        assert_eq!(h.current(), None);
        assert!(!h.focused());
        assert_eq!(h.key(b"\x1b\x0b"), None, "no mode: everything goes to the shell");
        assert!(h.switch("q").is_err());
        h.switch("fake").unwrap();
        assert_eq!(h.current(), Some("fake"));
        h.focus(false);
        assert_eq!(h.action(FOCUS_ACTION, 0), Effect::None);
        assert!(h.focused(), "a click on the hint focuses");
        h.focus(false);
        h.action("go", 0);
        assert!(h.focused(), "a click in the view focuses");
    }

    /// Without focus a mode takes exactly one key, the focus toggle, and the
    /// toggle is no bash, zsh or fish default (`mouser_core::modes` tests).
    #[test]
    fn unfocused_the_only_mode_key_is_the_toggle() {
        let mut keys: Vec<Vec<u8>> = Vec::new();
        for k in cmdr_all_keys() {
            keys.extend(mouser_core::modes::key_bytes(&k).unwrap());
            keys.extend(mouser_core::modes::key_bytes(&format!("alt+{k}")).unwrap());
        }
        for c in (0x20u8..0x7f).chain(1..0x20) {
            keys.push(vec![c]);
            keys.push(vec![0x1b, c]);
        }
        let mut h = Host::builtin();
        for b in &keys {
            let toggle = mouser_core::modes::key_bytes(mouser_core::modes::DEFAULT_FOCUS_KEY).unwrap().contains(b);
            assert_eq!(h.takes(b), toggle, "{b:?}");
            if !toggle {
                assert_eq!(h.key(b), None, "{b:?} goes to the shell");
            }
        }
        // Every mode built in: its defaults are plain keys, never the toggle.
        for m in [&commander::CommanderMode::new() as &dyn Mode, &prefs::PrefsMode::new()] {
            for b in m.bindings() {
                assert!(!b.key.starts_with("alt+") && !b.key.starts_with("ctrl+"), "{}: {} is not plain", m.name(), b.id);
            }
        }
    }

    fn cmdr_all_keys() -> Vec<String> {
        mouser_core::commander::BINDINGS.iter().map(|b| b.key.to_string()).collect()
    }

    #[test]
    fn host_windows_list_keys_and_modes() {
        let vp = UiViewport { width: 1200, height: 800, cell_w: 9, cell_h: 18 };
        let mut h = Host::builtin();
        h.show_keys();
        let v = h.view(&Context { viewport: vp, area: Rect { x: 0, y: 0, w: 900, h: 800 } });
        let p = v.panel.unwrap();
        assert!(p.items.iter().any(|i| i.text == "v"));
        assert!(p.items.iter().any(|i| i.text == "ctrl+alt+k"));
        assert!(h.escape() && !h.has_window());
        let m = h.modes_panel(vp);
        assert!(m.items.iter().any(|i| i.action == UiAction::SwitchMode('c')));
    }

    #[test]
    fn fit_keeps_aspect() {
        assert_eq!(fit(Rect { x: 0, y: 0, w: 100, h: 100 }, 200, 100), Rect { x: 0, y: 25, w: 100, h: 50 });
    }
}
