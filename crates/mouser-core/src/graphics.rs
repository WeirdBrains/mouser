//! Image store shared by the Fuzzball graphics protocol (see `fuzzball.rs`
//! and `docs/protocol/graphics.md`) and the Kitty graphics fallback
//! (`kitty_graphics.rs`): decoded images, cell-anchored placements, their
//! lifecycle (scrolling, erasing, resizing), memory accounting and the
//! base64 / PNG decoders.

use std::collections::HashMap;
use std::sync::Arc;

/// Total decoded image bytes kept (`g_mem`).
pub const MAX_BYTES: usize = 320 * 1024 * 1024;
/// Largest image width or height in pixels (`g_max_dim`).
pub const MAX_DIM: u32 = 10_000;
/// Most placements per screen (`g_max_place`).
pub const MAX_PLACEMENTS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    /// RGBA8, `width * height * 4` bytes.
    pub rgba: Arc<Vec<u8>>,
    /// Unique across the session (ids can be reused); lets renderers cache textures.
    pub key: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placement {
    pub image_id: u32,
    pub id: u32,
    /// Absolute row (lines scrolled off so far + screen row) and column of the top-left cell.
    pub row: i64,
    pub col: u32,
    /// Size in cells as shown now (scaled down to fit the screen width if needed).
    pub cols: u32,
    pub rows: u32,
    /// Column and size the application asked for; [`Placement::fit`] derives
    /// `col`/`cols`/`rows` from these, so scaling on resize is reversible.
    pub req_col: u32,
    pub req_cols: u32,
    pub req_rows: u32,
    /// Source rectangle in image pixels.
    pub src: (u32, u32, u32, u32),
    /// -1 behind text, 0 above text, 1 above text and cursor.
    pub z: i32,
    /// Alternate screen placement.
    pub alt: bool,
    /// Last time it was placed (for eviction order).
    pub stamp: u64,
    /// Kitty `X`/`Y`: pixel offset of the image inside the anchor cell.
    pub x_off: u32,
    pub y_off: u32,
    /// Kitty placement without `c`/`r`: drawn at this natural pixel size.
    pub natural: Option<(u32, u32)>,
    /// Keep the source aspect ratio inside the box, centred (Kitty `c`/`r`).
    pub letterbox: bool,
    /// Never refitted to the screen width (a Kitty placement already wider than
    /// the screen when placed is truncated on the right, as the protocol says).
    pub fixed: bool,
    /// Kitty `U=1` virtual placement: drawn only through Unicode placeholders.
    pub virt: bool,
    /// Kitty relative placement: parent `(image id, placement id)` and the
    /// `H`/`V` cell offset from it.
    pub parent: Option<(u32, u32)>,
    pub rel: (i32, i32),
}

impl Placement {
    /// Fit the requested box into a screen `screen_cols` wide, shrinking in place:
    /// unchanged when it fits; otherwise it keeps its anchor column and shrinks
    /// (aspect kept) to the columns left of the right edge. Once the anchor column
    /// itself is cut off it is pinned to the last column at one column wide, so the
    /// size changes by at most one column per column of resize and nothing is dropped.
    pub fn fit(&mut self, screen_cols: usize) {
        let w = (screen_cols.max(1) as u64).min(u32::MAX as u64) as u32;
        let (col, cols) = if self.req_col + self.req_cols <= w {
            (self.req_col, self.req_cols)
        } else {
            let c = w.saturating_sub(self.req_col).max(1);
            (self.req_col.min(w - c), c)
        };
        self.col = col;
        self.cols = cols.max(1);
        self.rows = ((self.req_rows as u64 * self.cols as u64).div_ceil(self.req_cols.max(1) as u64) as u32).max(1);
    }

    /// Displayed size in pixels for a cell size: `cols` cells wide, and as tall as
    /// the requested box scaled by the same factor.
    pub fn pixel_size(&self, cell: (u32, u32)) -> (u32, u32) {
        let r = self.draw_rect(cell);
        (r.2, r.3)
    }

