//! The agent chat mode (P5, docs/chat.md): a transcript docked above the
//! prompt, an input, approval cards for tool calls, and Cancel. The loop lives
//! in `mouser_core::agent`; this mode only draws it and forwards keys/clicks.

use super::{Context, Effect, Mode, View};
use crate::Waker;
use crate::frame::PADDING;
use crate::ui::{Panel, PanelItem, PanelStyle, Rect, UiAction};
use mouser_core::agent::{Answer, Chat, Entry, ToolState};
use mouser_core::models::setup::{self, Field, Kind, Setup, Step};
use mouser_core::modes::{Binding, Keymap};
use std::collections::BTreeSet;

const BINDINGS: &[Binding] = &[
    Binding { id: "focus", key: "i", describe: "type a message (Enter sends)" },
    Binding { id: "cancel", key: "x", describe: "cancel the running turn (also Esc)" },
    Binding { id: "allow", key: "y", describe: "approval card: allow once" },
    Binding { id: "allow_chat", key: "a", describe: "approval card: allow for this chat (not shell/files)" },
    Binding { id: "deny", key: "n", describe: "approval card: deny" },
    Binding { id: "clear", key: "l", describe: "new chat (clears the transcript)" },
    Binding { id: "up", key: "pageup", describe: "scroll the transcript up" },
    Binding { id: "down", key: "pagedown", describe: "scroll the transcript down" },
];

pub struct ChatMode {
    chat: Chat,
    input: String,
    focused: bool,
    /// Lines scrolled up from the bottom.
    scroll: usize,
    /// Tool entries with their output shown.
    open: BTreeSet<usize>,
    /// The model picker, while no usable model is set (docs/chat.md).
    setup: Option<Setup>,
    /// The message typed before a model was picked; sent once one is.
    held: Option<String>,
    waker: Option<Waker>,
    /// Tests: a picker made with these (download/preset URLs, local built in).
    pub make_setup: Option<Box<dyn Fn() -> Setup + Send>>,
}

impl ChatMode {
    pub fn new(chat: Chat) -> Self {
        ChatMode {
            chat,
            input: String::new(),
            focused: false,
            scroll: 0,
            open: BTreeSet::new(),
            setup: None,
            held: None,
            waker: None,
            make_setup: None,
        }
    }

    /// Open the picker if there is no usable model. True if it is open.
    fn need_model(&mut self) -> bool {
        if self.setup.is_none() && setup::usable_default().is_none() {
            let mut s = match &self.make_setup {
                Some(f) => f(),
                None => Setup::new(mouser_core::models::local::backend().is_some()),
            };
            if let Some(w) = &self.waker {
                s.set_notify(w.clone());
            }
            self.setup = Some(s);
        }
        self.setup.is_some()
    }

    pub fn setup(&self) -> Option<&Setup> {
        self.setup.as_ref()
    }

    /// The picker finished: note it, and send the held message.
    fn setup_done(&mut self, spec: &str, warn: Option<&str>) -> Effect {
        self.setup = None;
        {
            let mut s = self.chat.state.lock().unwrap();
            s.entries.push(Entry::Note(format!("using {spec} (the default model now; change it with mouser model default)")));
            if let Some(w) = warn {
                s.entries.push(Entry::Note(w.to_string()));
            }
        }
        match self.held.take() {
            Some(t) => {
                self.input = t;
                self.send()
            }
            None => Effect::None,
        }
    }

    fn send(&mut self) -> Effect {
        let t = std::mem::take(&mut self.input);
        if t.trim().is_empty() {
            return Effect::None;
        }
        if self.need_model() {
            self.held = Some(t);
            return Effect::None;
        }
        if self.chat.send(&t) {
            self.scroll = 0;
            Effect::None
        } else {
            self.input = t;
            Effect::Say("[chat] a turn is running; cancel it first (esc)".into())
        }
    }
}

fn act(id: &str, arg: i32) -> UiAction {
    UiAction::Mode { action: id.into(), arg }
}

