use crate::storage::{Preferences, ThemeMode, ensure_dirs, resolve_home};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
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
    /// Selected text colours; without them the selection is drawn inverted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_foreground: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_background: Option<String>,
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
    pub selection_fg: Option<u32>,
    pub selection_bg: Option<u32>,
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
            selection_fg: self.selection_foreground.as_deref().and_then(parse_hex),
            selection_bg: self.selection_background.as_deref().and_then(parse_hex),
        }
    }

    /// The palette, padded with defaults to 16 entries.
    pub fn full_palette(&self) -> Vec<String> {
        (0..16).map(|i| self.palette.get(i).cloned().unwrap_or_else(|| DEFAULT_PALETTE[i].to_string())).collect()
    }

    /// Fill missing cursor / accent / palette so the editor and duplicates never
    /// show blank colour fields. Cursor defaults to foreground; accent to ANSI 4
    /// (blue) from the resolved palette.
    pub fn ensure_complete(&mut self) {
        self.palette = self.full_palette();
        if self.cursor.as_deref().map(str::trim).unwrap_or("").is_empty() {
            self.cursor = Some(self.foreground.clone());
        }
        if self.accent.as_deref().map(str::trim).unwrap_or("").is_empty() {
            self.accent = Some(self.palette.get(4).cloned().unwrap_or_else(|| DEFAULT_PALETTE[4].to_string()));
        }
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

/// Default theme when preferences have none: Omarchy on Omarchy, else Red Star.
/// `MOUSER_OMARCHY=1|0` overrides detection (for tests).
pub fn default_theme_name() -> &'static str {
    if on_omarchy() { "Omarchy" } else { "Red Star" }
}

/// Read the current Omarchy theme's `colors.toml` (Trainyard etc.) into a
/// dark [`ThemeDef`] named "Omarchy". Used by **Match Omarchy**.
pub fn match_omarchy() -> Option<ThemeDef> {
    let home = dirs::home_dir()?;
    let name = fs::read_to_string(home.join(".local/state/omarchy/current/theme.name")).ok()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let dir = {
        let user = home.join(".config/omarchy/themes").join(&name);
        if user.join("colors.toml").is_file() {
            user
        } else {
            // Bundled Omarchy themes live next to the install prefix when set.
            let bundled = std::env::var_os("OMARCHY_PATH").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/usr/share/omarchy"));
            bundled.join("themes").join(&name)
        }
    };
    let raw = fs::read_to_string(dir.join("colors.toml")).ok()?;
    parse_omarchy_colors_for_test(&raw, "Omarchy")
}

pub fn parse_omarchy_colors_for_test(toml_src: &str, name: &str) -> Option<ThemeDef> {
    let v: toml::Value = toml::from_str(toml_src).ok()?;
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|s| s.to_string());
    let background = get("background")?;
    let foreground = get("foreground")?;
    let accent = get("accent");
    let cursor = get("cyan").or_else(|| get("bright_cyan")).or_else(|| accent.clone());
    let selection_background = get("selection");
    let selection_foreground = get("selection_foreground").or_else(|| get("background"));
    let keys = [
        "dark_background",
        "red",
        "green",
        "yellow",
        "blue",
        "magenta",
        "cyan",
        "foreground",
        "lighter_background",
        "bright_red",
        "bright_green",
        "bright_yellow",
        "bright_blue",
        "bright_magenta",
        "bright_cyan",
        "bright_foreground",
    ];
    // Prefer explicit dark_background for ANSI 0; fall back to background.
    let mut palette = Vec::with_capacity(16);
    for (i, k) in keys.iter().enumerate() {
        let fallback = if i == 0 { background.as_str() } else { "#888888" };
        palette.push(get(k).unwrap_or_else(|| fallback.to_string()));
    }
    // yellow key in Omarchy file is bright cyan-ish; map orange into yellow if present for ANSI 3? Keep as-is from colors.toml.
    if let Some(y) = get("yellow") {
        palette[3] = y;
    }
    Some(ThemeDef {
        name: name.to_string(),
        variant: "dark".to_string(),
        foreground,
        background,
        cursor,
        accent,
        palette,
        selection_foreground,
        selection_background,
    })
}

