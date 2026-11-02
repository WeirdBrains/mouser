//! Font loading, monospace cell metrics and glyph drawing for the CPU renderer.

use fontdue::{Font, FontSettings, Metrics};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Built-in fallback face (Liberation Mono, SIL OFL 1.1) so Mouser always has a
/// real monospace font, even on systems without one.
pub const EMBEDDED_MONO: &[u8] = include_bytes!("../assets/fonts/LiberationMono-Regular.ttf");

/// Preferred monospace families, in order, when no family is configured.
const PREFERRED_MONO: &[&str] = &[
    "JetBrainsMono Nerd Font",
    "JetBrains Mono",
    "Fira Code",
    "Cascadia Mono",
    "DejaVu Sans Mono",
    "Noto Sans Mono",
    "Ubuntu Mono",
    "Hack",
    "Source Code Pro",
    "Menlo",
    "SF Mono",
    "Consolas",
    "Liberation Mono",
    "Adwaita Mono",
];

/// Faces consulted for characters the primary font lacks (symbols, Nerd Font icons, CJK).
const FALLBACK_FAMILIES: &[&str] = &[
    "Symbols Nerd Font Mono",
    "Symbols Nerd Font",
    "JetBrainsMono Nerd Font",
    "Noto Sans Symbols 2",
    "Noto Sans Symbols",
    "DejaVu Sans",
    "Noto Sans Mono CJK SC",
    "Noto Sans CJK SC",
    "Segoe UI Symbol",
    "Apple Symbols",
];

#[derive(Debug, Clone)]
pub struct Glyph {
    pub metrics: Metrics,
    pub bitmap: Vec<u8>,
}

/// A primary monospace font plus fallbacks, rasterised at one pixel size.
///
/// Every character occupies one `cell_w` x `cell_h` cell; glyphs are placed on a
/// shared baseline `ascent` pixels below the top of the cell.
pub struct FontStack {
    fonts: Vec<Arc<Font>>,
    pub size: f32,
    pub cell_w: i32,
    pub cell_h: i32,
    pub ascent: i32,
    cache: HashMap<char, Glyph>,
    /// Colour emoji face (see [`EMOJI_FAMILIES`]) and rendered clusters.
    emoji: Option<Arc<EmojiFace>>,
    emoji_cache: HashMap<String, Option<Arc<crate::frame::ImageDraw>>>,
    /// Unique per (fonts, size); lets GPU atlases know when to start over.
    pub generation: u64,
}

impl FontStack {
    /// Only the embedded font (deterministic; used by tests).
    pub fn embedded(size: f32) -> Self {
        Self::from_fonts(vec![embedded_font()], size)
    }

    /// The configured or best available system monospace font, then fallbacks,
    /// then the embedded font.
    pub fn system(family: Option<&str>, size: f32) -> Self {
        Self::from_db(&system_db(), family, size)
    }

    /// Same as [`FontStack::system`] with an already loaded font database.
    pub fn from_db(db: &fontdb::Database, family: Option<&str>, size: f32) -> Self {
        let mut fonts: Vec<Arc<Font>> = Vec::new();
        let mut loaded = Vec::new();
        let mut load = |name: &str, want_mono: bool, fonts: &mut Vec<Arc<Font>>| {
            let query = fontdb::Query { families: &[fontdb::Family::Name(name)], ..Default::default() };
            let Some(id) = db.query(&query) else { return false };
            if loaded.contains(&id) {
                return false;
            }
            if want_mono && !db.face(id).is_some_and(|f| f.monospaced) {
                return false;
            }
            match parsed_face(db, id) {
                Some(f) => {
                    loaded.push(id);
                    fonts.push(f);
                    true
                }
                None => false,
            }
        };
        let configured = family.filter(|f| !f.trim().is_empty()).map(|f| load(f.trim(), false, &mut fonts)).unwrap_or(false);
        if !configured {
            for name in PREFERRED_MONO {
                if load(name, true, &mut fonts) {
                    break;
                }
            }
        }
        if fonts.is_empty() {
            fonts.push(embedded_font());
        }
        for name in FALLBACK_FAMILIES {
            load(name, false, &mut fonts);
        }
        fonts.push(embedded_font());
        Self::from_fonts(fonts, size)
    }

