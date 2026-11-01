//! Out-of-process custom engines (docs/engines.md §7).
//!
//! `mouser __engine-host <library> <name>` loads a custom engine in a child
//! process with a `SURFACE_PIXELS` target. Mouser streams frames to it over
//! stdin (glyph bitmaps and images are sent once and cached by key) and reads
//! back the pixels over stdout, then presents them. If the child dies, the
//! read fails with [`EngineCrashed`] and Mouser moves on to the next engine;
//! nothing in the main process is lost.

use crate::engine::{CustomLibrary, Engine};
use crate::text::{DisplayList, DrawOp, FontStack};
use anyhow::{Result, anyhow, bail};
use mouser_engine_abi as abi;
use std::collections::{HashMap, HashSet};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// A frame taking longer than this means the host hung: it is killed.
pub const HANG_TIMEOUT: Duration = Duration::from_secs(5);

/// The isolated engine process died (or hung and was killed).
#[derive(Debug)]
pub struct EngineCrashed(pub String);

impl std::fmt::Display for EngineCrashed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for EngineCrashed {}

static HOST: OnceLock<(PathBuf, Vec<String>)> = OnceLock::new();

/// Set the command that runs the host (the `mouser` binary sets itself).
pub fn set_host_command(program: PathBuf, args: Vec<String>) {
    let _ = HOST.set((program, args));
}

/// Whether custom engines run isolated in this process (Unix with a host command).
pub fn isolated() -> bool {
    cfg!(unix) && HOST.get().is_some()
}

// --- wire helpers (little endian) ---
fn put32(w: &mut impl Write, v: u32) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn puti(w: &mut impl Write, v: i32) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn put64(w: &mut impl Write, v: u64) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn get32(r: &mut impl Read) -> std::io::Result<u32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn geti(r: &mut impl Read) -> std::io::Result<i32> {
    Ok(get32(r)? as i32)
}
fn get64(r: &mut impl Read) -> std::io::Result<u64> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}
fn getn(r: &mut impl Read, n: usize) -> std::io::Result<Vec<u8>> {
    let mut v = vec![0; n];
    r.read_exact(&mut v)?;
    Ok(v)
}

const MAX_DIM: u32 = 16384;

