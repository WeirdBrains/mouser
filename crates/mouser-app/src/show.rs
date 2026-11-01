//! `mouser show`: display images in the terminal (docs/protocol/graphics.md).
//!
//! Fuzzball graphics come first; terminals that only answer the Kitty graphics
//! query get Kitty graphics instead. Inside tmux (with `allow-passthrough on`)
//! images go through passthrough as Kitty Unicode placeholders when the outer
//! terminal speaks Kitty graphics, since placeholders are text that tmux
//! moves and redraws itself; otherwise Fuzzball is passed through.
//!
//! The pure parts (capability parsing, size fitting, message encoding) are
//! separate from terminal I/O so they can be tested against Mouser's parser.
//! Video and audio are not supported.
#![cfg_attr(not(unix), allow(unused_imports, dead_code))]

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// Which graphics protocol to speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proto {
    Fuzzball,
    /// Kitty graphics placed at the cursor.
    Kitty,
    /// Kitty graphics shown through Unicode placeholders (tmux).
    KittyPlaceholders,
}

/// Image id of the Kitty support query.
pub const KITTY_QUERY_ID: u32 = 31;
/// Largest placeholder grid (the diacritic table has 297 entries).
pub const MAX_PLACEHOLDER_CELLS: u32 = 297;

/// What the terminal told us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermInfo {
    pub proto: Proto,
    pub max_chunk: usize,
    pub g_mem: usize,
    pub g_max_dim: u32,
    pub png: bool,
    /// Cell size in pixels.
    pub cell: (u32, u32),
    /// Screen width in columns and the cursor's 0-based column.
    pub cols: u32,
    pub cursor_col: u32,
    /// Wrap messages for tmux passthrough.
    pub tmux: bool,
    /// Fuzzball `g_fit` lists `w`: placements can fill the width (`fit=w`).
    pub fit_w: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Width / height in cells; the other side follows the aspect ratio.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Natural size (one image pixel per screen pixel) instead of fitting the width.
    pub original: bool,
    /// Image id to use (default: derived from the process id).
    pub id: Option<u32>,
}

/// Whether the replies hold an OK to the Kitty graphics query.
pub fn kitty_answered(r: &str) -> bool {
    r.contains(&format!("\x1b_Gi={KITTY_QUERY_ID};OK"))
}

/// Whether the replies hold a Fuzzball capability reply with graphics.
pub fn fuzzball_answered(r: &str) -> bool {
    r.split("Fzcr;")
        .nth(1)
        .and_then(|c| c.split('\x1b').next())
        .is_some_and(|c| c.split(',').any(|kv| kv.strip_prefix("sub=").is_some_and(|s| s.contains('g'))))
}

/// The queries `mouser show` sends: Fuzzball capabilities, Kitty graphics
/// support, cell size, screen size, cursor position, then DA1.
pub fn query(tmux: bool) -> String {
    message("cq;id=1", tmux) + &kitty_apc(&format!("i={KITTY_QUERY_ID},s=1,v=1,a=q,t=d,f=24"), "AAAA", tmux) + "\x1b[16t\x1b[18t\x1b[6n\x1b[c"
}

