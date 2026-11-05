//! Mouser's chat mode as a plugin program (`transport = "mode"`,
//! docs/chat.md). The agent session runs inside Mouser (`agent.send`,
//! `agent.cancel`, `agent.clear`): your models, API keys, the tools and their
//! approval cards stay there, and Mouser draws the cards itself. This
//! program only draws the conversation from `agent.state` events (Markdown
//! replies as they stream in), the input line and a model picker.

use mouser_mode_api::events::{Event, Listener};
use mouser_mode_api::host_api::Call;
use mouser_mode_api::markdown as md;
use mouser_mode_api::protocol::{Accepted, ApiRange, BindingDecl, Handled, Hello, Provide};
use mouser_mode_api::{Action, Context, Effect, Host, Mode, Panel, PanelItem, PanelStyle, Rect, serve};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};

const PADDING: i32 = 8;
/// Prefix width of transcript rows (`you  `, `ai   `).
const PREFIX: usize = 5;

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
    Other,
}

#[derive(Default)]
struct Chat {
    ctx: Option<Context>,
    entries: Vec<Entry>,
    running: bool,
    model: String,
    card_open: bool,
    input: String,
    focused: bool,
    scroll: usize,
    open: BTreeSet<usize>,
    links: Vec<String>,
    /// Model picker: (provider/model choices, highlighted).
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
    ("clear", "new chat", &[Cx::Chat]),
    ("up", "scroll up", &[Cx::Chat]),
    ("down", "scroll down", &[Cx::Chat]),
];

/// The chat's key bindings (`[chat.keys]` in modes.toml).
const KEYS: &[(&str, &str)] = &[("focus", "i"), ("cancel", "x"), ("model", "m"), ("clear", "l"), ("up", "pageup"), ("down", "pagedown")];

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

enum Row {
    Plain(String, PanelStyle, Action),
    Md { prefix: String, style: PanelStyle, line: md::Line, code: (bool, bool) },
}

/// The rows of the shown entries, each with its entry's index.
fn rows(entries: &[Entry], open: &BTreeSet<usize>, hidden: &BTreeSet<usize>, cols: usize) -> Vec<(usize, Row)> {
    let mut out: Vec<(usize, Row)> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if hidden.contains(&i) {
            continue;
        }
        // Tool rows toggle their own entry: keep the index in their action.
        out.extend(entry_rows(i, e, open, cols).into_iter().map(|r| (i, r)));
    }
    out
}

