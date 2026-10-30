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
        Self { fonts, size, cell_w, cell_h, ascent: ascent_px + top_pad, cache: HashMap::new(), generation }
    }

    /// The same fonts at another pixel size (cheap: parsed faces are shared).
    pub fn resized(&self, size: f32) -> Self {
        Self::from_fonts(self.fonts.clone(), size)
    }

    pub fn font_count(&self) -> usize {
        self.fonts.len()
    }

    /// Rasterised glyph for `ch`, from the first font in the stack that has it.
    /// Keys made by [`italic_key`] give a synthetically slanted glyph.
    pub fn glyph(&mut self, ch: char) -> &Glyph {
        let fonts = &self.fonts;
        let size = self.size;
        self.cache.entry(ch).or_insert_with(|| {
            let (base, italic) = match italic_base(ch) {
                Some(b) => (b, true),
                None => (ch, false),
            };
            let font = fonts.iter().find(|f| f.lookup_glyph_index(base) != 0).unwrap_or(&fonts[0]);
            let (metrics, bitmap) = font.rasterize(base, size);
            let g = Glyph { metrics, bitmap };
            if italic { shear(g) } else { g }
        })
    }
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
