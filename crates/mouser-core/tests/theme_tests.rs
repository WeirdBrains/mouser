use mouser_core::storage::{Preferences, ThemeMode};
use mouser_core::themes::{resolve_active_colors, write_builtins_if_absent};
use serial_test::serial;
use std::env;

#[test]
#[serial]
fn writes_builtins_under_home() {
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        env::set_var("MOUSER_HOME", tmp.path());
    }
    write_builtins_if_absent().unwrap();
    let rs = tmp.path().join("themes").join("Red Star");
    assert!(rs.join("dark.toml").exists());
    assert!(rs.join("light.toml").exists());
}

#[test]
#[serial]
fn resolves_system_dark_env() {
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        env::set_var("MOUSER_HOME", tmp.path());
    }
    write_builtins_if_absent().unwrap();
    unsafe {
        env::set_var("MOUSER_SYSTEM_DARK", "1");
    }
    let prefs = Preferences { theme: Some("Red Star".into()), font_family: None, font_size: None, mode: Some(ThemeMode::System) };
    let (bg, fg) = resolve_active_colors(&prefs);
    assert_eq!(bg, 0x020f1b);
    assert_eq!(fg, 0xe6fbfa);
}
