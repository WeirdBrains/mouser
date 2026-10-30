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
