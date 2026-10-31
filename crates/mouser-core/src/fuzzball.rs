//! Fuzzball terminal protocol (docs/protocol/): common capability query,
//! graphics (upload / place / delete / query) and keyboard mode.
//!
//! Messages arrive as the body of `DCS = F z <sub><verb> ; <params> [: payload] ST`
//! (the parser strips `DCS = F`; the body here starts with `z`).

use crate::graphics::{self, Graphics, Placement};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Protocol version implemented (`v`).
pub const VERSION: u32 = 0;
/// Largest payload accepted per message, in wire bytes (`max_chunk`).
pub const MAX_CHUNK: usize = 65536;
/// Largest DCS body the parser keeps for a Fuzzball message.
pub const MAX_MESSAGE: usize = MAX_CHUNK + 1024;
/// Partial uploads are dropped after this long without a chunk.
pub const UPLOAD_TIMEOUT: Duration = Duration::from_secs(10);

/// Capability reply body (after `id=N,`).
pub fn capabilities() -> String {
    format!(
        "v={VERSION},sub=gk,raw=0,max_chunk={MAX_CHUNK},g_mem={},g_max_dim={},g_fmt=rgba:rgb:png,g_max_place={},g_fit=w,g_reflow=1,k=1,k_ime=0,k_release=1",
        graphics::MAX_BYTES,
        graphics::MAX_DIM,
        graphics::MAX_PLACEMENTS
    )
}

/// Where the cursor is, for placements.
#[derive(Debug, Clone, Copy)]
pub struct Ctx {
    /// Absolute row of the cursor (lines scrolled off + screen row).
    pub abs_row: i64,
    pub col: u32,
    pub cell_w: u32,
    pub cell_h: u32,
    pub alt: bool,
    /// Screen width in columns.
    pub screen_cols: usize,
}

/// What the parser should do after a message.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Effect {
    pub reply: Option<String>,
    /// Move the cursor this many cells right of the anchor and down, like printed text.
    pub cursor: Option<(u32, u32)>,
    /// Graphics changed (redraw).
    pub changed: bool,
}

#[derive(Debug)]
struct Upload {
    id: u32,
    fmt: Fmt,
    w: u32,
    h: u32,
    size: usize,
    data: Vec<u8>,
    last: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fmt {
    Rgba,
    Rgb,
    Png,
}

#[derive(Debug, Default)]
pub struct Fuzzball {
    pub gfx: Graphics,
    upload: Option<Upload>,
    /// Keyboard mode per screen: [main, alternate].
    keyboard: [bool; 2],
    /// Bytes the Kitty graphics store holds of the shared quota.
    pub other_mem: usize,
}

/// Error codes (README §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NotSup,
    Inval,
    Proto,
    NoEnt,
    NoMem,
    Perm,
    Version,
}

impl Error {
    pub fn code(self) -> &'static str {
        match self {
            Error::NotSup => "ENOTSUP",
            Error::Inval => "EINVAL",
            Error::Proto => "EPROTO",
            Error::NoEnt => "ENOENT",
            Error::NoMem => "ENOMEM",
            Error::Perm => "EPERM",
            Error::Version => "EVERSION",
        }
    }
}

type Params = HashMap<String, String>;

