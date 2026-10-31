//! Swappable GPU rendering engines (docs/engines.md).
//!
//! The core composes each frame into a [`DisplayList`]; an engine rasterises
//! and presents it. Built in: `mouser` (wgpu) and, with the `skia` feature,
//! `skia`. Custom engines are shared libraries speaking the C ABI in
//! `mouser-engine-abi`. The softbuffer CPU renderer is not an engine.

use crate::text::{DisplayList, DrawOp, FontStack};
use anyhow::{Result, anyhow, bail};
use mouser_core::engines;
use mouser_engine_abi as abi;
use std::ffi::{CStr, c_char};
use std::path::Path;
use std::sync::Arc;
use winit::window::Window;

/// A GPU rendering engine bound to one window.
pub trait Engine {
    /// Engine name (`mouser`, `skia`, or the custom engine's name).
    fn name(&self) -> &str;
    /// Backend details for the startup log.
    fn description(&self) -> String;
    fn resize(&mut self, width: u32, height: u32);
    /// Draw and present; `Ok(false)` asks for another frame (surface reset).
    fn draw(&mut self, list: &DisplayList, fonts: &mut FontStack) -> Result<bool>;
}

/// The default engine: the wgpu renderer.
pub struct MouserEngine(pub crate::gpu::GpuPainter);

impl Engine for MouserEngine {
    fn name(&self) -> &str {
        "mouser"
    }
    fn description(&self) -> String {
        format!("wgpu, {}", self.0.description)
    }
    fn resize(&mut self, width: u32, height: u32) {
        self.0.resize(width, height)
    }
    fn draw(&mut self, list: &DisplayList, fonts: &mut FontStack) -> Result<bool> {
        Ok(self.0.draw(list, fonts))
    }
}

/// Built-in engines this build has.
pub fn available_builtins() -> Vec<(&'static str, &'static str)> {
    let mut v = vec![("mouser", "wgpu (Vulkan/GL/Metal/DX12), default")];
    if cfg!(feature = "skia") {
        v.push(("skia", "Skia over OpenGL"));
    }
    v
}

fn create_named(name: &str, window: &Arc<Window>, width: u32, height: u32) -> Result<Box<dyn Engine>> {
    if name == "mouser" {
        return Ok(Box::new(MouserEngine(crate::gpu::GpuPainter::new(window.clone(), width, height)?)));
    }
    if let Err(why) = skip_check(name) {
        bail!("{why}");
    }
    if !engines::is_builtin(name) && crate::engine_host::isolated() {
        // Isolated: the library is only ever loaded in the host process, and a
        // crash there is caught as EngineCrashed (no crash marker needed).
        let inst = engines::find(name).ok_or_else(|| anyhow!("no engine named {name} (see `mouser engine list`)"))?;
        return Ok(Box::new(crate::engine_host::HostedEngine::create(&inst.library(), name, window, width, height)?));
    }
    let lib = if engines::is_builtin(name) { None } else { Some(load_installed(name)?) };
    #[cfg(not(feature = "skia"))]
    if lib.is_none() {
        bail!("this build has no Skia engine (built without the `skia` feature)");
    }
    engines::set_sentinel(name);
    let made: Result<Box<dyn Engine>> = match lib {
        Some(lib) => CustomEngine::create(lib, window, width, height).map(|e| Box::new(e) as Box<dyn Engine>),
        #[cfg(feature = "skia")]
        None => crate::skia::SkiaEngine::new(window.clone(), width, height).map(|e| Box::new(e) as Box<dyn Engine>),
        #[cfg(not(feature = "skia"))]
        None => unreachable!(),
    };
    match made {
        Ok(inner) => Ok(Box::new(Guarded { inner, frames: 0 })),
        Err(e) => {
            engines::clear_sentinel(name);
            Err(e)
        }
    }
}

/// The first engine of `order` that is not skipped and exists in this build
/// (what a window would try first).
pub fn first_usable(order: &[String]) -> Option<String> {
    engines::normalize(order.iter().cloned())
        .into_iter()
        .find(|n| skip_check(n).is_ok() && (n == "mouser" || available_builtins().iter().any(|(b, _)| b == n) || engines::find(n).is_some()))
}

/// The message of a caught panic.
pub fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown panic".into())
}

/// The command that restores a skipped or failed engine.
pub fn unskip_command(name: &str) -> String {
    format!("mouser engine unskip {name}")
}