fn entry_rows(i: usize, e: &Entry, open: &BTreeSet<usize>, cols: usize) -> Vec<Row> {
    let mut v: Vec<Row> = Vec::new();
    let add = |v: &mut Vec<Row>, text: &str, prefix: &str, style: PanelStyle, a: Action| {
        let pad: String = " ".repeat(prefix.chars().count());
        for (i, l) in wrap(text, cols.saturating_sub(prefix.chars().count())).into_iter().enumerate() {
            v.push(Row::Plain(format!("{}{l}", if i == 0 { prefix } else { &pad }), style, a.clone()));
        }
    };
    let markdown = |v: &mut Vec<Row>, text: &str, prefix: &str, style: PanelStyle| {
        let lines = md::layout(text, cols.saturating_sub(PREFIX));
        let code = |i: usize| lines.get(i).is_some_and(|l| l.kind == md::Kind::Code);
        for (i, l) in lines.iter().enumerate() {
            let prefix = if i == 0 { prefix.to_string() } else { String::new() };
            v.push(Row::Md { prefix, style, line: l.clone(), code: (i > 0 && code(i - 1), code(i + 1)) });
        }
        !lines.is_empty()
    };
    {
        match e {
            Entry::User(t) => {
                if !markdown(&mut v, t, "you  ", PanelStyle::Title) {
                    add(&mut v, t, "you  ", PanelStyle::Title, Action::None);
                }
            }
            Entry::Assistant(t) => {
                if !markdown(&mut v, t, "ai   ", PanelStyle::Label) {
                    add(&mut v, "…", "ai   ", PanelStyle::Label, Action::None);
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
                let a: String = args.chars().take(160).collect();
                add(&mut v, &format!("{mark} {display} {a} — {st}"), "tool ", style, act("toggle", i as i32));
                if shown && !output.is_empty() {
                    add(&mut v, output, "     │ ", PanelStyle::Hint, act("toggle", i as i32));
                }
            }
            Entry::Note(t) => add(&mut v, t, "note ", PanelStyle::Hint, Action::None),
            Entry::Error(t) => add(&mut v, t, "err  ", PanelStyle::Error, Action::None),
        }
    }
    v
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
    }

    fn send(&mut self, host: &Host) {
        let t = std::mem::take(&mut self.input);
        if t.trim().is_empty() {
            return;
        }
        self.scroll = 0;
        self.status = None;
        self.input = t.clone();
        self.call(host, "agent.send", json!({"text": t}), Asked::Send);
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
        if let Some((choices, sel)) = self.picker.as_mut() {
            match id {
                "up" => *sel = sel.saturating_sub(1),
                "down" => *sel = (*sel + 1).min(choices.len().saturating_sub(1)),
                "pick" => {
                    let i = if arg >= 0 { arg as usize } else { *sel };
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
            "cancel" => self.call(host, "agent.cancel", json!({}), Asked::Other),
            "clear" => {
                self.call(host, "agent.clear", json!({}), Asked::Other);
                self.open.clear();
                self.hidden.clear();
                self.scroll = 0;
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
        if self.picker.is_some() {
            match s {
                "\r" | "\n" => self.action("pick", -1, host),
                "\x1b[A" | "\x1bOA" => self.action("up", 0, host),
                "\x1b[B" | "\x1bOB" => self.action("down", 0, host),
                _ => {}
            }
            return;
        }
        if s.starts_with('\x1b') {
            return;
        }
        match s {
            "\r" | "\n" => self.send(host),
            "\x7f" | "\x08" => {
                self.input.pop();
            }
            "\x03" if self.running => self.call(host, "agent.cancel", json!({}), Asked::Other),
            "\x03" | "\x15" => self.input.clear(),
            _ => self.input.extend(s.chars().filter(|c| !c.is_control())),
        }
    }

    fn escape(&mut self, host: &Host) {
        if self.picker.take().is_some() {
            return;
        }
        if self.running {
            self.call(host, "agent.cancel", json!({}), Asked::Other);
        } else {
            self.focused = false;
        }
    }

    fn draw(&mut self, host: &Host) {
        let Some(ctx) = self.ctx else { return };
        let vp = ctx.viewport;
        let (cw, ch) = (vp.cell_w.max(1), vp.cell_h.max(1));
        let a = ctx.area;
        let max_h = (a.h - ch * 8).max(ch * 6);
        let h = self.dock.height(a, ch, (a.h / 2).max(ch * 12).min(max_h));
        let rect = Rect { x: a.x + PADDING, y: a.y, w: a.w - 2 * PADDING, h };
        let row_h = ch + 4;
        let (x0, w) = (rect.x + 10, rect.w - 20);
        let cols = (w / cw).max(10) as usize;
        let mut items = Vec::new();
        let model = if !self.model.is_empty() { self.model.clone() } else { self.default_model.clone().unwrap_or_else(|| "model ▾".into()) };
        let mut y = rect.y + 6;
        items.push(item(Rect { x: x0, y, w: w / 2, h: row_h }, format!("chat · {model} ▾"), PanelStyle::Title, act("model", 0)));
        let mut bx = x0 + w;
        let mut button = |t: &str, a: Action, primary: bool, items: &mut Vec<PanelItem>| {
            let bw = (t.chars().count() as i32 + 2) * cw;
            bx -= bw;
            items.push(item(Rect { x: bx, y, w: bw, h: row_h }, t, PanelStyle::Button { primary }, a));
            bx -= cw / 2;
        };
        button("new", act("clear", 0), false, &mut items);
        if self.running {
            button("cancel (esc)", act("cancel", 0), true, &mut items);
        }
        y += row_h + 4;
        let input_y = rect.y + rect.h - row_h - 8;
        let mut bottom = input_y - 4;
        if let Some((s, err)) = &self.status {
            bottom -= row_h;
            items.push(item(
                Rect { x: x0, y: bottom, w, h: row_h },
                s.clone(),
                if *err { PanelStyle::Error } else { PanelStyle::Hint },
                Action::None,
            ));
        } else if self.card_open {
            bottom -= row_h;
            items.push(item(
                Rect { x: x0, y: bottom, w, h: row_h },
                "waiting for your answer in Mouser's approval popup",
                PanelStyle::Hint,
                Action::None,
            ));
        }
        let input_text = if self.input.is_empty() && !self.focused {
            "ask about this terminal… (i to type)".to_string()
        } else if self.focused {
            format!("{}▏", self.input)
        } else {
            self.input.clone()
        };
        let shown: String = {
            let n = input_text.chars().count();
            input_text.chars().skip(n.saturating_sub(cols.saturating_sub(2))).collect()
        };
        // Focused only while no picker is open (the picker takes the keys then).
        let focused = self.focused && self.picker.is_none();
        items.push(item(Rect { x: x0, y: input_y, w, h: row_h + 4 }, shown, PanelStyle::Field { focused, valid: true }, act("focus", 0)));
        let all = rows(&self.entries, &self.open, &self.hidden, cols);
        let fit = ((bottom - y) / row_h).max(0) as usize;
        self.scroll = self.scroll.min(all.len().saturating_sub(fit));
        let end = all.len() - self.scroll;
        let start = end.saturating_sub(fit);
        if all.is_empty() {
            let hint = "Ask about what is on the screen, or ask it to do something. Tool calls that type, run or write show Mouser's approval popup first. Esc cancels; m (or click the model name) switches models.";
            for (i, l) in wrap(hint, cols).into_iter().enumerate() {
                items.push(item(Rect { x: x0, y: y + i as i32 * row_h, w, h: row_h }, l, PanelStyle::Hint, Action::None));
            }
        }
        self.links.clear();
        let links = &mut self.links;
        let mut link = |u: &str| {
            links.push(u.to_string());
            act("link", links.len() as i32 - 1)
        };
        self.hits.clear();
        for (i, (entry, r)) in all[start..end].iter().enumerate() {
            let ry = y + i as i32 * row_h;
            self.hits.push((Rect { x: x0, y: ry, w, h: row_h }, *entry));
            match r {
                Row::Plain(t, st, a) => items.push(item(Rect { x: x0, y: ry, w, h: row_h }, t.clone(), *st, a.clone())),
                Row::Md { prefix, style, line, code } => {
                    let pw = PREFIX as i32 * cw;
                    if !prefix.is_empty() {
                        items.push(item(Rect { x: x0, y: ry, w: pw, h: row_h }, prefix.clone(), *style, Action::None));
                    }
                    md::line_items(line, *code, x0 + pw, ry, (cw, row_h), w - pw, &mut link, &mut items);
                }
            }
        }
        if start > 0 {
            items.push(item(Rect { x: x0 + w - 12 * cw, y, w: 12 * cw, h: row_h }, format!("↑ {start} more"), PanelStyle::Hint, act("up", 0)));
        }
        if let Some((choices, sel)) = &self.picker {
            let pw = (w / 2).max(cw * 30);
            let ph = (choices.len().max(1) as i32) * row_h;
            let top = rect.y + 6 + row_h;
            items.push(item(Rect { x: x0, y: top, w: pw, h: ph }, "", PanelStyle::Popup, Action::None));
            if choices.is_empty() {
                items.push(item(
                    Rect { x: x0, y: top, w: pw, h: row_h },
                    "no model set up: esc :p (Models) or `mouser model add`",
                    PanelStyle::Hint,
                    Action::None,
                ));
            }
            for (j, c) in choices.iter().enumerate() {
                let r = Rect { x: x0, y: top + j as i32 * row_h, w: pw, h: row_h };
                items.push(item(r, c.clone(), PanelStyle::Option { selected: j == *sel, highlight: 0 }, act("pick", j as i32)));
            }
        }
        let _ = host.view(Some(Panel { rect, items }), None, vec![], false);
    }
}

impl Mode for Chat {
    fn hello(&self) -> Hello {
        let keys = KEYS;
        Hello {
            api: ApiRange::CURRENT,
            name: "chat".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            provides: vec![Provide::Mode {
                hotkey: 'a',
                bindings: keys.iter().map(|(id, k)| BindingDecl { id: (*id).into(), key: (*k).into(), describe: String::new() }).collect(),
                activate_on: vec![],
            }],
            listeners: ["mode.*", "ui.action", "input.text", "input.escape", "view.resized", "menu.open", "agent.state"]
                .iter()
                .map(|e| Listener::on(e))
                .collect(),
            emits: vec![],
            wants_mcp: false,
            shm: false,
        }
    }

    fn welcome(&mut self, w: &Accepted, host: &Host) {
        self.ctx = Some(w.ctx);
        self.focused = true;
        self.call(host, "agent.state", json!({}), Asked::State);
        self.call(host, "agent.models", json!({}), Asked::Other);
    }

    fn event(&mut self, e: &Event, host: &Host) -> Handled {
        match e.name.as_str() {
            "mode.activated" => self.focused = true,
            "mode.deactivated" => self.focused = false,
            "ui.action" => self.action(e.payload["id"].as_str().unwrap_or(""), e.payload["arg"].as_i64().unwrap_or(0) as i32, host),
            "input.text" => self.text(e.payload["utf8"].as_str().unwrap_or(""), host),
            "input.escape" => self.escape(host),
            "agent.state" => self.apply_state(&e.payload),
            "menu.open" => {
                let (x, y) = (e.payload["x"].as_i64().unwrap_or(-1) as i32, e.payload["y"].as_i64().unwrap_or(-1) as i32);
                self.open_menu(x, y, host);
            }
            "view.resized" => {
                self.dock = mouser_mode_api::DockSize::from_payload(&e.payload);
                if let Ok(c) = serde_json::from_value::<Context>(json!({"viewport": e.payload["viewport"], "area": e.payload["area"]})) {
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
                let choices: Vec<String> = v["providers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|p| p["configured"].as_bool().unwrap_or(false))
                    .flat_map(|p| {
                        let n = p["name"].as_str().unwrap_or("").to_string();
                        p["models"].as_array().cloned().unwrap_or_default().into_iter().filter_map(move |m| m.as_str().map(|m| format!("{n}/{m}")))
                    })
                    .collect();
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
            (Asked::Send, Err(e)) => self.status = Some((e, true)),
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
        let r = rows(&c.entries, &c.open, &c.hidden, 80);
        assert!(r.iter().all(|(i, _)| *i != 0) && r.iter().any(|(i, _)| *i == 1));
        assert_eq!(c.entries.len(), 3, "the agent's history is untouched");
    }

    /// Every binding appears in some menu.
    #[test]
    fn every_key_is_in_a_menu() {
        for (id, _) in KEYS {
            assert!(ACTIONS.iter().any(|a| a.0 == *id), "{id}");
        }
    }
}
