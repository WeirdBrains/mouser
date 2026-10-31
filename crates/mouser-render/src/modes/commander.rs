//! The commander mode plugin: the model is `mouser_core::commander`; this
//! builds its view (list, grid, carousel, file view) and loads previews.
//!
//! Previews load lazily on a worker thread: only what is on screen is
//! requested (grid: the visible tiles; carousel: the focused item and its
//! two neighbours), and anything that scrolls away is dropped from the queue.

use super::{Context, Effect, Mode, ModeImage, View, fit};
use crate::Waker;
use crate::frame::PADDING;
use crate::ui::{Panel, PanelItem, PanelStyle, Rect, UiAction};
use mouser_core::commander::{self as cmdr, Action, Commander, Entry, Kind, Overlay, Sort, TILE_SIZES, View as Layout};
use mouser_core::modes::{Binding, Keymap};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

#[derive(Clone)]
struct Thumb {
    key: u64,
    rgba: Arc<Vec<u8>>,
    w: u32,
    h: u32,
}

type Want = (PathBuf, u32);
/// Options button: label, on, action id, arg.
type Btn<'a> = (String, bool, &'a str, i32);

#[derive(Default)]
struct Shared {
    want: Vec<Want>,
    /// `None`: could not be decoded.
    done: HashMap<Want, Option<Thumb>>,
    waker: Option<Waker>,
    quit: bool,
    /// Decodes done (tests).
    decoded: Vec<Want>,
}

/// The preview loader.
#[derive(Default)]
pub struct Previews {
    s: Arc<(Mutex<Shared>, Condvar)>,
    started: bool,
}

/// Texture keys apart from the terminal's image ids.
static NEXT_KEY: AtomicU64 = AtomicU64::new(1 << 62);

impl Previews {
    /// Load exactly these (path, longest side) now; everything else queued
    /// before is dropped.
    pub fn want(&mut self, list: Vec<Want>) {
        let (m, cv) = &*self.s;
        let mut s = m.lock().unwrap();
        if s.want == list {
            return;
        }
        // Keep a few recent ones so scrolling back is instant.
        if s.done.len() > 48 {
            s.done.retain(|k, _| list.contains(k));
        }
        s.want = list;
        drop(s);
        cv.notify_all();
        if !self.started {
            self.started = true;
            let s = self.s.clone();
            let _ = std::thread::Builder::new().name("commander-previews".into()).spawn(move || worker(&s));
        }
    }

    fn get(&self, w: &Want) -> Option<Option<Thumb>> {
        self.s.0.lock().unwrap().done.get(w).cloned()
    }

    pub fn set_waker(&self, w: Waker) {
        self.s.0.lock().unwrap().waker = Some(w);
    }

    /// Everything decoded so far, in order (tests).
    pub fn decoded(&self) -> Vec<Want> {
        self.s.0.lock().unwrap().decoded.clone()
    }
}

impl Drop for Previews {
    fn drop(&mut self) {
        self.s.0.lock().unwrap().quit = true;
        self.s.1.notify_all();
    }
}

fn worker(s: &(Mutex<Shared>, Condvar)) {
    let (m, cv) = s;
    loop {
        let job = {
            let mut g = m.lock().unwrap();
            loop {
                if g.quit {
                    return;
                }
                if let Some(w) = g.want.iter().find(|w| !g.done.contains_key(*w)).cloned() {
                    break w;
                }
                g = cv.wait(g).unwrap();
            }
        };
        let thumb = decode(&job.0, job.1);
        let mut g = m.lock().unwrap();
        g.decoded.push(job.clone());
        g.done.insert(job, thumb);
        if let Some(w) = g.waker.clone() {
            drop(g);
            w();
        }
    }
}

fn decode(p: &Path, max: u32) -> Option<Thumb> {
    let img = image::ImageReader::open(p).ok()?.with_guessed_format().ok()?.decode().ok()?;
    let img = if img.width() > max || img.height() > max { img.thumbnail(max, max) } else { img };
    let rgba = img.to_rgba8();
    Some(Thumb { key: NEXT_KEY.fetch_add(1, Ordering::Relaxed), w: rgba.width(), h: rgba.height(), rgba: Arc::new(rgba.into_raw()) })
}

