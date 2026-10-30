use mouser_core::appearance::*;
use mouser_core::storage::{Preferences, ThemeMode, load_preferences, update_preferences};
use mouser_core::themes::{list_themes, load_theme, parse_hex, resolve_active_colors, resolve_active_theme, save_theme};
use serial_test::serial;

fn temp_home() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("MOUSER_HOME", tmp.path());
        std::env::set_var("MOUSER_SYSTEM_DARK", "1");
    }
    tmp
}

fn type_str(ed: &mut ThemeEditor, s: &str) -> Outcome {
    let mut last = Outcome::Continue;
    for c in s.chars() {
        last = ed.handle(EditKey::Char(c));
    }
    last
}

#[test]
fn parse_key_maps_terminal_bytes() {
    assert_eq!(parse_key(b"\x1b"), Some(EditKey::Cancel));
    assert_eq!(parse_key(b"\t"), Some(EditKey::Next));
    assert_eq!(parse_key(b"\x1b[Z"), Some(EditKey::Prev));
    assert_eq!(parse_key(b"\x1b[A"), Some(EditKey::Up));
    assert_eq!(parse_key(b"\x7f"), Some(EditKey::Backspace));
    assert_eq!(parse_key(b"\x13"), Some(EditKey::Save));
    assert_eq!(parse_key(b"a"), Some(EditKey::Char('a')));
    assert_eq!(parse_key(b"\x01"), None);
}

#[test]
fn hex_parsing() {
    assert_eq!(parse_hex("#04192A"), Some(0x04192A));
    assert_eq!(parse_hex("04192a"), Some(0x04192A));
    assert_eq!(parse_hex("#fff"), Some(0xFFFFFF));
    assert_eq!(parse_hex("#04192"), None);
    assert_eq!(parse_hex("#zz192A"), None);
}

#[test]
#[serial]
fn editor_validates_and_previews() {
    let _h = temp_home();
    let def = load_theme("Red Star", "dark");
    let mut ed = ThemeEditor::new(&def, None, 15.0);
    assert_eq!(ed.fields.len(), 22);
    assert!(ed.validate().is_ok());
    // Clear background: invalid, no preview, error shown.
    ed.focus_field(BG);
    for _ in 0..7 {
        ed.handle(EditKey::Backspace);
    }
    assert_eq!(ed.handle(EditKey::Char('#')), Outcome::Continue);
    assert!(ed.error.as_deref().unwrap().contains("Background"));
    // Non-hex characters are ignored.
    ed.handle(EditKey::Char('g'));
    assert_eq!(ed.fields[BG].value, "#");
    assert_eq!(type_str(&mut ed, "10203"), Outcome::Continue);
    assert_eq!(ed.handle(EditKey::Char('0')), Outcome::Preview);
    assert!(ed.error.is_none());
    // Max length is enforced.
    ed.handle(EditKey::Char('f'));
    assert_eq!(ed.fields[BG].value, "#102030");
    let r = ed.validate().unwrap();
    assert_eq!(r.theme.background, "#102030");
    assert_eq!(r.theme.palette.len(), 16);
    assert_eq!(r.theme.accent.as_deref(), Some("#2FD8E5"));
}

#[test]
#[serial]
fn editor_optional_colors_font_and_navigation() {
    let _h = temp_home();
    let mut ed = ThemeEditor::new(&load_theme("Nord", "dark"), Some("Fira Code"), 14.0);
    // Optional cursor may be empty.
    ed.focus_field(CURSOR);
    assert!(ed.validate().unwrap().theme.cursor.is_none());
    // Tab / Shift+Tab wrap around.
    ed.focus_field(0);
    ed.handle(EditKey::Prev);
    assert_eq!(ed.focus, FONT_SIZE);
    ed.handle(EditKey::Next);
    assert_eq!(ed.focus, 0);
    // Font size bounds.
    ed.focus_field(FONT_SIZE);
    ed.handle(EditKey::Backspace);
    ed.handle(EditKey::Backspace);
    assert!(ed.validate().is_err());
    assert_eq!(type_str(&mut ed, "99"), Outcome::Continue);
    assert!(ed.error.as_deref().unwrap().contains("Font size"));
    ed.handle(EditKey::Backspace);
    ed.handle(EditKey::Backspace);
    assert_eq!(type_str(&mut ed, "18.5"), Outcome::Preview);
    let r = ed.validate().unwrap();
    assert_eq!(r.font_size, 18.5);
    assert_eq!(r.font_family.as_deref(), Some("Fira Code"));
    // Save refuses while invalid, accepts when valid.
    ed.focus_field(FG);
    ed.handle(EditKey::Backspace);
    assert_eq!(ed.handle(EditKey::Save), Outcome::Continue);
    ed.handle(EditKey::Char('0'));
    assert_eq!(ed.handle(EditKey::Save), Outcome::Save);
    assert_eq!(ed.handle(EditKey::Cancel), Outcome::Close);
}