    fn from_fonts(fonts: Vec<Arc<Font>>, size: f32) -> Self {
        let primary = &fonts[0];
        let (ascent, descent, gap) =
            primary.horizontal_line_metrics(size).map(|m| (m.ascent, m.descent, m.line_gap)).unwrap_or((size * 0.8, -size * 0.2, 0.0));
        let ascent_px = ascent.ceil() as i32;
        let descent_px = (-descent).ceil() as i32;
        let cell_h = (ascent_px + descent_px + gap.max(0.0).round() as i32).max(1);
        let advance = primary.metrics('M', size).advance_width;
        let cell_w = advance.round().max(1.0) as i32;
        // Spread any line gap evenly above and below the glyphs.
        let top_pad = (cell_h - ascent_px - descent_px) / 2;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let generation = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self { fonts, size, cell_w, cell_h, ascent: ascent_px + top_pad, cache: HashMap::new(), emoji: None, emoji_cache: HashMap::new(), generation }
    }

    /// The same fonts at another pixel size (cheap: parsed faces are shared).
    pub fn resized(&self, size: f32) -> Self {
        let mut s = Self::from_fonts(self.fonts.clone(), size);
        s.emoji = self.emoji.clone();
        s
    }

    /// Add the colour emoji face: `family` if installed (`"none"` disables
    /// colour emoji), else the first of [`EMOJI_FAMILIES`] found.
    pub fn with_emoji(mut self, db: &fontdb::Database, family: Option<&str>) -> Self {
        self.emoji = pick_emoji_family(family, &installed_families(db)).and_then(|name| emoji_face(db, &name));
        self.emoji_cache.clear();
        self
    }

    /// Name of the colour emoji face in use, if any.
    pub fn emoji_family(&self) -> Option<&str> {
        self.emoji.as_ref().map(|e| e.family.as_str())
    }

    /// `cluster` (base plus joiners, modifiers and selectors) drawn in colour
    /// to fit `cells` cells, or `None` to draw it as text.
    pub fn emoji_image(&mut self, cluster: &str, cells: i32) -> Option<Arc<crate::frame::ImageDraw>> {
        let face = self.emoji.clone()?;
        if cluster.contains('\u{fe0e}') {
            return None;
        }
        let (bw, bh, generation) = (self.cell_w * cells.max(1), self.cell_h, self.generation);
        let key = format!("{cells}{cluster}");
        self.emoji_cache
            .entry(key)
            .or_insert_with(|| {
                let (rgba, w, h) = face.render(cluster, bw.max(1) as u32, bh.max(1) as u32)?;
                use std::hash::{Hash, Hasher};
                let mut hs = std::collections::hash_map::DefaultHasher::new();
                (generation, cells, cluster).hash(&mut hs);
                Some(Arc::new(crate::frame::ImageDraw {
                    key: hs.finish() | (1 << 63),
                    rgba: Arc::new(rgba),
                    img_w: w,
                    img_h: h,
                    src: (0, 0, w, h),
                    col: 0,
                    row: 0,
                    x_off: 0,
                    y_off: 0,
                    width: w,
                    height: h,
                    z: 0,
                }))
            })
            .clone()
    }

    /// Glyphs the colour emoji face shapes `cluster` into (1 = one combined
    /// emoji), or `None` without a colour face.
    pub fn emoji_glyph_count(&self, cluster: &str) -> Option<usize> {
        let face = self.emoji.as_ref()?;
        let face = rustybuzz::Face::from_slice(&face.data, face.index)?;
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(cluster);
        Some(rustybuzz::shape(&face, &[], buf).len())
    }

    pub fn font_count(&self) -> usize {
        self.fonts.len()
    }

    /// Rasterised glyph for `ch`, from the first font in the stack that has it.
    /// Keys made by [`italic_key`] give a synthetically slanted glyph.
    pub fn glyph(&mut self, ch: char) -> &Glyph {
        let fonts = &self.fonts;
        let size = self.size;
        let (cw, chh, ascent) = (self.cell_w, self.cell_h, self.ascent);
        self.cache.entry(ch).or_insert_with(|| {
            let (base, italic) = match italic_base(ch) {
                Some(b) => (b, true),
                None => (ch, false),
            };
            if let Some(bitmap) = crate::boxdraw::render(base, cw, chh) {
                // Fill the cell exactly: top row at the cell top (see glyph_origin).
                let metrics = Metrics {
                    xmin: 0,
                    ymin: ascent - chh,
                    width: cw.max(1) as usize,
                    height: chh.max(1) as usize,
                    advance_width: cw as f32,
                    ..Metrics::default()
                };
                return Glyph { metrics, bitmap };
            }
            let font = fonts.iter().find(|f| f.lookup_glyph_index(base) != 0).unwrap_or(&fonts[0]);
            // Zero-width marks keep their own placement (see `mark_cell_x`).
            let g = match mouser_core::grid::char_width(base) {
                0 => {
                    let (metrics, bitmap) = font.rasterize(base, size);
                    Glyph { metrics, bitmap }
                }
                cells => fit_glyph(font, base, size, cw * cells as i32),
            };
            if italic { shear(g) } else { g }
        })
    }
}

