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
    let prefs =
        Preferences { theme: Some("Red Star".into()), font_family: None, font_size: None, mode: Some(ThemeMode::System), ..Default::default() };
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
    // No preferences at all: Red Star, system mode (force off Omarchy auto-default).
    unsafe { env::set_var("MOUSER_OMARCHY", "0"); }
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
    let prefs =
        Preferences { theme: Some("Red Star".into()), font_family: None, font_size: None, mode: Some(ThemeMode::Light), ..Default::default() };
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

#[test]
#[serial]
fn omarchy_builtin_and_auto_default() {
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("MOUSER_HOME", tmp.path());
        std::env::set_var("MOUSER_SYSTEM_DARK", "1");
        std::env::set_var("MOUSER_OMARCHY", "0");
    }
    assert_eq!(mouser_core::themes::default_theme_name(), "Red Star");
    assert!(mouser_core::themes::list_themes().contains(&"Omarchy".into()));
    let def = mouser_core::themes::load_theme("Omarchy", "dark");
    assert_eq!(def.background.to_lowercase(), "#070d16");
    assert_eq!(def.accent.as_deref(), Some("#5e84a9"));
    assert_eq!(def.palette.len(), 16);
    unsafe { std::env::set_var("MOUSER_OMARCHY", "1"); }
    assert_eq!(mouser_core::themes::default_theme_name(), "Omarchy");
    let (bg, fg) = mouser_core::themes::resolve_active_colors(&mouser_core::storage::Preferences::default());
    assert_eq!(bg, 0x070d16);
    assert_eq!(fg, 0xfff1c8);
    unsafe { std::env::set_var("MOUSER_OMARCHY", "0"); }
}

#[test]
fn match_omarchy_reads_colors_toml() {
    let sample = concat!(
        "mode = \"dark\"\n",
        "accent = \"#5e84a9\"\n",
        "selection = \"#20252d\"\n",
        "selection_foreground = \"#070D16\"\n",
        "background = \"#070D16\"\n",
        "foreground = \"#fff1c8\"\n",
        "red = \"#8e92bd\"\n",
        "yellow = \"#a5ffff\"\n",
        "green = \"#86d0e3\"\n",
        "cyan = \"#93e7ff\"\n",
        "blue = \"#5e84a9\"\n",
        "magenta = \"#91ace2\"\n",
        "bright_red = \"#a1a7dc\"\n",
        "bright_yellow = \"#8dffff\"\n",
        "bright_green = \"#87eaff\"\n",
        "bright_cyan = \"#96ffff\"\n",
        "bright_blue = \"#6b9ac7\"\n",
        "bright_magenta = \"#9fc2ff\"\n",
        "dark_background = \"#050a11\"\n",
        "lighter_background = \"#20252d\"\n",
        "bright_foreground = \"#fff5d6\"\n",
    );
    let def = mouser_core::themes::parse_omarchy_colors_for_test(sample, "Omarchy").expect("parse");
    assert_eq!(def.background, "#070D16");
    assert_eq!(def.foreground, "#fff1c8");
    assert_eq!(def.accent.as_deref(), Some("#5e84a9"));
    assert_eq!(def.palette.len(), 16);
    assert_eq!(def.palette[1], "#8e92bd");
    assert_eq!(def.palette[2], "#86d0e3");
}


#[test]
#[serial]
fn duplicate_export_import_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        env::set_var("MOUSER_HOME", tmp.path());
        env::set_var("MOUSER_OMARCHY", "0");
    }
    write_builtins_if_absent().unwrap();

    mouser_core::themes::create_theme("Mine").unwrap();
    assert!(mouser_core::themes::list_themes().iter().any(|t| t == "Mine"));
    assert!(mouser_core::themes::create_theme("Mine").is_err());

    mouser_core::themes::duplicate_theme("Mine", "Mine Two").unwrap();
    let export = tmp.path().join("export-mine");
    mouser_core::themes::export_theme("Mine Two", &export).unwrap();
    assert!(export.join("dark.toml").is_file());
    assert!(export.join("light.toml").is_file());

    let imported = mouser_core::themes::import_theme(&export, Some("From Export")).unwrap();
    assert_eq!(imported, "From Export");
    let dark = mouser_core::themes::load_theme("From Export", "dark");
    assert_eq!(dark.name, "From Export");
}