/// Whether this host looks like Omarchy (os-release `ID=omarchy`, or override).
pub fn on_omarchy() -> bool {
    match std::env::var("MOUSER_OMARCHY").ok().as_deref() {
        Some("1") | Some("true") | Some("yes") => return true,
        Some("0") | Some("false") | Some("no") => return false,
        _ => {}
    }
    fs::read_to_string("/etc/os-release").ok().is_some_and(|s| {
        s.lines().any(|l| {
            let l = l.trim();
            l == "ID=omarchy" || l.starts_with("ID=") && l.contains("omarchy")
        })
    })
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

/// Valid user theme name: non-empty, no `/` or `\0`, not only whitespace.
pub fn valid_theme_name(name: &str) -> bool {
    let t = name.trim();
    !t.is_empty() && t.len() <= 64 && !t.contains('/') && !t.contains('\0') && !t.starts_with('.')
}

/// Whether `name` already exists (built-in or on disk).
pub fn theme_exists(name: &str) -> bool {
    list_themes().iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// Copy both variants of `src` into a new theme named `dest` (trimmed).
/// Fails if the name is invalid or already taken.
pub fn duplicate_theme(src: &str, dest: &str) -> Result<(), String> {
    let dest = dest.trim();
    if !valid_theme_name(dest) {
        return Err("name must be 1–64 characters without '/'".into());
    }
    if theme_exists(dest) {
        return Err(format!("a theme named '{dest}' already exists"));
    }
    let mut dark = load_theme(src, "dark");
    let mut light = load_theme(src, "light");
    dark.name = dest.to_string();
    light.name = dest.to_string();
    dark.variant = "dark".into();
    light.variant = "light".into();
    dark.ensure_complete();
    light.ensure_complete();
    save_theme(&dark).map_err(|e| e.to_string())?;
    save_theme(&light).map_err(|e| e.to_string())?;
    Ok(())
}

/// Create a new theme from Red Star (both variants) under `name`.
pub fn create_theme(name: &str) -> Result<(), String> {
    duplicate_theme("Red Star", name)
}

/// Whether `name` is one of the built-in themes (those are read-only).
pub fn is_builtin(name: &str) -> bool {
    builtins().iter().any(|(n, _, _)| n == name)
}

/// The canonical spelling of `name` (built-in or on disk), ignoring case.
pub fn find_theme(name: &str) -> Option<String> {
    list_themes().into_iter().find(|n| n.eq_ignore_ascii_case(name))
}

/// `"<name> copy"`, then `"<name> copy 2"`, and so on, until the name is free.
pub fn unique_copy_name(src: &str) -> String {
    let base = format!("{src} copy");
    if !theme_exists(&base) {
        return base;
    }
    for n in 2..1000 {
        let c = format!("{base} {n}");
        if !theme_exists(&c) {
            return c;
        }
    }
    format!("{base} {}", std::process::id())
}

/// Duplicate `src`. `name` empty or absent becomes [`unique_copy_name`].
pub fn duplicate_named(src: &str, name: Option<&str>) -> Result<String, String> {
    let src = find_theme(src).ok_or_else(|| format!("no theme named '{src}'"))?;
    let dest = match name.map(str::trim).filter(|s| !s.is_empty()) {
        Some(n) => n.to_string(),
        None => unique_copy_name(&src),
    };
    duplicate_theme(&src, &dest)?;
    Ok(dest)
}

/// Create `name` from Red Star, then overlay `patch`.
///
/// Missing colour fields stay as Red Star. `variant` is `dark`, `light` or
/// `both` (default `both`): `both` writes the same overlay into each variant,
/// otherwise only that variant changes and the other stays Red Star. A short
/// `palette` is padded with the default ANSI colours. A bad hex value is an
/// error and nothing is written.
pub fn create_from_values(name: &str, patch: &serde_json::Value) -> Result<(), String> {
    let name = name.trim();
    if !valid_theme_name(name) {
        return Err("name must be 1–64 characters, not starting with '.' and without '/'".into());
    }
    if theme_exists(name) {
        return Err(format!("a theme named '{name}' already exists"));
    }
    let mut dark = load_theme("Red Star", "dark");
    let mut light = load_theme("Red Star", "light");
    dark.name = name.to_string();
    light.name = name.to_string();
    dark.variant = "dark".into();
    light.variant = "light".into();
    apply_variant(&mut dark, &mut light, patch)?;
    dark.ensure_complete();
    light.ensure_complete();
    save_theme(&dark).map_err(|e| e.to_string())?;
    save_theme(&light).map_err(|e| e.to_string())?;
    Ok(())
}

/// Merge `patch` into a user theme. Built-ins are refused.
/// Omitted fields stay as they are. `variant` selects which file changes
/// (`both` is the default). See [`create_from_values`] for the fields.
pub fn edit_user_theme(name: &str, patch: &serde_json::Value) -> Result<(), String> {
    let name = find_theme(name).ok_or_else(|| format!("no theme named '{name}'"))?;
    if is_builtin(&name) {
        return Err(format!("'{name}' is a built-in theme and read-only; use duplicate_theme"));
    }
    let mut dark = load_theme(&name, "dark");
    let mut light = load_theme(&name, "light");
    apply_variant(&mut dark, &mut light, patch)?;
    dark.name = name.clone();
    light.name = name;
    dark.variant = "dark".into();
    light.variant = "light".into();
    dark.ensure_complete();
    light.ensure_complete();
    save_theme(&dark).map_err(|e| e.to_string())?;
    save_theme(&light).map_err(|e| e.to_string())?;
    Ok(())
}

/// Both variants of `name`, plus whether it is built in.
pub fn theme_values(name: &str) -> Result<serde_json::Value, String> {
    let name = find_theme(name).ok_or_else(|| format!("no theme named '{name}'"))?;
    let dark = load_theme(&name, "dark");
    let light = load_theme(&name, "light");
    Ok(serde_json::json!({"name": name, "builtin": is_builtin(&name), "dark": dark, "light": light}))
}

/// Make `name` the theme stored in preferences. The caller refreshes open windows.
pub fn set_active_theme(name: &str) -> Result<String, String> {
    let name = find_theme(name).ok_or_else(|| format!("no theme named '{name}'"))?;
    crate::storage::update_preferences(|p| p.theme = Some(name.clone())).map_err(|e| e.to_string())?;
    Ok(name)
}

fn apply_variant(dark: &mut ThemeDef, light: &mut ThemeDef, patch: &serde_json::Value) -> Result<(), String> {
    let variant = patch.get("variant").and_then(|v| v.as_str()).unwrap_or("both");
    match variant {
        "dark" => apply_patch(dark, patch)?,
        "light" => apply_patch(light, patch)?,
        "both" => {
            apply_patch(dark, patch)?;
            apply_patch(light, patch)?;
        }
        _ => return Err("variant must be dark, light or both".into()),
    }
    Ok(())
}

fn apply_patch(def: &mut ThemeDef, patch: &serde_json::Value) -> Result<(), String> {
    if let Some(s) = hex_field(patch, "foreground")? {
        def.foreground = s;
    }
    if let Some(s) = hex_field(patch, "background")? {
        def.background = s;
    }
    if let Some(s) = hex_field(patch, "cursor")? {
        def.cursor = Some(s);
    }
    if let Some(s) = hex_field(patch, "accent")? {
        def.accent = Some(s);
    }
    if let Some(s) = hex_field(patch, "selection_foreground")? {
        def.selection_foreground = Some(s);
    }
    if let Some(s) = hex_field(patch, "selection_background")? {
        def.selection_background = Some(s);
    }
    if let Some(p) = patch.get("palette") {
        let arr = p.as_array().ok_or("palette must be an array of #rrggbb")?;
        if arr.len() > 16 {
            return Err("palette has at most 16 colours".into());
        }
        let mut out = Vec::new();
        for (i, c) in arr.iter().enumerate() {
            let s = c.as_str().ok_or("palette entries must be strings")?;
            out.push(normalize_hex(s).ok_or_else(|| format!("palette[{i}] must be #rrggbb"))?);
        }
        def.palette = out;
    }
    Ok(())
}

fn hex_field(v: &serde_json::Value, key: &str) -> Result<Option<String>, String> {
    match v.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(normalize_hex(s).ok_or_else(|| format!("{key} must be #rrggbb (got {s})"))?)),
        _ => Err(format!("{key} must be a string")),
    }
}

