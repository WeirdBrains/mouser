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
    let img = if cmdr::kind_of(&p.to_string_lossy()) == Kind::Video {
        video_thumb(p, max)?
    } else {
        image::ImageReader::open(p).ok()?.with_guessed_format().ok()?.decode().ok()?
    };
    let img = if img.width() > max || img.height() > max { img.thumbnail(max, max) } else { img };
    let rgba = img.to_rgba8();
    Some(Thumb { key: NEXT_KEY.fetch_add(1, Ordering::Relaxed), w: rgba.width(), h: rgba.height(), rgba: Arc::new(rgba.into_raw()) })
}

/// Where a video's thumbnail is cached: `$MOUSER_HOME/cache/thumbs/<hash>.png`,
/// the hash over path, mtime, size and the requested size (a changed file
/// gets a new entry).
pub fn thumb_cache_path(p: &Path, max: u32) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let m = std::fs::metadata(p).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (p, m.modified().ok(), m.len(), max).hash(&mut h);
    Some(mouser_core::storage::resolve_home().join("cache/thumbs").join(format!("{:016x}.png", h.finish())))
}

/// A representative frame: at 10 % of the duration (at most 30 s in, 1 s for
/// clips without a duration), scaled so the longest side is `max`. Cached on
/// disk; `None` without ffmpeg or for files it cannot read.
fn video_thumb(p: &Path, max: u32) -> Option<image::DynamicImage> {
    let cache = thumb_cache_path(p, max);
    if let Some(c) = &cache
        && let Ok(i) = image::open(c)
    {
        return Some(i);
    }
    let dur = super::player::probe(p).map(|i| i.duration).unwrap_or(0.0);
    let at = if dur > 0.0 { (dur * 0.1).min(30.0) } else { 1.0 };
    let scale = format!("scale='if(gt(iw,ih),{max},-2)':'if(gt(iw,ih),-2,{max})'");
    let run = |t: f64| {
        std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-nostdin", "-ss", &format!("{t:.3}"), "-i"])
            .arg(p)
            .args(["-frames:v", "1", "-vf", &scale, "-f", "image2pipe", "-c:v", "png", "pipe:1"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success() && !o.stdout.is_empty())
    };
    let out = run(at).or_else(|| run(0.0))?;
    let img = image::load_from_memory(&out.stdout).ok()?;
    if let Some(c) = &cache {
        let _ = std::fs::create_dir_all(c.parent()?);
        let _ = std::fs::write(c, &out.stdout);
    }
    Some(img)
}

/// What a tile shows while its preview loads or when there is none.
fn kind_label(k: Kind) -> &'static str {
    match k {
        Kind::Dir => "folder",
        Kind::Video => "▶ video",
        Kind::Audio => "♪ audio",
        Kind::Image => "image",
        _ => "file",
    }
}

/// Carousel slide: eases the shown position from where it was to the
/// selection over [`Slide::DUR`]; a new target mid-slide starts from the
/// current position and speed is continuous enough (ease-in-out).
#[derive(Debug, Clone, Copy, Default)]
pub struct Slide {
    from: f64,
    to: f64,
    start: Option<std::time::Instant>,
    /// A redraw ticker is running for this slide.
    ticking: bool,
}

impl Slide {
    pub const DUR: std::time::Duration = std::time::Duration::from_millis(180);

    /// Ease-in-out (cubic): slow start, fast middle, slow stop.
    pub fn ease(t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
    }

    fn value(&self, now: std::time::Instant) -> f64 {
        match self.start {
            Some(s) => {
                let t = now.saturating_duration_since(s).as_secs_f64() / Self::DUR.as_secs_f64();
                self.from + (self.to - self.from) * Self::ease(t)
            }
            None => self.to,
        }
    }

    /// The position to draw for `target` at `now` (retargets smoothly).
    pub fn at(&mut self, target: f64, now: std::time::Instant) -> f64 {
        if self.start.is_none() && self.from == 0.0 && self.to == 0.0 {
            self.to = target;
            self.from = target;
        }
        if target != self.to {
            let cur = self.value(now);
            // Far jumps (Home/End, search) don't fly past every item.
            self.from = if (target - cur).abs() > 3.0 { target - (target - cur).signum() * 1.0 } else { cur };
            self.to = target;
            self.start = Some(now);
            self.ticking = false;
        }
        let v = self.value(now);
        if self.start.is_some_and(|s| now.saturating_duration_since(s) >= Self::DUR) {
            self.start = None;
            self.from = self.to;
            self.ticking = false;
            return self.to;
        }
        v
    }

    pub fn moving(&self) -> bool {
        self.start.is_some()
    }

    fn end(&self) -> std::time::Instant {
        self.start.map_or_else(std::time::Instant::now, |s| s + Self::DUR)
    }
}

pub struct CommanderMode {
    pub c: Commander,
    per_row: usize,
    previews: Previews,
    /// First lines of text files on screen.
    texts: HashMap<PathBuf, Vec<String>>,
    /// The file view's text.
    full_text: Option<(PathBuf, Vec<String>, Option<String>)>,
    /// Last click (item, when) for double-click.
    last_click: Option<(usize, std::time::Instant)>,
    /// The media player while a video or audio file is open.
    pub player: Option<super::player::Player>,
    waker: Option<Waker>,
    /// Width of the view's body (the player decodes to it).
    body_w: i32,
    /// The player's key hints, from the current keymap.
    player_keys: String,
    slide: Slide,
    /// Image view zoom (None: fit).
    zoom: Option<Zoom>,
    /// Natural size of the image in the file view.
    nat: Option<(PathBuf, (u32, u32))>,
    /// Last pointer position over the view (window px).
    pointer: (i32, i32),
    press: Option<Press>,
    hits: Hits,
    /// Header overflow (•••) dropdown open.
    menu_open: bool,
    last_scrub: Option<std::time::Instant>,
}

/// Zoom of the image in the file view: scale (1.0 = actual pixels) and the
/// image point (0..1) at the centre of the body.
#[derive(Clone, Debug, PartialEq)]
pub struct Zoom {
    pub path: PathBuf,
    pub scale: f64,
    pub cx: f64,
    pub cy: f64,
}

#[derive(Clone, Copy, Debug)]
enum Press {
    Pan { at: (i32, i32), c0: (f64, f64) },
    Scrub,
    Volume,
}

/// Where the interactive parts were drawn last frame.
#[derive(Clone, Copy, Debug, Default)]
struct Hits {
    bar: Rect,
    vol: Rect,
    image: Rect,
    video: Rect,
    menu: Rect,
}

fn inside(r: Rect, (x, y): (i32, i32)) -> bool {
    r.w > 0 && x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
}

/// Pointer position packed into an action argument (`x | y << 15`).
pub fn pack_xy(x: i32, y: i32) -> i32 {
    x.clamp(0, 0x7fff) | (y.clamp(0, 0x7fff) << 15)
}

fn unpack_xy(a: i32) -> (i32, i32) {
    (a & 0x7fff, (a >> 15) & 0x7fff)
}

/// Fit scale of a `nat` image in `body`.
fn fit_scale(body: Rect, nat: (u32, u32)) -> f64 {
    if nat.0 == 0 || nat.1 == 0 {
        return 1.0;
    }
    (body.w as f64 / nat.0 as f64).min(body.h as f64 / nat.1 as f64)
}

/// Where a zoomed image goes: centred when smaller than the body, else
/// positioned by the centre point (clamped so no gap shows at the edges).
pub fn zoom_rect(body: Rect, nat: (u32, u32), z: &mut Zoom) -> Rect {
    let (dw, dh) = (nat.0 as f64 * z.scale, nat.1 as f64 * z.scale);
    let axis = |b0: i32, bl: i32, d: f64, c: &mut f64| -> i32 {
        if d <= bl as f64 {
            *c = 0.5;
            b0 + ((bl as f64 - d) / 2.0) as i32
        } else {
            let half = bl as f64 / 2.0 / d;
            *c = c.clamp(half, 1.0 - half);
            (b0 as f64 + bl as f64 / 2.0 - *c * d).round() as i32
        }
    };
    let x = axis(body.x, body.w, dw, &mut z.cx);
    let y = axis(body.y, body.h, dh, &mut z.cy);
    Rect { x, y, w: dw.round().max(1.0) as i32, h: dh.round().max(1.0) as i32 }
}

