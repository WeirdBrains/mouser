//! Font loading, monospace cell metrics and glyph drawing for the CPU renderer.

use fontdue::{Font, FontSettings, Metrics};
use std::collections::HashMap;

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
    fonts: Vec<Font>,
    pub size: f32,
    pub cell_w: i32,
    pub cell_h: i32,
    pub ascent: i32,
    cache: HashMap<char, Glyph>,
}

impl FontStack {
    /// Only the embedded font (deterministic; used by tests).
    pub fn embedded(size: f32) -> Self {
        let font = Font::from_bytes(EMBEDDED_MONO, FontSettings::default()).expect("embedded font");
        Self::from_fonts(vec![font], size)
    }

    /// The configured or best available system monospace font, then fallbacks,
    /// then the embedded font.
    pub fn system(family: Option<&str>, size: f32) -> Self {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let mut fonts = Vec::new();
        let mut loaded = Vec::new();
        let mut load = |name: &str, want_mono: bool, fonts: &mut Vec<Font>| {
            let query = fontdb::Query { families: &[fontdb::Family::Name(name)], ..Default::default() };
            let Some(id) = db.query(&query) else { return false };
            if loaded.contains(&id) {
                return false;
            }
            if want_mono && !db.face(id).is_some_and(|f| f.monospaced) {
                return false;
            }
            let font = db
                .with_face_data(id, |data, index| Font::from_bytes(data, FontSettings { collection_index: index, ..Default::default() }).ok())
                .flatten();
            match font {
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
            fonts.push(Font::from_bytes(EMBEDDED_MONO, FontSettings::default()).expect("embedded font"));
        }
        for name in FALLBACK_FAMILIES {
            load(name, false, &mut fonts);
        }
        fonts.push(Font::from_bytes(EMBEDDED_MONO, FontSettings::default()).expect("embedded font"));
        Self::from_fonts(fonts, size)
    }

    fn from_fonts(fonts: Vec<Font>, size: f32) -> Self {
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
        Self { fonts, size, cell_w, cell_h, ascent: ascent_px + top_pad, cache: HashMap::new() }
    }

    pub fn font_count(&self) -> usize {
        self.fonts.len()
    }

    /// Rasterised glyph for `ch`, from the first font in the stack that has it.
    pub fn glyph(&mut self, ch: char) -> &Glyph {
        let fonts = &self.fonts;
        let size = self.size;
        self.cache.entry(ch).or_insert_with(|| {
            let font = fonts.iter().find(|f| f.lookup_glyph_index(ch) != 0).unwrap_or(&fonts[0]);
            let (metrics, bitmap) = font.rasterize(ch, size);
            Glyph { metrics, bitmap }
        })
    }
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
        let baseline = cell_top + fonts.ascent;
        let glyph = fonts.glyph(ch);
        let m = glyph.metrics;
        // fontdue: ymin is the bottom edge relative to the baseline (negative for
        // descenders), so the bitmap's top row sits at baseline - (height + ymin).
        let top = baseline - (m.height as i32 + m.ymin);
        let left = cell_x + m.xmin;
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
