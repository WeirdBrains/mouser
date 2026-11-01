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
    /// GPU rendering engine (`mouser engine use`); unset means `mouser`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// GPU engines in priority order (`mouser engine priority`); `mouser` is
    /// always in it, last unless moved. Replaces `engine`, which is still read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engines: Option<Vec<String>>,
    /// `[mcp.server]`: Mouser's own MCP server (docs/mcp.md); off unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<McpPrefs>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct McpPrefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<McpServerPrefs>,
    /// `[[mcp.servers]]`: MCP servers Mouser connects to (docs/mcp.md §Client).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<McpServerEntry>,
}

/// One `[[mcp.servers]]` entry: a stdio `command` or an HTTP(S) `url`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct McpServerEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    /// Extra environment for a stdio server; values may be `keychain:NAME`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Bearer token reference: `keychain:<secret name>` (never the token itself).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// PEM CA/cert to trust for an HTTPS url (e.g. Mouser's self-signed cert).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// `[mcp.server]` in preferences.toml.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct McpServerPrefs {
    #[serde(default)]
    pub enabled: bool,
    /// Loopback HTTP port (also what `mouser mcp serve --stdio` connects to).
    #[serde(default = "default_http_port")]
    pub port: u16,
    /// Also serve HTTPS on `https_port`.
    #[serde(default)]
    pub https: bool,
    #[serde(default = "default_https_port")]
    pub https_port: u16,
    /// PEM certificate chain and key; unset: a self-signed pair in $MOUSER_HOME/mcp/.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

fn default_http_port() -> u16 {
    7801
}
fn default_https_port() -> u16 {
    7802
}

impl Default for McpServerPrefs {
    fn default() -> Self {
        McpServerPrefs { enabled: false, port: default_http_port(), https: false, https_port: default_https_port(), cert: None, key: None }
    }
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