/// Zoom by `factor` keeping the image point under `anchor` still.
pub fn zoom_about(body: Rect, nat: (u32, u32), z: &mut Zoom, factor: f64, anchor: (i32, i32)) {
    let r = zoom_rect(body, nat, z);
    let u = ((anchor.0 - r.x) as f64 / r.w as f64).clamp(0.0, 1.0);
    let v = ((anchor.1 - r.y) as f64 / r.h as f64).clamp(0.0, 1.0);
    let fit = fit_scale(body, nat);
    let s1 = (z.scale * factor).clamp(fit.min(1.0) * 0.5, 16.0);
    let (dw, dh) = (nat.0 as f64 * s1, nat.1 as f64 * s1);
    z.cx = u + (body.x as f64 + body.w as f64 / 2.0 - anchor.0 as f64) / dw;
    z.cy = v + (body.y as f64 + body.h as f64 / 2.0 - anchor.1 as f64) / dh;
    z.scale = s1;
}

/// Header buttons that fit in `room` px (right to left); the rest go to the
/// ••• menu. Returns how many fit.
pub fn header_fit(widths: &[i32], room: i32, menu_w: i32) -> usize {
    let all: i32 = widths.iter().sum();
    if all <= room {
        return widths.len();
    }
    let mut used = menu_w;
    widths
        .iter()
        .take_while(|w| {
            used += **w;
            used <= room
        })
        .count()
}

impl Default for CommanderMode {
    fn default() -> Self {
        Self::new()
    }
}

/// `cd -- '<path>'` (run with [`Effect::Run`], keeping the typed line).
pub fn cd_line(p: &Path) -> String {
    format!("cd -- {}", cmdr::shell_quote(&p.to_string_lossy()))
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
    /// Player frames, ready to draw.
    frames: Vec<ModeImage>,
    /// (what, full rect, visible part)
    slots: Vec<(Want, Rect, Rect)>,
    texts: Vec<PathBuf>,
    /// Loading in the background (not drawn yet).
    extra: Vec<Want>,
    /// The open ••• dropdown (images under it are hidden).
    menu: Option<Rect>,
}

impl CommanderMode {
    pub fn new() -> Self {
        Self {
            c: Commander::new(),
            per_row: 1,
            previews: Previews::default(),
            texts: HashMap::new(),
            full_text: None,
            last_click: None,
            player: None,
            waker: None,
            body_w: 0,
            player_keys: String::new(),
            slide: Slide::default(),
            zoom: None,
            nat: None,
            pointer: (0, 0),
            press: None,
            hits: Hits::default(),
            menu_open: false,
            last_scrub: None,
        }
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
        // Remote files are not fetched: their kind and a badge.
        if self.c.remote.is_some() {
            let label = if e.kind == Kind::Dir { "folder".to_string() } else { format!("{} · remote", kind_label(e.kind)) };
            let w = (label.chars().count() as i32 * cw).min(clip.w);
            let r = Rect { x: clip.x + (clip.w - w).max(0) / 2, y: clip.y + (clip.h - ch) / 2, w, h: ch };
            o.items.push(item(r, label, if e.kind == Kind::Dir { PanelStyle::Label } else { PanelStyle::Hint }, UiAction::None));
            return;
        }
        match e.kind {
            Kind::Image | Kind::Video => {
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
                let label = kind_label(k);
                let w = label.chars().count() as i32 * cw;
                let r = Rect { x: clip.x + (clip.w - w).max(0) / 2, y: clip.y + (clip.h - ch) / 2, w: w.min(clip.w), h: ch };
                o.items.push(item(r, label, if k == Kind::Dir { PanelStyle::Label } else { PanelStyle::Hint }, UiAction::None));
            }
        }
    }

    fn list(&mut self, body: Rect, cw: i32, ch: i32, o: &mut Out) {
        let row_h = ch + 4;
        let rows = ((body.h / row_h) - 1).max(1) as usize;
        self.c.page = rows;
        self.c.follow(rows);
        let cols = body.w / cw.max(1);
        let name_w = (cols - 10 - 2 - 6 - 2 - 2 - 13 - 2).max(8) as usize;
        let head = format!(" {:<10} {:>6}  {:<name_w$}  {}", "Attributes", "Size", "Name", "Last Modified");
        o.items.push(item(Rect { x: body.x, y: body.y, w: body.w, h: row_h }, head, PanelStyle::Hint, UiAction::None));
        let vis: Vec<Entry> = self.c.visible().into_iter().cloned().collect();
        for (n, e) in vis.iter().enumerate().skip(self.c.scroll).take(rows) {
            let y = body.y + row_h * (1 + (n - self.c.scroll) as i32);
            let name = if e.kind == Kind::Dir && e.name != "." && e.name != ".." { format!("{}/", e.name) } else { e.name.clone() };
            let t = e.modified.map(cmdr::format_time).unwrap_or_default();
            let mark = if self.c.marks.contains(&e.path) { '*' } else { ' ' };
            let text = format!("{mark}{:<10} {:>6}  {:<name_w$}  {t}", e.mode, self.size(e), clip_text(&name, name_w as i32));
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
        self.c.page = per_row * vis_rows;
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
            let label = if self.c.marks.contains(&e.path) { format!("* {}", e.name) } else { e.name.clone() };
            let label = clip_text(&label, ts / cw.max(1));
            o.items.push(item(Rect { x: tile.x, y: tile.y + ts + 2, w: ts, h: label_h }, label, PanelStyle::Label, act("item", n as i32)));
        }
    }

    fn carousel(&mut self, body: Rect, cw: i32, ch: i32, o: &mut Out) {
        self.c.page = 5;
        let vis: Vec<Entry> = self.c.visible().into_iter().cloned().collect();
        let Some(e) = vis.get(self.c.selected).cloned() else { return };
        let pos = self.slide.at(self.c.selected as f64, std::time::Instant::now());
        if self.slide.moving() {
            self.keep_redrawing();
        }
        let label_h = ch + 6;
        let ih = (body.h - label_h - 6).max(ch);
        let w = (body.w * 55 / 100).max(cw * 10);
        let gap = 14;
        let x_mid = body.x + (body.w - w) / 2;
        // Items sit on a strip; `pos` (fractional while sliding) is in the middle.
        // Neighbours are the same size, cut off by the edges (partial).
        // What is on the strip now, plus the selection's neighbours (the
        // incoming item while sliding).
        let sel = self.c.selected as i64;
        let lo = (pos.floor() as i64 - 1).min(sel - 1).max(0) as usize;
        let hi = ((pos.ceil() as i64 + 1).max(sel + 1).max(0) as usize).min(vis.len().saturating_sub(1));
        let mut order: Vec<usize> = (lo..=hi).collect();
        // The one nearest the middle loads (and draws its frame) first.
        order.sort_by(|a, b| (*a as f64 - pos).abs().total_cmp(&(*b as f64 - pos).abs()));
        for &i in &order {
            let x = x_mid + ((i as f64 - pos) * (w + gap) as f64).round() as i32;
            let full = Rect { x, y: body.y, w, h: ih };
            let (l, r) = (full.x.max(body.x), (full.x + w).min(body.x + body.w));
            if r - l < 8 {
                // Off the strip still: load it so it is ready when it slides in.
                if matches!(vis[i].kind, Kind::Image | Kind::Video) {
                    o.slots.push(((vis[i].path.clone(), 1024), full, Rect { w: 0, h: 0, ..full }));
                }
                continue;
            }
            let clip = Rect { x: l, y: full.y, w: r - l, h: ih };
            o.items.push(item(clip, "", PanelStyle::Tile { selected: i == self.c.selected }, act("item", i as i32)));
            let inner = |r: Rect| Rect { x: r.x + 6, y: r.y + 6, w: r.w - 12, h: r.h - 12 };
            self.content(&vis[i], (inner(full), inner(clip)), 1024, (cw, ch), o);
        }
        let t = e.modified.map(cmdr::format_time).unwrap_or_default();
        let mk = if self.c.marks.contains(&e.path) { "* " } else { "" };
        let label = format!("{mk}{}   {}   {}   ({}/{})", e.name, t, self.size(&e), self.c.selected + 1, vis.len());
        let lw = (label.chars().count() as i32 * cw).min(body.w);
        o.items.push(item(
            Rect { x: body.x + (body.w - lw) / 2, y: body.y + ih + 6, w: lw, h: label_h },
            clip_text(&label, body.w / cw.max(1)),
            PanelStyle::Label,
            UiAction::None,
        ));
    }

