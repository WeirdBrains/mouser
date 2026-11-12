//! The preferences mode's model (docs/preferences.md): every known
//! preference by section, its allowed values, validation, reset, and a
//! write-back with `toml_edit` that keeps comments, order and keys Mouser
//! doesn't know. Secrets never go into the file: they go to the OS keychain
//! and the file keeps a `keychain:NAME` reference.

use crate::storage::{Preferences, preferences_path};
use anyhow::{Context, Result, bail};
use std::fs;
pub use toml_edit::DocumentMut;
use toml_edit::{Item, Table, value};

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Bool,
    Int {
        min: i64,
        max: i64,
    },
    Float {
        min: f64,
        max: f64,
    },
    /// One of these values (a dropdown).
    Choice(Vec<String>),
    /// Free text; `url` = must be http(s)://.
    Text {
        url: bool,
    },
    /// Kept in the OS keychain under this name; the file holds `keychain:NAME`.
    Secret(&'static str),
    /// Shown, changed elsewhere (`hint` says how).
    ReadOnly(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub section: &'static str,
    /// Dotted path in preferences.toml, e.g. `models.local.context`.
    pub key: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    /// What an unset value means.
    pub default: &'static str,
    /// Applied at once in open windows (else: new windows).
    pub live: bool,
}

fn f(section: &'static str, key: &'static str, label: &'static str, kind: Kind, default: &'static str, live: bool) -> Field {
    Field { section, key, label, kind, default, live }
}

fn choice(v: &[&str]) -> Kind {
    Kind::Choice(v.iter().map(|s| s.to_string()).collect())
}

/// Every preference, in display order.
pub fn schema() -> Vec<Field> {
    let themes = crate::themes::list_themes();
    let audio = crate::storage::load_preferences().audio.unwrap_or_default();
    let host = audio.host.as_deref();
    let mut devices = vec!["default".to_string()];
    devices.extend(crate::audio::output_devices(host));
    vec![
        f("Appearance", "theme", "Theme", Kind::Choice(themes), "Red Star", true),
        f("Appearance", "mode", "Light / dark", choice(&["system", "light", "dark"]), "system", true),
        f("Appearance", "font_family", "Font family", Kind::Text { url: false }, "(config.toml)", false),
        f("Appearance", "emoji_font", "Emoji font", Kind::Text { url: false }, "(auto: Noto Color Emoji, Apple Color Emoji, Segoe UI Emoji…)", false),
        f("Appearance", "font_size", "Font size", Kind::Float { min: 6.0, max: 72.0 }, "16", false),
        f("Appearance", "appearance.reduce_motion", "Reduce motion", Kind::Bool, "false", true),
        f("Window", "sidebar_open", "Sidebar open", Kind::Bool, "false", false),
        f("Window", "sidebar_collapsed", "Sidebar collapsed", Kind::Bool, "false", false),
        f("Window", "sidebar_width", "Sidebar width (px)", Kind::Int { min: 120, max: 4000 }, "auto", false),
        f("Window", "window.dock_height", "Mode panel height (% of the terminal)", Kind::Int { min: 10, max: 100 }, "auto", false),
        f("Window", "window.dock_full", "Mode panel fullscreen", Kind::Bool, "false", false),
        f("Keys", "keys.leader_timeout_ms", "Esc wait for : (ms)", Kind::Int { min: 100, max: 10_000 }, "1200", true),
        f("Audio", "audio.host", "Audio host", choice(crate::audio::host_choices()), "auto", false),
        f("Audio", "audio.output_device", "Output device", Kind::Choice(devices), "default", false),
        f("Audio", "audio.active_host", "Active host", Kind::ReadOnly("set Audio host"), crate::audio::active_host(host), false),
        f("Plugins", "plugins.max_restarts", "Crash restarts (within 30 s)", Kind::Int { min: 0, max: 100 }, "3", true),
        f("Tabs", "tabs.shell", "Default shell for a new tab", Kind::Text { url: false }, "$SHELL", true),
        f("Chat", "chat.transcripts", "Keep transcripts", Kind::Bool, "true", true),
        f("Models", "models.default", "Default model", Kind::Text { url: false }, "(none)", true),
        f("Local model", "models.local.disabled", "Disabled", Kind::Bool, "false", true),
        f("Local model", "models.local.gpu_layers", "GPU layers", Kind::Int { min: 0, max: 1000 }, "all", true),
        f("Local model", "models.local.context", "Context (tokens)", Kind::Int { min: 512, max: 1_048_576 }, "8192", true),
        f("Local model", "models.local.url", "Server URL", Kind::Text { url: true }, "(built-in llama.cpp)", true),
        f("Local model", "models.local.api", "Server API", choice(&["openai", "anthropic", "google"]), "openai", true),
        f("Local model", "models.local.model", "Server model", Kind::Text { url: false }, "(selected gemma)", true),
        f("Local model", "models.local.key", "Server key", Kind::Secret("local-model-key"), "(none)", true),
        f("Local model", "models.local.dir", "Model folder", Kind::ReadOnly("mouser model local move <dir>"), "$MOUSER_HOME/models", false),
        f("MCP server", "mcp.server.enabled", "Enabled", Kind::Bool, "false", false),
        f("MCP server", "mcp.server.port", "HTTP port", Kind::Int { min: 1024, max: 65535 }, "7801", false),
        f("MCP server", "mcp.server.https", "HTTPS", Kind::Bool, "false", false),
        f("MCP server", "mcp.server.https_port", "HTTPS port", Kind::Int { min: 1024, max: 65535 }, "7802", false),
        f("MCP server", "mcp.server.cert", "Certificate (PEM path)", Kind::Text { url: false }, "(self-signed)", false),
        f("MCP server", "mcp.server.key", "Key (PEM path)", Kind::Text { url: false }, "(self-signed)", false),
        f("MCP server", "mcp.server.bind", "HTTPS address (non-loopback needs HTTPS + token)", Kind::Text { url: false }, "127.0.0.1", false),
        f("MCP server", "mcp.server.tls.source", "Certificate source", choice(&crate::mcp_server::TlsSource::NAMES), "self-signed", false),
        f("MCP server", "mcp.server.tls.domains", "Let's Encrypt domain(s), comma-separated", Kind::Text { url: false }, "(none)", false),
        f("MCP server", "mcp.server.tls.email", "Let's Encrypt contact email", Kind::Text { url: false }, "(none)", false),
        f("MCP server", "mcp.server.tls.staging", "Let's Encrypt staging CA", Kind::Bool, "false", false),
        f("MCP server", "mcp.server.tls.challenge", "ACME challenge", choice(&crate::mcp_acme::Challenge::NAMES), "tls-alpn-01", false),
        f("MCP server", "mcp.server.tls.http_port", "http-01 port", Kind::Int { min: 1, max: 65535 }, "80", false),
        f("MCP server", "mcp.server.tls.dns_hook", "dns-01 hook command", Kind::Text { url: false }, "(ask in mouser mcp cert renew)", false),
        f("MCP server", "mcp.server.tls.directory", "ACME directory URL (other CA)", Kind::Text { url: true }, "(Let's Encrypt)", false),
    ]
}

/// Check and normalise a typed value for `field`.
pub fn validate(field: &Field, raw: &str) -> Result<String> {
    let t = raw.trim();
    match &field.kind {
        Kind::Bool => match t.to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Ok("true".into()),
            "false" | "off" | "no" | "0" => Ok("false".into()),
            _ => bail!("{}: on or off", field.label),
        },
        Kind::Int { min, max } => {
            let n: i64 = t.parse().map_err(|_| anyhow::anyhow!("{}: a whole number", field.label))?;
            if n < *min || n > *max {
                bail!("{}: {min}..{max}", field.label);
            }
            Ok(n.to_string())
        }
        Kind::Float { min, max } => {
            let n: f64 = t.parse().map_err(|_| anyhow::anyhow!("{}: a number", field.label))?;
            if !n.is_finite() || n < *min || n > *max {
                bail!("{}: {min}..{max}", field.label);
            }
            Ok(n.to_string())
        }
        Kind::Choice(v) => v.iter().find(|c| c.eq_ignore_ascii_case(t)).cloned().with_context(|| format!("{}: one of {}", field.label, v.join(", "))),
        Kind::Text { url } => {
            if t.is_empty() {
                bail!("{}: empty (use reset to unset)", field.label);
            }
            if t.chars().any(|c| c.is_control()) {
                bail!("{}: no control characters", field.label);
            }
            if *url && !(t.starts_with("http://") || t.starts_with("https://")) {
                bail!("{}: an http:// or https:// URL", field.label);
            }
            Ok(t.to_string())
        }
        Kind::Secret(_) => {
            if t.is_empty() {
                bail!("{}: empty (use reset to remove)", field.label);
            }
            Ok(t.to_string())
        }
        Kind::ReadOnly(hint) => bail!("{}: change it with `{hint}`", field.label),
    }
}

