//! Image store shared by the Fuzzball graphics protocol (see `fuzzball.rs`
//! and `docs/protocol/graphics.md`): decoded images, cell-anchored placements,
//! memory accounting and the base64 / PNG decoders.

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub image_id: u32,
    pub id: u32,
    /// Absolute row (lines scrolled off so far + screen row) and column of the top-left cell.
    pub row: i64,
    pub col: u32,
    /// Size in cells.
    pub cols: u32,
    pub rows: u32,
    /// Source rectangle in image pixels.
    pub src: (u32, u32, u32, u32),
    /// Displayed size in pixels (`cols * cell_w`, `rows * cell_h`).
    pub width: u32,
    pub height: u32,
    /// -1 behind text, 0 above text, 1 above text and cursor.
    pub z: i32,
    /// Alternate screen placement.
    pub alt: bool,
    /// Last time it was placed (for eviction order).
    pub stamp: u64,
}

/// A published limit (`g_mem`, `g_max_place`) would be exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverLimit;

#[derive(Debug, Default)]
pub struct Graphics {
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
        self.placements.retain(|q| !(q.image_id == p.image_id && q.id == p.id && q.alt == p.alt));
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
        v.sort_by_key(|(p, _)| (p.z, p.stamp));
        v
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
