//! The preferences mode (`esc :p`, docs/preferences.md): every preference by
//! section, edited in place and written back to preferences.toml with its
//! comments kept. Theme, light/dark, the Esc wait, chat and model settings
//! apply at once; the rest apply to new windows (the row says so).

use super::{Context, Effect, Mode, View};
use crate::frame::PADDING;
use crate::ui::{Panel, PanelItem, PanelStyle, Rect, UiAction};
use mouser_core::modes::{Binding, Keymap};
use mouser_core::prefs_edit::{self as pe, Field, Kind};

pub struct PrefsMode {
    fields: Vec<Field>,
    doc: Option<pe::DocumentMut>,
    sel: usize,
    top: usize,
    /// Typing a value: (text, error).
    edit: Option<(String, Option<String>)>,
    /// Dropdown open: highlighted choice.
    drop: Option<usize>,
    status: Option<(String, bool)>,
    rows_shown: usize,
}

impl Default for PrefsMode {
    fn default() -> Self {
        Self::new()
    }
}

fn item(rect: Rect, text: impl Into<String>, style: PanelStyle, action: UiAction) -> PanelItem {
    PanelItem { rect, text: text.into(), style, action }
}

fn act(id: &str, arg: i32) -> UiAction {
    UiAction::Mode { action: id.into(), arg }
}

impl PrefsMode {
    pub fn new() -> Self {
        PrefsMode { fields: Vec::new(), doc: None, sel: 0, top: 0, edit: None, drop: None, status: None, rows_shown: 8 }
    }

    fn reload(&mut self) {
        self.fields = pe::schema();
        match pe::load_doc() {
            Ok(d) => self.doc = Some(d),
            Err(e) => {
                self.doc = None;
                self.status = Some((format!("{e:#}"), true));
            }
        }
        self.sel = self.sel.min(self.fields.len().saturating_sub(1));
    }

    fn field(&self) -> Option<&Field> {
        self.fields.get(self.sel)
    }

    fn value(&self, f: &Field) -> Option<String> {
        self.doc.as_ref().and_then(|d| pe::get(d, f))
    }

    /// Apply `f` to the document and save; on any error nothing is written.
    fn change(&mut self, what: impl FnOnce(&mut pe::DocumentMut, &Field) -> anyhow::Result<()>) -> Effect {
        let Some(f) = self.field().cloned() else { return Effect::None };
        let Some(doc) = self.doc.as_ref() else {
            return Effect::Say("preferences.toml doesn't parse; fix it first (nothing was changed)".into());
        };
        let mut d = doc.clone();
        match what(&mut d, &f).and_then(|_| pe::save(&d)) {
            Ok(()) => {
                self.doc = Some(d);
                let v = self.value(&f).unwrap_or_else(|| format!("default ({})", f.default));
                let note = if f.live { "" } else { " (new windows)" };
                self.status = Some((format!("{}: {v}{note}", f.label), false));
                Effect::None
            }
            Err(e) => {
                self.status = Some((format!("{e:#}"), true));
                Effect::None
            }
        }
    }

    fn set(&mut self, raw: String) -> Effect {
        self.change(move |d, f| pe::set(d, f, &raw))
    }

    fn step(&mut self, dir: i64) -> Effect {
        let Some(f) = self.field().cloned() else { return Effect::None };
        let cur = self.value(&f);
        match pe::step(&f, cur.as_deref(), dir) {
            Some(v) => self.set(v),
            None => Effect::None,
        }
    }

    fn open_edit(&mut self) -> Effect {
        let Some(f) = self.field().cloned() else { return Effect::None };
        match &f.kind {
            Kind::Bool => self.step(1),
            Kind::Choice(v) => {
                let cur = self.value(&f).unwrap_or_else(|| f.default.to_string());
                self.drop = Some(v.iter().position(|c| *c == cur).unwrap_or(0));
                Effect::None
            }
            Kind::ReadOnly(hint) => {
                self.status = Some((format!("{}: change it with `{hint}`", f.label), true));
                Effect::None
            }
            Kind::Secret(_) => {
                self.edit = Some((String::new(), None));
                Effect::None
            }
            _ => {
                self.edit = Some((self.value(&f).unwrap_or_default(), None));
                Effect::None
            }
        }
    }

