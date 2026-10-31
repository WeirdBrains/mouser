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