    /// Wake the UI every ~8 ms until the carousel stops (only while it moves).
    fn keep_redrawing(&mut self) {
        if self.slide.ticking {
            return;
        }
        let Some(w) = self.waker.clone() else { return };
        self.slide.ticking = true;
        let until = self.slide.end();
        let _ = std::thread::Builder::new().name("carousel-anim".into()).spawn(move || {
            while std::time::Instant::now() < until + std::time::Duration::from_millis(10) {
                std::thread::sleep(std::time::Duration::from_millis(8));
                w();
            }
        });
    }

    /// The full-width view of a file.
    fn file_view(&mut self, p: &Path, body: Rect, cw: i32, ch: i32, o: &mut Out) {
        if self.player.as_ref().is_some_and(|pl| pl.path == p) {
            return self.player_view(body, cw, ch, o);
        }
        if cmdr::kind_of(&p.to_string_lossy()) == Kind::Image {
            self.hits.image = body;
            let nat = self.natural(p);
            let zoomed = self.zoom.as_ref().is_some_and(|z| z.path == p);
            match (nat, zoomed) {
                (Some(nat), true) => {
                    let z = self.zoom.as_mut().unwrap();
                    let r = zoom_rect(body, nat, z);
                    // Past the 2048 px preview: the full image, the preview until it loads.
                    let hi = (p.to_path_buf(), 16384);
                    if r.w.max(r.h) > 2048 && !matches!(self.previews.get(&hi), Some(Some(_))) {
                        o.extra.push(hi);
                        o.slots.push(((p.to_path_buf(), 2048), r, body));
                    } else if r.w.max(r.h) > 2048 {
                        o.slots.push((hi, r, body));
                    } else {
                        o.slots.push(((p.to_path_buf(), 2048), r, body));
                    }
                }
                _ => o.slots.push(((p.to_path_buf(), 2048), body, body)),
            }
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

    /// Natural size of an image (cached for the open file).
    fn natural(&mut self, p: &Path) -> Option<(u32, u32)> {
        if self.nat.as_ref().is_none_or(|n| n.0 != p) {
            let d = image::image_dimensions(p).ok()?;
            self.nat = Some((p.to_path_buf(), d));
        }
        self.nat.as_ref().map(|n| n.1)
    }

    /// Zoom label for the header ("fit 37%", "200%").
    fn zoom_label(&mut self, p: &Path) -> Option<String> {
        let body = self.hits.image;
        let nat = self.natural(p)?;
        Some(match self.zoom.as_ref().filter(|z| z.path == p) {
            Some(z) => format!("{:.0}%", z.scale * 100.0),
            None => format!("fit {:.0}%", fit_scale(body, nat) * 100.0),
        })
    }

    /// Zoom the open image; `factor` None = fit, Some(0.0) = 100%.
    fn zoom_image(&mut self, factor: Option<f64>, anchor: Option<(i32, i32)>) -> bool {
        let Some(Overlay::Preview(p)) = self.c.overlay.clone() else { return false };
        if cmdr::kind_of(&p.to_string_lossy()) != Kind::Image {
            return false;
        }
        let Some(nat) = self.natural(&p) else { return true };
        let body = self.hits.image;
        let fit = fit_scale(body, nat);
        let mut z = self.zoom.clone().filter(|z| z.path == p).unwrap_or(Zoom { path: p.clone(), scale: fit, cx: 0.5, cy: 0.5 });
        let centre = (body.x + body.w / 2, body.y + body.h / 2);
        match factor {
            None => {
                self.zoom = None;
                return true;
            }
            Some(0.0) => {
                let k = 1.0 / z.scale;
                zoom_about(body, nat, &mut z, k, centre)
            }
            Some(f) => zoom_about(body, nat, &mut z, f, anchor.unwrap_or(centre)),
        }
        self.zoom = if (z.scale - fit).abs() < 1e-3 { None } else { Some(z) };
        true
    }

    /// Pan the zoomed image by pixels.
    fn pan(&mut self, dx: i32, dy: i32) -> bool {
        let Some(nat) = self.nat.as_ref().map(|n| n.1) else { return false };
        let Some(z) = self.zoom.as_mut().filter(|z| matches!(&self.c.overlay, Some(Overlay::Preview(p)) if *p == z.path)) else { return false };
        z.cx += dx as f64 / (nat.0 as f64 * z.scale);
        z.cy += dy as f64 / (nat.1 as f64 * z.scale);
        zoom_rect(self.hits.image, nat, z);
        true
    }

    fn pointer(&mut self, id: &str, at: (i32, i32)) {
        self.pointer = at;
        let frac = |r: Rect| ((at.0 - r.x) as f64 / r.w.max(1) as f64).clamp(0.0, 1.0);
        match id {
            "pointer_down" => {
                self.press = None;
                if let Some(pl) = self.player.as_mut() {
                    if inside(self.hits.bar, at) {
                        pl.seek_to(frac(self.hits.bar) * pl.info.duration);
                        self.press = Some(Press::Scrub);
                    } else if inside(self.hits.vol, at) {
                        let v = (frac(self.hits.vol) * 100.0).round() as i32;
                        pl.change_volume(v - pl.volume as i32);
                        self.press = Some(Press::Volume);
                    } else if inside(self.hits.video, at) {
                        pl.toggle();
                    }
                } else if let Some(z) = &self.zoom
                    && inside(self.hits.image, at)
                {
                    self.press = Some(Press::Pan { at, c0: (z.cx, z.cy) });
                }
            }
            "pointer_move" => match self.press {
                Some(Press::Scrub) => {
                    let now = std::time::Instant::now();
                    if self.last_scrub.is_none_or(|t| now.duration_since(t).as_millis() >= 120)
                        && let Some(pl) = self.player.as_mut()
                    {
                        pl.seek_to(frac(self.hits.bar) * pl.info.duration);
                        self.last_scrub = Some(now);
                    }
                }
                Some(Press::Volume) => {
                    if let Some(pl) = self.player.as_mut() {
                        let v = (frac(self.hits.vol) * 100.0).round() as i32;
                        pl.change_volume(v - pl.volume as i32);
                    }
                }
                Some(Press::Pan { at: a0, c0 }) => {
                    if let (Some(z), Some(nat)) = (self.zoom.as_mut(), self.nat.as_ref().map(|n| n.1)) {
                        z.cx = c0.0 - (at.0 - a0.0) as f64 / (nat.0 as f64 * z.scale);
                        z.cy = c0.1 - (at.1 - a0.1) as f64 / (nat.1 as f64 * z.scale);
                        zoom_rect(self.hits.image, nat, z);
                    }
                }
                None => {}
            },
            _ => {
                if let (Some(Press::Scrub), Some(pl)) = (self.press, self.player.as_mut()) {
                    pl.seek_to(frac(self.hits.bar) * pl.info.duration);
                }
                self.press = None;
                self.last_scrub = None;
            }
        }
    }

    /// Video fitted to the width with the controls below; audio: name,
    /// progress and controls.
    fn player_view(&mut self, body: Rect, cw: i32, ch: i32, o: &mut Out) {
        let controls = self.player_keys.clone();
        let Some(pl) = self.player.as_mut() else { return };
        pl.tick();
        let row_h = ch + 6;
        let pos = pl.position();
        let dur = pl.info.duration;
        let mut y = body.y;
        let name = pl.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if pl.info.video {
            let r = Rect { x: body.x, y, w: body.w, h: (body.h - 3 * row_h - 4).max(ch) };
            self.hits.video = r;
            match pl.frame() {
                Some(f) => {
                    o.frames.push(ModeImage { key: f.key, rgba: f.rgba, width: f.w, height: f.h, src: (0, 0, f.w, f.h), rect: fit(r, f.w, f.h) })
                }
                None => o.items.push(item(Rect { x: r.x, y: r.y, w: r.w, h: ch }, "loading…", PanelStyle::Hint, UiAction::None)),
            }
            y = r.y + r.h;
        } else {
            self.hits.video = Rect::default();
            o.items.push(item(Rect { x: body.x, y, w: body.w, h: row_h }, format!("♪ {name}"), PanelStyle::Title, UiAction::None));
            y += row_h * 2;
        }
        if let Some(e) = &pl.error {
            o.items.push(item(Rect { x: body.x, y, w: body.w, h: row_h }, e.clone(), PanelStyle::Error, UiAction::None));
            y += row_h;
        }
        // Seek bar: time, bar (click seeks, drag scrubs, hover shows the time), length.
        let per = |t: f64| if dur > 0.0 { ((t / dur).clamp(0.0, 1.0) * 1000.0) as u16 } else { 0 };
        let (l, rt) = (super::player::clock(pos), super::player::clock(dur));
        let lw = (l.chars().count() as i32 + 1) * cw;
        let rw = (rt.chars().count() as i32 + 1) * cw;
        let bar = Rect { x: body.x + lw, y, w: (body.w - lw - rw).max(cw), h: row_h };
        let hover = if inside(bar, self.pointer) || matches!(self.press, Some(Press::Scrub)) {
            Some(((self.pointer.0 - bar.x) as f64 / bar.w as f64).clamp(0.0, 1.0) * dur)
        } else {
            None
        };
        o.items.push(item(Rect { x: body.x, w: lw, ..bar }, l, PanelStyle::Label, UiAction::None));
        o.items.push(item(bar, "", PanelStyle::Progress { done: per(pos), hover: hover.map_or(u16::MAX, per) }, UiAction::None));
        o.items.push(item(Rect { x: bar.x + bar.w + cw, w: rw, ..bar }, rt, PanelStyle::Label, UiAction::None));
        self.hits.bar = bar;
        y += row_h + 2;
        // Buttons, state (or the hovered time), volume.
        let mut x = body.x;
        let bh = row_h - 2;
        let play = if pl.playing() { "❚❚ pause" } else { "▶ play" };
        for (t, a) in [("« 10s", "seek_back"), (play, "play_pause"), ("10s »", "seek_forward")] {
            let bw = (t.chars().count() as i32 + 2) * cw;
            o.items.push(item(Rect { x, y, w: bw, h: bh }, t, PanelStyle::Button { primary: a == "play_pause" }, act(a, 0)));
            x += bw + cw / 2;
        }
        let status = match (hover, &pl.error) {
            (Some(t), _) => format!("→ {}", super::player::clock(t)),
            (None, _) => if pl.playing() { "playing" } else { "paused" }.to_string(),
        };
        let vw = (cw * 12).min(body.w / 5);
        let vlabel = format!("vol {}%", pl.volume);
        let vlw = (vlabel.chars().count() as i32 + 1) * cw;
        let vol = Rect { x: body.x + body.w - vw, y, w: vw, h: bh };
        o.items.push(item(Rect { x: x + cw, y, w: (vol.x - vlw - x - cw).max(0), h: bh }, status, PanelStyle::Label, UiAction::None));
        o.items.push(item(Rect { x: vol.x - vlw, y, w: vlw, h: bh }, vlabel, PanelStyle::Label, UiAction::None));
        o.items.push(item(vol, "", PanelStyle::Progress { done: pl.volume as u16 * 10, hover: u16::MAX }, UiAction::None));
        self.hits.vol = vol;
        y += row_h;
        o.items.push(item(Rect { x: body.x, y, w: body.w, h: row_h }, clip_text(&controls, body.w / cw.max(1)), PanelStyle::Hint, UiAction::None));
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

    fn remote_changed(&mut self, label: &str, dir: Option<&Path>, listing: Option<Result<Vec<Entry>, String>>) {
        self.c.set_remote(label.to_string(), dir.map(Path::to_path_buf), listing);
    }

    fn set_waker(&mut self, w: Waker) {
        self.waker = Some(w.clone());
        self.previews.set_waker(w);
    }

    fn text(&mut self, bytes: &[u8]) -> Option<Effect> {
        // Bound alt keys still work while searching.
        if bytes.len() > 1 && bytes[0] == 0x1b {
            return None;
        }
        if let Some(e) = self.c.pipe_key(bytes) {
            return Some(self.effect(e));
        }
        self.c.search_key(bytes).then_some(Effect::None)
    }

    fn wants_text(&self) -> bool {
        self.c.searching || self.c.piping
    }

    fn escape(&mut self) -> bool {
        if self.menu_open {
            self.menu_open = false;
            true
        } else if self.c.piping {
            self.c.pipe_key(b"\x1b");
            true
        } else if self.c.searching {
            self.c.search_key(b"\x1b");
            true
        } else {
            self.c.overlay.take().is_some()
        }
    }

    fn action(&mut self, id: &str, arg: i32) -> Effect {
        if matches!(id, "pointer_down" | "pointer_move" | "pointer_up") {
            self.pointer(id, unpack_xy(arg));
            return Effect::None;
        }
        if id == "menu" {
            if self.hits.menu.w == 0 && !self.menu_open {
                return Effect::Pass;
            }
            self.menu_open = !self.menu_open;
            return Effect::None;
        }
        self.menu_open = false;
        let viewing_image = matches!(&self.c.overlay, Some(Overlay::Preview(p)) if cmdr::kind_of(&p.to_string_lossy()) == Kind::Image);
        // The zoom and volume keys are shared by default: each view takes its own.
        let id = match id {
            "volume_up" if viewing_image => "zoom_in",
            "volume_down" if viewing_image => "zoom_out",
            "zoom_in" if self.player.is_some() => "volume_up",
            "zoom_out" if self.player.is_some() => "volume_down",
            i => i,
        };
        match id {
            "zoom_in" | "zoom_out" | "zoom_fit" | "zoom_actual" | "zoom_wheel" => {
                let f = match id {
                    "zoom_in" => Some(1.25),
                    "zoom_out" => Some(0.8),
                    "zoom_fit" => None,
                    "zoom_actual" => Some(0.0),
                    _ => Some(if arg > 0 { 1.15f64.powi(arg) } else { 0.87f64.powi(-arg) }),
                };
                let anchor = (id == "zoom_wheel").then_some(self.pointer);
                return if self.zoom_image(f, anchor) { Effect::None } else { Effect::Pass };
            }
            _ => {}
        }
        if self.zoom.is_some() && viewing_image {
            let step = (self.hits.image.h / 8).max(16);
            let (dx, dy) = match id {
                "left" => (-step, 0),
                "right" => (step, 0),
                "up" => (0, -step),
                "down" => (0, step),
                "scroll" => (0, arg * step / 2),
                _ => (0, 0),
            };
            if (dx, dy) != (0, 0) && self.pan(dx, dy) {
                return Effect::None;
            }
        }
        if id == "scroll"
            && let Some(p) = self.player.as_mut()
            && inside(self.hits.vol, self.pointer)
        {
            p.change_volume(-arg * 5);
            return Effect::None;
        }
        // Player keys belong to the player; without one they go to the shell.
        if matches!(id, "play_pause" | "seek_back" | "seek_forward" | "volume_down" | "volume_up") {
            let Some(p) = self.player.as_mut() else { return Effect::Pass };
            match id {
                "play_pause" => p.toggle(),
                "seek_back" => p.seek(-10.0),
                "seek_forward" => p.seek(10.0),
                "volume_down" => p.change_volume(-10),
                _ => p.change_volume(10),
            }
            return Effect::None;
        }
        let n = self.c.visible().len();
        let previewing = matches!(self.c.overlay, Some(Overlay::Preview(_)));
        let reading = previewing || self.c.overlay == Some(Overlay::Man);
        match id {
            "item" => {
                let i = (arg.max(0) as usize).min(n.saturating_sub(1));
                // A second click on the same item within 400 ms opens it.
                let now = std::time::Instant::now();
                let double = self.last_click.is_some_and(|(j, t)| j == i && now.duration_since(t) < std::time::Duration::from_millis(400));
                self.c.selected = i;
                if double {
                    self.last_click = None;
                    let e = self.c.open_selected();
                    return self.effect(e);
                }
                self.last_click = Some((i, now));
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
            "up_to" => {
                let mut d = self.c.cwd.clone();
                for _ in 0..arg.max(0) {
                    d = d.and_then(|d| d.parent().map(Path::to_path_buf));
                }
                return d.map_or(Effect::None, |d| Effect::Run(cd_line(&d)));
            }
            // In the file view (and the player) back closes the view first.
            "parent" | "back" if previewing => {
                self.c.overlay = None;
                return Effect::None;
            }
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
                if reading && matches!(a, Action::Up | Action::Down | Action::PageUp | Action::PageDown) {
                    let d = match a {
                        Action::Up => -1,
                        Action::Down => 1,
                        Action::PageUp => -20,
                        _ => 20,
                    };
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
                self.c.relist();
                return self.effect(e);
            }
        }
        // Flags changed: re-list.
        self.c.relist();
        Effect::None
    }

    fn view(&mut self, ctx: &Context, keys: &Keymap) -> View {
        let vp = ctx.viewport;
        let (cw, ch) = (vp.cell_w.max(1), vp.cell_h.max(1));
        let a = ctx.area;
        let max_h = (a.h - ch * 8).max(ch * 4);
        let h = (a.h / 2).max(ch * 10).min(max_h);
        let rect = Rect { x: a.x + PADDING, y: a.y, w: a.w - 2 * PADDING, h };
        // Playback stops when the viewer closes or shows another file.
        if self.player.as_ref().is_some_and(|pl| !matches!(&self.c.overlay, Some(Overlay::Preview(p)) if *p == pl.path)) {
            self.player = None;
        }
        let mut o = Out { items: Vec::new(), frames: Vec::new(), slots: Vec::new(), texts: Vec::new(), extra: Vec::new(), menu: None };
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
        // Narrow: the path shortens first (down to ~16 columns), then the
        // buttons that still do not fit go, one by one, into a ••• menu.
        let widths: Vec<i32> = buttons.iter().map(|(t, _)| (t.chars().count() as i32 + 2) * cw + cw / 2).collect();
        let menu_w = 5 * cw + cw / 2;
        let back_min = 8 * cw;
        let fits = header_fit(&widths, w - back_min - 16 * cw, menu_w);
        let mut rest = buttons;
        let hidden = rest.split_off(fits);
        self.hits.menu = Rect::default();
        if !hidden.is_empty() {
            bx -= menu_w - cw / 2;
            let mr = Rect { x: bx, y: y0, w: menu_w - cw / 2, h: row_h - 2 };
            self.hits.menu = mr;
            o.items.push(item(mr, "•••", PanelStyle::Button { primary: self.menu_open }, act("menu", 0)));
            bx -= cw / 2;
            if self.menu_open {
                let mw = hidden.iter().map(|(t, _)| t.chars().count() as i32 + 4).max().unwrap_or(8) * cw;
                let r = Rect { x: (mr.x + mr.w - mw).max(rect.x + 4), y: mr.y + mr.h + 4, w: mw, h: hidden.len() as i32 * row_h + 8 };
                o.menu = Some(r);
                o.items.push(item(r, "", PanelStyle::Popup, UiAction::None));
                for (i, (t, a)) in hidden.into_iter().enumerate() {
                    let rr = Rect { x: r.x + 4, y: r.y + 4 + i as i32 * row_h, w: r.w - 8, h: row_h };
                    o.items.push(item(rr, t, PanelStyle::Option { selected: false, highlight: 0 }, a));
                }
            }
        } else {
            self.menu_open = false;
        }
        for (t, a) in rest {
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
        // Back control: goes to the parent folder (or closes the file view);
        // disabled at /.
        let can_back = file.is_some() || self.c.cwd.as_ref().is_some_and(|d| d.parent().is_some());
        let back =
            if (x0 + w - bx) / cw.max(1) + 40 < w / cw.max(1) { format!("‹ back ({})", keys.label("parent")) } else { "‹ back".to_string() };
        let back_w = (back.chars().count() as i32 + 2) * cw;
        o.items.push(item(
            Rect { x: x0, y: y0, w: back_w, h: row_h - 2 },
            back,
            if can_back { PanelStyle::Button { primary: false } } else { PanelStyle::Hint },
            if can_back { act("parent", 0) } else { UiAction::None },
        ));
        let mut tx = x0 + back_w + cw;
        let room = |tx: i32| ((bx - tx) / cw - 1).max(0);
        match (&file, &self.c.cwd) {
            (Some(p), _) => {
                let t = match self.zoom_label(p) {
                    Some(z) => format!("{z} · {}", show(p)),
                    None => show(p),
                };
                let t = clip_text(&t, room(tx));
                o.items.push(item(Rect { x: tx, y: y0, w: (bx - tx).max(0), h: row_h }, t, PanelStyle::Title, UiAction::None));
            }
            (None, Some(d)) => {
                if let Some(r) = &self.c.remote {
                    let t = format!("{r}:");
                    let n = (t.chars().count() as i32).min(room(tx));
                    o.items.push(item(Rect { x: tx, y: y0, w: n * cw, h: row_h }, clip_text(&t, n), PanelStyle::Title, UiAction::None));
                    tx += n * cw;
                }
                // Path segments: a click goes to that folder. Too long: the
                // leading ones give way to "…".
                let shown = if self.c.remote.is_some() { d.display().to_string() } else { show(d) };
                let mut segs = path_segments(&shown);
                let width = |v: &[(String, usize)]| v.iter().map(|s| s.0.chars().count() as i32).sum::<i32>();
                if width(&segs) > room(tx) {
                    while segs.len() > 1 && width(&segs) + 1 > room(tx) {
                        segs.remove(0);
                    }
                    o.items.push(item(Rect { x: tx, y: y0, w: cw, h: row_h }, "…", PanelStyle::Hint, UiAction::None));
                    tx += cw;
                }
                for (label, depth) in segs {
                    let n = (label.chars().count() as i32).min(room(tx));
                    let label = clip_text(&label, n);
                    let up = depth;
                    let a = if up > 0 { act("up_to", up as i32) } else { UiAction::None };
                    o.items.push(item(Rect { x: tx, y: y0, w: n * cw, h: row_h }, label, PanelStyle::Title, a));
                    tx += n * cw;
                }
                let marks = if self.c.marks.is_empty() { String::new() } else { format!(" · {} marked", self.c.marks.len()) };
                let rest = format!("   {} items{marks} · ls {}", self.c.visible().len(), self.c.flags.ls());
                o.items.push(item(
                    Rect { x: tx, y: y0, w: (bx - tx).max(0), h: row_h },
                    clip_text(&rest, room(tx)),
                    PanelStyle::Hint,
                    UiAction::None,
                ));
            }
            (None, None) => {
                let t = self.c.remote.as_ref().map_or("commander".to_string(), |r| format!("{r}: (remote, folder unknown)"));
                o.items.push(item(Rect { x: tx, y: y0, w: (bx - tx).max(0), h: row_h }, clip_text(&t, room(tx)), PanelStyle::Title, UiAction::None))
            }
        }
        let mut body = Rect { x: x0, y: y0 + row_h + 6, w, h: rect.y + h - (y0 + row_h + 6) - 8 };
        self.body_w = body.w;
        self.player_keys = format!(
            "{} play/pause · {} / {} seek 10s · {} / {} volume · {} close",
            keys.label("play_pause"),
            keys.label("seek_back"),
            keys.label("seek_forward"),
            keys.label("volume_down"),
            keys.label("volume_up"),
            keys.label("close")
        );
        if let Some(p) = file {
            self.file_view(&p, body, cw, ch, &mut o);
        } else {
            // Search field, or the pipe prompt.
            let fw = (cw * 40).min(w);
            if self.c.piping {
                let n = self.c.pipe_paths().len();
                let fr = Rect { x: x0, y: body.y, w: (cw * 60).min(w), h: row_h };
                o.items.push(item(fr, format!("| {}", self.c.pipe_cmd), PanelStyle::Field { focused: true, valid: true }, UiAction::None));
                o.items.push(item(
                    Rect { x: fr.x + fr.w + cw, y: fr.y, w: (w - fr.w - cw).max(0), h: row_h },
                    format!("pipe {n} path(s) · Enter runs · Esc cancels"),
                    PanelStyle::Hint,
                    UiAction::None,
                ));
            } else {
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
                // Feedback: what is filtered, how many match, and that typing goes here.
                let n = self.c.visible().len();
                let fb = match (self.c.searching, self.c.filter.is_empty()) {
                    (true, true) => "searching · type to filter · Enter keeps · Esc cancels".to_string(),
                    (s, false) => {
                        let m = if n == 0 {
                            format!("no matches for \"{}\"", self.c.filter)
                        } else {
                            format!("{n} match{} for \"{}\"", if n == 1 { "" } else { "es" }, self.c.filter)
                        };
                        if s { format!("{m} · Enter keeps · Esc clears") } else { format!("{m} · {} to edit", keys.label("search")) }
                    }
                    (false, true) => self.c.notice.clone().unwrap_or_default(),
                };
                if !fb.is_empty() {
                    o.items.push(item(
                        Rect { x: fr.x + fr.w + cw, y: fr.y, w: (w - fr.w - cw).max(0), h: row_h },
                        clip_text(&fb, (w - fr.w - cw) / cw),
                        if n == 0 { PanelStyle::Error } else { PanelStyle::Hint },
                        UiAction::None,
                    ));
                }
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
        self.previews.want(o.slots.iter().map(|s| s.0.clone()).chain(o.extra.iter().cloned()).collect());
        self.texts.retain(|p, _| o.texts.contains(p));
        let mut images = Vec::new();
        for (want, full, clip) in &o.slots {
            match self.previews.get(want) {
                Some(Some(t)) => {
                    let r = fit(*full, t.w, t.h);
                    images.push((ModeImage { key: t.key, rgba: t.rgba.clone(), width: t.w, height: t.h, src: (0, 0, t.w, t.h), rect: r }, *clip));
                }
                // Loading or no preview: the item's kind, centred and dim
                // (a video without ffmpeg stays "▶ video").
                _ => {
                    let label = kind_label(cmdr::kind_of(&want.0.to_string_lossy()));
                    let lw = (label.chars().count() as i32 * cw).min(clip.w);
                    let r = Rect { x: clip.x + (clip.w - lw).max(0) / 2, y: clip.y + (clip.h - ch).max(0) / 2, w: lw, h: ch };
                    o.items.push(item(r, label, PanelStyle::Hint, UiAction::None));
                }
            }
        }
        let panel = match self.c.overlay {
            Some(Overlay::Options) => Some(self.options_panel(ctx)),
            Some(Overlay::Man) => Some(self.man_panel(ctx, keys)),
            _ => None,
        };
        // Images are drawn as a layer above text; under a popup window they
        // would show through it, so they are left out while one is open.
        if let Some(p) = &panel {
            images.retain(|(i, _)| !overlaps(i.rect, p.rect));
        }
        let menu = o.menu;
        // Never outside the view (e.g. over the prompt below it), and clear
        // of its rounded corners.
        let inner = Rect { x: rect.x + crate::frame::RADIUS, w: rect.w - 2 * crate::frame::RADIUS, ..rect };
        let mut images: Vec<(ModeImage, Rect)> = images.into_iter().map(|(i, c)| (i, intersect(c, inner))).collect();
        let mut frames = o.frames;
        if let Some(m) = menu {
            images.retain(|(i, c)| !overlaps(intersect(i.rect, *c), m));
            frames.retain(|f| !overlaps(f.rect, m));
        }
        View {
            dock: Some(Panel { rect, items: o.items }),
            panel,
            images: images.into_iter().chain(frames.into_iter().map(|f| (f, inner))).filter_map(|(i, clip)| crop(i, clip)).collect(),
        }
    }
}

impl CommanderMode {
    fn effect(&mut self, e: cmdr::Effect) -> Effect {
        match e {
            cmdr::Effect::None => Effect::None,
            cmdr::Effect::Cd(p) => Effect::Run(cd_line(&p)),
            // The in-app player; the desktop opener only without ffmpeg.
            cmdr::Effect::External(p) if super::player::available() => {
                self.player = Some(super::player::Player::open(&p, self.body_w.max(320) as u32, self.waker.clone()));
                self.c.overlay = Some(Overlay::Preview(p));
                self.c.overlay_scroll = 0;
                Effect::None
            }
            cmdr::Effect::External(p) => Effect::Open(p),
            cmdr::Effect::Run(s) => Effect::Run(s),
        }
    }
}

/// The header's path split into clickable pieces: (text, how many levels up
/// it is). `~/a/b` gives `~`, `/a`, `/b` with 2, 1, 0; `/x` gives `/`, `x`.
pub fn path_segments(shown: &str) -> Vec<(String, usize)> {
    let parts: Vec<&str> = shown.split('/').collect();
    let mut out: Vec<String> = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        match (i, p.is_empty()) {
            (0, true) => out.push("/".into()),
            (_, true) => {}
            (0, false) => out.push(p.to_string()),
            _ if out.last().is_some_and(|l| l == "/") && out.len() == 1 => out.push(p.to_string()),
            _ => out.push(format!("/{p}")),
        }
    }
    let n = out.len();
    out.into_iter().enumerate().map(|(i, t)| (t, n - 1 - i)).collect()
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let (l, t) = (a.x.max(b.x), a.y.max(b.y));
    let (r, bt) = ((a.x + a.w).min(b.x + b.w), (a.y + a.h).min(b.y + b.h));
    Rect { x: l, y: t, w: (r - l).max(0), h: (bt - t).max(0) }
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
        for _ in 0..200 {
            wait_loaded(&mut m, 3);
            if m.previews().decoded().iter().any(|w| w.0.ends_with("p5.png")) {
                break;
            }
        }
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
        m.last_click = None;
        assert_eq!(m.action("item", 1), Effect::None, "a slow second click only selects");
        let cd = m.action("item", 1);
        assert_eq!(cd, Effect::Run(format!("cd -- '{}'", t.path().join("sub dir").display())));
        m.action("switch_view", 0);
        let v = m.view(&ctx(), &k);
        assert!(v.dock.unwrap().items.iter().filter(|i| matches!(i.style, PanelStyle::Tile { .. })).count() == 2);
        assert!(m.text(b"x").is_none(), "not searching: typing goes to the shell");
        m.action("search", 0);
        assert_eq!(m.text(b"s"), Some(Effect::None));
        assert_eq!(m.c.filter, "s");
        assert!(m.text(b"\x1bt").is_none(), "bound keys still work while searching");
    }

    #[test]
    fn pages_marks_and_pipe_in_every_view() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..60 {
            std::fs::write(t.path().join(format!("f{i:02}.txt")), "x").unwrap();
        }
        let k = Keymap::new(cmdr::BINDINGS);
        for view in 0..3 {
            let mut m = CommanderMode::new();
            m.c.flags.all = false;
            m.cwd_changed(t.path());
            m.action("view", view);
            m.view(&ctx(), &k);
            let page = m.c.page;
            assert!(page >= 1);
            m.action("page_down", 0);
            assert_eq!(m.c.selected, page.min(59), "view {view}");
            m.action("last", 0);
            assert_eq!(m.c.selected, 59);
            let v = m.view(&ctx(), &k);
            let sel =
                v.dock.unwrap().items.iter().any(|i| {
                    i.action == act("item", 59) && matches!(i.style, PanelStyle::Row { selected: true } | PanelStyle::Tile { selected: true })
                });
            assert!(sel, "view {view}: the selection scrolled into sight");
            m.action("first", 0);
            m.action("mark", 0);
            // Mark moves on in list and carousel; go back so f00 is on screen.
            m.action("first", 0);
            let v = m.view(&ctx(), &k);
            assert!(v.dock.unwrap().items.iter().any(|i| i.text.contains("* f00.txt") || i.text.starts_with("*-")), "view {view}: marks shown");
            m.action("pipe", 0);
            assert!(m.wants_text());
            for b in [b"c", b"a", b"t"] {
                m.text(b);
            }
            let Some(Effect::Run(line)) = m.text(b"\r") else { panic!() };
            assert!(line.contains("f00.txt") && line.ends_with("| cat"), "{line}");
        }
    }

    #[test]
    fn media_opens_the_player_and_closing_stops_it() {
        let k = Keymap::new(cmdr::BINDINGS);
        let mut m = CommanderMode::new();
        assert_eq!(m.action("play_pause", 0), Effect::Pass, "no player: alt+k goes to the shell");
        if !super::super::player::available() {
            eprintln!("ffmpeg not installed, skipped");
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let ok = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-f", "lavfi", "-i", "testsrc=size=160x90:rate=10:duration=4", "-pix_fmt", "yuv420p"])
            .arg(t.path().join("v.mp4"))
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return;
        }
        m.c.flags.all = false;
        m.cwd_changed(t.path());
        m.view(&ctx(), &k);
        m.action("open", 0);
        assert!(m.player.is_some(), "the in-app player, not the desktop opener");
        let mut got = false;
        for _ in 0..150 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let v = m.view(&ctx(), &k);
            if !v.images.is_empty() {
                assert!(v.dock.unwrap().items.iter().any(|i| i.text.contains("alt+k play/pause")));
                got = true;
                break;
            }
        }
        assert!(got, "a frame is shown");
        assert_eq!(m.action("play_pause", 0), Effect::None);
        assert!(!m.player.as_ref().unwrap().playing());
        assert!(m.escape());
        m.view(&ctx(), &k);
        assert!(m.player.is_none(), "closing the viewer stops playback");
    }

    // POSIX paths in the header (Windows paths are one segment).
    #[cfg(unix)]
    #[test]
    fn back_control_segments_and_no_dot_entries() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("a/b")).unwrap();
        let k = Keymap::new(cmdr::BINDINGS);
        let mut m = CommanderMode::new();
        m.cwd_changed(&t.path().join("a/b"));
        let d = m.view(&ctx(), &k).dock.unwrap();
        let back = d.items.iter().find(|i| i.text.starts_with("‹ back")).expect("back control");
        assert_eq!(back.text, "‹ back (alt+p)");
        assert_eq!(back.action, act("parent", 0));
        assert!(!d.items.iter().any(|i| i.text.contains(" ..") || i.text.ends_with(" .")), "no . or ..");
        // The parent's segment jumps one level up; the current one is not a link.
        assert!(d.items.iter().any(|i| i.text == "/a" && i.action == act("up_to", 1)));
        assert!(d.items.iter().any(|i| i.text == "/b" && i.action == UiAction::None));
        assert_eq!(m.action("up_to", 2), Effect::Run(cd_line(t.path())));
        assert_eq!(m.action("parent", 0), Effect::Run(cd_line(&t.path().join("a"))));
        // Coming back up selects the folder we left.
        m.cwd_changed(&t.path().join("a"));
        assert_eq!(m.c.selected_entry().unwrap().name, "b");
        // At / the control is disabled.
        m.cwd_changed(Path::new("/"));
        let d = m.view(&ctx(), &k).dock.unwrap();
        let back = d.items.iter().find(|i| i.text.starts_with("‹ back")).unwrap();
        assert_eq!((back.style, back.action.clone()), (PanelStyle::Hint, UiAction::None));
        assert_eq!(path_segments("~/x/y"), [("~".to_string(), 2), ("/x".into(), 1), ("/y".into(), 0)]);
        assert_eq!(path_segments("/"), [("/".to_string(), 0)]);
        assert_eq!(path_segments("/usr/lib"), [("/".to_string(), 2), ("usr".into(), 1), ("/lib".into(), 0)]);
    }

    #[test]
    fn search_shows_query_and_count() {
        let t = tempfile::tempdir().unwrap();
        for n in ["apple.txt", "apricot.txt", "banana.txt"] {
            std::fs::write(t.path().join(n), "x").unwrap();
        }
        let k = Keymap::new(cmdr::BINDINGS);
        let mut m = CommanderMode::new();
        m.cwd_changed(t.path());
        let texts = |m: &mut CommanderMode| m.view(&ctx(), &k).dock.unwrap().items.into_iter().map(|i| i.text).collect::<Vec<_>>();
        assert!(!texts(&mut m).iter().any(|t| t.contains("searching")), "not focused until invoked");
        m.action("search", 0);
        assert!(texts(&mut m).iter().any(|t| t.starts_with("searching")));
        m.text(b"a");
        m.text(b"p");
        assert!(texts(&mut m).iter().any(|t| t.starts_with("2 matches for \"ap\"")), "{:?}", texts(&mut m));
        m.text(b"z");
        assert!(texts(&mut m).iter().any(|t| t.starts_with("no matches for \"apz\"")));
    }

    #[test]
    fn popups_hide_images_beneath_and_images_stay_in_the_view() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..6 {
            png(&t.path().join(format!("p{i}.png")));
        }
        let mut m = CommanderMode::new();
        m.cwd_changed(t.path());
        m.action("view", 1);
        let v = wait_loaded(&mut m, 6);
        let dock = v.dock.unwrap().rect;
        assert!(!v.images.is_empty());
        assert!(v.images.iter().all(|i| i.rect.y >= dock.y && i.rect.y + i.rect.h <= dock.y + dock.h), "never over the prompt");
        m.action("options", 0);
        let v = m.view(&ctx(), &Keymap::new(cmdr::BINDINGS));
        let p = v.panel.unwrap().rect;
        assert!(v.images.iter().all(|i| !overlaps(i.rect, p)), "nothing shows through the options window");
    }

