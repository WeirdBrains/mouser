use crate::storage::{Preferences, ThemeMode, ensure_dirs, resolve_home};
use serde::{Deserialize, Serialize};
use std::fs;
// no path usage

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeDef {
    pub name: String,
    pub variant: String,    // "dark" or "light"
    pub foreground: String, // hex "#rrggbb"
    pub background: String, // hex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    /// 16 ANSI colours (0-7 normal, 8-15 bright); empty = defaults.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub palette: Vec<String>,
}

/// Default 16-colour ANSI palette used when a theme has none.
pub const DEFAULT_PALETTE: [&str; 16] = [
    "#1E2A38", "#F7768E", "#9ECE6A", "#E0AF68", "#7AA2F7", "#BB9AF7", "#2FD8E5", "#C0CAF5", "#41566B", "#FF8FA3", "#B9F27C", "#FFC777", "#94B4FF",
    "#D5B8FF", "#94F3F2", "#E6FBFA",
];

/// Resolved colours of the active theme variant (0xRRGGBB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeColors {
    pub background: u32,
    pub foreground: u32,
    pub cursor: Option<u32>,
    pub accent: Option<u32>,
    /// ANSI 0-15.
    pub palette: [u32; 16],
}

impl ThemeDef {
    pub fn colors(&self) -> ThemeColors {
        ThemeColors {
            background: parse_hex_rgb(&self.background),
            foreground: parse_hex_rgb(&self.foreground),
            cursor: self.cursor.as_deref().and_then(parse_hex),
            accent: self.accent.as_deref().and_then(parse_hex),
            palette: {
                let full = self.full_palette();
                std::array::from_fn(|i| parse_hex_rgb(&full[i]))
            },
        }
    }

    /// The palette, padded with defaults to 16 entries.
    pub fn full_palette(&self) -> Vec<String> {
        (0..16).map(|i| self.palette.get(i).cloned().unwrap_or_else(|| DEFAULT_PALETTE[i].to_string())).collect()
    }
}

/// `#rrggbb` / `rrggbb` / `#rgb` to 0xRRGGBB.
pub fn parse_hex(s: &str) -> Option<u32> {
    let t = s.trim().trim_start_matches('#');
    if !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match t.len() {
        6 => u32::from_str_radix(t, 16).ok(),
        3 => {
            let v = u32::from_str_radix(t, 16).ok()?;
            let (r, g, b) = ((v >> 8) & 0xF, (v >> 4) & 0xF, v & 0xF);
            Some((r * 17) << 16 | (g * 17) << 8 | (b * 17))
        }
        _ => None,
    }
}

/// "dark" or "light" for the preferences' mode (system resolved now).
pub fn active_variant(prefs: &Preferences) -> &'static str {
    let dark = match prefs.mode.clone().unwrap_or_default() {
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
        ThemeMode::System => system_prefers_dark(),
    };
    if dark { "dark" } else { "light" }
}

/// Built-in theme names first, then user themes found under `themes/`.
pub fn list_themes() -> Vec<String> {
    let mut names: Vec<String> = builtins().into_iter().map(|(n, _, _)| n).collect();
    if let Ok(rd) = fs::read_dir(resolve_home().join("themes")) {
        let mut user: Vec<String> =
            rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).filter(|n| !names.contains(n)).collect();
        user.sort_by_key(|n| n.to_lowercase());
        names.extend(user);
    }
    names
}

/// A theme variant from disk, else the built-in, else Red Star renamed.
pub fn load_theme(name: &str, variant: &str) -> ThemeDef {
    let path = resolve_home().join("themes").join(name).join(format!("{variant}.toml"));
    if let Ok(s) = fs::read_to_string(&path)
        && let Ok(def) = toml::from_str::<ThemeDef>(&s)
    {
        return def;
    }
    let all = builtins();
    let pick = |(_, d, l): (String, ThemeDef, ThemeDef)| if variant == "light" { l } else { d };
    if let Some(b) = all.iter().find(|(n, _, _)| n == name).cloned() {
        return pick(b);
    }
    let mut def = pick(all[0].clone());
    def.name = name.to_string();
    def
}

/// Write `themes/<name>/<variant>.toml`.
pub fn save_theme(def: &ThemeDef) -> std::io::Result<()> {
    let dir = resolve_home().join("themes").join(&def.name);
    fs::create_dir_all(&dir)?;
    let variant = if def.variant == "light" { "light" } else { "dark" };
    fs::write(dir.join(format!("{variant}.toml")), toml::to_string_pretty(def).unwrap())
}

