//! Views and media Mouser runs for a mode plugin (mode-api 0.4,
//! `mouser_mode_api::hosted`, docs/mode-protocol.md §Host views and media).
//! The plugin registers them by id ([`Out::Hosted`]); this keeps their
//! selection, scroll, layout and carousel slide, lays them out with
//! `mouser-views`, loads thumbnails and plays media with `mouser-media`,
//! takes their keys and clicks and tells the plugin what happened as
//! `view.*` / `media.*` events. [`super::stdio::StdioMode`] owns one.
//!
//! [`Out::Hosted`]: mouser_mode_api::protocol::Out::Hosted

use super::{ModeImage, View};
use crate::Waker;
use crate::ui::{Panel, PanelItem, PanelStyle, Rect, UiAction};
use mouser_media::detect::{Kind, MediaRef, kind_of};
use mouser_media::still::{Slot, fit};
use mouser_media::view::{MediaView, Pic};
use mouser_mode_api::hosted::{Hosted, HostedItem, HostedView, ItemIcon, ViewLayout};
use mouser_views::anim::Slide;
use mouser_views::carousel::Strip;
use mouser_views::geom::crop;
use mouser_views::grid::{Grid, Tiles};
use mouser_views::text::clip_text;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Action ids of hosted items start with this (the plugin never sees them).
pub const PREFIX: &str = "__hosted.";

#[derive(Default)]
struct Items {
    selected: usize,
    scroll: usize,
    layout: ViewLayout,
    /// The last `select` / `layout` the plugin sent (a change moves it).
    sent_select: Option<u32>,
    sent_layout: Option<ViewLayout>,
    slide: Slide,
    len: usize,
    /// Items a row and a page hold in the last frame (keys move by them).
    per_row: usize,
    page: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct MediaState {
    playing: bool,
    position: i64,
    duration: f64,
    fullscreen: bool,
    error: Option<String>,
}

#[derive(Default)]
pub struct HostedViews {
    specs: Vec<HostedView>,
    items: HashMap<String, Items>,
    media: MediaView,
    /// The fullscreen player's space and layer (`true`: the panel).
    full_area: Option<(Rect, bool)>,
    told: HashMap<String, MediaState>,
    ended: HashSet<String>,
    autoplayed: HashSet<(String, String)>,
    events: Vec<(String, Value)>,
    waker: Option<Waker>,
}

fn act(id: impl Into<String>, arg: i32) -> UiAction {
    UiAction::Mode { action: format!("{PREFIX}{}", id.into()), arg }
}

fn item(rect: Rect, text: impl Into<String>, style: PanelStyle, action: UiAction) -> PanelItem {
    PanelItem { rect, text: text.into(), style, action }
}

fn media_ref(path: &str) -> MediaRef {
    let path = PathBuf::from(path);
    MediaRef { kind: kind_of(&path).unwrap_or(Kind::Image), path }
}

impl HostedViews {
    pub fn set_waker(&mut self, w: Waker) {
        self.media.waker = Some(w.clone());
        self.waker = Some(w);
    }

    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// The plugin's views now (each [`Out::Hosted`] replaces the last).
    ///
    /// [`Out::Hosted`]: mouser_mode_api::protocol::Out::Hosted
    pub fn set(&mut self, specs: Vec<HostedView>) {
        let mut ids = HashSet::new();
        for s in &specs {
            ids.insert(s.id.clone());
            let Hosted::Items { layout, items, select, .. } = &s.content else { continue };
            let st = self.items.entry(s.id.clone()).or_default();
            if st.sent_layout != Some(*layout) {
                st.layout = *layout;
                st.sent_layout = Some(*layout);
            }
            if *select != st.sent_select {
                if let Some(i) = select {
                    st.selected = *i as usize;
                }
                st.sent_select = *select;
            }
            st.len = items.len();
            st.selected = st.selected.min(items.len().saturating_sub(1));
        }
        self.items.retain(|k, _| ids.contains(k));
        let paths: HashSet<PathBuf> = specs
            .iter()
            .filter_map(|s| match &s.content {
                Hosted::Media { path, .. } => Some(PathBuf::from(path)),
                _ => None,
            })
            .collect();
        if self.media.player.as_ref().is_some_and(|p| !paths.contains(&p.path)) {
            self.media.player = None;
        }
        if self.media.full.as_ref().is_some_and(|m| !paths.contains(&m.path)) {
            self.media.full = None;
            self.media.anim = None;
        }
        self.told.retain(|k, _| ids.contains(k));
        self.autoplayed.retain(|(k, _)| ids.contains(k));
        self.specs = specs;
    }

