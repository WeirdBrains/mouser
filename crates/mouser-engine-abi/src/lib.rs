//! The stable C ABI between Mouser and custom GPU rendering engines (v1).
//!
//! An engine is a shared library exporting
//! `extern "C" fn mouser_engine_v1() -> *const MouserEngineV1`. Mouser builds
//! a [`MouserFrame`] per frame (everything already composed: cells, cursor,
//! selection, UI, images) and the engine rasterises and presents it to the
//! window described by [`MouserSurface`]. The C header is `mouser_engine.h`.
//! See `docs/engines.md`.
#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_void};

/// ABI version; bumped only on incompatible changes.
pub const MOUSER_ENGINE_ABI: u32 = 1;
/// Name of the exported entry symbol (NUL-terminated).
pub const ENTRY_SYMBOL: &[u8] = b"mouser_engine_v1\0";

pub const SURFACE_WAYLAND: u32 = 1;
pub const SURFACE_XLIB: u32 = 2;
pub const SURFACE_XCB: u32 = 3;
pub const SURFACE_APPKIT: u32 = 4;
pub const SURFACE_WIN32: u32 = 5;
/// Offscreen: `window` points to a [`MouserPixels`]; `draw` fills it and Mouser
/// presents it. Used when the engine runs in Mouser's engine host process.
pub const SURFACE_PIXELS: u32 = 6;

pub const OP_RECT: u32 = 0;
pub const OP_GLYPH: u32 = 1;
pub const OP_IMAGE: u32 = 2;

/// `draw` results.
pub const DRAW_OK: c_int = 0;
pub const DRAW_AGAIN: c_int = 1;

/// The window to present to. For Wayland `display` is the `wl_display*` and
/// `window` the `wl_surface*`; for Xlib `window` holds the XID; for Xcb the
/// connection and window id; AppKit the `NSView*`; Win32 the `HWND` (display = HINSTANCE).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MouserSurface {
    pub kind: u32,
    pub display: *mut c_void,
    pub window: *mut c_void,
}

/// The pixel buffer for [`SURFACE_PIXELS`]: `0x00RRGGBB` per pixel, `stride`
/// pixels per row. Mouser may change `data`/`width`/`height` between calls (it
/// calls `resize` first); read them in every `draw`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MouserPixels {
    pub data: *mut u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

/// Solid rectangle; colours are 0xRRGGBB.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MouserRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub color: u32,
}

/// Glyph coverage bitmap (8-bit alpha, `stride` bytes per row) with its
/// top-left corner at (`x`, `y`), tinted with `color`. `key` is stable for the
/// same character and font generation, so engines may cache uploads.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MouserGlyph {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub stride: u32,
    pub color: u32,
    pub coverage: *const u8,
    pub key: u64,
}

/// Image placement: the source rectangle (`src_*`) of an RGBA8 image
/// (`img_w` x `img_h`, straight alpha) drawn scaled to `w` x `h` with its
/// top-left at (`x`, `y`). `key` is unique per uploaded image.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MouserImage {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub img_w: u32,
    pub img_h: u32,
    pub src_x: u32,
    pub src_y: u32,
    pub src_w: u32,
    pub src_h: u32,
    pub rgba: *const u8,
    pub key: u64,
}

/// One operation: `index` into the rects, glyphs or images array.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouserOp {
    pub kind: u32,
    pub index: u32,
}

/// A whole frame. Pointers are valid only during `draw`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MouserFrame {
    pub width: u32,
    pub height: u32,
    pub clear: u32,
    /// Damage rectangle; v1 always passes the whole surface.
    pub dirty_x: i32,
    pub dirty_y: i32,
    pub dirty_w: i32,
    pub dirty_h: i32,
    pub ops: *const MouserOp,
    pub op_count: usize,
    pub rects: *const MouserRect,
    pub rect_count: usize,
    pub glyphs: *const MouserGlyph,
    pub glyph_count: usize,
    pub images: *const MouserImage,
    pub image_count: usize,
}

/// The engine's function table.
#[repr(C)]
pub struct MouserEngineV1 {
    pub abi: u32,
    /// `size_of::<MouserEngineV1>()`.
    pub size: u32,
    /// NUL-terminated engine name.
    pub name: *const c_char,
    /// Create an engine for `surface`; NULL on failure with a message in `err`.
    pub create: unsafe extern "C" fn(surface: *const MouserSurface, w: u32, h: u32, err: *mut c_char, err_len: usize) -> *mut c_void,
    pub resize: unsafe extern "C" fn(engine: *mut c_void, w: u32, h: u32),
    /// Draw and present: `DRAW_OK`, `DRAW_AGAIN`, or negative on error.
    pub draw: unsafe extern "C" fn(engine: *mut c_void, frame: *const MouserFrame) -> c_int,
    pub destroy: unsafe extern "C" fn(engine: *mut c_void),
}

// SAFETY: the table is immutable static data.
unsafe impl Sync for MouserEngineV1 {}

impl MouserFrame {
    /// The ops as a slice.
    ///
    /// # Safety
    /// The frame's pointers must be valid (true inside `draw`).
    pub unsafe fn parts(&self) -> (&[MouserOp], &[MouserRect], &[MouserGlyph], &[MouserImage]) {
        unsafe fn s<'a, T>(p: *const T, n: usize) -> &'a [T] {
            if n == 0 || p.is_null() { &[] } else { unsafe { std::slice::from_raw_parts(p, n) } }
        }
        unsafe { (s(self.ops, self.op_count), s(self.rects, self.rect_count), s(self.glyphs, self.glyph_count), s(self.images, self.image_count)) }
    }
}

/// Copy `msg` into a C error buffer (truncated, NUL-terminated).
///
/// # Safety
/// `err` must point to `len` writable bytes or be null.
pub unsafe fn write_err(err: *mut c_char, len: usize, msg: &str) {
    if err.is_null() || len == 0 {
        return;
    }
    let n = msg.len().min(len - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(msg.as_ptr(), err as *mut u8, n);
        *err.add(n) = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_stable() {
        // Guards the v1 layout on 64-bit targets.
        #[cfg(target_pointer_width = "64")]
        {
            assert_eq!(std::mem::size_of::<MouserRect>(), 20);
            assert_eq!(std::mem::size_of::<MouserGlyph>(), 40);
            assert_eq!(std::mem::size_of::<MouserImage>(), 56);
            assert_eq!(std::mem::size_of::<MouserOp>(), 8);
            assert_eq!(std::mem::size_of::<MouserSurface>(), 24);
            assert_eq!(std::mem::size_of::<MouserFrame>(), 96);
            assert_eq!(std::mem::size_of::<MouserEngineV1>(), 48);
            assert_eq!(std::mem::size_of::<MouserPixels>(), 24);
        }
    }

    #[test]
    fn err_is_truncated_and_terminated() {
        let mut buf = [1 as c_char; 4];
        unsafe { write_err(buf.as_mut_ptr(), buf.len(), "hello") };
        assert_eq!(buf.map(|c| c as u8), *b"hel\0");
    }
}
