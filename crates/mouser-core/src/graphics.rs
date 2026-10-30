//! Kitty graphics protocol (https://sw.kovidgoyal.net/kitty/graphics-protocol/):
//! `ESC _ G <keys> ; <base64 payload> ESC \`.
//!
//! Supported: direct transmission (t=d) of RGB (f=24), RGBA (f=32) and PNG
//! (f=100), chunking (m=1), actions transmit / transmit+put / put / delete /
//! query (a=t,T,p,d,q), image ids and numbers, placement ids, cell-anchored
//! placements with pixel offsets, source rectangles, c/r scaling, z-index and
//! cursor movement (C). Not supported (error reply): file, temp-file and
//! shared-memory transmission, compression (o=z), animation and unicode
//! placeholders.

use std::collections::HashMap;
use std::sync::Arc;

/// Total decoded image bytes kept before the oldest images are evicted.
const MAX_BYTES: usize = 320 * 1024 * 1024;
const MAX_DIM: u32 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub id: u32,
    pub number: u32,
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
    /// Pixel offset inside that cell.
    pub x_off: u32,
    pub y_off: u32,
    /// Source rectangle in image pixels.
    pub src: (u32, u32, u32, u32),
    /// Displayed size in pixels.
    pub width: u32,
    pub height: u32,
    pub z: i32,
}

/// What the parser should do after a command.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Effect {
    pub reply: Option<String>,
    /// Move the cursor by this many cells (right, down) after a placement.
    pub cursor: Option<(u32, u32)>,
}

#[derive(Debug, Default, Clone)]
struct Pending {
    keys: HashMap<char, String>,
    data: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct Graphics {
    pub images: HashMap<u32, Image>,
    pub placements: Vec<Placement>,
    order: Vec<u32>,
    next_id: u32,
    next_key: u64,
    pending: Option<Pending>,
}

/// Where the cursor is and how big a cell is, for placements.
#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    /// Absolute row of screen row 0.
    pub top: i64,
    pub abs_row: i64,
    pub col: u32,
    pub cell_w: u32,
    pub cell_h: u32,
}

fn parse_keys(s: &str) -> HashMap<char, String> {
    s.split(',')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            let mut c = k.chars();
            let key = c.next()?;
            c.next().is_none().then(|| (key, v.to_string()))
        })
        .collect()
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

fn num(keys: &HashMap<char, String>, k: char) -> Option<u32> {
    keys.get(&k).and_then(|v| v.parse().ok())
}

impl Graphics {
    /// Handle one APC payload (the bytes after `G`).
    pub fn command(&mut self, body: &[u8], cur: Cursor) -> Effect {
        let (ctrl, payload) = match body.iter().position(|&b| b == b';') {
            Some(i) => (&body[..i], &body[i + 1..]),
            None => (body, &[][..]),
        };
        let ctrl = String::from_utf8_lossy(ctrl);
        let keys = parse_keys(&ctrl);
        // Chunked transfer: continuation chunks carry only m= (and maybe q=).
        if let Some(p) = &mut self.pending {
            p.data.extend_from_slice(payload);
            if keys.get(&'m').map(String::as_str) == Some("1") {
                return Effect::default();
            }
            let p = self.pending.take().unwrap();
            return self.run(p.keys, &p.data, cur);
        }
        if keys.get(&'m').map(String::as_str) == Some("1") {
            self.pending = Some(Pending { keys, data: payload.to_vec() });
            return Effect::default();
        }
        self.run(keys, payload, cur)
    }

    fn reply(keys: &HashMap<char, String>, msg: &str, id: Option<u32>) -> Option<String> {
        let quiet = num(keys, 'q').unwrap_or(0);
        let ok = msg == "OK";
        if (ok && quiet >= 1) || (!ok && quiet >= 2) {
            return None;
        }
        let id = id.or_else(|| num(keys, 'i').filter(|&i| i > 0));
        let number = num(keys, 'I').filter(|&n| n > 0);
        if id.is_none() && number.is_none() {
            return None;
        }
        let mut s = String::from("\x1b_G");
        let mut parts = Vec::new();
        if let Some(i) = id {
            parts.push(format!("i={i}"));
        }
        if let Some(n) = number {
            parts.push(format!("I={n}"));
        }
        if let Some(p) = num(keys, 'p').filter(|&p| p > 0) {
            parts.push(format!("p={p}"));
        }
        s += &parts.join(",");
        s += ";";
        s += msg;
        s += "\x1b\\";
        Some(s)
    }

