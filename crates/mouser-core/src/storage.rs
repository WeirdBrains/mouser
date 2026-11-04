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
    /// Colour emoji font family (`"none"` turns colour emoji off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji_font: Option<String>,
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
    /// `[models]`: model providers and the default model (docs/models.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<crate::models::ModelsPrefs>,
    /// `[chat]`: the agent chat mode (docs/chat.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<crate::agent::ChatPrefs>,
    /// `[keys]`: keyboard timing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<KeysPrefs>,
    /// `[plugins]`: how plugins are supervised (docs/mode-protocol.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<PluginsPrefs>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PluginsPrefs {
    /// Crashes within 30 s after which a plugin stays off until restarted
    /// from the status bar. Default [`DEFAULT_MAX_RESTARTS`]; 0 never restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_restarts: Option<u32>,
}

/// Default `[plugins] max_restarts`.
pub const DEFAULT_MAX_RESTARTS: u32 = 3;

/// `[plugins] max_restarts`, at most 100.
pub fn max_restarts(p: &Preferences) -> u32 {
    p.plugins.as_ref().and_then(|x| x.max_restarts).unwrap_or(DEFAULT_MAX_RESTARTS).min(100)
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KeysPrefs {
    /// How long a lone Esc waits for `:` (the `esc :` command line) before it
    /// goes to the shell, in ms. Default [`DEFAULT_LEADER_TIMEOUT_MS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leader_timeout_ms: Option<u64>,
}

/// Default `esc :` wait (was 500 ms before 1.0.1).
pub const DEFAULT_LEADER_TIMEOUT_MS: u64 = 1200;

/// The `esc :` wait from `[keys] leader_timeout_ms`, kept within 100 ms..10 s.
pub fn leader_timeout(p: &Preferences) -> std::time::Duration {
    let ms = p.keys.as_ref().and_then(|k| k.leader_timeout_ms).unwrap_or(DEFAULT_LEADER_TIMEOUT_MS);
    std::time::Duration::from_millis(ms.clamp(100, 10_000))
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
    /// Address the HTTPS listener binds (default 127.0.0.1). A non-loopback
    /// address needs HTTPS and a token; plain HTTP always stays on loopback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
    /// `[mcp.server.tls]`: where the HTTPS certificate comes from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<McpTlsPrefs>,
}

/// `[mcp.server.tls]` (docs/mcp.md §Let's Encrypt).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct McpTlsPrefs {
    /// `self-signed`, `files` (cert/key above) or `acme` (Let's Encrypt).
    /// Unset: `files` when cert and key are set, else `self-signed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// ACME: the public DNS name(s), comma-separated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domains: Option<String>,
    /// ACME: contact email for expiry notices (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// ACME: use Let's Encrypt's staging CA (untrusted test certs, loose limits).
    #[serde(default)]
    pub staging: bool,
    /// ACME: another CA's directory URL (overrides `staging`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// ACME: `tls-alpn-01` (default, on the HTTPS port), `http-01` or `dns-01`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenge: Option<String>,
    /// ACME http-01: port of the challenge listener (default 80).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_port: Option<u16>,
    /// ACME dns-01: command run to add/remove the TXT record (gets
    /// MOUSER_ACME_ACTION/NAME/TXT/DOMAIN); unset: `mouser mcp cert renew` asks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_hook: Option<String>,
}

fn default_http_port() -> u16 {
    7801
}
fn default_https_port() -> u16 {
    7802
}

impl Default for McpServerPrefs {
    fn default() -> Self {
        McpServerPrefs {
            enabled: false,
            port: default_http_port(),
            https: false,
            https_port: default_https_port(),
            cert: None,
            key: None,
            bind: None,
            tls: None,
        }
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
