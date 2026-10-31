//! Example custom Mouser engine, "flat".
//!
//! It rasterises the frame on the CPU and presents it with softbuffer, which
//! keeps the example small while exercising the whole ABI: rects, glyph
//! coverage, images, resize and redraw. Build with
//! `cargo build -p mouser-engine-example --release` and install with
//! `mouser engine install crates/mouser-engine-example` after copying the
//! library next to `engine.toml` (see docs/engines.md).
//!
//! Test hooks: `MOUSER_FLAT_FAIL=create` makes `create` fail,
//! `MOUSER_FLAT_FAIL=draw` makes `draw` return an error, and
//! `MOUSER_FLAT_ABI=<n>` reports another ABI version, and
//! `MOUSER_FLAT_CRASH_AT=<n>` aborts the process in the n-th `draw` (to test
//! crash isolation). Supports window surfaces (Wayland, Xlib) and
//! `SURFACE_PIXELS` (Mouser's engine host).

use mouser_engine_abi::*;
use raw_window_handle as rwh;
use std::ffi::{c_char, c_int, c_void};
use std::num::NonZeroU32;
use std::ptr::NonNull;

/// Raw handles from a [`MouserSurface`].
#[derive(Clone, Copy)]
struct Handles {
    display: rwh::RawDisplayHandle,
    window: rwh::RawWindowHandle,
}

impl rwh::HasDisplayHandle for Handles {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        // SAFETY: Mouser keeps the window alive while the engine exists.
        Ok(unsafe { rwh::DisplayHandle::borrow_raw(self.display) })
    }
}
impl rwh::HasWindowHandle for Handles {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        // SAFETY: as above.
        Ok(unsafe { rwh::WindowHandle::borrow_raw(self.window) })
    }
}

fn handles(s: &MouserSurface) -> Result<Handles, String> {
    let nn = |p: *mut c_void| NonNull::new(p).ok_or_else(|| "null surface handle".to_string());
    match s.kind {
        SURFACE_WAYLAND => Ok(Handles {
            display: rwh::WaylandDisplayHandle::new(nn(s.display)?).into(),
            window: rwh::WaylandWindowHandle::new(nn(s.window)?).into(),
        }),
        SURFACE_XLIB => Ok(Handles {
            display: rwh::XlibDisplayHandle::new(Some(nn(s.display)?), 0).into(),
            window: rwh::XlibWindowHandle::new(s.window as std::ffi::c_ulong).into(),
        }),
        k => Err(format!("unsupported surface kind {k}")),
    }
}

/// The frame rasteriser (shared with tests).
pub fn rasterize(buf: &mut [u32], width: usize, height: usize, frame: &MouserFrame) {
    buf.fill(frame.clear);
    // SAFETY: called from `draw` with Mouser's frame.
    let (ops, rects, glyphs, images) = unsafe { frame.parts() };
    let blend = |d: u32, s: u32, a: u32| -> u32 {
        let ch = |sh: u32| ((((s >> sh) & 0xff) * a + ((d >> sh) & 0xff) * (255 - a)) / 255) << sh;
        ch(16) | ch(8) | ch(0)
    };
    for op in ops {
        match op.kind {
            OP_RECT => {
                let Some(r) = rects.get(op.index as usize) else { continue };
                let (x0, y0) = (r.x.max(0) as usize, r.y.max(0) as usize);
                let x1 = ((r.x + r.w).max(0) as usize).min(width);
                let y1 = ((r.y + r.h).max(0) as usize).min(height);
                for y in y0..y1 {
                    if x0 < x1 {
                        buf[y * width + x0..y * width + x1].fill(r.color);
                    }
                }
            }
            OP_GLYPH => {
                let Some(g) = glyphs.get(op.index as usize) else { continue };
                if g.coverage.is_null() {
                    continue;
                }
                // SAFETY: Mouser passes stride * h bytes.
                let cov = unsafe { std::slice::from_raw_parts(g.coverage, (g.stride * g.h) as usize) };
                for iy in 0..g.h as i32 {
                    let py = g.y + iy;
                    if py < 0 || py >= height as i32 {
                        continue;
                    }
                    for ix in 0..g.w as i32 {
                        let px = g.x + ix;
                        if px < 0 || px >= width as i32 {
                            continue;
                        }
                        let a = cov[(iy as u32 * g.stride + ix as u32) as usize] as u32;
                        if a > 0 {
                            let o = py as usize * width + px as usize;
                            buf[o] = blend(buf[o], g.color, a);
                        }
                    }
                }
            }
            OP_IMAGE => {
                let Some(im) = images.get(op.index as usize) else { continue };
                if im.rgba.is_null() || im.img_w == 0 || im.img_h == 0 {
                    continue;
                }
                // SAFETY: Mouser passes img_w * img_h * 4 bytes.
                let px = unsafe { std::slice::from_raw_parts(im.rgba, (im.img_w * im.img_h * 4) as usize) };
                let (dw, dh) = (im.w.max(1) as i32, im.h.max(1) as i32);
                for y in im.y.max(0)..(im.y + dh).min(height as i32) {
                    let iy = (im.src_y + ((y - im.y) as u64 * im.src_h as u64 / dh as u64) as u32).min(im.img_h - 1);
                    for x in im.x.max(0)..(im.x + dw).min(width as i32) {
                        let ix = (im.src_x + ((x - im.x) as u64 * im.src_w as u64 / dw as u64) as u32).min(im.img_w - 1);
                        let o = ((iy * im.img_w + ix) * 4) as usize;
                        let c = (px[o] as u32) << 16 | (px[o + 1] as u32) << 8 | px[o + 2] as u32;
                        let d = y as usize * width + x as usize;
                        buf[d] = blend(buf[d], c, px[o + 3] as u32);
                    }
                }
            }
            _ => {}
        }
    }
}

