//! Mouser's preferences mode as a plugin program (`transport = "mode"`,
//! docs/mode-protocol.md). Every preference by section: arrows move, left
//! and right step choices, on/off and numbers, Enter types a value or opens
//! a dropdown, `r` resets. Mouser validates and writes preferences.toml
//! (`prefs.set`, comments kept); API keys are typed into Mouser's own popup
//! (`prefs.secret_prompt`), so this program never sees one.

use mouser_mode_api::events::{Event, Listener};
use mouser_mode_api::host_api::Call;
use mouser_mode_api::protocol::{Accepted, ApiRange, BindingDecl, Handled, Hello, Provide};
use mouser_mode_api::{Action, Context, Host, Mode, Panel, PanelItem, PanelStyle, Rect, serve};
use serde_json::{Value, json};
use std::collections::HashMap;

const PADDING: i32 = 8;

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Bool,
    Int { min: i64, max: i64 },
    Float { min: f64, max: f64 },
    Choice(Vec<String>),
    Text,
    Secret,
    ReadOnly(String),
}

#[derive(Debug, Clone)]
struct Field {
    section: String,
    key: String,
    label: String,
    kind: Kind,
    default: String,
    live: bool,
}

/// What a pending request was for.
enum Asked {
    Schema,
    Get(String),
    /// Set or reset of this key; the answer is its new value.
    Change(String),
    Secret(String),
}

#[derive(Default)]
struct Prefs {
    ctx: Option<Context>,
    fields: Vec<Field>,
    values: HashMap<String, Option<String>>,
    sel: usize,
    top: usize,
    rows_shown: usize,
    /// Typing a value: (text, error).
    edit: Option<(String, Option<String>)>,
    /// Dropdown open: highlighted choice.
    drop: Option<usize>,
    status: Option<(String, bool)>,
    asked: HashMap<u64, Asked>,
    /// The mode panel's size the user chose.
    dock: mouser_mode_api::DockSize,
    /// Setting rows drawn last: (rect, field index), for a right-click.
    hits: Vec<(Rect, usize)>,
}

/// The preference keys (`[preferences.keys]` in modes.toml).
const KEYS: &[(&str, &str)] = &[
    ("up", "up"),
    ("down", "down"),
    ("prev", "left"),
    ("next", "right"),
    ("edit", "enter"),
    ("reset", "r"),
    ("page_up", "pageup"),
    ("page_down", "pagedown"),
];

/// The menu's actions: (id, label, on a setting, off the settings).
const ACTIONS: &[(&str, &str, bool, bool)] = &[
    ("edit", "edit", true, false),
    ("prev", "previous value", true, false),
    ("next", "next value", true, false),
    ("reset", "reset to default", true, false),
    ("copy_value", "copy value", true, false),
    ("copy_key", "copy the setting's name", true, false),
    ("up", "previous setting", true, true),
    ("down", "next setting", true, true),
    ("page_up", "page up", false, true),
    ("page_down", "page down", false, true),
];