/// Run the host: load `lib`, check its name, create the engine on a pixel
/// buffer and serve frames until stdin closes.
pub fn serve(lib: &Path, name: &str, input: impl Read, output: impl Write) -> Result<()> {
    let mut rx = BufReader::new(input);
    let mut tx = BufWriter::new(output);
    let w = get32(&mut rx)?.clamp(1, MAX_DIM);
    let h = get32(&mut rx)?.clamp(1, MAX_DIM);
    let mut pixels = vec![0u32; (w * h) as usize];
    let mut target = Box::new(abi::MouserPixels { data: pixels.as_mut_ptr(), width: w, height: h, stride: w });
    let made = CustomLibrary::load(lib).and_then(|l| {
        if l.name() != name {
            bail!("library reports name {:?}, manifest says {name:?}", l.name());
        }
        let surface = abi::MouserSurface { kind: abi::SURFACE_PIXELS, display: std::ptr::null_mut(), window: &mut *target as *mut _ as *mut _ };
        let handle = l.create_raw(&surface, w, h)?;
        Ok((l, handle))
    });
    let (lib, handle) = match made {
        Ok(v) => {
            tx.write_all(&[0])?;
            tx.flush()?;
            v
        }
        Err(e) => {
            let msg = format!("{e:#}");
            tx.write_all(&[1])?;
            put32(&mut tx, msg.len() as u32)?;
            tx.write_all(msg.as_bytes())?;
            tx.flush()?;
            return Ok(());
        }
    };
    let (mut cw, mut ch) = (w, h);
    let mut glyphs: HashMap<u64, (u32, u32, Vec<u8>)> = HashMap::new();
    let mut images: HashMap<u64, (u32, u32, Vec<u8>)> = HashMap::new();
    let mut tag = [0u8; 1];
    loop {
        if rx.read_exact(&mut tag).is_err() {
            break;
        }
        match tag[0] {
            b'G' => {
                let (k, gw, gh) = (get64(&mut rx)?, get32(&mut rx)?.min(MAX_DIM), get32(&mut rx)?.min(MAX_DIM));
                let b = getn(&mut rx, (gw * gh) as usize)?;
                glyphs.insert(k, (gw, gh, b));
            }
            b'I' => {
                let (k, iw, ih) = (get64(&mut rx)?, get32(&mut rx)?.min(MAX_DIM), get32(&mut rx)?.min(MAX_DIM));
                let b = getn(&mut rx, (iw * ih * 4) as usize)?;
                images.insert(k, (iw, ih, b));
            }
            b'R' => glyphs.clear(),
            b'F' => {
                let fw = get32(&mut rx)?.clamp(1, MAX_DIM);
                let fh = get32(&mut rx)?.clamp(1, MAX_DIM);
                let clear = get32(&mut rx)?;
                let n = get32(&mut rx)? as usize;
                let (mut ops, mut rects, mut gl, mut im) = (Vec::with_capacity(n), Vec::new(), Vec::new(), Vec::new());
                let mut used = HashSet::new();
                for _ in 0..n {
                    let mut k = [0u8; 1];
                    rx.read_exact(&mut k)?;
                    match k[0] {
                        0 => {
                            let r = abi::MouserRect {
                                x: geti(&mut rx)?,
                                y: geti(&mut rx)?,
                                w: geti(&mut rx)?,
                                h: geti(&mut rx)?,
                                color: get32(&mut rx)?,
                            };
                            ops.push(abi::MouserOp { kind: abi::OP_RECT, index: rects.len() as u32 });
                            rects.push(r);
                        }
                        1 => {
                            let (key, x, y, color) = (get64(&mut rx)?, geti(&mut rx)?, geti(&mut rx)?, get32(&mut rx)?);
                            if let Some((gw, gh, b)) = glyphs.get(&key) {
                                ops.push(abi::MouserOp { kind: abi::OP_GLYPH, index: gl.len() as u32 });
                                gl.push(abi::MouserGlyph { x, y, w: *gw, h: *gh, stride: *gw, color, coverage: b.as_ptr(), key });
                            }
                        }
                        _ => {
                            let key = get64(&mut rx)?;
                            let (x, y) = (geti(&mut rx)?, geti(&mut rx)?);
                            let (dw, dh, sx, sy, sw, sh) =
                                (get32(&mut rx)?, get32(&mut rx)?, get32(&mut rx)?, get32(&mut rx)?, get32(&mut rx)?, get32(&mut rx)?);
                            used.insert(key);
                            if let Some((iw, ih, b)) = images.get(&key) {
                                ops.push(abi::MouserOp { kind: abi::OP_IMAGE, index: im.len() as u32 });
                                im.push(abi::MouserImage {
                                    x,
                                    y,
                                    w: dw,
                                    h: dh,
                                    img_w: *iw,
                                    img_h: *ih,
                                    src_x: sx,
                                    src_y: sy,
                                    src_w: sw,
                                    src_h: sh,
                                    rgba: b.as_ptr(),
                                    key,
                                });
                            }
                        }
                    }
                }
                images.retain(|k, _| used.contains(k));
                if (fw, fh) != (cw, ch) {
                    cw = fw;
                    ch = fh;
                    pixels = vec![0u32; (cw * ch) as usize];
                    *target = abi::MouserPixels { data: pixels.as_mut_ptr(), width: cw, height: ch, stride: cw };
                    lib.resize_raw(handle, cw, ch);
                }
                let frame = abi::MouserFrame {
                    width: cw,
                    height: ch,
                    clear,
                    dirty_x: 0,
                    dirty_y: 0,
                    dirty_w: cw as i32,
                    dirty_h: ch as i32,
                    ops: ops.as_ptr(),
                    op_count: ops.len(),
                    rects: rects.as_ptr(),
                    rect_count: rects.len(),
                    glyphs: gl.as_ptr(),
                    glyph_count: gl.len(),
                    images: im.as_ptr(),
                    image_count: im.len(),
                };
                let r = lib.draw_raw(handle, &frame);
                puti(&mut tx, r)?;
                if r == abi::DRAW_OK {
                    // SAFETY: u32 slice viewed as bytes.
                    let bytes = unsafe { std::slice::from_raw_parts(pixels.as_ptr() as *const u8, pixels.len() * 4) };
                    tx.write_all(bytes)?;
                }
                tx.flush()?;
            }
            t => bail!("engine host: unknown message {t}"),
        }
    }
    lib.destroy_raw(handle);
    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Mouser's side of one host process.
pub struct HostClient {
    name: String,
    child: Arc<Mutex<Child>>,
    tx: BufWriter<ChildStdin>,
    rx: BufReader<ChildStdout>,
    glyphs: HashSet<u64>,
    images: HashSet<u64>,
    generation: u64,
    busy: Arc<AtomicU64>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl HostClient {
    /// Start `program args.. <lib> <name>` and create the engine at `w` x `h`.
    pub fn spawn(program: &Path, args: &[String], lib: &Path, name: &str, w: u32, h: u32) -> Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .arg(lib)
            .arg(name)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| anyhow!("starting the engine host: {e}"))?;
        let tx = BufWriter::with_capacity(1 << 16, child.stdin.take().unwrap());
        let rx = BufReader::with_capacity(1 << 20, child.stdout.take().unwrap());
        let child = Arc::new(Mutex::new(child));
        let busy = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let (child, busy, stop) = (child.clone(), busy.clone(), stop.clone());
            std::thread::Builder::new()
                .name("engine-watchdog".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(200));
                        let b = busy.load(Ordering::Relaxed);
                        if b != 0 && now_ms().saturating_sub(b) > HANG_TIMEOUT.as_millis() as u64 {
                            log::warn!("engine host hung for {HANG_TIMEOUT:?}; killing it");
                            let _ = child.lock().unwrap().kill();
                            break;
                        }
                    }
                })
                .ok();
        }
        let mut c = Self { name: name.to_string(), child, tx, rx, glyphs: HashSet::new(), images: HashSet::new(), generation: u64::MAX, busy, stop };
        c.begin();
        let started = (|| -> std::io::Result<u8> {
            put32(&mut c.tx, w.max(1))?;
            put32(&mut c.tx, h.max(1))?;
            c.tx.flush()?;
            let mut s = [0u8; 1];
            c.rx.read_exact(&mut s)?;
            Ok(s[0])
        })();
        c.end();
        match started {
            Ok(0) => Ok(c),
            Ok(_) => {
                let n = get32(&mut c.rx).unwrap_or(0).min(4096) as usize;
                let msg = getn(&mut c.rx, n).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
                bail!("{msg}")
            }
            Err(_) => Err(c.crashed().into()),
        }
    }

    fn begin(&self) {
        self.busy.store(now_ms().max(1), Ordering::Relaxed);
    }
    fn end(&self) {
        self.busy.store(0, Ordering::Relaxed);
    }

    /// Describe how the host ended.
    fn crashed(&self) -> EngineCrashed {
        let mut child = self.child.lock().unwrap();
        let mut status = None;
        for _ in 0..50 {
            if let Ok(Some(s)) = child.try_wait() {
                status = Some(s);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let how = match status {
            #[cfg(unix)]
            Some(s) if std::os::unix::process::ExitStatusExt::signal(&s).is_some() => {
                format!("signal {}", std::os::unix::process::ExitStatusExt::signal(&s).unwrap())
            }
            Some(s) => format!("{s}"),
            None => "stopped responding".to_string(),
        };
        EngineCrashed(format!("engine {} crashed in its engine host ({how})", self.name))
    }

    /// Draw `list` and copy the pixels into `out` (w*h). Ok(false): engine asked to redraw.
    pub fn draw_into(&mut self, list: &DisplayList, fonts: &mut FontStack, out: &mut [u32]) -> Result<bool> {
        self.begin();
        let r = self.draw_io(list, fonts, out);
        self.end();
        match r {
            Ok(v) => v,
            Err(_) => Err(self.crashed().into()),
        }
    }

    fn draw_io(&mut self, list: &DisplayList, fonts: &mut FontStack, out: &mut [u32]) -> std::io::Result<Result<bool>> {
        let tx = &mut self.tx;
        if self.generation != fonts.generation {
            self.generation = fonts.generation;
            self.glyphs.clear();
            tx.write_all(b"R")?;
        }
        // Uploads first.
        for op in &list.ops {
            match *op {
                DrawOp::Glyph { ch, .. } => {
                    let key = ch as u64;
                    if self.glyphs.insert(key) {
                        let g = fonts.glyph(ch);
                        tx.write_all(b"G")?;
                        put64(tx, key)?;
                        put32(tx, g.metrics.width as u32)?;
                        put32(tx, g.metrics.height as u32)?;
                        tx.write_all(&g.bitmap[..g.metrics.width * g.metrics.height])?;
                    }
                }
                DrawOp::Image { index, .. } => {
                    if let Some(img) = list.images.get(index)
                        && self.images.insert(img.key)
                    {
                        tx.write_all(b"I")?;
                        put64(tx, img.key)?;
                        put32(tx, img.img_w)?;
                        put32(tx, img.img_h)?;
                        tx.write_all(&img.rgba[..(img.img_w * img.img_h * 4) as usize])?;
                    }
                }
                DrawOp::Rect { .. } => {}
            }
        }
        let used: HashSet<u64> = list.images.iter().map(|i| i.key).collect();
        self.images.retain(|k| used.contains(k));
        tx.write_all(b"F")?;
        put32(tx, list.width as u32)?;
        put32(tx, list.height as u32)?;
        put32(tx, list.clear)?;
        put32(tx, list.ops.len() as u32)?;
        for op in &list.ops {
            match *op {
                DrawOp::Rect { x, y, w, h, color } => {
                    tx.write_all(&[0])?;
                    for v in [x, y, w, h] {
                        puti(tx, v)?;
                    }
                    put32(tx, color)?;
                }
                DrawOp::Glyph { x, y, ch, color } => {
                    tx.write_all(&[1])?;
                    put64(tx, ch as u64)?;
                    puti(tx, x)?;
                    puti(tx, y)?;
                    put32(tx, color)?;
                }
                DrawOp::Image { index, x, y } => {
                    let img = &list.images[index];
                    tx.write_all(&[2])?;
                    put64(tx, img.key)?;
                    puti(tx, x)?;
                    puti(tx, y)?;
                    let (sx, sy, sw, sh) = img.src;
                    for v in [img.width, img.height, sx, sy, sw, sh] {
                        put32(tx, v)?;
                    }
                }
            }
        }
        tx.flush()?;
        let r = geti(&mut self.rx)?;
        if r != abi::DRAW_OK {
            return Ok(if r < 0 { Err(anyhow!("engine {} draw returned {r}", self.name)) } else { Ok(false) });
        }
        let n = list.width * list.height;
        // SAFETY: u32 slice viewed as bytes.
        let bytes = unsafe { std::slice::from_raw_parts_mut(out.as_mut_ptr() as *mut u8, n.min(out.len()) * 4) };
        self.rx.read_exact(bytes)?;
        if out.len() < n {
            let mut rest = vec![0u8; (n - out.len()) * 4];
            self.rx.read_exact(&mut rest)?;
        }
        Ok(Ok(true))
    }
}

impl Drop for HostClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let mut c = self.child.lock().unwrap();
        let _ = c.kill();
        let _ = c.wait();
    }
}