    /// Where the image is drawn relative to the anchor cell's top-left pixel:
    /// `(x, y, width, height)`. The requested box (natural size, or `req_cols x
    /// req_rows` cells) is scaled by `cols / req_cols` after a shrink, then the
    /// source rectangle is letterboxed into it when asked.
    pub fn draw_rect(&self, cell: (u32, u32)) -> (u32, u32, u32, u32) {
        let (cw, ch) = (cell.0.max(1) as u64, cell.1.max(1) as u64);
        let (num, den) = (self.cols as u64, self.req_cols.max(1) as u64);
        let scale = |v: u64| if num == den { v } else { (v * num / den).max(1) };
        let (bw, bh) = match self.natural {
            Some((w, h)) => (scale(w as u64), scale(h as u64)),
            None if num == den => (self.cols as u64 * cw, self.rows as u64 * ch),
            None => (self.cols as u64 * cw, scale(self.req_rows as u64 * ch)),
        };
        let off = |v: u64| v * num / den;
        let (mut x, mut y, mut w, mut h) = (off(self.x_off as u64), off(self.y_off as u64), bw.max(1), bh.max(1));
        if self.letterbox && self.natural.is_none() {
            let (sw, sh) = (self.src.2.max(1) as u64, self.src.3.max(1) as u64);
            if w * sh > h * sw {
                let nw = (h * sw / sh).max(1);
                x += (w - nw) / 2;
                w = nw;
            } else {
                let nh = (w * sh / sw).max(1);
                y += (h - nh) / 2;
                h = nh;
            }
        }
        let c = |v: u64| v.min(u32::MAX as u64) as u32;
        (c(x), c(y), c(w), c(h))
    }

    /// Whether the placement covers cell (`row` absolute, `col`).
    pub fn covers(&self, row: i64, col: u32) -> bool {
        !self.virt && row >= self.row && row < self.row + self.rows as i64 && col >= self.col && col < self.col + self.cols
    }
}

/// One image to draw: anchor cell (row relative to the top of the view, may
/// be negative), pixel offset from it and drawn size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draw {
    pub image_id: u32,
    pub src: (u32, u32, u32, u32),
    pub row: i64,
    pub col: i64,
    pub x_off: i64,
    pub y_off: i64,
    pub width: u32,
    pub height: u32,
    pub z: i32,
}

/// A published limit (`g_mem`, `g_max_place`) would be exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverLimit;

#[derive(Debug, Default)]
pub struct Graphics {
    /// Follow the Kitty rules: several unnamed (`p=0`) placements per image,
    /// erase commands other than clear-screen leave placements alone, and only
    /// placements entirely inside a scroll region scroll with it.
    pub kitty: bool,
    pub images: HashMap<u32, Image>,
    pub placements: Vec<Placement>,
    next_key: u64,
    clock: u64,
    /// Image id -> last placement time.
    last_placed: HashMap<u32, u64>,
}

impl Graphics {
    pub fn mem_used(&self) -> usize {
        self.images.values().map(|i| i.rgba.len()).sum()
    }

    /// Store (or replace) an image. Evicts least recently placed images without
    /// visible placements to stay under `limit`; `Err` if it still does not fit.
    pub fn insert(&mut self, id: u32, width: u32, height: u32, rgba: Vec<u8>, limit: usize) -> Result<(), OverLimit> {
        self.remove_image(id);
        let need = rgba.len();
        while self.mem_used() + need > limit {
            let victim = self
                .images
                .keys()
                .filter(|i| !self.placements.iter().any(|p| p.image_id == **i))
                .min_by_key(|i| self.last_placed.get(i).copied().unwrap_or(0))
                .copied();
            match victim {
                Some(v) => self.remove_image(v),
                None => return Err(OverLimit),
            }
        }
        self.next_key += 1;
        self.images.insert(id, Image { id, width, height, rgba: Arc::new(rgba), key: self.next_key });
        Ok(())
    }

