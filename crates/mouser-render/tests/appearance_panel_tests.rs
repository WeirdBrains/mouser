use mouser_render::ui::{self, EditorFieldView, PanelStyle, UiAction};

fn inside(outer: ui::Rect, r: ui::Rect) -> bool {
    r.x >= outer.x && r.y >= outer.y && r.x + r.w <= outer.x + outer.w && r.y + r.h <= outer.y + outer.h
}

#[test]
fn picker_lists_themes_modes_and_actions() {
    let themes: Vec<String> = (0..20).map(|i| format!("Theme {i}")).collect();
    let p = ui::picker_panel(1200, 800, 9, 18, &themes, 17, "light");
    for it in &p.items {
        assert!(inside(p.rect, it.rect), "{it:?} outside {:?}", p.rect);
    }
    let rows: Vec<_> = p.items.iter().filter(|i| matches!(i.action, UiAction::PickTheme(_))).collect();
    assert!(rows.len() <= 14 && rows.len() >= 3);
    // The selected theme is visible and marked.
    assert!(rows.iter().any(|r| r.action == UiAction::PickTheme(17) && r.style == PanelStyle::Row { selected: true }));
    let light = p.items.iter().find(|i| i.action == UiAction::SetMode("light".into())).unwrap();
    assert_eq!(light.style, PanelStyle::Row { selected: true });
    let layout = ui::UiLayout { panel: Some(p.clone()), ..ui::without_sidebar(1200, 800) };
    assert_eq!(ui::hit_test(&layout, light.rect.x + 1, light.rect.y + 1), UiAction::SetMode("light".into()));
    assert_eq!(ui::hit_test(&layout, 1, 1), UiAction::ClosePanel);
    assert!(p.items.iter().any(|i| i.action == UiAction::EditTheme));
}

#[test]
fn editor_fields_fit_and_are_clickable() {
    let fields: Vec<EditorFieldView> = (0..22)
        .map(|i| EditorFieldView { label: format!("f{i}"), value: "#04192A".into(), valid: i != 5, swatch: (i < 20).then_some(0x04192A) })
        .collect();
    let p = ui::editor_panel(1200, 800, 9, 18, "Edit Red Star (dark)", &fields, 21, Some("Palette 1: use #rrggbb"));
    for it in &p.items {
        assert!(inside(p.rect, it.rect), "{it:?} outside {:?}", p.rect);
    }
    for i in 0..22 {
        assert!(p.items.iter().any(|it| it.action == UiAction::FocusField(i) && matches!(it.style, PanelStyle::Field { .. })), "field {i}");
    }
    assert!(p.items.iter().any(|it| it.action == UiAction::FocusField(21) && it.style == PanelStyle::Field { focused: true, valid: true }));
    assert!(p.items.iter().any(|it| it.style == PanelStyle::Field { focused: false, valid: false }));
    assert_eq!(p.items.iter().filter(|it| matches!(it.style, PanelStyle::Swatch(_))).count(), 20);
    assert!(p.items.iter().any(|it| it.style == PanelStyle::Error));
    // Field rects never overlap.
    let f: Vec<_> = p.items.iter().filter(|i| matches!(i.style, PanelStyle::Field { .. })).map(|i| i.rect).collect();
    for (a, ra) in f.iter().enumerate() {
        for rb in &f[a + 1..] {
            let overlap = ra.x < rb.x + rb.w && rb.x < ra.x + ra.w && ra.y < rb.y + rb.h && rb.y < ra.y + ra.h;
            assert!(!overlap, "{ra:?} overlaps {rb:?}");
        }
    }
}

#[test]
fn sidebar_has_appearance_button() {
    let l = ui::build_sidebar_layout_metrics(1200, 800, 9, 18, &Default::default(), None);
    let a = l.appearance_rect;
    assert!(inside(l.sidebar_rect, a));
    assert_eq!(ui::hit_test(&l, a.x + 1, a.y + 1), UiAction::OpenAppearance);
}

#[test]
fn ctrl_zoom_keys() {
    assert_eq!(mouser_render::zoom_for_char("="), Some(1));
    assert_eq!(mouser_render::zoom_for_char("+"), Some(1));
    assert_eq!(mouser_render::zoom_for_char("-"), Some(-1));
    assert_eq!(mouser_render::zoom_for_char("0"), Some(0));
    assert_eq!(mouser_render::zoom_for_char("a"), None);
}