    /// The mode is hidden: nothing keeps playing.
    pub fn pause(&mut self) {
        if let Some(p) = self.media.player.as_mut().filter(|p| p.playing()) {
            p.pause();
        }
    }

    /// Events for the plugin since the last call.
    pub fn take_events(&mut self) -> Vec<(String, Value)> {
        std::mem::take(&mut self.events)
    }

    fn focused(&self) -> Option<&HostedView> {
        self.specs.iter().rev().find(|s| matches!(s.content, Hosted::Items { focused: true, .. }))
    }

    /// A fullscreen player is open.
    pub fn fullscreen(&self) -> bool {
        self.media.is_open()
    }

    fn select(&mut self, id: &str, to: usize) {
        let Some(st) = self.items.get_mut(id) else { return };
        let to = to.min(st.len.saturating_sub(1));
        if st.len == 0 || to == st.selected {
            return;
        }
        st.selected = to;
        self.events.push(("view.selected".into(), json!({"id": id, "index": to})));
        self.wake();
    }

    fn activate(&mut self, id: &str) {
        if let Some(st) = self.items.get(id).filter(|s| s.len > 0) {
            self.events.push(("view.activated".into(), json!({"id": id, "index": st.selected})));
        }
    }

    fn wake(&self) {
        if let Some(w) = &self.waker {
            w()
        }
    }

    /// What a key does in the focused view (`None`: not taken).
    fn key_move(&self, s: &str) -> Option<(String, KeyDo)> {
        if self.media.is_open() {
            return None;
        }
        let v = self.focused()?;
        let st = self.items.get(&v.id)?;
        if st.len == 0 {
            return None;
        }
        let (row, page, sel, last) = (st.per_row.max(1) as i64, st.page.max(1) as i64, st.selected as i64, st.len as i64 - 1);
        let (horiz, vert) = match st.layout {
            ViewLayout::List => (0, 1),
            ViewLayout::Grid => (1, row),
            ViewLayout::Carousel => (1, 0),
        };
        let to = |d: i64| Some(KeyDo::Select((sel + d).clamp(0, last) as usize));
        let d = match s {
            "\x1b[A" | "\x1bOA" if vert > 0 => to(-vert),
            "\x1b[B" | "\x1bOB" if vert > 0 => to(vert),
            "\x1b[D" | "\x1bOD" if horiz > 0 => to(-1),
            "\x1b[C" | "\x1bOC" if horiz > 0 => to(1),
            "\x1b[5~" => to(-page),
            "\x1b[6~" => to(page),
            "\x1b[H" | "\x1bOH" | "\x1b[1~" => to(-sel),
            "\x1b[F" | "\x1bOF" | "\x1b[4~" => to(last - sel),
            "\r" | "\n" => Some(KeyDo::Activate),
            "v" => Some(KeyDo::Layout),
            _ => None,
        }?;
        Some((v.id.clone(), d))
    }

    /// Would [`HostedViews::key`] take these bytes?
    pub fn takes(&self, bytes: &[u8]) -> bool {
        self.media.is_open() || self.key_move(&String::from_utf8_lossy(bytes)).is_some()
    }

    /// A key while the mode has the keyboard: the fullscreen player's, then
    /// the focused view's navigation. `true`: taken.
    pub fn key(&mut self, bytes: &[u8]) -> bool {
        let s = String::from_utf8_lossy(bytes).into_owned();
        if let Some((area, _)) = self.full_area.filter(|_| self.media.is_open()) {
            return self.media.key(&s, area);
        }
        let Some((id, d)) = self.key_move(&s) else { return false };
        match d {
            KeyDo::Select(i) => self.select(&id, i),
            KeyDo::Activate => self.activate(&id),
            KeyDo::Layout => {
                if let Some(st) = self.items.get_mut(&id) {
                    st.layout = st.layout.next();
                    let l = st.layout.name();
                    self.events.push(("view.layout".into(), json!({"id": id, "layout": l})));
                    self.wake();
                }
            }
        }
        true
    }

    /// Esc: closes the fullscreen player. `true`: taken.
    pub fn escape(&mut self) -> bool {
        match self.full_area.filter(|_| self.media.is_open()) {
            Some((area, _)) => {
                self.media.close(area);
                true
            }
            None => false,
        }
    }

