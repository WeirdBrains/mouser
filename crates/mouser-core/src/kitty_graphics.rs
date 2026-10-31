//! Kitty graphics protocol (<https://sw.kovidgoyal.net/kitty/graphics-protocol/>),
//! the fallback next to Fuzzball (`fuzzball.rs`, which takes precedence: its
//! images draw above Kitty images at the same z and its replies come first).
//!
//! Commands arrive as APC strings `ESC _ G <keys> ; <payload> ESC \`. Images
//! and placements live in their own [`Graphics`] store (Kitty ids are a separate
//! namespace) and share the placement lifecycle, renderer, scale cache and the
//! memory quota with Fuzzball. See `docs/protocol/graphics.md` §5 for what is
//! supported and the deliberate limits.

use crate::graphics::{Draw, Graphics, MAX_BYTES, MAX_DIM, OverLimit, Placement, base64_decode, decode_png};
use crate::grid::{Cell, Color};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The Unicode placeholder character.
pub const PLACEHOLDER: char = '\u{10EEEE}';
/// Longest relative placement chain (the protocol requires at least 8).
pub const MAX_CHAIN: usize = 8;
/// Idle time after which a partial chunked upload is dropped.
pub const UPLOAD_TIMEOUT: Duration = crate::fuzzball::UPLOAD_TIMEOUT;
/// Largest file / shared memory object read (`t=f`, `t=t`, `t=s`).
pub const MAX_FILE: usize = MAX_BYTES;
/// Quota for animation frames beyond the root frame.
pub const MAX_FRAME_BYTES: usize = MAX_BYTES;
/// Default gap between animation frames (ms).
pub const DEFAULT_GAP: i32 = 40;

/// The single error kitty uses for every file failure, so a remote program
/// learns nothing about files it cannot read itself.
const EBADF_FILE: &str = "EBADF:Failed to read image file";

// ---------------------------------------------------------------------------
// Control data

/// Parsed control keys (`a=T,f=100,...`). Values are either one character or a
/// (signed 64-bit) integer.
#[derive(Debug, Clone, Default)]
pub struct Cmd {
    vals: Vec<(u8, Val)>,
    /// A key failed to parse.
    pub bad: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Val {
    Num(i64),
    Char(u8),
}

/// Keys whose value is a single character.
const CHAR_KEYS: &[u8] = b"atode";

impl Cmd {
    pub fn parse(control: &[u8]) -> Cmd {
        let mut cmd = Cmd::default();
        for pair in control.split(|&b| b == b',').filter(|p| !p.is_empty()) {
            let Some(eq) = pair.iter().position(|&b| b == b'=') else {
                cmd.bad = true;
                continue;
            };
            let (k, v) = (&pair[..eq], &pair[eq + 1..]);
            if k.len() != 1 || !k[0].is_ascii_alphabetic() {
                cmd.bad = true;
                continue;
            }
            let k = k[0];
            let val = if CHAR_KEYS.contains(&k) {
                match v {
                    [c] if c.is_ascii_alphabetic() => Val::Char(*c),
                    _ => {
                        cmd.bad = true;
                        continue;
                    }
                }
            } else {
                match std::str::from_utf8(v).ok().and_then(|s| s.parse::<i64>().ok()) {
                    Some(n) if (i32::MIN as i64..=u32::MAX as i64).contains(&n) => Val::Num(n),
                    _ => {
                        cmd.bad = true;
                        continue;
                    }
                }
            };
            cmd.vals.retain(|(kk, _)| *kk != k);
            cmd.vals.push((k, val));
        }
        cmd
    }

    fn get(&self, k: u8) -> Option<Val> {
        self.vals.iter().find(|(kk, _)| *kk == k).map(|(_, v)| *v)
    }

    pub fn has(&self, k: u8) -> bool {
        self.get(k).is_some()
    }

    /// Integer key (0 when missing).
    pub fn num(&self, k: u8) -> i64 {
        match self.get(k) {
            Some(Val::Num(n)) => n,
            _ => 0,
        }
    }

    /// Non-negative integer key as u32 (0 when missing or negative).
    pub fn u(&self, k: u8) -> u32 {
        self.num(k).clamp(0, u32::MAX as i64) as u32
    }

    /// Character key.
    pub fn ch(&self, k: u8) -> Option<u8> {
        match self.get(k) {
            Some(Val::Char(c)) => Some(c),
            _ => None,
        }
    }

    pub fn action(&self) -> u8 {
        self.ch(b'a').unwrap_or(b't')
    }
}

/// The reply for `cmd` with `msg` (`OK` or `ECODE:detail`), honouring `q`.
/// `id` overrides the `i` key (for ids the terminal picked). Nothing is sent
/// for commands without `i`/`I`.
pub fn reply(cmd: &Cmd, id: Option<u32>, msg: &str) -> Option<String> {
    let q = cmd.num(b'q');
    let ok = msg == "OK";
    if q >= 2 || (ok && q >= 1) {
        return None;
    }
    let (i, n, p) = (id.unwrap_or(cmd.u(b'i')), cmd.u(b'I'), cmd.u(b'p'));
    if i == 0 && n == 0 {
        return None;
    }
    let mut keys = Vec::new();
    if i != 0 {
        keys.push(format!("i={i}"));
    }
    if n != 0 {
        keys.push(format!("I={n}"));
    }
    if p != 0 {
        keys.push(format!("p={p}"));
    }
    Some(format!("\x1b_G{};{msg}\x1b\\", keys.join(",")))
}

// ---------------------------------------------------------------------------
// Transmission media, compression and pixel formats

/// Directories a `t=t` temporary file may be deleted from.
fn temp_dirs() -> Vec<PathBuf> {
    let mut v = vec![PathBuf::from("/tmp"), PathBuf::from("/dev/shm"), std::env::temp_dir()];
    if let Some(t) = std::env::var_os("TMPDIR") {
        v.push(PathBuf::from(t));
    }
    v.into_iter().filter_map(|d| d.canonicalize().ok()).collect()
}

/// Paths never read: reading special files can have side effects.
fn sensitive(path: &Path) -> bool {
    let s = path.to_string_lossy();
    if s.starts_with("/dev/shm/") {
        return false;
    }
    ["/proc", "/sys", "/dev"].iter().any(|d| s == *d || s.starts_with(&format!("{d}/")))
}

/// Read `size` bytes (0: to the end) at `offset` from a regular file. With
/// `temp`, the file is deleted afterwards if it is in a temporary directory and
/// its path contains `tty-graphics-protocol`. Every failure is the same error.
pub fn read_file(path: &str, size: usize, offset: u64, temp: bool) -> Result<Vec<u8>, String> {
    let fail = || EBADF_FILE.to_string();
    let canon = Path::new(path).canonicalize().map_err(|_| fail())?;
    if sensitive(&canon) {
        return Err(fail());
    }
    let meta = std::fs::metadata(&canon).map_err(|_| fail())?;
    if !meta.is_file() {
        return Err(fail());
    }
    let result = (|| {
        let mut f = std::fs::File::open(&canon).ok()?;
        let avail = meta.len().checked_sub(offset)?;
        let want = if size == 0 { avail } else { size as u64 };
        if want > avail || want > MAX_FILE as u64 {
            return None;
        }
        f.seek(SeekFrom::Start(offset)).ok()?;
        let mut buf = Vec::with_capacity(want as usize);
        f.take(want).read_to_end(&mut buf).ok()?;
        (buf.len() as u64 == want).then_some(buf)
    })();
    if temp && canon.to_string_lossy().contains("tty-graphics-protocol") && temp_dirs().iter().any(|d| canon.starts_with(d)) {
        let _ = std::fs::remove_file(&canon);
    }
    result.ok_or_else(fail)
}

/// Read and unlink a POSIX shared memory object.
#[cfg(unix)]
pub fn read_shm(name: &str, size: usize, offset: usize) -> Result<Vec<u8>, String> {
    let fail = || "EBADF:Failed to read shared memory".to_string();
    let c = std::ffi::CString::new(name).map_err(|_| fail())?;
    // SAFETY: plain libc calls on a NUL-terminated name; the mapping is read
    // within its length and unmapped before returning.
    unsafe {
        let fd = libc::shm_open(c.as_ptr(), libc::O_RDONLY, 0);
        if fd < 0 {
            return Err(fail());
        }
        let mut st: libc::stat = std::mem::zeroed();
        let ok = libc::fstat(fd, &mut st) == 0;
        let len = if ok { st.st_size.max(0) as usize } else { 0 };
        let want = if size == 0 { len.saturating_sub(offset) } else { size };
        let mut out = Err(fail());
        if ok && len > 0 && offset.checked_add(want).is_some_and(|e| e <= len) && want <= MAX_FILE {
            let p = libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, fd, 0);
            if p != libc::MAP_FAILED {
                out = Ok(std::slice::from_raw_parts((p as *const u8).add(offset), want).to_vec());
                libc::munmap(p, len);
            }
        }
        libc::close(fd);
        libc::shm_unlink(c.as_ptr());
        out
    }
}

