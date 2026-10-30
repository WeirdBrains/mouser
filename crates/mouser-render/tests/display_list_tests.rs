//! The GPU path paints a recorded DisplayList; replaying it on the CPU canvas
//! must give exactly the pixels of the direct CPU path.
use mouser_render::text::DrawOp;
use mouser_render::ui::{self, Category};
use mouser_render::{Canvas, DisplayList, FontStack, Frame, render_frame};
use std::collections::BTreeMap;

fn frame_parts() -> (Vec<String>, ui::UiLayout) {
    let lines = vec!["~ ➜ ls -la under_score_".to_string(), "gjpqy |_| ▓ ✓".to_string()];
    let cats = BTreeMap::from([(Category::Ssh, vec!["alien-blue".to_string()])]);
    let layout = ui::build_sidebar_layout_metrics(900, 500, 9, 18, &cats, None);
    (lines, layout)
}

#[test]
fn display_list_replay_matches_cpu_canvas() {
    let (w, h) = (900usize, 500usize);
    let (lines, layout) = frame_parts();
    let frame = Frame {
        lines: &lines,
        cursor: Some((3, 1)),
        bg: 0x04192A,
        fg: 0xD8E1EA,
        ui: Some(&layout),
        status: Some(":theme"),
        accent: Some(0xE0404A),
        cursor_color: None,
        images: &[],
    };
    let mut fonts = FontStack::embedded(15.0);
    let mut direct = vec![0u32; w * h];
    render_frame(&mut Canvas::new(&mut direct, w, h), &mut fonts, &frame);

    let mut list = DisplayList::new(w, h);
    render_frame(&mut list, &mut fonts, &frame);
    assert!(list.ops.iter().any(|o| matches!(o, DrawOp::Glyph { .. })));
    assert!(list.ops.iter().any(|o| matches!(o, DrawOp::Rect { .. })));
    let mut replayed = vec![0u32; w * h];
    list.replay(&mut Canvas::new(&mut replayed, w, h), &mut fonts);
    assert!(direct == replayed, "display list differs from direct CPU render");
}

#[test]
fn images_draw_under_and_over_text_same_on_both_paths() {
    use mouser_render::ImageDraw;
    use std::sync::Arc;
    let (w, h) = (200usize, 100usize);
    let lines = vec!["███".to_string()];
    let img = |z: i32, rgba: [u8; 4], col: i32| ImageDraw {
        key: (z + 10) as u64,
        rgba: Arc::new(rgba.repeat(4)),
        img_w: 2,
        img_h: 2,
        src: (0, 0, 2, 2),
        col,
        row: 0,
        x_off: 0,
        y_off: 0,
        width: 20,
        height: 10,
        z,
    };
    let images = vec![img(-1, [255, 0, 0, 255], 0), img(1, [0, 0, 255, 255], 5)];
    let frame = Frame { lines: &lines, cursor: None, bg: 0, fg: 0xFFFFFF, ui: None, status: None, accent: None, cursor_color: None, images: &images };
    let mut fonts = FontStack::embedded(15.0);
    let mut direct = vec![0u32; w * h];
    render_frame(&mut Canvas::new(&mut direct, w, h), &mut fonts, &frame);
    let px = |x: usize, y: usize| direct[y * w + x];
    let cw = fonts.cell_w as usize;
    // Below-text image is covered by the full block glyph; the z=1 one shows on top.
    assert_eq!(px(8 + 1, 8 + 2), 0xFFFFFF);
    assert_eq!(px(8 + 5 * cw + 2, 8 + 2), 0x0000FF);
    let mut list = DisplayList::new(w, h);
    render_frame(&mut list, &mut fonts, &frame);
    let mut replayed = vec![0u32; w * h];
    list.replay(&mut Canvas::new(&mut replayed, w, h), &mut fonts);
    assert!(direct == replayed);
}
