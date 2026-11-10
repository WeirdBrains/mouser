//! Mouser's chat mode as a plugin program (`transport = "mode"`,
//! docs/chat.md). The agent session runs inside Mouser (`agent.send`,
//! `agent.cancel`, `agent.clear`): your models, API keys, the tools and their
//! approval cards stay there, and Mouser draws the cards itself (Allow /
//! Allow all / Deny). This program draws the conversation from `agent.state`
//! events in a grokbot-tui-like layout: a rounded frame, your messages in
//! bubbles on the right and the model's on the left (Markdown as it streams
//! in), the input line, a model picker, the Hugging Face browser
//! ([`hub`]) and players for media named in the chat ([`media`]).

mod hub;
mod media;

use mouser_media::detect::{self, MediaRef};
use mouser_mode_api::events::{Event, EventDecl, Listener};
use mouser_mode_api::host_api::Call;
use mouser_mode_api::markdown as md;
use mouser_mode_api::protocol::{Accepted, ApiRange, BindingDecl, Handled, Hello, ImageFormat, ImageRef, Pixels, Provide, Wants};
use mouser_mode_api::{Action, Context, Effect, Host, Mode, Panel, PanelItem, PanelStyle, Rect, dock_chrome_height, serve};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const PADDING: i32 = 8;
/// The host's fullscreen toggle for the mode panel (also its ⤢ and `F`).
const DOCK_FULL: &str = "mouser.dock_full";
/// Self-sent to draw again (a decoded picture, a video frame, an animation).
const TICK: &str = "chat.tick";
/// Images at least this big go through shared memory when there is a ring.
const SHM_MIN: usize = 256 << 10;
/// The chat's own commands (typed with `/`, Tab completes them).
const SLASH: &[(&str, &str)] = &[
    ("/hf", "search Hugging Face for a model to install (/hf qwen)"),
    ("/model", "switch model"),
    ("/new", "new chat"),
    ("/revoke", "turn Allow all off (ask again for every computer action)"),
    ("/help", "list these commands"),
];

#[derive(Debug, Clone, PartialEq)]
enum Entry {
    User(String),
    Assistant(String),
    Tool { display: String, args: String, state: String, output: String },
    Note(String),
    Error(String),
}

enum Asked {
    Send,
    State,
    Models,
    SetDefault,
    Granted,
    Permission,
    HfSearch,
    HfDetails,
    HfInstall,
    Other,
}

#[derive(Default)]
struct Chat {
    ctx: Option<Context>,
    entries: Vec<Entry>,
    running: bool,
    model: String,
    card_open: bool,
    /// "Allow all" is on for this chat (no cards until a new chat).
    allow_all: bool,
    input: String,
    focused: bool,
    scroll: usize,
    open: BTreeSet<usize>,
    links: Vec<String>,
    /// Model picker: (provider/model choices, highlighted). Choice 0 is
    /// "Search Hugging Face…".
    picker: Option<(Vec<String>, usize)>,
    default_model: Option<String>,
    status: Option<(String, bool)>,
    asked: HashMap<u64, Asked>,
    /// The mode panel's size the user chose.
    dock: mouser_mode_api::DockSize,
    /// Entries removed from the view ("delete from view"; the agent keeps them).
    hidden: BTreeSet<usize>,
    /// Rows drawn last: (rect, entry index), for a right-click.
    hits: Vec<(Rect, usize)>,
    complete: mouser_mode_api::complete::Engine,
    sent: Vec<String>,
    /// The Hugging Face browser, when open.
    hub: Option<hub::Hub>,
    hub_closed: Option<hub::Hub>,
    /// Mouser's last `models.download` state.
    download: Option<Value>,
    granted: Vec<String>,
    media: media::MediaView,
    /// A `chat.tick` is on its way.
    tick: Arc<AtomicBool>,
    /// Shared-memory slots not yet copied by Mouser.
    busy: Vec<bool>,
    /// Image keys Mouser already has.
    uploaded: HashSet<u64>,
    wants: Option<Wants>,
    /// The panel drawn last (for media fullscreen).
    area: Rect,
}

/// Where the context menu was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cx {
    /// On your message, an answer, a note or an error.
    Message,
    /// On a tool call card.
    Card,
    /// Anywhere else in the chat.
    Chat,
}

/// The chat's actions: (id, label, contexts). Bound ones show their key.
const ACTIONS: &[(&str, &str, &[Cx])] = &[
    ("copy", "copy", &[Cx::Message]),
    ("quote", "quote in a reply", &[Cx::Message]),
    ("retry", "retry", &[Cx::Message]),
    ("toggle", "show / hide the output", &[Cx::Card]),
    ("copy_output", "copy the output", &[Cx::Card]),
    ("copy", "copy the call", &[Cx::Card]),
    ("hide", "delete from view", &[Cx::Message, Cx::Card]),
    ("focus", "type a message", &[Cx::Message, Cx::Card, Cx::Chat]),
    ("cancel", "cancel the answer", &[Cx::Message, Cx::Card, Cx::Chat]),
    ("model", "switch model", &[Cx::Chat]),
    ("hub", "search Hugging Face models", &[Cx::Chat]),
    ("revoke", "turn Allow all off", &[Cx::Chat]),
    ("clear", "new chat", &[Cx::Chat]),
    ("up", "scroll up", &[Cx::Chat]),
    ("down", "scroll down", &[Cx::Chat]),
];

/// The chat's key bindings (`[chat.keys]` in modes.toml).
const KEYS: &[(&str, &str)] =
    &[("focus", "i"), ("cancel", "x"), ("model", "m"), ("hub", "h"), ("clear", "l"), ("up", "pageup"), ("down", "pagedown")];

fn act(id: &str, arg: i32) -> Action {
    Action::Mode { action: id.into(), arg }
}

fn item(rect: Rect, text: impl Into<String>, style: PanelStyle, action: Action) -> PanelItem {
    PanelItem { rect, text: text.into(), style, action }
}

/// Wrap to `cols` characters (hard-wraps long words).
pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(8);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let wl = word.chars().count();
            let ll = line.chars().count();
            if ll > 0 && ll + 1 + wl > cols {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
            while line.chars().count() > cols {
                let head: String = line.chars().take(cols).collect();
                line = line.chars().skip(cols).collect();
                out.push(head);
            }
        }
        out.push(line);
    }
    out
}

enum Body {
    Plain(String, PanelStyle, Action),
    Md {
        line: md::Line,
        code: (bool, bool),
    },
    /// Part `part` of `of` rows a player takes.
    Media {
        m: MediaRef,
        part: usize,
        of: usize,
    },
    /// Space between messages.
    Gap,
}

