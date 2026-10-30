use mouser_render::text::FontStack;
use mouser_render::ui::{self, SidebarSpec, UiAction};
use mouser_render::{PADDING, grid_size};
use std::collections::BTreeMap;

fn spec(width: i32, requested: Option<i32>, collapsed: bool) -> SidebarSpec {
    SidebarSpec { width_px: width, height_px: 700, cell_w: 9, cell_h: 18, requested_width: requested, collapsed }
}

#[test]
fn width_is_clamped_to_bounds() {
    let (min, max) = ui::sidebar_width_bounds(1200, 9);
    assert_eq!(min, 200);
    assert_eq!(max, 720);
    assert_eq!(ui::clamp_sidebar_width(10, 1200, 9), min);
    assert_eq!(ui::clamp_sidebar_width(5000, 1200, 9), max);
    assert_eq!(ui::clamp_sidebar_width(300, 1200, 9), 300);
    // Tiny window: never wider than the window, never panics.
    assert!(ui::clamp_sidebar_width(300, 150, 9) <= 150);
}

#[test]
fn requested_width_is_used_and_clamped() {
    let g = BTreeMap::new();
    let l = ui::build_sidebar(spec(1200, Some(400), false), &g, None);
    assert_eq!(l.sidebar_rect.w, 400);
    assert_eq!(l.sidebar_rect.x, 800);
    let l = ui::build_sidebar(spec(1200, Some(5000), false), &g, None);
    assert_eq!(l.sidebar_rect.w, ui::sidebar_width_bounds(1200, 9).1);
    // Everything stays inside the sidebar at the minimum width.
    let mut g = BTreeMap::new();
    g.insert(ui::Category::Ssh, vec!["prod".to_string()]);
    let l = ui::build_sidebar(spec(1200, Some(1), false), &g, None);
    let s = l.sidebar_rect;
    for c in &l.categories {
        assert!(c.add_rect.x >= s.x && c.add_rect.x + c.add_rect.w <= s.x + s.w);
        assert!(c.header_rect.x >= s.x);
    }
    assert!(l.collapse_rect.x + l.collapse_rect.w <= s.x + s.w);
}

#[test]
fn terminal_columns_follow_sidebar_width() {
    let fonts = FontStack::embedded(16.0);
    let g = BTreeMap::new();
    let w = 1200;
    let mk = |req| {
        ui::build_sidebar(
            SidebarSpec { width_px: w, height_px: 700, cell_w: fonts.cell_w, cell_h: fonts.cell_h, requested_width: Some(req), collapsed: false },
            &g,
            None,
        )
    };
    let narrow = mk(250);
    let wide = mk(450);
    let (c_narrow, _) = grid_size(&fonts, narrow.sidebar_rect.x, 700);
    let (c_wide, _) = grid_size(&fonts, wide.sidebar_rect.x, 700);
    assert_eq!(c_narrow, ((w - 250 - 2 * PADDING) / fonts.cell_w) as usize);
    assert_eq!(c_wide, ((w - 450 - 2 * PADDING) / fonts.cell_w) as usize);
    assert!(c_wide < c_narrow);
}

#[test]
fn collapsed_sidebar_is_a_thin_clickable_strip() {
    let g = BTreeMap::new();
    let l = ui::build_sidebar(spec(1200, Some(400), true), &g, None);
    assert!(l.collapsed);
    assert_eq!(l.sidebar_rect.w, ui::collapsed_width(9));
    assert_eq!(l.sidebar_rect.x, 1200 - ui::collapsed_width(9));
    assert!(l.categories.is_empty());
    // Clicking the strip expands; there is no drag handle.
    assert_eq!(ui::hit_test(&l, 1195, 300), UiAction::ToggleCollapse);
    assert!(!ui::on_resize_edge(&l, l.sidebar_rect.x, 300));
}

#[test]
fn chevron_collapses_and_edge_is_draggable() {
    let g = BTreeMap::new();
    let l = ui::build_sidebar(spec(1200, Some(400), false), &g, None);
    let c = l.collapse_rect;
    assert_eq!(ui::hit_test(&l, c.x + 1, c.y + 1), UiAction::ToggleCollapse);
    assert!(ui::on_resize_edge(&l, 800, 300));
    assert!(ui::on_resize_edge(&l, 798, 300));
    assert!(!ui::on_resize_edge(&l, 820, 300));
}
