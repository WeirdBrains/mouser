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
    assert_eq!(bg, 0x04192a);
    assert_eq!(fg, 0xe6fbfa);
}

#[test]
#[serial]
fn system_mode_defaults_to_red_star_dark() {
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        env::set_var("MOUSER_HOME", tmp.path());
        env::set_var("MOUSER_SYSTEM_DARK", "1");
    }
    // No preferences at all: Red Star, system mode.
    let (bg, fg) = resolve_active_colors(&Preferences::default());
    assert_eq!(bg, 0x04192a);
    assert_eq!(fg, 0xe6fbfa);
}

#[test]
#[serial]
fn red_star_light_is_readable() {
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        env::set_var("MOUSER_HOME", tmp.path());
    }
    let prefs = Preferences { theme: Some("Red Star".into()), font_family: None, font_size: None, mode: Some(ThemeMode::Light) };
    let (bg, fg) = resolve_active_colors(&prefs);
    let lum = |c: u32| ((c >> 16) & 0xff) as f32 * 0.299 + ((c >> 8) & 0xff) as f32 * 0.587 + (c & 0xff) as f32 * 0.114;
    assert!(lum(bg) > 200.0, "light background");
    assert!(lum(bg) - lum(fg) > 150.0, "strong contrast");
}

#[test]
#[serial]
fn legacy_builtin_theme_files_are_upgraded_but_user_edits_kept() {
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        env::set_var("MOUSER_HOME", tmp.path());
    }
    let dir = tmp.path().join("themes").join("Red Star");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("dark.toml"), "name = \"Red Star\"\nvariant = \"dark\"\nforeground = \"#E6FBFA\"\nbackground = \"#020F1B\"\n").unwrap();
    std::fs::write(dir.join("light.toml"), "name = \"Red Star\"\nvariant = \"light\"\nforeground = \"#111111\"\nbackground = \"#eeeeee\"\n").unwrap();
    write_builtins_if_absent().unwrap();
    assert!(std::fs::read_to_string(dir.join("dark.toml")).unwrap().contains("#04192A"));
    assert!(std::fs::read_to_string(dir.join("light.toml")).unwrap().contains("#111111"));
}
