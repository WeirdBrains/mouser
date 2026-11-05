//! Mouser's Skia renderer as an engine library (engine name `skia`, ABI v2).
//!
//! The same drawing as the built-in `skia` engine (`crates/mouser-render/src/skia.rs`):
//! Skia rects, glyph coverage in one A8 atlas tinted with the paint colour,
//! and images drawn nearest-neighbour, on Ganesh over OpenGL (glutin) for a
//! window, or on a CPU raster surface for the pixel host. Glyph bitmaps come
//! from Mouser's frame (keyed by `MouserGlyph::key`).
#![allow(clippy::missing_safety_doc)]

use glutin::config::{ConfigTemplateBuilder, GlConfig};
use glutin::context::{ContextApi, ContextAttributesBuilder, PossiblyCurrentContext};
use glutin::display::{Display, DisplayApiPreference, GlDisplay};
use glutin::prelude::{GlSurface, NotCurrentGlContext};
use glutin::surface::{Surface as GlSurfaceT, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use mouser_engine_abi::*;
use skia_safe::gpu::{self, DirectContext, SurfaceOrigin, backend_render_targets, gl::FramebufferInfo};
use skia_safe::{AlphaType, Color, ColorType, Data, Image, ImageInfo, Paint, Rect, SamplingOptions, Surface, canvas::SrcRectConstraint};
use std::collections::HashMap;
use std::ffi::{CString, c_char, c_int, c_void};
use std::num::NonZeroU32;

const ATLAS: usize = 2048;

struct Atlas {
    pixels: Vec<u8>,
    slots: HashMap<u64, (i32, i32, i32, i32)>,
    x: i32,
    y: i32,
    row_h: i32,
    image: Option<Image>,
    dirty: bool,
}

impl Atlas {
    fn new() -> Self {
        Self { pixels: vec![0; ATLAS * ATLAS], slots: HashMap::new(), x: 0, y: 0, row_h: 0, image: None, dirty: true }
    }

    /// # Safety
    /// `g.coverage` holds `stride * h` bytes.
    unsafe fn slot(&mut self, g: &MouserGlyph) -> Option<(i32, i32, i32, i32)> {
        if let Some(s) = self.slots.get(&g.key) {
            return Some(*s);
        }
        let (w, h) = (g.w as i32, g.h as i32);
        if w == 0 || h == 0 || w > ATLAS as i32 || h > ATLAS as i32 || g.coverage.is_null() {
            return None;
        }
        if self.x + w + 1 > ATLAS as i32 {
            self.x = 0;
            self.y += self.row_h + 1;
            self.row_h = 0;
        }
        if self.y + h > ATLAS as i32 {
            return None;
        }
        // SAFETY: the caller guarantees the coverage.
        let data = unsafe { std::slice::from_raw_parts(g.coverage, (g.stride * g.h) as usize) };
        for row in 0..h as usize {
            let o = (self.y as usize + row) * ATLAS + self.x as usize;
            self.pixels[o..o + w as usize].copy_from_slice(&data[row * g.stride as usize..][..w as usize]);
        }
        let s = (self.x, self.y, w, h);
        self.slots.insert(g.key, s);
        self.x += w + 1;
        self.row_h = self.row_h.max(h);
        self.dirty = true;
        Some(s)
    }

    fn image(&mut self) -> Option<&Image> {
        if self.dirty || self.image.is_none() {
            let info = ImageInfo::new((ATLAS as i32, ATLAS as i32), ColorType::Alpha8, AlphaType::Premul, None);
            self.image = skia_safe::images::raster_from_data(&info, Data::new_copy(&self.pixels), ATLAS);
            self.dirty = false;
        }
        self.image.as_ref()
    }
}

/// ABI v2 colour: the top byte is `255 - opacity`.
fn color(c: u32) -> Color {
    Color::from_argb((255 - (c >> 24)) as u8, (c >> 16) as u8, (c >> 8) as u8, c as u8)
}

struct Gl {
    // Drop order: Skia first, then GL.
    surface: Surface,
    gr: DirectContext,
    fb: FramebufferInfo,
    samples: usize,
    stencil: usize,
    gl_surface: GlSurfaceT<WindowSurface>,
    gl_context: PossiblyCurrentContext,
}

impl Drop for Gl {
    fn drop(&mut self) {
        self.gr.release_resources_and_abandon();
    }
}

fn nz(v: u32) -> NonZeroU32 {
    NonZeroU32::new(v.max(1)).unwrap()
}

fn make_gl_surface(gr: &mut DirectContext, fb: FramebufferInfo, samples: usize, stencil: usize, w: u32, h: u32) -> Result<Surface, String> {
    let target = backend_render_targets::make_gl((w.max(1) as i32, h.max(1) as i32), samples, stencil, fb);
    gpu::surfaces::wrap_backend_render_target(gr, &target, SurfaceOrigin::BottomLeft, ColorType::RGBA8888, None, None)
        .ok_or_else(|| "Skia could not wrap the GL framebuffer".to_string())
}

fn raster(w: u32, h: u32) -> Result<Surface, String> {
    let info = ImageInfo::new((w.max(1) as i32, h.max(1) as i32), ColorType::RGBA8888, AlphaType::Premul, None);
    skia_safe::surfaces::raster(&info, None, None).ok_or_else(|| "Skia raster surface".to_string())
}

enum Target {
    Gl(Box<Gl>),
    Raster(Surface),
}

/// The renderer on a GL window or a raster surface.
pub struct Renderer {
    target: Target,
    atlas: Atlas,
    images: HashMap<u64, Image>,
    width: u32,
    height: u32,
    pub description: String,
}

impl Renderer {
    /// Draw on a CPU raster surface (the pixel host, tests).
    pub fn raster(width: u32, height: u32) -> Result<Self, String> {
        Ok(Self {
            target: Target::Raster(raster(width, height)?),
            atlas: Atlas::new(),
            images: HashMap::new(),
            width: width.max(1),
            height: height.max(1),
            description: "Skia raster".into(),
        })
    }

    /// Ganesh over OpenGL on a native window.
    ///
    /// # Safety
    /// The handles must be valid and outlive the renderer.
    pub unsafe fn on_window(
        raw_display: raw_window_handle::RawDisplayHandle,
        raw_window: raw_window_handle::RawWindowHandle,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        fn e(what: &'static str) -> impl Fn(glutin::error::Error) -> String {
            move |err| format!("{what}: {err}")
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        let pref = DisplayApiPreference::Egl;
        #[cfg(target_os = "macos")]
        let pref = DisplayApiPreference::Cgl;
        #[cfg(windows)]
        let pref = DisplayApiPreference::WglThenEgl(Some(raw_window));
        // SAFETY: the caller guarantees the handles.
        let display = unsafe { Display::new(raw_display, pref) }.map_err(e("GL display"))?;
        let template = ConfigTemplateBuilder::new().compatible_with_native_window(raw_window).with_alpha_size(8).build();
        // SAFETY: as above.
        let config = unsafe { display.find_configs(template) }
            .map_err(e("GL configs"))?
            .reduce(|a, c| if c.num_samples() < a.num_samples() { c } else { a })
            .ok_or("no GL config for this window")?;
        let attrs = ContextAttributesBuilder::new().build(Some(raw_window));
        let gles = ContextAttributesBuilder::new().with_context_api(ContextApi::Gles(None)).build(Some(raw_window));
        // SAFETY: as above.
        let ctx = unsafe { display.create_context(&config, &attrs).or_else(|_| display.create_context(&config, &gles)) }.map_err(e("GL context"))?;
        let sattrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(raw_window, nz(width), nz(height));
        // SAFETY: as above.
        let gl_surface = unsafe { display.create_window_surface(&config, &sattrs) }.map_err(e("GL window surface"))?;
        let gl_context = ctx.make_current(&gl_surface).map_err(e("making the GL context current"))?;
        let _ = gl_surface.set_swap_interval(&gl_context, SwapInterval::DontWait);
        let interface = skia_safe::gpu::gl::Interface::new_load_with(|name| {
            if name == "eglGetCurrentDisplay" {
                return std::ptr::null();
            }
            CString::new(name).map(|n| display.get_proc_address(&n)).unwrap_or(std::ptr::null())
        })
        .ok_or("Skia could not load GL")?;
        let mut gr = gpu::direct_contexts::make_gl(interface, None).ok_or("Skia GL context")?;
        let fb = FramebufferInfo { fboid: 0, format: skia_safe::gpu::gl::Format::RGBA8.into(), ..Default::default() };
        let (samples, stencil) = (config.num_samples() as usize, config.stencil_size() as usize);
        let surface = make_gl_surface(&mut gr, fb, samples, stencil, width, height)?;
        let description = format!("Skia GL, {:?}, {samples} samples", display.version_string());
        Ok(Self {
            target: Target::Gl(Box::new(Gl { surface, gr, fb, samples, stencil, gl_surface, gl_context })),
            atlas: Atlas::new(),
            images: HashMap::new(),
            width: width.max(1),
            height: height.max(1),
            description,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let (w, h) = (width.max(1), height.max(1));
        if (w, h) == (self.width, self.height) {
            return;
        }
        (self.width, self.height) = (w, h);
        match &mut self.target {
            Target::Gl(g) => {
                g.gl_surface.resize(&g.gl_context, nz(w), nz(h));
                if let Ok(s) = make_gl_surface(&mut g.gr, g.fb, g.samples, g.stencil, w, h) {
                    g.surface = s;
                }
            }
            Target::Raster(s) => {
                if let Ok(n) = raster(w, h) {
                    *s = n;
                }
            }
        }
    }

    pub fn evict(&mut self, key: u64) {
        self.images.remove(&key);
    }

    /// The raster surface's pixels as `0x00RRGGBB` (`None` on a GL window).
    pub fn read_pixels(&mut self) -> Option<Vec<u32>> {
        let Target::Raster(s) = &mut self.target else { return None };
        let (w, h) = (self.width as i32, self.height as i32);
        let info = ImageInfo::new((w, h), ColorType::RGBA8888, AlphaType::Premul, None);
        let mut px = vec![0u8; (w * h * 4) as usize];
        if !s.read_pixels(&info, &mut px, (w * 4) as usize, (0, 0)) {
            return None;
        }
        Some(px.as_chunks::<4>().0.iter().map(|p| (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32).collect())
    }

    /// Draw (and present on a window).
    ///
    /// # Safety
    /// The frame's pointers must be valid for the call.
    pub unsafe fn draw(&mut self, frame: &MouserFrame) -> c_int {
        if (frame.width, frame.height) != (self.width, self.height) {
            self.resize(frame.width, frame.height);
        }
        // SAFETY: the caller guarantees the frame.
        let (ops, rects, glyphs, images) = unsafe { frame.parts() };
        let mut full = false;
        for g in glyphs {
            // SAFETY: as above.
            if unsafe { self.atlas.slot(g) }.is_none() && g.w > 0 && g.h > 0 {
                full = true;
                break;
            }
        }
        if full {
            self.atlas = Atlas::new();
        }
        let used: std::collections::HashSet<u64> = images.iter().map(|i| i.key).collect();
        self.images.retain(|k, _| used.contains(k));
        let imgs: Vec<Option<Image>> = images
            .iter()
            .map(|img| {
                if let Some(i) = self.images.get(&img.key) {
                    return Some(i.clone());
                }
                if img.rgba.is_null() || img.img_w == 0 || img.img_h == 0 {
                    return None;
                }
                // SAFETY: `img_w * img_h * 4` bytes during draw.
                let rgba = unsafe { std::slice::from_raw_parts(img.rgba, (img.img_w * img.img_h * 4) as usize) };
                let info = ImageInfo::new((img.img_w as i32, img.img_h as i32), ColorType::RGBA8888, AlphaType::Unpremul, None);
                let i = skia_safe::images::raster_from_data(&info, Data::new_copy(rgba), img.img_w as usize * 4)?;
                self.images.insert(img.key, i.clone());
                Some(i)
            })
            .collect();
        let atlas = self.atlas.image().cloned();
        let canvas = match &mut self.target {
            Target::Gl(g) => g.surface.canvas(),
            Target::Raster(s) => s.canvas(),
        };
        let near = SamplingOptions::default();
        let mut paint = Paint::default();
        paint.set_anti_alias(false);
        canvas.clear(color(frame.clear));
        for op in ops {
            let i = op.index as usize;
            match op.kind {
                OP_RECT => {
                    let Some(r) = rects.get(i) else { continue };
                    paint.set_color(color(r.color));
                    canvas.draw_rect(Rect::from_xywh(r.x as f32, r.y as f32, r.w as f32, r.h as f32), &paint);
                }
                OP_GLYPH => {
                    let Some(g) = glyphs.get(i) else { continue };
                    let (Some(a), Some(&(sx, sy, w, h))) = (&atlas, self.atlas.slots.get(&g.key)) else { continue };
                    paint.set_color(color(g.color));
                    canvas.draw_image_rect_with_sampling_options(
                        a,
                        Some((&Rect::from_xywh(sx as f32, sy as f32, w as f32, h as f32), SrcRectConstraint::Strict)),
                        Rect::from_xywh(g.x as f32, g.y as f32, w as f32, h as f32),
                        near,
                        &paint,
                    );
                }
                OP_IMAGE => {
                    let (Some(Some(im)), Some(d)) = (imgs.get(i), images.get(i)) else { continue };
                    paint.set_color(Color::WHITE);
                    canvas.draw_image_rect_with_sampling_options(
                        im,
                        Some((&Rect::from_xywh(d.src_x as f32, d.src_y as f32, d.src_w as f32, d.src_h as f32), SrcRectConstraint::Strict)),
                        Rect::from_xywh(d.x as f32, d.y as f32, d.w as f32, d.h as f32),
                        near,
                        &paint,
                    );
                }
                _ => {}
            }
        }
        if let Target::Gl(g) = &mut self.target {
            let _ = g.gr.flush_and_submit();
            if g.gl_surface.swap_buffers(&g.gl_context).is_err() {
                return DRAW_AGAIN;
            }
        }
        DRAW_OK
    }
}

// ---- C ABI -----------------------------------------------------------------

struct Engine {
    r: Renderer,
    pixels: Option<*mut MouserPixels>,
}

fn raw_handles(s: &MouserSurface) -> Result<(raw_window_handle::RawDisplayHandle, raw_window_handle::RawWindowHandle), String> {
    use raw_window_handle::*;
    use std::ptr::NonNull;
    let nn = |p: *mut c_void, what: &str| NonNull::new(p).ok_or(format!("null {what}"));
    Ok(match s.kind {
        SURFACE_WAYLAND => (
            RawDisplayHandle::Wayland(WaylandDisplayHandle::new(nn(s.display, "wl_display")?)),
            RawWindowHandle::Wayland(WaylandWindowHandle::new(nn(s.window, "wl_surface")?)),
        ),
        SURFACE_XLIB => (
            RawDisplayHandle::Xlib(XlibDisplayHandle::new(NonNull::new(s.display), 0)),
            RawWindowHandle::Xlib(XlibWindowHandle::new(s.window as usize as _)),
        ),
        SURFACE_XCB => (
            RawDisplayHandle::Xcb(XcbDisplayHandle::new(NonNull::new(s.display), 0)),
            RawWindowHandle::Xcb(XcbWindowHandle::new(std::num::NonZeroU32::new(s.window as usize as u32).ok_or("null xcb window")?)),
        ),
        SURFACE_APPKIT => {
            (RawDisplayHandle::AppKit(AppKitDisplayHandle::new()), RawWindowHandle::AppKit(AppKitWindowHandle::new(nn(s.window, "NSView")?)))
        }
        SURFACE_WIN32 => {
            let mut w = Win32WindowHandle::new(std::num::NonZeroIsize::new(s.window as isize).ok_or("null HWND")?);
            w.hinstance = std::num::NonZeroIsize::new(s.display as isize);
            (RawDisplayHandle::Windows(WindowsDisplayHandle::new()), RawWindowHandle::Win32(w))
        }
        k => return Err(format!("unsupported surface kind {k}")),
    })
}

unsafe extern "C" fn create(surface: *const MouserSurface, w: u32, h: u32, err: *mut c_char, err_len: usize) -> *mut c_void {
    let fail = |m: String| {
        // SAFETY: Mouser passes a valid error buffer.
        unsafe { write_err(err, err_len, &m) };
        std::ptr::null_mut()
    };
    // SAFETY: Mouser passes a valid surface.
    let Some(s) = (unsafe { surface.as_ref() }) else { return fail("no surface".into()) };
    let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if s.kind == SURFACE_PIXELS {
            Renderer::raster(w, h).map(|r| Engine { r, pixels: Some(s.window as *mut MouserPixels) })
        } else {
            let (d, win) = raw_handles(s)?;
            // SAFETY: the window outlives the engine (ABI).
            unsafe { Renderer::on_window(d, win, w, h) }.map(|r| Engine { r, pixels: None })
        }
    }));
    match made {
        Ok(Ok(e)) => Box::into_raw(Box::new(e)) as *mut c_void,
        Ok(Err(e)) => fail(e),
        Err(_) => fail("skia panicked while starting".into()),
    }
}

unsafe fn engine<'a>(p: *mut c_void) -> Option<&'a mut Engine> {
    // SAFETY: `p` came from `create`.
    unsafe { (p as *mut Engine).as_mut() }
}

unsafe extern "C" fn resize(e: *mut c_void, w: u32, h: u32) {
    if let Some(e) = unsafe { engine(e) } {
        e.r.resize(w, h);
    }
}

unsafe extern "C" fn draw(e: *mut c_void, frame: *const MouserFrame) -> c_int {
    let (Some(e), Some(frame)) = (unsafe { engine(e) }, unsafe { frame.as_ref() }) else { return -1 };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: valid during draw (ABI).
        let r = unsafe { e.r.draw(frame) };
        if let Some(px) = e.pixels
            && let Some(out) = e.r.read_pixels()
            // SAFETY: Mouser's pixel buffer, valid during draw.
            && let Some(px) = unsafe { px.as_ref() }
        {
            let rw = e.r.width as usize;
            let (w, h) = (px.width.min(e.r.width) as usize, px.height.min(e.r.height) as usize);
            for y in 0..h {
                // SAFETY: `stride * height` pixels.
                let dst = unsafe { std::slice::from_raw_parts_mut(px.data.add(y * px.stride as usize), w) };
                dst.copy_from_slice(&out[y * rw..][..w]);
            }
        }
        r
    }))
    .unwrap_or(-2)
}