/// Parse terminal replies: the Fuzzball `cq` reply (preferred), the Kitty
/// query reply, `CSI 6;h;w t` (cell size), `CSI 8;rows;cols t` and the cursor
/// report `CSI row;col R`. `None` when neither protocol answered. Inside tmux
/// a Kitty-capable terminal gets placeholders.
pub fn parse_replies(r: &str, tmux: bool) -> Option<TermInfo> {
    let proto = match (fuzzball_answered(r), kitty_answered(r)) {
        (_, true) if tmux => Proto::KittyPlaceholders,
        (true, _) => Proto::Fuzzball,
        (false, true) => Proto::Kitty,
        (false, false) => return None,
    };
    let caps = r.split("Fzcr;").nth(1).and_then(|c| c.split('\x1b').next()).unwrap_or("");
    let get = |k: &str| caps.split(',').find_map(|kv| kv.strip_prefix(k).and_then(|v| v.strip_prefix('=')));
    let fz = proto == Proto::Fuzzball;
    let csi = |end: char, lead: Option<&str>| -> Option<Vec<u32>> {
        r.split("\x1b[").skip(1).find_map(|s| {
            let body = &s[..s.find(end)?];
            let nums: Vec<u32> = body.split(';').map(|n| n.parse().ok()).collect::<Option<_>>()?;
            match lead {
                Some(l) => (nums.first()? == &l.parse::<u32>().ok()?).then_some(nums),
                None => Some(nums),
            }
        })
    };
    let cell = csi('t', Some("6")).filter(|v| v.len() == 3).map(|v| (v[2], v[1])).unwrap_or((10, 20));
    let cols = csi('t', Some("8")).filter(|v| v.len() == 3).map(|v| v[2]).unwrap_or(80);
    let cursor_col = csi('R', None).filter(|v| v.len() == 2).map(|v| v[1].saturating_sub(1)).unwrap_or(0);
    // Kitty has no capability report: its chunk size is fixed at 4096 and
    // Mouser's (and kitty's) limits are used.
    Some(TermInfo {
        proto,
        max_chunk: if fz { get("max_chunk").and_then(|v| v.parse().ok()).unwrap_or(4096) } else { 4096 },
        g_mem: if fz { get("g_mem").and_then(|v| v.parse().ok()).unwrap_or(64 << 20) } else { 320 << 20 },
        g_max_dim: if fz { get("g_max_dim").and_then(|v| v.parse().ok()).unwrap_or(4096) } else { 10_000 },
        png: !fz || get("g_fmt").is_some_and(|f| f.split(':').any(|x| x == "png")),
        cell,
        cols: cols.max(1),
        cursor_col,
        tmux,
        fit_w: fz && get("g_fit").is_some_and(|f| f.split(':').any(|x| x == "w")),
    })
}

/// Whether the image fits the terminal width (no `--width`, `--height` or
/// `--original`): it is shown from column 0 across the whole width.
pub fn fills(o: &Options) -> bool {
    o.width.is_none() && o.height.is_none() && !o.original
}

/// Cells for a `w` x `h` pixel image. Aspect ratio is kept; by default the image
/// is scaled up or down to the columns right of the cursor (the whole width once
/// the cursor is at column 0); `--original` keeps one image pixel per screen pixel.
pub fn fit_cells(w: u32, h: u32, t: &TermInfo, o: &Options) -> (u32, u32) {
    let (cw, ch) = (t.cell.0.max(1) as f64, t.cell.1.max(1) as f64);
    let (w, h) = (w.max(1) as f64, h.max(1) as f64);
    // Rows for a given column count (and vice versa), keeping the pixel aspect.
    let rows_for = |c: f64| (c * cw * h / w / ch).ceil().max(1.0);
    let cols_for = |r: f64| (r * ch * w / h / cw).ceil().max(1.0);
    let avail = t.cols.saturating_sub(t.cursor_col).max(1) as f64;
    let (c, r) = match (o.width, o.height) {
        (Some(c), Some(r)) => {
            // Fit inside the box.
            let c = (c as f64).min(cols_for(r as f64));
            (c, rows_for(c).min(r as f64))
        }
        (Some(c), None) => (c as f64, rows_for(c as f64)),
        (None, Some(r)) => (cols_for(r as f64), r as f64),
        (None, None) => {
            let c = if o.original { (w / cw).ceil().max(1.0) } else { avail };
            (c, rows_for(c))
        }
    };
    let max = if t.proto == Proto::KittyPlaceholders { MAX_PLACEHOLDER_CELLS as f64 } else { 1000.0 };
    if c > max || r > max {
        // Shrink both, keeping the aspect.
        let k = (max / c).min(max / r);
        return (((c * k).floor().max(1.0)) as u32, ((r * k).floor().max(1.0)) as u32);
    }
    (c as u32, r as u32)
}

/// An image ready to upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    pub fmt: &'static str,
    pub w: u32,
    pub h: u32,
    pub bytes: Vec<u8>,
}

fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    (b.len() >= 24 && b.starts_with(b"\x89PNG\r\n\x1a\n") && &b[12..16] == b"IHDR")
        .then(|| (u32::from_be_bytes(b[16..20].try_into().unwrap()), u32::from_be_bytes(b[20..24].try_into().unwrap())))
}