#[cfg(not(unix))]
pub fn read_shm(_name: &str, _size: usize, _offset: usize) -> Result<Vec<u8>, String> {
    Err("EBADF:Shared memory is not supported on this platform".into())
}

/// zlib-inflate at most `limit` bytes.
pub fn inflate(data: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("EINVAL:Failed to inflate image data: {e}"))?;
    if out.len() > limit {
        return Err("EFBIG:Decompressed image data too large".into());
    }
    Ok(out)
}

/// Colours for the indexed formats (`f=8`/`f=16`), as 0xRRGGBB: the 256-colour
/// table with 248-255 replaced by the special colours.
pub type Palette = [u32; 256];

/// Decode pixel data in format `f` to RGBA8. `w`/`h` are required for the raw
/// formats.
pub fn decode(f: u32, w: u32, h: u32, data: &[u8], palette: &Palette) -> Result<(u32, u32, Vec<u8>), String> {
    if f == 100 {
        return decode_png(data).map_err(|e| format!("EBADPNG:Failed to decode PNG: {e}"));
    }
    let bpp = match f {
        8 => 1,
        16 => 2,
        24 => 3,
        32 => 4,
        _ => return Err(format!("EINVAL:Unknown image format: {f}")),
    };
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM {
        return Err("EINVAL:Image width and height must be given and within limits".into());
    }
    let n = w as usize * h as usize;
    if data.len() < n * bpp {
        return Err(format!("ENODATA:Insufficient image data: {} < {}", data.len(), n * bpp));
    }
    let data = &data[..n * bpp];
    let pal = |i: u8| palette[i as usize].to_be_bytes();
    let rgba = match f {
        32 => data.to_vec(),
        24 => data.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        8 => data
            .iter()
            .flat_map(|&i| {
                let c = pal(i);
                [c[1], c[2], c[3], 255]
            })
            .collect(),
        _ => data
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| {
                let c = pal(p[0]);
                [c[1], c[2], c[3], p[1]]
            })
            .collect(),
    };
    Ok((w, h, rgba))
}

/// Bytes per pixel of the raw formats (None for PNG / unknown).
fn raw_bpp(f: u32) -> Option<usize> {
    match f {
        8 => Some(1),
        16 => Some(2),
        24 => Some(3),
        32 => Some(4),
        _ => None,
    }
}

/// Load the pixel data a transmit command describes: medium, compression,
/// format. `dims` supplies the size when `s`/`v` are missing (frames).
pub fn load(cmd: &Cmd, payload: &[u8], dims: Option<(u32, u32)>, palette: &Palette) -> Result<(u32, u32, Vec<u8>), String> {
    let f = if cmd.has(b'f') { cmd.u(b'f') } else { 32 };
    let (mut w, mut h) = (cmd.u(b's'), cmd.u(b'v'));
    if let Some((dw, dh)) = dims {
        if w == 0 {
            w = dw;
        }
        if h == 0 {
            h = dh;
        }
    }
    let raw = match cmd.ch(b't').unwrap_or(b'd') {
        b'd' => base64_decode(payload).ok_or("EINVAL:Invalid base64 payload")?,
        m @ (b'f' | b't' | b's') => {
            let name = base64_decode(payload).and_then(|b| String::from_utf8(b).ok()).ok_or("EINVAL:Invalid base64 file name")?;
            let (size, offset) = (cmd.u(b'S') as usize, cmd.u(b'O'));
            if m == b's' { read_shm(&name, size, offset as usize)? } else { read_file(&name, size, offset as u64, m == b't')? }
        }
        m => return Err(format!("EINVAL:Unknown transmission medium: {}", m as char)),
    };
    let data = match cmd.ch(b'o') {
        None => raw,
        Some(b'z') => {
            let limit = match raw_bpp(f) {
                Some(bpp) if w > 0 && h > 0 => w as usize * h as usize * bpp,
                _ => MAX_BYTES,
            };
            inflate(&raw, limit.min(MAX_BYTES))?
        }
        Some(c) => return Err(format!("EINVAL:Unknown compression: {}", c as char)),
    };
    decode(f, w, h, &data, palette)
}

// ---------------------------------------------------------------------------
// Commands

/// Terminal state a command needs.
#[derive(Debug, Clone, Copy)]
pub struct Ctx<'a> {
    /// Cursor: absolute row and column.
    pub abs_row: i64,
    pub col: u32,
    /// Absolute row of screen row 0 and the screen size.
    pub top: i64,
    pub rows: usize,
    pub cols: usize,
    pub cell: (u32, u32),
    pub alt: bool,
    /// Bytes the other (Fuzzball) store uses of the shared quota.
    pub other_mem: usize,
    pub palette: &'a Palette,
    pub now: Instant,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Effect {
    pub reply: Option<String>,
    /// Move the cursor right `dx` columns from where it was and down `dy` rows.
    pub cursor: Option<(u32, u32)>,
    /// Something visible may have changed.
    pub changed: bool,
}