    fn run(&mut self, keys: HashMap<char, String>, payload: &[u8], cur: Cursor) -> Effect {
        let action = keys.get(&'a').and_then(|a| a.chars().next()).unwrap_or('t');
        match action {
            't' | 'T' | 'q' => {
                let img = match self.decode(&keys, payload) {
                    Ok(img) => img,
                    Err(e) => return Effect { reply: Self::reply(&keys, &e, None), cursor: None },
                };
                if action == 'q' {
                    return Effect { reply: Self::reply(&keys, "OK", None), cursor: None };
                }
                let id = self.store(&keys, img);
                let mut eff = Effect { reply: None, cursor: None };
                if action == 'T' {
                    match self.place(&keys, id, cur) {
                        Ok(mv) => eff.cursor = mv,
                        Err(e) => return Effect { reply: Self::reply(&keys, &e, Some(id)), cursor: None },
                    }
                }
                let explicit = num(&keys, 'i').filter(|&i| i > 0).or(num(&keys, 'I').filter(|&n| n > 0).map(|_| id));
                eff.reply = explicit.and_then(|_| Self::reply(&keys, "OK", Some(id)));
                eff
            }
            'p' => {
                let id = match self.resolve_id(&keys) {
                    Some(id) => id,
                    None => return Effect { reply: Self::reply(&keys, "ENOENT:image not found", None), cursor: None },
                };
                match self.place(&keys, id, cur) {
                    Ok(mv) => Effect { reply: Self::reply(&keys, "OK", Some(id)), cursor: mv },
                    Err(e) => Effect { reply: Self::reply(&keys, &e, Some(id)), cursor: None },
                }
            }
            'd' => {
                self.delete(&keys, cur);
                Effect::default()
            }
            _ => Effect { reply: Self::reply(&keys, "EINVAL:unsupported action", None), cursor: None },
        }
    }

    fn resolve_id(&self, keys: &HashMap<char, String>) -> Option<u32> {
        if let Some(i) = num(keys, 'i').filter(|&i| i > 0) {
            return self.images.contains_key(&i).then_some(i);
        }
        let n = num(keys, 'I').filter(|&n| n > 0)?;
        // Newest image with that number.
        self.order.iter().rev().find(|id| self.images.get(id).is_some_and(|im| im.number == n)).copied()
    }

    fn decode(&self, keys: &HashMap<char, String>, payload: &[u8]) -> Result<Image, String> {
        match keys.get(&'t').map(String::as_str).unwrap_or("d") {
            "d" => {}
            _ => return Err("ENOTSUPPORTED:only direct transmission (t=d) is supported".into()),
        }
        if keys.contains_key(&'o') {
            return Err("ENOTSUPPORTED:compression is not supported".into());
        }
        let data = base64_decode(payload).ok_or("EINVAL:bad base64")?;
        let format = num(keys, 'f').unwrap_or(32);
        let (w, h, rgba) = match format {
            24 | 32 => {
                let (w, h) = (num(keys, 's').unwrap_or(0), num(keys, 'v').unwrap_or(0));
                if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM {
                    return Err("EINVAL:missing or bad s/v size".into());
                }
                let bpp = (format / 8) as usize;
                if data.len() < w as usize * h as usize * bpp {
                    return Err("ENODATA:insufficient image data".into());
                }
                let rgba = if format == 32 {
                    data[..(w * h * 4) as usize].to_vec()
                } else {
                    data[..(w * h * 3) as usize].as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect()
                };
                (w, h, rgba)
            }
            100 => decode_png(&data).map_err(|e| format!("EBADPNG:{e}"))?,
            _ => return Err("EINVAL:unknown format".into()),
        };
        Ok(Image { id: 0, number: num(keys, 'I').unwrap_or(0), width: w, height: h, rgba: Arc::new(rgba), key: 0 })
    }

    fn store(&mut self, keys: &HashMap<char, String>, mut img: Image) -> u32 {
        let id = match num(keys, 'i').filter(|&i| i > 0) {
            Some(i) => i,
            None => {
                // Auto ids count down from the top so they don't clash with client ids.
                self.next_id += 1;
                u32::MAX - self.next_id
            }
        };
        self.next_key += 1;
        img.id = id;
        img.key = self.next_key;
        // Re-transmitting an id replaces the image and its placements.
        self.placements.retain(|p| p.image_id != id);
        self.order.retain(|&i| i != id);
        self.order.push(id);
        self.images.insert(id, img);
        self.evict();
        id
    }

    fn evict(&mut self) {
        let mut total: usize = self.images.values().map(|i| i.rgba.len()).sum();
        while total > MAX_BYTES && self.order.len() > 1 {
            let old = self.order.remove(0);
            if let Some(im) = self.images.remove(&old) {
                total -= im.rgba.len();
            }
            self.placements.retain(|p| p.image_id != old);
        }
    }

