use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub fn resolve_home() -> PathBuf {
    if let Ok(override_home) = std::env::var("MOUSER_HOME") {
        return PathBuf::from(override_home);
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".mouser")
}

pub fn ensure_dirs() -> std::io::Result<()> {
    let root = resolve_home();
    fs::create_dir_all(root.join("themes"))?;
    fs::create_dir_all(root.join("connections"))?;
    Ok(())
}

pub fn connections_dir() -> PathBuf {
    resolve_home().join("connections")
}

pub fn preferences_path() -> PathBuf {
    resolve_home().join("preferences.toml")
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Preferences {
    pub theme: Option<String>,
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub mode: Option<ThemeMode>,
    /// Sidebar width in physical pixels (clamped to the window when used).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<i32>,
    /// Whether the connections sidebar is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_open: Option<bool>,
    /// Whether the open sidebar is collapsed to a thin strip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_collapsed: Option<bool>,
}

/// Load preferences, apply `f`, and save them back (keeps unrelated fields).
pub fn update_preferences(f: impl FnOnce(&mut Preferences)) -> std::io::Result<Preferences> {
    let mut p = load_preferences();
    f(&mut p);
    save_preferences(&p)?;
    Ok(p)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Light,
    Dark,
    #[default]
    System,
}

pub fn load_preferences() -> Preferences {
    let path = preferences_path();
    match fs::read_to_string(&path) {
        Ok(s) => toml::from_str::<Preferences>(&s).unwrap_or_default(),
        Err(_) => Preferences::default(),
    }
}

pub fn save_preferences(p: &Preferences) -> std::io::Result<()> {
    ensure_dirs()?;
    let path = preferences_path();
    let s = toml::to_string_pretty(p).unwrap();
    fs::write(path, s)
}