    /// A click on a hosted item (`id` without [`PREFIX`]).
    pub fn action(&mut self, id: &str, arg: i32) {
        if let Some(view) = id.strip_prefix("item:") {
            let i = arg.max(0) as usize;
            let view = view.to_string();
            if self.items.get(&view).is_some_and(|s| s.selected == i) {
                self.activate(&view);
            } else {
                self.select(&view, i);
            }
        } else if id.starts_with("media_") {
            let area = self.full_area.map(|a| a.0).or_else(|| self.media.shown.get(arg.max(0) as usize).map(|s| s.1)).unwrap_or_default();
            self.media.action(id, arg, area);
            self.wake();
        }
    }

    /// Draw the views into `view` (onto the plugin's dock and panel).
    /// `files`: the plugin has `files.read` (thumbnails and media need it).
    pub fn draw(&mut self, view: &mut View, (cw, ch): (i32, i32), files: bool, reduce_motion: bool) {
        if self.specs.is_empty() {
            return;
        }
        let (cw, ch) = (cw.max(1), ch.max(1));
        self.media.reduce_motion = reduce_motion;
        self.media.begin();
        let mut dock = Vec::new();
        let mut panel = Vec::new();
        let mut pics: Vec<(Pic, Rect)> = Vec::new();
        let specs = self.specs.clone();
        let mut full_layer = None;
        for s in &specs {
            let out = if s.in_panel { &mut panel } else { &mut dock };
            match &s.content {
                Hosted::Items { items, focused, tile, .. } => {
                    self.draw_items(s, items, *focused, *tile, (cw, ch), files, reduce_motion, out, &mut pics)
                }
                Hosted::Media { path, autoplay } => {
                    let m = media_ref(path);
                    if !files {
                        let n = m.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        out.push(item(Rect { h: ch, ..s.rect }, format!("{n} (needs files.read)"), PanelStyle::Hint, UiAction::None));
                        continue;
                    }
                    let mut items = Vec::new();
                    let mut p = Vec::new();
                    let first = self.media.shown.len() as i32;
                    self.media.inline(&m, s.rect, (cw, ch + 4), &mut items, &mut p);
                    out.extend(items.into_iter().map(|i| PanelItem { rect: i.rect, text: i.text, style: i.style, action: host_action(i.action) }));
                    pics.extend(p.into_iter().map(|p| (p, s.rect)));
                    if self.media.full.as_ref() == Some(&m) {
                        full_layer = Some(s.in_panel);
                    }
                    let key = (s.id.clone(), path.clone());
                    if *autoplay && m.kind != Kind::Image && !self.autoplayed.contains(&key) {
                        self.autoplayed.insert(key);
                        if self.media.player.as_ref().is_none_or(|p| p.path != m.path) {
                            self.media.action("media_play", first, s.rect);
                        }
                    }
                    self.tell_media(&s.id, &m);
                }
            }
        }
        let layer_rect = |p: &Option<Panel>, fallback: Rect| p.as_ref().map_or(fallback, |p| p.rect);
        let mut full_items = Vec::new();
        let mut full_pics = Vec::new();
        let mut full_rect = None;
        if let Some(in_panel) = full_layer.or(self.full_area.map(|a| a.1)).filter(|_| self.media.is_open()) {
            let fallback = specs.iter().find(|s| s.in_panel == in_panel).map(|s| s.rect).unwrap_or_default();
            let area = layer_rect(if in_panel { &view.panel } else { &view.dock }, fallback);
            self.full_area = Some((area, in_panel));
            let mut items = Vec::new();
            if self.media.fullscreen(area, (cw, ch + 4), &mut items, &mut full_pics) {
                full_items =
                    items.into_iter().map(|i| PanelItem { rect: i.rect, text: i.text, style: i.style, action: host_action(i.action) }).collect();
                full_rect = Some((area, in_panel));
            }
        } else if !self.media.is_open() {
            self.full_area = specs.iter().find(|s| matches!(s.content, Hosted::Media { .. })).map(|s| {
                let p = if s.in_panel { &view.panel } else { &view.dock };
                (layer_rect(p, s.rect), s.in_panel)
            });
        }
        for (items, in_panel) in [(dock, false), (panel, true)] {
            if items.is_empty() {
                continue;
            }
            let slot = if in_panel { &mut view.panel } else { &mut view.dock };
            let rect = specs.iter().filter(|s| s.in_panel == in_panel).map(|s| s.rect).reduce(union).unwrap_or_default();
            slot.get_or_insert_with(|| Panel { rect, items: vec![] }).items.extend(items);
        }
        for (p, clip) in pics {
            push_pic(&mut view.images, p, clip);
        }
        if let Some((area, in_panel)) = full_rect {
            // Nothing drawn under the player shows through it.
            view.images.retain(|i| !mouser_views::geom::overlaps(i.rect, area));
            let slot = if in_panel { &mut view.panel } else { &mut view.dock };
            slot.get_or_insert_with(|| Panel { rect: area, items: vec![] }).items.extend(full_items);
            for p in full_pics {
                push_pic(&mut view.images, p, area);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_items(
        &mut self,
        s: &HostedView,
        items: &[HostedItem],
        focused: bool,
        tile: i32,
        (cw, ch): (i32, i32),
        files: bool,
        reduce_motion: bool,
        out: &mut Vec<PanelItem>,
        pics: &mut Vec<(Pic, Rect)>,
    ) {
        let waker = self.waker.clone();
        let stills = self.media.stills.clone();
        let Some(st) = self.items.get_mut(&s.id) else { return };
        let body = s.rect;
        let row_h = ch + 6;
        let id = s.id.as_str();
        let thumb = |it: &HostedItem, r: Rect, clip: Rect, out: &mut Vec<PanelItem>, pics: &mut Vec<(Pic, Rect)>| {
            let shown = match (&it.image, files) {
                (Some(p), true) => match stills.get(std::path::Path::new(p), waker.clone()) {
                    Slot::Ready(s) => {
                        let fr = fit(r, s.w, s.h, true);
                        pics.push((Pic { key: s.key, rgba: s.rgba, w: s.w, h: s.h, rect: fr }, clip));
                        true
                    }
                    _ => false,
                },
                _ => false,
            };
            if !shown {
                let ir = Rect { x: r.x + r.w / 4, y: r.y + r.h / 4, w: r.w / 2, h: r.h / 2 };
                match it.icon {
                    ItemIcon::Folder => out.push(item(mouser_views::geom::intersect(ir, clip), "", PanelStyle::FolderIcon, UiAction::None)),
                    ItemIcon::Badge(b) => out.push(item(mouser_views::geom::intersect(ir, clip), "", PanelStyle::Badge(b), UiAction::None)),
                    ItemIcon::None => {}
                }
            }
        };
        match st.layout {
            ViewLayout::List => {
                let vis = (body.h / row_h).max(1) as usize;
                if st.selected < st.scroll {
                    st.scroll = st.selected;
                }
                if st.selected >= st.scroll + vis {
                    st.scroll = st.selected + 1 - vis;
                }
                st.scroll = st.scroll.min(items.len().saturating_sub(vis));
                st.per_row = 1;
                st.page = vis;
                let bar = 6;
                for (n, i) in (st.scroll..items.len().min(st.scroll + vis)).enumerate() {
                    let it = &items[i];
                    let r = Rect { x: body.x, y: body.y + n as i32 * row_h, w: body.w - bar - 2, h: row_h };
                    let dw = if it.detail.is_empty() { 0 } else { (it.detail.chars().count() as i32 + 2) * cw };
                    let cols = ((r.w - dw) / cw - 1).max(1);
                    out.push(item(
                        r,
                        clip_text(&it.label, cols),
                        PanelStyle::Row { selected: i == st.selected && focused },
                        act(format!("item:{id}"), i as i32),
                    ));
                    if dw > 0 {
                        out.push(item(Rect { x: r.x + r.w - dw, w: dw, ..r }, it.detail.clone(), PanelStyle::Hint, UiAction::None));
                    }
                }
                let track = Rect { x: body.x + body.w - bar, y: body.y, w: bar, h: body.h };
                out.extend(mouser_views::scroll::scrollbar(track, st.scroll, vis, items.len(), UiAction::None));
            }
            ViewLayout::Grid => {
                let size = if tile > 0 { tile } else { (ch * 7).clamp(64, 200) };
                let g = Grid::new(body, Tiles { size, gap: cw, label_h: row_h, reserve: 8 }, items.len(), st.selected, st.scroll);
                st.scroll = g.first;
                st.per_row = g.per_row;
                st.page = g.page();
                for i in g.shown(items.len()) {
                    let r = g.tile_rect(i);
                    out.push(item(r, "", PanelStyle::Tile { selected: i == st.selected }, act(format!("item:{id}"), i as i32)));
                    thumb(&items[i], Rect { x: r.x + 4, y: r.y + 4, w: r.w - 8, h: r.h - 8 }, body, out, pics);
                    let lr = Rect { y: r.y + r.h + 2, h: row_h - 2, ..r };
                    out.push(item(lr, clip_text(&items[i].label, (r.w / cw).max(1)), PanelStyle::Label, act(format!("item:{id}"), i as i32)));
                }
                let track = Rect { x: body.x + body.w - 6, y: body.y, w: 6, h: body.h };
                out.extend(mouser_views::scroll::scrollbar(track, g.first, g.vis_rows, g.total_rows, UiAction::None));
            }
            ViewLayout::Carousel => {
                let h = if tile > 0 { tile.min(body.h - row_h) } else { body.h - row_h - 4 }.max(ch);
                let w = (h * 4 / 3).min(body.w * 3 / 5).max(cw);
                let strip = Strip::new(body, w, 2 * cw);
                let now = Instant::now();
                let pos = if reduce_motion { st.selected as f64 } else { st.slide.at(st.selected as f64, now) };
                if st.slide.moving() && !st.slide.ticking {
                    st.slide.ticking = true;
                    if let Some(w) = waker.clone() {
                        let until = st.slide.end() + Duration::from_millis(10);
                        std::thread::spawn(move || {
                            while Instant::now() < until {
                                std::thread::sleep(Duration::from_millis(8));
                                w();
                            }
                        });
                    }
                }
                st.per_row = 1;
                st.page = 5;
                for i in Strip::order(pos, st.selected, items.len()) {
                    let full = strip.rect(i, pos, h);
                    let Some(r) = strip.visible(full, 8) else { continue };
                    out.push(item(r, "", PanelStyle::Tile { selected: i == st.selected }, act(format!("item:{id}"), i as i32)));
                    thumb(&items[i], Rect { x: full.x + 4, y: full.y + 4, w: full.w - 8, h: full.h - 8 }, r, out, pics);
                }
                if let Some(it) = items.get(st.selected) {
                    let lw = ((it.label.chars().count() as i32 + 2) * cw).min(body.w);
                    let lr = Rect { x: body.x + (body.w - lw) / 2, y: body.y + h + 4, w: lw, h: row_h - 2 };
                    out.push(item(lr, clip_text(&it.label, lw / cw), PanelStyle::FloatLabel, UiAction::None));
                }
            }
        }
    }

    fn tell_media(&mut self, id: &str, m: &MediaRef) {
        let p = self.media.player.as_ref().filter(|p| p.path == m.path);
        let now = MediaState {
            playing: p.is_some_and(|p| p.playing()),
            position: p.map_or(0, |p| p.position().floor() as i64),
            duration: p.map_or(0.0, |p| p.info.duration),
            fullscreen: self.media.full.as_ref() == Some(m),
            error: p.and_then(|p| p.error.clone()),
        };
        let at_end = p.is_some_and(|p| !p.playing() && p.info.duration > 0.0 && p.position() >= p.info.duration - 0.25);
        if self.told.get(id) != Some(&now) {
            self.events.push((
                "media.state".into(),
                json!({"id": id, "playing": now.playing, "position": now.position, "duration": now.duration, "fullscreen": now.fullscreen, "error": now.error}),
            ));
            self.told.insert(id.to_string(), now.clone());
        }
        if at_end && self.ended.insert(id.to_string()) {
            self.events.push(("media.ended".into(), json!({"id": id})));
        } else if now.playing {
            self.ended.remove(id);
        }
    }
}

enum KeyDo {
    Select(usize),
    Activate,
    Layout,
}

/// A `mouser-media` action as the host routes it back here.
fn host_action(a: mouser_mode_api::Action) -> UiAction {
    match a {
        mouser_mode_api::Action::Mode { action, arg } => act(action, arg),
        mouser_mode_api::Action::None => UiAction::None,
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
    Rect { x, y, w: (a.x + a.w).max(b.x + b.w) - x, h: (a.y + a.h).max(b.y + b.h) - y }
}

fn push_pic(out: &mut Vec<ModeImage>, p: Pic, clip: Rect) {
    if let Some((src, rect)) = crop(p.rect, (p.w, p.h), clip) {
        out.push(ModeImage { key: p.key, rgba: p.rgba, width: p.w, height: p.h, src, rect });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(n: usize, layout: ViewLayout, select: Option<u32>) -> HostedView {
        HostedView {
            id: "v".into(),
            rect: Rect { x: 0, y: 0, w: 400, h: 300 },
            in_panel: false,
            content: Hosted::Items {
                layout,
                items: (0..n).map(|i| HostedItem { label: format!("item {i}"), ..Default::default() }).collect(),
                select,
                focused: true,
                tile: 0,
            },
        }
    }

    fn draw(h: &mut HostedViews) -> View {
        let mut v = View::default();
        h.draw(&mut v, (8, 16), true, true);
        v
    }

    fn selected(v: &View) -> Vec<String> {
        v.dock.iter().flat_map(|d| &d.items).filter(|i| matches!(i.style, PanelStyle::Row { selected: true })).map(|i| i.text.clone()).collect()
    }

    #[test]
    fn keys_move_a_list_and_tell_the_mode() {
        let mut h = HostedViews::default();
        h.set(vec![items(30, ViewLayout::List, None)]);
        let v = draw(&mut h);
        assert_eq!(selected(&v), ["item 0"]);
        assert!(h.takes(b"\x1b[B") && !h.takes(b"\x1b[C") && !h.takes(b"x"));
        assert!(h.key(b"\x1b[B") && h.key(b"\x1b[F"));
        draw(&mut h);
        assert_eq!(selected(&draw(&mut h)), ["item 29"], "End, scrolled into view");
        assert!(h.key(b"\r"));
        let ev = h.take_events();
        assert_eq!(ev.iter().map(|e| e.0.as_str()).collect::<Vec<_>>(), ["view.selected", "view.selected", "view.activated"]);
        assert_eq!(ev[2].1, json!({"id": "v", "index": 29}));
    }

    #[test]
    fn the_mode_moves_the_selection_only_when_it_changes_what_it_sends() {
        let mut h = HostedViews::default();
        h.set(vec![items(5, ViewLayout::List, Some(2))]);
        h.key(b"\x1b[B");
        // The same `select` again keeps the user's move.
        h.set(vec![items(5, ViewLayout::List, Some(2))]);
        assert_eq!(selected(&draw(&mut h)), ["item 3"]);
        h.set(vec![items(5, ViewLayout::List, Some(0))]);
        assert_eq!(selected(&draw(&mut h)), ["item 0"]);
        // Fewer items: the selection stays on one.
        h.set(vec![items(1, ViewLayout::List, Some(9))]);
        assert_eq!(selected(&draw(&mut h)), ["item 0"]);
    }

    #[test]
    fn grid_rows_carousel_and_layout_key() {
        let mut h = HostedViews::default();
        h.set(vec![items(10, ViewLayout::Grid, None)]);
        let v = draw(&mut h);
        let tiles = v.dock.iter().flat_map(|d| &d.items).filter(|i| matches!(i.style, PanelStyle::Tile { .. })).count();
        assert!(tiles > 1);
        let per_row = h.items["v"].per_row;
        h.key(b"\x1b[B");
        assert_eq!(h.items["v"].selected, per_row);
        h.key(b"v");
        assert_eq!(h.take_events().last().unwrap().1, json!({"id": "v", "layout": "carousel"}));
        let v = draw(&mut h);
        assert!(v.dock.iter().flat_map(|d| &d.items).any(|i| i.style == PanelStyle::FloatLabel && i.text == format!("item {per_row}")));
        assert!(!h.takes(b"\x1b[B") && h.takes(b"\x1b[D"));
        // Clicks: another item selects, the selected one activates.
        h.action("item:v", 0);
        h.action("item:v", 0);
        let ev = h.take_events();
        assert_eq!((ev[0].0.as_str(), ev[1].0.as_str()), ("view.selected", "view.activated"));
    }

    #[test]
    fn media_without_files_read_reads_nothing() {
        let mut h = HostedViews::default();
        let m = HostedView {
            id: "m".into(),
            rect: Rect { x: 0, y: 0, w: 300, h: 200 },
            in_panel: true,
            content: Hosted::Media { path: "/no/such.png".into(), autoplay: true },
        };
        h.set(vec![m]);
        let mut v = View::default();
        h.draw(&mut v, (8, 16), false, true);
        let p = v.panel.expect("drawn on the panel");
        assert_eq!(p.items[0].text, "such.png (needs files.read)");
        assert!(v.images.is_empty() && h.take_events().is_empty());
        h.set(vec![]);
        assert!(h.is_empty());
    }
}