fn normalize_hex(s: &str) -> Option<String> {
    parse_hex(s).map(|v| format!("#{v:06X}"))
}

/// Export both variants of `name` into `dir` as `dark.toml` / `light.toml`.
pub fn export_theme(name: &str, dir: &std::path::Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for variant in ["dark", "light"] {
        let mut def = load_theme(name, variant);
        def.name = name.to_string();
        def.variant = variant.into();
        let path = dir.join(format!("{variant}.toml"));
        let body = toml::to_string_pretty(&def).map_err(|e| e.to_string())?;
        fs::write(&path, body).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Import a theme from `dir` containing `dark.toml` and/or `light.toml`.
/// `as_name` overrides the name stored in the files / directory name.
pub fn import_theme(dir: &std::path::Path, as_name: Option<&str>) -> Result<String, String> {
    let dark_path = dir.join("dark.toml");
    let light_path = dir.join("light.toml");
    let dark = read_variant_file(&dark_path)?;
    let light = read_variant_file(&light_path)?;
    if dark.is_none() && light.is_none() {
        return Err("no dark.toml or light.toml in that folder".into());
    }
    let mut dark = dark.or_else(|| light.clone()).expect("checked");
    let mut light = light.unwrap_or_else(|| dark.clone());
    let fallback = dir.file_name().and_then(|s| s.to_str()).unwrap_or("Imported").to_string();
    let from_file = dark.name.trim().to_string();
    let candidate = as_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .filter(|s| valid_theme_name(s))
        .or_else(|| valid_theme_name(&from_file).then_some(from_file))
        .or_else(|| valid_theme_name(&fallback).then_some(fallback.clone()));
    let name = candidate.ok_or_else(|| "could not derive a valid theme name".to_string())?;
    if theme_exists(&name) {
        return Err(format!("a theme named '{name}' already exists"));
    }
    dark.name = name.clone();
    dark.variant = "dark".into();
    light.name = name.clone();
    light.variant = "light".into();
    save_theme(&dark).map_err(|e| e.to_string())?;
    save_theme(&light).map_err(|e| e.to_string())?;
    Ok(name)
}

fn read_variant_file(path: &std::path::Path) -> Result<Option<ThemeDef>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let s = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let def: ThemeDef = toml::from_str(&s).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(def))
}

