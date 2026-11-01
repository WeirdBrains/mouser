//! Plugin approval popups (docs/plugins.md §Approval): a queue of requests the
//! permission broker said to ask about, shown one at a time as a Mouser panel
//! over the terminal. While one is open it owns the keyboard and mouse, so the
//! program in the tab can neither draw it nor answer it.

use crate::tabs::Session;
use mouser_core::permissions::{Answer, Scope};
use mouser_core::plugins;
use mouser_render::{UiViewport, ui};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Answers are ignored this long after a popup appears (no click/keypress-through).
pub const ARM_DELAY: Duration = Duration::from_millis(400);

/// What to do once answered.
pub enum Origin {
    /// Reply to a Fuzzball `p h` in this tab.
    Fuzzball { session: Arc<Session>, id: u32 },
    /// Run this `[mouser]` line with the plugin after a grant.
    Command { line: String },
}

pub struct Request {
    pub plugin: String,
    pub caps: Vec<String>,
    /// Extra lines: the exact command, the tab, …
    pub detail: Vec<String>,
    pub scope: Scope,
    pub origin: Origin,
    shown: Option<Instant>,
}

impl Request {
    pub fn new(plugin: &str, caps: Vec<String>, detail: Vec<String>, scope: Scope, origin: Origin) -> Self {
        Request { plugin: plugin.into(), caps, detail, scope, origin, shown: None }
    }
}

#[derive(Default)]
pub struct Approvals {
    queue: VecDeque<Request>,
}

impl Approvals {
    pub fn open(&self) -> bool {
        !self.queue.is_empty()
    }

    pub fn push(&mut self, r: Request) {
        if self.queue.len() < 16 {
            self.queue.push_back(r);
        }
    }

    fn armed(&self) -> bool {
        self.queue.front().and_then(|r| r.shown).is_some_and(|t| t.elapsed() >= ARM_DELAY)
    }

    /// Key while the popup is open; `Some` once answered.
    pub fn key(&mut self, bytes: &[u8]) -> Option<(Request, Answer)> {
        let offer = offer(&self.queue.front()?.scope);
        let a = match bytes {
            b"y" | b"Y" | b"o" | b"\r" => Answer::Once,
            b"a" if offer.contains(&1) => Answer::Always,
            b"n" | b"\x1b" => Answer::Deny,
            b"N" if offer.contains(&3) => Answer::Never,
            _ => return None,
        };
        // Esc always works; the rest only once armed.
        if a != Answer::Deny && !self.armed() {
            return None;
        }
        Some((self.queue.pop_front()?, a))
    }

    pub fn action(&mut self, i: u8) -> Option<(Request, Answer)> {
        if !self.armed() {
            return None;
        }
        let offer = offer(&self.queue.front()?.scope);
        let a = match i {
            0 => Answer::Once,
            1 if offer.contains(&1) => Answer::Always,
            2 => Answer::Deny,
            3 if offer.contains(&3) => Answer::Never,
            _ => return None,
        };
        Some((self.queue.pop_front()?, a))
    }

    /// The popup for the first request (starts its arm timer); `wake` is asked
    /// to redraw when it arms.
    pub fn panel_view(&mut self, vp: UiViewport) -> Option<ui::Panel> {
        let more = self.queue.len().saturating_sub(1);
        let r = self.queue.front_mut()?;
        if r.shown.is_none() {
            r.shown = Some(Instant::now());
            crate::wake_after(ARM_DELAY + Duration::from_millis(20));
        }
        let armed = r.shown.is_some_and(|t| t.elapsed() >= ARM_DELAY);
        let who = match r.scope {
            Scope::Installed => format!("Plugin \"{}\" asks to:", r.plugin),
            Scope::FuzzballLocal => format!("Program in this tab (\"{}\") asks to:", r.plugin),
            Scope::FuzzballRemote { .. } => format!("REMOTE program (over ssh, \"{}\") asks to:", r.plugin),
        };
        let mut lines: Vec<String> = r.caps.iter().map(|c| format!("  • {}  ({c})", plugins::capability_text(c))).collect();
        if r.caps.is_empty() {
            lines.push("  • connect as a plugin (no capabilities)".into());
        }
        lines.extend(r.detail.iter().cloned());
        match &r.scope {
            Scope::FuzzballRemote { host: Some(h) } => {
                lines.push(format!("Host: {h}"));
                lines.push("Allow always remembers this host, plugin and capabilities only.".into());
            }
            Scope::FuzzballRemote { host: None } => lines.push("Unknown remote host: Mouser asks every time.".into()),
            _ => {}
        }
        if more > 0 {
            lines.push(format!("({more} more waiting)"));
        }
        Some(ui::approval_panel(vp.width, vp.height, vp.cell_w, vp.cell_h, &format!("Mouser approval — {who}"), &lines, &offer(&r.scope), armed))
    }
}

/// The answers offered: remote gets once / always (known host) / deny.
pub fn offer(scope: &Scope) -> Vec<u8> {
    match scope {
        Scope::FuzzballRemote { host: Some(_) } => vec![0, 1, 2],
        Scope::FuzzballRemote { host: None } => vec![0, 2],
        _ => vec![0, 1, 2, 3],
    }
}