/// Err(message) if `name` is skipped because it crashed Mouser while starting.
pub fn skip_check(name: &str) -> std::result::Result<(), String> {
    if !engines::crashed_last_time(name) {
        return Ok(());
    }
    let when = engines::skipped_since(name).map(|t| format!(" ({})", engines::ago(t))).unwrap_or_default();
    let why = engines::skip_reason(name).unwrap_or_else(|| "it crashed Mouser while starting".into());
    Err(format!("engine {name} is skipped: {why}{when}. Restore it with `{}`", unskip_command(name)))
}

/// Load an installed custom engine's library, refusing one that crashed
/// Mouser on its last start or whose library does not match its manifest.
pub fn load_installed(name: &str) -> Result<CustomLibrary> {
    let inst = engines::find(name).ok_or_else(|| anyhow!("no engine named {name} (see `mouser engine list`)"))?;
    if let Err(why) = skip_check(name) {
        bail!("{why}");
    }
    let lib = CustomLibrary::load(&inst.library())?;
    if lib.name() != inst.manifest.name {
        bail!("library reports name {:?}, manifest says {:?}", lib.name(), inst.manifest.name);
    }
    Ok(lib)
}

/// Walk `order` (mouser is always in it) and start the first engine that
/// works. Returns it and one message per engine passed over (skipped or
/// failed), each with its restore command. Errors only if even mouser fails
/// (the caller then uses the CPU renderer).
pub fn create(order: &[String], window: &Arc<Window>, width: u32, height: u32) -> Result<(Box<dyn Engine>, Vec<String>)> {
    let mut notes = Vec::new();
    let mut last = None;
    for name in engines::normalize(order.iter().cloned()) {
        let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| create_named(&name, window, width, height)));
        let made = match made {
            Ok(r) => r,
            Err(p) => {
                let why = format!("engine {name} panicked while starting: {}", panic_text(&p));
                engines::mark_skipped(&name, &why);
                if name != "mouser" {
                    notes.push(format!("{why}; skipped until `{}`", unskip_command(&name)));
                    continue;
                }
                Err(anyhow!(why))
            }
        };
        match made {
            Ok(e) => {
                engines::clear_failure(&name);
                return Ok((e, notes));
            }
            Err(e) if name == "mouser" => last = Some(e),
            Err(e) => notes.push(fallback_message(&name, &format!("{e:#}"), "failed to start")),
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("no GPU engine")))
}

/// The engines to try after `failed` broke at run time: the rest of the
/// priority order (mouser always included).
pub fn after(order: &[String], failed: &str) -> Vec<String> {
    let order = engines::normalize(order.iter().cloned());
    let i = order.iter().position(|n| n == failed).map(|i| i + 1).unwrap_or(0);
    let rest: Vec<String> = order[i..].to_vec();
    engines::normalize(rest)
}

/// Record a failure (unless the engine is skipped, which says it already) and
/// build the "using mouser; restore with ..." message.
pub fn fallback_message(name: &str, err: &str, what: &str) -> String {
    if engines::crashed_last_time(name) && err.contains("is skipped") {
        return format!("{err}; trying the next engine");
    }
    engines::record_failure(name, &format!("{what}: {err}"));
    format!("engine {name} {what}: {err}; trying the next engine. Details: `mouser engine list`; clear with `{}`", unskip_command(name))
}

/// Clears the crash marker once an engine has presented a few frames.
struct Guarded {
    inner: Box<dyn Engine>,
    frames: u32,
}

impl Engine for Guarded {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn description(&self) -> String {
        self.inner.description()
    }
    fn resize(&mut self, width: u32, height: u32) {
        self.inner.resize(width, height)
    }
    fn draw(&mut self, list: &DisplayList, fonts: &mut FontStack) -> Result<bool> {
        let r = self.inner.draw(list, fonts);
        if matches!(r, Ok(true)) && self.frames < STARTED_AFTER {
            self.frames += 1;
            if self.frames == STARTED_AFTER {
                engines::clear_sentinel(self.inner.name());
            }
        }
        r
    }
}

/// Fallback after a runtime failure.
pub fn create_default(window: &Arc<Window>, width: u32, height: u32) -> Result<Box<dyn Engine>> {
    create_named("mouser", window, width, height)
}

/// A loaded custom engine library with a checked v1 table.
pub struct CustomLibrary {
    table: &'static abi::MouserEngineV1,
    name: String,
    // Keep last: the table points into the library.
    _lib: libloading::Library,
}