/// The active theme variant for `prefs`.
pub fn resolve_active_theme(prefs: &Preferences) -> ThemeDef {
    load_theme(prefs.theme.as_deref().unwrap_or(default_theme_name()), active_variant(prefs))
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
    let theme_name = prefs.theme.as_deref().unwrap_or(default_theme_name());
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
    // Omarchy (Trainyard) palette from ~/.config/omarchy/themes/trainyard/colors.toml.
    let omarchy_pal = [
        "#050a11", "#8e92bd", "#86d0e3", "#a5ffff", "#5e84a9", "#91ace2", "#93e7ff", "#fff1c8", "#20252d", "#a1a7dc", "#87eaff", "#8dffff",
        "#6b9ac7", "#9fc2ff", "#96ffff", "#fff5d6",
    ];
    if let Some((_, dark, light)) = all.iter_mut().find(|(n, _, _)| n == "Omarchy") {
        dark.accent = Some("#5e84a9".into());
        dark.cursor = Some("#93e7ff".into());
        dark.selection_background = Some("#20252d".into());
        dark.selection_foreground = Some("#070D16".into());
        dark.palette = omarchy_pal.iter().map(|s| (*s).into()).collect();
        light.accent = Some("#5e84a9".into());
        light.cursor = Some("#5e84a9".into());
        light.selection_background = Some("#d8e3ef".into());
        light.selection_foreground = Some("#070D16".into());
        light.palette = omarchy_pal.iter().map(|s| (*s).into()).collect();
    }
    all
}

fn builtin_pairs() -> Vec<(String, ThemeDef, ThemeDef)> {
    vec![
        // Red Star (Starlight variant) — extracted from owner's wallpaper palette
        theme_pair("Red Star", "#E6FBFA", "#04192A", "#0B2A3F", "#F2FAFB"),
        // Omarchy (Trainyard) — from Omarchy theme colors; default on Omarchy hosts.
        theme_pair("Omarchy", "#fff1c8", "#070D16", "#070D16", "#f5f0e6"),
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
            selection_foreground: None,
            selection_background: None,
        },
        ThemeDef {
            name: name.to_string(),
            variant: "light".to_string(),
            foreground: light_fg.to_string(),
            background: light_bg.to_string(),
            cursor: None,
            accent: None,
            palette: Vec::new(),
            selection_foreground: None,
            selection_background: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn theme_tools_create_duplicate_edit_and_refuse_builtins() {
        let t = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("MOUSER_HOME", t.path()) };
        assert!(is_builtin("Red Star"));
        assert_eq!(theme_values("red star").unwrap()["builtin"], true);
        let err = edit_user_theme("Red Star", &serde_json::json!({"foreground": "#112233"})).unwrap_err();
        assert!(err.contains("duplicate_theme"), "{err}");

        create_from_values("Moss", &serde_json::json!({"foreground": "#abc"})).unwrap();
        let got = theme_values("Moss").unwrap();
        assert_eq!(got["dark"]["foreground"], "#AABBCC");
        assert_eq!(got["dark"]["background"], theme_values("Red Star").unwrap()["dark"]["background"]);
        assert!(create_from_values("Bad", &serde_json::json!({"foreground": "nope"})).unwrap_err().contains("#rrggbb"));

        edit_user_theme("Moss", &serde_json::json!({"variant": "light", "accent": "#010203"})).unwrap();
        let got = theme_values("Moss").unwrap();
        assert_eq!(got["light"]["accent"], "#010203");
        assert_ne!(got["dark"]["accent"].as_str(), Some("#010203"));

        let copy = duplicate_named("Moss", None).unwrap();
        assert_eq!(copy, "Moss copy");
        let copy2 = duplicate_named("Moss", None).unwrap();
        assert_eq!(copy2, "Moss copy 2");
        assert_eq!(duplicate_named("Moss", Some("Pond")).unwrap(), "Pond");

        let active = set_active_theme("pond").unwrap();
        assert_eq!(active, "Pond");
        assert_eq!(crate::storage::load_preferences().theme.as_deref(), Some("Pond"));
        unsafe { std::env::remove_var("MOUSER_HOME") };
    }
}
