//! Mode plugins (docs/modes.md). A mode changes how you work with the files
//! under the shell: it gets the keys bound in its keymap, follows the shell's
//! working directory and draws a view above the prompt. Exactly one mode is
//! active at a time (or none); Mouser starts in the default one, commander.
//!
//! Modes are compiled in for now ([`Host::builtin`]). The trait only passes
//! plain data in and out (key ids, paths, panels, pixels), so modes loaded
//! from libraries or helper processes can implement it later, like engines.

pub mod commander;
pub mod player;

use crate::ui::{Panel, PanelItem, PanelStyle, Rect, UiAction};
use crate::{UiViewport, Waker};
use mouser_core::modes::{Binding, Keymap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where a mode may draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context {
    pub viewport: UiViewport,
    /// The terminal area (left of the sidebar, below the tab strip); the view
    /// takes its top part and the terminal grid moves down.
    pub area: Rect,
}

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
}

/// What the mode asks Mouser to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Type this into the shell.
    Type(String),
    /// Run a command line in the shell; what the user has typed is kept
    /// (cut to the kill ring and yanked back when the prompt returns).
    Run(String),
    /// Hand a file to the desktop's opener (players etc.).
    Open(PathBuf),
    /// Show the key mappings window.
    ShowKeys,
    /// Status-bar message.
    Say(String),
    /// Not handled now: the key goes to the shell.
    Pass,
}