    #[test]
    fn video_tiles_get_a_cached_thumbnail() {
        let t = tempfile::tempdir().unwrap();
        // SAFETY: only this test reads MOUSER_HOME in this binary.
        unsafe { std::env::set_var("MOUSER_HOME", t.path().join("home")) };
        let media = t.path().join("m");
        std::fs::create_dir(&media).unwrap();
        let v = media.join("clip.mp4");
        let ok = super::super::player::available()
            && std::process::Command::new("ffmpeg")
                .args(["-loglevel", "error", "-f", "lavfi", "-i", "testsrc=size=320x180:rate=10:duration=3", "-pix_fmt", "yuv420p"])
                .arg(&v)
                .status()
                .is_ok_and(|s| s.success());
        let k = Keymap::new(cmdr::BINDINGS);
        let mut m = CommanderMode::new();
        if !ok {
            // Without ffmpeg the tile says what it is.
            std::fs::write(&v, b"not really").unwrap();
            m.cwd_changed(&media);
            m.action("view", 1);
            let _ = wait_loaded(&mut m, 1);
            assert!(m.view(&ctx(), &k).dock.unwrap().items.iter().any(|i| i.text == "▶ video"));
            return;
        }
        m.cwd_changed(&media);
        m.action("view", 1);
        let view = wait_loaded(&mut m, 1);
        assert_eq!(view.images.len(), 1, "the video tile has a picture");
        let (w, want) = (view.images[0].width, m.previews().decoded()[0].clone());
        assert!(w <= want.1 && w > 0);
        assert!(thumb_cache_path(&v, want.1).unwrap().exists(), "cached on disk by path+mtime+size");
        assert!(thumb_cache_path(&v, want.1).unwrap().starts_with(t.path().join("home/cache/thumbs")));
        // Carousel also shows it.
        m.action("view", 2);
        assert_eq!(wait_loaded(&mut m, 1).images.len(), 1);
    }