enum Target {
    Window { _context: softbuffer::Context<Handles>, surface: softbuffer::Surface<Handles, Handles> },
    Pixels(*const MouserPixels),
}

struct Flat {
    target: Target,
    width: u32,
    height: u32,
    frames: u64,
}

fn fail(what: &str) -> bool {
    std::env::var("MOUSER_FLAT_FAIL").is_ok_and(|v| v == what)
}

unsafe extern "C" fn create(surface: *const MouserSurface, w: u32, h: u32, err: *mut c_char, err_len: usize) -> *mut c_void {
    let made = (|| -> Result<Flat, String> {
        if fail("create") {
            return Err("MOUSER_FLAT_FAIL=create".into());
        }
        // SAFETY: Mouser passes a valid surface or null.
        let s = unsafe { surface.as_ref() }.ok_or("null surface")?;
        if s.kind == SURFACE_PIXELS {
            if s.window.is_null() {
                return Err("null pixel buffer".into());
            }
            return Ok(Flat { target: Target::Pixels(s.window as *const MouserPixels), width: w.max(1), height: h.max(1), frames: 0 });
        }
        let h_ = handles(s)?;
        let context = softbuffer::Context::new(h_).map_err(|e| e.to_string())?;
        let mut sf = softbuffer::Surface::new(&context, h_).map_err(|e| e.to_string())?;
        sf.resize(NonZeroU32::new(w.max(1)).unwrap(), NonZeroU32::new(h.max(1)).unwrap()).map_err(|e| e.to_string())?;
        Ok(Flat { target: Target::Window { _context: context, surface: sf }, width: w.max(1), height: h.max(1), frames: 0 })
    })();
    match made {
        Ok(f) => Box::into_raw(Box::new(f)) as *mut c_void,
        Err(e) => {
            // SAFETY: Mouser passes err_len writable bytes.
            unsafe { write_err(err, err_len, &e) };
            std::ptr::null_mut()
        }
    }
}

unsafe extern "C" fn resize(engine: *mut c_void, w: u32, h: u32) {
    // SAFETY: `engine` came from `create`.
    let Some(f) = (unsafe { (engine as *mut Flat).as_mut() }) else { return };
    f.width = w.max(1);
    f.height = h.max(1);
    if let Target::Window { surface, .. } = &mut f.target {
        let _ = surface.resize(NonZeroU32::new(f.width).unwrap(), NonZeroU32::new(f.height).unwrap());
    }
}