    fn move_to(&mut self, i: usize) {
        self.sel = i.min(self.fields.len().saturating_sub(1));
        if self.sel < self.top {
            self.top = self.sel;
        } else if self.sel >= self.top + self.rows_shown {
            self.top = self.sel + 1 - self.rows_shown;
        }
    }

    fn pick(&mut self) -> Effect {
        let (Some(i), Some(Kind::Choice(v))) = (self.drop.take(), self.field().map(|f| f.kind.clone())) else { return Effect::None };
        match v.get(i) {
            Some(c) => self.set(c.clone()),
            None => Effect::None,
        }
    }
}

impl Mode for PrefsMode {
    fn name(&self) -> &'static str {
        "preferences"
    }
    fn hotkey(&self) -> char {
        'p'
    }
    fn bindings(&self) -> &'static [Binding] {
        pe::BINDINGS
    }
    fn activate(&mut self) {
        self.reload();
    }
    fn deactivate(&mut self) {
        self.edit = None;
        self.drop = None;
        self.status = None;
    }

    fn action(&mut self, id: &str, arg: i32) -> Effect {
        if self.fields.is_empty() {
            self.reload();
        }
        let n = self.fields.len();
        match id {
            "up" => {
                if let Some(d) = self.drop.as_mut() {
                    *d = d.saturating_sub(1);
                } else {
                    self.move_to(self.sel.saturating_sub(1));
                }
            }
            "down" => {
                if let (Some(d), Some(Kind::Choice(v))) = (self.drop, self.field().map(|f| &f.kind)) {
                    self.drop = Some((d + 1).min(v.len().saturating_sub(1)));
                } else {
                    self.move_to((self.sel + 1).min(n.saturating_sub(1)));
                }
            }
            "page_up" => self.move_to(self.sel.saturating_sub(self.rows_shown)),
            "page_down" => self.move_to(self.sel + self.rows_shown),
            "prev" => return self.step(-1),
            "next" => return self.step(1),
            "edit" => return if self.drop.is_some() { self.pick() } else { self.open_edit() },
            "reset" => {
                self.edit = None;
                self.drop = None;
                return self.change(pe::reset);
            }
            // Clicks: a row selects it; its value edits; a dropdown row picks.
            "row" if arg >= 0 => self.move_to(arg as usize),
            "value" if arg >= 0 => {
                self.move_to(arg as usize);
                return self.open_edit();
            }
            "choice" if arg >= 0 => {
                self.drop = Some(arg as usize);
                return self.pick();
            }
            "reset_row" if arg >= 0 => {
                self.move_to(arg as usize);
                return self.change(pe::reset);
            }
            _ => {}
        }
        Effect::None
    }

    fn text(&mut self, bytes: &[u8]) -> Option<Effect> {
        if self.drop.is_some() {
            return match bytes {
                b"\r" | b"\n" => Some(self.pick()),
                b"\x1b[A" | b"\x1bOA" => Some(self.action("up", 0)),
                b"\x1b[B" | b"\x1bOB" => Some(self.action("down", 0)),
                _ => Some(Effect::None),
            };
        }
        let f = self.field().cloned()?;
        if bytes.first() == Some(&0x1b) {
            return None;
        }
        let s = String::from_utf8_lossy(bytes).into_owned();
        for c in s.chars() {
            let (buf, err) = self.edit.as_mut()?;
            match c {
                '\r' | '\n' => {
                    let raw = buf.clone();
                    if let Err(e) = pe::validate(&f, &raw) {
                        *err = Some(format!("{e:#}"));
                        return Some(Effect::None);
                    }
                    self.edit = None;
                    return Some(self.set(raw));
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
        Some(Effect::None)
    }

    fn wants_text(&self) -> bool {
        self.edit.is_some() || self.drop.is_some()
    }

    fn wants_key(&self, bytes: &[u8]) -> bool {
        self.drop.is_some() && matches!(bytes, b"\x1b[A" | b"\x1bOA" | b"\x1b[B" | b"\x1bOB")
    }

    fn escape(&mut self) -> bool {
        self.edit.take().is_some() || self.drop.take().is_some()
    }

    fn view(&mut self, ctx: &Context, keys: &Keymap) -> View {
        if self.fields.is_empty() {
            self.reload();
        }
        let vp = ctx.viewport;
        let (cw, ch) = (vp.cell_w.max(1), vp.cell_h.max(1));
        let a = ctx.area;
        let row_h = ch + 4;
        let h = (a.h / 2).max(row_h * 6).min(a.h);
        let rect = Rect { x: a.x + PADDING, y: a.y, w: a.w - 2 * PADDING, h };
        let (x0, w) = (rect.x + 10, rect.w - 30);
        let mut items = Vec::new();
        let total = self.fields.len();
        // Title row, then field rows, then the status / hint row.
        let body = ((h - 3 * row_h - 12) / row_h).max(1) as usize;
        self.rows_shown = body;
        self.move_to(self.sel);
        let range = super::commander::range_label(self.top, body.min(total), total);
        items.push(item(Rect { x: x0, y: rect.y + 6, w, h: row_h }, "Preferences", PanelStyle::Title, UiAction::None));
        let rl = range.chars().count() as i32 * cw;
        items.push(item(Rect { x: x0 + w - rl, y: rect.y + 6, w: rl, h: row_h }, range, PanelStyle::Hint, UiAction::None));
        let label_w = 26 * cw;
        let sec_w = 13 * cw;
        let mut y = rect.y + 6 + row_h;
        let mut drop_at = None;
        for (i, f) in self.fields.iter().enumerate().skip(self.top).take(body) {
            let selected = i == self.sel;
            let first_of_section = i == 0 || self.fields[i - 1].section != f.section || i == self.top;
            let r = Rect { x: x0, y, w, h: row_h };
            items.push(item(r, "", PanelStyle::Row { selected }, act("row", i as i32)));
            if first_of_section {
                items.push(item(Rect { x: x0 + 4, y, w: sec_w, h: row_h }, f.section, PanelStyle::Hint, act("row", i as i32)));
            }
            items.push(item(Rect { x: x0 + sec_w, y, w: label_w, h: row_h }, f.label, PanelStyle::Label, act("row", i as i32)));
            let vx = x0 + sec_w + label_w;
            let vw = (w - sec_w - label_w - 8 * cw).max(cw * 8);
            let editing = selected && self.edit.is_some();
            if editing {
                let (buf, err) = self.edit.as_ref().unwrap();
                let shown = if matches!(f.kind, Kind::Secret(_)) { "•".repeat(buf.chars().count()) } else { buf.clone() };
                items.push(item(
                    Rect { x: vx, y, w: vw, h: row_h },
                    shown,
                    PanelStyle::Field { focused: true, valid: err.is_none() },
                    UiAction::None,
                ));
            } else {
                let (text, style) = match self.value(f) {
                    Some(v) => (v, PanelStyle::Label),
                    None => (format!("default ({})", f.default), PanelStyle::Hint),
                };
                let text = match &f.kind {
                    Kind::Choice(_) => format!("{text} ▾"),
                    _ if !f.live => format!("{text}  · new windows"),
                    _ => text,
                };
                items.push(item(Rect { x: vx, y, w: vw, h: row_h }, text, style, act("value", i as i32)));
            }
            if self.value(f).is_some() && !matches!(f.kind, Kind::ReadOnly(_)) {
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
        let track = Rect { x: rect.x + rect.w - 12, y: rect.y + 6 + row_h, w: 4, h: body as i32 * row_h };
        if total > body {
            let start = (self.top * 1000 / total) as u16;
            let len = (body * 1000 / total).max(1) as u16;
            items.push(item(track, "", PanelStyle::Scrollbar { start, len }, UiAction::None));
        }
        let hint = match (&self.status, self.edit.as_ref().and_then(|(_, e)| e.clone())) {
            (_, Some(e)) => (e, PanelStyle::Error),
            (Some((s, err)), _) => (s.clone(), if *err { PanelStyle::Error } else { PanelStyle::Hint }),
            _ => {
                let k = |id: &str| keys.label(id);
                (
                    format!(
                        "{}/{} move · {}/{} change · {} edit · {} reset · esc :p off",
                        k("up"),
                        k("down"),
                        k("prev"),
                        k("next"),
                        k("edit"),
                        k("reset")
                    ),
                    PanelStyle::Hint,
                )
            }
        };
        items.push(item(Rect { x: x0, y: rect.y + h - row_h - 6, w, h: row_h }, hint.0, hint.1, UiAction::None));
        // The dropdown last, over the rows under it.
        if let (Some((dx, dy, dw)), Some(d), Some(Kind::Choice(v))) = (drop_at, self.drop, self.field().map(|f| &f.kind)) {
            let fit = (((rect.y + h - row_h) - dy) / row_h).max(1) as usize;
            let first = d.saturating_sub(fit - 1);
            let n = v.len().min(fit);
            items.push(item(Rect { x: dx, y: dy, w: dw, h: n as i32 * row_h }, "", PanelStyle::Popup, UiAction::None));
            for (j, c) in v.iter().enumerate().skip(first).take(fit) {
                let r = Rect { x: dx, y: dy + (j - first) as i32 * row_h, w: dw, h: row_h };
                items.push(item(r, c.clone(), PanelStyle::Option { selected: j == d, highlight: 0 }, act("choice", j as i32)));
            }
        }
        View { dock: Some(Panel { rect, items }), panel: None, images: vec![], fullscreen: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UiViewport;

    fn ctx() -> Context {
        Context { viewport: UiViewport { width: 1200, height: 800, cell_w: 9, cell_h: 18 }, area: Rect { x: 0, y: 0, w: 1200, h: 800 } }
    }

    #[test]
    fn edits_write_back_and_reset() {
        let _g = crate::modes::commander::tests::ENV.lock().unwrap_or_else(|e| e.into_inner());
        let t = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("MOUSER_HOME", t.path());
            std::env::set_var("MOUSER_SECRET_STORE", "file");
        }
        let mut m = PrefsMode::new();
        m.activate();
        let i = m.fields.iter().position(|f| f.key == "mode").unwrap();
        m.action("row", i as i32);
        m.action("edit", 0);
        assert!(m.wants_text(), "dropdown open");
        m.action("down", 0);
        m.text(b"\r");
        assert_eq!(mouser_core::storage::load_preferences().mode, Some(mouser_core::storage::ThemeMode::Light));
        let k = m.fields.iter().position(|f| f.key == "keys.leader_timeout_ms").unwrap();
        m.action("row", k as i32);
        m.action("edit", 0);
        m.text(b"50");
        m.text(b"\r");
        assert!(m.edit.as_ref().unwrap().1.is_some(), "out of range stays open with an error");
        m.text(b"\x7f\x7f");
        m.text(b"700\r");
        assert_eq!(mouser_core::storage::load_preferences().keys.unwrap().leader_timeout_ms, Some(700));
        m.action("reset", 0);
        assert_eq!(mouser_core::storage::load_preferences().keys.and_then(|k| k.leader_timeout_ms), None);
        let keys = Keymap::new(pe::BINDINGS);
        let v = m.view(&ctx(), &keys);
        let d = v.dock.unwrap();
        assert!(d.items.iter().any(|it| it.text.contains("of ")), "range label");
        assert!(d.items.iter().any(|it| matches!(it.style, PanelStyle::Scrollbar { .. })), "scrollbar when not all fit");
        unsafe {
            std::env::remove_var("MOUSER_HOME");
            std::env::remove_var("MOUSER_SECRET_STORE");
        }
    }
}