    #[test]
    fn plain_keys_reach_the_shell_with_the_viewer_or_player_open() {
        let t = tempfile::tempdir().unwrap();
        png(&t.path().join("a.png"));
        std::fs::write(t.path().join("b.mp4"), b"x").unwrap();
        let mut h = super::super::Host::new(vec![Box::new(CommanderMode::new())]);
        h.set_cwd(Some(t.path().to_path_buf()));
        // Viewer: open a.png (alt+Enter), then type.
        assert!(h.key(b"\x1b\r").is_some());
        for open in ["viewer", "player"] {
            let mut pty = Vec::new();
            for b in b"cd x" {
                let k = [*b];
                if !h.takes(&k) && h.key(&k).is_none() {
                    pty.push(*b);
                }
            }
            assert_eq!(pty, b"cd x", "{open}: every plain key goes to the shell, none dropped");
            // Next: the player (or its fallback view) for b.mp4.
            h.escape();
            h.key(b"\x1b[1;3B");
            h.key(b"\x1b\r");
        }
    }

    #[test]
    fn carousel_slide_eases_and_retargets_smoothly() {
        use std::time::{Duration, Instant};
        assert_eq!((Slide::ease(0.0), Slide::ease(1.0)), (0.0, 1.0));
        assert!(Slide::ease(0.1) < 0.1 && Slide::ease(0.9) > 0.9, "slow at both ends");
        let t0 = Instant::now();
        let mut s = Slide::default();
        assert_eq!(s.at(3.0, t0), 3.0, "first frame: no slide");
        assert!(!s.moving());
        assert_eq!(s.at(4.0, t0), 3.0, "starts where it was");
        let mid = s.at(4.0, t0 + Duration::from_millis(90));
        assert!((mid - 3.5).abs() < 0.01, "{mid}");
        // A second key mid-slide: continues from the current position.
        let again = s.at(5.0, t0 + Duration::from_millis(90));
        assert!((again - mid).abs() < 1e-9, "no jump on retarget");
        assert_eq!(s.at(5.0, t0 + Duration::from_millis(90) + Slide::DUR), 5.0);
        assert!(!s.moving(), "stops: no more redraws");
        // Far jumps only slide the last item.
        s.at(40.0, t0 + Duration::from_secs(1));
        assert!((s.at(40.0, t0 + Duration::from_secs(1)) - 39.0).abs() < 1e-9);
    }

