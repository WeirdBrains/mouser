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
    let p = ui::editor_panel(1200, 800, 9, 18, "Edit Red Star (dark)", &fields, 21, Some("Palette 1: use #rrggbb"), None);
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

#[test]
fn read_only_hosts_and_notice() {
    let mut g = std::collections::BTreeMap::new();
    g.insert(ui::Category::Ssh, vec!["prod".to_string(), "web1".to_string()]);
    g.insert(ui::Category::Waypipe, vec!["wp".to_string()]);
    let mut l = ui::build_sidebar_layout_metrics(1200, 800, 9, 18, &g, None);
    let wp_before = l.categories[1].header_rect.y;
    ui::mark_read_only(&mut l, &ui::Category::Ssh, &["web1".to_string()]);
    ui::add_notice(&mut l, &ui::Category::Ssh, "Move 2 saved to ~/.ssh/config", 18);
    let ssh = &l.categories[0];
    let (prod, web1) = (&ssh.items[0], &ssh.items[1]);
    assert!(!prod.read_only && web1.read_only);
    assert_eq!(web1.delete_rect.w, 0);
    assert_eq!(ui::hit_test(&l, web1.edit_rect.x + 1, web1.edit_rect.y + 1), UiAction::OpenEdit { category: ui::Category::Ssh, name: "web1".into() });
    assert_eq!(ui::hit_test(&l, prod.name_rect.x + 1, prod.name_rect.y + 1), UiAction::Connect { category: ui::Category::Ssh, name: "prod".into() });
    let (nr, _) = l.notice.clone().unwrap();
    assert_eq!(ui::hit_test(&l, nr.x + 1, nr.y + 1), UiAction::Notice);
    assert!(l.categories[1].header_rect.y > wp_before);
    assert!(prod.name_rect.y >= nr.y + nr.h, "items move below the notice");
}

#[test]
fn form_and_confirm_panels() {
    let fields: Vec<EditorFieldView> = ["Alias", "HostName", "User", "Port", "IdentityFile", "ProxyJump"]
        .iter()
        .map(|l| EditorFieldView { label: l.to_string(), value: String::new(), valid: true, swatch: None })
        .collect();
    let p = ui::form_panel(1200, 800, 9, 18, "Add SSH host", &fields, 0, Some("note"), Some("err"), true);
    for it in &p.items {
        assert!(inside(p.rect, it.rect));
    }
    assert!(p.items.iter().any(|i| i.action == UiAction::SavePanel));
    let ro = ui::form_panel(1200, 800, 9, 18, "SSH web1 (read-only)", &fields, 0, Some("why"), None, false);
    assert!(!ro.items.iter().any(|i| i.action == UiAction::SavePanel));
    let c = ui::confirm_panel(1200, 800, 9, 18, "Delete Host prod?", "Delete");
    assert!(c.items.iter().any(|i| i.action == UiAction::SavePanel && i.text == "Delete"));
}

#[test]
fn approval_popup_buttons_arm_and_remote_has_no_always() {
    let lines = vec!["  • show notices  (notify)".to_string()];
    let buttons = |p: &ui::Panel| p.items.iter().filter(|i| matches!(i.style, PanelStyle::Button { .. })).cloned().collect::<Vec<_>>();
    let p = ui::approval_panel(1200, 800, 9, 18, "Mouser approval — Plugin \"x\" asks to:", &lines, &[0, 1, 2, 3], true);
    let b = buttons(&p);
    assert_eq!(b.iter().map(|i| i.action.clone()).collect::<Vec<_>>(), (0..4).map(UiAction::Approve).collect::<Vec<_>>());
    let layout = ui::UiLayout { panel: Some(p.clone()), ..ui::without_sidebar(1200, 800) };
    let r = b[1].rect;
    assert_eq!(ui::hit_test(&layout, r.x + 2, r.y + 2), UiAction::Approve(1));
    // Not armed yet: the buttons do nothing.
    let p0 = ui::approval_panel(1200, 800, 9, 18, "t", &lines, &[0, 1, 2, 3], false);
    assert!(buttons(&p0).iter().all(|i| i.action == UiAction::None));
    // Remote: Allow once, Allow always (when the host is known) and Deny.
    let pr = ui::approval_panel(1200, 800, 9, 18, "t", &lines, &[0, 1, 2], true);
    assert_eq!(
        buttons(&pr).iter().map(|i| i.action.clone()).collect::<Vec<_>>(),
        vec![UiAction::Approve(0), UiAction::Approve(1), UiAction::Approve(2)]
    );
    let pu = ui::approval_panel(1200, 800, 9, 18, "t", &lines, &[0, 2], true);
    assert_eq!(buttons(&pu).len(), 2);
    // Fits in a small window.
    let ps = ui::approval_panel(400, 300, 9, 18, "t", &lines, &[0, 1, 2, 3], true);
    assert!(ps.rect.x >= 0 && ps.rect.x + ps.rect.w <= 400);
}