type Window = winit::window::Window;
type Present = softbuffer::Surface<Arc<Window>, Arc<Window>>;

/// A custom engine running in the engine host, presented with softbuffer.
pub struct HostedEngine {
    client: HostClient,
    surface: Present,
    _context: softbuffer::Context<Arc<Window>>,
    size: (u32, u32),
    last: Instant,
}

impl HostedEngine {
    pub fn create(lib: &Path, name: &str, window: &Arc<Window>, w: u32, h: u32) -> Result<Self> {
        let (program, args) = HOST.get().ok_or_else(|| anyhow!("no engine host"))?;
        let client = HostClient::spawn(program, args, lib, name, w, h)?;
        let context = softbuffer::Context::new(window.clone()).map_err(|e| anyhow!("softbuffer: {e}"))?;
        let mut surface = softbuffer::Surface::new(&context, window.clone()).map_err(|e| anyhow!("softbuffer: {e}"))?;
        let nz = |v: u32| std::num::NonZeroU32::new(v.max(1)).unwrap();
        surface.resize(nz(w), nz(h)).map_err(|e| anyhow!("softbuffer: {e}"))?;
        Ok(Self { client, surface, _context: context, size: (w.max(1), h.max(1)), last: Instant::now() })
    }
}

impl Engine for HostedEngine {
    fn name(&self) -> &str {
        &self.client.name
    }
    fn description(&self) -> String {
        "custom, ABI 1, isolated in the engine host".into()
    }
    fn resize(&mut self, width: u32, height: u32) {
        let nz = |v: u32| std::num::NonZeroU32::new(v.max(1)).unwrap();
        self.size = (width.max(1), height.max(1));
        let _ = self.surface.resize(nz(width), nz(height));
    }
    fn draw(&mut self, list: &DisplayList, fonts: &mut FontStack) -> Result<bool> {
        if (list.width as u32, list.height as u32) != self.size {
            self.resize(list.width as u32, list.height as u32);
        }
        let Ok(mut buf) = self.surface.buffer_mut() else { return Ok(false) };
        let done = self.client.draw_into(list, fonts, &mut buf)?;
        if done {
            buf.present().map_err(|e| anyhow!("present: {e}"))?;
        }
        self.last = Instant::now();
        Ok(done)
    }
}