/// preferences.toml as an editable document (empty if missing). A file
/// that doesn't parse is an error: nothing is written over it.
pub fn load_doc() -> Result<DocumentMut> {
    match fs::read_to_string(preferences_path()) {
        Ok(s) => s.parse::<DocumentMut>().context("preferences.toml doesn't parse; fix it first"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(e).context("read preferences.toml"),
    }
}

fn lookup<'a>(doc: &'a DocumentMut, key: &str) -> Option<&'a Item> {
    let mut it = doc.as_item();
    for part in key.split('.') {
        it = it.get(part)?;
    }
    Some(it)
}

/// The value as shown (`None`: unset, the default applies). Secrets show
/// `set` / unset, never the value.
pub fn get(doc: &DocumentMut, field: &Field) -> Option<String> {
    let v = lookup(doc, field.key)?.as_value()?;
    if let Kind::Secret(_) = field.kind {
        return Some("set (keychain)".into());
    }
    Some(match v {
        toml_edit::Value::String(s) => s.value().clone(),
        other => other.to_string().trim().to_string(),
    })
}

fn table_for<'a>(doc: &'a mut DocumentMut, key: &str) -> Result<(&'a mut Table, String)> {
    let mut parts: Vec<&str> = key.split('.').collect();
    let last = parts.pop().unwrap().to_string();
    let mut t = doc.as_table_mut();
    for p in parts {
        let e = t.entry(p).or_insert_with(|| {
            let mut n = Table::new();
            n.set_implicit(true);
            Item::Table(n)
        });
        t = e.as_table_mut().with_context(|| format!("[{p}] is not a table"))?;
    }
    Ok((t, last))
}