/// The active theme variant for `prefs`.
pub fn resolve_active_theme(prefs: &Preferences) -> ThemeDef {
    load_theme(prefs.theme.as_deref().unwrap_or("Red Star"), active_variant(prefs))
}

pub fn write_builtins_if_absent() -> std::io::Result<()> {
    ensure_dirs()?;
    for (name, dark, light) in builtins() {
        let dir = resolve_home().join("themes").join(&name);
        fs::create_dir_all(&dir)?;
        let dark_path = dir.join("dark.toml");
        write_builtin(&dark_path, &dark)?;
        write_builtin(&dir.join("light.toml"), &light)?;
    }
    Ok(())
}

/// Write a built-in theme file if it is missing, or if it still holds an older
/// built-in default (so palette fixes reach existing installs without
/// overwriting user edits).
fn write_builtin(path: &std::path::Path, def: &ThemeDef) -> std::io::Result<()> {
    let fresh = toml::to_string_pretty(def).unwrap();
    match fs::read_to_string(path) {
        Err(_) => fs::write(path, fresh),
        Ok(existing) => {
            let is_legacy = toml::from_str::<ThemeDef>(&existing).is_ok_and(|old| {
                LEGACY_BUILTINS.iter().any(|(n, v, fg, bg)| {
                    old.name == *n && old.variant == *v && old.foreground.eq_ignore_ascii_case(fg) && old.background.eq_ignore_ascii_case(bg)
                })
            });
            if is_legacy { fs::write(path, fresh) } else { Ok(()) }
        }
    }
}

/// Previously shipped built-in palettes: (name, variant, fg, bg).
const LEGACY_BUILTINS: &[(&str, &str, &str, &str)] = &[("Red Star", "dark", "#E6FBFA", "#020F1B"), ("Red Star", "light", "#04192A", "#E6FBFA")];

/// Whether the desktop prefers a dark appearance. `MOUSER_SYSTEM_DARK=1|0`
/// overrides detection. When the desktop does not say, Mouser defaults to dark.
pub fn system_prefers_dark() -> bool {
    if let Ok(v) = std::env::var("MOUSER_SYSTEM_DARK") {
        return v.trim() == "1" || v.trim().eq_ignore_ascii_case("true");
    }
    static DETECTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DETECTED.get_or_init(detect_system_dark)
}

#[cfg(target_os = "linux")]
fn detect_system_dark() -> bool {
    // freedesktop / GNOME colour-scheme preference; only an explicit light
    // preference selects the light variant.
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !String::from_utf8_lossy(&o.stdout).contains("prefer-light"))
        .unwrap_or(true)
}

#[cfg(target_os = "macos")]
fn detect_system_dark() -> bool {
    // `AppleInterfaceStyle` is only set (to "Dark") in dark mode, but treat an
    // unreadable setting as dark too.
    std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .map(|o| !o.status.success() || String::from_utf8_lossy(&o.stdout).contains("Dark"))
        .unwrap_or(true)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn detect_system_dark() -> bool {
    true
}

pub fn resolve_active_colors(prefs: &Preferences) -> (u32, u32) {
    let theme_name = prefs.theme.as_deref().unwrap_or("Red Star");
    let use_dark = match prefs.mode.clone().unwrap_or_default() {
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
        ThemeMode::System => system_prefers_dark(),
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
    // Unknown theme: fall back to Red Star
    let (_, dark, light) = builtins().remove(0);
    let def = if use_dark { dark } else { light };
    (parse_hex_rgb(&def.background), parse_hex_rgb(&def.foreground))
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
    let mut all = builtin_pairs();
    // Red Star accents from the wallpaper palette.
    all[0].1.accent = Some("#2FD8E5".into());
    all[0].1.cursor = Some("#94F3F2".into());
    all[0].2.accent = Some("#0B7C88".into());
    all
}

fn builtin_pairs() -> Vec<(String, ThemeDef, ThemeDef)> {
    vec![
        // Red Star (Starlight variant) — extracted from owner's wallpaper palette
        theme_pair("Red Star", "#E6FBFA", "#04192A", "#0B2A3F", "#F2FAFB"),
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
        ThemeDef {
            name: name.to_string(),
            variant: "dark".to_string(),
            foreground: dark_fg.to_string(),
            background: dark_bg.to_string(),
            cursor: None,
            accent: None,
            palette: Vec::new(),
        },
        ThemeDef {
            name: name.to_string(),
            variant: "light".to_string(),
            foreground: light_fg.to_string(),
            background: light_bg.to_string(),
            cursor: None,
            accent: None,
            palette: Vec::new(),
        },
    )
}
