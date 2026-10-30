use mouser_core::storage::{Preferences, ThemeMode, load_preferences, save_preferences, update_preferences};
use serial_test::serial;

#[test]
#[serial]
fn preferences_round_trip_including_sidebar() {
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("MOUSER_HOME", tmp.path());
    }
    let p = Preferences {
        theme: Some("Nord".into()),
        font_family: Some("JetBrains Mono".into()),
        font_size: Some(14.5),
        mode: Some(ThemeMode::Dark),
        sidebar_width: Some(340),
        sidebar_open: Some(true),
        sidebar_collapsed: Some(true),
    };
    save_preferences(&p).unwrap();
    let text = std::fs::read_to_string(tmp.path().join("preferences.toml")).unwrap();
    assert!(text.contains("sidebar_width = 340"), "{text}");
    let back = load_preferences();
    assert_eq!(back.sidebar_width, Some(340));
    assert_eq!(back.sidebar_open, Some(true));
    assert_eq!(back.sidebar_collapsed, Some(true));
    assert_eq!(back.theme.as_deref(), Some("Nord"));
    assert_eq!(back.font_size, Some(14.5));
}

#[test]
#[serial]
fn old_preferences_files_still_load_and_updates_keep_other_fields() {
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("MOUSER_HOME", tmp.path());
    }
    std::fs::write(tmp.path().join("preferences.toml"), "theme = \"Red Star\"\nmode = \"light\"\n").unwrap();
    let p = load_preferences();
    assert_eq!(p.mode, Some(ThemeMode::Light));
    assert_eq!(p.sidebar_width, None);
    update_preferences(|p| p.sidebar_width = Some(280)).unwrap();
    let p = load_preferences();
    assert_eq!(p.sidebar_width, Some(280));
    assert_eq!(p.mode, Some(ThemeMode::Light));
    assert_eq!(p.theme.as_deref(), Some("Red Star"));
}