/// Set `field` to a validated value in `doc` (secrets: to the keychain).
pub fn set(doc: &mut DocumentMut, field: &Field, raw: &str) -> Result<()> {
    let v = validate(field, raw)?;
    let item = match &field.kind {
        Kind::Bool => value(v == "true"),
        Kind::Int { .. } => value(v.parse::<i64>()?),
        Kind::Float { .. } => value(v.parse::<f64>()?),
        Kind::Secret(name) => {
            crate::secrets::set_keychain_only(name, &v)?;
            value(format!("keychain:{name}"))
        }
        _ => value(v),
    };
    let (t, last) = table_for(doc, field.key)?;
    // Keep the old value's comments and spacing.
    match (t.get_mut(&last).and_then(|i| i.as_value_mut()), item) {
        (Some(old), Item::Value(mut new)) => {
            *new.decor_mut() = old.decor().clone();
            *old = new;
        }
        (_, item) => {
            t.insert(&last, item);
        }
    }
    Ok(())
}

/// Back to the default: the key is removed (a secret is deleted too).
pub fn reset(doc: &mut DocumentMut, field: &Field) -> Result<()> {
    if let Kind::ReadOnly(hint) = field.kind {
        bail!("{}: change it with `{hint}`", field.label);
    }
    if let Kind::Secret(name) = field.kind {
        crate::secrets::delete(name);
    }
    let (t, last) = table_for(doc, field.key)?;
    t.remove(&last);
    Ok(())
}