/// `base` rasterised so its ink stays inside `limit` px (its cells): a
/// fallback glyph wider than that, like Segoe's text-style ❤, is drawn at a
/// smaller size and centred instead of spilling into the next cell.
fn fit_glyph(font: &Font, base: char, size: f32, limit: i32) -> Glyph {
    let (mut metrics, mut bitmap) = font.rasterize(base, size);
    let limit = limit.max(1);
    // Far too wide: a smaller size, centred. A pixel or two of overhang
    // (underscores, italics in some faces) is only trimmed below.
    if metrics.width as i32 > limit + limit / 4 {
        (metrics, bitmap) = font.rasterize(base, size * limit as f32 / metrics.width as f32);
        metrics.xmin = (limit - metrics.width as i32).max(0) / 2;
    }
    let (lo, hi) = ((-metrics.xmin).max(0), (limit - metrics.xmin).min(metrics.width as i32));
    if lo > 0 || hi < metrics.width as i32 {
        let (w, lo, hi) = (metrics.width, lo.max(0) as usize, hi.max(lo) as usize);
        bitmap = bitmap.chunks(w.max(1)).flat_map(|r| r[lo.min(w)..hi.min(w)].to_vec()).collect();
        metrics.xmin += lo as i32;
        metrics.width = hi.saturating_sub(lo);
    }
    Glyph { metrics, bitmap }
}

/// Italic glyphs are cached under a private key: the BMP char moved into
/// Supplementary Private Use Area-B (U+100000 + c). Real PUA-B text is rare;
/// such characters simply draw upright.
pub fn italic_key(c: char) -> char {
    let v = c as u32;
    if v < 0x10000 { char::from_u32(0x100000 + v).unwrap_or(c) } else { c }
}

fn italic_base(c: char) -> Option<char> {
    let v = c as u32;
    (0x100000..0x110000).contains(&v).then(|| char::from_u32(v - 0x100000)).flatten()
}

/// Slant a bitmap ~12° about the baseline (rows above lean right).
fn shear(g: Glyph) -> Glyph {
    const SLANT: f32 = 0.21;
    let m = g.metrics;
    if m.width == 0 || m.height == 0 {
        return g;
    }
    let base_row = m.height as i32 + m.ymin; // rows above the baseline
    let shift = |iy: usize| ((base_row - iy as i32) as f32 * SLANT).round() as i32;
    let (top, bottom) = (shift(0), shift(m.height - 1));
    let (lo, hi) = (top.min(bottom), top.max(bottom));
    let w = m.width + (hi - lo) as usize;
    let mut out = vec![0u8; w * m.height];
    for iy in 0..m.height {
        let off = (shift(iy) - lo) as usize;
        out[iy * w + off..iy * w + off + m.width].copy_from_slice(&g.bitmap[iy * m.width..(iy + 1) * m.width]);
    }
    let mut metrics = m;
    metrics.width = w;
    metrics.xmin = m.xmin + lo;
    Glyph { metrics, bitmap: out }
}

impl FontStack {
    /// Left x for drawing a zero-width mark centred over a base glyph of
    /// `width` px whose cell starts at `cell_x` (pass it to `draw_char`).
    pub fn mark_cell_x(&mut self, mark: char, cell_x: i32, width: i32) -> i32 {
        let m = self.glyph(mark).metrics;
        cell_x + (width - m.width as i32) / 2 - m.xmin
    }
}

/// The embedded face, parsed once per process.
fn embedded_font() -> Arc<Font> {
    static EMBEDDED: OnceLock<Arc<Font>> = OnceLock::new();
    EMBEDDED.get_or_init(|| Arc::new(Font::from_bytes(EMBEDDED_MONO, FontSettings::default()).expect("embedded font"))).clone()
}

/// Parse a system face once per process; later font stacks reuse it, so
/// switching family only parses the new primary face.
fn parsed_face(db: &fontdb::Database, id: fontdb::ID) -> Option<Arc<Font>> {
    static FACES: OnceLock<Mutex<HashMap<fontdb::ID, Arc<Font>>>> = OnceLock::new();
    let faces = FACES.get_or_init(Default::default);
    if let Some(f) = faces.lock().unwrap().get(&id) {
        return Some(f.clone());
    }
    let font = db
        .with_face_data(id, |data, index| Font::from_bytes(data, FontSettings { collection_index: index, ..Default::default() }).ok())
        .flatten()
        .map(Arc::new)?;
    faces.lock().unwrap().insert(id, font.clone());
    Some(font)
}

/// Family names of every installed face (may repeat; dedupe before showing).
pub fn installed_families(db: &fontdb::Database) -> Vec<String> {
    db.faces().filter_map(|f| f.families.first().map(|(n, _)| n.clone())).collect()
}