/// A mode plugin.
pub trait Mode: Send {
    fn name(&self) -> &'static str;
    /// Typed after `esc :` to switch to it.
    fn hotkey(&self) -> char;
    /// Default key bindings; users remap them in `modes.toml`.
    fn bindings(&self) -> &'static [Binding];
    fn activate(&mut self) {}
    /// Leaving the mode: drop transient state (search, windows).
    fn deactivate(&mut self) {}
    fn cwd_changed(&mut self, _dir: &Path) {}
    /// The shell is on another host (`label` = user@host) in `dir` (`None`:
    /// unknown), listed over ssh (`None`: loading). Called every frame.
    fn remote_changed(&mut self, _label: &str, _dir: Option<&Path>, _listing: Option<Result<Vec<mouser_core::commander::Entry>, String>>) {}
    /// A bound key (`id` from [`Mode::bindings`]) or a click in the mode's
    /// view or window (`id`, `arg` from its `UiAction::Mode` items).
    fn action(&mut self, id: &str, arg: i32) -> Effect;
    /// Typed text while the mode has an input focused; `None` = not taken.
    fn text(&mut self, _bytes: &[u8]) -> Option<Effect> {
        None
    }
    /// An input of the mode has focus: plain typing goes to [`Mode::text`].
    fn wants_text(&self) -> bool {
        false
    }
    /// Escape: close the mode's window or input; false if nothing was open.
    fn escape(&mut self) -> bool {
        false
    }
    fn view(&mut self, ctx: &Context, keys: &Keymap) -> View;
    fn set_waker(&mut self, _w: Waker) {}
}

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
        let mut h = Self { current: None, window: None, cwd: None, notes, modes };
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

    pub fn set_waker(&mut self, w: Waker) {
        for (m, _) in &mut self.modes {
            m.set_waker(w.clone());
        }
    }

    pub fn current(&self) -> Option<&'static str> {
        self.current.map(|i| self.modes[i].0.name())
    }

    /// Switch to the mode with `hotkey` (or `name`), replacing the current
    /// one; the current mode's own hotkey turns modes off.
    pub fn switch(&mut self, key: &str) -> Result<String, String> {
        let key = key.trim().trim_start_matches(':');
        let Some(i) = self.modes.iter().position(|(m, _)| key == m.name() || key.chars().eq([m.hotkey()])) else {
            return Err(format!("no mode {key:?} (ctrl+shift+m lists them)"));
        };
        if let Some(c) = self.current.take() {
            self.modes[c].0.deactivate();
            if c == i {
                return Ok(format!("{} mode off", self.modes[i].0.name()));
            }
        }
        self.current = Some(i);
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

    /// A key from the keyboard: the current mode's bound keys (or its focused
    /// input); `None` = not a mode key, send it to the shell.
    pub fn key(&mut self, bytes: &[u8]) -> Option<Effect> {
        let (m, k) = self.mode()?;
        if let Some(e) = m.text(bytes) {
            return Some(e);
        }
        let id = k.lookup(bytes)?.to_string();
        Some(m.action(&id, 0)).filter(|e| *e != Effect::Pass)
    }

    /// Whether `key` (a hotkey or name) is a registered mode.
    pub fn has(&self, key: &str) -> bool {
        let key = key.trim().trim_start_matches(':');
        !key.is_empty() && self.modes.iter().any(|(m, _)| key == m.name() || key.chars().eq([m.hotkey()]))
    }

    /// Would [`Host::key`] take these bytes (no side effects)?
    pub fn takes(&self, bytes: &[u8]) -> bool {
        self.current.is_some_and(|i| {
            let (m, k) = &self.modes[i];
            m.wants_text() && !(bytes.len() > 1 && bytes[0] == 0x1b) || k.lookup(bytes).is_some()
        })
    }

    pub fn action(&mut self, id: &str, arg: i32) -> Effect {
        self.mode().map_or(Effect::None, |(m, _)| m.action(id, arg))
    }

    /// Escape: closes a host window, then the mode's; false if nothing was open.
    pub fn escape(&mut self) -> bool {
        if self.window.take().is_some() {
            return true;
        }
        self.mode().is_some_and(|(m, _)| m.escape())
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
    pub fn set_remote(&mut self, label: &str, dir: Option<&Path>, listing: Option<Result<Vec<mouser_core::commander::Entry>, String>>) {
        self.cwd = None;
        if let Some((m, _)) = self.mode() {
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
        match window {
            Some(Window::Keys) => v.panel = Some(self.keys_panel(ctx.viewport)),
            Some(Window::Modes) => v.panel = Some(self.modes_panel(ctx.viewport)),
            None => {}
        }
        v
    }

    /// Ctrl+Shift+K: the current mode's mappings plus the global keys.
    pub fn keys_panel(&self, vp: UiViewport) -> Panel {
        let mut lines: Vec<(String, String)> = Vec::new();
        match self.current {
            Some(i) => {
                let (m, k) = &self.modes[i];
                lines.push((format!("{} mode", m.name()), String::new()));
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
        assert_eq!(h.key(b"\x1bg"), Some(Effect::Say("go".into())));
        assert_eq!(h.key(b"\x1bt"), None, "commander's key is not routed while fake is on");
        assert_eq!(h.key(b"\x1bb"), None);
        assert_eq!(h.switch("c").unwrap(), "commander mode");
        assert_eq!(h.current(), Some("commander"), "switching replaces the mode");
        assert_eq!(h.key(b"\x1bg"), None);
        assert!(h.key(b"\x1bt").is_some());
        assert_eq!(h.switch(":c").unwrap(), "commander mode off");
        assert_eq!(h.current(), None);
        assert_eq!(h.key(b"\x1bt"), None, "no mode: everything goes to the shell");
        assert!(h.switch("q").is_err());
        h.switch("fake").unwrap();
        assert_eq!(h.current(), Some("fake"));
    }

    #[test]
    fn host_windows_list_keys_and_modes() {
        let vp = UiViewport { width: 1200, height: 800, cell_w: 9, cell_h: 18 };
        let mut h = Host::builtin();
        h.show_keys();
        let v = h.view(&Context { viewport: vp, area: Rect { x: 0, y: 0, w: 900, h: 800 } });
        let p = v.panel.unwrap();
        assert!(p.items.iter().any(|i| i.text == "alt+t"));
        assert!(h.escape() && !h.has_window());
        let m = h.modes_panel(vp);
        assert!(m.items.iter().any(|i| i.action == UiAction::SwitchMode('c')));
    }

    #[test]
    fn fit_keeps_aspect() {
        assert_eq!(fit(Rect { x: 0, y: 0, w: 100, h: 100 }, 200, 100), Rect { x: 0, y: 25, w: 100, h: 50 });
    }
}