/// Write `doc` back: only if Mouser still reads it, atomically (tmp + rename).
pub fn save(doc: &DocumentMut) -> Result<()> {
    let s = doc.to_string();
    toml::from_str::<Preferences>(&s).context("the change would make preferences.toml unreadable; not saved")?;
    crate::storage::ensure_dirs()?;
    let p = preferences_path();
    let tmp = p.with_extension("toml.tmp");
    fs::write(&tmp, &s)?;
    fs::rename(&tmp, &p)?;
    Ok(())
}

/// Sections in display order.
pub fn sections(fields: &[Field]) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = Vec::new();
    for x in fields {
        if !v.contains(&x.section) {
            v.push(x.section);
        }
    }
    v
}

/// The preferences mode's keys (alt+key by default, like every mode).
pub const BINDINGS: &[crate::modes::Binding] = &[
    crate::modes::Binding { id: "up", key: "up", describe: "previous preference" },
    crate::modes::Binding { id: "down", key: "down", describe: "next preference" },
    crate::modes::Binding { id: "prev", key: "left", describe: "previous value (choices, on/off, numbers)" },
    crate::modes::Binding { id: "next", key: "right", describe: "next value (choices, on/off, numbers)" },
    crate::modes::Binding { id: "edit", key: "enter", describe: "edit: type a value, or pick from the dropdown" },
    crate::modes::Binding { id: "reset", key: "r", describe: "reset to the default" },
    crate::modes::Binding { id: "page_up", key: "pageup", describe: "page up" },
    crate::modes::Binding { id: "page_down", key: "pagedown", describe: "page down" },
];

/// The value after `cur` (`dir` = +1 / -1) for choices, on/off and numbers;
/// `None` for text, secrets and read-only fields.
pub fn step(field: &Field, cur: Option<&str>, dir: i64) -> Option<String> {
    let cur = cur.unwrap_or(field.default);
    match &field.kind {
        Kind::Bool => Some(if cur == "true" { "false" } else { "true" }.into()),
        Kind::Choice(v) if !v.is_empty() => {
            let i = v.iter().position(|c| c == cur).map(|i| i as i64).unwrap_or(-dir.signum().max(0));
            Some(v[(i + dir).rem_euclid(v.len() as i64) as usize].clone())
        }
        Kind::Int { min, max } => {
            let n: i64 = cur.parse().unwrap_or(*min);
            let by = if max - min > 1000 { (max - min) / 100 } else { 1 }.max(1);
            Some((n + dir * by).clamp(*min, *max).to_string())
        }
        Kind::Float { min, max } => {
            let n: f64 = cur.parse().unwrap_or(*min);
            Some((n + dir as f64).clamp(*min, *max).to_string())
        }
        _ => None,
    }
}

/// The mode panel's size from `preferences.toml` (`[window] dock_height`, a
/// percentage, and `dock_full`), as the mode API's `DockSize`.
pub fn dock_size() -> (Option<u16>, bool) {
    let Ok(doc) = load_doc() else { return (None, false) };
    let w = doc.get("window");
    let pct = w.and_then(|w| w.get("dock_height")).and_then(|v| v.as_integer()).filter(|p| (10..=100).contains(p));
    let full = w.and_then(|w| w.get("dock_full")).and_then(|v| v.as_bool()).unwrap_or(false);
    (pct.map(|p| (p * 10) as u16), full)
}

/// Save the mode panel's size (`None`: the mode's default height).
pub fn save_dock_size(permille: Option<u16>, full: bool) -> Result<()> {
    let mut doc = load_doc()?;
    if doc.get("window").is_none() {
        doc["window"] = toml_edit::table();
    }
    match permille {
        Some(p) => doc["window"]["dock_height"] = toml_edit::value(((p as i64 + 5) / 10).clamp(10, 100)),
        None => {
            if let Some(t) = doc["window"].as_table_mut() {
                t.remove("dock_height");
            }
        }
    }
    doc["window"]["dock_full"] = toml_edit::value(full);
    save(&doc)
}