#[derive(Debug)]
struct Pending {
    cmd: Cmd,
    data: Vec<u8>,
    last: Instant,
    too_big: bool,
}

#[derive(Debug, Clone)]
struct Frame {
    rgba: Arc<Vec<u8>>,
    /// ms to the next frame; <= 0 is gapless (skipped).
    gap: i32,
}

#[derive(Debug, Clone)]
struct Anim {
    frames: Vec<Frame>,
    cur: usize,
    /// 1 stopped, 2 running but waiting at the last frame, 3 looping.
    state: u8,
    /// Loops left to play (`None`: infinite).
    loops: Option<u32>,
    played: u32,
    next: Option<Instant>,
}

impl Anim {
    fn extra_bytes(&self) -> usize {
        self.frames.iter().skip(1).map(|f| f.rgba.len()).sum()
    }
}

/// Kitty graphics state for one terminal.
#[derive(Debug)]
pub struct KittyGraphics {
    pub gfx: Graphics,
    pending: Option<Pending>,
    /// Image number (`I`) -> newest image id with it.
    numbers: HashMap<u32, u32>,
    /// Ids given to images sent without `i`/`I` (never reported).
    anon: HashSet<u32>,
    anims: HashMap<u32, Anim>,
    next_id: u32,
}

impl Default for KittyGraphics {
    fn default() -> Self {
        let mut gfx = Graphics::default();
        gfx.kitty = true;
        Self { gfx, pending: None, numbers: HashMap::new(), anon: HashSet::new(), anims: HashMap::new(), next_id: 0 }
    }
}

/// Error helpers.
fn einval(msg: &str) -> String {
    format!("EINVAL:{msg}")
}

impl KittyGraphics {
    /// Bytes used, including animation frames.
    pub fn mem_used(&self) -> usize {
        self.gfx.mem_used() + self.anims.values().map(Anim::extra_bytes).sum::<usize>()
    }

    /// Whether a chunked upload is in progress.
    pub fn upload_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Drop a partial upload idle for more than [`UPLOAD_TIMEOUT`]; returns when
    /// the pending one (if any) expires.
    pub fn expire_upload(&mut self, now: Instant) -> Option<Instant> {
        let deadline = self.pending.as_ref()?.last + UPLOAD_TIMEOUT;
        if now >= deadline {
            self.pending = None;
            return None;
        }
        Some(deadline)
    }

    /// Image id for a command's `i` / `I` keys.
    fn target(&self, cmd: &Cmd) -> Option<u32> {
        match (cmd.u(b'i'), cmd.u(b'I')) {
            (0, 0) => None,
            (0, n) => self.numbers.get(&n).copied(),
            (i, _) => Some(i),
        }
    }

    /// Handle one APC body (starting with `G`).
    pub fn handle(&mut self, body: &[u8], ctx: Ctx) -> Effect {
        let body = body.strip_prefix(b"G").unwrap_or(body);
        let (control, payload) = match body.iter().position(|&b| b == b';') {
            Some(i) => (&body[..i], &body[i + 1..]),
            None => (body, &[][..]),
        };
        let cmd = Cmd::parse(control);
        // While a chunked upload runs, every command but a delete continues it.
        if let Some(mut p) = self.pending.take()
            && (cmd.action() != b'd' || !cmd.has(b'a'))
        {
            {
                if p.data.len() + payload.len() > MAX_BYTES / 3 * 4 + 16 {
                    p.too_big = true;
                } else {
                    p.data.extend_from_slice(payload);
                }
                p.last = ctx.now;
                if cmd.num(b'm') == 1 {
                    self.pending = Some(p);
                    return Effect::default();
                }
                if p.too_big {
                    return Effect { reply: reply(&p.cmd, None, "EFBIG:Image data too large"), ..Effect::default() };
                }
                return self.run(&p.cmd, &p.data, ctx);
            }
        }
        if cmd.bad {
            return Effect { reply: reply(&cmd, None, &einval("Malformed control data")), ..Effect::default() };
        }
        if cmd.num(b'm') == 1
            && matches!(cmd.action(), b't' | b'T' | b'q' | b'f')
            && cmd.ch(b't').unwrap_or(b'd') == b'd'
            && !(cmd.action() == b'q' && cmd.has(b'e'))
        {
            self.pending = Some(Pending { cmd, data: payload.to_vec(), last: ctx.now, too_big: false });
            return Effect::default();
        }
        self.run(&cmd, payload, ctx)
    }

    fn run(&mut self, cmd: &Cmd, payload: &[u8], ctx: Ctx) -> Effect {
        if cmd.u(b'i') != 0 && cmd.u(b'I') != 0 {
            return Effect { reply: reply(cmd, None, &einval("Must not specify both i and I")), ..Effect::default() };
        }
        if let (b'q', Some(e)) = (cmd.action(), cmd.ch(b'e')) {
            return Effect { reply: feature_reply(cmd, e), ..Effect::default() };
        }
        let res = match cmd.action() {
            b'q' => self.query(cmd, payload, ctx),
            b't' | b'T' => self.transmit(cmd, payload, ctx),
            b'p' => self.put(cmd, ctx),
            b'd' => {
                let changed = self.delete(cmd, ctx);
                return Effect { changed, ..Effect::default() };
            }
            b'f' => self.frame(cmd, payload, ctx),
            b'a' => self.control(cmd, ctx),
            b'c' => self.compose(cmd),
            a => Err((None, einval(&format!("Unknown action: {}", a as char)))),
        };
        match res {
            Ok((id, cursor, changed, ok)) => Effect { reply: if ok { reply(cmd, id, "OK") } else { None }, cursor, changed },
            Err((id, e)) => Effect { reply: reply(cmd, id, &e), ..Effect::default() },
        }
    }
}

/// `(reported id, cursor move, changed, send OK)` or `(reported id, error)`.
type Outcome = Result<(Option<u32>, Option<(u32, u32)>, bool, bool), (Option<u32>, String)>;

impl KittyGraphics {
    fn query(&mut self, cmd: &Cmd, payload: &[u8], ctx: Ctx) -> Outcome {
        load(cmd, payload, None, ctx.palette).map_err(|e| (None, e))?;
        Ok((None, None, false, true))
    }

    fn alloc_id(&mut self) -> u32 {
        loop {
            self.next_id = self.next_id.wrapping_add(1).max(1);
            if !self.gfx.images.contains_key(&self.next_id) {
                return self.next_id;
            }
        }
    }

    fn alloc_anon(&mut self) -> u32 {
        (1..=u32::MAX).rev().find(|i| !self.gfx.images.contains_key(i)).unwrap_or(u32::MAX)
    }

    /// Drop bookkeeping for images that are gone.
    fn prune(&mut self) {
        let images = &self.gfx.images;
        self.anon.retain(|i| images.contains_key(i));
        self.anims.retain(|i, _| images.contains_key(i));
        self.numbers.retain(|_, i| images.contains_key(i));
    }