impl CustomLibrary {
    /// Load a library and check its ABI.
    pub fn load(path: &Path) -> Result<Self> {
        // SAFETY: loading runs the library's initialisers; installing an engine
        // means trusting it (docs/engines.md).
        let lib = unsafe { libloading::Library::new(path) }.map_err(|e| anyhow!("loading {}: {e}", path.display()))?;
        // SAFETY: the symbol's type is fixed by the ABI.
        let entry: libloading::Symbol<'_, unsafe extern "C" fn() -> *const abi::MouserEngineV1> =
            unsafe { lib.get(abi::ENTRY_SYMBOL) }.map_err(|_| anyhow!("{} has no mouser_engine_v1 symbol", path.display()))?;
        // SAFETY: entry returns a pointer to static data in the library.
        let ptr = unsafe { entry() };
        let table = unsafe { ptr.as_ref() }.ok_or_else(|| anyhow!("mouser_engine_v1 returned null"))?;
        // SAFETY: lives as long as `_lib`, which this struct owns.
        let table: &'static abi::MouserEngineV1 = unsafe { &*(table as *const abi::MouserEngineV1) };
        if table.abi != abi::MOUSER_ENGINE_ABI {
            bail!("engine ABI {} is not supported (this Mouser speaks ABI {})", table.abi, abi::MOUSER_ENGINE_ABI);
        }
        if (table.size as usize) < std::mem::size_of::<abi::MouserEngineV1>() {
            bail!("engine function table is too small ({} bytes)", table.size);
        }
        if table.name.is_null() {
            bail!("engine has no name");
        }
        // SAFETY: checked non-null; NUL-terminated by the ABI.
        let name = unsafe { CStr::from_ptr(table.name) }.to_string_lossy().into_owned();
        Ok(Self { table, name, _lib: lib })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Create an engine on a raw surface (tests use a null/unsupported one).
    pub fn create_raw(&self, surface: &abi::MouserSurface, width: u32, height: u32) -> Result<*mut std::ffi::c_void> {
        let mut err = [0 as c_char; 512];
        // SAFETY: ABI call with valid pointers.
        let p = unsafe { (self.table.create)(surface, width, height, err.as_mut_ptr(), err.len()) };
        if p.is_null() {
            err[err.len() - 1] = 0;
            // SAFETY: NUL-terminated above.
            let msg = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().into_owned();
            bail!("create failed: {}", if msg.is_empty() { "no reason given" } else { &msg });
        }
        Ok(p)
    }
}

impl CustomLibrary {
    pub(crate) fn resize_raw(&self, handle: *mut std::ffi::c_void, w: u32, h: u32) {
        // SAFETY: handle from create_raw.
        unsafe { (self.table.resize)(handle, w, h) }
    }
    pub(crate) fn draw_raw(&self, handle: *mut std::ffi::c_void, frame: &abi::MouserFrame) -> i32 {
        // SAFETY: handle from create_raw; frame valid for the call.
        unsafe { (self.table.draw)(handle, frame) }
    }
    pub(crate) fn destroy_raw(&self, handle: *mut std::ffi::c_void) {
        // SAFETY: handle from create_raw, not used afterwards.
        unsafe { (self.table.destroy)(handle) }
    }
}

/// Check a library before installing it; returns the name it reports.
pub fn probe(path: &Path) -> Result<String> {
    Ok(CustomLibrary::load(path)?.name)
}

fn surface_of(window: &Window) -> Result<abi::MouserSurface> {
    use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle as D, RawWindowHandle as W};
    let d = window.display_handle().map_err(|e| anyhow!("display handle: {e}"))?.as_raw();
    let w = window.window_handle().map_err(|e| anyhow!("window handle: {e}"))?.as_raw();
    let null = std::ptr::null_mut();
    Ok(match (d, w) {
        (D::Wayland(d), W::Wayland(w)) => abi::MouserSurface { kind: abi::SURFACE_WAYLAND, display: d.display.as_ptr(), window: w.surface.as_ptr() },
        (D::Xlib(d), W::Xlib(w)) => abi::MouserSurface {
            kind: abi::SURFACE_XLIB,
            display: d.display.map(|p| p.as_ptr()).unwrap_or(null),
            window: w.window as usize as *mut _,
        },
        (D::Xcb(d), W::Xcb(w)) => abi::MouserSurface {
            kind: abi::SURFACE_XCB,
            display: d.connection.map(|p| p.as_ptr()).unwrap_or(null),
            window: w.window.get() as usize as *mut _,
        },
        (_, W::AppKit(w)) => abi::MouserSurface { kind: abi::SURFACE_APPKIT, display: null, window: w.ns_view.as_ptr() },
        (_, W::Win32(w)) => abi::MouserSurface {
            kind: abi::SURFACE_WIN32,
            display: w.hinstance.map(|h| h.get() as *mut _).unwrap_or(null),
            window: w.hwnd.get() as *mut _,
        },
        _ => bail!("unsupported window system for custom engines"),
    })
}