fn item(rect: Rect, text: impl Into<String>, style: PanelStyle, action: UiAction) -> PanelItem {
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

/// One transcript row: text, style, click action.
type Row = (String, PanelStyle, UiAction);

fn rows(entries: &[Entry], open: &BTreeSet<usize>, cols: usize) -> Vec<Row> {
    let mut v: Vec<Row> = Vec::new();
    let mut add = |text: &str, prefix: &str, style: PanelStyle, a: UiAction| {
        let pad: String = " ".repeat(prefix.chars().count());
        for (i, l) in wrap(text, cols.saturating_sub(prefix.chars().count())).into_iter().enumerate() {
            v.push((format!("{}{l}", if i == 0 { prefix } else { &pad }), style, a.clone()));
        }
    };
    for (i, e) in entries.iter().enumerate() {
        match e {
            Entry::User(t) => add(t, "you  ", PanelStyle::Title, UiAction::None),
            Entry::Assistant(t) => add(if t.is_empty() { "…" } else { t }, "ai   ", PanelStyle::Label, UiAction::None),
            Entry::Tool { display, args, state, output } => {
                let st = match state {
                    ToolState::Waiting => "waiting for you",
                    ToolState::Running => "running…",
                    ToolState::Done => "done",
                    ToolState::Failed => "failed",
                    ToolState::Denied => "denied",
                    ToolState::Cancelled => "cancelled",
                };
                let shown = open.contains(&i);
                let mark = if output.is_empty() {
                    "•"
                } else if shown {
                    "▾"
                } else {
                    "▸"
                };
                let style = if matches!(state, ToolState::Failed | ToolState::Denied) { PanelStyle::Error } else { PanelStyle::Hint };
                let a: String = args.chars().take(160).collect();
                add(&format!("{mark} {display} {a} — {st}"), "tool ", style, act("toggle", i as i32));
                if shown && !output.is_empty() {
                    add(output, "     │ ", PanelStyle::Hint, act("toggle", i as i32));
                }
            }
            Entry::Note(t) => add(t, "note ", PanelStyle::Hint, UiAction::None),
            Entry::Error(t) => add(t, "err  ", PanelStyle::Error, UiAction::None),
        }
    }
    v
}

impl ChatMode {
    /// Move a finished picker on (Done: continue into the chat).
    fn poll_setup(&mut self) -> Effect {
        let Some(s) = self.setup.as_mut() else { return Effect::None };
        s.poll();
        if let Step::Done(spec, warn) = s.step.clone() {
            return self.setup_done(&spec, warn.as_deref());
        }
        Effect::None
    }
}

/// The picker in the chat's dock slot.
pub fn setup_view(s: &Setup, ctx: &Context) -> View {
    let vp = ctx.viewport;
    let (cw, ch) = (vp.cell_w.max(1), vp.cell_h.max(1));
    let a = ctx.area;
    let max_h = (a.h - ch * 8).max(ch * 6);
    let h = (a.h / 2).max(ch * 16).min(max_h);
    let rect = Rect { x: a.x + PADDING, y: a.y, w: a.w - 2 * PADDING, h };
    let row_h = ch + 4;
    let (x0, w) = (rect.x + 10, rect.w - 20);
    let cols = (w / cw).max(10) as usize;
    let mut items = Vec::new();
    let mut y = rect.y + 6;
    let line = |items: &mut Vec<PanelItem>, y: &mut i32, t: String, st: PanelStyle, a: UiAction| {
        for l in wrap(&t, cols) {
            items.push(item(Rect { x: x0, y: *y, w, h: row_h }, l, st, a.clone()));
            *y += row_h;
        }
    };
    let button = |items: &mut Vec<PanelItem>, x: &mut i32, y: i32, t: &str, id: &str, primary: bool| {
        let bw = (t.chars().count() as i32 + 2) * cw;
        items.push(item(Rect { x: *x, y, w: bw, h: row_h }, t, PanelStyle::Button { primary }, act(id, 0)));
        *x += bw + cw;
    };
    line(&mut items, &mut y, "chat · pick a model".into(), PanelStyle::Title, UiAction::None);
    match &s.step {
        Step::Choose => {
            line(
                &mut items,
                &mut y,
                "No model is set up yet. Pick one (↑/↓ and Enter, or click); Esc closes.".into(),
                PanelStyle::Hint,
                UiAction::None,
            );
            let label_w = s.opts.iter().map(|o| o.label.chars().count()).max().unwrap_or(10) + 2;
            for (i, o) in s.opts.iter().enumerate() {
                let t = format!("{:<label_w$}{}", o.label, o.detail);
                let t: String = t.chars().take(cols).collect();
                let st = if o.available { PanelStyle::Row { selected: i == s.sel } } else { PanelStyle::Hint };
                items.push(item(Rect { x: x0, y, w, h: row_h }, t, st, act("setup_pick", i as i32)));
                y += row_h;
            }
        }
        Step::Consent(wt) => {
            for (i, l) in wt.consent_text().into_iter().enumerate() {
                line(&mut items, &mut y, l, if i == 0 { PanelStyle::Label } else { PanelStyle::Hint }, UiAction::None);
            }
            line(
                &mut items,
                &mut y,
                "Change the folder with MOUSER_MODELS_DIR or [models.local] dir in preferences.toml.".into(),
                PanelStyle::Hint,
                UiAction::None,
            );
            y += 4;
            let mut x = x0;
            button(&mut items, &mut x, y, "download (enter)", "setup_ok", true);
            button(&mut items, &mut x, y, "back (esc)", "setup_back", false);
        }
        Step::Downloading(wt) => {
            let (d, n) = s.progress();
            let n = if n == 0 { wt.size } else { n };
            line(&mut items, &mut y, format!("Downloading {} to {}", wt.label, wt.path().display()), PanelStyle::Label, UiAction::None);
            let frac = if n > 0 { d as f64 / n as f64 } else { 0.0 };
            let bar = cols.saturating_sub(20).max(10);
            let full = ((frac * bar as f64) as usize).min(bar);
            let t = format!(
                "[{}{}] {:>3.0}%  {} / {}",
                "█".repeat(full),
                "░".repeat(bar - full),
                frac * 100.0,
                mouser_core::models::local::human(d),
                mouser_core::models::local::human(n)
            );
            line(&mut items, &mut y, t, PanelStyle::Label, UiAction::None);
            line(
                &mut items,
                &mut y,
                "The file is checked (SHA256) when it finishes. Esc cancels; downloading again resumes.".into(),
                PanelStyle::Hint,
                UiAction::None,
            );
            let mut x = x0;
            button(&mut items, &mut x, y + 4, "cancel (esc)", "setup_back", false);
        }
        Step::Form(kind, f) | Step::Testing(kind, f) => {
            let testing = matches!(s.step, Step::Testing(..));
            let name = s.opts.iter().find(|o| &o.kind == kind).map_or("model", |o| o.label.as_str());
            line(
                &mut items,
                &mut y,
                format!(
                    "{name}: {}",
                    if testing { "testing the connection…" } else { "Tab moves between fields, Enter saves and tests, Esc goes back." }
                ),
                PanelStyle::Label,
                UiAction::None,
            );
            let fs = setup::fields(kind);
            for (i, fld) in fs.iter().enumerate() {
                let focused = !testing && f.field == i;
                let (label, value) = match fld {
                    Field::Url => ("server URL", f.url.clone()),
                    Field::Api => ("API (space switches)", if f.api_anthropic { "anthropic".to_string() } else { "openai".to_string() }),
                    Field::Key => {
                        (if *kind == Kind::Custom { "API key (optional)" } else { "API key (kept in the OS keychain)" }, setup::masked(&f.key))
                    }
                    Field::Model => (if *kind == Kind::Custom { "model name" } else { "model (empty: the default)" }, f.model.clone()),
                };
                items.push(item(Rect { x: x0, y, w, h: row_h }, label, PanelStyle::Hint, UiAction::None));
                y += row_h;
                let shown = if focused { format!("{value}▏") } else { value };
                let n = shown.chars().count();
                let shown: String = shown.chars().skip(n.saturating_sub(cols.saturating_sub(2))).collect();
                items.push(item(Rect { x: x0, y, w, h: row_h + 4 }, shown, PanelStyle::Field { focused, valid: true }, act("setup_field", i as i32)));
                y += row_h + 8;
            }
            let mut x = x0;
            if testing {
                button(&mut items, &mut x, y, "cancel (esc)", "setup_back", false);
            } else {
                button(&mut items, &mut x, y, "save and test (enter)", "setup_ok", true);
                button(&mut items, &mut x, y, "back (esc)", "setup_back", false);
            }
        }
        Step::Failed(m, _) => {
            line(&mut items, &mut y, m.clone(), PanelStyle::Error, UiAction::None);
            let mut x = x0;
            button(&mut items, &mut x, y + 4, "ok (enter)", "setup_ok", true);
        }
        Step::Done(spec, _) => line(&mut items, &mut y, format!("Ready: {spec}"), PanelStyle::Label, UiAction::None),
    }
    View { dock: Some(Panel { rect, items }), panel: None, images: vec![] }
}

impl Mode for ChatMode {
    fn name(&self) -> &'static str {
        "chat"
    }
    fn hotkey(&self) -> char {
        'a'
    }
    fn bindings(&self) -> &'static [Binding] {
        BINDINGS
    }
    fn activate(&mut self) {
        self.focused = true;
        // esc :a with no usable model: pick one now.
        self.need_model();
    }
    fn restore(&mut self) {
        // At launch: no picker until the chat is used.
        self.focused = true;
    }
    fn deactivate(&mut self) {
        // The conversation (and a running turn) stays; only the input lets go.
        self.focused = false;
    }
    fn set_waker(&mut self, w: Waker) {
        self.waker = Some(w.clone());
        self.chat.set_notify(w);
    }

    fn action(&mut self, id: &str, arg: i32) -> Effect {
        if let Some(s) = self.setup.as_mut() {
            match id {
                "setup_pick" => s.pick(arg.max(0) as usize),
                "setup_ok" => s.enter(),
                "setup_back" => {
                    if !s.back() {
                        self.setup = None;
                    }
                }
                "setup_field" => s.focus(arg.max(0) as usize),
                "up" => s.up(),
                "down" => s.down(),
                "scroll" if arg > 0 => s.up(),
                "scroll" => s.down(),
                _ => return Effect::Pass,
            }
            return self.poll_setup();
        }
        match id {
            "focus" => {
                self.focused = true;
                self.need_model();
            }
            "send" => return self.send(),
            "cancel" => {
                if !self.chat.cancel() {
                    return Effect::Say("[chat] nothing running".into());
                }
            }
            "allow" => {
                self.chat.answer(Answer::Once);
            }
            "allow_chat" => {
                self.chat.answer(Answer::ForChat);
            }
            "deny" => {
                self.chat.answer(Answer::Deny);
            }
            "clear" => {
                self.chat.clear();
                self.open.clear();
                self.scroll = 0;
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
            _ => return Effect::Pass,
        }
        Effect::None
    }

    fn text(&mut self, bytes: &[u8]) -> Option<Effect> {
        if let Some(s) = self.setup.as_mut() {
            match bytes {
                b"\r" | b"\n" => s.enter(),
                b"\x1b[A" | b"\x1bOA" => s.up(),
                b"\x1b[B" | b"\x1bOB" => s.down(),
                // Other alt keys and escapes stay with the mode/window.
                _ if bytes.len() > 1 && bytes[0] == 0x1b && bytes != b"\x1b[Z" => return None,
                _ => {
                    if !s.key(bytes) {
                        return None;
                    }
                }
            }
            return Some(self.poll_setup());
        }
        if !self.focused || (bytes.len() > 1 && bytes[0] == 0x1b) {
            return None;
        }
        match bytes {
            b"\r" | b"\n" => return Some(self.send()),
            b"\x7f" | b"\x08" => {
                self.input.pop();
            }
            // Ctrl+C cancels a turn (or clears the input), Ctrl+U clears the input.
            b"\x03" => {
                if !self.chat.cancel() {
                    self.input.clear();
                }
            }
            b"\x15" => self.input.clear(),
            _ => match std::str::from_utf8(bytes) {
                Ok(s) if !s.chars().any(char::is_control) => self.input.push_str(s),
                _ => return None,
            },
        }
        Some(Effect::None)
    }

    fn wants_text(&self) -> bool {
        self.focused || self.setup.is_some()
    }

    fn wants_key(&self, bytes: &[u8]) -> bool {
        self.setup.is_some() && matches!(bytes, b"\x1b[A" | b"\x1bOA" | b"\x1b[B" | b"\x1bOB" | b"\x1b[Z")
    }

    fn escape(&mut self) -> bool {
        if let Some(s) = self.setup.as_mut() {
            if !s.back() {
                self.setup = None;
                self.held = None;
            }
            return true;
        }
        if self.chat.cancel() {
            true
        } else if self.focused {
            self.focused = false;
            true
        } else {
            false
        }
    }

    fn view(&mut self, ctx: &Context, keys: &Keymap) -> View {
        let _ = self.poll_setup();
        if let Some(s) = &self.setup {
            return setup_view(s, ctx);
        }
        let vp = ctx.viewport;
        let (cw, ch) = (vp.cell_w.max(1), vp.cell_h.max(1));
        let a = ctx.area;
        let max_h = (a.h - ch * 8).max(ch * 6);
        let h = (a.h / 2).max(ch * 12).min(max_h);
        let rect = Rect { x: a.x + PADDING, y: a.y, w: a.w - 2 * PADDING, h };
        let row_h = ch + 4;
        let (x0, w) = (rect.x + 10, rect.w - 20);
        let cols = (w / cw).max(10) as usize;
        let mut items = Vec::new();
        let (entries, pending, running, model) = {
            let s = self.chat.state.lock().unwrap();
            (s.entries.clone(), s.pending.clone(), s.running, s.model.clone())
        };
        let model = if model.is_empty() {
            mouser_core::models::default_choice().map(|(p, m)| format!("{p}/{m}")).unwrap_or_else(|| "no model (mouser model default …)".into())
        } else {
            model
        };
        // Header.
        let mut y = rect.y + 6;
        items.push(item(Rect { x: x0, y, w: w / 2, h: row_h }, format!("chat · {model}"), PanelStyle::Title, UiAction::None));
        let mut bx = x0 + w;
        let mut button = |t: String, a: UiAction, primary: bool, items: &mut Vec<PanelItem>| {
            let bw = (t.chars().count() as i32 + 2) * cw;
            bx -= bw;
            items.push(item(Rect { x: bx, y, w: bw, h: row_h }, t, PanelStyle::Button { primary }, a));
            bx -= cw / 2;
        };
        button(format!("new ({})", keys.label("clear")), act("clear", 0), false, &mut items);
        if running {
            button(format!("cancel (esc, {})", keys.label("cancel")), act("cancel", 0), true, &mut items);
        }
        y += row_h + 4;
        // Bottom: the input, and above it the approval card.
        let input_y = rect.y + rect.h - row_h - 8;
        let mut bottom = input_y - 4;
        if let Some(p) = &pending {
            let mut lines = vec![format!("Approve {}?", p.display)];
            for l in &p.lines {
                lines.extend(wrap(l, cols.saturating_sub(4)).into_iter().map(|x| format!("  {x}")));
            }
            lines.truncate(8);
            let card_h = (lines.len() as i32 + 1) * row_h + 12;
            let top = bottom - card_h;
            items.push(item(Rect { x: x0 - 4, y: top, w: w + 8, h: card_h }, "", PanelStyle::Popup, UiAction::None));
            for (i, l) in lines.iter().enumerate() {
                let st = if i == 0 { PanelStyle::Title } else { PanelStyle::Label };
                items.push(item(Rect { x: x0 + 4, y: top + 4 + i as i32 * row_h, w: w - 8, h: row_h }, l.clone(), st, UiAction::None));
            }
            let by = top + 4 + lines.len() as i32 * row_h + 2;
            let mut bx = x0 + 4;
            let mut opts = vec![(format!("allow once ({})", keys.label("allow")), "allow", true)];
            if p.for_chat {
                opts.push((format!("allow for this chat ({})", keys.label("allow_chat")), "allow_chat", false));
            }
            opts.push((format!("deny ({})", keys.label("deny")), "deny", false));
            for (t, id, primary) in opts {
                let bw = (t.chars().count() as i32 + 2) * cw;
                items.push(item(Rect { x: bx, y: by, w: bw, h: row_h }, t, PanelStyle::Button { primary }, act(id, 0)));
                bx += bw + cw;
            }
            bottom = top - 4;
        }
        let input_text = if self.input.is_empty() && !self.focused {
            format!("ask about this terminal… ({} to type)", keys.label("focus"))
        } else if self.focused {
            format!("{}▏", self.input)
        } else {
            self.input.clone()
        };
        let shown: String = {
            let n = input_text.chars().count();
            input_text.chars().skip(n.saturating_sub(cols.saturating_sub(2))).collect()
        };
        items.push(item(
            Rect { x: x0, y: input_y, w, h: row_h + 4 },
            shown,
            PanelStyle::Field { focused: self.focused, valid: true },
            act("focus", 0),
        ));
        // Transcript: the newest rows that fit above, minus the scroll.
        let all = rows(&entries, &self.open, cols);
        let fit = ((bottom - y) / row_h).max(0) as usize;
        let max_scroll = all.len().saturating_sub(fit);
        self.scroll = self.scroll.min(max_scroll);
        let end = all.len() - self.scroll;
        let start = end.saturating_sub(fit);
        if all.is_empty() {
            let hint =
                "Ask about what is on the screen, or ask it to do something. Tool calls that type, run or write show a card first. Esc cancels.";
            for (i, l) in wrap(hint, cols).into_iter().enumerate() {
                items.push(item(Rect { x: x0, y: y + i as i32 * row_h, w, h: row_h }, l, PanelStyle::Hint, UiAction::None));
            }
        }
        for (i, (t, st, a)) in all[start..end].iter().enumerate() {
            items.push(item(Rect { x: x0, y: y + i as i32 * row_h, w, h: row_h }, t.clone(), *st, a.clone()));
        }
        if start > 0 {
            items.push(item(Rect { x: x0 + w - 12 * cw, y, w: 12 * cw, h: row_h }, format!("↑ {start} more"), PanelStyle::Hint, act("up", 0)));
        }
        View { dock: Some(Panel { rect, items }), panel: None, images: vec![] }
    }
}