    #[test]
    fn carousel_slide_loads_the_incoming_item_too() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..8 {
            png(&t.path().join(format!("p{i}.png")));
        }
        let mut m = CommanderMode::new();
        m.c.flags.all = false;
        m.cwd_changed(t.path());
        m.action("view", 2);
        m.c.selected = 3;
        wait_loaded(&mut m, 3);
        m.action("right", 0);
        let k = Keymap::new(cmdr::BINDINGS);
        let v = m.view(&ctx(), &k);
        assert!(m.slide.moving());
        let _ = v;
        std::thread::sleep(Slide::DUR / 2);
        let v = m.view(&ctx(), &k);
        let tiles = v.dock.unwrap().items.iter().filter(|i| matches!(i.style, PanelStyle::Tile { .. })).count();
        assert!(tiles >= 2, "mid-slide: two halves on the strip");
        assert!(m.previews.s.0.lock().unwrap().want.iter().any(|w| w.0.ends_with("p5.png")), "the incoming one is wanted");
        assert!(!m.previews.s.0.lock().unwrap().want.iter().any(|w| w.0.ends_with("p6.png")));
        std::thread::sleep(Slide::DUR + std::time::Duration::from_millis(20));
        let v = m.view(&ctx(), &k);
        assert!(!m.slide.moving());
        assert_eq!(v.dock.unwrap().items.iter().filter(|i| matches!(i.style, PanelStyle::Tile { .. })).count(), 3);
    }

    #[test]
    fn remote_header_badges_and_unknown_state() {
        let k = Keymap::new(cmdr::BINDINGS);
        let mut m = CommanderMode::new();
        let ent = |n: &str, dir: bool| Entry {
            name: n.into(),
            path: PathBuf::from(format!("/srv/{n}")),
            kind: if dir { Kind::Dir } else { cmdr::kind_of(n) },
            mode: "-rw-r--r--".into(),
            size: 3,
            modified: None,
        };
        m.remote_changed("me@web1", None, None);
        let d = m.view(&ctx(), &k).dock.unwrap();
        assert!(d.items.iter().any(|i| i.text.contains("no directory info")), "clear unknown state");
        assert!(d.items.iter().any(|i| i.text.starts_with("me@web1:")));
        m.remote_changed("me@web1", Some(Path::new("/srv")), Some(Ok(vec![ent("a.png", false), ent("app", true)])));
        m.action("view", 1);
        let d = m.view(&ctx(), &k).dock.unwrap();
        assert!(d.items.iter().any(|i| i.text == "me@web1:"));
        assert!(d.items.iter().any(|i| i.text == "/srv" || i.text == "/"), "{:?}", d.items.iter().map(|i| &i.text).collect::<Vec<_>>());
        assert!(d.items.iter().any(|i| i.text == "image · remote"), "remote badge, nothing fetched");
        assert!(m.previews().decoded().is_empty());
        m.c.selected = 1;
        assert_eq!(m.action("open", 0), Effect::Run("cd -- '/srv/app'".into()), "cd over the PTY");
    }

    #[test]
    fn image_zoom_keys_wheel_pan_and_header_percent() {
        let t = tempfile::tempdir().unwrap();
        image::RgbaImage::from_pixel(400, 200, image::Rgba([0, 0, 255, 255])).save(t.path().join("big.png")).unwrap();
        let k = Keymap::new(cmdr::BINDINGS);
        let mut m = CommanderMode::new();
        m.cwd_changed(t.path());
        m.c.overlay = Some(Overlay::Preview(t.path().join("big.png")));
        let v = wait_loaded(&mut m, 1);
        let title = |v: &View| v.dock.as_ref().unwrap().items.iter().find(|i| i.text.contains("big.png")).unwrap().text.clone();
        assert!(title(&v).contains("fit "), "{}", title(&v));
        // volume keys zoom in the image view (shared keys), alt+1 = 100%.
        assert_eq!(m.action("volume_up", 0), Effect::None);
        assert!(m.zoom.is_some());
        assert_eq!(m.action("zoom_actual", 0), Effect::None);
        let v = m.view(&ctx(), &k);
        assert!(title(&v).starts_with("100% · "), "{}", title(&v));
        for _ in 0..8 {
            m.action("zoom_in", 0);
        }
        let v = m.view(&ctx(), &k);
        let body = m.hits.image;
        // Clipped to the view: never larger than the body.
        for i in &v.images {
            assert!(i.rect.x >= body.x - 1 && i.rect.y >= body.y - 1 && i.rect.y + i.rect.h <= body.y + body.h + 1, "{:?} {:?}", i.rect, body);
        }
        // Pan with arrows and drag moves the centre.
        let c0 = m.zoom.as_ref().unwrap().cx;
        m.action("right", 0);
        assert!(m.zoom.as_ref().unwrap().cx > c0);
        let mid = (body.x + body.w / 2, body.y + body.h / 2);
        m.action("pointer_down", pack_xy(mid.0, mid.1));
        m.action("pointer_move", pack_xy(mid.0 + 50, mid.1));
        m.action("pointer_up", pack_xy(mid.0 + 50, mid.1));
        assert!(m.zoom.as_ref().unwrap().cx < c0 + 0.2);
        // Ctrl+wheel zooms about the pointer: the image point under it stays put.
        m.action("zoom_fit", 0);
        assert!(m.zoom.is_none());
        let nat = (400, 200);
        let mut z = Zoom { path: "x".into(), scale: fit_scale(body, nat), cx: 0.5, cy: 0.5 };
        let a = (body.x + body.w / 4, body.y + body.h / 2);
        let r0 = zoom_rect(body, nat, &mut z);
        let u0 = (a.0 - r0.x) as f64 / r0.w as f64;
        zoom_about(body, nat, &mut z, 2.0, a);
        let r1 = zoom_rect(body, nat, &mut z);
        assert!((((a.0 - r1.x) as f64 / r1.w as f64) - u0).abs() < 0.01);
        // Without an image view the zoom keys go to the shell.
        m.c.overlay = None;
        assert_eq!(m.action("zoom_fit", 0), Effect::Pass);
    }

    #[test]
    fn narrow_header_collapses_buttons_into_menu() {
        assert_eq!(header_fit(&[50, 50, 50], 200, 30), 3);
        assert_eq!(header_fit(&[50, 50, 50], 140, 30), 2, "progressive: only what does not fit");
        assert_eq!(header_fit(&[50, 50, 50], 40, 30), 0);
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("a/b")).unwrap();
        let k = Keymap::new(cmdr::BINDINGS);
        let mut m = CommanderMode::new();
        m.cwd_changed(&t.path().join("a/b"));
        let wide = m.view(&ctx(), &k).dock.unwrap();
        assert!(!wide.items.iter().any(|i| i.text == "•••"));
        assert_eq!(m.action("menu", 0), Effect::Pass, "no menu when everything fits");
        let narrow = Context { viewport: UiViewport { width: 420, height: 600, cell_w: 9, cell_h: 18 }, area: Rect { x: 0, y: 8, w: 420, h: 592 } };
        let d = m.view(&narrow, &k).dock.unwrap();
        let dots = d.items.iter().find(|i| i.text == "•••").expect("overflow button");
        assert!(dots.rect.x + dots.rect.w > 360, "upper right");
        assert!(d.items.iter().any(|i| i.text.starts_with("‹ back")), "back stays");
        assert!(d.items.iter().any(|i| i.text == "help"), "help still fits");
        assert!(!d.items.iter().any(|i| i.text == "options"));
        assert_eq!(m.action("menu", 0), Effect::None);
        let d = m.view(&narrow, &k).dock.unwrap();
        let opt = d.items.iter().find(|i| i.text == "options").expect("in the dropdown");
        assert!(matches!(opt.style, PanelStyle::Option { .. }));
        assert!(d.items.iter().any(|i| i.style == PanelStyle::Popup));
        assert_eq!(m.action("options", 0), Effect::None);
        assert!(!m.menu_open, "choosing closes it");
        m.action("menu", 0);
        assert!(m.escape() && !m.menu_open);
    }

    #[test]
    fn player_bar_and_volume_hits() {
        let mut m = CommanderMode::new();
        m.hits.vol = Rect { x: 100, y: 10, w: 100, h: 20 };
        m.pointer = (150, 15);
        // No player: pointer events are harmless.
        m.action("pointer_down", pack_xy(150, 15));
        m.action("pointer_up", pack_xy(150, 15));
        assert!(m.press.is_none());
        assert_eq!(unpack_xy(pack_xy(1234, 987)), (1234, 987));
    }
}