/// System font database (slow to build; load once and reuse).
pub fn system_db() -> fontdb::Database {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    db
}

/// A mutable 0x00RRGGBB pixel buffer.
pub struct Canvas<'a> {
    pub buf: &'a mut [u32],
    pub width: usize,
    pub height: usize,
}

pub fn blend(dst: u32, src: u32, alpha: u32) -> u32 {
    let ch = |shift: u32| {
        let d = (dst >> shift) & 0xFF;
        let s = (src >> shift) & 0xFF;
        (s * alpha + d * (255 - alpha)) / 255
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// Linear mix of two colours, `t` in 0..=1 towards `b`.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    blend(a, b, (t.clamp(0.0, 1.0) * 255.0).round() as u32)
}

pub fn luminance(c: u32) -> f32 {
    ((c >> 16) & 0xFF) as f32 * 0.299 + ((c >> 8) & 0xFF) as f32 * 0.587 + (c & 0xFF) as f32 * 0.114
}

/// Drawing surface used by [`crate::render_frame`]: the CPU [`Canvas`] paints
/// pixels directly, [`DisplayList`] records the same operations for the GPU.
pub trait Draw {
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    fn clear(&mut self, color: u32);
    fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32);
    /// Draw one glyph in the cell whose top-left corner is (`cell_x`, `cell_top`).
    fn draw_char(&mut self, fonts: &mut FontStack, cell_x: i32, cell_top: i32, ch: char, color: u32);
    /// Draw an image placement with its top-left corner at (`x`, `y`).
    fn draw_image(&mut self, img: &crate::frame::ImageDraw, x: i32, y: i32);
    /// Draw `text` on the monospace grid starting at cell (`x`, `top`).
    fn draw_text(&mut self, fonts: &mut FontStack, x: i32, top: i32, text: &str, color: u32) {
        let cw = fonts.cell_w;
        for (i, ch) in text.chars().enumerate() {
            self.draw_char(fonts, x + i as i32 * cw, top, ch, color);
        }
    }
}

/// Top-left pixel of `ch`'s bitmap when drawn in the cell at (`cell_x`, `cell_top`).
pub fn glyph_origin(fonts: &mut FontStack, cell_x: i32, cell_top: i32, ch: char) -> (i32, i32) {
    let baseline = cell_top + fonts.ascent;
    let m = fonts.glyph(ch).metrics;
    // fontdue: ymin is the bottom edge relative to the baseline (negative for
    // descenders), so the bitmap's top row sits at baseline - (height + ymin).
    (cell_x + m.xmin, baseline - (m.height as i32 + m.ymin))
}

/// One recorded drawing operation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrawOp {
    Rect {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        color: u32,
    },
    /// Glyph bitmap with its top-left corner at (`x`, `y`).
    Glyph {
        x: i32,
        y: i32,
        ch: char,
        color: u32,
    },
    /// `images[index]` with its top-left corner at (`x`, `y`).
    Image {
        index: usize,
        x: i32,
        y: i32,
    },
}

/// A frame as a list of operations, in painting order.
#[derive(Debug, Clone, Default)]
pub struct DisplayList {
    pub width: usize,
    pub height: usize,
    pub clear: u32,
    pub ops: Vec<DrawOp>,
    pub images: Vec<crate::frame::ImageDraw>,
}

impl DisplayList {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, ..Default::default() }
    }

    /// Paint the recorded frame on a CPU canvas (reference for the GPU path).
    pub fn replay(&self, canvas: &mut Canvas, fonts: &mut FontStack) {
        canvas.clear(self.clear);
        for op in &self.ops {
            match *op {
                DrawOp::Rect { x, y, w, h, color } => canvas.fill_rect(x, y, w, h, color),
                DrawOp::Glyph { x, y, ch, color } => canvas.blit_glyph(fonts, x, y, ch, color),
                DrawOp::Image { index, x, y } => canvas.blit_image(&self.images[index], x, y),
            }
        }
    }
}

impl Draw for DisplayList {
    fn width(&self) -> usize {
        self.width
    }
    fn height(&self) -> usize {
        self.height
    }
    fn clear(&mut self, color: u32) {
        self.clear = color;
        self.ops.clear();
        self.images.clear();
    }
    fn draw_image(&mut self, img: &crate::frame::ImageDraw, x: i32, y: i32) {
        self.images.push(img.clone());
        self.ops.push(DrawOp::Image { index: self.images.len() - 1, x, y });
    }
    fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        if w > 0 && h > 0 {
            self.ops.push(DrawOp::Rect { x, y, w, h, color });
        }
    }
    fn draw_char(&mut self, fonts: &mut FontStack, cell_x: i32, cell_top: i32, ch: char, color: u32) {
        if ch == ' ' || ch == '\0' {
            return;
        }
        let (x, y) = glyph_origin(fonts, cell_x, cell_top, ch);
        self.ops.push(DrawOp::Glyph { x, y, ch, color });
    }
}

