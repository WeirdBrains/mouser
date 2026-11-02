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
        engine: Some("skia".into()),
        engines: None,
        mcp: Some(mouser_core::storage::McpPrefs {
            server: Some(mouser_core::storage::McpServerPrefs { enabled: true, https: true, ..Default::default() }),
            servers: vec![],
        }),
        models: None,
        chat: None,
        keys: None,
    };
    save_preferences(&p).unwrap();
    let text = std::fs::read_to_string(tmp.path().join("preferences.toml")).unwrap();
    assert!(text.contains("sidebar_width = 340"), "{text}");
    assert!(text.contains("[mcp.server]") && text.contains("https_port = 7802"), "{text}");
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

#[test]
fn leader_timeout_defaults_reads_keys_and_clamps() {
    use mouser_core::storage::{DEFAULT_LEADER_TIMEOUT_MS, leader_timeout};
    use std::time::Duration;
    let d = Preferences::default();
    assert_eq!(leader_timeout(&d), Duration::from_millis(DEFAULT_LEADER_TIMEOUT_MS));
    assert!(DEFAULT_LEADER_TIMEOUT_MS >= 1000, "long enough for a slow esc then :");
    let p: Preferences = toml::from_str("[keys]\nleader_timeout_ms = 2000\n").unwrap();
    assert_eq!(leader_timeout(&p), Duration::from_millis(2000));
    let p: Preferences = toml::from_str("[keys]\nleader_timeout_ms = 5\n").unwrap();
    assert_eq!(leader_timeout(&p), Duration::from_millis(100), "clamped");
    // Saved back without a [keys] table when unset.
    assert!(!toml::to_string(&d).unwrap().contains("[keys]"));
}