pub struct CommanderMode {
    pub c: Commander,
    per_row: usize,
    previews: Previews,
    /// First lines of text files on screen.
    texts: HashMap<PathBuf, Vec<String>>,
    /// The file view's text.
    full_text: Option<(PathBuf, Vec<String>, Option<String>)>,
}

impl Default for CommanderMode {
    fn default() -> Self {
        Self::new()
    }
}

/// `cd` typed into the shell: Ctrl+U clears the line first.
pub fn cd_line(p: &Path) -> String {
    format!("\x15cd -- '{}'\r", p.display().to_string().replace('\'', "'\\''"))
}

fn item(rect: Rect, text: impl Into<String>, style: PanelStyle, action: UiAction) -> PanelItem {
    PanelItem { rect, text: text.into(), style, action }
}

fn act(id: &str, arg: i32) -> UiAction {
    UiAction::Mode { action: id.into(), arg }
}

fn clip_text(s: &str, cols: i32) -> String {
    let cols = cols.max(1) as usize;
    if s.chars().count() <= cols {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(cols.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// Builder state for one frame.
struct Out {
    items: Vec<PanelItem>,
    /// (what, full rect, visible part)
    slots: Vec<(Want, Rect, Rect)>,
    texts: Vec<PathBuf>,
}

impl CommanderMode {
    pub fn new() -> Self {
        Self { c: Commander::new(), per_row: 1, previews: Previews::default(), texts: HashMap::new(), full_text: None }
    }

    pub fn previews(&self) -> &Previews {
        &self.previews
    }

    fn size(&self, e: &Entry) -> String {
        if self.c.flags.human { cmdr::human_size(e.size) } else { e.size.to_string() }
    }

    fn text_lines(&mut self, p: &Path) -> &[String] {
        self.texts.entry(p.to_path_buf()).or_insert_with(|| {
            let mut buf = vec![0; 4096];
            let n = std::fs::File::open(p).and_then(|mut f| std::io::Read::read(&mut f, &mut buf)).unwrap_or(0);
            buf.truncate(n);
            if buf.contains(&0) {
                return vec!["(binary)".into()];
            }
            String::from_utf8_lossy(&buf).lines().take(40).map(|l| l.replace('\t', "  ")).collect()
        })
    }

    /// The preview of `e` inside `full` (visible part `clip`).
    fn content(&mut self, e: &Entry, (full, clip): (Rect, Rect), max: u32, (cw, ch): (i32, i32), o: &mut Out) {
        match e.kind {
            Kind::Image => {
                o.slots.push(((e.path.clone(), max), full, clip));
            }
            Kind::Text => {
                o.texts.push(e.path.clone());
                let cols = clip.w / cw.max(1);
                let rows = (clip.h / ch.max(1)) as usize;
                let lines: Vec<String> = self.text_lines(&e.path).iter().take(rows).map(|l| clip_text(l, cols)).collect();
                for (i, l) in lines.into_iter().enumerate() {
                    o.items.push(item(Rect { x: clip.x, y: clip.y + i as i32 * ch, w: clip.w, h: ch }, l, PanelStyle::Hint, UiAction::None));
                }
            }
            k => {
                let label = match k {
                    Kind::Dir => "folder",
                    Kind::Video => "video",
                    Kind::Audio => "audio",
                    _ => "file",
                };
                let w = label.chars().count() as i32 * cw;
                let r = Rect { x: clip.x + (clip.w - w).max(0) / 2, y: clip.y + (clip.h - ch) / 2, w: w.min(clip.w), h: ch };
                o.items.push(item(r, label, if k == Kind::Dir { PanelStyle::Label } else { PanelStyle::Hint }, UiAction::None));
            }
        }
    }

    fn list(&mut self, body: Rect, cw: i32, ch: i32, o: &mut Out) {
        let row_h = ch + 4;
        let rows = ((body.h / row_h) - 1).max(1) as usize;
        self.c.follow(rows);
        let cols = body.w / cw.max(1);
        let name_w = (cols - 10 - 2 - 6 - 2 - 2 - 13 - 2).max(8) as usize;
        let head = format!("{:<10}  {:>6}  {:<name_w$}  {}", "Attributes", "Size", "Name", "Last Modified");
        o.items.push(item(Rect { x: body.x, y: body.y, w: body.w, h: row_h }, head, PanelStyle::Hint, UiAction::None));
        let vis: Vec<Entry> = self.c.visible().into_iter().cloned().collect();
        for (n, e) in vis.iter().enumerate().skip(self.c.scroll).take(rows) {
            let y = body.y + row_h * (1 + (n - self.c.scroll) as i32);
            let name = if e.kind == Kind::Dir && e.name != "." && e.name != ".." { format!("{}/", e.name) } else { e.name.clone() };
            let t = e.modified.map(cmdr::format_time).unwrap_or_default();
            let text = format!("{:<10}  {:>6}  {:<name_w$}  {t}", e.mode, self.size(e), clip_text(&name, name_w as i32));
            o.items.push(item(
                Rect { x: body.x, y, w: body.w, h: row_h },
                text,
                PanelStyle::Row { selected: n == self.c.selected },
                act("item", n as i32),
            ));
        }
    }

    fn grid(&mut self, body: Rect, cw: i32, ch: i32, o: &mut Out) {
        let ts = TILE_SIZES[self.c.tile.min(TILE_SIZES.len() - 1)];
        let (gap, label_h) = (10, ch + 4);
        let per_row = ((body.w + gap) / (ts + gap)).max(1) as usize;
        let cell_h = ts + label_h + gap;
        let vis_rows = ((body.h + gap) / cell_h).max(1) as usize;
        self.per_row = per_row;
        let row = self.c.selected / per_row;
        let mut first = self.c.scroll.min(row);
        if row >= first + vis_rows {
            first = row + 1 - vis_rows;
        }
        self.c.scroll = first;
        let vis: Vec<Entry> = self.c.visible().into_iter().cloned().collect();
        let start = first * per_row;
        for (n, e) in vis.iter().enumerate().skip(start).take(per_row * vis_rows) {
            let (r, col) = ((n - start) / per_row, (n - start) % per_row);
            let tile = Rect { x: body.x + col as i32 * (ts + gap), y: body.y + r as i32 * cell_h, w: ts, h: ts };
            o.items.push(item(tile, "", PanelStyle::Tile { selected: n == self.c.selected }, act("item", n as i32)));
            let inner = Rect { x: tile.x + 4, y: tile.y + 4, w: ts - 8, h: ts - 8 };
            self.content(e, (inner, inner), (ts as u32 * 2).max(256), (cw, ch), o);
            let label = clip_text(&e.name, ts / cw.max(1));
            o.items.push(item(Rect { x: tile.x, y: tile.y + ts + 2, w: ts, h: label_h }, label, PanelStyle::Label, act("item", n as i32)));
        }
    }

    fn carousel(&mut self, body: Rect, cw: i32, ch: i32, o: &mut Out) {
        let vis: Vec<Entry> = self.c.visible().into_iter().cloned().collect();
        let Some(e) = vis.get(self.c.selected).cloned() else { return };
        let label_h = ch + 6;
        let ih = (body.h - label_h - 6).max(ch);
        let w = (body.w * 55 / 100).max(cw * 10);
        let gap = 14;
        let center = Rect { x: body.x + (body.w - w) / 2, y: body.y, w, h: ih };
        // The neighbours are the same size, cut off by the edges (partial).
        let sides =
            [(self.c.selected.checked_sub(1), center.x - gap - w), (Some(self.c.selected + 1).filter(|i| *i < vis.len()), center.x + w + gap)];
        for (i, x) in sides {
            let Some(i) = i else { continue };
            let full = Rect { x, y: center.y, w, h: ih };
            let (l, r) = (full.x.max(body.x), (full.x + w).min(body.x + body.w));
            if r - l < 8 {
                continue;
            }
            let clip = Rect { x: l, y: full.y, w: r - l, h: ih };
            o.items.push(item(clip, "", PanelStyle::Tile { selected: false }, act("item", i as i32)));
            let inner = |r: Rect| Rect { x: r.x + 6, y: r.y + 6, w: r.w - 12, h: r.h - 12 };
            self.content(&vis[i], (inner(full), inner(clip)), 1024, (cw, ch), o);
        }
        o.items.push(item(center, "", PanelStyle::Tile { selected: true }, act("item", self.c.selected as i32)));
        let inner = Rect { x: center.x + 6, y: center.y + 6, w: w - 12, h: ih - 12 };
        self.content(&e, (inner, inner), 1024, (cw, ch), o);
        let t = e.modified.map(cmdr::format_time).unwrap_or_default();
        let label = format!("{}   {}   {}   ({}/{})", e.name, t, self.size(&e), self.c.selected + 1, vis.len());
        let lw = (label.chars().count() as i32 * cw).min(body.w);
        o.items.push(item(
            Rect { x: body.x + (body.w - lw) / 2, y: body.y + ih + 6, w: lw, h: label_h },
            clip_text(&label, body.w / cw.max(1)),
            PanelStyle::Label,
            UiAction::None,
        ));
    }

    /// The full-width view of a file.
    fn file_view(&mut self, p: &Path, body: Rect, cw: i32, ch: i32, o: &mut Out) {
        if cmdr::kind_of(&p.to_string_lossy()) == Kind::Image {
            o.slots.push(((p.to_path_buf(), 2048), body, body));
            return;
        }
        if self.full_text.as_ref().is_none_or(|t| t.0 != p) {
            let (l, e) = cmdr::text_preview(p);
            self.full_text = Some((p.to_path_buf(), l, e));
        }
        let (_, lines, err) = self.full_text.as_ref().unwrap();
        if let Some(e) = err {
            o.items.push(item(Rect { x: body.x, y: body.y, w: body.w, h: ch + 4 }, e.clone(), PanelStyle::Error, UiAction::None));
            return;
        }
        let rows = (body.h / ch.max(1)).max(1) as usize;
        let scroll = self.c.overlay_scroll.min(lines.len().saturating_sub(rows));
        self.c.overlay_scroll = scroll;
        for (i, l) in lines.iter().skip(scroll).take(rows).enumerate() {
            o.items.push(item(
                Rect { x: body.x, y: body.y + i as i32 * ch, w: body.w, h: ch },
                clip_text(&l.replace('\t', "    "), body.w / cw.max(1)),
                PanelStyle::Label,
                UiAction::None,
            ));
        }
    }

    fn options_panel(&self, ctx: &Context) -> Panel {
        let vp = ctx.viewport;
        let (cw, ch) = (vp.cell_w, vp.cell_h);
        let row_h = ch + 10;
        let w = (cw * 56).min(vp.width - 16);
        let h = (row_h * 8 + 24).min(vp.height - 16);
        let rect = Rect { x: (vp.width - w) / 2, y: (vp.height - h) / 2, w, h };
        let (x0, mut y) = (rect.x + 14, rect.y + 12);
        let mut items = vec![
            item(
                Rect { x: x0, y, w: w - 28 - 8 * cw, h: row_h },
                format!("Commander options   ls {}", self.c.flags.ls()),
                PanelStyle::Title,
                UiAction::None,
            ),
            item(Rect { x: rect.x + w - 14 - 7 * cw, y, w: 7 * cw, h: row_h - 4 }, "close", PanelStyle::Button { primary: false }, act("close", 0)),
        ];
        y += row_h + 6;
        let f = &self.c.flags;
        let rows: [(&str, Vec<Btn>); 5] = [
            (
                "View",
                ["list", "grid", "carousel"].iter().enumerate().map(|(i, n)| (n.to_string(), self.c.view as usize == i, "view", i as i32)).collect(),
            ),
            (
                "Tiles",
                vec![
                    ("smaller".into(), false, "smaller", 0),
                    (format!("{}px", TILE_SIZES[self.c.tile]), true, "", 0),
                    ("bigger".into(), false, "bigger", 0),
                ],
            ),
            ("Show", vec![("dotfiles (-a)".into(), f.all, "hidden", 0), ("human sizes (-h)".into(), f.human, "human", 0)]),
            (
                "Sort",
                [("name", Sort::Name), ("modified (-t)", Sort::Modified), ("size (-S)", Sort::Size)]
                    .iter()
                    .enumerate()
                    .map(|(i, (n, s))| (n.to_string(), f.sort == *s, "sort", i as i32))
                    .collect(),
            ),
            ("Order", vec![("reverse (-r)".into(), f.reverse, "reverse", 0)]),
        ];
        for (label, btns) in rows {
            items.push(item(Rect { x: x0, y, w: 8 * cw, h: row_h - 4 }, label, PanelStyle::Label, UiAction::None));
            let mut x = x0 + 9 * cw;
            for (t, on, id, arg) in btns {
                let bw = (t.chars().count() as i32 + 2) * cw;
                let a = if id.is_empty() { UiAction::None } else { act(id, arg) };
                items.push(item(Rect { x, y, w: bw, h: row_h - 4 }, t, PanelStyle::Button { primary: on }, a));
                x += bw + cw;
            }
            y += row_h;
        }
        Panel { rect, items }
    }

    fn man_panel(&mut self, ctx: &Context, keys: &Keymap) -> Panel {
        let vp = ctx.viewport;
        let lines = cmdr::manual(keys);
        let rows = ((vp.height - 80) / (vp.cell_h + 6)).max(1) as usize;
        self.c.overlay_scroll = self.c.overlay_scroll.min(lines.len().saturating_sub(rows));
        let shown: Vec<(String, String)> = lines.into_iter().skip(self.c.overlay_scroll).take(rows).map(|l| (l, String::new())).collect();
        super::list_panel(vp, "commander(1)", &shown, |_| UiAction::None)
    }
}

impl Mode for CommanderMode {
    fn name(&self) -> &'static str {
        "commander"
    }

    fn hotkey(&self) -> char {
        'c'
    }

    fn bindings(&self) -> &'static [Binding] {
        cmdr::BINDINGS
    }

    fn deactivate(&mut self) {
        self.c.reset();
        self.previews.want(Vec::new());
    }

    fn cwd_changed(&mut self, dir: &Path) {
        self.c.set_cwd(Some(dir.to_path_buf()));
    }

    fn set_waker(&mut self, w: Waker) {
        self.previews.set_waker(w);
    }

    fn text(&mut self, bytes: &[u8]) -> Option<Effect> {
        // Bound alt keys still work while searching.
        if bytes.len() > 1 && bytes[0] == 0x1b {
            return None;
        }
        self.c.search_key(bytes).then_some(Effect::None)
    }

    fn wants_text(&self) -> bool {
        self.c.searching
    }

    fn escape(&mut self) -> bool {
        if self.c.searching {
            self.c.search_key(b"\x1b");
            true
        } else {
            self.c.overlay.take().is_some()
        }
    }

    fn action(&mut self, id: &str, arg: i32) -> Effect {
        let n = self.c.visible().len();
        let previewing = matches!(self.c.overlay, Some(Overlay::Preview(_)));
        let reading = previewing || self.c.overlay == Some(Overlay::Man);
        match id {
            "item" => {
                let i = (arg.max(0) as usize).min(n.saturating_sub(1));
                if i == self.c.selected {
                    let e = self.c.open_selected();
                    return self.effect(e);
                }
                self.c.selected = i;
                return Effect::None;
            }
            "view" => {
                self.c.view = [Layout::List, Layout::Grid, Layout::Carousel][arg.clamp(0, 2) as usize];
                self.c.scroll = 0;
                return Effect::None;
            }
            "human" => self.c.flags.human = !self.c.flags.human,
            "reverse" => self.c.flags.reverse = !self.c.flags.reverse,
            "sort" => self.c.flags.sort = [Sort::Name, Sort::Modified, Sort::Size][arg.clamp(0, 2) as usize],
            "close" => self.c.overlay = None,
            "keys" => return Effect::ShowKeys,
            "scroll" => {
                if reading {
                    self.c.overlay_scroll = (self.c.overlay_scroll as i64 + arg as i64 * 3).max(0) as usize;
                } else {
                    let step = if self.c.view == Layout::Grid { self.per_row as i64 } else { 1 };
                    self.c.selected = (self.c.selected as i64 + arg as i64 * step).clamp(0, n.saturating_sub(1) as i64) as usize;
                }
                return Effect::None;
            }
            _ => {
                let Some(a) = Action::from_id(id) else { return Effect::None };
                if reading && matches!(a, Action::Up | Action::Down) {
                    let d = if a == Action::Up { -1 } else { 1 };
                    self.c.overlay_scroll = (self.c.overlay_scroll as i64 + d).max(0) as usize;
                    return Effect::None;
                }
                let e = self.c.apply(a, self.per_row);
                if previewing && matches!(a, Action::Left | Action::Right) {
                    // Step through files in the file view.
                    if let Some(en) = self.c.selected_entry().cloned()
                        && !matches!(en.kind, Kind::Dir | Kind::Video | Kind::Audio)
                    {
                        self.c.overlay = Some(Overlay::Preview(en.path));
                        self.c.overlay_scroll = 0;
                    }
                }
                // `hidden` changes the listing.
                let cwd = self.c.cwd.clone();
                self.c.set_cwd(cwd);
                return self.effect(e);
            }
        }
        // Flags changed: re-list.
        let cwd = self.c.cwd.clone();
        self.c.set_cwd(cwd);
        Effect::None
    }

    fn view(&mut self, ctx: &Context, keys: &Keymap) -> View {
        let vp = ctx.viewport;
        let (cw, ch) = (vp.cell_w.max(1), vp.cell_h.max(1));
        let a = ctx.area;
        let max_h = (a.h - ch * 8).max(ch * 4);
        let h = (a.h / 2).max(ch * 10).min(max_h);
        let rect = Rect { x: a.x + PADDING, y: a.y, w: a.w - 2 * PADDING, h };
        let mut o = Out { items: Vec::new(), slots: Vec::new(), texts: Vec::new() };
        let row_h = ch + 8;
        let (x0, y0, w) = (rect.x + 10, rect.y + 6, rect.w - 20);
        // Header: where we are, then the buttons.
        let mut bx = x0 + w;
        let file = match &self.c.overlay {
            Some(Overlay::Preview(p)) => Some(p.clone()),
            _ => None,
        };
        let buttons: Vec<(String, UiAction)> = if file.is_some() {
            vec![("close (esc)".into(), act("close", 0))]
        } else {
            vec![
                ("help".into(), act("keys", 0)),
                ("man".into(), act("man", 0)),
                ("options".into(), act("options", 0)),
                (format!("{} ({})", self.c.view.name(), keys.label("switch_view")), act("switch_view", 0)),
            ]
        };
        for (t, a) in buttons {
            let bw = (t.chars().count() as i32 + 2) * cw;
            bx -= bw;
            o.items.push(item(Rect { x: bx, y: y0, w: bw, h: row_h - 2 }, t, PanelStyle::Button { primary: false }, a));
            bx -= cw / 2;
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let show = |p: &Path| match home.as_ref().and_then(|h| p.strip_prefix(h).ok()) {
            Some(r) if r.as_os_str().is_empty() => "~".to_string(),
            Some(r) => format!("~/{}", r.display()),
            None => p.display().to_string(),
        };
        let title = match (&file, &self.c.cwd) {
            (Some(p), _) => format!("commander · {}", show(p)),
            (None, Some(d)) => format!("commander · {}   {} items · ls {}", show(d), self.c.visible().len(), self.c.flags.ls()),
            (None, None) => "commander".into(),
        };
        o.items.push(item(
            Rect { x: x0, y: y0, w: (bx - x0 - cw).max(0), h: row_h },
            clip_text(&title, (bx - x0) / cw - 1),
            PanelStyle::Title,
            UiAction::None,
        ));
        let mut body = Rect { x: x0, y: y0 + row_h + 6, w, h: rect.y + h - (y0 + row_h + 6) - 8 };
        if let Some(p) = file {
            self.file_view(&p, body, cw, ch, &mut o);
        } else {
            // Search field.
            let fw = (cw * 40).min(w);
            let fr = Rect { x: x0, y: body.y, w: fw, h: row_h };
            o.items.push(item(
                fr,
                self.c.filter.clone(),
                PanelStyle::Field { focused: self.c.searching, valid: !self.c.visible().is_empty() || self.c.filter.is_empty() },
                act("search", 0),
            ));
            if self.c.filter.is_empty() && !self.c.searching {
                o.items.push(item(
                    Rect { x: fr.x + cw, w: fw - 2 * cw, ..fr },
                    format!("search ({})", keys.label("search")),
                    PanelStyle::Hint,
                    act("search", 0),
                ));
            }
            body.y += row_h + 8;
            body.h -= row_h + 8;
            if let Some(e) = &self.c.error {
                o.items.push(item(Rect { x: body.x, y: body.y, w: body.w, h: row_h }, e.clone(), PanelStyle::Error, UiAction::None));
            } else {
                match self.c.view {
                    Layout::List => {
                        self.per_row = 1;
                        self.list(body, cw, ch, &mut o)
                    }
                    Layout::Grid => self.grid(body, cw, ch, &mut o),
                    Layout::Carousel => {
                        self.per_row = 1;
                        self.carousel(body, cw, ch, &mut o)
                    }
                }
            }
        }
        // Only what is on screen loads; text snippets of files that left are dropped.
        self.previews.want(o.slots.iter().map(|s| s.0.clone()).collect());
        self.texts.retain(|p, _| o.texts.contains(p));
        let mut images = Vec::new();
        for (want, full, clip) in &o.slots {
            match self.previews.get(want) {
                Some(Some(t)) => {
                    let r = fit(*full, t.w, t.h);
                    images.push((ModeImage { key: t.key, rgba: t.rgba.clone(), width: t.w, height: t.h, src: (0, 0, t.w, t.h), rect: r }, *clip));
                }
                Some(None) => o.items.push(item(Rect { x: clip.x, y: clip.y, w: clip.w, h: ch }, "(no preview)", PanelStyle::Hint, UiAction::None)),
                None => o.items.push(item(Rect { x: clip.x, y: clip.y, w: clip.w, h: ch }, "loading…", PanelStyle::Hint, UiAction::None)),
            }
        }
        let panel = match self.c.overlay {
            Some(Overlay::Options) => Some(self.options_panel(ctx)),
            Some(Overlay::Man) => Some(self.man_panel(ctx, keys)),
            _ => None,
        };
        View { dock: Some(Panel { rect, items: o.items }), panel, images: images.into_iter().filter_map(|(i, clip)| crop(i, clip)).collect() }
    }
}

impl CommanderMode {
    fn effect(&self, e: cmdr::Effect) -> Effect {
        match e {
            cmdr::Effect::None => Effect::None,
            cmdr::Effect::Cd(p) => Effect::Type(cd_line(&p)),
            cmdr::Effect::External(p) => Effect::Open(p),
        }
    }
}

/// Cut an image drawn at `i.rect` to the visible part `clip` (carousel
/// neighbours): the destination shrinks and `src` follows.
fn crop(mut i: ModeImage, clip: Rect) -> Option<ModeImage> {
    let f = i.rect;
    let (l, t) = (f.x.max(clip.x), f.y.max(clip.y));
    let (r, b) = ((f.x + f.w).min(clip.x + clip.w), (f.y + f.h).min(clip.y + clip.h));
    if r <= l || b <= t {
        return None;
    }
    let (sx, sy) = (i.width as f64 / f.w as f64, i.height as f64 / f.h as f64);
    let px = |v: i32, s: f64, max: u32| ((v as f64 * s) as u32).min(max);
    let (x0, y0) = (px(l - f.x, sx, i.width), px(t - f.y, sy, i.height));
    let (x1, y1) = (px(r - f.x, sx, i.width), px(b - f.y, sy, i.height));
    i.src = (x0, y0, (x1 - x0).max(1), (y1 - y0).max(1));
    i.rect = Rect { x: l, y: t, w: r - l, h: b - t };
    Some(i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UiViewport;

    fn ctx() -> Context {
        let viewport = UiViewport { width: 1200, height: 900, cell_w: 9, cell_h: 18 };
        Context { viewport, area: Rect { x: 0, y: 8, w: 1000, h: 892 } }
    }

    fn png(p: &Path) {
        image::RgbaImage::from_pixel(40, 20, image::Rgba([255, 0, 0, 255])).save(p).unwrap();
    }

    fn wait_loaded(m: &mut CommanderMode, n: usize) -> View {
        let k = Keymap::new(cmdr::BINDINGS);
        for _ in 0..200 {
            let v = m.view(&ctx(), &k);
            if v.images.len() >= n {
                return v;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        m.view(&ctx(), &k)
    }

    #[test]
    fn carousel_loads_three_and_follows() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..8 {
            png(&t.path().join(format!("p{i}.png")));
        }
        let mut m = CommanderMode::new();
        m.c.flags.all = false;
        m.cwd_changed(t.path());
        m.action("view", 2);
        m.c.selected = 3;
        let v = wait_loaded(&mut m, 3);
        assert_eq!(v.images.len(), 3, "focused + two partial neighbours");
        let mut got: Vec<_> = m.previews().decoded().into_iter().map(|w| w.0.file_name().unwrap().to_string_lossy().into_owned()).collect();
        got.sort();
        assert_eq!(got, ["p2.png", "p3.png", "p4.png"], "nothing off screen is loaded");
        // The neighbours are cut by the view's edges: a part of the image.
        let side = v.images.iter().min_by_key(|i| i.rect.x).unwrap();
        assert!(side.src.2 < side.width, "left neighbour is partial");
        m.action("right", 0);
        wait_loaded(&mut m, 3);
        assert!(m.previews().decoded().iter().any(|w| w.0.ends_with("p5.png")), "the next one loads as it moves");
        assert!(!m.previews().decoded().iter().any(|w| w.0.ends_with("p6.png")));
    }

    #[test]
    fn list_grid_and_open() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("sub dir")).unwrap();
        std::fs::write(t.path().join("a.txt"), "hello\nworld\n").unwrap();
        let mut m = CommanderMode::new();
        m.c.flags.all = false;
        m.cwd_changed(t.path());
        let k = Keymap::new(cmdr::BINDINGS);
        let v = m.view(&ctx(), &k);
        let d = v.dock.unwrap();
        assert!(d.items.iter().any(|i| i.text.contains("Attributes") && i.text.contains("Last Modified")));
        assert!(d.items.iter().any(|i| i.text.contains("sub dir/") && i.action == act("item", 1)));
        assert!(d.rect.h < 892, "leaves room for the prompt");
        m.action("item", 0);
        assert_eq!(m.action("item", 0), Effect::None, "a.txt: file view");
        let v = m.view(&ctx(), &k);
        assert!(v.dock.unwrap().items.iter().any(|i| i.text == "world"));
        assert!(m.escape());
        assert_eq!(m.action("item", 1), Effect::None, "first click selects");
        let cd = m.action("item", 1);
        assert_eq!(cd, Effect::Type(format!("\x15cd -- '{}/sub dir'\r", t.path().display())));
        m.action("switch_view", 0);
        let v = m.view(&ctx(), &k);
        assert!(v.dock.unwrap().items.iter().filter(|i| matches!(i.style, PanelStyle::Tile { .. })).count() == 2);
        assert!(m.text(b"x").is_none(), "not searching: typing goes to the shell");
        m.action("search", 0);
        assert_eq!(m.text(b"s"), Some(Effect::None));
        assert_eq!(m.c.filter, "s");
        assert!(m.text(b"\x1bt").is_none(), "bound keys still work while searching");
    }
}