unsafe extern "C" fn draw(engine: *mut c_void, frame: *const MouserFrame) -> c_int {
    if fail("draw") {
        return -1;
    }
    // SAFETY: pointers come from Mouser and are valid for this call.
    let (Some(f), Some(frame)) = (unsafe { (engine as *mut Flat).as_mut() }, unsafe { frame.as_ref() }) else { return -2 };
    f.frames += 1;
    if std::env::var("MOUSER_FLAT_CRASH_AT").ok().and_then(|v| v.parse::<u64>().ok()) == Some(f.frames) {
        std::process::abort();
    }
    if frame.width != f.width || frame.height != f.height {
        unsafe { resize(engine, frame.width, frame.height) };
    }
    match &mut f.target {
        Target::Pixels(p) => {
            // SAFETY: Mouser's host keeps the buffer valid during draw.
            let Some(px) = (unsafe { p.as_ref() }) else { return -3 };
            let (w, h) = (px.width as usize, px.height as usize);
            if px.data.is_null() || px.stride as usize != w {
                return -4;
            }
            let buf = unsafe { std::slice::from_raw_parts_mut(px.data, w * h) };
            rasterize(buf, w, h, frame);
            DRAW_OK
        }
        Target::Window { surface, .. } => {
            let (w, h) = (f.width as usize, f.height as usize);
            let Ok(mut buf) = surface.buffer_mut() else { return DRAW_AGAIN };
            if buf.len() < w * h {
                return DRAW_AGAIN;
            }
            rasterize(&mut buf[..w * h], w, h, frame);
            if buf.present().is_err() { DRAW_AGAIN } else { DRAW_OK }
        }
    }
}

unsafe extern "C" fn destroy(engine: *mut c_void) {
    if !engine.is_null() {
        // SAFETY: `engine` came from `create`.
        drop(unsafe { Box::from_raw(engine as *mut Flat) });
    }
}

static TABLE: MouserEngineV1 = MouserEngineV1 {
    abi: MOUSER_ENGINE_ABI,
    size: std::mem::size_of::<MouserEngineV1>() as u32,
    name: c"flat".as_ptr(),
    create,
    resize,
    draw,
    destroy,
};

static mut ODD: MouserEngineV1 = MouserEngineV1 { abi: 0, ..TABLE };

/// The engine entry point.
#[unsafe(no_mangle)]
pub extern "C" fn mouser_engine_v1() -> *const MouserEngineV1 {
    if let Some(abi) = std::env::var("MOUSER_FLAT_ABI").ok().and_then(|v| v.parse().ok()) {
        // SAFETY: test hook; single-threaded use in tests.
        unsafe {
            ODD.abi = abi;
            return &raw const ODD;
        }
    }
    &TABLE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterizes_rect_glyph_and_image() {
        let rects = [MouserRect { x: 1, y: 1, w: 2, h: 2, color: 0xff0000 }];
        let cov = [255u8, 0, 128, 255];
        let glyphs = [MouserGlyph { x: 0, y: 0, w: 2, h: 2, stride: 2, color: 0x00ff00, coverage: cov.as_ptr(), key: 1 }];
        let rgba = [0u8, 0, 255, 255];
        let images =
            [MouserImage { x: 3, y: 3, w: 1, h: 1, img_w: 1, img_h: 1, src_x: 0, src_y: 0, src_w: 1, src_h: 1, rgba: rgba.as_ptr(), key: 2 }];
        let ops = [MouserOp { kind: OP_RECT, index: 0 }, MouserOp { kind: OP_GLYPH, index: 0 }, MouserOp { kind: OP_IMAGE, index: 0 }];
        let frame = MouserFrame {
            width: 4,
            height: 4,
            clear: 0x111111,
            dirty_x: 0,
            dirty_y: 0,
            dirty_w: 4,
            dirty_h: 4,
            ops: ops.as_ptr(),
            op_count: 3,
            rects: rects.as_ptr(),
            rect_count: 1,
            glyphs: glyphs.as_ptr(),
            glyph_count: 1,
            images: images.as_ptr(),
            image_count: 1,
        };
        let mut buf = vec![0u32; 16];
        rasterize(&mut buf, 4, 4, &frame);
        assert_eq!(buf[0], 0x00ff00);
        assert_eq!(buf[1], 0x111111);
        assert_eq!(buf[5], 0x00ff00); // glyph over rect, full coverage
        assert_eq!(buf[6], 0xff0000);
        assert_eq!(buf[15], 0x0000ff);
    }

    #[test]
    fn create_reports_errors() {
        let mut err = [0 as c_char; 64];
        let s = MouserSurface { kind: 99, display: std::ptr::null_mut(), window: std::ptr::null_mut() };
        let p = unsafe { create(&s, 10, 10, err.as_mut_ptr(), err.len()) };
        assert!(p.is_null());
        let msg = unsafe { std::ffi::CStr::from_ptr(err.as_ptr()) }.to_string_lossy();
        assert!(msg.contains("unsupported surface"), "{msg}");
    }
}