/// Decode `data` (PNG, JPEG, GIF first frame, WebP, BMP). PNG is sent as-is when
/// the terminal takes PNG and it is within its limits; otherwise RGBA, scaled down
/// to `g_max_dim` and `g_mem`.
pub fn prepare(data: &[u8], t: &TermInfo) -> Result<Payload> {
    if let Some((w, h)) = png_size(data)
        && t.png
        && w <= t.g_max_dim
        && h <= t.g_max_dim
        && (w as usize * h as usize * 4) <= t.g_mem
    {
        return Ok(Payload { fmt: "png", w, h, bytes: data.to_vec() });
    }
    let fmt =
        image::guess_format(data).map_err(|_| anyhow::anyhow!("unsupported format (supported: PNG, JPEG, GIF, WebP, BMP; no video or audio)"))?;
    if !matches!(
        fmt,
        image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::Gif | image::ImageFormat::WebP | image::ImageFormat::Bmp
    ) {
        bail!("unsupported format {fmt:?} (supported: PNG, JPEG, GIF, WebP, BMP; no video or audio)");
    }
    let mut img = image::load_from_memory_with_format(data, fmt).context("decoding image")?;
    let max = t.g_max_dim.max(1);
    let budget = (t.g_mem / 4).max(1) as f64;
    let (w, h) = (img.width(), img.height());
    let k = (max as f64 / w.max(h) as f64).min((budget / (w as f64 * h as f64)).sqrt()).min(1.0);
    if k < 1.0 {
        let (nw, nh) = (((w as f64 * k) as u32).max(1), ((h as f64 * k) as u32).max(1));
        img = img.resize_exact(nw, nh, image::imageops::FilterType::Triangle);
    }
    let rgba = img.to_rgba8();
    Ok(Payload { fmt: "rgba", w: rgba.width(), h: rgba.height(), bytes: rgba.into_raw() })
}

fn b64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() { A[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

/// One Fuzzball message, wrapped for tmux passthrough if needed.
pub fn message(body: &str, tmux: bool) -> String {
    let m = format!("\x1bP=Fz{body}\x1b\\");
    if tmux { format!("\x1bPtmux;{}\x1b\\", m.replace('\x1b', "\x1b\x1b")) } else { m }
}

/// One Kitty graphics command, wrapped for tmux passthrough if needed.
pub fn kitty_apc(keys: &str, payload: &str, tmux: bool) -> String {
    let m = if payload.is_empty() { format!("\x1b_G{keys}\x1b\\") } else { format!("\x1b_G{keys};{payload}\x1b\\") };
    if tmux { format!("\x1bPtmux;{}\x1b\\", m.replace('\x1b', "\x1b\x1b")) } else { m }
}

/// Kitty: upload `p` as image `id` in 4096-character chunks with `first`'s keys
/// on the first chunk.
fn kitty_upload(p: &Payload, first: &str, tmux: bool) -> String {
    let data = b64(&p.bytes);
    let parts: Vec<&str> = if data.is_empty() { vec![""] } else { data.as_bytes().chunks(4096).map(|c| std::str::from_utf8(c).unwrap()).collect() };
    let f = if p.fmt == "png" { 100 } else { 32 };
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        let m = u8::from(i + 1 < parts.len());
        let keys = if i == 0 { format!("{first},f={f},s={},v={},m={m}", p.w, p.h) } else { format!("m={m}") };
        out += &kitty_apc(&keys, part, tmux);
    }
    out
}

/// Kitty: transmit and place at the cursor in `cols` x `rows` cells (errors
/// only are reported), then continue below the image.
pub fn encode_kitty(p: &Payload, id: u32, cols: u32, rows: u32, tmux: bool) -> String {
    kitty_upload(p, &format!("a=T,i={id},c={cols},r={rows},q=1"), tmux) + "\r\n"
}

/// Kitty through Unicode placeholders: transmit quietly, create a virtual
/// placement, then print `rows` lines of `cols` placeholder cells. The id's
/// low byte goes in a 256-colour foreground (tmux turns true colour into 256
/// colours for terminals it does not know take RGB) and its high byte in the
/// third diacritic; use [`placeholder_id`] ids.
pub fn encode_placeholders(p: &Payload, id: u32, cols: u32, rows: u32, tmux: bool) -> String {
    use mouser_core::kitty_graphics::{PLACEHOLDER, diacritic};
    let mut out = kitty_upload(p, &format!("a=t,i={id},q=2"), tmux);
    out += &kitty_apc(&format!("a=p,U=1,i={id},c={cols},r={rows},q=2"), "", tmux);
    let (lo, hi) = (id & 0xff, id >> 24);
    for row in 0..rows {
        out += &format!("\x1b[38;5;{lo}m");
        for col in 0..cols {
            out.push(PLACEHOLDER);
            out.extend(diacritic(row).into_iter().chain(diacritic(col)).chain(diacritic(hi)));
        }
        out += "\x1b[39m\r\n";
    }
    out
}

/// An id placeholders can carry through a 256-colour foreground: low byte
/// 1-255, high byte 0-255, nothing in between.
pub fn placeholder_id(seed: u32) -> u32 {
    let lo = 1 + seed % 255;
    let hi = (seed / 255) % 256;
    hi << 24 | lo
}

/// Bytes that upload `p` as image `id` in chunks of at most `max_chunk` base64
/// characters, place it at the cursor in `cols` x `rows` cells and move the
/// cursor to the start of the line below it. Only the last chunk asks for a
/// success reply; errors are always reported. `fill` asks the terminal to keep
/// the placement as wide as the screen (`fit=w`), also after a resize.
pub fn encode(p: &Payload, id: u32, cols: u32, rows: u32, fill: bool, t: &TermInfo) -> String {
    let data = b64(&p.bytes);
    let chunk = (t.max_chunk.min(1 << 20) / 4 * 4).max(4);
    let mut out = String::new();
    let parts: Vec<&str> = if data.is_empty() { vec![""] } else { data.as_bytes().chunks(chunk).map(|c| std::str::from_utf8(c).unwrap()).collect() };
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let head = if i == 0 { format!("id={id},fmt={},w={},h={},size={},enc=b64,", p.fmt, p.w, p.h, p.bytes.len()) } else { format!("id={id},") };
        let tail = if last { "quiet=0" } else { "more=1,quiet=1" };
        out += &message(&format!("gu;{head}len={},{tail}:{part}", part.len()), t.tmux);
    }
    let fit = if fill { ",fit=w" } else { "" };
    out += &message(&format!("gp;id={id},pid=1,cols={cols},rows={rows}{fit},quiet=1"), t.tmux);
    // `move=1` left the cursor on the image's last row; continue below it.
    out += "\r\n";
    out
}

