//! Plugin approval popups (docs/plugins.md §Approval): a queue of requests the
//! permission broker said to ask about, shown one at a time as a Mouser panel
//! over the terminal. While one is open it owns the keyboard and mouse, so the
//! program in the tab can neither draw it nor answer it.

use crate::tabs::Session;
use mouser_core::permissions::{Answer, Scope};
use mouser_core::plugins;
use mouser_render::modes::prompt::{Prompt, PromptAnswer, PromptKind};
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
    /// An MCP tool call waiting for the answer.
    Mcp { reply: std::sync::mpsc::Sender<bool> },
    /// Let an MCP plugin start (sidebar MCP section).
    McpRun { id: String },
    /// Call this tool on an MCP server after a grant.
    McpTool { id: String, tool: String, args: Option<serde_json::Value> },
    /// Download these local-model weights (asked every time, never saved).
    ModelDownload { id: String },
    /// A mode plugin's question (`mouser_render::modes::prompt`).
    ModePrompt { reply: std::sync::mpsc::Sender<PromptAnswer>, tag: PromptTag },
}

/// Which kind of mode-plugin question a [`Origin::ModePrompt`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptTag {
    Approve,
    /// Type a key: the popup has a masked field; Save / Cancel.
    Secret,
    /// A permission from the plugin's manifest: Allow always / Deny / Never.
    Permission,
    /// An agent tool card: Allow / Allow all / Deny.
    AgentTool,
}

pub struct Request {
    pub plugin: String,
    pub caps: Vec<String>,
    /// Extra lines: the exact command, the tab, …
    pub detail: Vec<String>,
    pub scope: Scope,
    pub origin: Origin,
    /// Only "Allow once" / "Deny" (nothing is remembered).
    pub once_only: bool,
    /// A secret being typed (masked; only for [`PromptTag::Secret`]).
    pub secret: Option<String>,
    shown: Option<Instant>,
}

impl Request {
    pub fn new(plugin: &str, caps: Vec<String>, detail: Vec<String>, scope: Scope, origin: Origin) -> Self {
        Request { plugin: plugin.into(), caps, detail, scope, origin, once_only: false, secret: None, shown: None }
    }

    /// A mode plugin's question, drawn and answered here (never by the plugin).
    pub fn from_prompt(p: Prompt) -> Self {
        let (tag, caps, detail, secret) = match p.kind {
            PromptKind::Approve { title, detail } => {
                let mut d = vec![format!("  {title}")];
                d.extend(detail.into_iter().map(|l| format!("  {l}")));
                (PromptTag::Approve, vec![], d, None)
            }
            PromptKind::Secret { key, label } => (
                PromptTag::Secret,
                vec![],
                vec![format!("  {label} ({key})"), "  Saved in your keychain; the plugin never sees it.".into()],
                Some(String::new()),
            ),
            PromptKind::Permission { cap, detail } => (PromptTag::Permission, vec![cap], detail.into_iter().skip(1).collect(), None),
            PromptKind::AgentTool { display, lines } => {
                let mut d = vec![format!("  run {display}")];
                d.extend(lines.into_iter().map(|l| format!("  {l}")));
                d.push("  Allow all: every computer action in this chat runs without asking (until a new chat).".into());
                (PromptTag::AgentTool, vec![], d, None)
            }
        };
        let once_only = tag != PromptTag::Permission;
        Request {
            plugin: p.plugin,
            caps,
            detail,
            scope: Scope::Installed,
            origin: Origin::ModePrompt { reply: p.reply, tag },
            once_only,
            secret,
            shown: None,
        }
    }

    fn tag(&self) -> Option<PromptTag> {
        match &self.origin {
            Origin::ModePrompt { tag, .. } => Some(*tag),
            _ => None,
        }
    }

    /// A question that is never saved as a permission (e.g. a model download).
    pub fn once(who: &str, detail: Vec<String>, origin: Origin) -> Self {
        Request { once_only: true, ..Request::new(who, vec![], detail, Scope::Installed, origin) }
    }
}

