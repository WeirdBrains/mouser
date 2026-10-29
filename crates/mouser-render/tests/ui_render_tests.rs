use mouser_render::ui::*;
use std::collections::BTreeMap;

#[test]
fn sidebar_layout_contains_categories_and_buttons() {
    let mut groups = BTreeMap::new();
    groups.insert(Category::Ssh, vec!["prod".into(), "staging".into()]);
    groups.insert(Category::Waypipe, vec![]);
    groups.insert(Category::Tunnels, vec!["socks".into()]);
    groups.insert(Category::X11, vec!["workstation".into()]);
    let layout = build_sidebar_layout(1200, 800, 16.0, &groups, None);
    assert!(layout.sidebar_rect.w >= 220);
    assert_eq!(layout.categories.len(), 4);
    // SSH add button clickable
    let ssh_add = &layout.categories[0].add_rect;
    assert!(ssh_add.w > 0 && ssh_add.h > 0);
    // One item in Tunnels must have edit/delete rects
    let tunnels = &layout.categories[2];
    assert_eq!(tunnels.items.len(), 1);
    assert!(tunnels.items[0].edit_rect.w > 0);
    assert!(tunnels.items[0].delete_rect.w > 0);
}

#[test]
fn hit_test_add_edit_delete() {
    let mut groups = BTreeMap::new();
    groups.insert(Category::Ssh, vec!["server".into()]);
    groups.insert(Category::Waypipe, vec![]);
    groups.insert(Category::Tunnels, vec![]);
    groups.insert(Category::X11, vec![]);
    let layout = build_sidebar_layout(1000, 700, 16.0, &groups, None);
    let ssh = &layout.categories[0];
    // Add
    let act = hit_test(&layout, ssh.add_rect.x + 1, ssh.add_rect.y + 1);
    assert!(matches!(act, UiAction::OpenAdd(Category::Ssh)));
    // Edit and delete on first item
    let it = &ssh.items[0];
    let act = hit_test(&layout, it.edit_rect.x + 1, it.edit_rect.y + 1);
    assert!(matches!(act, UiAction::OpenEdit { .. }));
    let act = hit_test(&layout, it.delete_rect.x + 1, it.delete_rect.y + 1);
    assert!(matches!(act, UiAction::OpenDeleteConfirm { .. }));
}

#[test]
fn modal_consumes_clicks_and_toggles_advanced() {
    let groups = BTreeMap::new();
    let modal = ModalState { kind: ModalKind::Add(Category::Ssh), advanced_open: false, rect: Rect { x: 100, y: 100, w: 400, h: 300 } };
    let layout = build_sidebar_layout(1200, 800, 16.0, &groups, Some(modal));
    // Inside modal -> toggle advanced
    let act = hit_test(&layout, 120, 120);
    assert_eq!(act, UiAction::ToggleAdvanced);
    // Outside -> cancel
    let act = hit_test(&layout, 10, 10);
    assert_eq!(act, UiAction::CancelModal);
}