/// Errors in Fuzzball (`ok=0`) and Kitty (`ECODE:msg`) replies, as `ERR: msg`.
pub fn reply_errors(r: &str) -> Vec<String> {
    let kitty = r.split("\x1b_G").skip(1).filter_map(|s| {
        let msg = s.split('\x1b').next()?.split_once(';')?.1;
        (msg != "OK").then(|| match msg.split_once(':') {
            Some((code, m)) => format!("{code}: {m}"),
            None => msg.to_string(),
        })
    });
    let fuzz: Vec<String> = r
        .split("Fzgr;")
        .skip(1)
        .filter_map(|s| {
            let body = s.split('\x1b').next()?;
            body.contains("ok=0").then(|| {
                let f = |k: &str| body.split(',').find_map(|kv| kv.strip_prefix(k)).unwrap_or("");
                format!("{}: {}", f("err="), f("msg="))
            })
        })
        .collect();
    fuzz.into_iter().chain(kitty).collect()
}

#[cfg(unix)]
mod tty {
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    pub struct Tty {
        pub out: File,
        rx: mpsc::Receiver<Vec<u8>>,
        saved: String,
    }

    impl Tty {
        pub fn open() -> Result<Self> {
            let out = OpenOptions::new().read(true).write(true).open("/dev/tty").context("no terminal (/dev/tty)")?;
            let stty = |args: &[&str]| Command::new("stty").args(args).stdin(Stdio::from(out.try_clone().unwrap())).output();
            let saved = String::from_utf8_lossy(&stty(&["-g"]).context("stty")?.stdout).trim().to_string();
            stty(&["raw", "-echo"]).context("stty raw")?;
            let mut input = out.try_clone()?;
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = input.read(&mut buf) {
                    if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            });
            Ok(Self { out, rx, saved })
        }

