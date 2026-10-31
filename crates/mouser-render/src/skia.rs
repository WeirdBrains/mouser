//! The `skia` engine: Skia (Ganesh) over OpenGL through glutin.
//!
//! Rects are Skia rects; glyph coverage goes into one A8 atlas image drawn
//! per glyph with the glyph colour (Skia tints alpha-only images with the
//! paint colour, and batches quads that share the atlas); images are Skia
//! images cached by key and drawn nearest-neighbour like the other engines.

use crate::engine::Engine;
use crate::text::{DisplayList, DrawOp, FontStack};
use anyhow::{Context as _, Result, anyhow};
use glutin::config::{ConfigTemplateBuilder, GlConfig};
use glutin::context::{ContextApi, ContextAttributesBuilder, PossiblyCurrentContext};
use glutin::display::{Display, DisplayApiPreference, GlDisplay};
use glutin::prelude::{GlSurface, NotCurrentGlContext};
use glutin::surface::{Surface as GlSurfaceT, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use skia_safe::gpu::{self, DirectContext, SurfaceOrigin, backend_render_targets, gl::FramebufferInfo};
use skia_safe::{AlphaType, Color, ColorType, Data, Image, ImageInfo, Paint, Rect, SamplingOptions, Surface, canvas::SrcRectConstraint};
use std::collections::HashMap;
use std::ffi::CString;
use std::num::NonZeroU32;
use std::sync::Arc;
use winit::window::Window;

const ATLAS: usize = 2048;

/// Shelf-packed A8 glyph atlas mirrored into a Skia image.
struct Atlas {
    pixels: Vec<u8>,
    slots: HashMap<char, (i32, i32, i32, i32)>,
    x: i32,
    y: i32,
    row_h: i32,
    generation: u64,
    image: Option<Image>,
    dirty: bool,
}

impl Atlas {
    fn new(generation: u64) -> Self {
        Self { pixels: vec![0; ATLAS * ATLAS], slots: HashMap::new(), x: 0, y: 0, row_h: 0, generation, image: None, dirty: true }
    }

    /// Slot for `ch`, adding it if needed; None when the atlas is full.
    fn slot(&mut self, fonts: &mut FontStack, ch: char) -> Option<(i32, i32, i32, i32)> {
        if let Some(s) = self.slots.get(&ch) {
            return Some(*s);
        }
        let g = fonts.glyph(ch);
        let (w, h) = (g.metrics.width as i32, g.metrics.height as i32);
        if w == 0 || h == 0 || w > ATLAS as i32 || h > ATLAS as i32 {
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
        for row in 0..h as usize {
            let o = (self.y as usize + row) * ATLAS + self.x as usize;
            self.pixels[o..o + w as usize].copy_from_slice(&g.bitmap[row * w as usize..(row + 1) * w as usize]);
        }
        let s = (self.x, self.y, w, h);
        self.slots.insert(ch, s);
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

pub struct SkiaEngine {
    // Field order is drop order: Skia first, then GL objects, then the window.
    surface: Surface,
    gr: DirectContext,
    fb: FramebufferInfo,
    samples: usize,
    stencil: usize,
    gl_surface: GlSurfaceT<WindowSurface>,
    gl_context: PossiblyCurrentContext,
    atlas: Atlas,
    images: HashMap<u64, Image>,
    renderer: String,
    width: u32,
    height: u32,
    _window: Arc<Window>,
}

fn nz(v: u32) -> NonZeroU32 {
    NonZeroU32::new(v.max(1)).unwrap()
}

fn make_surface(gr: &mut DirectContext, fb: FramebufferInfo, samples: usize, stencil: usize, w: u32, h: u32) -> Result<Surface> {
    let target = backend_render_targets::make_gl((w.max(1) as i32, h.max(1) as i32), samples, stencil, fb);
    gpu::surfaces::wrap_backend_render_target(gr, &target, SurfaceOrigin::BottomLeft, ColorType::RGBA8888, None, None)
        .ok_or_else(|| anyhow!("Skia could not wrap the GL framebuffer"))
}

impl SkiaEngine {
    pub fn new(window: Arc<Window>, width: u32, height: u32) -> Result<Self> {
        let raw_display = window.display_handle().map_err(|e| anyhow!("display handle: {e}"))?.as_raw();
        let raw_window = window.window_handle().map_err(|e| anyhow!("window handle: {e}"))?.as_raw();
        #[cfg(all(unix, not(target_os = "macos")))]
        let pref = DisplayApiPreference::EglThenGlx(Box::new(winit::platform::x11::register_xlib_error_hook));
        #[cfg(target_os = "macos")]
        let pref = DisplayApiPreference::Cgl;
        #[cfg(windows)]
        let pref = DisplayApiPreference::WglThenEgl(Some(raw_window));
        // SAFETY: the handles belong to `window`, which this engine keeps alive.
        let display = unsafe { Display::new(raw_display, pref) }.context("GL display")?;
        let template = ConfigTemplateBuilder::new().compatible_with_native_window(raw_window).with_alpha_size(8).build();
        // SAFETY: as above.
        let config = unsafe { display.find_configs(template) }
            .context("GL configs")?
            .reduce(|a, c| if c.num_samples() < a.num_samples() { c } else { a })
            .ok_or_else(|| anyhow!("no GL config for this window"))?;
        let attrs = ContextAttributesBuilder::new().build(Some(raw_window));
        let gles = ContextAttributesBuilder::new().with_context_api(ContextApi::Gles(None)).build(Some(raw_window));
        // SAFETY: as above.
        let ctx = unsafe { display.create_context(&config, &attrs).or_else(|_| display.create_context(&config, &gles)) }.context("GL context")?;
        let sattrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(raw_window, nz(width), nz(height));
        // SAFETY: as above.
        let gl_surface = unsafe { display.create_window_surface(&config, &sattrs) }.context("GL window surface")?;
        let gl_context = ctx.make_current(&gl_surface).context("making the GL context current")?;
        let _ = gl_surface.set_swap_interval(&gl_context, SwapInterval::DontWait);
        let interface = skia_safe::gpu::gl::Interface::new_load_with(|name| {
            if name == "eglGetCurrentDisplay" {
                return std::ptr::null();
            }
            CString::new(name).map(|n| display.get_proc_address(&n)).unwrap_or(std::ptr::null())
        })
        .ok_or_else(|| anyhow!("Skia could not load GL"))?;
        let mut gr = gpu::direct_contexts::make_gl(interface, None).ok_or_else(|| anyhow!("Skia GL context"))?;
        let fb = FramebufferInfo { fboid: 0, format: skia_safe::gpu::gl::Format::RGBA8.into(), ..Default::default() };
        let (samples, stencil) = (config.num_samples() as usize, config.stencil_size() as usize);
        let surface = make_surface(&mut gr, fb, samples, stencil, width, height)?;
        let renderer = format!("{:?}, {} samples", display.version_string(), samples);
        Ok(Self {
            surface,
            gr,
            fb,
            samples,
            stencil,
            gl_surface,
            gl_context,
            atlas: Atlas::new(0),
            images: HashMap::new(),
            renderer,
            width: width.max(1),
            height: height.max(1),
            _window: window,
        })
    }

    fn image_for(&mut self, img: &crate::frame::ImageDraw) -> Option<Image> {
        if let Some(i) = self.images.get(&img.key) {
            return Some(i.clone());
        }
        let info = ImageInfo::new((img.img_w as i32, img.img_h as i32), ColorType::RGBA8888, AlphaType::Unpremul, None);
        let i = skia_safe::images::raster_from_data(&info, Data::new_copy(&img.rgba), img.img_w as usize * 4)?;
        self.images.insert(img.key, i.clone());
        Some(i)
    }
}

impl Drop for SkiaEngine {
    fn drop(&mut self) {
        self.gr.release_resources_and_abandon();
    }
}

fn color(c: u32) -> Color {
    Color::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

impl Engine for SkiaEngine {
    fn name(&self) -> &str {
        "skia"
    }
    fn description(&self) -> String {
        format!("Skia GL, {}", self.renderer)
    }
    fn resize(&mut self, width: u32, height: u32) {
        let (w, h) = (width.max(1), height.max(1));
        if (w, h) == (self.width, self.height) {
            return;
        }
        self.width = w;
        self.height = h;
        self.gl_surface.resize(&self.gl_context, nz(w), nz(h));
        if let Ok(s) = make_surface(&mut self.gr, self.fb, self.samples, self.stencil, w, h) {
            self.surface = s;
        }
    }

    fn draw(&mut self, list: &DisplayList, fonts: &mut FontStack) -> Result<bool> {
        if (list.width as u32, list.height as u32) != (self.width, self.height) {
            self.resize(list.width as u32, list.height as u32);
        }
        if self.atlas.generation != fonts.generation {
            self.atlas = Atlas::new(fonts.generation);
        }
        // Make sure every glyph has a slot before drawing (start over once if full).
        let mut full = false;
        for op in &list.ops {
            if let DrawOp::Glyph { ch, .. } = *op
                && self.atlas.slot(fonts, ch).is_none()
                && fonts.glyph(ch).metrics.width > 0
            {
                full = true;
                break;
            }
        }
        if full {
            self.atlas = Atlas::new(fonts.generation);
        }
        let used: std::collections::HashSet<u64> = list.images.iter().map(|i| i.key).collect();
        self.images.retain(|k, _| used.contains(k));
        let images: Vec<Option<Image>> = list.images.iter().map(|i| self.image_for(i)).collect();
        let atlas = self.atlas.image().cloned();
        let near = SamplingOptions::default();
        let mut paint = Paint::default();
        paint.set_anti_alias(false);
        let canvas = self.surface.canvas();
        canvas.clear(color(list.clear));
        for op in &list.ops {
            match *op {
                DrawOp::Rect { x, y, w, h, color: c } => {
                    paint.set_color(color(c));
                    canvas.draw_rect(Rect::from_xywh(x as f32, y as f32, w as f32, h as f32), &paint);
                }
                DrawOp::Glyph { x, y, ch, color: c } => {
                    let (Some(a), Some(&(sx, sy, w, h))) = (&atlas, self.atlas.slots.get(&ch)) else { continue };
                    paint.set_color(color(c));
                    canvas.draw_image_rect_with_sampling_options(
                        a,
                        Some((&Rect::from_xywh(sx as f32, sy as f32, w as f32, h as f32), SrcRectConstraint::Strict)),
                        Rect::from_xywh(x as f32, y as f32, w as f32, h as f32),
                        near,
                        &paint,
                    );
                }
                DrawOp::Image { index, x, y } => {
                    let (Some(Some(im)), Some(d)) = (images.get(index), list.images.get(index)) else { continue };
                    let (sx, sy, sw, sh) = d.src;
                    paint.set_color(Color::WHITE);
                    canvas.draw_image_rect_with_sampling_options(
                        im,
                        Some((&Rect::from_xywh(sx as f32, sy as f32, sw as f32, sh as f32), SrcRectConstraint::Strict)),
                        Rect::from_xywh(x as f32, y as f32, d.width as f32, d.height as f32),
                        near,
                        &paint,
                    );
                }
            }
        }
        let t0 = std::time::Instant::now();
        let _ = self.gr.flush_and_submit();
        let t1 = std::time::Instant::now();
        self.gl_surface.swap_buffers(&self.gl_context).map_err(|e| anyhow!("swap: {e}"))?;
        log::debug!(target: "mouser_frame", "skia flush {:.2} ms swap {:.2} ms", (t1 - t0).as_secs_f64() * 1e3, t1.elapsed().as_secs_f64() * 1e3);
        Ok(true)
    }
}
