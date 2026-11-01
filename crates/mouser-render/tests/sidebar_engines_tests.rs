//! Sidebar Engines section: rows in the given (priority) order above the
//! Appearance button, each a click target for `UiAction::UseEngine`.

use mouser_render::ui::{self, EngineState, UiAction};
use std::collections::BTreeMap;

fn spec(collapsed: bool) -> ui::SidebarSpec {
    ui::SidebarSpec { width_px: 1200, height_px: 800, cell_w: 9, cell_h: 18, requested_width: None, collapsed }
}

fn rows() -> Vec<(String, EngineState, Option<String>)> {
    vec![
        ("skia".into(), EngineState::Skipped, Some("the UI process crashed (signal 6 (SIGABRT)) while drawing with skia".into())),
        ("flat".into(), EngineState::Ok, None),
        ("mouser".into(), EngineState::Active, None),
    ]
}

fn centre(r: ui::Rect) -> (i32, i32) {
    (r.x + r.w / 2, r.y + r.h / 2)
}

#[test]
fn rows_follow_priority_above_appearance_and_click_to_use() {
    let mut l = ui::build_sidebar(spec(false), &BTreeMap::new(), None);
    let appearance = l.appearance_rect;
    ui::add_engines(&mut l, &rows(), 18);
    let names: Vec<_> = l.engines.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["skia", "flat", "mouser"]);
    assert_eq!(l.appearance_rect, appearance, "fits above the Appearance button");
    for w in l.engines.windows(2) {
        assert!(w[0].rect.y + w[0].rect.h <= w[1].rect.y, "rows don't overlap");
    }
    let last = l.engines.last().unwrap().rect;
    assert!(last.y + last.h < appearance.y);
    assert!(l.engines_title.y + l.engines_title.h <= l.engines[0].rect.y);
    assert!(l.engines[0].rect.h > l.engines[1].rect.h, "a skipped row has room for its reason");
    for e in &l.engines {
        assert!(l.sidebar_rect.x <= e.rect.x && e.rect.x + e.rect.w <= l.sidebar_rect.x + l.sidebar_rect.w);
        let (x, y) = centre(e.rect);
        assert_eq!(ui::hit_test(&l, x, y), UiAction::UseEngine(e.name.clone()));
        // The reason line of a skipped row is part of the click target.
        assert_eq!(ui::hit_test(&l, e.rect.x + 2, e.rect.y + e.rect.h - 2), UiAction::UseEngine(e.name.clone()));
    }
    let (x, y) = centre(l.engines_title);
    assert_eq!(ui::hit_test(&l, x, y), UiAction::None, "the title is not a button");
    let (x, y) = centre(l.appearance_rect);
    assert_eq!(ui::hit_test(&l, x, y), UiAction::OpenAppearance);
}

#[test]
fn many_connections_push_the_section_down_not_over_them() {
    let mut groups = BTreeMap::new();
    groups.insert(ui::Category::Ssh, (0..20).map(|i| format!("host{i}")).collect::<Vec<_>>());
    let mut l = ui::build_sidebar(spec(false), &groups, None);
    ui::add_engines(&mut l, &rows(), 18);
    let bottom = l.categories.iter().flat_map(|c| c.items.iter().map(|i| i.name_rect.y + i.name_rect.h)).max().unwrap();
    assert!(l.engines_title.y > bottom);
    assert!(l.appearance_rect.y > l.engines.last().unwrap().rect.y);
}

#[test]
fn collapsed_sidebar_and_panels_have_no_engine_targets() {
    let mut l = ui::build_sidebar(spec(true), &BTreeMap::new(), None);
    ui::add_engines(&mut l, &rows(), 18);
    assert!(l.engines.is_empty());
    let mut l = ui::build_sidebar(spec(false), &BTreeMap::new(), None);
    ui::add_engines(&mut l, &rows(), 18);
    let (x, y) = centre(l.engines[1].rect);
    l.panel = Some(ui::Panel { rect: ui::Rect { x: 0, y: 0, w: 100, h: 100 }, items: Vec::new() });
    assert_eq!(ui::hit_test(&l, x, y), UiAction::ClosePanel, "an open panel takes the click");
}

#[test]
fn state_labels() {
    assert_eq!(
        [EngineState::Active, EngineState::Ok, EngineState::Skipped, EngineState::Failed].map(|s| s.label()),
        ["active", "ok", "skipped", "failed"]
    );
}