impl Draw for Canvas<'_> {
    fn width(&self) -> usize {
        self.width
    }
    fn height(&self) -> usize {
        self.height
    }
    fn clear(&mut self, color: u32) {
        Canvas::clear(self, color)
    }
    fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        Canvas::fill_rect(self, x, y, w, h, color)
    }
    fn draw_char(&mut self, fonts: &mut FontStack, cell_x: i32, cell_top: i32, ch: char, color: u32) {
        Canvas::draw_char(self, fonts, cell_x, cell_top, ch, color)
    }
    fn draw_image(&mut self, img: &crate::frame::ImageDraw, x: i32, y: i32) {
        self.blit_image(img, x, y)
    }
}

impl Canvas<'_> {
    pub fn new(buf: &mut [u32], width: usize, height: usize) -> Canvas<'_> {
        Canvas { buf, width, height }
    }

    pub fn clear(&mut self, color: u32) {
        self.buf[..self.width * self.height].fill(color);
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        let x0 = x.clamp(0, self.width as i32) as usize;
        let y0 = y.clamp(0, self.height as i32) as usize;
        let x1 = (x + w).clamp(0, self.width as i32) as usize;
        let y1 = (y + h).clamp(0, self.height as i32) as usize;
        for row in y0..y1 {
            self.buf[row * self.width + x0..row * self.width + x1].fill(color);
        }
    }

    fn blend_px(&mut self, x: i32, y: i32, color: u32, alpha: u32) {
        if alpha == 0 || x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return;
        }
        let i = y as usize * self.width + x as usize;
        self.buf[i] = blend(self.buf[i], color, alpha);
    }

    /// Draw one glyph in the cell whose top-left corner is (`cell_x`, `cell_top`).
    pub fn draw_char(&mut self, fonts: &mut FontStack, cell_x: i32, cell_top: i32, ch: char, color: u32) {
        if ch == ' ' || ch == '\0' {
            return;
        }
        let (left, top) = glyph_origin(fonts, cell_x, cell_top, ch);
        self.blit_glyph(fonts, left, top, ch, color);
    }

    /// Draw an image placement scaled (nearest neighbour) to its display size,
    /// alpha-blended, with its top-left corner at (`x`, `y`).
    pub fn blit_image(&mut self, img: &crate::frame::ImageDraw, x: i32, y: i32) {
        let (sx, sy, sw, sh) = img.src;
        let (dw, dh) = (img.width.max(1) as i32, img.height.max(1) as i32);
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + dw).min(self.width as i32);
        let y1 = (y + dh).min(self.height as i32);
        for py in y0..y1 {
            let iy = sy + ((py - y) as u64 * sh as u64 / dh as u64) as u32;
            for px in x0..x1 {
                let ix = sx + ((px - x) as u64 * sw as u64 / dw as u64) as u32;
                let o = ((iy.min(img.img_h - 1) * img.img_w + ix.min(img.img_w - 1)) * 4) as usize;
                let Some(p) = img.rgba.get(o..o + 4) else { continue };
                let c = (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32;
                self.blend_px(px, py, c, p[3] as u32);
            }
        }
    }

    /// Blend `ch`'s bitmap with its top-left corner at (`left`, `top`).
    pub fn blit_glyph(&mut self, fonts: &mut FontStack, left: i32, top: i32, ch: char, color: u32) {
        let glyph = fonts.glyph(ch);
        let m = glyph.metrics;
        for iy in 0..m.height {
            for ix in 0..m.width {
                let a = glyph.bitmap[iy * m.width + ix] as u32;
                self.blend_px(left + ix as i32, top + iy as i32, color, a);
            }
        }
    }

    /// Draw `text` on the monospace grid starting at cell (`x`, `top`).
    pub fn draw_text(&mut self, fonts: &mut FontStack, x: i32, top: i32, text: &str, color: u32) {
        let cw = fonts.cell_w;
        for (i, ch) in text.chars().enumerate() {
            self.draw_char(fonts, x + i as i32 * cw, top, ch, color);
        }
    }
}

/// Colour emoji families, in order of preference, when none is configured.
pub const EMOJI_FAMILIES: &[&str] =
    &["Noto Color Emoji", "Apple Color Emoji", "Segoe UI Emoji", "Twemoji", "Twitter Color Emoji", "JoyPixels", "EmojiOne Color", "OpenMoji Color"];