#[test]
fn picker_navigation_and_modes() {
    let themes = vec!["Red Star".to_string(), "Nord".to_string(), "Mine".to_string()];
    let mut p = ThemePicker::new(themes, "Nord", ThemeMode::System);
    assert_eq!(p.theme(), "Nord");
    assert_eq!(p.handle(EditKey::Down), Outcome::Preview);
    assert_eq!(p.theme(), "Mine");
    p.handle(EditKey::Down);
    assert_eq!(p.theme(), "Red Star");
    p.handle(EditKey::Up);
    assert_eq!(p.theme(), "Mine");
    assert_eq!(p.handle(EditKey::Right), Outcome::Preview);
    assert_eq!(p.mode, ThemeMode::Dark);
    p.handle(EditKey::Char('l'));
    assert_eq!(p.mode, ThemeMode::Light);
    assert_eq!(p.handle(EditKey::Char('l')), Outcome::Continue);
    assert_eq!(p.handle(EditKey::Char('e')), Outcome::OpenEditor);
    assert_eq!(p.handle(EditKey::Enter), Outcome::Save);
    assert_eq!(p.handle(EditKey::Cancel), Outcome::Close);
    assert_eq!(parse_mode("System"), Some(ThemeMode::System));
    assert_eq!(parse_mode("purple"), None);
}

#[test]
#[serial]
fn theme_switching_persists_and_applies() {
    let _h = temp_home();
    update_preferences(|p| {
        p.theme = Some("Nord".into());
        p.mode = Some(ThemeMode::Light);
    })
    .unwrap();
    let prefs = load_preferences();
    assert_eq!(resolve_active_theme(&prefs).variant, "light");
    let (bg, _) = resolve_active_colors(&prefs);
    assert_eq!(bg, 0xeceff4);
    update_preferences(|p| p.mode = Some(ThemeMode::Dark)).unwrap();
    let (bg, _) = resolve_active_colors(&load_preferences());
    assert_eq!(bg, 0x2e3440);
}

#[test]
#[serial]
fn edited_theme_round_trips_and_user_themes_are_listed() {
    let h = temp_home();
    let mut ed = ThemeEditor::new(&load_theme("Red Star", "dark"), None, 15.0);
    ed.theme = "My Star".into();
    ed.focus_field(ACCENT);
    for _ in 0..7 {
        ed.handle(EditKey::Backspace);
    }
    type_str(&mut ed, "#FF0066");
    let r = ed.validate().unwrap();
    save_theme(&r.theme).unwrap();
    assert!(h.path().join("themes/My Star/dark.toml").exists());
    let back = load_theme("My Star", "dark");
    assert_eq!(back, r.theme);
    assert_eq!(back.colors().accent, Some(0xFF0066));
    let names = list_themes();
    assert_eq!(names[0], "Red Star");
    assert!(names.contains(&"My Star".to_string()));
    let prefs = Preferences { theme: Some("My Star".into()), mode: Some(ThemeMode::Dark), ..Default::default() };
    assert_eq!(resolve_active_theme(&prefs).accent.as_deref(), Some("#FF0066"));
}

mod font_dropdown {
    use mouser_core::appearance::{EditKey, FONT_FAMILY, FONT_ROWS, Outcome, ThemeEditor};
    use std::sync::Arc;

    fn def() -> mouser_core::themes::ThemeDef {
        mouser_core::themes::builtins().remove(0).1
    }

    fn editor() -> ThemeEditor {
        let mut e = ThemeEditor::new(&def(), None, 14.0);
        let fams = mouser_core::fuzzy::dedupe_names(
            ["Noto Sans Mono", "Fira Code", "DejaVu Sans", "fira sans", "Hack", "JetBrains Mono", "Fira Code"].map(String::from),
        );
        e.set_families(Arc::new(fams));
        e
    }
    fn shown(e: &ThemeEditor) -> Vec<String> {
        e.fonts.visible().map(|(_, n, _)| n.to_string()).collect()
    }

    #[test]
    fn focusing_opens_alphabetical_list_and_typing_filters_in_order() {
        let mut e = editor();
        assert!(!e.fonts.open);
        e.focus_field(FONT_FAMILY);
        assert!(e.fonts.open);
        assert_eq!(shown(&e), ["DejaVu Sans", "Fira Code", "fira sans", "Hack", "JetBrains Mono", "Noto Sans Mono"]);
        e.handle(EditKey::Down);
        assert_eq!(e.fonts.selected, 1);
        for c in "fis".chars() {
            e.handle(EditKey::Char(c));
        }
        // Filter only; still alphabetical, selection back on the first row.
        assert_eq!(shown(&e), ["fira sans"]);
        assert_eq!(e.fonts.selected, 0);
        e.handle(EditKey::Backspace);
        assert_eq!(shown(&e), ["Fira Code", "fira sans"]);
        e.handle(EditKey::Down);
        assert_eq!(e.handle(EditKey::Enter), Outcome::FontPicked);
        assert_eq!(e.fields[FONT_FAMILY].value, "fira sans");
        assert!(!e.fonts.open);
    }

    #[test]
    fn esc_closes_list_only_and_typing_never_loads() {
        let mut e = editor();
        e.focus_field(FONT_FAMILY);
        assert_eq!(e.handle(EditKey::Cancel), Outcome::Continue);
        assert!(!e.fonts.open);
        // Typing reopens and only previews (no FontPicked).
        assert_ne!(e.handle(EditKey::Char('h')), Outcome::FontPicked);
        assert!(e.fonts.open);
        e.focus_field(0);
        assert!(!e.fonts.open);
    }

    #[test]
    fn scrolling_keeps_selection_visible() {
        let mut e = ThemeEditor::new(&def(), None, 14.0);
        e.set_families(Arc::new((0..30).map(|i| format!("Font {i:02}")).collect()));
        e.focus_field(FONT_FAMILY);
        e.scroll_fonts(12);
        assert_eq!(e.fonts.selected, 12);
        assert_eq!(e.fonts.scroll, 12 + 1 - FONT_ROWS);
        assert_eq!(e.pick_font(12), Outcome::FontPicked);
        assert_eq!(e.fields[FONT_FAMILY].value, "Font 12");
    }
}