/// Frames presented before an engine counts as started (sentinel removed).
const STARTED_AFTER: u32 = 3;

/// A custom engine instance.
pub struct CustomEngine {
    handle: *mut std::ffi::c_void,
    buf: FrameBuf,
    // Dropped after `handle` is destroyed (see Drop); keeps the window alive.
    _window: Arc<Window>,
    lib: CustomLibrary,
}

impl CustomEngine {
    pub fn create(lib: CustomLibrary, window: &Arc<Window>, width: u32, height: u32) -> Result<Self> {
        let surface = surface_of(window)?;
        let handle = lib.create_raw(&surface, width, height)?;
        Ok(Self { handle, buf: FrameBuf::default(), _window: window.clone(), lib })
    }
}

impl Drop for CustomEngine {
    fn drop(&mut self) {
        // SAFETY: handle came from create.
        unsafe { (self.lib.table.destroy)(self.handle) };
    }
}

impl Engine for CustomEngine {
    fn name(&self) -> &str {
        self.lib.name()
    }
    fn description(&self) -> String {
        format!("custom, ABI {}", self.lib.table.abi)
    }
    fn resize(&mut self, width: u32, height: u32) {
        // SAFETY: valid handle.
        unsafe { (self.lib.table.resize)(self.handle, width, height) }
    }
    fn draw(&mut self, list: &DisplayList, fonts: &mut FontStack) -> Result<bool> {
        let frame = self.buf.build(list, fonts);
        // SAFETY: `frame` points into `self.buf` and `fonts`, alive for the call.
        let r = unsafe { (self.lib.table.draw)(self.handle, &frame) };
        if r < 0 {
            bail!("engine {} draw returned {r}", self.lib.name());
        }
        Ok(r == abi::DRAW_OK)
    }
}

/// Reusable C-side arrays for one frame.
#[derive(Default)]
pub struct FrameBuf {
    ops: Vec<abi::MouserOp>,
    rects: Vec<abi::MouserRect>,
    glyphs: Vec<abi::MouserGlyph>,
    images: Vec<abi::MouserImage>,
}

impl FrameBuf {
    /// Convert a display list. The result borrows `self`, `list` and the
    /// glyph cache in `fonts` (bitmaps don't move once cached).
    pub fn build(&mut self, list: &DisplayList, fonts: &mut FontStack) -> abi::MouserFrame {
        self.ops.clear();
        self.rects.clear();
        self.glyphs.clear();
        self.images.clear();
        let generation = fonts.generation;
        for op in &list.ops {
            match *op {
                DrawOp::Rect { x, y, w, h, color } => {
                    self.ops.push(abi::MouserOp { kind: abi::OP_RECT, index: self.rects.len() as u32 });
                    self.rects.push(abi::MouserRect { x, y, w, h, color });
                }
                DrawOp::Glyph { x, y, ch, color } => {
                    let g = fonts.glyph(ch);
                    let (w, h) = (g.metrics.width as u32, g.metrics.height as u32);
                    if w == 0 || h == 0 {
                        continue;
                    }
                    self.ops.push(abi::MouserOp { kind: abi::OP_GLYPH, index: self.glyphs.len() as u32 });
                    self.glyphs.push(abi::MouserGlyph {
                        x,
                        y,
                        w,
                        h,
                        stride: w,
                        color,
                        coverage: g.bitmap.as_ptr(),
                        key: generation << 32 | ch as u64,
                    });
                }
                DrawOp::Image { index, x, y } => {
                    let Some(img) = list.images.get(index) else { continue };
                    self.ops.push(abi::MouserOp { kind: abi::OP_IMAGE, index: self.images.len() as u32 });
                    let (sx, sy, sw, sh) = img.src;
                    self.images.push(abi::MouserImage {
                        x,
                        y,
                        w: img.width,
                        h: img.height,
                        img_w: img.img_w,
                        img_h: img.img_h,
                        src_x: sx,
                        src_y: sy,
                        src_w: sw,
                        src_h: sh,
                        rgba: img.rgba.as_ptr(),
                        key: img.key,
                    });
                }
            }
        }
        abi::MouserFrame {
            width: list.width as u32,
            height: list.height as u32,
            clear: list.clear,
            dirty_x: 0,
            dirty_y: 0,
            dirty_w: list.width as i32,
            dirty_h: list.height as i32,
            ops: self.ops.as_ptr(),
            op_count: self.ops.len(),
            rects: self.rects.as_ptr(),
            rect_count: self.rects.len(),
            glyphs: self.glyphs.as_ptr(),
            glyph_count: self.glyphs.len(),
            images: self.images.as_ptr(),
            image_count: self.images.len(),
        }
    }
}