    fn transmit(&mut self, cmd: &Cmd, payload: &[u8], ctx: Ctx) -> Outcome {
        let (i, n) = (cmd.u(b'i'), cmd.u(b'I'));
        let (w, h, rgba) = load(cmd, payload, None, ctx.palette).map_err(|e| (None, e))?;
        let id = match (i, n) {
            (0, 0) => self.alloc_anon(),
            (0, _) => self.alloc_id(),
            (i, _) => i,
        };
        let reported = (n != 0).then_some(id);
        self.anims.remove(&id);
        self.anon.remove(&id);
        let extra: usize = self.anims.values().map(Anim::extra_bytes).sum();
        let limit = MAX_BYTES.saturating_sub(ctx.other_mem + extra);
        let res = self.gfx.insert(id, w, h, rgba, limit);
        self.prune();
        res.map_err(|OverLimit| (reported, "ENOSPC:Image storage quota exceeded".to_string()))?;
        if i == 0 && n == 0 {
            self.anon.insert(id);
        }
        if n != 0 {
            self.numbers.insert(n, id);
        }
        if cmd.action() == b'T' {
            let cursor = self.place(cmd, id, ctx).map_err(|e| (reported, e))?;
            return Ok((reported, cursor, true, true));
        }
        Ok((reported, None, true, true))
    }

    fn put(&mut self, cmd: &Cmd, ctx: Ctx) -> Outcome {
        let reported = |id| (cmd.u(b'I') != 0).then_some(id);
        let id = self.target(cmd).filter(|i| self.gfx.images.contains_key(i));
        let Some(id) = id else {
            return Err((None, "ENOENT:Image not found".into()));
        };
        let cursor = self.place(cmd, id, ctx).map_err(|e| (reported(id), e))?;
        Ok((reported(id), cursor, true, true))
    }

    fn find(&self, key: (u32, u32)) -> Option<&Placement> {
        self.gfx.placements.iter().find(|q| q.image_id == key.0 && q.id == key.1)
    }

