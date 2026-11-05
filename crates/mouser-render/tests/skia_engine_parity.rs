//! The skia engine library (`crates/mouser-engine-skia`, through the C ABI
//! frame) draws what Mouser's former built-in skia engine drew: its CPU raster
//! frame must match `golden/skia-900x520.png` (rendered by the built-in
//! before it was removed) within a small tolerance.

use mouser_render::engine::FrameBuf;
use mouser_render::frame::{CellPaint, CursorShape, Frame, ImageDraw, render_frame};
use mouser_render::text::{DisplayList, DrawOp, FontStack, translucent};
use mouser_render::ui::{self, SidebarSpec, TabBar};
use std::collections::BTreeMap;
use std::sync::Arc;

const W: u32 = 900;
const H: u32 = 520;
/// Largest per-channel difference allowed (rounding in blending and filtering).
const TOLERANCE: u32 = 2;

fn gradient(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            v.extend_from_slice(&[(x * 255 / w) as u8, (y * 255 / h) as u8, 128, if (x / 8 + y / 8) % 2 == 0 { 255 } else { 160 }]);
        }
    }
    v
}

/// A terminal screen with styles, a cursor, tabs, the sidebar, a status
/// line, an image and a translucent overlay.
fn scene(fonts: &mut FontStack) -> DisplayList {
    let lines: Vec<String> = vec![
        "mouser@box:~/src/mouser$ ls -lah".into(),
        "drwxr-xr-x  8 mouser mouser 4.0K Nov  4 23:30 crates".into(),
        "-rw-r--r--  1 mouser mouser  12K Nov  4 23:30 CHANGELOG.md".into(),
        "λ unicode → ✓ — box ─┼─ ░▒▓".into(),
        "mouser@box:~/src/mouser$ ".into(),
    ];
    let mut styles: Vec<Vec<CellPaint>> = vec![Vec::new(); lines.len()];
    styles[1] = (0..10)
        .map(|_| CellPaint {
            fg: 0x5fafff,
            bg: Some(0x203040),
            flags: mouser_render::frame::paint::BOLD,
            underline: 1,
            ul_color: 0xff8800,
            comb: ['\0'; 10],
        })
        .collect();
    let images = vec![ImageDraw {
        key: 7,
        rgba: Arc::new(gradient(64, 48)),
        img_w: 64,
        img_h: 48,
        src: (0, 0, 64, 48),
        col: 30,
        row: 5,
        x_off: 0,
        y_off: 0,
        width: 128,
        height: 96,
        z: 1,
    }];
    let tabs = TabBar { titles: vec!["zsh".into(), "ssh box".into()], active: 1 };
    let layout = ui::build_sidebar(
        SidebarSpec { width_px: W as i32, height_px: H as i32, cell_w: 9, cell_h: 18, requested_width: Some(240), collapsed: false },
        &BTreeMap::new(),
        None,
    );
    let frame = Frame {
        lines: &lines,
        cursor: Some((25, 4)),
        bg: 0x101418,
        fg: 0xd0d0d0,
        ui: Some(&layout),
        status: Some("basic renderer — no GPU engine running yet"),
        accent: None,
        cursor_color: None,
        images: &images,
        styles: &styles,
        cursor_shape: CursorShape::Block,
        tabs: Some(&tabs),
    };
    let mut list = DisplayList::new(W as usize, H as usize);
    render_frame(&mut list, fonts, &frame);
    list.ops.push(DrawOp::Rect { x: 40, y: 60, w: 300, h: 80, color: translucent(0x3366ff, 96) });
    list
}

fn max_diff(a: &[u32], b: &[u32]) -> (u32, usize, usize) {
    let mut worst = (0, 0, 0);
    let mut differing = 0;
    for (i, (p, q)) in a.iter().zip(b).enumerate() {
        let d = [16, 8, 0].iter().map(|s| ((p >> s) & 0xff).abs_diff((q >> s) & 0xff)).max().unwrap();
        if d > 0 {
            differing += 1;
        }
        if d > worst.0 {
            worst = (d, i, 0);
        }
    }
    (worst.0, worst.1, differing)
}

#[test]
fn the_skia_engine_library_draws_like_the_former_built_in_engine() {
    let mut fonts = FontStack::embedded(16.0);
    let list = scene(&mut fonts);
    let builtin = {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/skia-900x520.png");
        let img = image::open(&path).unwrap().to_rgb8();
        img.pixels().map(|p| (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32).collect::<Vec<u32>>()
    };
    let mut lib = mouser_engine_skia::Renderer::raster(W, H).unwrap();
    let mut buf = FrameBuf::default();
    buf.keep_alpha = true;
    for round in 0..2 {
        let frame = buf.build(&list, &mut fonts);
        // SAFETY: `frame` points into `buf`, `list` and `fonts`, alive for the call.
        assert_eq!(unsafe { lib.draw(&frame) }, mouser_engine_abi::DRAW_OK);
        let b = lib.read_pixels().unwrap();
        let (worst, at, differing) = max_diff(&builtin, &b);
        eprintln!("round {round}: max channel diff {worst}, {differing} pixels differ");
        assert!(builtin.iter().any(|&p| p != builtin[0]), "the golden image has a scene");
        assert!(
            worst <= TOLERANCE,
            "round {round}: pixel ({}, {}) differs by {worst} (golden {:06x}, library {:06x})",
            at as u32 % W,
            at as u32 / W,
            builtin[at],
            b[at]
        );
    }
}