    fn place(&mut self, keys: &HashMap<char, String>, id: u32, cur: Cursor) -> Result<Option<(u32, u32)>, String> {
        let img = self.images.get(&id).ok_or("ENOENT:image not found")?;
        let sx = num(keys, 'x').unwrap_or(0).min(img.width);
        let sy = num(keys, 'y').unwrap_or(0).min(img.height);
        let sw = num(keys, 'w').filter(|&w| w > 0).unwrap_or(img.width - sx).min(img.width - sx);
        let sh = num(keys, 'h').filter(|&h| h > 0).unwrap_or(img.height - sy).min(img.height - sy);
        if sw == 0 || sh == 0 {
            return Err("EINVAL:empty source rectangle".into());
        }
        let (cw, ch) = (cur.cell_w.max(1), cur.cell_h.max(1));
        let cols = num(keys, 'c').filter(|&c| c > 0);
        let rows = num(keys, 'r').filter(|&r| r > 0);
        let (width, height) = match (cols, rows) {
            (Some(c), Some(r)) => (c * cw, r * ch),
            (Some(c), None) => (c * cw, (c as u64 * cw as u64 * sh as u64 / sw as u64) as u32),
            (None, Some(r)) => ((r as u64 * ch as u64 * sw as u64 / sh as u64) as u32, r * ch),
            (None, None) => (sw, sh),
        };
        let x_off = num(keys, 'X').unwrap_or(0).min(cw - 1);
        let y_off = num(keys, 'Y').unwrap_or(0).min(ch - 1);
        let pid = num(keys, 'p').unwrap_or(0);
        if pid > 0 {
            self.placements.retain(|p| !(p.image_id == id && p.id == pid));
        }
        let z = keys.get(&'z').and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
        self.placements.push(Placement {
            image_id: id,
            id: pid,
            row: cur.abs_row,
            col: cur.col,
            x_off,
            y_off,
            src: (sx, sy, sw, sh),
            width,
            height,
            z,
        });
        if num(keys, 'C') == Some(1) {
            return Ok(None);
        }
        let span_c = (x_off + width).div_ceil(cw);
        let span_r = (y_off + height).div_ceil(ch);
        // The cursor ends up on the last row of the image, just after it.
        Ok(Some((span_c, span_r.saturating_sub(1))))
    }

    fn delete(&mut self, keys: &HashMap<char, String>, cur: Cursor) {
        let what = keys.get(&'d').and_then(|v| v.chars().next()).unwrap_or('a');
        let free = what.is_ascii_uppercase();
        let mut removed_ids = Vec::new();
        let hit = |p: &Placement, col: u32, row: i64| {
            let cols = (p.x_off + p.width).div_ceil(cur.cell_w.max(1));
            let rows = (p.y_off + p.height).div_ceil(cur.cell_h.max(1));
            col >= p.col && col < p.col + cols && row >= p.row && row < p.row + rows as i64
        };
        match what.to_ascii_lowercase() {
            'a' => {
                removed_ids.extend(self.placements.iter().map(|p| p.image_id));
                self.placements.clear();
            }
            'i' | 'n' => {
                let id = if what.eq_ignore_ascii_case(&'i') { num(keys, 'i') } else { self.resolve_id(keys) };
                if let Some(id) = id {
                    let pid = num(keys, 'p').unwrap_or(0);
                    self.placements.retain(|p| !(p.image_id == id && (pid == 0 || p.id == pid)));
                    removed_ids.push(id);
                }
            }
            'c' => {
                removed_ids.extend(self.placements.iter().filter(|p| hit(p, cur.col, cur.abs_row)).map(|p| p.image_id));
                self.placements.retain(|p| !hit(p, cur.col, cur.abs_row));
            }
            'p' => {
                let (x, y) = (num(keys, 'x').unwrap_or(1).saturating_sub(1), num(keys, 'y').unwrap_or(1).saturating_sub(1) as i64);
                let row = cur.top + y;
                removed_ids.extend(self.placements.iter().filter(|p| hit(p, x, row)).map(|p| p.image_id));
                self.placements.retain(|p| !hit(p, x, row));
            }
            'z' => {
                let z = keys.get(&'z').and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
                removed_ids.extend(self.placements.iter().filter(|p| p.z == z).map(|p| p.image_id));
                self.placements.retain(|p| p.z != z);
            }
            _ => {}
        }
        if free {
            for id in removed_ids {
                if !self.placements.iter().any(|p| p.image_id == id) {
                    self.images.remove(&id);
                    self.order.retain(|&i| i != id);
                }
            }
        }
    }

    /// Placements visible in rows `top .. top + rows` (absolute), lowest z first.
    pub fn visible(&self, top: i64, rows: usize, cell_h: u32) -> Vec<(&Placement, &Image)> {
        let mut v: Vec<_> = self
            .placements
            .iter()
            .filter(|p| {
                let span = (p.y_off + p.height).div_ceil(cell_h.max(1)) as i64;
                p.row + span > top && p.row < top + rows as i64
            })
            .filter_map(|p| self.images.get(&p.image_id).map(|i| (p, i)))
            .collect();
        v.sort_by_key(|(p, _)| p.z);
        v
    }
}