/// Buttons for a request: (label, answer index, primary).
fn buttons_for(r: &Request) -> Vec<(&'static str, u8, bool)> {
    match r.tag() {
        Some(PromptTag::Secret) => vec![("Save [enter]", 0, true), ("Cancel [esc]", 2, false)],
        Some(PromptTag::AgentTool) => vec![("Allow [y]", 0, true), ("Allow all [a]", 1, false), ("Deny [n]", 2, false)],
        _ => ui::APPROVAL_BUTTONS.iter().filter(|b| offer_for(r).contains(&b.1)).copied().collect(),
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
        let armed = self.armed();
        let front = self.queue.front_mut()?;
        if let Some(buf) = &mut front.secret {
            match bytes {
                b"\x1b" => return Some((self.queue.pop_front()?, Answer::Deny)),
                b"\r" | b"\n" if armed && !buf.is_empty() => return Some((self.queue.pop_front()?, Answer::Once)),
                b"\x7f" | b"\x08" => {
                    buf.pop();
                }
                b"\x15" => buf.clear(),
                // Typed or pasted text (no escape sequences, no control characters).
                _ if bytes.first() != Some(&0x1b) => {
                    if let Ok(t) = std::str::from_utf8(bytes) {
                        // Up to an escape sequence (arrows etc. are ignored).
                        buf.extend(t.split('\x1b').next().unwrap_or("").chars().filter(|c| !c.is_control()));
                        buf.truncate(buf.char_indices().nth(4096).map_or(buf.len(), |(i, _)| i));
                    }
                }
                _ => {}
            }
            return None;
        }
        let offer = offer_for(front);
        let a = match bytes {
            b"y" | b"Y" | b"o" | b"\r" if offer.contains(&0) => Answer::Once,
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
        let front = self.queue.front()?;
        if front.secret.as_ref().is_some_and(|b| b.is_empty()) && i == 0 {
            return None;
        }
        let offer = offer_for(front);
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
            _ if r.tag() == Some(PromptTag::Secret) => format!("Plugin \"{}\" asks for a key:", r.plugin),
            _ if r.tag() == Some(PromptTag::Permission) => format!("Plugin \"{}\" asks for a permission:", r.plugin),
            _ if r.tag().is_some() => format!("Plugin \"{}\" asks:", r.plugin),
            _ if r.once_only => format!("{} (asked every time):", r.plugin),
            Scope::Installed if r.plugin.starts_with("mcp:") => format!("MCP client \"{}\" asks to:", &r.plugin[4..]),
            Scope::Installed => format!("Plugin \"{}\" asks to:", r.plugin),
            Scope::FuzzballLocal => format!("Program in this tab (\"{}\") asks to:", r.plugin),
            Scope::FuzzballRemote { .. } => format!("REMOTE program (over ssh, \"{}\") asks to:", r.plugin),
        };
        let mut lines: Vec<String> = r.caps.iter().map(|c| format!("  • {}  ({c})", plugins::capability_text(c))).collect();
        if r.caps.is_empty() && !r.once_only {
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
        if let Some(buf) = &r.secret {
            let dots = "•".repeat(buf.chars().count().min(48));
            lines.push(format!("  Key: {}", if dots.is_empty() { "(type or paste it)".to_string() } else { dots }));
        }
        if more > 0 {
            lines.push(format!("({more} more waiting)"));
        }
        let b = buttons_for(r);
        Some(ui::approval_panel_with(vp.width, vp.height, vp.cell_w, vp.cell_h, &format!("Mouser approval — {who}"), &lines, &b, armed))
    }
}

/// The answers offered: remote gets once / always (known host) / deny.
fn offer_for(r: &Request) -> Vec<u8> {
    match r.tag() {
        Some(PromptTag::Permission) => vec![1, 2, 3],
        Some(PromptTag::AgentTool) => vec![0, 1, 2],
        _ if r.once_only => vec![0, 2],
        _ => offer(&r.scope),
    }
}

/// What a mode plugin's question was answered with.
pub fn prompt_answer(r: &mut Request, a: Answer) -> PromptAnswer {
    match (a, r.secret.take()) {
        (Answer::Once, Some(v)) => PromptAnswer::Secret(v),
        (Answer::Once, None) => PromptAnswer::Once,
        (Answer::Always, _) if r.tag() == Some(PromptTag::AgentTool) => PromptAnswer::AllowAll,
        (Answer::Always, _) => PromptAnswer::Once,
        _ => PromptAnswer::Deny,
    }
}

pub fn offer(scope: &Scope) -> Vec<u8> {
    match scope {
        Scope::FuzzballRemote { host: Some(_) } => vec![0, 1, 2],
        Scope::FuzzballRemote { host: None } => vec![0, 2],
        _ => vec![0, 1, 2, 3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp() -> UiViewport {
        UiViewport { width: 1200, height: 800, cell_w: 8, cell_h: 16 }
    }

    fn arm(a: &mut Approvals) {
        a.panel_view(vp());
        a.queue.front_mut().unwrap().shown = Some(Instant::now() - ARM_DELAY * 2);
    }

    #[test]
    fn key_prompt_is_masked_and_goes_only_to_the_reply() {
        let (p, rx) = Prompt::new("prefs", PromptKind::Secret { key: "models.local.key".into(), label: "Server key".into() });
        let mut a = Approvals::default();
        a.push(Request::from_prompt(p));
        assert!(a.key(b"\r").is_none(), "Enter does nothing before it arms or while empty");
        a.key(b"sk-");
        a.key(b"12\x1b[A");
        a.key(b"34");
        a.key(b"\x7f");
        arm(&mut a);
        let panel = a.panel_view(vp()).unwrap();
        let text: Vec<&str> = panel.items.iter().map(|i| i.text.as_str()).collect();
        assert!(text.iter().any(|t| t.contains("Key: ••••••")), "{text:?}");
        assert!(!text.iter().any(|t| t.contains("sk-")), "never drawn in clear");
        let (mut req, ans) = a.key(b"\r").expect("answered");
        let pa = prompt_answer(&mut req, ans);
        assert_eq!(pa, PromptAnswer::Secret("sk-123".into()));
        if let Origin::ModePrompt { reply, .. } = req.origin {
            reply.send(pa).unwrap();
        }
        assert_eq!(rx.recv().unwrap(), PromptAnswer::Secret("sk-123".into()));
    }

    #[test]
    fn plugin_questions_offer_the_right_answers() {
        let mut a = Approvals::default();
        let (p, _rx) = Prompt::new("x", PromptKind::Permission { cap: "net.fetch".into(), detail: vec!["• …".into(), "Why: weather".into()] });
        a.push(Request::from_prompt(p));
        arm(&mut a);
        assert!(a.key(b"y").is_none(), "no Allow once for a permission");
        let (mut r, ans) = a.key(b"a").unwrap();
        assert_eq!((ans, r.once_only, r.caps.clone()), (Answer::Always, false, vec!["net.fetch".to_string()]));
        assert_eq!(prompt_answer(&mut r, ans), PromptAnswer::Once);
        let (p, _rx) = Prompt::new("chat", PromptKind::AgentTool { display: "mouser.shell_run".into(), lines: vec!["ls".into()] });
        a.push(Request::from_prompt(p));
        arm(&mut a);
        let labels: Vec<&str> = buttons_for(a.queue.front().unwrap()).iter().map(|b| b.0).collect();
        assert_eq!(labels, ["Allow [y]", "Allow all [a]", "Deny [n]"]);
        let (mut r, ans) = a.key(b"a").unwrap();
        assert_eq!(prompt_answer(&mut r, ans), PromptAnswer::AllowAll);
        let (p, _rx) = Prompt::new("chat", PromptKind::AgentTool { display: "mouser.terminal_run".into(), lines: vec!["ls".into()] });
        a.push(Request::from_prompt(p));
        arm(&mut a);
        let (mut r, ans) = a.key(b"n").unwrap();
        assert_eq!(prompt_answer(&mut r, ans), PromptAnswer::Deny, "deny is the refusal sent to the model");
        let (p, _rx) = Prompt::new("x", PromptKind::Approve { title: "Go?".into(), detail: vec![] });
        a.push(Request::from_prompt(p));
        assert!(a.key(b"a").is_none(), "Approve is once or deny");
        let (mut r, ans) = a.key(b"\x1b").unwrap();
        assert_eq!(prompt_answer(&mut r, ans), PromptAnswer::Deny);
    }
}