    /// Create (or move) a placement of image `id` from a put command; returns the
    /// cursor movement.
    fn place(&mut self, cmd: &Cmd, id: u32, ctx: Ctx) -> Result<Option<(u32, u32)>, String> {
        let img = &self.gfx.images[&id];
        let (iw, ih) = (img.width, img.height);
        let (x, y) = (cmd.u(b'x').min(iw), cmd.u(b'y').min(ih));
        let sw = match cmd.u(b'w') {
            0 => iw - x,
            w => w.min(iw - x),
        };
        let sh = match cmd.u(b'h') {
            0 => ih - y,
            h => h.min(ih - y),
        };
        if sw == 0 || sh == 0 {
            return Err(einval("Source rectangle is empty"));
        }
        let (cw, chh) = (ctx.cell.0.max(1) as u64, ctx.cell.1.max(1) as u64);
        let (xo, yo) = (cmd.u(b'X').min(cw as u32 - 1), cmd.u(b'Y').min(chh as u32 - 1));
        let (c, r) = (cmd.u(b'c') as u64, cmd.u(b'r') as u64);
        let (sw64, sh64) = (sw as u64, sh as u64);
        let (cols, rows, natural) = match (c, r) {
            (0, 0) => ((xo as u64 + sw64).div_ceil(cw), (yo as u64 + sh64).div_ceil(chh), Some((sw, sh))),
            (c, 0) => (c, (c * cw * sh64).div_ceil(sw64 * chh), None),
            (0, r) => ((r * chh * sw64).div_ceil(sh64 * cw), r, None),
            (c, r) => (c, r, None),
        };
        let clampc = |v: u64| v.clamp(1, MAX_DIM as u64) as u32;
        let (cols, rows) = (clampc(cols), clampc(rows));
        let virt = cmd.u(b'U') == 1;
        let parent = (cmd.u(b'P'), cmd.u(b'Q'));
        if virt && parent.0 != 0 {
            return Err(einval("A virtual placement cannot be relative"));
        }
        // Images without a client id cannot have placement ids.
        let pid = if self.anon.contains(&id) { 0 } else { cmd.u(b'p') };
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
            src: (x, y, sw, sh),
            z: cmd.num(b'z') as i32,
            alt: ctx.alt,
            x_off: xo,
            y_off: yo,
            natural,
            letterbox: natural.is_none(),
            fixed: ctx.col as usize + cols as usize > ctx.cols,
            virt,
            ..Placement::default()
        };
        if virt {
            (pl.row, pl.col, pl.req_col, pl.fixed, pl.letterbox, pl.natural, pl.x_off, pl.y_off) = (0, 0, 0, true, true, None, 0, 0);
        }
        let mut moves = !virt && cmd.u(b'C') != 1;
        if parent.0 != 0 {
            moves = false;
            let par = self.find(parent).cloned().ok_or("ENOPARENT:Parent placement not found")?;
            let mut depth = 1;
            let mut cur = Some(par.clone());
            while let Some(q) = cur {
                if pid != 0 && q.image_id == id && q.id == pid {
                    return Err("ECYCLE:Relative placement would create a cycle".into());
                }
                cur = match q.parent {
                    Some(pp) => {
                        depth += 1;
                        if depth > MAX_CHAIN {
                            return Err("ETOODEEP:Relative placement chain too deep".into());
                        }
                        self.find(pp).cloned()
                    }
                    None => None,
                };
            }
            let (h, v) = (cmd.num(b'H') as i32, cmd.num(b'V') as i32);
            pl.parent = Some(parent);
            pl.rel = (h, v);
            pl.alt = par.alt;
            if par.virt {
                // Positioned from the parent's placeholder cells when drawn.
                pl.virt = true;
            } else {
                pl.row = par.row + v as i64;
                pl.col = (par.col as i64 + h as i64).clamp(0, u32::MAX as i64) as u32;
                pl.req_col = pl.col;
                pl.fixed = pl.col as usize + cols as usize > ctx.cols;
            }
        }
        if pid != 0
            && let Some(old) = self.find((id, pid)).cloned()
            && !old.virt
            && !pl.virt
        {
            self.shift_children((id, pid), pl.row - old.row, pl.col as i64 - old.col as i64, 0);
        }
        self.gfx.place(pl).map_err(|OverLimit| "ENOSPC:Too many placements".to_string())?;
        Ok(moves.then_some((cols, rows - 1)))
    }

    /// Move the relative placements hanging off `key` along with it.
    fn shift_children(&mut self, key: (u32, u32), dr: i64, dc: i64, depth: usize) {
        if depth > MAX_CHAIN || (dr, dc) == (0, 0) {
            return;
        }
        let mut kids = Vec::new();
        for p in self.gfx.placements.iter_mut().filter(|p| p.parent == Some(key) && !p.virt) {
            p.row += dr;
            p.col = (p.col as i64 + dc).clamp(0, u32::MAX as i64) as u32;
            p.req_col = p.col;
            kids.push((p.image_id, p.id));
        }
        for k in kids {
            self.shift_children(k, dr, dc, depth + 1);
        }
    }

    /// Remove relative placements whose parent is gone (and their images once
    /// they have no placements left), repeatedly.
    fn cascade(&mut self) {
        loop {
            let keys: HashSet<(u32, u32)> = self.gfx.placements.iter().map(|p| (p.image_id, p.id)).collect();
            let orphans: Vec<u32> = self.gfx.placements.iter().filter(|p| p.parent.is_some_and(|k| !keys.contains(&k))).map(|p| p.image_id).collect();
            if orphans.is_empty() {
                return;
            }
            self.gfx.placements.retain(|p| !p.parent.is_some_and(|k| !keys.contains(&k)));
            for id in orphans {
                if !self.gfx.placements.iter().any(|p| p.image_id == id) {
                    self.gfx.remove_image(id);
                }
            }
        }
    }

    fn delete(&mut self, cmd: &Cmd, ctx: Ctx) -> bool {
        self.pending = None;
        let d = cmd.ch(b'd').unwrap_or(b'a');
        let free = d.is_ascii_uppercase();
        let (top, bottom, alt) = (ctx.top, ctx.top + ctx.rows as i64, ctx.alt);
        let cell = |k| cmd.u(k).saturating_sub(1);
        let (cx, cy) = (cell(b'x'), top + cell(b'y') as i64);
        let z = cmd.num(b'z') as i32;
        let pid = cmd.u(b'p');
        let frames = |s: &Self| s.anims.values().map(|a| a.frames.len()).sum::<usize>();
        let before = (self.gfx.placements.len(), self.gfx.images.len(), frames(self));
        let mut drop_images = Vec::new();
        match d.to_ascii_lowercase() {
            b'a' => self.gfx.delete(free, |p| !p.virt && p.alt == alt && p.row + p.rows as i64 > top && p.row < bottom),
            k @ (b'i' | b'n') => {
                let id = if k == b'i' { Some(cmd.u(b'i')) } else { self.numbers.get(&cmd.u(b'I')).copied() };
                let Some(id) = id.filter(|i| *i != 0) else {
                    return false;
                };
                if pid != 0 {
                    self.gfx.delete(free, |p| p.image_id == id && p.id == pid);
                } else {
                    self.gfx.delete(free, |p| p.image_id == id);
                    if free {
                        drop_images.push(id);
                    }
                }
            }
            b'c' => self.gfx.delete(free, |p| p.alt == alt && p.covers(ctx.abs_row, ctx.col)),
            b'p' => self.gfx.delete(free, |p| p.alt == alt && p.covers(cy, cx)),
            b'q' => self.gfx.delete(free, |p| p.alt == alt && p.covers(cy, cx) && p.z == z),
            b'r' => {
                let (lo, hi) = (cmd.u(b'x'), cmd.u(b'y'));
                let anon = &self.anon;
                let inr = |i: u32| i >= lo && i <= hi && !anon.contains(&i);
                if free {
                    drop_images.extend(self.gfx.images.keys().copied().filter(|i| inr(*i)));
                }
                self.gfx.delete(free, |p| inr(p.image_id));
            }
            b'x' => self.gfx.delete(free, |p| !p.virt && p.alt == alt && cx >= p.col && cx < p.col + p.cols),
            b'y' => self.gfx.delete(free, |p| !p.virt && p.alt == alt && cy >= p.row && cy < p.row + p.rows as i64),
            b'z' => self.gfx.delete(free, |p| !p.virt && p.alt == alt && p.z == z),
            b'f' => {
                let ids: Vec<u32> = match self.target(cmd) {
                    Some(id) => vec![id],
                    None => self.anims.keys().copied().collect(),
                };
                for id in ids {
                    if let Some(a) = self.anims.get_mut(&id) {
                        a.frames.truncate(1);
                        a.cur = 0;
                        a.state = 1;
                        a.next = None;
                        self.gfx.set_rgba(id, a.frames[0].rgba.clone());
                    }
                }
            }
            _ => return false,
        }
        for id in drop_images {
            self.gfx.remove_image(id);
        }
        self.cascade();
        self.prune();
        before != (self.gfx.placements.len(), self.gfx.images.len(), frames(self))
    }

    // -- Animation ---------------------------------------------------------

    fn anim_image(&mut self, cmd: &Cmd) -> Result<(u32, Option<u32>), (Option<u32>, String)> {
        let id = self.target(cmd).filter(|i| self.gfx.images.contains_key(i)).ok_or((None, "ENOENT:Image not found".to_string()))?;
        if !self.anims.contains_key(&id) {
            let rgba = self.gfx.images[&id].rgba.clone();
            self.anims.insert(id, Anim { frames: vec![Frame { rgba, gap: 0 }], cur: 0, state: 1, loops: None, played: 0, next: None });
        }
        Ok((id, (cmd.u(b'I') != 0).then_some(id)))
    }

    fn frame(&mut self, cmd: &Cmd, payload: &[u8], ctx: Ctx) -> Outcome {
        let (id, rep) = self.anim_image(cmd)?;
        let (iw, ih) = (self.gfx.images[&id].width, self.gfx.images[&id].height);
        // A frame may carry no data at all (just its base canvas).
        let (dw, dh, data) = if payload.is_empty() && cmd.ch(b't').unwrap_or(b'd') == b'd' {
            (0, 0, Vec::new())
        } else {
            load(cmd, payload, Some((iw, ih)), ctx.palette).map_err(|e| (rep, e))?
        };
        let (x, y, blend, gap) = (cmd.u(b'x'), cmd.u(b'y'), cmd.u(b'X') != 1, cmd.num(b'z') as i32);
        let extra: usize = self.anims.values().map(Anim::extra_bytes).sum();
        let a = self.anims.get_mut(&id).expect("created above");
        let edit = cmd.u(b'r') as usize;
        let shown = if edit > 0 {
            let f = a.frames.get_mut(edit - 1).ok_or((rep, "ENOENT:Frame not found".to_string()))?;
            let mut canvas = (*f.rgba).clone();
            paste(&mut canvas, iw, ih, &data, (dw, dh), (0, 0, dw, dh), (x, y), blend);
            f.rgba = Arc::new(canvas);
            if gap != 0 {
                f.gap = gap;
            }
            edit - 1
        } else {
            let mut canvas = match cmd.u(b'c') as usize {
                0 => cmd.u(b'Y').to_be_bytes().repeat(iw as usize * ih as usize),
                c => (*a.frames.get(c - 1).ok_or((rep, "ENOENT:Base frame not found".to_string()))?.rgba).clone(),
            };
            if extra + canvas.len() > MAX_FRAME_BYTES {
                return Err((rep, "ENOSPC:Animation frame quota exceeded".into()));
            }
            paste(&mut canvas, iw, ih, &data, (dw, dh), (0, 0, dw, dh), (x, y), blend);
            a.frames.push(Frame { rgba: Arc::new(canvas), gap: if gap == 0 { DEFAULT_GAP } else { gap } });
            if a.state == 2 && a.next.is_none() {
                a.next = Some(ctx.now);
            }
            a.frames.len() - 1
        };
        if shown == a.cur {
            self.gfx.set_rgba(id, a.frames[shown].rgba.clone());
        }
        Ok((rep, None, shown == a.cur, true))
    }

    fn control(&mut self, cmd: &Cmd, ctx: Ctx) -> Outcome {
        let (id, rep) = self.anim_image(cmd)?;
        let a = self.anims.get_mut(&id).expect("created above");
        let notfound = || (rep, "ENOENT:Frame not found".to_string());
        if cmd.u(b'r') > 0 {
            let f = a.frames.get_mut(cmd.u(b'r') as usize - 1).ok_or_else(notfound)?;
            if cmd.num(b'z') != 0 {
                f.gap = cmd.num(b'z') as i32;
            }
        }
        if cmd.u(b'v') > 0 {
            a.loops = if cmd.u(b'v') == 1 { None } else { Some(cmd.u(b'v') - 1) };
        }
        let mut changed = false;
        if cmd.u(b'c') > 0 {
            let c = cmd.u(b'c') as usize - 1;
            if c >= a.frames.len() {
                return Err(notfound());
            }
            changed = a.cur != c;
            a.cur = c;
            self.gfx.set_rgba(id, a.frames[c].rgba.clone());
        }
        match cmd.u(b's') {
            1 => {
                a.state = 1;
                a.next = None;
                a.played = 0;
            }
            s @ (2 | 3) => {
                a.state = s as u8;
                let g = a.frames[a.cur].gap.max(0) as u64;
                a.next = Some(ctx.now + Duration::from_millis(g));
            }
            _ => {}
        }
        Ok((rep, None, changed, false))
    }

    fn compose(&mut self, cmd: &Cmd) -> Outcome {
        let (id, rep) = self.anim_image(cmd)?;
        let (iw, ih) = (self.gfx.images[&id].width, self.gfx.images[&id].height);
        let a = self.anims.get_mut(&id).expect("created above");
        let (src, dst) = (cmd.u(b'r').max(1) as usize - 1, cmd.u(b'c').max(1) as usize - 1);
        if src >= a.frames.len() || dst >= a.frames.len() {
            return Err((rep, "ENOENT:Frame not found".into()));
        }
        let w = if cmd.u(b'w') == 0 { iw } else { cmd.u(b'w') };
        let h = if cmd.u(b'h') == 0 { ih } else { cmd.u(b'h') };
        let (sx, sy, dx, dy) = (cmd.u(b'X'), cmd.u(b'Y'), cmd.u(b'x'), cmd.u(b'y'));
        let fits = |x: u32, y: u32| x.checked_add(w).is_some_and(|e| e <= iw) && y.checked_add(h).is_some_and(|e| e <= ih);
        if !fits(sx, sy) || !fits(dx, dy) {
            return Err((rep, einval("Rectangle out of bounds")));
        }
        if src == dst && sx < dx + w && dx < sx + w && sy < dy + h && dy < sy + h {
            return Err((rep, einval("Overlapping rectangles in the same frame")));
        }
        let from = a.frames[src].rgba.clone();
        let mut canvas = (*a.frames[dst].rgba).clone();
        paste(&mut canvas, iw, ih, &from, (iw, ih), (sx, sy, w, h), (dx, dy), cmd.u(b'C') != 1);
        a.frames[dst].rgba = Arc::new(canvas);
        let shown = dst == a.cur;
        if shown {
            self.gfx.set_rgba(id, a.frames[dst].rgba.clone());
        }
        Ok((rep, None, shown, false))
    }

    /// Advance running animations to `now`. Returns whether a shown frame
    /// changed and when the next frame is due.
    pub fn tick(&mut self, now: Instant) -> (bool, Option<Instant>) {
        let mut changed = false;
        let mut due: Option<Instant> = None;
        for (&id, a) in self.anims.iter_mut() {
            if a.state == 1 || a.frames.len() < 2 {
                continue;
            }
            let Some(mut t) = a.next else { continue };
            if now.saturating_duration_since(t) > Duration::from_secs(1) {
                t = now;
            }
            let start = a.cur;
            let mut steps = 0;
            let mut stopped = false;
            while t <= now {
                steps += 1;
                if steps > 4 * a.frames.len() + 16 {
                    // Nothing but gapless frames: try again later.
                    t = now + Duration::from_millis(DEFAULT_GAP as u64);
                    break;
                }
                let mut idx = a.cur + 1;
                if idx >= a.frames.len() {
                    if a.state == 2 {
                        stopped = true;
                        break;
                    }
                    if let Some(n) = a.loops
                        && a.played >= n
                    {
                        a.state = 1;
                        a.played = 0;
                        stopped = true;
                        break;
                    }
                    a.played += 1;
                    idx = 0;
                }
                a.cur = idx;
                let g = a.frames[idx].gap;
                if g > 0 {
                    t += Duration::from_millis(g as u64);
                }
            }
            a.next = if stopped { None } else { Some(t) };
            if let Some(t) = a.next {
                due = Some(due.map_or(t, |d| d.min(t)));
            }
            if a.cur != start {
                self.gfx.set_rgba(id, a.frames[a.cur].rgba.clone());
                changed = true;
            }
        }
        (changed, due)
    }

    /// When the next animation frame is due.
    pub fn next_tick(&self) -> Option<Instant> {
        self.anims.values().filter(|a| a.state != 1 && a.frames.len() > 1).filter_map(|a| a.next).min()
    }

    /// Current 0-based frame of image `id` (tests / diagnostics).
    pub fn current_frame(&self, id: u32) -> Option<(usize, usize)> {
        self.anims.get(&id).map(|a| (a.cur, a.frames.len()))
    }
}

