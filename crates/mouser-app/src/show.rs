//! `mouser show`: display images in a Fuzzball terminal (docs/protocol/graphics.md).
//!
//! The pure parts (capability parsing, size fitting, message encoding) are
//! separate from terminal I/O so they can be tested against Mouser's parser.
//! Video and audio are not supported.
#![cfg_attr(not(unix), allow(unused_imports, dead_code))]

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// What the terminal told us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermInfo {
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

/// Parse terminal replies: the Fuzzball `cq` reply, `CSI 6;h;w t` (cell size),
/// `CSI 8;rows;cols t` and the cursor report `CSI row;col R`. `None` without `cq`.
pub fn parse_replies(r: &str, tmux: bool) -> Option<TermInfo> {
    let caps = r.split("Fzcr;").nth(1)?.split('\x1b').next()?;
    let get = |k: &str| caps.split(',').find_map(|kv| kv.strip_prefix(k).and_then(|v| v.strip_prefix('=')));
    if !get("sub").is_some_and(|s| s.contains('g')) {
        return None;
    }
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
    Some(TermInfo {
        max_chunk: get("max_chunk").and_then(|v| v.parse().ok()).unwrap_or(4096),
        g_mem: get("g_mem").and_then(|v| v.parse().ok()).unwrap_or(64 << 20),
        g_max_dim: get("g_max_dim").and_then(|v| v.parse().ok()).unwrap_or(4096),
        png: get("g_fmt").is_some_and(|f| f.split(':').any(|x| x == "png")),
        cell,
        cols: cols.max(1),
        cursor_col,
        tmux,
    })
}

/// Cells for a `w` x `h` pixel image. Aspect ratio is kept; by default the width is
/// capped at the columns right of the cursor.
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
            let c = (w / cw).ceil().max(1.0);
            let c = if o.original { c } else { c.min(avail) };
            (c, rows_for(c))
        }
    };
    ((c.min(1000.0)) as u32, (r.min(1000.0)) as u32)
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

/// Bytes that upload `p` as image `id` in chunks of at most `max_chunk` base64
/// characters, place it at the cursor in `cols` x `rows` cells and move the
/// cursor to the start of the line below it. Only the last chunk asks for a
/// success reply; errors are always reported.
pub fn encode(p: &Payload, id: u32, cols: u32, rows: u32, t: &TermInfo) -> String {
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
    out += &message(&format!("gp;id={id},pid=1,cols={cols},rows={rows},quiet=1"), t.tmux);
    // `move=1` left the cursor on the image's last row; continue below it.
    out += "\r\n";
    out
}

/// Errors in Fuzzball replies (`ok=0`), as `ERR: msg`.
pub fn reply_errors(r: &str) -> Vec<String> {
    r.split("Fzgr;")
        .skip(1)
        .filter_map(|s| {
            let body = s.split('\x1b').next()?;
            body.contains("ok=0").then(|| {
                let f = |k: &str| body.split(',').find_map(|kv| kv.strip_prefix(k)).unwrap_or("");
                format!("{}: {}", f("err="), f("msg="))
            })
        })
        .collect()
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
    let q = message("cq;id=1", tmux) + "\x1b[16t\x1b[18t\x1b[6n\x1b[c";
    tty.out.write_all(q.as_bytes())?;
    let replies = tty.read_until(Duration::from_millis(1500), has_da1);
    let Some(mut t) = parse_replies(&replies, tmux) else {
        if tmux {
            bail!("no Fuzzball reply through tmux (tmux does not forward the terminal's replies); run it outside tmux");
        }
        bail!("this terminal does not support Fuzzball graphics (no reply to the capability query)");
    };
    let base = o.id.unwrap_or(0x1000_0000 | (std::process::id() & 0x0fff_ffff) << 4);
    let mut failed = false;
    for (i, f) in files.iter().enumerate() {
        let data = if f == "-" {
            let mut v = Vec::new();
            std::io::stdin().read_to_end(&mut v).context("reading stdin")?;
            v
        } else {
            std::fs::read(f).with_context(|| format!("reading {f}"))?
        };
        let res = prepare(&data, &t).and_then(|p| {
            let (cols, rows) = fit_cells(p.w, p.h, &t, o);
            let id = base.wrapping_add(i as u32).max(1);
            tty.out.write_all(encode(&p, id, cols, rows, &t).as_bytes())?;
            let r = tty.read_until(Duration::from_secs(5), |s| s.contains("Fzgr;"));
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
        TermInfo { max_chunk: 8, g_mem: 1 << 20, g_max_dim: 100, png: true, cell: (10, 20), cols: 80, cursor_col: 0, tmux: false }
    }

    #[test]
    fn parses_caps_cell_size_cols_and_cursor() {
        let r = "\x1bP=Fzcr;id=1,v=0,sub=gk,raw=0,max_chunk=65536,g_mem=335544320,g_max_dim=10000,g_fmt=rgba:rgb:png\x1b\\\x1b[6;19;10t\x1b[8;24;87t\x1b[3;5R\x1b[?62;22c";
        let t = parse_replies(r, false).unwrap();
        assert_eq!(
            t,
            TermInfo { max_chunk: 65536, g_mem: 335544320, g_max_dim: 10000, png: true, cell: (10, 19), cols: 87, cursor_col: 4, tmux: false }
        );
        assert!(parse_replies("\x1b[?62;22c", false).is_none());
        assert!(has_da1(r) && !has_da1("\x1b[6;19;10t"));
    }

    #[test]
    fn fits_width_keeps_aspect() {
        let t = term();
        // 400x200 px image, 10x20 cells: natural 40x10 cells.
        assert_eq!(fit_cells(400, 200, &t, &Options::default()), (40, 10));
        let narrow = TermInfo { cols: 30, cursor_col: 10, ..t.clone() };
        assert_eq!(fit_cells(400, 200, &narrow, &Options::default()), (20, 5));
        assert_eq!(fit_cells(400, 200, &narrow, &Options { original: true, ..Default::default() }), (40, 10));
        assert_eq!(fit_cells(400, 200, &t, &Options { width: Some(20), ..Default::default() }), (20, 5));
        assert_eq!(fit_cells(400, 200, &t, &Options { height: Some(5), ..Default::default() }), (20, 5));
        assert_eq!(fit_cells(400, 200, &t, &Options { width: Some(20), height: Some(2), ..Default::default() }), (8, 2));
        assert_eq!(fit_cells(1, 1, &t, &Options::default()), (1, 1));
    }

    #[test]
    fn encodes_chunked_upload_and_place() {
        let p = Payload { fmt: "rgba", w: 2, h: 1, bytes: vec![255, 0, 0, 255, 0, 0, 255, 255] };
        let s = encode(&p, 7, 4, 2, &term());
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
    fn reply_errors_are_reported() {
        assert_eq!(reply_errors("\x1bP=Fzgr;id=7,ok=0,err=ENOMEM,msg=too big\x1b\\"), vec!["ENOMEM: too big"]);
        assert!(reply_errors("\x1bP=Fzgr;id=7,ok=1,w=2,h=1\x1b\\").is_empty());
    }
}