fn kind_of(v: &Value) -> Kind {
    match v["type"].as_str().unwrap_or("") {
        "bool" => Kind::Bool,
        "int" => Kind::Int { min: v["min"].as_i64().unwrap_or(i64::MIN), max: v["max"].as_i64().unwrap_or(i64::MAX) },
        "float" => Kind::Float { min: v["min"].as_f64().unwrap_or(f64::MIN), max: v["max"].as_f64().unwrap_or(f64::MAX) },
        "choice" => Kind::Choice(v["values"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()),
        "secret" => Kind::Secret,
        "readonly" => Kind::ReadOnly(v["hint"].as_str().unwrap_or("").into()),
        _ => Kind::Text,
    }
}

/// The value after `cur` (`dir` +1 / -1) for choices, on/off and numbers.
fn step(f: &Field, cur: Option<&str>, dir: i64) -> Option<String> {
    let cur = cur.unwrap_or(&f.default);
    match &f.kind {
        Kind::Bool => Some(if cur == "true" { "false" } else { "true" }.into()),
        Kind::Choice(v) if !v.is_empty() => {
            let i = v.iter().position(|c| c == cur).map(|i| i as i64).unwrap_or(-dir.signum().max(0));
            Some(v[(i + dir).rem_euclid(v.len() as i64) as usize].clone())
        }
        Kind::Int { min, max } => {
            let n: i64 = cur.parse().unwrap_or(*min);
            let by = if max.saturating_sub(*min) > 1000 { (max - min) / 100 } else { 1 }.max(1);
            Some((n.saturating_add(dir * by)).clamp(*min, *max).to_string())
        }
        Kind::Float { min, max } => {
            let n: f64 = cur.parse().unwrap_or(*min);
            Some((n + dir as f64).clamp(*min, *max).to_string())
        }
        _ => None,
    }
}

fn item(rect: Rect, text: impl Into<String>, style: PanelStyle, action: Action) -> PanelItem {
    PanelItem { rect, text: text.into(), style, action }
}

fn act(id: &str, arg: i32) -> Action {
    Action::Mode { action: id.into(), arg }
}

fn range_label(first: usize, shown: usize, total: usize) -> String {
    if total == 0 {
        return "0 of 0".into();
    }
    format!("{}–{} of {total}", (first + 1).min(total), (first + shown).min(total))
}

impl Prefs {
    fn call(&mut self, host: &Host, method: &str, params: Value, why: Asked) {
        if let Ok(id) = host.request(Call::Service { method: method.into(), params }) {
            self.asked.insert(id, why);
        }
    }

    fn reload(&mut self, host: &Host) {
        self.call(host, "prefs.schema", json!({}), Asked::Schema);
    }

    fn field(&self) -> Option<&Field> {
        self.fields.get(self.sel)
    }

    fn value(&self, f: &Field) -> Option<String> {
        self.values.get(&f.key).cloned().flatten()
    }

    fn set(&mut self, host: &Host, raw: String) {
        let Some(f) = self.field().cloned() else { return };
        self.call(host, "prefs.set", json!({"key": f.key, "value": raw}), Asked::Change(f.key));
    }

    fn change_step(&mut self, host: &Host, dir: i64) {
        let Some(f) = self.field().cloned() else { return };
        if let Some(v) = step(&f, self.value(&f).as_deref(), dir) {
            self.set(host, v);
        }
    }

    fn reset(&mut self, host: &Host) {
        let Some(f) = self.field().cloned() else { return };
        self.edit = None;
        self.drop = None;
        self.call(host, "prefs.reset", json!({"key": f.key}), Asked::Change(f.key));
    }

    fn open_edit(&mut self, host: &Host) {
        let Some(f) = self.field().cloned() else { return };
        match &f.kind {
            Kind::Bool => self.change_step(host, 1),
            Kind::Choice(v) => {
                let cur = self.value(&f).unwrap_or_else(|| f.default.clone());
                self.drop = Some(v.iter().position(|c| *c == cur).unwrap_or(0));
            }
            Kind::ReadOnly(hint) => self.status = Some((format!("{}: change it with `{hint}`", f.label), true)),
            // Mouser asks for the key in its own popup; it goes to the keychain.
            Kind::Secret => self.call(host, "prefs.secret_prompt", json!({"key": f.key}), Asked::Secret(f.key.clone())),
            _ => self.edit = Some((self.value(&f).unwrap_or_default(), None)),
        }
    }

    fn move_to(&mut self, i: usize) {
        self.sel = i.min(self.fields.len().saturating_sub(1));
        let shown = self.rows_shown.max(1);
        if self.sel < self.top {
            self.top = self.sel;
        } else if self.sel >= self.top + shown {
            self.top = self.sel + 1 - shown;
        }
    }

    fn pick(&mut self, host: &Host) {
        let (Some(i), Some(Kind::Choice(v))) = (self.drop.take(), self.field().map(|f| f.kind.clone())) else { return };
        if let Some(c) = v.get(i) {
            self.set(host, c.clone());
        }
    }

    /// The context menu on setting `i` (`None`: off the settings).
    fn menu(&self, i: Option<usize>) -> Vec<mouser_mode_api::MenuEntry> {
        let f = i.and_then(|i| self.fields.get(i));
        ACTIONS
            .iter()
            .filter(|(_, _, on, off)| if f.is_some() { *on } else { *off })
            .map(|(id, label, _, _)| {
                let enabled = match (*id, f) {
                    ("reset", Some(f)) => self.value(f).is_some() && !matches!(f.kind, Kind::ReadOnly(_)),
                    // Keys never leave the keychain.
                    ("copy_value", Some(f)) => !matches!(f.kind, Kind::Secret),
                    ("prev" | "next", Some(f)) => matches!(f.kind, Kind::Bool | Kind::Choice(_) | Kind::Int { .. } | Kind::Float { .. }),
                    _ => true,
                };
                mouser_mode_api::MenuEntry {
                    id: format!("menu:{id}"),
                    arg: i.map_or(-1, |i| i as i32),
                    label: (*label).into(),
                    key: KEYS.iter().find(|k| k.0 == *id).map(|k| k.1.to_string()).unwrap_or_default(),
                    enabled,
                }
            })
            .collect()
    }

    fn open_menu(&mut self, x: i32, y: i32, host: &Host) {
        let i = if x < 0 {
            (self.sel < self.fields.len()).then_some(self.sel)
        } else {
            self.hits.iter().find(|(r, _)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h).map(|(_, i)| *i)
        };
        let (x, y) = if x < 0 { self.hits.iter().find(|(_, j)| Some(*j) == i).map_or((40, 40), |(r, _)| (r.x + r.w / 3, r.y + r.h)) } else { (x, y) };
        if let Some(i) = i {
            self.move_to(i);
        }
        let _ = host.menu(x, y, self.menu(i));
    }

    fn action(&mut self, id: &str, arg: i32, host: &Host) {
        if let Some(m) = id.strip_prefix("menu:") {
            if arg >= 0 {
                self.move_to(arg as usize);
            }
            let copy = match m {
                "copy_value" => self.field().filter(|f| !matches!(f.kind, Kind::Secret)).map(|f| self.value(f).unwrap_or_else(|| f.default.clone())),
                "copy_key" => self.field().map(|f| f.key.clone()),
                _ => None,
            };
            if let Some(t) = copy {
                let _ = host.request(Call::ClipboardSet(t));
                self.status = Some(("copied".into(), false));
                return;
            }
            let m = m.to_string();
            return self.action(&m, 0, host);
        }
        let n = self.fields.len();
        match id {
            "up" => match self.drop.as_mut() {
                Some(d) => *d = d.saturating_sub(1),
                None => self.move_to(self.sel.saturating_sub(1)),
            },
            "down" => {
                if let (Some(d), Some(Kind::Choice(v))) = (self.drop, self.field().map(|f| &f.kind)) {
                    self.drop = Some((d + 1).min(v.len().saturating_sub(1)));
                } else {
                    self.move_to((self.sel + 1).min(n.saturating_sub(1)));
                }
            }
            "page_up" => self.move_to(self.sel.saturating_sub(self.rows_shown)),
            "page_down" => self.move_to(self.sel + self.rows_shown),
            "prev" => self.change_step(host, -1),
            "next" => self.change_step(host, 1),
            "edit" if self.drop.is_some() => self.pick(host),
            "edit" => self.open_edit(host),
            "reset" => self.reset(host),
            "row" if arg >= 0 => self.move_to(arg as usize),
            "value" if arg >= 0 => {
                self.move_to(arg as usize);
                self.open_edit(host);
            }
            "choice" if arg >= 0 => {
                self.drop = Some(arg as usize);
                self.pick(host);
            }
            "reset_row" if arg >= 0 => {
                self.move_to(arg as usize);
                self.reset(host);
            }
            _ => {}
        }
    }

    /// Typed text: the open field, or the dropdown's arrows and Enter.
    fn text(&mut self, s: &str, host: &Host) {
        if self.drop.is_some() {
            match s {
                "\r" | "\n" => self.pick(host),
                "\x1b[A" | "\x1bOA" => self.action("up", 0, host),
                "\x1b[B" | "\x1bOB" => self.action("down", 0, host),
                _ => {}
            }
            return;
        }
        if s.starts_with('\x1b') {
            return;
        }
        for c in s.chars() {
            let Some((buf, err)) = self.edit.as_mut() else { return };
            match c {
                '\r' | '\n' => {
                    let raw = buf.clone();
                    self.edit = None;
                    return self.set(host, raw);
                }
                '\x7f' | '\x08' => {
                    buf.pop();
                    *err = None;
                }
                c if c.is_control() => {}
                c => {
                    buf.push(c);
                    *err = None;
                }
            }
        }
    }

    fn draw(&mut self, host: &Host) {
        let Some(ctx) = self.ctx else { return };
        let vp = ctx.viewport;
        let (cw, ch) = (vp.cell_w.max(1), vp.cell_h.max(1));
        let a = ctx.area;
        let row_h = ch + 4;
        let h = self.dock.height(a, ch, (a.h / 2).max(row_h * 6).min(a.h));
        let rect = Rect { x: a.x + PADDING, y: a.y, w: a.w - 2 * PADDING, h };
        let (x0, w) = (rect.x + 10, rect.w - 30);
        let mut items = Vec::new();
        let total = self.fields.len();
        let body = ((h - 3 * row_h - 12) / row_h).max(1) as usize;
        self.rows_shown = body;
        self.move_to(self.sel);
        let range = range_label(self.top, body.min(total), total);
        items.push(item(Rect { x: x0, y: rect.y + 6, w, h: row_h }, "Preferences", PanelStyle::Title, Action::None));
        let rl = range.chars().count() as i32 * cw;
        items.push(item(Rect { x: x0 + w - rl, y: rect.y + 6, w: rl, h: row_h }, range, PanelStyle::Hint, Action::None));
        let label_w = 26 * cw;
        let sec_w = 13 * cw;
        let mut y = rect.y + 6 + row_h;
        let mut drop_at = None;
        self.hits.clear();
        for (i, f) in self.fields.iter().enumerate().skip(self.top).take(body) {
            let selected = i == self.sel;
            let first_of_section = i == 0 || self.fields[i - 1].section != f.section || i == self.top;
            let r = Rect { x: x0, y, w, h: row_h };
            self.hits.push((r, i));
            items.push(item(r, "", PanelStyle::Row { selected }, act("row", i as i32)));
            if first_of_section {
                items.push(item(Rect { x: x0 + 4, y, w: sec_w, h: row_h }, f.section.clone(), PanelStyle::Hint, act("row", i as i32)));
            }
            items.push(item(Rect { x: x0 + sec_w, y, w: label_w, h: row_h }, f.label.clone(), PanelStyle::Label, act("row", i as i32)));
            let vx = x0 + sec_w + label_w;
            let vw = (w - sec_w - label_w - 8 * cw).max(cw * 8);
            let value = self.value(f);
            match (selected, &self.edit) {
                (true, Some((buf, err))) => items.push(item(
                    Rect { x: vx, y, w: vw, h: row_h },
                    buf.clone(),
                    PanelStyle::Field { focused: true, valid: err.is_none() },
                    Action::None,
                )),
                _ => {
                    let (text, style) = match &value {
                        Some(v) => (v.clone(), PanelStyle::Label),
                        None => (format!("default ({})", f.default), PanelStyle::Hint),
                    };
                    let text = match &f.kind {
                        Kind::Choice(_) => format!("{text} ▾"),
                        Kind::Secret => format!("{text}  · enter to type a key"),
                        _ if !f.live => format!("{text}  · new windows"),
                        _ => text,
                    };
                    items.push(item(Rect { x: vx, y, w: vw, h: row_h }, text, style, act("value", i as i32)));
                }
            }
            if value.is_some() && !matches!(f.kind, Kind::ReadOnly(_)) {
                items.push(item(
                    Rect { x: vx + vw + cw, y, w: 6 * cw, h: row_h },
                    "reset",
                    PanelStyle::Button { primary: false },
                    act("reset_row", i as i32),
                ));
            }
            if selected && self.drop.is_some() {
                drop_at = Some((vx, y + row_h, vw));
            }
            y += row_h;
        }
        if total > body {
            let track = Rect { x: rect.x + rect.w - 12, y: rect.y + 6 + row_h, w: 4, h: body as i32 * row_h };
            let start = (self.top * 1000 / total) as u16;
            let len = (body * 1000 / total).max(1) as u16;
            items.push(item(track, "", PanelStyle::Scrollbar { start, len }, Action::None));
        }
        let hint = match (&self.status, self.edit.as_ref().and_then(|(_, e)| e.clone())) {
            (_, Some(e)) => (e, PanelStyle::Error),
            (Some((s, err)), _) => (s.clone(), if *err { PanelStyle::Error } else { PanelStyle::Hint }),
            _ if total == 0 => ("loading…".to_string(), PanelStyle::Hint),
            _ => ("↑/↓ move · ←/→ change · enter edit · r reset · esc :p off".to_string(), PanelStyle::Hint),
        };
        items.push(item(Rect { x: x0, y: rect.y + h - row_h - 6, w, h: row_h }, hint.0, hint.1, Action::None));
        if let (Some((dx, dy, dw)), Some(d), Some(Kind::Choice(v))) = (drop_at, self.drop, self.field().map(|f| &f.kind)) {
            let fit = (((rect.y + h - row_h) - dy) / row_h).max(1) as usize;
            let first = d.saturating_sub(fit - 1);
            let n = v.len().min(fit);
            items.push(item(Rect { x: dx, y: dy, w: dw, h: n as i32 * row_h }, "", PanelStyle::Popup, Action::None));
            for (j, c) in v.iter().enumerate().skip(first).take(fit) {
                let r = Rect { x: dx, y: dy + (j - first) as i32 * row_h, w: dw, h: row_h };
                items.push(item(r, c.clone(), PanelStyle::Option { selected: j == d, highlight: 0 }, act("choice", j as i32)));
            }
        }
        let _ = host.view(Some(Panel { rect, items }), None, vec![], false);
    }
}

impl Mode for Prefs {
    fn hello(&self) -> Hello {
        let keys = KEYS;
        Hello {
            api: ApiRange::CURRENT,
            name: "preferences".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            provides: vec![Provide::Mode {
                hotkey: 'p',
                bindings: keys.iter().map(|(id, k)| BindingDecl { id: (*id).into(), key: (*k).into(), describe: String::new() }).collect(),
                activate_on: vec![],
            }],
            listeners: ["mode.*", "ui.action", "input.text", "input.escape", "view.resized", "menu.open"].iter().map(|e| Listener::on(e)).collect(),
            emits: vec![],
            wants_mcp: false,
            shm: false,
        }
    }

    fn welcome(&mut self, w: &Accepted, host: &Host) {
        self.ctx = Some(w.ctx);
        self.reload(host);
    }

    fn event(&mut self, e: &Event, host: &Host) -> Handled {
        match e.name.as_str() {
            "mode.activated" => {
                self.reload(host);
            }
            "mode.deactivated" => {
                self.edit = None;
                self.drop = None;
                self.status = None;
            }
            "ui.action" => self.action(e.payload["id"].as_str().unwrap_or(""), e.payload["arg"].as_i64().unwrap_or(0) as i32, host),
            "input.text" => self.text(e.payload["utf8"].as_str().unwrap_or(""), host),
            "input.escape" => {
                self.edit = None;
                self.drop = None;
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
        let Some(why) = self.asked.remove(&id) else { return };
        match (why, result) {
            (Asked::Schema, Ok(v)) => {
                self.fields = v
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|f| Field {
                        section: f["section"].as_str().unwrap_or("").into(),
                        key: f["key"].as_str().unwrap_or("").into(),
                        label: f["label"].as_str().unwrap_or("").into(),
                        kind: kind_of(&f["kind"]),
                        default: f["default"].as_str().unwrap_or("").into(),
                        live: f["live"].as_bool().unwrap_or(true),
                    })
                    .collect();
                for k in self.fields.iter().map(|f| f.key.clone()).collect::<Vec<_>>() {
                    self.call(host, "prefs.get", json!({"key": k}), Asked::Get(k.clone()));
                }
            }
            (Asked::Get(k), Ok(v)) => {
                self.values.insert(k, v.as_str().map(String::from));
            }
            (Asked::Change(k), Ok(v)) => {
                let label = self.fields.iter().find(|f| f.key == k).map(|f| (f.label.clone(), f.live, f.default.clone()));
                self.values.insert(k, v.as_str().map(String::from));
                if let Some((label, live, default)) = label {
                    let shown = v.as_str().map(String::from).unwrap_or_else(|| format!("default ({default})"));
                    self.status = Some((format!("{label}: {shown}{}", if live { "" } else { " (new windows)" }), false));
                }
            }
            (Asked::Secret(k), Ok(_)) => {
                self.status = Some(("key saved to your keychain".into(), false));
                self.call(host, "prefs.get", json!({"key": k}), Asked::Get(k.clone()));
            }
            (Asked::Secret(_), Err(e)) if e == "cancelled" => self.status = Some(("no key entered (nothing changed)".into(), false)),
            (Asked::Change(_), Err(e)) => match self.edit.as_mut() {
                Some((_, err)) => *err = Some(e),
                None => self.status = Some((e, true)),
            },
            (_, Err(e)) => self.status = Some((e, true)),
        }
        self.draw(host);
    }
}

fn main() {
    if let Err(e) = serve(Prefs::default()) {
        eprintln!("mouser-preferences: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod menu_tests {
    use super::*;

    fn field(key: &str, kind: Kind) -> Field {
        Field { section: "S".into(), key: key.into(), label: key.into(), kind, default: "1".into(), live: true }
    }

    #[test]
    fn setting_menu_and_off_settings() {
        let mut p = Prefs { fields: vec![field("n", Kind::Int { min: 0, max: 9 }), field("k", Kind::Secret)], ..Prefs::default() };
        p.values.insert("n".into(), Some("3".into()));
        let m = p.menu(Some(0));
        for (id, key) in [("reset", "r"), ("copy_value", ""), ("edit", "enter"), ("prev", "left"), ("next", "right")] {
            assert!(m.iter().any(|e| e.id == format!("menu:{id}") && e.key == key && e.enabled), "setting: {id}");
        }
        let s = p.menu(Some(1));
        assert!(!s.iter().find(|e| e.id == "menu:copy_value").unwrap().enabled, "a key is never copied");
        assert!(!s.iter().find(|e| e.id == "menu:reset").unwrap().enabled, "nothing set: nothing to reset");
        let off = p.menu(None);
        assert!(off.iter().any(|e| e.id == "menu:page_down") && !off.iter().any(|e| e.id == "menu:reset"));
    }

    #[test]
    fn every_key_is_in_a_menu() {
        for (id, _) in KEYS {
            assert!(ACTIONS.iter().any(|a| a.0 == *id), "{id}");
        }
    }
}