    pub fn remove_image(&mut self, id: u32) {
        self.images.remove(&id);
        self.last_placed.remove(&id);
        self.placements.retain(|p| p.image_id != id);
    }

    /// Add or move placement `(image_id, id)`.
    pub fn place(&mut self, mut p: Placement) -> Result<(), OverLimit> {
        if !(self.kitty && p.id == 0) {
            self.placements.retain(|q| !(q.image_id == p.image_id && q.id == p.id && (q.alt == p.alt || self.kitty)));
        }
        if self.placements.iter().filter(|q| q.alt == p.alt).count() >= MAX_PLACEMENTS {
            return Err(OverLimit);
        }
        self.clock += 1;
        p.stamp = self.clock;
        self.last_placed.insert(p.image_id, self.clock);
        self.placements.push(p);
        Ok(())
    }

    /// Drop placements matching `f`; with `free`, also image data left without placements.
    pub fn delete(&mut self, free: bool, f: impl Fn(&Placement) -> bool) {
        let gone: Vec<u32> = self.placements.iter().filter(|p| f(p)).map(|p| p.image_id).collect();
        self.placements.retain(|p| !f(p));
        if free {
            for id in gone {
                if !self.placements.iter().any(|p| p.image_id == id) {
                    self.remove_image(id);
                }
            }
        }
    }

    /// Placements on screen `alt` visible in rows `top .. top + rows` (absolute), lowest z first.
    pub fn visible(&self, alt: bool, top: i64, rows: usize) -> Vec<(&Placement, &Image)> {
        let mut v: Vec<_> = self
            .placements
            .iter()
            .filter(|p| p.alt == alt && p.row + p.rows as i64 > top && p.row < top + rows as i64)
            .filter_map(|p| self.images.get(&p.image_id).map(|i| (p, i)))
            .collect();
        v.retain(|(p, _)| !p.virt);
        v.sort_by_key(|(p, _)| (p.z, if self.kitty { p.image_id } else { 0 }, p.stamp));
        v
    }

    /// Draws for the placements visible in absolute rows `top .. top + rows`.
    pub fn draws(&self, alt: bool, top: i64, rows: usize, cell: (u32, u32)) -> Vec<Draw> {
        self.visible(alt, top, rows)
            .into_iter()
            .map(|(p, _)| {
                let r = p.draw_rect(cell);
                Draw {
                    image_id: p.image_id,
                    src: p.src,
                    row: p.row - top,
                    col: p.col as i64,
                    x_off: r.0 as i64,
                    y_off: r.1 as i64,
                    width: r.2,
                    height: r.3,
                    z: p.z,
                }
            })
            .collect()
    }

    /// Replace image `id`'s pixels (same size), e.g. for an animation frame.
    pub fn set_rgba(&mut self, id: u32, rgba: Arc<Vec<u8>>) {
        self.next_key += 1;
        if let Some(i) = self.images.get_mut(&id) {
            i.rgba = rgba;
            i.key = self.next_key;
        }
    }

    /// The alternate screen was left: its placements go with it.
    pub fn leave_alt(&mut self) -> bool {
        let before = self.placements.len();
        self.delete(false, |p| p.alt && !p.virt);
        before != self.placements.len()
    }