/// The emoji family to use from the `installed` family names: the configured
/// one when installed, else the first of [`EMOJI_FAMILIES`]. `"none"` or
/// `"off"` turns colour emoji off. Matching ignores case.
pub fn pick_emoji_family(configured: Option<&str>, installed: &[String]) -> Option<String> {
    let find = |want: &str| installed.iter().find(|n| n.eq_ignore_ascii_case(want)).cloned();
    if let Some(c) = configured.map(str::trim).filter(|c| !c.is_empty()) {
        if c.eq_ignore_ascii_case("none") || c.eq_ignore_ascii_case("off") {
            return None;
        }
        if let Some(n) = find(c) {
            return Some(n);
        }
        log::warn!("emoji font {c:?} is not installed; using the default");
    }
    EMOJI_FAMILIES.iter().find_map(|f| find(f))
}

/// A colour emoji font: raw data for shaping, bitmaps (CBDT/sbix PNG) and
/// COLR v0/v1 vector glyphs.
pub struct EmojiFace {
    pub family: String,
    data: Arc<Vec<u8>>,
    index: u32,
}

fn emoji_face(db: &fontdb::Database, family: &str) -> Option<Arc<EmojiFace>> {
    static FACES: OnceLock<Mutex<HashMap<String, Option<Arc<EmojiFace>>>>> = OnceLock::new();
    let faces = FACES.get_or_init(Default::default);
    if let Some(f) = faces.lock().unwrap().get(family) {
        return f.clone();
    }
    let query = fontdb::Query { families: &[fontdb::Family::Name(family)], ..Default::default() };
    let face = db.query(&query).and_then(|id| {
        db.with_face_data(id, |data, index| {
            let data = Arc::new(data.to_vec());
            Arc::new(EmojiFace { family: family.to_string(), data, index })
        })
    });
    faces.lock().unwrap().insert(family.to_string(), face.clone());
    face
}

/// COLR v0/v1 painter onto a tiny-skia pixmap (font units mapped by `base`).
struct ColrPainter<'f> {
    face: &'f ttf_parser::Face<'f>,
    base: tiny_skia::Transform,
    transforms: Vec<tiny_skia::Transform>,
    path: Option<tiny_skia::Path>,
    clips: Vec<Option<tiny_skia::Mask>>,
    layers: Vec<(tiny_skia::Pixmap, tiny_skia::BlendMode)>,
}

struct PathSink(tiny_skia::PathBuilder);

impl ttf_parser::OutlineBuilder for PathSink {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.0.close();
    }
}

fn skia_color(c: ttf_parser::RgbaColor) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(c.red, c.green, c.blue, c.alpha)
}

fn blend_mode(m: ttf_parser::colr::CompositeMode) -> tiny_skia::BlendMode {
    use tiny_skia::BlendMode as B;
    use ttf_parser::colr::CompositeMode as C;
    match m {
        C::Clear => B::Clear,
        C::Source => B::Source,
        C::Destination => B::Destination,
        C::SourceOver => B::SourceOver,
        C::DestinationOver => B::DestinationOver,
        C::SourceIn => B::SourceIn,
        C::DestinationIn => B::DestinationIn,
        C::SourceOut => B::SourceOut,
        C::DestinationOut => B::DestinationOut,
        C::SourceAtop => B::SourceAtop,
        C::DestinationAtop => B::DestinationAtop,
        C::Xor => B::Xor,
        C::Plus => B::Plus,
        C::Screen => B::Screen,
        C::Overlay => B::Overlay,
        C::Darken => B::Darken,
        C::Lighten => B::Lighten,
        C::ColorDodge => B::ColorDodge,
        C::ColorBurn => B::ColorBurn,
        C::HardLight => B::HardLight,
        C::SoftLight => B::SoftLight,
        C::Difference => B::Difference,
        C::Exclusion => B::Exclusion,
        C::Multiply => B::Multiply,
        C::Hue => B::Hue,
        C::Saturation => B::Saturation,
        C::Color => B::Color,
        C::Luminosity => B::Luminosity,
    }
}

fn stops(it: impl Iterator<Item = ttf_parser::colr::ColorStop>) -> Vec<tiny_skia::GradientStop> {
    it.map(|s| tiny_skia::GradientStop::new(s.stop_offset, skia_color(s.color))).collect()
}

fn spread(e: ttf_parser::colr::GradientExtend) -> tiny_skia::SpreadMode {
    match e {
        ttf_parser::colr::GradientExtend::Pad => tiny_skia::SpreadMode::Pad,
        ttf_parser::colr::GradientExtend::Repeat => tiny_skia::SpreadMode::Repeat,
        ttf_parser::colr::GradientExtend::Reflect => tiny_skia::SpreadMode::Reflect,
    }
}

