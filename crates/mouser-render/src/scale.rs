//! Downscaled copies of Fuzzball images, cached so both renderers draw them
//! 1:1 (area-averaged, no nearest-neighbour blockiness) without rescaling every frame.

use crate::frame::ImageDraw;
use std::collections::HashMap;
use std::sync::Arc;

/// Larger targets are drawn unscaled (renderer sampling) rather than prescaled.
const MAX_SCALED_PIXELS: u64 = 16 * 1024 * 1024;
/// Keys of scaled copies have this bit set so they never collide with uploads.
const SCALED_KEY: u64 = 1 << 63;

type CacheKey = (u64, (u32, u32, u32, u32), u32, u32);

#[derive(Default)]
pub struct ImageScaler {
    cache: HashMap<CacheKey, (u64, Arc<Vec<u8>>)>,
    next: u64,
    /// Scaled copies made so far (for tests).
    pub made: u64,
}

impl ImageScaler {
    /// Replace draws that shrink their source by a cached area-averaged copy at
    /// the displayed size. Copies not used by this frame are dropped.
    pub fn prepare(&mut self, draws: &mut [ImageDraw]) {
        let mut used = std::collections::HashSet::new();
        for d in draws.iter_mut() {
            let (_, _, sw, sh) = d.src;
            let (w, h) = (d.width.max(1), d.height.max(1));
            if w > sw || h > sh || (w == sw && h == sh) || w as u64 * h as u64 > MAX_SCALED_PIXELS {
                continue;
            }
            let k = (d.key, d.src, w, h);
            used.insert(k);
            if !self.cache.contains_key(&k) {
                self.next += 1;
                self.made += 1;
                let px = area_scale(&d.rgba, d.img_w, d.img_h, d.src, w, h);
                self.cache.insert(k, (SCALED_KEY | self.next, Arc::new(px)));
            }
            let (key, px) = &self.cache[&k];
            d.key = *key;
            d.rgba = px.clone();
            d.img_w = w;
            d.img_h = h;
            d.src = (0, 0, w, h);
        }
        self.cache.retain(|k, _| used.contains(k));
    }
}

/// Area-average `src` of an RGBA image down to `w` x `h` (alpha-weighted colour).
pub fn area_scale(rgba: &[u8], img_w: u32, img_h: u32, src: (u32, u32, u32, u32), w: u32, h: u32) -> Vec<u8> {
    let (sx, sy, sw, sh) = src;
    let mut out = vec![0u8; w as usize * h as usize * 4];
    for oy in 0..h {
        let y0 = sy + (oy as u64 * sh as u64 / h as u64) as u32;
        let y1 = (sy + ((oy as u64 + 1) * sh as u64 / h as u64) as u32).max(y0 + 1).min(img_h);
        for ox in 0..w {
            let x0 = sx + (ox as u64 * sw as u64 / w as u64) as u32;
            let x1 = (sx + ((ox as u64 + 1) * sw as u64 / w as u64) as u32).max(x0 + 1).min(img_w);
            let (mut r, mut g, mut b, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y as usize * img_w as usize + x as usize) * 4;
                    let Some(p) = rgba.get(i..i + 4) else { continue };
                    let pa = p[3] as u64;
                    r += p[0] as u64 * pa;
                    g += p[1] as u64 * pa;
                    b += p[2] as u64 * pa;
                    a += pa;
                    n += 1;
                }
            }
            let o = (oy as usize * w as usize + ox as usize) * 4;
            if let (Some(r), Some(g), Some(b)) = (r.checked_div(a), g.checked_div(a), b.checked_div(a)) {
                out[o..o + 3].copy_from_slice(&[r as u8, g as u8, b as u8]);
            }
            out[o + 3] = a.checked_div(n).unwrap_or(0) as u8;
        }
    }
    out
}