struct Row {
    entry: usize,
    /// Cells from the frame's left edge.
    indent: usize,
    /// In a bubble: (yours, its width in cells).
    bubble: Option<(bool, usize)>,
    body: Body,
}

/// How a tool call reads in the transcript (a command line for the terminal tool).
fn tool_line(display: &str, args: &str) -> String {
    if display == "mouser.terminal_run"
        && let Ok(v) = serde_json::from_str::<Value>(args)
        && let Some(c) = v["command"].as_str()
    {
        return format!("$ {c}");
    }
    let a: String = args.chars().take(160).collect();
    format!("{display} {a}")
}

/// The rows of the shown entries. `media`: show players (else one line
/// asking for `files.read`); `home` expands `~/` in paths.
fn rows(entries: &[Entry], open: &BTreeSet<usize>, hidden: &BTreeSet<usize>, cols: usize, media: bool, home: Option<&std::path::Path>) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if hidden.contains(&i) {
            continue;
        }
        if !out.is_empty() {
            out.push(Row { entry: i, indent: 0, bubble: None, body: Body::Gap });
        }
        entry_rows(i, e, open, cols, &mut out);
        let text = match e {
            Entry::User(t) | Entry::Assistant(t) => t.as_str(),
            Entry::Tool { output, .. } => output.as_str(),
            _ => "",
        };
        let indent = if matches!(e, Entry::User(_)) { cols.saturating_sub(cols * 3 / 4) } else { 1 };
        for m in detect::find(text, home) {
            if !media {
                let name = m.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                out.push(Row {
                    entry: i,
                    indent,
                    bubble: None,
                    body: Body::Plain(format!("▶ show {name} here (allow reading files)"), PanelStyle::Hint, act("media_allow", 0)),
                });
                continue;
            }
            let of = media::rows(m.kind);
            for part in 0..of {
                out.push(Row { entry: i, indent, bubble: None, body: Body::Media { m: m.clone(), part, of } });
            }
        }
    }
    out
}

fn entry_rows(i: usize, e: &Entry, open: &BTreeSet<usize>, cols: usize, v: &mut Vec<Row>) {
    let plain = |v: &mut Vec<Row>, text: &str, prefix: &str, style: PanelStyle, a: Action| {
        let pad: String = " ".repeat(prefix.chars().count());
        for (k, l) in wrap(text, cols.saturating_sub(prefix.chars().count() + 1)).into_iter().enumerate() {
            v.push(Row {
                entry: i,
                indent: 1,
                bubble: None,
                body: Body::Plain(format!("{}{l}", if k == 0 { prefix } else { &pad }), style, a.clone()),
            });
        }
    };
    match e {
        Entry::User(t) => {
            // Yours: on the right, in an accent bubble, as typed.
            let max = (cols * 3 / 4).max(12).min(cols);
            let lines = wrap(t, max.saturating_sub(2));
            let width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) + 2;
            let indent = cols.saturating_sub(width);
            for l in lines {
                v.push(Row { entry: i, indent: indent + 1, bubble: Some((true, width)), body: Body::Plain(l, PanelStyle::Label, Action::None) });
            }
        }
        Entry::Assistant(t) => {
            // The model's: on the left, Markdown.
            let max = (cols * 9 / 10).max(12).min(cols);
            let lines = md::layout(t, max.saturating_sub(2));
            if lines.is_empty() {
                v.push(Row { entry: i, indent: 1, bubble: Some((false, 3)), body: Body::Plain("…".into(), PanelStyle::Label, Action::None) });
                return;
            }
            let width = (lines.iter().map(|l| l.width()).max().unwrap_or(0) + 2).min(max);
            let code = |k: usize| lines.get(k).is_some_and(|l| l.kind == md::Kind::Code);
            for (k, l) in lines.iter().enumerate() {
                v.push(Row {
                    entry: i,
                    indent: 1,
                    bubble: Some((false, width)),
                    body: Body::Md { line: l.clone(), code: (k > 0 && code(k - 1), code(k + 1)) },
                });
            }
        }
        Entry::Tool { display, args, state, output } => {
            let st = match state.as_str() {
                "waiting" => "waiting for you",
                "running" => "running…",
                s => s,
            };
            let shown = open.contains(&i);
            let mark = if output.is_empty() {
                "•"
            } else if shown {
                "▾"
            } else {
                "▸"
            };
            let style = if matches!(state.as_str(), "failed" | "denied") { PanelStyle::Error } else { PanelStyle::Hint };
            plain(v, &format!("{mark} {} — {st}", tool_line(display, args)), "⚙ ", style, act("toggle", i as i32));
            if shown && !output.is_empty() {
                plain(v, output, "  │ ", PanelStyle::Hint, act("toggle", i as i32));
            }
        }
        Entry::Note(t) => plain(v, t, "", PanelStyle::Hint, Action::None),
        Entry::Error(t) => plain(v, t, "", PanelStyle::Error, Action::None),
    }
}