impl ColrPainter<'_> {
    fn transform(&self) -> tiny_skia::Transform {
        *self.transforms.last().unwrap_or(&self.base)
    }
    fn target(&mut self) -> &mut tiny_skia::Pixmap {
        &mut self.layers.last_mut().expect("base layer").0
    }
    fn shader(&self, paint: ttf_parser::colr::Paint<'_>) -> tiny_skia::Shader<'static> {
        use ttf_parser::colr::Paint;
        let ts = self.transform();
        // A degenerate gradient (one stop, zero length) paints its first colour.
        let first = |s: &[ttf_parser::colr::ColorStop]| {
            tiny_skia::Shader::SolidColor(s.first().map(|s| skia_color(s.color)).unwrap_or(tiny_skia::Color::BLACK))
        };
        match paint {
            Paint::Solid(c) => tiny_skia::Shader::SolidColor(skia_color(c)),
            Paint::LinearGradient(g) => {
                let raw: Vec<_> = g.stops(0, &[]).collect();
                let st = stops(raw.iter().copied());
                // Project p0->p1 onto the line perpendicular to p0->p2 (COLR v1 spec).
                let (p0, p1, p2) = ((g.x0, g.y0), (g.x1, g.y1), (g.x2, g.y2));
                let (nx, ny) = (p2.1 - p0.1, -(p2.0 - p0.0));
                let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
                let n2 = nx * nx + ny * ny;
                let end = if n2 > 0.0 {
                    let k = (dx * nx + dy * ny) / n2;
                    (p0.0 + k * nx, p0.1 + k * ny)
                } else {
                    p1
                };
                tiny_skia::LinearGradient::new(
                    tiny_skia::Point::from_xy(p0.0, p0.1),
                    tiny_skia::Point::from_xy(end.0, end.1),
                    st,
                    spread(g.extend),
                    ts,
                )
                .unwrap_or_else(|| first(&raw))
            }
            Paint::RadialGradient(g) => {
                let raw: Vec<_> = g.stops(0, &[]).collect();
                let st = stops(raw.iter().copied());
                tiny_skia::RadialGradient::new(
                    tiny_skia::Point::from_xy(g.x0, g.y0),
                    tiny_skia::Point::from_xy(g.x1, g.y1),
                    g.r1.max(g.r0),
                    st,
                    spread(g.extend),
                    ts,
                )
                .unwrap_or_else(|| first(&raw))
            }
            // tiny-skia has no sweep gradient: use the mid stop's colour.
            Paint::SweepGradient(g) => {
                let raw: Vec<_> = g.stops(0, &[]).collect();
                tiny_skia::Shader::SolidColor(raw.get(raw.len() / 2).map(|s| skia_color(s.color)).unwrap_or(tiny_skia::Color::BLACK))
            }
        }
    }
}

impl<'a> ttf_parser::colr::Painter<'a> for ColrPainter<'_> {
    fn outline_glyph(&mut self, glyph_id: ttf_parser::GlyphId) {
        let mut sink = PathSink(tiny_skia::PathBuilder::new());
        self.path = self.face.outline_glyph(glyph_id, &mut sink).and_then(|_| sink.0.finish()).and_then(|p| p.transform(self.transform()));
    }
    fn paint(&mut self, paint: ttf_parser::colr::Paint<'a>) {
        let shader = self.shader(paint);
        let p = tiny_skia::Paint { shader, anti_alias: true, ..Default::default() };
        let mask = self.clips.last().cloned().flatten();
        let path = self.path.take();
        let target = self.target();
        match path {
            Some(path) => target.fill_path(&path, &p, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), mask.as_ref()),
            None => {
                let r = tiny_skia::Rect::from_xywh(0.0, 0.0, target.width() as f32, target.height() as f32).unwrap();
                target.fill_rect(r, &p, tiny_skia::Transform::identity(), mask.as_ref());
            }
        }
    }
    fn push_clip(&mut self) {
        let (w, h) = (self.layers[0].0.width(), self.layers[0].0.height());
        let mut mask = self.clips.last().cloned().flatten();
        if let Some(path) = self.path.take() {
            match mask.as_mut() {
                Some(m) => m.intersect_path(&path, tiny_skia::FillRule::Winding, true, tiny_skia::Transform::identity()),
                None => {
                    let mut m = tiny_skia::Mask::new(w, h).unwrap();
                    m.fill_path(&path, tiny_skia::FillRule::Winding, true, tiny_skia::Transform::identity());
                    mask = Some(m);
                }
            }
        }
        self.clips.push(mask);
    }
    fn push_clip_box(&mut self, b: ttf_parser::colr::ClipBox) {
        self.path = tiny_skia::Rect::from_ltrb(b.x_min, b.y_min, b.x_max, b.y_max)
            .map(tiny_skia::PathBuilder::from_rect)
            .and_then(|p| p.transform(self.transform()));
        self.push_clip();
    }
    fn pop_clip(&mut self) {
        self.clips.pop();
    }
    fn push_layer(&mut self, mode: ttf_parser::colr::CompositeMode) {
        let (w, h) = (self.layers[0].0.width(), self.layers[0].0.height());
        self.layers.push((tiny_skia::Pixmap::new(w, h).unwrap(), blend_mode(mode)));
    }
    fn pop_layer(&mut self) {
        if self.layers.len() > 1 {
            let (top, mode) = self.layers.pop().unwrap();
            let p = tiny_skia::PixmapPaint { blend_mode: mode, ..Default::default() };
            self.target().draw_pixmap(0, 0, top.as_ref(), &p, tiny_skia::Transform::identity(), None);
        }
    }
    fn push_transform(&mut self, t: ttf_parser::Transform) {
        let next = self.transform().pre_concat(tiny_skia::Transform::from_row(t.a, t.b, t.c, t.d, t.e, t.f));
        self.transforms.push(next);
    }
    fn pop_transform(&mut self) {
        self.transforms.pop();
    }
}