        /// Read until `done` matches or `timeout` passes.
        pub fn read_until(&self, timeout: Duration, done: impl Fn(&str) -> bool) -> String {
            let end = Instant::now() + timeout;
            let mut s = String::new();
            while let Some(left) = end.checked_duration_since(Instant::now()) {
                match self.rx.recv_timeout(left) {
                    Ok(b) => {
                        s += &String::from_utf8_lossy(&b);
                        if done(&s) {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            s
        }
    }

    impl Drop for Tty {
        fn drop(&mut self) {
            if let Ok(f) = self.out.try_clone() {
                let _ = Command::new("stty").arg(&self.saved).stdin(Stdio::from(f)).status();
            }
        }
    }
}

/// DA1 reply (`ESC [ ? … c`) ends the capability exchange.
fn has_da1(s: &str) -> bool {
    s.split("\x1b[?").skip(1).any(|t| t.bytes().find(|b| !b.is_ascii_digit() && *b != b';') == Some(b'c'))
}

/// Run `mouser show`.
#[cfg(unix)]
pub fn run(files: &[String], o: &Options) -> Result<()> {
    let tmux = std::env::var_os("TMUX").is_some();
    let mut tty = tty::Tty::open()?;
    tty.out.write_all(query(tmux).as_bytes())?;
    // A terminal answers in order, so DA1 ends the exchange. tmux answers DA1
    // itself and forwards the outer terminal's replies later: wait for both.
    let replies = if tmux {
        tty.read_until(Duration::from_millis(1500), |s| fuzzball_answered(s) && kitty_answered(s))
    } else {
        tty.read_until(Duration::from_millis(1500), has_da1)
    };
    let Some(mut t) = parse_replies(&replies, tmux) else {
        if tmux {
            bail!(
                "no graphics reply through tmux; enable passthrough with `tmux set -g allow-passthrough on` \
                 (the outer terminal must support Fuzzball or Kitty graphics)"
            );
        }
        bail!("this terminal supports neither Fuzzball nor Kitty graphics (no reply to either query)");
    };
    // Placeholder ids travel in a 24-bit colour; keep every id below 2^24 so
    // one scheme fits all protocols.
    let base = o.id.unwrap_or(0x10_0000 | (std::process::id() & 0xffff) << 4) & 0xff_ffff;
    let mut failed = false;
    for (i, f) in files.iter().enumerate() {
        let data = if f == "-" {
            let mut v = Vec::new();
            std::io::stdin().read_to_end(&mut v).context("reading stdin")?;
            v
        } else {
            std::fs::read(f).with_context(|| format!("reading {f}"))?
        };
        let fill = fills(o);
        if fill && t.cursor_col > 0 {
            // Fitting the width starts at column 0.
            tty.out.write_all(b"\r\n")?;
            t.cursor_col = 0;
        }
        let res = prepare(&data, &t).and_then(|p| {
            let (cols, rows) = fit_cells(p.w, p.h, &t, o);
            // Too tall to fill the width (over the row cap): fixed size instead.
            let fill = fill && cols == t.cols && t.fit_w;
            let id = base.wrapping_add(i as u32).max(1);
            let bytes = match t.proto {
                Proto::Fuzzball => encode(&p, id, cols, rows, fill, &t),
                Proto::Kitty => encode_kitty(&p, id, cols, rows, tmux),
                Proto::KittyPlaceholders => encode_placeholders(&p, placeholder_id(id), cols, rows, tmux),
            };
            tty.out.write_all(bytes.as_bytes())?;
            let r = match t.proto {
                Proto::Fuzzball => tty.read_until(Duration::from_secs(5), |s| s.contains("Fzgr;")),
                // Kitty only reports errors here (q=1), or nothing (q=2).
                Proto::Kitty => tty.read_until(Duration::from_millis(300), |s| s.contains("\x1b_G")),
                Proto::KittyPlaceholders => String::new(),
            };
            if let Some(e) = reply_errors(&r).first() {
                bail!("terminal rejected the image ({e})");
            }
            Ok(())
        });
        if let Err(e) = res {
            drop(tty);
            eprintln!("mouser show: {f}: {e:#}");
            failed = true;
            tty = tty::Tty::open()?;
        }
        t.cursor_col = 0;
    }
    drop(tty);
    if failed { bail!("some files could not be shown") } else { Ok(()) }
}

#[cfg(not(unix))]
pub fn run(_files: &[String], _o: &Options) -> Result<()> {
    bail!("mouser show is only supported on Unix-like systems for now")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term() -> TermInfo {
        TermInfo {
            proto: Proto::Fuzzball,
            max_chunk: 8,
            g_mem: 1 << 20,
            g_max_dim: 100,
            png: true,
            cell: (10, 20),
            cols: 80,
            cursor_col: 0,
            tmux: false,
            fit_w: true,
        }
    }

    #[test]
    fn parses_caps_cell_size_cols_and_cursor() {
        let r = "\x1bP=Fzcr;id=1,v=0,sub=gk,raw=0,max_chunk=65536,g_mem=335544320,g_max_dim=10000,g_fmt=rgba:rgb:png\x1b\\\x1b[6;19;10t\x1b[8;24;87t\x1b[3;5R\x1b[?62;22c";
        let t = parse_replies(r, false).unwrap();
        assert_eq!(
            t,
            TermInfo {
                proto: Proto::Fuzzball,
                max_chunk: 65536,
                g_mem: 335544320,
                g_max_dim: 10000,
                png: true,
                cell: (10, 19),
                cols: 87,
                cursor_col: 4,
                tmux: false,
                fit_w: false,
            }
        );
        assert!(parse_replies("\x1b[?62;22c", false).is_none());
        let fit = r.replace("g_fmt=rgba:rgb:png", "g_fmt=rgba:rgb:png,g_fit=w,g_reflow=1");
        assert!(parse_replies(&fit, false).unwrap().fit_w, "g_fit=w advertised");
        // Both answered: Fuzzball wins; inside tmux, Kitty placeholders.
        let both = format!("{r}\x1b_Gi=31;OK\x1b\\");
        assert_eq!(parse_replies(&both, false).unwrap().proto, Proto::Fuzzball);
        assert_eq!(parse_replies(&both, true).unwrap().proto, Proto::KittyPlaceholders);
        let kitty = parse_replies("\x1b_Gi=31;OK\x1b\\\x1b[6;20;10t\x1b[8;24;80t\x1b[?62c", false).unwrap();
        assert_eq!((kitty.proto, kitty.cell, kitty.max_chunk, kitty.png), (Proto::Kitty, (10, 20), 4096, true));
        assert!(parse_replies("\x1b_Gi=31;EINVAL:no\x1b\\\x1b[?62c", false).is_none());
        assert!(has_da1(r) && !has_da1("\x1b[6;19;10t"));
    }

    #[test]
    fn fits_width_keeps_aspect() {
        assert_eq!(term().cols % 4, 0, "the assertions below assume this");
        let t = term();
        // 400x200 px image, 10x20 cells: natural 40x10 cells.
        // Scaled up to the whole width.
        assert_eq!(fit_cells(400, 200, &t, &Options::default()), (t.cols, t.cols / 4));
        let narrow = TermInfo { cols: 30, cursor_col: 10, ..t.clone() };
        assert_eq!(fit_cells(400, 200, &narrow, &Options::default()), (20, 5));
        assert_eq!(fit_cells(400, 200, &narrow, &Options { original: true, ..Default::default() }), (40, 10));
        assert_eq!(fit_cells(400, 200, &t, &Options { width: Some(20), ..Default::default() }), (20, 5));
        assert_eq!(fit_cells(400, 200, &t, &Options { height: Some(5), ..Default::default() }), (20, 5));
        assert_eq!(fit_cells(400, 200, &t, &Options { width: Some(20), height: Some(2), ..Default::default() }), (8, 2));
        assert_eq!(fit_cells(1, 1, &t, &Options::default()), (t.cols, t.cols / 2));
        assert!(fills(&Options::default()) && !fills(&Options { original: true, ..Default::default() }));
        assert!(!fills(&Options { width: Some(3), ..Default::default() }));
    }

    #[test]
    fn encodes_chunked_upload_and_place() {
        let p = Payload { fmt: "rgba", w: 2, h: 1, bytes: vec![255, 0, 0, 255, 0, 0, 255, 255] };
        let s = encode(&p, 7, 4, 2, false, &term());
        // 12 base64 chars in chunks of 8.
        assert_eq!(
            s,
            "\x1bP=Fzgu;id=7,fmt=rgba,w=2,h=1,size=8,enc=b64,len=8,more=1,quiet=1:/wAA/wAA\x1b\\\
             \x1bP=Fzgu;id=7,len=4,quiet=0://8=\x1b\\\
             \x1bP=Fzgp;id=7,pid=1,cols=4,rows=2,quiet=1\x1b\\\r\n"
        );
    }

    #[test]
    fn tmux_wrapping_doubles_escapes() {
        assert_eq!(message("cq;id=1", true), "\x1bPtmux;\x1b\x1bP=Fzcq;id=1\x1b\x1b\\\x1b\\");
    }

    #[test]
    fn png_passes_through_and_others_decode() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let p = prepare(&png, &term()).unwrap();
        assert_eq!((p.fmt, p.w, p.h, p.bytes == png), ("png", 3, 2, true));
        let no_png = TermInfo { png: false, ..term() };
        let p = prepare(&png, &no_png).unwrap();
        assert_eq!((p.fmt, p.w, p.h, p.bytes.len()), ("rgba", 3, 2, 24));
        let mut bmp = Vec::new();
        image::RgbaImage::from_pixel(300, 150, image::Rgba([9, 9, 9, 255]))
            .write_to(&mut std::io::Cursor::new(&mut bmp), image::ImageFormat::Bmp)
            .unwrap();
        let p = prepare(&bmp, &term()).unwrap();
        assert_eq!((p.fmt, p.w, p.h), ("rgba", 100, 50), "scaled to g_max_dim");
        assert!(prepare(b"not an image", &term()).unwrap_err().to_string().contains("unsupported"));
    }

    #[test]
    fn kitty_encodings() {
        let p = Payload { fmt: "rgba", w: 1, h: 1, bytes: vec![1, 2, 3, 4] };
        assert_eq!(encode_kitty(&p, 9, 2, 1, false), "\x1b_Ga=T,i=9,c=2,r=1,q=1,f=32,s=1,v=1,m=0;AQIDBA==\x1b\\\r\n");
        assert_eq!(kitty_apc("a=p,i=1", "", true), "\x1bPtmux;\x1b\x1b_Ga=p,i=1\x1b\x1b\\\x1b\\");
        let big = Payload { fmt: "rgba", w: 64, h: 64, bytes: vec![0; 64 * 64 * 4] };
        let s = encode_kitty(&big, 3, 4, 4, false);
        assert_eq!(s.matches("\x1b_G").count(), 22_000usize.div_ceil(4096).max(6));
        assert!(s.starts_with("\x1b_Ga=T,i=3,") && s.contains("\x1b_Gm=1;") && s.contains("\x1b_Gm=0;"));
        let id = placeholder_id(255 * 2 + 4);
        assert_eq!(id, 2 << 24 | 5);
        let ph = encode_placeholders(&p, id, 2, 2, false);
        assert!(ph.contains(&format!("\x1b_Ga=t,i={id},q=2,f=32,s=1,v=1,m=0;")));
        assert!(ph.contains(&format!("\x1b_Ga=p,U=1,i={id},c=2,r=2,q=2\x1b\\")));
        assert!(ph.contains("\x1b[38;5;5m\u{10EEEE}\u{305}\u{305}\u{30E}\u{10EEEE}\u{305}\u{30D}\u{30E}\x1b[39m\r\n"));
        assert!(ph.ends_with("\u{10EEEE}\u{30D}\u{305}\u{30E}\u{10EEEE}\u{30D}\u{30D}\u{30E}\x1b[39m\r\n"));
        assert!((0..100_000).map(placeholder_id).all(|i| i & 0xff != 0 && i & 0x00ff_ff00 == 0));
    }

    #[test]
    fn placeholder_grid_is_capped() {
        let t = TermInfo { proto: Proto::KittyPlaceholders, cols: 1000, ..term() };
        let (c, r) = fit_cells(4000, 100, &t, &Options { original: true, ..Default::default() });
        assert!(c <= MAX_PLACEHOLDER_CELLS && r >= 1);
    }

    #[test]
    fn reply_errors_are_reported() {
        assert_eq!(reply_errors("\x1b_Gi=3;ENOSPC:full\x1b\\\x1b_Gi=4;OK\x1b\\"), vec!["ENOSPC: full"]);
        assert_eq!(reply_errors("\x1bP=Fzgr;id=7,ok=0,err=ENOMEM,msg=too big\x1b\\"), vec!["ENOMEM: too big"]);
        assert!(reply_errors("\x1bP=Fzgr;id=7,ok=1,w=2,h=1\x1b\\").is_empty());
    }
}