fn entry(v: &Value) -> Option<Entry> {
    let s = |k: &str| v[k].as_str().map(String::from);
    if let Some(t) = s("user") {
        return Some(Entry::User(t));
    }
    if let Some(t) = s("assistant") {
        return Some(Entry::Assistant(t));
    }
    if let Some(t) = s("note") {
        return Some(Entry::Note(t));
    }
    if let Some(t) = s("error") {
        return Some(Entry::Error(t));
    }
    let t = &v["tool"];
    t.is_object().then(|| Entry::Tool {
        display: t["display"].as_str().unwrap_or("").into(),
        args: t["args"].as_str().unwrap_or("").into(),
        state: t["state"].as_str().unwrap_or("").into(),
        output: t["output"].as_str().unwrap_or("").into(),
    })
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

impl Chat {
    fn call(&mut self, host: &Host, method: &str, params: Value, why: Asked) {
        if let Ok(id) = host.request(Call::Service { method: method.into(), params }) {
            self.asked.insert(id, why);
        }
    }

    fn apply_state(&mut self, v: &Value) {
        self.entries = v["entries"].as_array().into_iter().flatten().filter_map(entry).collect();
        self.running = v["running"].as_bool().unwrap_or(false);
        self.model = v["model"].as_str().unwrap_or("").into();
        self.card_open = v["card_open"].as_bool().unwrap_or(false);
        self.allow_all = v["allow_all"].as_bool().unwrap_or(false);
    }

    fn send(&mut self, host: &Host) {
        let t = std::mem::take(&mut self.input);
        if t.trim().is_empty() {
            return;
        }
        self.sent.retain(|s| *s != t);
        self.sent.push(t.clone());
        self.complete.reset();
        if t.starts_with('/') && !t.starts_with("//") {
            return self.slash(t.trim(), host);
        }
        self.scroll = 0;
        self.status = None;
        self.input = t.clone();
        self.call(host, "agent.send", json!({"text": t}), Asked::Send);
    }

    /// A `/command` typed in the input.
    fn slash(&mut self, t: &str, host: &Host) {
        let (cmd, rest) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
        match cmd {
            "/hf" | "/hub" => self.open_hub(rest.trim(), host),
            "/model" | "/models" => self.action("model", 0, host),
            "/new" | "/clear" => self.action("clear", 0, host),
            "/revoke" => self.action("revoke", 0, host),
            "/help" => {
                let l: Vec<String> = SLASH.iter().map(|(c, d)| format!("{c} — {d}")).collect();
                self.status = Some((l.join(" · "), false));
            }
            _ => self.status = Some((format!("unknown command {cmd} (try /help; start with // to send a /)"), true)),
        }
    }

    fn open_hub(&mut self, query: &str, host: &Host) {
        self.picker = None;
        // A closed browser keeps its searches and results.
        let mut h = self.hub.take().or_else(|| self.hub_closed.take()).unwrap_or_else(|| hub::Hub::new(""));
        if !query.is_empty() {
            h.query = query.to_string();
            h.details = None;
            let cmd = h.text("\r");
            self.hub = Some(h);
            if let Some(c) = cmd {
                self.hub_cmd(c, host);
            }
            return;
        }
        self.hub = Some(h);
    }

    fn hub_cmd(&mut self, c: hub::Cmd, host: &Host) {
        match c {
            hub::Cmd::Search(q) => self.call(host, "models.hf_search", json!({"query": q}), Asked::HfSearch),
            hub::Cmd::Details(id) => self.call(host, "models.hf_details", json!({"id": id}), Asked::HfDetails),
            hub::Cmd::Install { id, quant } => {
                self.status = Some(("confirm the download in Mouser's popup".into(), false));
                self.call(host, "models.hf_install", json!({"id": id, "quant": quant}), Asked::HfInstall);
            }
            hub::Cmd::Use(spec) => {
                self.hub_closed = self.hub.take();
                self.call(host, "agent.set_default", json!({"model": spec}), Asked::SetDefault);
            }
            hub::Cmd::Cancel => self.call(host, "models.hf_cancel", json!({}), Asked::Other),
            hub::Cmd::Close => self.hub_closed = self.hub.take(),
        }
    }

    /// Mouser's `models.download` state changed.
    fn download_changed(&mut self, v: &Value, host: &Host) {
        let state = v["state"].as_str().unwrap_or("");
        let was = self.download.as_ref().and_then(|d| d["state"].as_str().map(String::from));
        self.download = Some(v.clone());
        if was.as_deref() == Some(state) {
            return;
        }
        match state {
            "done" => {
                let spec = v["spec"].as_str().unwrap_or("");
                self.status = Some((format!("installed: {spec} is now the current model"), false));
                self.call(host, "agent.models", json!({}), Asked::Other);
                let repo = v["repo"].as_str().unwrap_or("").to_string();
                if self.hub.as_ref().and_then(|h| h.details.as_ref()).is_some_and(|d| d.id == repo) {
                    self.call(host, "models.hf_details", json!({"id": repo}), Asked::HfDetails);
                }
            }
            "failed" => self.status = Some((format!("download failed: {}", v["error"].as_str().unwrap_or("")), true)),
            "cancelled" => self.status = Some(("download cancelled; Install again resumes it".into(), false)),
            _ => {}
        }
    }

    fn entry_cx(&self, i: Option<usize>) -> Cx {
        match i.and_then(|i| self.entries.get(i)) {
            Some(Entry::Tool { .. }) => Cx::Card,
            Some(_) => Cx::Message,
            None => Cx::Chat,
        }
    }

    /// The context menu on entry `i` (`None`: off the messages).
    fn menu(&self, i: Option<usize>, keys: &[(String, String)]) -> Vec<mouser_mode_api::MenuEntry> {
        let cx = self.entry_cx(i);
        let e = i.and_then(|i| self.entries.get(i));
        ACTIONS
            .iter()
            .filter(|(_, _, c)| c.contains(&cx))
            .map(|(id, label, _)| {
                let enabled = match *id {
                    "cancel" => self.running,
                    "revoke" => self.allow_all,
                    "retry" => !self.running && matches!(e, Some(Entry::User(_) | Entry::Assistant(_) | Entry::Error(_))),
                    "copy_output" => matches!(e, Some(Entry::Tool { output, .. }) if !output.is_empty()),
                    _ => true,
                };
                mouser_mode_api::MenuEntry {
                    id: format!("menu:{id}"),
                    arg: i.map_or(-1, |i| i as i32),
                    label: (*label).into(),
                    key: keys.iter().find(|k| k.0 == *id).map(|k| k.1.clone()).unwrap_or_default(),
                    enabled,
                }
            })
            .collect()
    }

    fn open_menu(&mut self, x: i32, y: i32, host: &Host) {
        let i = if x < 0 {
            (0..self.entries.len()).rev().find(|i| !self.hidden.contains(i))
        } else {
            self.hits.iter().find(|(r, _)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h).map(|(_, i)| *i)
        };
        let (x, y) = if x < 0 {
            let r = i.and_then(|i| self.hits.iter().rev().find(|(_, j)| *j == i).map(|(r, _)| *r));
            r.map_or((40, 40), |r| (r.x + 4 * 9, r.y + r.h))
        } else {
            (x, y)
        };
        // Default keys; Mouser shows the ones bound now.
        let keys: Vec<(String, String)> = KEYS.iter().map(|(i, k)| (i.to_string(), k.to_string())).collect();
        let _ = host.menu(x, y, self.menu(i, &keys));
    }

    /// The text of entry `i` (a tool call: its name and arguments).
    fn entry_text(&self, i: usize) -> Option<String> {
        Some(match self.entries.get(i)? {
            Entry::User(t) | Entry::Assistant(t) | Entry::Note(t) | Entry::Error(t) => t.clone(),
            Entry::Tool { display, args, .. } => format!("{display} {args}"),
        })
    }

    fn menu_action(&mut self, id: &str, i: usize, host: &Host) {
        match id {
            "copy" => {
                if let Some(t) = self.entry_text(i) {
                    let _ = host.request(Call::ClipboardSet(t));
                    let _ = host.effect(Effect::Say("copied".into()));
                }
            }
            "copy_output" => {
                if let Some(Entry::Tool { output, .. }) = self.entries.get(i) {
                    let _ = host.request(Call::ClipboardSet(output.clone()));
                    let _ = host.effect(Effect::Say("copied".into()));
                }
            }
            "quote" => {
                if let Some(t) = self.entry_text(i) {
                    let q: Vec<String> = t.lines().map(|l| format!("> {l}")).collect();
                    self.input = format!("{}\n\n{}", q.join("\n"), self.input);
                    self.focused = true;
                }
            }
            "retry" => {
                // Your message again; on an answer, the message it answered.
                let ask = self.entries[..=i.min(self.entries.len().saturating_sub(1))].iter().rev().find_map(|e| match e {
                    Entry::User(t) => Some(t.clone()),
                    _ => None,
                });
                if let Some(t) = ask {
                    self.input = t;
                    self.send(host);
                }
            }
            "hide" => {
                self.hidden.insert(i);
            }
            "toggle" => self.action("toggle", i as i32, host),
            other => self.action(other, 0, host),
        }
    }

    fn action(&mut self, id: &str, arg: i32, host: &Host) {
        if let Some(m) = id.strip_prefix("menu:") {
            let m = m.to_string();
            return self.menu_action(&m, arg.max(0) as usize, host);
        }
        if id.starts_with("media_") {
            if id == "media_allow" {
                let why = "show the pictures, video and sound named in the chat";
                self.call(host, "permissions.request", json!({"permission": "files.read", "why": why}), Asked::Permission);
            } else {
                self.media.action(id, arg, self.area);
            }
            return;
        }
        if let Some(h) = self.hub.as_mut()
            && (id.starts_with("hub_") || matches!(id, "up" | "down" | "scroll"))
        {
            if let Some(c) = h.action(id, arg) {
                self.hub_cmd(c, host);
            }
            return;
        }
        if let Some((choices, sel)) = self.picker.as_mut() {
            match id {
                "up" => *sel = sel.saturating_sub(1),
                "down" => *sel = (*sel + 1).min(choices.len().saturating_sub(1)),
                "pick" => {
                    let i = if arg >= 0 { arg as usize } else { *sel };
                    if i == 0 {
                        self.picker = None;
                        return self.open_hub("", host);
                    }
                    if let Some(m) = choices.get(i).cloned() {
                        self.picker = None;
                        self.call(host, "agent.set_default", json!({"model": m}), Asked::SetDefault);
                    }
                }
                _ => self.picker = None,
            }
            return;
        }
        match id {
            "focus" => self.focused = true,
            "send" => self.send(host),
            "model" => self.call(host, "agent.models", json!({}), Asked::Models),
            "hub" => self.open_hub("", host),
            "cancel" => self.call(host, "agent.cancel", json!({}), Asked::Other),
            "revoke" => {
                self.call(host, "agent.revoke_all", json!({}), Asked::Other);
                self.allow_all = false;
                self.status = Some(("Allow all is off: every computer action asks again".into(), false));
            }
            "clear" => {
                self.call(host, "agent.clear", json!({}), Asked::Other);
                self.open.clear();
                self.hidden.clear();
                self.scroll = 0;
                self.media.player = None;
                self.media.full = None;
                self.media.stills.clear();
            }
            "link" => {
                if let Some(u) = self.links.get(arg.max(0) as usize) {
                    let _ = host.effect(Effect::OpenLink(u.clone()));
                }
            }
            "toggle" => {
                let i = arg.max(0) as usize;
                if !self.open.remove(&i) {
                    self.open.insert(i);
                }
            }
            "up" => self.scroll += 5,
            "down" => self.scroll = self.scroll.saturating_sub(5),
            "scroll" => self.scroll = (self.scroll as i64 - arg as i64).max(0) as usize,
            _ => {}
        }
    }

    fn text(&mut self, s: &str, host: &Host) {
        if self.media.is_open() {
            self.media.key(s, self.area);
            return;
        }
        if let Some(h) = self.hub.as_mut() {
            if let Some(c) = h.text(s) {
                self.hub_cmd(c, host);
            }
            return;
        }
        if self.picker.is_some() {
            match s {
                "\r" | "\n" => self.action("pick", -1, host),
                "\x1b[A" | "\x1bOA" => self.action("up", 0, host),
                "\x1b[B" | "\x1bOB" => self.action("down", 0, host),
                _ => {}
            }
            return;
        }
        if s == "\t" || s == "\x1b[Z" {
            self.complete_input(s == "\x1b[Z");
            return;
        }
        match s {
            // ↑ on an empty line: the last message again.
            "\x1b[A" | "\x1bOA" if self.input.is_empty() => {
                if let Some(t) = self.sent.last() {
                    self.input = t.clone();
                }
                return;
            }
            _ if s.starts_with('\x1b') => return,
            _ => {}
        }
        match s {
            "\r" | "\n" => self.send(host),
            "\x7f" | "\x08" => {
                self.input.pop();
                self.complete.reset();
            }
            "\x03" if self.running => self.call(host, "agent.cancel", json!({}), Asked::Other),
            "\x03" | "\x15" => {
                self.input.clear();
                self.complete.reset();
            }
            _ => {
                self.input.extend(s.chars().filter(|c| !c.is_control()));
                self.complete.reset();
            }
        }
    }

    fn complete_input(&mut self, reverse: bool) {
        use mouser_mode_api::complete::{self, FsSource};
        let input = self.input.clone();
        let (stem_pre, token) = match input.rfind(char::is_whitespace) {
            Some(i) => (input[..=i].to_string(), input[i + 1..].to_string()),
            None => (String::new(), input.clone()),
        };
        let looks_path = token.starts_with('/') && stem_pre.is_empty() && !SLASH.iter().any(|(c, _)| c.starts_with(&token));
        let (stem, token, matches) = if token.starts_with('/') && stem_pre.is_empty() && !looks_path {
            let cmds: Vec<String> = SLASH.iter().map(|(c, _)| format!("{c} ")).collect();
            complete::value_matches(&input, &cmds)
        } else if looks_path || token.starts_with('/') || token.starts_with('.') || token.starts_with('~') {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let (pstem, ptoken, m) = complete::path_matches(&token, &cwd, &FsSource, false);
            (format!("{stem_pre}{pstem}"), ptoken, m)
        } else {
            complete::value_matches(&input, &self.sent)
        };
        let out = self.complete.tab(&input, &stem, &token, &matches, reverse);
        self.input = out.text.clone();
        if let Some(msg) = out.message {
            self.status = Some((msg, false));
        } else if !out.candidates.is_empty() {
            self.status = Some((out.list_hint(72), false));
        }
    }

    fn escape(&mut self, host: &Host) {
        if self.media.is_open() {
            return self.media.close(self.area);
        }
        if let Some(h) = self.hub.as_mut() {
            if let Some(c) = h.escape() {
                self.hub_cmd(c, host);
            }
            return;
        }
        if self.picker.take().is_some() {
            return;
        }
        if self.running {
            self.call(host, "agent.cancel", json!({}), Asked::Other);
        } else {
            self.focused = false;
        }
    }

    fn can_read(&self) -> bool {
        self.granted.iter().any(|g| g == "files.read")
    }

    /// What the keyboard should bring: typing while the input, the browser,
    /// the picker or a fullscreen player is up; Escape while something is open.
    fn wants(&self) -> Wants {
        let text = self.focused || self.hub.is_some() || self.picker.is_some() || self.media.is_open();
        let escape = text || self.running;
        let keys = if text {
            ["\x1b[A", "\x1b[B", "\x1b[C", "\x1b[D", "\x1bOA", "\x1bOB", "\x1bOC", "\x1bOD", "\x1b[Z"].iter().map(|k| k.to_string()).collect()
        } else {
            vec![]
        };
        Wants { text, keys, escape }
    }

    /// The header's model name.
    fn model_name(&self) -> String {
        if !self.model.is_empty() { self.model.clone() } else { self.default_model.clone().unwrap_or_else(|| "no model".into()) }
    }

    fn draw(&mut self, host: &Host) {
        let Some(ctx) = self.ctx else { return };
        let vp = ctx.viewport;
        let (cw, ch) = (vp.cell_w.max(1), vp.cell_h.max(1));
        let a = ctx.area;
        let max_h = (a.h - ch * 8).max(ch * 6);
        let h = self.dock.height(a, ch, (a.h / 2).max(ch * 12).min(max_h));
        let rect = Rect { x: a.x + PADDING, y: a.y, w: a.w - 2 * PADDING, h };
        self.area = rect;
        self.media.reduce_motion = ctx.reduce_motion;
        self.media.begin();
        let row_h = ch + 4;
        let (x0, w) = (rect.x + 4, rect.w - 8);
        let mut items = Vec::new();
        let mut pics = Vec::new();
        // Header: the model (click to switch) and the chat's buttons.
        let model = self.model_name();
        let y = rect.y + 4;
        items.push(item(Rect { x: x0, y, w: w / 2, h: row_h }, format!("◆ {model} ▾"), PanelStyle::Title, act("model", 0)));
        let mut bx = x0 + w;
        let mut button = |t: &str, a: Action, primary: bool, items: &mut Vec<PanelItem>| {
            let bw = (t.chars().count() as i32 + 2) * cw;
            bx -= bw;
            items.push(item(Rect { x: bx, y, w: bw, h: row_h }, t, PanelStyle::Button { primary }, a));
            bx -= cw / 2;
        };
        button(if self.dock.full { "⤡" } else { "⤢" }, act(DOCK_FULL, 0), false, &mut items);
        button("new", act("clear", 0), false, &mut items);
        button("⌕ models", act("hub", 0), self.hub.is_some(), &mut items);
        if self.allow_all {
            button("allow all: on ✕", act("revoke", 0), false, &mut items);
        }
        if self.running {
            button("cancel esc", act("cancel", 0), true, &mut items);
        }
        // Leave the host's bottom chrome free (⤢ fullscreen + focus hint).
        let chrome = dock_chrome_height(ch);
        let input_y = rect.y + rect.h - row_h - 8 - chrome;
        let frame = Rect { x: x0, y: y + row_h + ch / 2 + 2, w, h: (input_y - 6) - (y + row_h + ch / 2 + 2) };
        let state = if self.hub.is_some() {
            "Hugging Face".to_string()
        } else if self.card_open {
            "waiting for you".into()
        } else if self.running {
            "thinking…".into()
        } else if self.allow_all {
            "allow all".into()
        } else {
            "ready".into()
        };
        let short: String = model.rsplit('/').next().unwrap_or(&model).chars().take(40).collect();
        let focused = self.focused && self.picker.is_none() && self.hub.is_none();
        items.push(item(frame, format!("Chat: {short} · {state}"), PanelStyle::Frame { focused: focused || self.hub.is_some() }, Action::None));
        let inner = Rect { x: frame.x + cw, y: frame.y + ch / 2 + 6, w: frame.w - 2 * cw, h: frame.h - ch / 2 - 12 };
        let cols = (inner.w / cw).max(10) as usize;
        let mut bottom = inner.y + inner.h;
        // The status line, inside the frame at the bottom.
        if let Some((s, err)) = &self.status {
            bottom -= row_h;
            let s: String = s.chars().take(cols).collect();
            items.push(item(
                Rect { x: inner.x, y: bottom, w: inner.w, h: row_h },
                s,
                if *err { PanelStyle::Error } else { PanelStyle::Hint },
                Action::None,
            ));
        } else if self.card_open {
            bottom -= row_h;
            items.push(item(
                Rect { x: inner.x, y: bottom, w: inner.w, h: row_h },
                "waiting for your answer in Mouser's popup: Allow, Allow all or Deny",
                PanelStyle::Hint,
                Action::None,
            ));
        } else if let Some(d) = self.download.as_ref().filter(|d| d["state"] == "downloading" && self.hub.is_none()) {
            bottom -= row_h;
            let (done, total) = (d["done"].as_u64().unwrap_or(0), d["total"].as_u64().unwrap_or(0).max(1));
            let t = format!("downloading {} {}: {}%", d["repo"].as_str().unwrap_or(""), d["quant"].as_str().unwrap_or(""), done * 100 / total);
            let tw = (t.chars().count() as i32 + 1) * cw;
            items.push(item(Rect { x: inner.x, y: bottom, w: tw, h: row_h }, t, PanelStyle::Hint, act("hub", 0)));
            let per = (done * 1000 / total).min(1000) as u16;
            items.push(item(
                Rect { x: inner.x + tw, y: bottom + 3, w: (inner.w - tw).max(cw), h: row_h - 6 },
                "",
                PanelStyle::Progress { done: per, hover: u16::MAX },
                Action::None,
            ));
        }
        // The input line.
        let input_text = if self.input.is_empty() && !self.focused {
            "> message, or /hf to find a model (i to type)".to_string()
        } else if self.focused {
            format!("> {}▏", self.input)
        } else {
            format!("> {}", self.input)
        };
        let shown: String = {
            let n = input_text.chars().count();
            let room = (w / cw).max(4) as usize - 2;
            input_text.chars().skip(n.saturating_sub(room)).collect()
        };
        items.push(item(Rect { x: x0, y: input_y, w, h: row_h + 4 }, shown, PanelStyle::Field { focused, valid: true }, act("focus", 0)));
        self.hits.clear();
        self.links.clear();
        if let Some(hb) = self.hub.as_mut() {
            hb.draw(Rect { h: bottom - inner.y, ..inner }, (cw, row_h), true, self.download.as_ref(), &mut items);
        } else {
            self.draw_transcript(Rect { h: bottom - inner.y, ..inner }, (cw, row_h), &mut items, &mut pics);
        }
        if let Some((choices, sel)) = &self.picker {
            let pw = (w / 2).max(cw * 34);
            let ph = (choices.len().max(1) as i32) * row_h;
            let top = y + row_h;
            items.push(item(Rect { x: x0, y: top, w: pw, h: ph }, "", PanelStyle::Popup, Action::None));
            for (j, c) in choices.iter().enumerate() {
                let r = Rect { x: x0, y: top + j as i32 * row_h, w: pw, h: row_h };
                items.push(item(r, c.clone(), PanelStyle::Option { selected: j == *sel, highlight: 0 }, act("pick", j as i32)));
            }
            if choices.len() == 1 {
                items.push(item(
                    Rect { x: x0, y: top + row_h, w: pw, h: row_h },
                    "no model set up yet: install one from Hugging Face above",
                    PanelStyle::Hint,
                    Action::None,
                ));
            }
        }
        // A fullscreen picture or player covers the chat's space.
        self.media.fullscreen(rect, (cw, row_h), &mut items, &mut pics);
        self.send_view(host, Panel { rect, items }, pics);
        let w = self.wants();
        if self.wants.as_ref() != Some(&w) {
            let _ = host.wants(w.clone());
            self.wants = Some(w);
        }
    }

    /// The messages, newest at the bottom, scrolled by `self.scroll` rows.
    fn draw_transcript(&mut self, inner: Rect, (cw, row_h): (i32, i32), items: &mut Vec<PanelItem>, pics: &mut Vec<media::Pic>) {
        let cols = (inner.w / cw).max(10) as usize;
        let home = home();
        let all = rows(&self.entries, &self.open, &self.hidden, cols, self.can_read(), home.as_deref());
        let fit = (inner.h / row_h).max(0) as usize;
        self.scroll = self.scroll.min(all.len().saturating_sub(fit));
        let end = all.len() - self.scroll;
        let start = end.saturating_sub(fit);
        let y = inner.y;
        if all.is_empty() {
            let hint = "Ask about what is on the screen, or ask it to do something: it can run commands in a terminal. \
                        Every computer action asks first in Mouser's popup (Allow, Allow all for this chat, or Deny). \
                        /hf finds a model on Hugging Face; m switches models; ⤢ or F fills the terminal space.";
            for (i, l) in wrap(hint, cols).into_iter().enumerate() {
                items.push(item(Rect { x: inner.x, y: y + i as i32 * row_h, w: inner.w, h: row_h }, l, PanelStyle::Hint, Action::None));
            }
            return;
        }
        let shown = &all[start..end];
        // Bubbles first, behind their rows.
        let mut k = 0;
        while k < shown.len() {
            let r = &shown[k];
            let Some((mine, width)) = r.bubble else {
                k += 1;
                continue;
            };
            let mut n = 1;
            while k + n < shown.len() && shown[k + n].entry == r.entry && shown[k + n].bubble.is_some() {
                n += 1;
            }
            let bx = inner.x + (r.indent as i32 - 1) * cw;
            let by = y + k as i32 * row_h;
            items.push(item(Rect { x: bx, y: by - 2, w: width as i32 * cw, h: n as i32 * row_h + 4 }, "", PanelStyle::Bubble { mine }, Action::None));
            k += n;
        }
        let links = &mut self.links;
        let mut link = |u: &str| {
            links.push(u.to_string());
            act("link", links.len() as i32 - 1)
        };
        for (i, r) in shown.iter().enumerate() {
            let ry = y + i as i32 * row_h;
            let x = inner.x + r.indent as i32 * cw;
            let rw = inner.w - r.indent as i32 * cw;
            self.hits.push((Rect { x: inner.x, y: ry, w: inner.w, h: row_h }, r.entry));
            match &r.body {
                Body::Plain(t, st, a) => items.push(item(Rect { x, y: ry, w: rw, h: row_h }, t.clone(), *st, a.clone())),
                Body::Md { line, code } => {
                    let bw = r.bubble.map_or(rw, |(_, w)| (w as i32 - 2) * cw);
                    md::line_items(line, *code, x, ry, (cw, row_h), bw, &mut link, items);
                }
                Body::Gap => {}
                Body::Media { m, part, of } => {
                    // Drawn once, from its first row, when all of it is in view.
                    let first_visible = i == 0 || !matches!(&shown[i - 1].body, Body::Media { m: pm, .. } if pm == m);
                    if !first_visible {
                        continue;
                    }
                    if *part == 0 && i + of <= shown.len() {
                        let mr = Rect { x, y: ry, w: rw, h: *of as i32 * row_h };
                        self.media.inline(m, mr, (cw, row_h), items, pics);
                    } else {
                        let name = m.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        items.push(item(Rect { x, y: ry, w: rw, h: row_h }, format!("▶ {name} (scroll to see it)"), PanelStyle::Hint, Action::None));
                    }
                }
            }
        }
        if start > 0 {
            items.push(item(
                Rect { x: inner.x + inner.w - 12 * cw, y, w: 12 * cw, h: row_h },
                format!("↑ {start} more"),
                PanelStyle::Hint,
                act("up", 0),
            ));
        }
    }

    /// Send the view: pictures Mouser already has by key, big new ones
    /// through free shared-memory slots, the rest inline.
    fn send_view(&mut self, host: &Host, panel: Panel, pics: Vec<media::Pic>) {
        let slot_bytes = host.with_slot(0, |s| s.len()).unwrap_or(0);
        let ring = slot_bytes > 0 && !self.busy.is_empty();
        let mut free = (0..self.busy.len() as u32).filter(|&s| !self.busy[s as usize]).collect::<Vec<_>>().into_iter();
        let mut sent = HashSet::new();
        let images = pics
            .into_iter()
            .map(|p| {
                let n = p.rgba.len();
                let pixels = if self.uploaded.contains(&p.key) || sent.contains(&p.key) {
                    Pixels::Cached
                } else {
                    let slot = if ring && n >= SHM_MIN && n <= slot_bytes { free.next() } else { None };
                    match slot.and_then(|s| host.with_slot(s, |b| b[..n].copy_from_slice(&p.rgba)).map(|_| s)) {
                        Some(s) => {
                            self.busy[s as usize] = true;
                            Pixels::Slot(s)
                        }
                        None => Pixels::Inline { format: ImageFormat::Rgba, bytes: p.rgba.to_vec() },
                    }
                };
                sent.insert(p.key);
                ImageRef { key: p.key, width: p.w, height: p.h, src: (0, 0, p.w, p.h), rect: p.rect, pixels }
            })
            .collect();
        self.uploaded = sent;
        let _ = host.view(Some(panel), None, images, false);
    }

    /// Draw again from another thread (a decoded picture, a video frame).
    /// One `chat.tick` at a time, at most one per 8 ms (well under the
    /// host's emit limit); the caller is a player, decoder or animation thread.
    fn waker(&self, host: &Host) -> mouser_media::Waker {
        let (h, flag) = (host.clone(), self.tick.clone());
        let last = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
        Arc::new(move || {
            if flag.swap(true, Ordering::SeqCst) {
                return;
            }
            let mut l = last.lock().unwrap_or_else(|e| e.into_inner());
            let gap = std::time::Duration::from_millis(8);
            if let Some(wait) = gap.checked_sub(l.elapsed()) {
                std::thread::sleep(wait);
            }
            *l = std::time::Instant::now();
            let _ = h.emit(TICK, json!({}));
        })
    }
}

impl Mode for Chat {
    fn hello(&self) -> Hello {
        Hello {
            api: ApiRange::CURRENT,
            name: "chat".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            provides: vec![Provide::Mode {
                hotkey: 'a',
                bindings: KEYS.iter().map(|(id, k)| BindingDecl { id: (*id).into(), key: (*k).into(), describe: String::new() }).collect(),
                activate_on: vec![],
            }],
            listeners: [
                "mode.*",
                "ui.action",
                "input.text",
                "input.escape",
                "view.resized",
                "menu.open",
                "agent.state",
                "models.*",
                "frame.presented",
                TICK,
            ]
            .iter()
            .map(|e| Listener::on(e))
            .collect(),
            emits: vec![EventDecl { name: TICK.into(), schema: None }],
            wants_mcp: false,
            shm: true,
        }
    }

    fn welcome(&mut self, w: &Accepted, host: &Host) {
        self.ctx = Some(w.ctx);
        self.granted = w.granted.clone();
        self.busy = vec![false; w.shm.as_ref().map_or(0, |r| r.slots as usize)];
        self.media.waker = Some(self.waker(host));
        self.focused = true;
        self.call(host, "agent.state", json!({}), Asked::State);
        self.call(host, "agent.models", json!({}), Asked::Other);
    }

    fn event(&mut self, e: &Event, host: &Host) -> Handled {
        match e.name.as_str() {
            "mode.activated" => self.focused = true,
            "mode.deactivated" => {
                self.focused = false;
                // Nothing plays in a hidden mode.
                if let Some(p) = self.media.player.as_mut()
                    && p.playing()
                {
                    p.pause();
                }
                self.uploaded.clear();
            }
            "ui.action" => self.action(e.payload["id"].as_str().unwrap_or(""), e.payload["arg"].as_i64().unwrap_or(0) as i32, host),
            "input.text" => self.text(e.payload["utf8"].as_str().unwrap_or(""), host),
            "input.escape" => self.escape(host),
            "agent.state" => self.apply_state(&e.payload),
            "models.download" => self.download_changed(&e.payload, host),
            TICK => self.tick.store(false, Ordering::SeqCst),
            "frame.presented" => {
                let slot = e.payload["slot"].as_u64().unwrap_or(u64::MAX) as usize;
                if let Some(b) = self.busy.get_mut(slot) {
                    *b = false;
                }
                return Handled::Handled;
            }
            "menu.open" => {
                let (x, y) = (e.payload["x"].as_i64().unwrap_or(-1) as i32, e.payload["y"].as_i64().unwrap_or(-1) as i32);
                self.open_menu(x, y, host);
            }
            "view.resized" => {
                self.dock = mouser_mode_api::DockSize::from_payload(&e.payload);
                if let Ok(c) = serde_json::from_value::<Context>(e.payload.clone()) {
                    self.ctx = Some(c);
                }
            }
            _ => return Handled::Pass,
        }
        self.draw(host);
        Handled::Handled
    }

    fn response(&mut self, id: u64, result: Result<Value, String>, host: &Host) {
        let why = self.asked.remove(&id).unwrap_or(Asked::Other);
        match (why, result) {
            (Asked::Send, Ok(_)) => self.input.clear(),
            (Asked::State, Ok(v)) => self.apply_state(&v),
            (why @ (Asked::Models | Asked::Other), Ok(v)) if v.get("providers").is_some() => {
                self.default_model = v["default"].as_str().map(String::from);
                let mut choices: Vec<String> = vec!["⌕ Search Hugging Face…".into()];
                choices.extend(v["providers"].as_array().into_iter().flatten().filter(|p| p["configured"].as_bool().unwrap_or(false)).flat_map(
                    |p| {
                        let n = p["name"].as_str().unwrap_or("").to_string();
                        p["models"].as_array().cloned().unwrap_or_default().into_iter().filter_map(move |m| m.as_str().map(|m| format!("{n}/{m}")))
                    },
                ));
                // Only the `model` action opens the picker.
                if matches!(why, Asked::Models) {
                    let sel = self.default_model.as_ref().and_then(|d| choices.iter().position(|c| c == d)).unwrap_or(0);
                    self.picker = Some((choices, sel));
                }
            }
            (Asked::SetDefault, Ok(v)) => {
                self.default_model = v.as_str().map(String::from);
                self.status = Some((format!("model: {}", v.as_str().unwrap_or("")), false));
            }
            (Asked::Granted, Ok(v)) => self.granted = v.as_array().into_iter().flatten().filter_map(|g| g.as_str().map(String::from)).collect(),
            (Asked::Permission, Ok(_)) => self.call(host, "permissions.granted", json!({}), Asked::Granted),
            (Asked::HfSearch, Ok(v)) => {
                if let Some(h) = self.hub.as_mut() {
                    h.hits_from(&v)
                }
            }
            (Asked::HfDetails, Ok(v)) => {
                if let Some(h) = self.hub.as_mut() {
                    h.details_from(&v)
                }
            }
            (Asked::HfInstall, Ok(v)) => {
                self.status = Some(if v["decision"] == "deny" {
                    ("not downloaded".into(), false)
                } else {
                    ("downloading; it becomes the current model when done".into(), false)
                });
            }
            (Asked::HfSearch | Asked::HfDetails | Asked::HfInstall, Err(e)) => match self.hub.as_mut() {
                Some(h) => h.failed(e),
                None => self.status = Some((e, true)),
            },
            (_, Err(e)) => self.status = Some((e, true)),
            _ => {}
        }
        self.draw(host);
    }
}

fn main() {
    if let Err(e) = serve(Chat::default()) {
        eprintln!("mouser-chat: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod menu_tests {
    use super::*;

    fn ids(c: &Chat, i: Option<usize>) -> Vec<(String, String, bool)> {
        let keys: Vec<(String, String)> = KEYS.iter().map(|(i, k)| (i.to_string(), k.to_string())).collect();
        c.menu(i, &keys).into_iter().map(|e| (e.id.trim_start_matches("menu:").to_string(), e.key, e.enabled)).collect()
    }

    fn chat() -> Chat {
        Chat {
            entries: vec![
                Entry::User("hi".into()),
                Entry::Assistant("hello".into()),
                Entry::Tool { display: "run".into(), args: "ls".into(), state: "done".into(), output: "a b".into() },
            ],
            ..Chat::default()
        }
    }

    #[test]
    fn message_card_and_chat_menus() {
        let c = chat();
        let m = ids(&c, Some(1));
        for id in ["copy", "quote", "retry", "hide", "focus"] {
            assert!(m.iter().any(|e| e.0 == id), "message: {id}");
        }
        assert!(m.iter().any(|e| e.0 == "focus" && e.1 == "i"), "the key is shown");
        assert!(!m.iter().find(|e| e.0 == "cancel").unwrap().2, "nothing running: cancel is greyed");
        let card = ids(&c, Some(2));
        for id in ["toggle", "copy_output", "copy", "hide"] {
            assert!(card.iter().any(|e| e.0 == id), "card: {id}");
        }
        assert!(!card.iter().any(|e| e.0 == "retry" || e.0 == "quote"));
        let off = ids(&c, None);
        for (id, key) in [("model", "m"), ("clear", "l"), ("up", "pageup"), ("down", "pagedown")] {
            assert!(off.iter().any(|e| e.0 == id && e.1 == key), "chat: {id}");
        }
        assert!(!off.iter().any(|e| e.0 == "copy"));
    }

    #[test]
    fn delete_from_view_hides_only_that_entry() {
        let mut c = chat();
        c.hidden.insert(0);
        let r = rows(&c.entries, &c.open, &c.hidden, 80, true, None);
        assert!(r.iter().all(|r| r.entry != 0) && r.iter().any(|r| r.entry == 1));
        assert_eq!(c.entries.len(), 3, "the agent's history is untouched");
    }

    fn text_of(r: &Row) -> String {
        match &r.body {
            Body::Plain(t, ..) => t.clone(),
            Body::Md { line, .. } => line.spans.iter().map(|s| s.text.as_str()).collect(),
            _ => String::new(),
        }
    }

    #[test]
    fn yours_on_the_right_the_models_on_the_left_in_bubbles() {
        let c = chat();
        let r = rows(&c.entries, &c.open, &c.hidden, 80, true, None);
        let you = r.iter().find(|r| r.entry == 0).unwrap();
        assert_eq!(text_of(you), "hi");
        assert_eq!(you.bubble, Some((true, 4)));
        assert_eq!(you.indent, 80 - 4 + 1, "right-aligned");
        let ai = r.iter().find(|r| r.entry == 1 && !matches!(r.body, Body::Gap)).unwrap();
        assert_eq!((ai.indent, ai.bubble.map(|b| b.0)), (1, Some(false)));
        assert!(matches!(ai.body, Body::Md { .. }), "Markdown");
        let tool = r.iter().find(|r| r.entry == 2 && !matches!(r.body, Body::Gap)).unwrap();
        assert!(tool.bubble.is_none() && text_of(tool).contains("run ls — done"));
        assert!(r.iter().any(|r| matches!(r.body, Body::Gap)), "space between messages");
    }

    #[test]
    fn terminal_calls_read_as_a_command_line() {
        assert_eq!(tool_line("mouser.terminal_run", r#"{"command":"ls -la","cwd":"/tmp"}"#), "$ ls -la");
        assert_eq!(tool_line("mouser.tab_list", "{}"), "mouser.tab_list {}");
    }

    #[test]
    fn media_rows_need_files_read() {
        let entries = vec![Entry::Tool {
            display: "mouser.terminal_run".into(),
            args: "{}".into(),
            state: "done".into(),
            output: "wrote /tmp/plot.png and /tmp/a.wav".into(),
        }];
        let none = BTreeSet::new();
        let r = rows(&entries, &none, &none, 80, false, None);
        let asks: Vec<String> = r.iter().filter(|r| matches!(&r.body, Body::Plain(_, _, a) if *a == act("media_allow", 0))).map(text_of).collect();
        assert_eq!(asks.len(), 2, "{asks:?}");
        let r = rows(&entries, &none, &none, 80, true, None);
        let parts = |k| r.iter().filter(|r| matches!(&r.body, Body::Media { m, .. } if m.kind == k)).count();
        assert_eq!(
            (parts(detect::Kind::Image), parts(detect::Kind::Audio)),
            (media::PICTURE_ROWS, media::SOUND_ROWS),
            "shown even with the output folded"
        );
    }

    #[test]
    fn keyboard_wants_follow_what_is_open() {
        let mut c = chat();
        assert!(!c.wants().text && !c.wants().escape, "not focused: keys go to the bindings");
        c.focused = true;
        let w = c.wants();
        assert!(w.text && w.escape && w.keys.contains(&"\x1b[A".to_string()));
        c.focused = false;
        c.hub = Some(hub::Hub::new(""));
        assert!(c.wants().text, "the browser's search field takes typing");
    }

    #[test]
    fn slash_commands_complete_with_tab() {
        let mut c = chat();
        c.input = "/h".into();
        c.complete_input(false);
        assert!(c.input == "/hf " || c.status.as_ref().is_some_and(|s| s.0.contains("/hf")), "{:?} {:?}", c.input, c.status);
        c.input = "/ne".into();
        c.complete.reset();
        c.complete_input(false);
        assert_eq!(c.input, "/new ");
    }

    /// Every binding appears in some menu.
    #[test]
    fn every_key_is_in_a_menu() {
        for (id, _) in KEYS {
            assert!(ACTIONS.iter().any(|a| a.0 == *id), "{id}");
        }
    }
}