/// Paints COLR (v0 or v1) glyph `gid` into a `bw` x `bh` straight-alpha RGBA box.
fn paint_colr(data: &[u8], index: u32, gid: u16, bw: u32, bh: u32) -> Option<Vec<u8>> {
    let face = ttf_parser::Face::parse(data, index).ok()?;
    let glyph = ttf_parser::GlyphId(gid);
    if !face.is_color_glyph(glyph) {
        return None;
    }
    // Size the em so the glyph fills the box height, leaving a little air.
    let upem = face.units_per_em().max(1) as f32;
    let (asc, desc) = (face.ascender() as f32, face.descender() as f32);
    let px = bh as f32 * 0.92 * upem / (asc - desc).max(1.0);
    let s = px / upem;
    let baseline = (asc * s).round();
    let advance = face.glyph_hor_advance(glyph).map(|a| a as f32 * s).unwrap_or(bw as f32);
    let left = ((bw as f32 - advance) / 2.0).round();
    let base = tiny_skia::Transform::from_row(s, 0.0, 0.0, -s, left, baseline);
    let mut painter = ColrPainter {
        face: &face,
        base,
        transforms: Vec::new(),
        path: None,
        clips: Vec::new(),
        layers: vec![(tiny_skia::Pixmap::new(bw, bh)?, tiny_skia::BlendMode::SourceOver)],
    };
    face.paint_color_glyph(glyph, 0, ttf_parser::RgbaColor::new(0, 0, 0, 255), &mut painter)?;
    let (pix, _) = painter.layers.swap_remove(0);
    Some(
        pix.pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect(),
    )
}

impl EmojiFace {
    /// The shaped cluster's colour glyph as RGBA, scaled to fit `bw` x `bh`
    /// and centred in it. `None` if the font has no colour glyph for it.
    pub fn render(&self, cluster: &str, bw: u32, bh: u32) -> Option<(Vec<u8>, u32, u32)> {
        let face = rustybuzz::Face::from_slice(&self.data, self.index)?;
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(cluster);
        let shaped = rustybuzz::shape(&face, &[], buf);
        let gid = shaped.glyph_infos().first()?.glyph_id as u16;
        if gid == 0 {
            return None;
        }
        let glyph = rustybuzz::ttf_parser::GlyphId(gid);
        if let Some(img) = face.glyph_raster_image(glyph, u16::MAX)
            && img.format == rustybuzz::ttf_parser::RasterImageFormat::PNG
        {
            let rgba = image::load_from_memory_with_format(img.data, image::ImageFormat::Png).ok()?.to_rgba8();
            return Some(fit(rgba, bw, bh));
        }
        paint_colr(&self.data, self.index, gid, bw, bh).map(|out| (out, bw, bh))
    }
}

/// Scale `img` to fit `bw` x `bh` keeping its aspect, centred on a clear box.
fn fit(img: image::RgbaImage, bw: u32, bh: u32) -> (Vec<u8>, u32, u32) {
    let (w, h) = img.dimensions();
    let scale = (bw as f32 / w.max(1) as f32).min(bh as f32 / h.max(1) as f32);
    let (tw, th) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
    let small = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle);
    let mut out = image::RgbaImage::new(bw, bh);
    image::imageops::overlay(&mut out, &small, ((bw - tw.min(bw)) / 2) as i64, ((bh - th.min(bh)) / 2) as i64);
    (out.into_raw(), bw, bh)
}