fn layout() -> ui::UiLayout {
    let mut l = ui::build_sidebar(spec(false), &BTreeMap::new(), None);
    ui::add_engines(&mut l, &rows(), 18);
    l
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn press_on_a_row_starts_a_drag_candidate_elsewhere_not() {
    let l = layout();
    let (x, y) = centre(l.engines[1].rect);
    let d = ui::EngineDrag::press(&l, x, y).unwrap();
    assert_eq!((d.from, d.active), (1, false));
    assert_eq!(ui::engine_row_at(&l, x, y), Some(1));
    let (tx, ty) = centre(l.engines_title);
    assert!(ui::EngineDrag::press(&l, tx, ty).is_none(), "title is not draggable");
    let mut p = l.clone();
    p.panel = Some(ui::Panel { rect: ui::Rect { x: 0, y: 0, w: 10, h: 10 }, items: Vec::new() });
    assert!(ui::EngineDrag::press(&p, x, y).is_none(), "no drag under a panel");
}

#[test]
fn small_moves_are_a_click_past_the_threshold_a_drag() {
    let l = layout();
    let (x, y) = centre(l.engines[1].rect);
    let mut d = ui::EngineDrag::press(&l, x, y).unwrap();
    d.moved(x + ui::DRAG_THRESHOLD - 1, y - (ui::DRAG_THRESHOLD - 1));
    assert!(!d.active);
    assert_eq!(d.release(&l), ui::EngineDrop::Click("flat".into()), "click still switches");
    d.moved(x, y + ui::DRAG_THRESHOLD);
    assert!(d.active);
    d.moved(x, y);
    assert!(d.active, "once a drag, always a drag");
    assert_eq!(d.release(&l), ui::EngineDrop::Cancel, "dropped where it was");
}

#[test]
fn drop_index_by_row_middles() {
    let l = layout();
    let r: Vec<_> = l.engines.iter().map(|e| e.rect).collect();
    assert_eq!(ui::drop_index(&l, r[0].y), 0);
    assert_eq!(ui::drop_index(&l, r[0].y + r[0].h / 2 + 1), 1);
    assert_eq!(ui::drop_index(&l, r[1].y + r[1].h - 1), 2);
    assert_eq!(ui::drop_index(&l, r[2].y + r[2].h + 3), 3, "below the last row: the end");
    assert_eq!(ui::reordered(&names(&["a", "b", "c"]), 0, 3), names(&["b", "c", "a"]));
    assert_eq!(ui::reordered(&names(&["a", "b", "c"]), 2, 0), names(&["c", "a", "b"]));
    assert_eq!(ui::reordered(&names(&["a", "b", "c"]), 1, 2), names(&["a", "b", "c"]), "just below itself: no move");
}

#[test]
fn dragging_reorders_and_mouser_can_go_anywhere() {
    let l = layout();
    // mouser (last) dragged to the top.
    let (x, y) = centre(l.engines[2].rect);
    let mut d = ui::EngineDrag::press(&l, x, y).unwrap();
    d.moved(x, l.engines[0].rect.y + 1);
    assert_eq!(d.target(&l), Some(0));
    assert_eq!(d.release(&l), ui::EngineDrop::Reorder(names(&["mouser", "skia", "flat"])));
    // skia (first) dragged below the last row.
    let (x, y) = centre(l.engines[0].rect);
    let mut d = ui::EngineDrag::press(&l, x, y).unwrap();
    let last = l.engines[2].rect;
    d.moved(x, last.y + last.h + 2);
    assert_eq!(d.release(&l), ui::EngineDrop::Reorder(names(&["flat", "mouser", "skia"])));
}

#[test]
fn dropping_outside_the_section_cancels() {
    let l = layout();
    let (x, y) = centre(l.engines[0].rect);
    let mut d = ui::EngineDrag::press(&l, x, y).unwrap();
    d.moved(x, 40); // up among the connections
    assert_eq!(d.target(&l), None);
    assert_eq!(d.release(&l), ui::EngineDrop::Cancel);
    d.moved(l.sidebar_rect.x - 50, y); // over the terminal
    assert_eq!(d.release(&l), ui::EngineDrop::Cancel);
}

/// The drag is drawn: a ghost under the pointer and an insertion line in the
/// accent colour. `MOUSER_TEST_PNG=path` saves the frame to look at.
#[test]
fn drag_draws_a_ghost_and_an_insertion_line() {
    use mouser_render::{Canvas, FontStack, Frame, render_frame};
    let (w, h) = (1200usize, 800usize);
    let mut l = layout();
    let (x, y) = centre(l.engines[2].rect);
    let mut d = ui::EngineDrag::press(&l, x, y).unwrap();
    d.moved(x, l.engines[0].rect.y + 3);
    l.engine_drag = Some(d);
    let accent = 0x2FD8E5u32;
    let render = |l: &ui::UiLayout| {
        let mut fonts = FontStack::embedded(15.0);
        let mut buf = vec![0u32; w * h];
        let lines: Vec<String> = Vec::new();
        let frame = Frame {
            lines: &lines,
            cursor: None,
            bg: 0x101820,
            fg: 0xE0E0E0,
            ui: Some(l),
            status: None,
            accent: Some(accent),
            cursor_color: None,
            images: &[],
            styles: &[],
            cursor_shape: Default::default(),
            tabs: None,
        };
        render_frame(&mut Canvas::new(&mut buf, w, h), &mut fonts, &frame);
        buf
    };
    let with = render(&l);
    if let Some(p) = std::env::var_os("MOUSER_TEST_PNG") {
        let rgba: Vec<u8> = with.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]).collect();
        image::save_buffer(p, &rgba, w as u32, h as u32, image::ColorType::Rgba8).unwrap();
    }
    let line_y = (l.engines[0].rect.y - 2) as usize;
    let r = l.engines[0].rect;
    let on_line = (r.x as usize..(r.x + r.w) as usize).filter(|&x| with[line_y * w + x] & 0xFFFFFF == accent).count();
    assert!(on_line as i32 > r.w * 9 / 10, "insertion line above the first row ({on_line} px)");
    l.engine_drag = None;
    let without = render(&l);
    assert_ne!(with, without);
    assert!(without[line_y * w + (r.x + r.w / 2) as usize] & 0xFFFFFF != accent);
}