    /// Follow a scroll on screen `alt`. `base`: absolute row of screen row 0 when it
    /// happened; `rows`: screen height. Placements scrolled out of the region go.
    /// Kitty placements only scroll when entirely inside the region.
    pub fn scrolled(&mut self, alt: bool, base: i64, ev: crate::grid::ScrollEvent, rows: usize) -> bool {
        let (top, bottom) = (base + ev.top as i64, base + ev.bottom as i64);
        let n = ev.delta.unsigned_abs() as i64;
        let mut changed = false;
        if ev.to_scrollback {
            // Region rows keep their absolute row; rows below the region stay on the
            // same screen row, which is now `n` further down in absolute terms.
            if ev.bottom + 1 < rows {
                for p in self.placements.iter_mut().filter(|p| p.alt == alt && !p.virt && p.row > bottom && p.row < base + rows as i64) {
                    p.row += n;
                    changed = true;
                }
            }
            return changed;
        }
        let kitty = self.kitty;
        let before = self.placements.len();
        let mut moved = Vec::new();
        for (i, p) in self.placements.iter_mut().enumerate() {
            let inside = p.row >= top && if kitty { p.row + p.rows as i64 - 1 <= bottom } else { p.row <= bottom };
            if p.alt == alt && !p.virt && inside {
                p.row += ev.delta as i64;
                changed = true;
                if p.row < top || p.row > bottom {
                    moved.push(i);
                }
            }
        }
        for i in moved.into_iter().rev() {
            self.placements.remove(i);
        }
        changed || before != self.placements.len()
    }

    /// Main-screen placements anchored above absolute row `first` (no longer in the
    /// scrollback) are removed. Image data stays until deleted or evicted.
    pub fn evict_scrollback(&mut self, first: i64) -> bool {
        let before = self.placements.len();
        self.delete(false, |p| !p.alt && !p.virt && p.row < first);
        before != self.placements.len()
    }

    /// Apply a grid resize. `alt`: the active screen; `top`: absolute row of the
    /// first visible line; `shift`: rows the active alt screen moved up to keep the
    /// cursor visible; `rows`/`cols`: the new size. Mouser does not rewrap text, so
    /// placements whose anchor cell was cut off (below the last row, right of the
    /// last column, or above the alt screen) are removed; the rest keep their cells.
    /// The inactive main screen is blanked by a resize in the alt screen, so its
    /// visible placements go too.
    pub fn resize(&mut self, alt: bool, top: i64, shift: i64, rows: usize, cols: usize) -> bool {
        let before = self.placements.len();
        if alt && shift != 0 {
            for p in self.placements.iter_mut().filter(|p| p.alt && !p.virt) {
                p.row -= shift;
            }
        }
        let bottom = top + rows as i64;
        self.delete(false, |p| {
            if p.virt {
                return false;
            }
            if p.alt != alt {
                return !p.alt && p.row >= top;
            }
            p.row >= bottom || (p.alt && p.row < top)
        });
        let mut changed = before != self.placements.len() || shift != 0;
        for p in self.placements.iter_mut().filter(|p| !p.virt && !p.fixed) {
            let old = (p.col, p.cols, p.rows);
            p.fit(cols);
            changed |= old != (p.col, p.cols, p.rows);
        }
        changed
    }

    /// Erase covered rows `from..to` (absolute) on a screen: placements touching them go.
    pub fn erase_rows(&mut self, alt: bool, from: i64, to: i64) -> bool {
        let before = self.placements.len();
        self.delete(false, |p| p.alt == alt && !p.virt && p.row < to && p.row + p.rows as i64 > from);
        before != self.placements.len()
    }

    /// Erase cells `c0..c1` of absolute row `row`.
    pub fn erase_cells(&mut self, alt: bool, row: i64, c0: u32, c1: u32) -> bool {
        let before = self.placements.len();
        self.delete(false, |p| p.alt == alt && !p.virt && row >= p.row && row < p.row + p.rows as i64 && c0 < p.col + p.cols && c1 > p.col);
        before != self.placements.len()
    }
}

/// Standard base64 (padding optional, whitespace ignored).
pub fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &c in input {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        acc = (acc << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Decode a PNG to RGBA8 (palette, gray, gray+alpha, RGB and 16-bit are expanded).
pub fn decode_png(data: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut dec = png::Decoder::new(data);
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16 | png::Transformations::ALPHA);
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let (w, h) = (reader.info().width, reader.info().height);
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM {
        return Err("bad dimensions".into());
    }
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::GrayscaleAlpha => buf.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Rgb => buf.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("palette not expanded".into()),
    };
    if rgba.len() != (w * h * 4) as usize {
        return Err("short image data".into());
    }
    Ok((w, h, rgba))
}