fn parse_params(s: &str) -> Params {
    s.split(',').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn num(p: &Params, k: &str) -> Option<u32> {
    p.get(k).and_then(|v| if v.len() <= 10 && v.bytes().all(|b| b.is_ascii_digit()) { v.parse().ok() } else { None })
}

type Res = Result<String, (Error, &'static str)>;

impl Fuzzball {
    pub fn keyboard(&self, alt: bool) -> bool {
        self.keyboard[alt as usize]
    }

    /// The alternate screen was left: its keyboard mode and placements go with it.
    pub fn leave_alt(&mut self) {
        self.keyboard[1] = false;
        self.gfx.leave_alt();
    }

    /// Drop a partial upload idle for more than [`UPLOAD_TIMEOUT`] at `now`.
    /// Returns when the pending upload (if any) will expire.
    pub fn expire_upload(&mut self, now: Instant) -> Option<Instant> {
        let deadline = self.upload.as_ref()?.last + UPLOAD_TIMEOUT;
        if now >= deadline {
            self.upload = None;
            return None;
        }
        Some(deadline)
    }

    /// Whether a chunked upload is in progress.
    pub fn upload_pending(&self) -> bool {
        self.upload.is_some()
    }

    /// Handle one message body (starting with `z`).
    pub fn handle(&mut self, body: &[u8], ctx: Ctx) -> Effect {
        let Some(body) = body.strip_prefix(b"z") else { return Effect::default() };
        if body.len() < 2 {
            return Effect::default();
        }
        let (sub, verb) = (body[0] as char, body[1] as char);
        let rest = &body[2..];
        let rest = rest.strip_prefix(b";").unwrap_or(rest);
        let (params, payload) = match rest.iter().position(|&b| b == b':') {
            Some(i) => (&rest[..i], Some(&rest[i + 1..])),
            None => (rest, None),
        };
        let p = parse_params(&String::from_utf8_lossy(params));
        let id = num(&p, "id").filter(|&i| i > 0);
        let quiet = num(&p, "quiet").unwrap_or(0);
        let mut eff = Effect::default();
        let res: Res = match (sub, verb) {
            ('c', 'q') => Ok(capabilities()),
            ('g', 'u') => self.upload(&p, payload, &mut eff),
            ('g', 'p') => self.place(&p, ctx, &mut eff),
            ('g', 'd') => self.delete(&p, ctx, &mut eff),
            ('g', 'q') => Ok(format!(
                "ok=1,mem_used={},mem_max={},images={},cell_w={},cell_h={}",
                self.gfx.mem_used(),
                graphics::MAX_BYTES,
                self.gfx.images.len(),
                ctx.cell_w,
                ctx.cell_h
            )),
            ('k', 's') => self.keyboard_set(&p, ctx.alt),
            ('k', 'q') => Ok(format!("ok=1,on={}", u8::from(self.keyboard(ctx.alt)))),
            _ => Err((Error::NotSup, "unknown subsystem or verb")),
        };
        // Continuation chunks with more=1 stay silent.
        let silent = matches!(&res, Ok(s) if s.is_empty());
        let reply_sub = if matches!(sub, 'c' | 'g' | 'k') { sub } else { 'c' };
        eff.reply = match (id, res) {
            (Some(id), Ok(s)) if !silent && quiet == 0 => Some(format!("\x1bP=Fz{reply_sub}r;id={id},{s}\x1b\\")),
            (Some(id), Err((e, msg))) if quiet < 2 => Some(format!("\x1bP=Fz{reply_sub}r;id={id},ok=0,err={},msg={msg}\x1b\\", e.code())),
            _ => None,
        };
        eff
    }

    fn keyboard_set(&mut self, p: &Params, alt: bool) -> Res {
        if num(p, "v").is_some_and(|v| v != VERSION) {
            return Err((Error::Version, "version not available"));
        }
        let on = match num(p, "on") {
            Some(1) => true,
            Some(0) => false,
            _ => return Err((Error::Inval, "on must be 0 or 1")),
        };
        self.keyboard[alt as usize] = on;
        Ok(format!("ok=1,on={}", u8::from(on)))
    }

    fn upload(&mut self, p: &Params, payload: Option<&[u8]>, eff: &mut Effect) -> Res {
        let id = num(p, "id").filter(|&i| i > 0).ok_or((Error::Inval, "id required"))?;
        if let Some(u) = &self.upload
            && (u.id != id || u.last.elapsed() > UPLOAD_TIMEOUT)
        {
            let stale = u.last.elapsed() > UPLOAD_TIMEOUT;
            self.upload = None;
            if !stale {
                return Err((Error::Proto, "chunk for another id aborted the upload"));
            }
        }
        match p.get("enc").map(String::as_str).unwrap_or("b64") {
            "b64" => {}
            "raw" => {
                self.upload = None;
                return Err((Error::Perm, "raw payloads are not enabled"));
            }
            _ => {
                self.upload = None;
                return Err((Error::NotSup, "unknown enc"));
            }
        }
        let payload = payload.ok_or((Error::Inval, "payload required"))?;
        let len = num(p, "len").ok_or((Error::Inval, "len required"))? as usize;
        if len != payload.len() {
            self.upload = None;
            return Err((Error::Proto, "len does not match payload"));
        }
        if len > MAX_CHUNK {
            self.upload = None;
            return Err((Error::NoMem, "chunk larger than max_chunk"));
        }
        let bytes = graphics::base64_decode(payload).ok_or((Error::Inval, "bad base64"));
        let bytes = match bytes {
            Ok(b) => b,
            Err(e) => {
                self.upload = None;
                return Err(e);
            }
        };
        let more = num(p, "more") == Some(1);
        if self.upload.is_none() {
            if let Some(cs) = p.get("cs")
                && cs != "srgb"
            {
                return Err((Error::NotSup, "only cs=srgb is supported"));
            }
            let fmt = match p.get("fmt").map(String::as_str) {
                Some("rgba") => Fmt::Rgba,
                Some("rgb") => Fmt::Rgb,
                Some("png") => Fmt::Png,
                Some(_) => return Err((Error::NotSup, "unknown fmt")),
                None => return Err((Error::Inval, "fmt required")),
            };
            let size = num(p, "size").ok_or((Error::Inval, "size required"))? as usize;
            let (w, h) = (num(p, "w").unwrap_or(0), num(p, "h").unwrap_or(0));
            if fmt != Fmt::Png {
                if w == 0 || h == 0 {
                    return Err((Error::Inval, "w and h required"));
                }
                if w > graphics::MAX_DIM || h > graphics::MAX_DIM {
                    return Err((Error::NoMem, "larger than g_max_dim"));
                }
                let bpp = if fmt == Fmt::Rgba { 4 } else { 3 };
                if size != w as usize * h as usize * bpp {
                    return Err((Error::Inval, "size does not match w*h"));
                }
            }
            if size > graphics::MAX_BYTES {
                return Err((Error::NoMem, "larger than g_mem"));
            }
            self.upload = Some(Upload { id, fmt, w, h, size, data: Vec::with_capacity(size.min(MAX_CHUNK * 4)), last: Instant::now() });
        }
        let u = self.upload.as_mut().unwrap();
        u.data.extend_from_slice(&bytes);
        u.last = Instant::now();
        if u.data.len() > u.size {
            self.upload = None;
            return Err((Error::Proto, "more data than size"));
        }
        if more {
            return Ok(String::new());
        }
        let u = self.upload.take().unwrap();
        if u.data.len() != u.size {
            return Err((Error::Proto, "less data than size"));
        }
        let (w, h, rgba) = match u.fmt {
            Fmt::Rgba => (u.w, u.h, u.data),
            Fmt::Rgb => (u.w, u.h, u.data.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect()),
            Fmt::Png => graphics::decode_png(&u.data).map_err(|_| (Error::Inval, "bad png"))?,
        };
        self.gfx.insert(id, w, h, rgba, graphics::MAX_BYTES.saturating_sub(self.other_mem)).map_err(|_| (Error::NoMem, "over g_mem"))?;
        eff.changed = true;
        Ok(format!("ok=1,w={w},h={h}"))
    }

    fn place(&mut self, p: &Params, ctx: Ctx, eff: &mut Effect) -> Res {
        let id = num(p, "id").filter(|&i| i > 0).ok_or((Error::Inval, "id required"))?;
        let img = self.gfx.images.get(&id).ok_or((Error::NoEnt, "no such image"))?;
        let (iw, ih) = (img.width, img.height);
        let sx = num(p, "sx").unwrap_or(0);
        let sy = num(p, "sy").unwrap_or(0);
        if sx >= iw || sy >= ih {
            return Err((Error::Inval, "source rectangle outside image"));
        }
        let sw = num(p, "sw").unwrap_or(iw - sx).min(iw - sx);
        let sh = num(p, "sh").unwrap_or(ih - sy).min(ih - sy);
        if sw == 0 || sh == 0 {
            return Err((Error::Inval, "empty source rectangle"));
        }
        let (cw, ch) = (ctx.cell_w.max(1) as u64, ctx.cell_h.max(1) as u64);
        let (cols, rows) = match (num(p, "cols").filter(|&c| c > 0), num(p, "rows").filter(|&r| r > 0)) {
            (Some(c), Some(r)) => (c, r),
            (Some(c), None) => (c, ((c as u64 * cw * sh as u64).div_ceil(sw as u64 * ch)).max(1) as u32),
            (None, Some(r)) => (((r as u64 * ch * sw as u64).div_ceil(sh as u64 * cw)).max(1) as u32, r),
            (None, None) => ((sw as u64).div_ceil(cw) as u32, (sh as u64).div_ceil(ch) as u32),
        };
        // `fit=w`: as wide as the screen right of the cursor, rows from the aspect.
        let fill = match p.get("fit").map(String::as_str) {
            None => false,
            Some("w") => true,
            Some(_) => return Err((Error::Inval, "fit must be w")),
        };
        let (cols, rows) = if fill {
            let c = (ctx.screen_cols.max(1) as u32).saturating_sub(ctx.col).max(1);
            (c, ((c as u64 * cw * sh as u64).div_ceil(sw as u64 * ch)).max(1) as u32)
        } else {
            (cols, rows)
        };
        if cols > 1000 || rows > 1000 {
            return Err((Error::Inval, "placement too large"));
        }
        let z = match p.get("z").map(String::as_str).unwrap_or("0") {
            "-1" => -1,
            "0" => 0,
            "1" => 1,
            _ => return Err((Error::Inval, "z must be -1 0 or 1")),
        };
        let pid = num(p, "pid").unwrap_or(0);
        let mut pl = Placement {
            image_id: id,
            id: pid,
            row: ctx.abs_row,
            col: ctx.col,
            cols,
            rows,
            req_col: ctx.col,
            req_cols: cols,
            req_rows: rows,
            src: (sx, sy, sw, sh),
            z,
            alt: ctx.alt,
            fill,
            ..Placement::default()
        };
        pl.fit(ctx.screen_cols);
        let (cols, rows) = (pl.cols, pl.rows);
        self.gfx.place(pl).map_err(|_| (Error::NoMem, "over g_max_place"))?;
        eff.changed = true;
        if num(p, "move").unwrap_or(1) == 1 {
            eff.cursor = Some((cols, rows - 1));
        }
        Ok("ok=1".into())
    }

    fn delete(&mut self, p: &Params, ctx: Ctx, eff: &mut Effect) -> Res {
        let free = num(p, "free") == Some(1);
        let alt = ctx.alt;
        match p.get("what").map(String::as_str) {
            Some("all") => self.gfx.delete(free, |q| q.alt == alt),
            Some("image") => {
                let id = num(p, "id").ok_or((Error::Inval, "id required"))?;
                if !self.gfx.images.contains_key(&id) {
                    return Err((Error::NoEnt, "no such image"));
                }
                let pid = num(p, "pid");
                self.gfx.delete(free, |q| q.alt == alt && q.image_id == id && pid.is_none_or(|x| q.id == x));
                if free && pid.is_none() {
                    self.gfx.remove_image(id);
                }
            }
            Some("cell") => {
                let (r, c) = (ctx.abs_row, ctx.col);
                self.gfx.delete(free, |q| q.alt == alt && r >= q.row && r < q.row + q.rows as i64 && c >= q.col && c < q.col + q.cols);
            }
            Some(_) => return Err((Error::NotSup, "unknown what")),
            None => return Err((Error::Inval, "what required")),
        }
        eff.changed = true;
        Ok("ok=1".into())
    }
}

/// Fuzzball keyboard modifier bits (keyboard.md §2).
pub mod mods {
    pub const SHIFT: u8 = 1;
    pub const ALT: u8 = 2;
    pub const CTRL: u8 = 4;
    pub const SUPER: u8 = 8;
    pub const HYPER: u8 = 16;
    pub const META: u8 = 32;
    pub const ALTGR: u8 = 64;
}

/// Press, repeat or release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Press,
    Repeat,
    Release,
}

/// One key event for Fuzzball keyboard mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub action: KeyAction,
    /// W3C UI Events `code`, e.g. `KeyA`.
    pub code: String,
    /// W3C `key` name, or the unmodified character it produces.
    pub key: KeyName,
    pub mods: u8,
    pub locks: u8,
    /// Text inserted (presses and repeats only).
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyName {
    Named(String),
    Char(char),
}

/// `DCS = F z k e ; … ST` for an event.
pub fn encode_key(e: &KeyEvent) -> String {
    let t = match e.action {
        KeyAction::Press => 'p',
        KeyAction::Repeat => 'r',
        KeyAction::Release => 'u',
    };
    let key = match &e.key {
        KeyName::Named(n) => n.clone(),
        KeyName::Char(c) => format!("U+{:04X}", *c as u32),
    };
    let mut s = format!("\x1bP=Fzke;t={t},code={},key={key}", e.code);
    if e.mods != 0 {
        s += &format!(",mods={}", e.mods);
    }
    if e.locks != 0 {
        s += &format!(",locks={}", e.locks);
    }
    if e.action != KeyAction::Release
        && let Some(text) = e.text.as_ref().filter(|t| !t.is_empty())
    {
        s += ",text=";
        for b in text.bytes() {
            s += &format!("{b:02x}");
        }
    }
    s += "\x1b\\";
    s
}