unsafe extern "C" fn destroy(e: *mut c_void) {
    if !e.is_null() {
        // SAFETY: from `create`.
        drop(unsafe { Box::from_raw(e as *mut Engine) });
    }
}

unsafe extern "C" fn caps(_e: *mut c_void, caps: *mut MouserCaps) {
    if let Some(c) = unsafe { caps.as_mut() } {
        c.flags = CAP_DIRECT_SURFACE | CAP_PIXELS;
        c.formats = FORMAT_XRGB8888;
        c.max_texture = 0;
    }
}

unsafe extern "C" fn evict(e: *mut c_void, key: u64) {
    if let Some(e) = unsafe { engine(e) } {
        e.r.evict(key);
    }
}

static TABLE_V2: MouserEngineV2 = MouserEngineV2 {
    base: MouserEngineV1 {
        abi: MOUSER_ENGINE_ABI_V2,
        size: std::mem::size_of::<MouserEngineV2>() as u32,
        name: c"skia".as_ptr(),
        create,
        resize,
        draw,
        destroy,
    },
    abi_min: MOUSER_ENGINE_ABI_V2,
    abi_max: MOUSER_ENGINE_ABI_V2,
    caps: Some(caps),
    set_scale: None,
    evict: Some(evict),
    set_vsync: None,
};

/// The v2 entry point.
#[unsafe(no_mangle)]
pub extern "C" fn mouser_engine_v2() -> *const MouserEngineV2 {
    &TABLE_V2
}
