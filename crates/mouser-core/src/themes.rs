use crate::storage::{Preferences, ThemeMode, ensure_dirs, resolve_home};
use serde::{Deserialize, Serialize};
use std::fs;
// no path usage

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeDef {
    pub name: String,
    pub variant: String,    // "dark" or "light"
    pub foreground: String, // hex "#rrggbb"
    pub background: String, // hex
}

pub fn write_builtins_if_absent() -> std::io::Result<()> {
    ensure_dirs()?;
    for (name, dark, light) in builtins() {
        let dir = resolve_home().join("themes").join(&name);
        fs::create_dir_all(&dir)?;
        let dark_path = dir.join("dark.toml");
        if !dark_path.exists() {
            fs::write(&dark_path, toml::to_string_pretty(&dark).unwrap())?;
        }
        let light_path = dir.join("light.toml");
        if !light_path.exists() {
            fs::write(&light_path, toml::to_string_pretty(&light).unwrap())?;
        }
    }
    Ok(())
}

pub fn resolve_active_colors(prefs: &Preferences) -> (u32, u32) {
    let theme_name = prefs.theme.as_deref().unwrap_or("Tokyo Night");
    let use_dark = match prefs.mode.clone().unwrap_or_default() {
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
        ThemeMode::System => std::env::var("MOUSER_SYSTEM_DARK").ok().as_deref() == Some("1"),
    };
    // Try disk first
    let variant = if use_dark { "dark" } else { "light" };
    let path = resolve_home().join("themes").join(theme_name).join(format!("{variant}.toml"));
    if let Ok(s) = fs::read_to_string(&path)
        && let Ok(def) = toml::from_str::<ThemeDef>(&s)
    {
        return (parse_hex_rgb(&def.background), parse_hex_rgb(&def.foreground));
    }
    // Fallback to in-binary builtins
    for (name, dark, light) in builtins() {
        if name == theme_name {
            let def = if use_dark { dark } else { light };
            return (parse_hex_rgb(&def.background), parse_hex_rgb(&def.foreground));
        }
    }
    // Default pleasant dark
    (parse_hex_rgb("#1a1b26"), parse_hex_rgb("#c0caf5"))
}

fn parse_hex_rgb(s: &str) -> u32 {
    let t = s.trim_start_matches('#');
    if t.len() == 6
        && let Ok(v) = u32::from_str_radix(t, 16)
    {
        return v;
    }
    0xFFFFFF
}

pub fn builtins() -> Vec<(String, ThemeDef, ThemeDef)> {
    vec![
        theme_pair("Tokyo Night", "#c0caf5", "#1a1b26", "#3760bf", "#e1e2e7"),
        theme_pair("Catppuccin", "#cdd6f4", "#1e1e2e", "#4c4f69", "#eff1f5"),
        theme_pair("Gruvbox", "#ebdbb2", "#282828", "#3c3836", "#fbf1c7"),
        theme_pair("Dracula", "#f8f8f2", "#282a36", "#44475a", "#f8f8f2"),
        theme_pair("Nord", "#e5e9f0", "#2e3440", "#2e3440", "#eceff4"),
        theme_pair("One Dark", "#abb2bf", "#282c34", "#383a42", "#fafafa"),
        theme_pair("One Light", "#383a42", "#fafafa", "#282c34", "#abb2bf"),
        theme_pair("Solarized", "#93a1a1", "#002b36", "#586e75", "#fdf6e3"),
        theme_pair("Rose Pine", "#e0def4", "#191724", "#575279", "#faf4ed"),
        theme_pair("Kanagawa", "#c8c093", "#1f1f28", "#223249", "#dcd7ba"),
        theme_pair("GitHub", "#24292e", "#ffffff", "#57606a", "#f6f8fa"),
    ]
}

fn theme_pair(name: &str, dark_fg: &str, dark_bg: &str, light_fg: &str, light_bg: &str) -> (String, ThemeDef, ThemeDef) {
    (
        name.to_string(),
        ThemeDef { name: name.to_string(), variant: "dark".to_string(), foreground: dark_fg.to_string(), background: dark_bg.to_string() },
        ThemeDef { name: name.to_string(), variant: "light".to_string(), foreground: light_fg.to_string(), background: light_bg.to_string() },
    )
}
