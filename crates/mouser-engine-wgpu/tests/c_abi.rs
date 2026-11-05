//! The engine through its C ABI table, on the pixel-host surface: negotiates
//! v2, reports caps, draws a frame into Mouser's pixel buffer, resizes and
//! evicts. Skipped without a wgpu adapter unless `MOUSER_REQUIRE_GPU` is set.

use mouser_engine_abi::*;
use mouser_engine_wgpu::mouser_engine_v2;

#[test]
fn draws_into_the_pixel_host_buffer_through_the_c_abi() {
    // SAFETY: a pointer to static data.
    let t = unsafe { &*mouser_engine_v2() };
    assert_eq!(t.base.abi, MOUSER_ENGINE_ABI_V2);
    assert_eq!(negotiate(t.abi_min, t.abi_max), Ok(2));
    // SAFETY: NUL-terminated static name.
    assert_eq!(unsafe { std::ffi::CStr::from_ptr(t.base.name) }.to_str().unwrap(), "mouser");

    let (w, h) = (64u32, 32u32);
    let mut px = vec![0u32; (w * h) as usize];
    let mut pixels = MouserPixels { data: px.as_mut_ptr(), width: w, height: h, stride: w };
    let surface = MouserSurface { kind: SURFACE_PIXELS, display: std::ptr::null_mut(), window: (&raw mut pixels).cast() };
    let mut err = [0 as std::ffi::c_char; 256];
    // SAFETY: valid pointers for the call.
    let e = unsafe { (t.base.create)(&surface, w, h, err.as_mut_ptr(), err.len()) };
    if e.is_null() {
        // SAFETY: NUL-terminated by write_err.
        let why = unsafe { std::ffi::CStr::from_ptr(err.as_ptr()) }.to_string_lossy().into_owned();
        assert!(std::env::var_os("MOUSER_REQUIRE_GPU").is_none_or(|v| v.is_empty()), "MOUSER_REQUIRE_GPU is set: {why}");
        eprintln!("no wgpu adapter ({why}); skipped");
        return;
    }
    let mut caps = MouserCaps { size: std::mem::size_of::<MouserCaps>() as u32, ..Default::default() };
    // SAFETY: valid engine and caps.
    unsafe { (t.caps.unwrap())(e, &mut caps) };
    assert_ne!(caps.flags & CAP_PIXELS, 0);
    assert!(caps.max_texture >= 2048, "{caps:?}");

    let rects =
        [MouserRect { x: 0, y: 0, w: 32, h: 32, color: 0xff0000 }, MouserRect { x: 32, y: 0, w: 32, h: 32, color: (255 - 128) << 24 | 0x0000ff }];
    let ops = [MouserOp { kind: OP_RECT, index: 0 }, MouserOp { kind: OP_RECT, index: 1 }];
    let frame = MouserFrame {
        width: w,
        height: h,
        clear: 0x000000,
        dirty_x: 0,
        dirty_y: 0,
        dirty_w: w as i32,
        dirty_h: h as i32,
        ops: ops.as_ptr(),
        op_count: 2,
        rects: rects.as_ptr(),
        rect_count: 2,
        glyphs: std::ptr::null(),
        glyph_count: 0,
        images: std::ptr::null(),
        image_count: 0,
    };
    // SAFETY: valid engine and frame.
    assert_eq!(unsafe { (t.base.draw)(e, &frame) }, DRAW_OK);
    assert_eq!(px[0], 0xff0000, "opaque red");
    let half = px[40];
    assert!(half & 0xff0000 == 0 && (0x7d..=0x82).contains(&(half & 0xff)), "half-transparent blue over black: {half:06x}");
    // SAFETY: valid engine.
    unsafe {
        (t.evict.unwrap())(e, 1);
        (t.base.resize)(e, 32, 16);
        (t.base.destroy)(e);
    }
}