/// Composite rectangle `(sx, sy, w, h)` of `src` (`sw x sh`) onto `dst`
/// (`dw x dh`) at `(x, y)`, clipped; alpha blended or replacing.
#[allow(clippy::too_many_arguments)]
fn paste(dst: &mut [u8], dw: u32, dh: u32, src: &[u8], (sw, sh): (u32, u32), (sx, sy, w, h): (u32, u32, u32, u32), (x, y): (u32, u32), blend: bool) {
    let w = w.min(sw.saturating_sub(sx)).min(dw.saturating_sub(x));
    let h = h.min(sh.saturating_sub(sy)).min(dh.saturating_sub(y));
    for row in 0..h {
        for col in 0..w {
            let s = (((sy + row) * sw + sx + col) * 4) as usize;
            let d = (((y + row) * dw + x + col) * 4) as usize;
            let (Some(sp), true) = (src.get(s..s + 4), d + 4 <= dst.len()) else { continue };
            if !blend {
                dst[d..d + 4].copy_from_slice(sp);
                continue;
            }
            let sa = sp[3] as u32;
            let da = dst[d + 3] as u32;
            let oa = sa * 255 + da * (255 - sa);
            if oa == 0 {
                dst[d..d + 4].copy_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            for k in 0..3 {
                let v = (sp[k] as u32 * sa * 255 + dst[d + k] as u32 * da * (255 - sa)) / oa;
                dst[d + k] = v as u8;
            }
            dst[d + 3] = (oa / 255) as u8;
        }
    }
}

/// The reply to a feature query (`a=q,e=<action>`): every non-default key we
/// support with that value, plus `e`.
pub fn feature_reply(cmd: &Cmd, e: u8) -> Option<String> {
    let i = cmd.u(b'i');
    if i == 0 || cmd.num(b'q') >= 1 {
        return None;
    }
    if !b"acdfpqtT".contains(&e) {
        return Some(format!("\x1b_Gi={i};OK\x1b\\"));
    }
    let mut keys = vec![format!("e={}", e as char)];
    for (k, v) in &cmd.vals {
        if b"aiImqe".contains(k) {
            continue;
        }
        let ok = match (e, *k, *v) {
            (_, _, Val::Num(0)) => false,
            (b't' | b'T' | b'q' | b'f', b'f', Val::Num(n)) => [8, 16, 24, 32, 100].contains(&n),
            (b't' | b'T' | b'q' | b'f', b't', Val::Char(c)) => c == b'f' || c == b't' || (c == b's' && cfg!(unix)),
            (b't' | b'T' | b'q' | b'f', b'o', Val::Char(c)) => c == b'z',
            (b't' | b'T' | b'q' | b'f', b's' | b'v' | b'S' | b'O' | b'N', Val::Num(_)) => true,
            (b'T' | b'p', b'U' | b'C', Val::Num(n)) => n == 1,
            (b'T' | b'p', b'x' | b'y' | b'w' | b'h' | b'X' | b'Y' | b'c' | b'r' | b'z' | b'P' | b'Q' | b'H' | b'V' | b'p', Val::Num(_)) => true,
            (b'd', b'd', Val::Char(c)) => b"aAcCfFnNiIpPqQrRxXyYzZ".contains(&c),
            (b'd', b'x' | b'y' | b'z' | b'p', Val::Num(_)) => true,
            (b'f', b'x' | b'y' | b'c' | b'r' | b'z' | b'Y', Val::Num(_)) => true,
            (b'f', b'X', Val::Num(n)) => n == 1,
            (b'a', b's', Val::Num(n)) => (1..=3).contains(&n),
            (b'a', b'r' | b'z' | b'c' | b'v', Val::Num(_)) => true,
            (b'c', b'r' | b'c' | b'x' | b'y' | b'w' | b'h' | b'X' | b'Y', Val::Num(_)) => true,
            (b'c', b'C', Val::Num(n)) => n == 1,
            _ => false,
        };
        if ok {
            keys.push(match v {
                Val::Num(n) => format!("{}={n}", *k as char),
                Val::Char(c) => format!("{}={}", *k as char, *c as char),
            });
        }
    }
    Some(format!("\x1b_Gi={i},{};OK\x1b\\", keys.join(",")))
}

// ---------------------------------------------------------------------------
// Unicode placeholders

/// Row / column / id-byte diacritics, in value order (kitty's
/// `gen/rowcolumn-diacritics.txt`).
const DIACRITICS: [u32; 297] = [
    0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F, 0x0346, 0x034A, 0x034B, 0x034C, 0x0350, 0x0351, 0x0352, 0x0357, 0x035B, 0x0363,
    0x0364, 0x0365, 0x0366, 0x0367, 0x0368, 0x0369, 0x036A, 0x036B, 0x036C, 0x036D, 0x036E, 0x036F, 0x0483, 0x0484, 0x0485, 0x0486, 0x0487, 0x0592,
    0x0593, 0x0594, 0x0595, 0x0597, 0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1, 0x05A8, 0x05A9, 0x05AB, 0x05AC, 0x05AF, 0x05C4,
    0x0610, 0x0611, 0x0612, 0x0613, 0x0614, 0x0615, 0x0616, 0x0617, 0x0657, 0x0658, 0x0659, 0x065A, 0x065B, 0x065D, 0x065E, 0x06D6, 0x06D7, 0x06D8,
    0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0, 0x06E1, 0x06E2, 0x06E4, 0x06E7, 0x06E8, 0x06EB, 0x06EC, 0x0730, 0x0732, 0x0733, 0x0735, 0x0736,
    0x073A, 0x073D, 0x073F, 0x0740, 0x0741, 0x0743, 0x0745, 0x0747, 0x0749, 0x074A, 0x07EB, 0x07EC, 0x07ED, 0x07EE, 0x07EF, 0x07F0, 0x07F1, 0x07F3,
    0x0816, 0x0817, 0x0818, 0x0819, 0x081B, 0x081C, 0x081D, 0x081E, 0x081F, 0x0820, 0x0821, 0x0822, 0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A,
    0x082B, 0x082C, 0x082D, 0x0951, 0x0953, 0x0954, 0x0F82, 0x0F83, 0x0F86, 0x0F87, 0x135D, 0x135E, 0x135F, 0x17DD, 0x193A, 0x1A17, 0x1A75, 0x1A76,
    0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C, 0x1B6B, 0x1B6D, 0x1B6E, 0x1B6F, 0x1B70, 0x1B71, 0x1B72, 0x1B73, 0x1CD0, 0x1CD1, 0x1CD2, 0x1CDA,
    0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4, 0x1DC5, 0x1DC6, 0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1, 0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5,
    0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9, 0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF, 0x1DE0, 0x1DE1, 0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE,
    0x20D0, 0x20D1, 0x20D4, 0x20D5, 0x20D6, 0x20D7, 0x20DB, 0x20DC, 0x20E1, 0x20E7, 0x20E9, 0x20F0, 0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2,
    0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6, 0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA, 0x2DEB, 0x2DEC, 0x2DED, 0x2DEE, 0x2DEF, 0x2DF0, 0x2DF1, 0x2DF2, 0x2DF3, 0x2DF4,
    0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA, 0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D, 0xA6F0, 0xA6F1, 0xA8E0, 0xA8E1,
    0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5, 0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9, 0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED, 0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2,
    0xAAB3, 0xAAB7, 0xAAB8, 0xAABE, 0xAABF, 0xAAC1, 0xFE20, 0xFE21, 0xFE22, 0xFE23, 0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38, 0x1D185, 0x1D186,
    0x1D187, 0x1D188, 0x1D189, 0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243, 0x1D244,
];

/// The number a placeholder diacritic stands for.
pub fn diacritic_value(c: char) -> Option<u32> {
    DIACRITICS.binary_search(&(c as u32)).ok().map(|i| i as u32)
}

/// The diacritic for `n` (0-296).
pub fn diacritic(n: u32) -> Option<char> {
    DIACRITICS.get(n as usize).and_then(|&c| char::from_u32(c))
}

/// Image id bits a colour carries (256-colour index or 24-bit RGB).
pub fn color_id(c: Color) -> Option<u32> {
    match c {
        Color::Indexed(n) => Some(n as u32),
        Color::Rgb(r, g, b) => Some((r as u32) << 16 | (g as u32) << 8 | b as u32),
        Color::Default => None,
    }
}

/// One placeholder cell, decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ph {
    x: usize,
    fg: u32,
    ul: u32,
    id: u32,
    row: u32,
    col: u32,
    msb: u32,
}

