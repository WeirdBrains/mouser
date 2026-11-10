//! The Hugging Face browser inside the chat (`h`, `/hf`, or "Search Hugging
//! Face…" in the model picker): a search field, the matching GGUF models,
//! and a model's details with its quantizations and an Install button.
//! Searching and details go through Mouser (`models.hf_search`,
//! `models.hf_details`); Install asks Mouser (`models.hf_install`), which
//! shows the consent popup with the size and license, downloads it with
//! progress (`models.download` events) and makes it the current model.

use mouser_mode_api::{Action, PanelItem, PanelStyle, Rect};
use serde_json::Value;

/// What the browser asks the chat to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Search(String),
    Details(String),
    Install {
        id: String,
        quant: String,
    },
    /// An installed quantization: make it the current model.
    Use(String),
    Cancel,
    Close,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hit {
    pub id: String,
    pub downloads: u64,
    pub likes: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Quant {
    pub label: String,
    pub human: String,
    pub files: u64,
    pub spec: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Details {
    pub id: String,
    pub facts: Vec<String>,
    pub summary: String,
    pub gated: bool,
    /// Why it won't run in the chat (an unsupported architecture or format).
    pub unsupported: Option<String>,
    /// The Hub doesn't say its architecture.
    pub unverified: bool,
    /// A supported architecture (not load-tested).
    pub arch: String,
    pub quants: Vec<Quant>,
    pub sel: usize,
}

#[derive(Default)]
pub struct Hub {
    pub query: String,
    /// The query the shown hits are for.
    pub searched: Option<String>,
    pub hits: Vec<Hit>,
    pub sel: usize,
    pub busy: Option<&'static str>,
    pub error: Option<String>,
    pub details: Option<Details>,
    pub scroll: usize,
    pub complete: mouser_mode_api::complete::Engine,
    pub history: Vec<String>,
    pub hint: Option<String>,
}

fn act(id: &str, arg: i32) -> Action {
    Action::Mode { action: id.into(), arg }
}

fn item(rect: Rect, text: impl Into<String>, style: PanelStyle, action: Action) -> PanelItem {
    PanelItem { rect, text: text.into(), style, action }
}

/// `12345678` as `12.3M`.
pub fn count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

impl Hub {
    pub fn new(query: &str) -> Self {
        Hub { query: query.into(), ..Hub::default() }
    }

    /// The `models.hf_search` answer.
    pub fn hits_from(&mut self, v: &Value) {
        self.busy = None;
        self.error = None;
        self.hits = v
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|h| {
                Some(Hit {
                    id: h["id"].as_str()?.to_string(),
                    downloads: h["downloads"].as_u64().unwrap_or(0),
                    likes: h["likes"].as_u64().unwrap_or(0),
                })
            })
            .collect();
        self.sel = 0;
        self.scroll = 0;
    }

    /// The `models.hf_details` answer.
    pub fn details_from(&mut self, v: &Value) {
        self.busy = None;
        self.error = None;
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        let mut facts = Vec::new();
        facts.push(format!("↓{} downloads · ♥{} likes", count(v["downloads"].as_u64().unwrap_or(0)), count(v["likes"].as_u64().unwrap_or(0))));
        let mut more = Vec::new();
        for (k, x) in [("license", s("license")), ("task", s("pipeline")), ("architecture", s("architecture")), ("updated", s("updated"))] {
            if !x.is_empty() {
                more.push(format!("{k}: {x}"));
            }
        }
        if let Some(p) = v["params"].as_u64().filter(|p| *p > 0) {
            more.push(format!("parameters: {:.1}B", p as f64 / 1e9));
        }
        if let Some(c) = v["context_length"].as_u64().filter(|c| *c > 0) {
            more.push(format!("context: {c}"));
        }
        if !more.is_empty() {
            facts.push(more.join(" · "));
        }
        let quants: Vec<Quant> = v["quants"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|q| Quant {
                label: q["label"].as_str().unwrap_or("").into(),
                human: q["human"].as_str().unwrap_or("").into(),
                files: q["files"].as_u64().unwrap_or(1),
                spec: q["spec"].as_str().map(String::from),
            })
            .collect();
        // Keep the choice when the details are refreshed (after an install).
        let prev = self.details.as_ref().filter(|d| d.id == s("id")).and_then(|d| d.quants.get(d.sel)).map(|q| q.label.clone());
        let want = prev.or_else(|| v["default_quant"].as_str().map(String::from));
        let sel = want.and_then(|w| quants.iter().position(|q| q.label == w)).unwrap_or(0);
        let sup = &v["support"];
        let unsupported = (sup["runs"] == false).then(|| sup["why"].as_str().unwrap_or("it can't run on Mouser's llama.cpp").to_string());
        self.details = Some(Details {
            id: s("id"),
            facts,
            summary: s("summary"),
            gated: v["gated"].as_bool().unwrap_or(false),
            unsupported,
            unverified: sup["unverified"] == true,
            arch: sup["architecture"].as_str().unwrap_or("").to_string(),
            quants,
            sel,
        });
        self.scroll = 0;
    }

    pub fn failed(&mut self, e: String) {
        self.busy = None;
        self.error = Some(e);
    }

    fn search(&mut self) -> Option<Cmd> {
        let q = self.query.trim().to_string();
        if q.is_empty() {
            self.hint = Some("type part of a model name, e.g. qwen 0.5b or gemma".into());
            return None;
        }
        self.history.retain(|h| *h != q);
        self.history.push(q.clone());
        self.details = None;
        self.busy = Some("searching Hugging Face…");
        self.searched = Some(q.clone());
        Some(Cmd::Search(q))
    }

    fn open(&mut self, i: usize) -> Option<Cmd> {
        let id = self.hits.get(i)?.id.clone();
        self.sel = i;
        self.busy = Some("loading the model card…");
        Some(Cmd::Details(id))
    }

    fn install(&mut self) -> Option<Cmd> {
        let d = self.details.as_ref()?;
        if d.gated {
            self.error = Some("this model is gated; Mouser can't download gated models yet".into());
            return None;
        }
        if let Some(why) = &d.unsupported {
            self.error = Some(format!("won't run: {why}"));
            return None;
        }
        let q = d.quants.get(d.sel)?;
        Some(match &q.spec {
            Some(spec) => Cmd::Use(spec.clone()),
            None => Cmd::Install { id: d.id.clone(), quant: q.label.clone() },
        })
    }

    /// A key typed while the browser is open.
    pub fn text(&mut self, s: &str) -> Option<Cmd> {
        if s == "\t" || s == "\x1b[Z" {
            self.tab(s == "\x1b[Z");
            return None;
        }
        let up = matches!(s, "\x1b[A" | "\x1bOA");
        let down = matches!(s, "\x1b[B" | "\x1bOB");
        if let Some(d) = self.details.as_mut() {
            match s {
                _ if up => d.sel = d.sel.saturating_sub(1),
                _ if down => d.sel = (d.sel + 1).min(d.quants.len().saturating_sub(1)),
                "\r" | "\n" => return self.install(),
                "\x7f" | "\x08" => self.details = None,
                _ if s.starts_with('\x1b') => {}
                _ => {
                    // Typing goes back to the search.
                    self.details = None;
                    self.query.extend(s.chars().filter(|c| !c.is_control()));
                }
            }
            return None;
        }
        match s {
            _ if up => self.sel = self.sel.saturating_sub(1),
            _ if down => self.sel = (self.sel + 1).min(self.hits.len().saturating_sub(1)),
            "\r" | "\n" => {
                let q = self.query.trim().to_string();
                return if !self.hits.is_empty() && self.searched.as_deref() == Some(q.as_str()) { self.open(self.sel) } else { self.search() };
            }
            "\x7f" | "\x08" => {
                self.query.pop();
                self.complete.reset();
            }
            "\x15" => {
                self.query.clear();
                self.complete.reset();
            }
            _ if s.starts_with('\x1b') => {}
            _ => {
                self.query.extend(s.chars().filter(|c| !c.is_control()));
                self.complete.reset();
            }
        }
        None
    }

    /// Tab: complete the query from earlier searches and the shown model ids.
    fn tab(&mut self, reverse: bool) {
        let mut values = self.history.clone();
        values.extend(self.hits.iter().map(|h| h.id.clone()));
        let input = self.query.clone();
        let (stem, token, matches) = mouser_mode_api::complete::value_matches(&input, &values);
        let out = self.complete.tab(&input, &stem, &token, &matches, reverse);
        self.query = out.text.clone();
        self.hint = if out.message.is_some() || !out.candidates.is_empty() { Some(out.list_hint(72)) } else { None };
        if self.details.is_some() {
            self.details = None;
        }
    }

    /// Escape: back from the details, else close.
    pub fn escape(&mut self) -> Option<Cmd> {
        if self.details.take().is_some() {
            return None;
        }
        Some(Cmd::Close)
    }

    /// A click or menu action (`hub_*`).
    pub fn action(&mut self, id: &str, arg: i32) -> Option<Cmd> {
        match id {
            "hub_search" => self.search(),
            "hub_pick" => self.open(arg.max(0) as usize),
            "hub_quant" => {
                if let Some(d) = self.details.as_mut() {
                    d.sel = (arg.max(0) as usize).min(d.quants.len().saturating_sub(1));
                }
                None
            }
            "hub_install" => self.install(),
            "hub_back" => {
                self.details = None;
                None
            }
            "hub_cancel" => Some(Cmd::Cancel),
            "hub_close" => Some(Cmd::Close),
            "up" => {
                self.scroll = self.scroll.saturating_sub(3);
                None
            }
            "down" => {
                self.scroll += 3;
                None
            }
            "scroll" => {
                self.scroll = (self.scroll as i64 - arg as i64).max(0) as usize;
                None
            }
            _ => None,
        }
    }

    /// Draw into `inner` (the chat's frame). `download`: Mouser's
    /// `models.download` state, if one was started.
    pub fn draw(&mut self, inner: Rect, (cw, row_h): (i32, i32), focused: bool, download: Option<&Value>, out: &mut Vec<PanelItem>) {
        let (x, w) = (inner.x, inner.w);
        let cols = (w / cw).max(10) as usize;
        let mut y = inner.y;
        // The search field and its buttons.
        let bw = 10 * cw;
        let shown = if self.query.is_empty() && !focused {
            "search Hugging Face GGUF models".to_string()
        } else if focused {
            format!("⌕ {}▏", self.query)
        } else {
            format!("⌕ {}", self.query)
        };
        let fw = w - 2 * (bw + cw / 2);
        out.push(item(Rect { x, y, w: fw, h: row_h + 4 }, shown, PanelStyle::Field { focused, valid: true }, act("hub_search", 0)));
        out.push(item(
            Rect { x: x + fw + cw / 2, y, w: bw, h: row_h + 4 },
            "search ⏎",
            PanelStyle::Button { primary: self.details.is_none() },
            act("hub_search", 0),
        ));
        out.push(item(Rect { x: x + w - bw, y, w: bw, h: row_h + 4 }, "close esc", PanelStyle::Button { primary: false }, act("hub_close", 0)));
        y += row_h + 10;
        let bottom = inner.y + inner.h;
        // Download progress (any model), above the rest.
        if let Some(d) = download {
            let state = d["state"].as_str().unwrap_or("");
            let (done, total) = (d["done"].as_u64().unwrap_or(0), d["total"].as_u64().unwrap_or(0).max(1));
            let name = format!("{} {}", d["repo"].as_str().unwrap_or(""), d["quant"].as_str().unwrap_or(""));
            let line = match state {
                "downloading" => format!("downloading {name}: {}%", done * 100 / total),
                "done" => format!("installed {name}: now the current model ({})", d["spec"].as_str().unwrap_or("")),
                "cancelled" => format!("cancelled {name} (Install again resumes it)"),
                "failed" => format!("failed {name}: {}", d["error"].as_str().unwrap_or("")),
                s => format!("{s} {name}"),
            };
            let style = if state == "failed" { PanelStyle::Error } else { PanelStyle::Hint };
            out.push(item(Rect { x, y, w, h: row_h }, line, style, Action::None));
            y += row_h;
            if state == "downloading" {
                let cbw = 10 * cw;
                let per = (done * 1000 / total).min(1000) as u16;
                out.push(item(
                    Rect { x, y: y + 2, w: w - cbw - cw, h: row_h - 4 },
                    "",
                    PanelStyle::Progress { done: per, hover: u16::MAX },
                    Action::None,
                ));
                out.push(item(Rect { x: x + w - cbw, y, w: cbw, h: row_h }, "cancel", PanelStyle::Button { primary: false }, act("hub_cancel", 0)));
                y += row_h;
            }
            y += 4;
        }
        let status = self.busy.map(String::from).or_else(|| self.error.clone()).or_else(|| self.hint.clone());
        if let Some(s) = status {
            let style = if self.error.is_some() && self.busy.is_none() { PanelStyle::Error } else { PanelStyle::Hint };
            for l in crate::wrap(&s, cols).into_iter().take(3) {
                out.push(item(Rect { x, y, w, h: row_h }, l, style, Action::None));
                y += row_h;
            }
        }
        let fit = ((bottom - y) / row_h).max(0) as usize;
        if let Some(d) = &self.details {
            // A model card: name, facts, summary, quantizations, Install.
            let mut rows: Vec<PanelItem> = Vec::new();
            let r = |rows: &Vec<PanelItem>| Rect { x, y: rows.len() as i32, w, h: row_h };
            rows.push(item(r(&rows), d.id.clone(), PanelStyle::Title, Action::None));
            for f in &d.facts {
                for l in crate::wrap(f, cols) {
                    rows.push(item(r(&rows), l, PanelStyle::Label, Action::None));
                }
            }
            if d.gated {
                rows.push(item(
                    r(&rows),
                    "gated: accept its terms on huggingface.co; gated downloads aren't supported yet",
                    PanelStyle::Error,
                    Action::None,
                ));
            }
            if let Some(why) = &d.unsupported {
                for l in crate::wrap(&format!("won't run: {why}"), cols) {
                    rows.push(item(r(&rows), l, PanelStyle::Error, Action::None));
                }
            } else if !d.arch.is_empty() {
                let t = format!("supported architecture ({}), not load-tested: the chat says so on the first message if it doesn't load", d.arch);
                for l in crate::wrap(&t, cols) {
                    rows.push(item(r(&rows), l, PanelStyle::Hint, Action::None));
                }
            } else if d.unverified {
                let t = "the Hub doesn't list its architecture: it may not load (the chat says so on the first message)";
                for l in crate::wrap(t, cols) {
                    rows.push(item(r(&rows), l, PanelStyle::Hint, Action::None));
                }
            }
            if !d.summary.is_empty() {
                rows.push(item(r(&rows), "", PanelStyle::Hint, Action::None));
                for l in crate::wrap(&d.summary, cols) {
                    rows.push(item(r(&rows), l, PanelStyle::Hint, Action::None));
                }
            }
            rows.push(item(r(&rows), "", PanelStyle::Hint, Action::None));
            rows.push(item(r(&rows), format!("{} GGUF files (↑↓ choose, Enter installs):", d.quants.len()), PanelStyle::Label, Action::None));
            for (i, q) in d.quants.iter().enumerate() {
                let parts = if q.files > 1 { format!(" · {} parts", q.files) } else { String::new() };
                let mark = if q.spec.is_some() { "  ✓ installed" } else { "" };
                let t = format!("{:<12} {:>9}{parts}{mark}", q.label, q.human);
                rows.push(item(r(&rows), t, PanelStyle::Option { selected: i == d.sel, highlight: 0 }, act("hub_quant", i as i32)));
            }
            // Keep the chosen quantization in view.
            let first_q = rows.len() - d.quants.len();
            let need = (first_q + d.sel + 1).saturating_sub(fit.saturating_sub(1));
            self.scroll = self.scroll.max(need).min(rows.len().saturating_sub(fit.saturating_sub(1)));
            let avail = fit.saturating_sub(1);
            for (k, mut it) in rows.into_iter().skip(self.scroll).take(avail).enumerate() {
                it.rect.y = y + k as i32 * row_h;
                out.push(it);
            }
            // Install / Use / Back on the last row.
            let by = bottom - row_h - 2;
            let q = d.quants.get(d.sel);
            let label = match q {
                Some(q) if q.spec.is_some() => format!("Use {}", q.label),
                Some(q) => format!("Install {} ({})", q.label, q.human),
                None => "no GGUF files".into(),
            };
            let downloading = download.is_some_and(|v| v["state"] == "downloading");
            let iw = (label.chars().count() as i32 + 4) * cw;
            let ok = q.is_some() && !d.gated && d.unsupported.is_none() && !downloading;
            out.push(item(
                Rect { x, y: by, w: iw, h: row_h + 2 },
                label,
                PanelStyle::Button { primary: ok },
                if ok { act("hub_install", 0) } else { Action::None },
            ));
            out.push(item(
                Rect { x: x + iw + cw, y: by, w: 10 * cw, h: row_h + 2 },
                "← back",
                PanelStyle::Button { primary: false },
                act("hub_back", 0),
            ));
            return;
        }
        if self.hits.is_empty() {
            let t = if self.searched.is_some() && self.busy.is_none() && self.error.is_none() {
                "No GGUF models match. Try a shorter name."
            } else {
                "Search Hugging Face for GGUF models that run on Mouser's built-in llama.cpp. Enter searches; Tab completes earlier searches."
            };
            for (i, l) in crate::wrap(t, cols).into_iter().enumerate() {
                out.push(item(Rect { x, y: y + i as i32 * row_h, w, h: row_h }, l, PanelStyle::Hint, Action::None));
            }
            return;
        }
        let fit = fit.max(1);
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + fit {
            self.scroll = self.sel + 1 - fit;
        }
        self.scroll = self.scroll.min(self.hits.len().saturating_sub(fit));
        for (k, (i, h)) in self.hits.iter().enumerate().skip(self.scroll).take(fit).enumerate() {
            let stats = format!("↓{}  ♥{}", count(h.downloads), count(h.likes));
            let room = cols.saturating_sub(stats.chars().count() + 2);
            let name: String = h.id.chars().take(room).collect();
            let gap = cols.saturating_sub(name.chars().count() + stats.chars().count());
            let t = format!("{name}{}{stats}", " ".repeat(gap));
            out.push(item(
                Rect { x, y: y + k as i32 * row_h, w, h: row_h },
                t,
                PanelStyle::Option { selected: i == self.sel, highlight: 0 },
                act("hub_pick", i as i32),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn typed(h: &mut Hub, s: &str) -> Vec<Cmd> {
        s.chars().filter_map(|c| h.text(&c.to_string())).collect()
    }

    #[test]
    fn search_pick_details_install_and_back() {
        let mut h = Hub::new("");
        assert_eq!(h.text("\r"), None, "an empty search asks for a name");
        assert!(h.hint.is_some());
        assert!(typed(&mut h, "qwen").is_empty());
        assert_eq!(h.text("\r"), Some(Cmd::Search("qwen".into())));
        h.hits_from(&json!([{"id": "a/qwen-GGUF", "downloads": 1500, "likes": 3}, {"id": "b/qwen2-GGUF"}]));
        assert_eq!(h.hits.len(), 2);
        h.text("\x1b[B");
        assert_eq!(h.text("\r"), Some(Cmd::Details("b/qwen2-GGUF".into())), "Enter on the same query opens the hit");
        h.details_from(&json!({"id": "b/qwen2-GGUF", "downloads": 10, "likes": 1, "license": "apache-2.0", "summary": "Small.",
            "quants": [{"label": "Q4_K_M", "human": "400 MB", "files": 1}, {"label": "F16", "human": "1.2 GB", "files": 2, "spec": "gemma-local/qwen2-F16"}],
            "default_quant": "Q4_K_M"}));
        let d = h.details.as_ref().unwrap();
        assert!(d.facts.iter().any(|f| f.contains("license: apache-2.0")));
        assert_eq!(h.text("\r"), Some(Cmd::Install { id: "b/qwen2-GGUF".into(), quant: "Q4_K_M".into() }));
        h.text("\x1b[B");
        assert_eq!(h.action("hub_install", 0), Some(Cmd::Use("gemma-local/qwen2-F16".into())), "installed: use it");
        assert_eq!(h.escape(), None, "Esc goes back to the list");
        assert!(h.details.is_none() && h.hits.len() == 2);
        assert_eq!(h.escape(), Some(Cmd::Close));
    }

    #[test]
    fn tab_completes_from_history_and_hits() {
        let mut h = Hub::new("");
        typed(&mut h, "gemma");
        h.text("\r");
        h.hits_from(&json!([{"id": "unsloth/gemma-3-1b-it-GGUF"}]));
        h.query.clear();
        typed(&mut h, "uns");
        h.text("\t");
        assert_eq!(h.query, "unsloth/gemma-3-1b-it-GGUF");
    }

    #[test]
    fn gated_models_cannot_be_installed() {
        let mut h = Hub::new("x");
        h.details_from(&json!({"id": "g/m", "gated": true, "quants": [{"label": "Q4_0", "human": "1 GB"}]}));
        assert_eq!(h.text("\r"), None);
        assert!(h.error.as_deref().unwrap().contains("gated"));
    }

    #[test]
    fn unsupported_models_say_why_and_cannot_be_installed() {
        let mut h = Hub::new("x");
        h.details_from(&json!({"id": "e/embed", "quants": [{"label": "F16", "human": "1 GB"}],
            "support": {"runs": false, "why": "e/embed is an embedding model (bert), not a chat model"}}));
        assert_eq!(h.text("\r"), None);
        assert!(h.error.as_deref().unwrap().contains("embedding model"));
        let mut out = Vec::new();
        h.draw(Rect { x: 0, y: 0, w: 800, h: 600 }, (8, 20), true, None, &mut out);
        assert!(out.iter().any(|i| i.style == PanelStyle::Error && i.text.contains("won't run")));
        assert_eq!(out.iter().find(|i| i.text.starts_with("Install")).unwrap().action, Action::None);
    }

    #[test]
    fn draws_progress_and_install_button() {
        let mut h = Hub::new("x");
        h.details_from(&json!({"id": "a/b", "quants": [{"label": "Q4_K_M", "human": "400 MB"}]}));
        let mut out = Vec::new();
        let dl = json!({"repo": "a/b", "quant": "Q4_K_M", "done": 50, "total": 100, "state": "downloading"});
        h.draw(Rect { x: 0, y: 0, w: 800, h: 600 }, (8, 20), true, Some(&dl), &mut out);
        let t: Vec<&str> = out.iter().map(|i| i.text.as_str()).collect();
        assert!(t.iter().any(|t| t.contains("downloading a/b Q4_K_M: 50%")), "{t:?}");
        assert!(out.iter().any(|i| matches!(i.style, PanelStyle::Progress { done: 500, .. })));
        assert!(t.contains(&"cancel"));
        h.details_from(
            &json!({"id": "a/b", "quants": [{"label": "Q4_K_M", "human": "400 MB"}], "support": {"runs": true, "architecture": "qwen35moe"}}),
        );
        let mut out2 = Vec::new();
        h.draw(Rect { x: 0, y: 0, w: 800, h: 600 }, (8, 20), true, None, &mut out2);
        assert!(out2.iter().any(|i| i.text.contains("supported architecture (qwen35moe), not load-tested")), "never claims it runs");
        let b = out.iter().find(|i| i.text.starts_with("Install Q4_K_M")).unwrap();
        assert_eq!(b.action, Action::None, "no second download while one runs");
        assert!(out.iter().any(|i| matches!(i.style, PanelStyle::Field { focused: true, .. })), "typing still reaches the field");
    }
}
