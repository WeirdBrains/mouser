//! The C ABI frame carries everything the core composes: rasterising it the
//! way the example custom engine does gives the CPU reference image.
use mouser_render::engine::FrameBuf;
use mouser_render::text::DrawOp;
use mouser_render::ui::{self, Category};
use mouser_render::{Canvas, DisplayList, FontStack, Frame, ImageDraw, render_frame};
use std::collections::BTreeMap;
use std::sync::Arc;

#[test]
fn abi_frame_matches_cpu_reference() {
    let (w, h) = (900usize, 500usize);
    let lines = vec!["~ ➜ ls -la under_score_".to_string(), "gjpqy |_| ▓ ✓ ███".to_string()];
    let cats = BTreeMap::from([(Category::Ssh, vec!["alien-blue".to_string()])]);
    let layout = ui::build_sidebar_layout_metrics(900, 500, 9, 18, &cats, None);
    let mut rgba = Vec::new();
    for i in 0..16u8 {
        rgba.extend_from_slice(&[i * 16, 255 - i * 16, 128, if i % 3 == 0 { 128 } else { 255 }]);
    }
    let images = vec![ImageDraw {
        key: 7,
        rgba: Arc::new(rgba),
        img_w: 4,
        img_h: 4,
        src: (0, 0, 4, 4),
        col: 2,
        row: 1,
        x_off: 0,
        y_off: 0,
        width: 37,
        height: 23,
        z: 1,
    }];
    let frame = Frame {
        lines: &lines,
        cursor: Some((3, 1)),
        bg: 0x04192A,
        fg: 0xD8E1EA,
        ui: Some(&layout),
        status: Some(":theme"),
        accent: Some(0xE0404A),
        cursor_color: None,
        images: &images,
        styles: &[],
        cursor_shape: Default::default(),
        tabs: None,
    };
    let mut fonts = FontStack::embedded(15.0);
    let mut list = DisplayList::new(w, h);
    render_frame(&mut list, &mut fonts, &frame);
    assert!(list.ops.iter().any(|o| matches!(o, DrawOp::Image { .. })));
    let mut reference = vec![0u32; w * h];
    list.replay(&mut Canvas::new(&mut reference, w, h), &mut fonts);

    let mut buf = FrameBuf::default();
    let f = buf.build(&list, &mut fonts);
    assert_eq!((f.width, f.height, f.clear), (w as u32, h as u32, 0x04192A));
    let mut out = vec![0u32; w * h];
    mouser_engine_flat::rasterize(&mut out, w, h, &f);
    // Blending may round differently by one step per channel.
    let off = reference
        .iter()
        .zip(&out)
        .filter(|(a, b)| (0..3).any(|s| (((**a >> (s * 8)) & 0xff) as i32 - ((**b >> (s * 8)) & 0xff) as i32).abs() > 1))
        .count();
    assert_eq!(off, 0, "{off} pixels differ");
}