/// Decode the placeholders of one row, inferring omitted diacritics from the
/// cell to the left as the protocol describes.
fn row_placeholders(cells: &[Cell]) -> Vec<Ph> {
    let mut out: Vec<Ph> = Vec::new();
    let mut prev: Option<Ph> = None;
    for (x, c) in cells.iter().enumerate() {
        if c.ch != PLACEHOLDER {
            prev = None;
            continue;
        }
        let Some(fg) = color_id(c.style.fg) else {
            prev = None;
            continue;
        };
        let ul = color_id(c.style.ul_color).unwrap_or(0);
        let d: Vec<u32> = c.comb.iter().filter(|m| **m != '\0').filter_map(|m| diacritic_value(*m)).collect();
        let same = prev.filter(|p| p.x + 1 == x && p.fg == fg && p.ul == ul);
        let (row, col, msb) = match d.as_slice() {
            [] => same.map_or((0, 0, 0), |p| (p.row, p.col + 1, p.msb)),
            [r] => same.filter(|p| p.row == *r).map_or((*r, 0, 0), |p| (*r, p.col + 1, p.msb)),
            [r, c] => (*r, *c, same.filter(|p| p.row == *r && p.col + 1 == *c).map_or(0, |p| p.msb)),
            [r, c, m, ..] => (*r, *c, *m),
        };
        let ph = Ph { x, fg, ul, id: fg | (msb & 0xff) << 24, row, col, msb };
        out.push(ph);
        prev = Some(ph);
    }
    out
}

impl KittyGraphics {
    /// The virtual placement a placeholder refers to.
    fn virtual_for(&self, id: u32, pid: u32) -> Option<&Placement> {
        let v = self.gfx.placements.iter().filter(|p| p.virt && p.parent.is_none() && p.image_id == id);
        if pid != 0 { v.into_iter().find(|p| p.id == pid) } else { v.max_by_key(|p| p.stamp) }
    }

    /// Draws for the Unicode placeholders on view rows `0..view_rows`; `row(i)`
    /// returns the cells of view row `i` when it is a live screen row (the
    /// scrollback keeps no colours, so placeholders there are not drawn), plus
    /// the relative placements hanging off the virtual placements shown.
    pub fn placeholder_draws<'c>(&self, view_rows: usize, row: &dyn Fn(usize) -> Option<&'c [Cell]>, cell: (u32, u32)) -> Vec<Draw> {
        let mut draws = Vec::new();
        if !self.gfx.placements.iter().any(|p| p.virt) {
            return draws;
        }
        let (cw, ch) = (cell.0.max(1) as f64, cell.1.max(1) as f64);
        let mut origin: HashMap<(u32, u32), (i64, i64)> = HashMap::new();
        for y in 0..view_rows {
            let Some(cells) = row(y) else { continue };
            let phs = row_placeholders(cells);
            let mut i = 0;
            while i < phs.len() {
                // A run: adjacent cells of one placement, consecutive columns.
                let mut j = i + 1;
                while j < phs.len() {
                    let (a, b) = (phs[j - 1], phs[j]);
                    if b.x != a.x + 1 || b.id != a.id || b.ul != a.ul || b.row != a.row || b.col != a.col + 1 {
                        break;
                    }
                    j += 1;
                }
                let (first, n) = (phs[i], (j - i) as u32);
                i = j;
                let Some(p) = self.virtual_for(first.id, first.ul) else { continue };
                if !self.gfx.images.contains_key(&p.image_id) {
                    continue;
                }
                let key = (p.image_id, p.id);
                let o = (y as i64 - first.row as i64, first.x as i64 - first.col as i64);
                let e = origin.entry(key).or_insert(o);
                *e = (e.0.min(o.0), e.1.min(o.1));
                if first.row >= p.rows || first.col >= p.cols {
                    continue;
                }
                let n = n.min(p.cols - first.col);
                // The image fitted (aspect kept, centred) into the cols x rows box.
                let (bw, bh) = (p.cols as f64 * cw, p.rows as f64 * ch);
                let (sw, sh) = (p.src.2 as f64, p.src.3 as f64);
                let s = (bw / sw).min(bh / sh);
                let (dw, dh) = (sw * s, sh * s);
                let (ox, oy) = ((bw - dw) / 2.0, (bh - dh) / 2.0);
                let (cx0, cx1) = (first.col as f64 * cw, (first.col + n) as f64 * cw);
                let (cy0, cy1) = (first.row as f64 * ch, (first.row + 1) as f64 * ch);
                let (ix0, ix1) = (cx0.max(ox).round(), cx1.min(ox + dw).round());
                let (iy0, iy1) = (cy0.max(oy).round(), cy1.min(oy + dh).round());
                if ix1 <= ix0 || iy1 <= iy0 {
                    continue;
                }
                let sx0 = ((ix0 - ox) / s).floor().max(0.0);
                let sy0 = ((iy0 - oy) / s).floor().max(0.0);
                let sx1 = ((ix1 - ox) / s).ceil().min(sw);
                let sy1 = ((iy1 - oy) / s).ceil().min(sh);
                draws.push(Draw {
                    image_id: p.image_id,
                    src: (p.src.0 + sx0 as u32, p.src.1 + sy0 as u32, ((sx1 - sx0) as u32).max(1), ((sy1 - sy0) as u32).max(1)),
                    row: y as i64,
                    col: first.x as i64,
                    x_off: (ix0 - cx0) as i64,
                    y_off: (iy0 - cy0) as i64,
                    width: (ix1 - ix0) as u32,
                    height: (iy1 - iy0) as u32,
                    z: p.z,
                });
            }
        }
        // Relative placements whose (chain of) parent is a shown virtual placement.
        let mut frontier: Vec<(u32, u32)> = origin.keys().copied().collect();
        let mut depth = 0;
        while !frontier.is_empty() && depth < MAX_CHAIN {
            depth += 1;
            let mut next = Vec::new();
            for key in frontier {
                let (r0, c0) = origin[&key];
                for p in self.gfx.placements.iter().filter(|p| p.virt && p.parent == Some(key)) {
                    let k = (p.image_id, p.id);
                    if origin.contains_key(&k) || !self.gfx.images.contains_key(&p.image_id) {
                        continue;
                    }
                    let (r, c) = (r0 + p.rel.1 as i64, c0 + p.rel.0 as i64);
                    origin.insert(k, (r, c));
                    next.push(k);
                    let d = p.draw_rect(cell);
                    draws.push(Draw {
                        image_id: p.image_id,
                        src: p.src,
                        row: r,
                        col: c,
                        x_off: d.0 as i64,
                        y_off: d.1 as i64,
                        width: d.2,
                        height: d.3,
                        z: p.z,
                    });
                }
            }
            frontier = next;
        }
        draws.sort_by_key(|d| (d.z, d.image_id));
        draws
    }
}
