//! Plugins (docs/plugins.md): the `plugin.toml` manifest, the plugins
//! directory, enable/disable and priority state, the registry by kind, and the
//! command transport (an executable behind `[mouser]` line commands).
//!
//! Everything is a plugin (P6): the built-in modes (commander, chat) and
//! engines (mouser, skia) are compiled-in plugins with their own `plugin.toml`
//! ([`BUILTIN_MANIFESTS`], `transport = "builtin"`). Engines may be described
//! by `plugin.toml` (`kind = "engine"`, `transport = "library"`, `[engine]`) or
//! the older `engine.toml`, which is read as the same manifest
//! ([`Manifest::from_engine`]); their C ABI and loader stay in
//! `crate::engines`. Built-ins can be disabled, except the `mouser` engine (the
//! permanent fallback); with every mode off Mouser is a plain terminal.

use crate::storage;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Plugin API version this build speaks.
pub const API: u32 = 1;

/// What a plugin provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Mode,
    Engine,
    ModelProvider,
    ToolProvider,
    Command,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Mode, Kind::Engine, Kind::ModelProvider, Kind::ToolProvider, Kind::Command];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Mode => "mode",
            Kind::Engine => "engine",
            Kind::ModelProvider => "model-provider",
            Kind::ToolProvider => "tool-provider",
            Kind::Command => "command",
        }
    }
}

/// How Mouser reaches a plugin. Native libraries are only for engines (their C ABI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    /// Compiled into Mouser (listed, never installed).
    Builtin,
    /// An MCP server over stdio (`run.command`).
    Mcp,
    /// A program inside the terminal, over Fuzzball `p` (the manifest is optional).
    Fuzzball,
    /// An executable run once per `[mouser]` command: JSON in, JSON out.
    Command,
    /// A model provider Mouser talks to over HTTP (`[model]` table, docs/models.md).
    Http,
    /// A native engine library over the engine C ABI (`[engine]` table, docs/engines.md).
    Library,
}

impl Transport {
    pub fn name(self) -> &'static str {
        match self {
            Transport::Builtin => "builtin",
            Transport::Mcp => "mcp",
            Transport::Fuzzball => "fuzzball",
            Transport::Command => "command",
            Transport::Http => "http",
            Transport::Library => "library",
        }
    }
}

/// Capabilities a plugin can ask for; each goes through the permission broker.
pub const CAPABILITIES: [(&str, &str); 9] = [
    ("run", "run its program on this computer"),
    ("notify", "show notices"),
    ("screen.read", "read the visible text of the tab"),
    ("scrollback.read", "read the tab's scrollback"),
    ("shell.type", "type or run commands in a shell"),
    ("files.read", "read files"),
    ("files.write", "change or delete files"),
    ("network", "use the network"),
    ("models.download", "download or start a model"),
];

pub fn capability_text(cap: &str) -> &'static str {
    if cap.starts_with("tool.") {
        return "call this MCP tool";
    }
    CAPABILITIES.iter().find(|(c, _)| *c == cap).map_or("(unknown capability)", |(_, t)| t)
}

pub fn known_capability(cap: &str) -> bool {
    cap.strip_prefix("tool.").is_some_and(|t| !t.is_empty()) || CAPABILITIES.iter().any(|(c, _)| *c == cap)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Provide {
    pub kind: Kind,
    /// Defaults to the plugin name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Run {
    pub transport: Transport,
    /// argv; a relative first element is resolved in the plugin's directory.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    /// `[mouser]` line commands (transport = "command").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
}

/// `plugin.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    #[serde(default = "api_default")]
    pub api: u32,
    #[serde(default)]
    pub description: String,
    pub provides: Vec<Provide>,
    pub run: Run,
    /// Requested capabilities (see [`CAPABILITIES`]).
    #[serde(default)]
    pub permissions: Vec<String>,
    /// `[model]`: a model-provider plugin served over HTTP (docs/models.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<crate::models::ModelSection>,
    /// `[engine]`: an engine plugin (`kind = "engine"`, `transport = "library"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineSection>,
    /// `[mode]`: a mode plugin's hotkey and keys (docs/modes.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ModeSection>,
}

/// `[engine]` in plugin.toml: the same fields engine.toml has.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EngineSection {
    /// Engine C ABI version (see `crate::engines::ABI`).
    pub abi: u32,
    /// The shared library, relative to the plugin directory.
    pub library: String,
}

/// `[mode]` in plugin.toml.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModeSection {
    /// The key after `ctrl+shift+m` that switches to this mode (one letter or digit).
    pub hotkey: String,
    /// Keys the mode handles while its dock has focus (`[[mode.keys]]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<ModeKey>,
}

/// One `[[mode.keys]]` entry: the plugin gets `id` when `key` is pressed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModeKey {
    pub id: String,
    pub key: String,
    #[serde(default)]
    pub describe: String,
}

fn api_default() -> u32 {
    API
}

impl Manifest {
    pub fn parse(s: &str) -> Result<Self> {
        let m: Manifest = toml::from_str(s).context("plugin.toml")?;
        m.check()?;
        Ok(m)
    }

    pub fn check(&self) -> Result<()> {
        if !crate::engines::valid_name(&self.name) {
            bail!("invalid plugin name {:?} (use a-z, 0-9, - and _)", self.name);
        }
        if BUILTINS.iter().any(|b| b.0 == self.name) || crate::engines::is_builtin(&self.name) {
            bail!("{:?} is a built-in name", self.name);
        }
        if self.api != API {
            bail!("plugin {} needs plugin API {}, this Mouser supports {API}", self.name, self.api);
        }
        if self.provides.is_empty() {
            bail!("plugin {} provides nothing (add [[provides]] kind = ...)", self.name);
        }
        let is_engine = self.provides.iter().any(|p| p.kind == Kind::Engine);
        if is_engine {
            let Some(e) = &self.engine else { bail!("an engine plugin needs an [engine] table (abi, library)") };
            if self.run.transport != Transport::Library {
                bail!("an engine plugin uses transport = \"library\"");
            }
            if self.provides.iter().any(|p| p.kind != Kind::Engine) {
                bail!("an engine plugin provides only kind = \"engine\"");
            }
            if e.abi != crate::engines::ABI {
                bail!("engine ABI {} is not supported (this Mouser speaks {})", e.abi, crate::engines::ABI);
            }
            if e.library.is_empty() || e.library.contains("..") || Path::new(&e.library).is_absolute() {
                bail!("[engine] library must be a file name inside the plugin directory");
            }
        } else if self.engine.is_some() || self.run.transport == Transport::Library {
            bail!("[engine] and transport \"library\" are only for kind = \"engine\"");
        }
        let is_mode = self.provides.iter().any(|p| p.kind == Kind::Mode);
        if is_mode {
            let Some(m) = &self.mode else { bail!("a mode plugin needs a [mode] table with a hotkey") };
            check_mode(&self.name, m)?;
            match self.run.transport {
                Transport::Command => {}
                Transport::Mcp => bail!("mode plugins over MCP are not supported yet; use transport = \"command\""),
                t => bail!("a mode plugin uses transport = \"command\" (not {:?})", t.name()),
            }
        } else if self.mode.is_some() {
            bail!("[mode] is only for kind = \"mode\"");
        }
        for c in &self.permissions {
            if !known_capability(c) {
                bail!("unknown permission {c:?}");
            }
        }
        match self.run.transport {
            Transport::Builtin => bail!("transport \"builtin\" is reserved for plugins compiled into Mouser"),
            Transport::Mcp | Transport::Command if self.run.command.is_empty() => {
                bail!("run.command is required for transport {:?}", self.run.transport.name())
            }
            Transport::Fuzzball if !self.run.command.is_empty() => bail!("a Fuzzball plugin runs in the terminal; it has no run.command"),
            Transport::Http if self.model.is_none() => bail!("transport \"http\" is for model providers with a [model] table"),
            _ => {}
        }
        let has_cmd_kind = self.provides.iter().any(|p| p.kind == Kind::Command);
        if self.run.transport == Transport::Command {
            if has_cmd_kind != !self.run.commands.is_empty() || (!has_cmd_kind && !is_mode) {
                bail!("a command plugin lists run.commands and provides kind = \"command\" (or provides a mode)");
            }
            for c in &self.run.commands {
                if !valid_command(c) {
                    bail!("invalid command name {c:?}");
                }
            }
        } else if !self.run.commands.is_empty() {
            bail!("run.commands is only for transport \"command\"");
        }
        Ok(())
    }

    /// engine.toml read as plugin.toml: the same engine as a kind-engine manifest.
    pub fn from_engine(e: &crate::engines::Manifest) -> Self {
        Manifest {
            name: e.name.clone(),
            version: e.version.clone(),
            api: API,
            description: e.description.clone(),
            provides: vec![Provide { kind: Kind::Engine, id: String::new() }],
            run: Run { transport: Transport::Library, command: vec![], commands: vec![] },
            permissions: vec![],
            model: None,
            engine: Some(EngineSection { abi: e.abi, library: e.library.clone() }),
            mode: None,
        }
    }

    /// A kind-engine manifest as the engine loader's manifest.
    pub fn to_engine(&self) -> Option<crate::engines::Manifest> {
        let e = self.engine.as_ref()?;
        Some(crate::engines::Manifest {
            name: self.name.clone(),
            version: self.version.clone(),
            abi: e.abi,
            library: e.library.clone(),
            description: self.description.clone(),
        })
    }

    pub fn kinds(&self) -> Vec<Kind> {
        let s: BTreeSet<Kind> = self.provides.iter().map(|p| p.kind).collect();
        s.into_iter().collect()
    }
}

/// Hotkeys the built-in modes use (a plugin mode can't take them).
fn builtin_hotkeys() -> Vec<String> {
    BUILTIN_MANIFESTS.iter().filter_map(|(_, s)| builtin_parse(s).mode.map(|m| m.hotkey)).collect()
}

fn valid_key(k: &str) -> bool {
    let mut c = k.chars();
    matches!((c.next(), c.next()), (Some(ch), None) if ch.is_ascii_lowercase() || ch.is_ascii_digit())
        || matches!(k, "up" | "down" | "left" | "right" | "enter" | "pageup" | "pagedown" | "home" | "end" | "tab")
}

fn check_mode(name: &str, m: &ModeSection) -> Result<()> {
    let mut c = m.hotkey.chars();
    if !matches!((c.next(), c.next()), (Some(ch), None) if ch.is_ascii_lowercase() || ch.is_ascii_digit()) {
        bail!("[mode] hotkey must be one lowercase letter or digit, not {:?}", m.hotkey);
    }
    // `p` toggles the mode picker; built-in modes keep theirs.
    if m.hotkey == "p" || builtin_hotkeys().contains(&m.hotkey) {
        bail!("[mode] hotkey {:?} is taken by Mouser (built-ins use {})", m.hotkey, builtin_hotkeys().join(", "));
    }
    let mut ids = BTreeSet::new();
    for k in &m.keys {
        if !valid_command(&k.id) && !crate::engines::valid_name(&k.id) {
            bail!("[[mode.keys]] id {:?} is not a valid name", k.id);
        }
        if !valid_key(&k.key) || k.key == "esc" {
            bail!("[[mode.keys]] key {:?} for {} is not a single letter/digit or arrow/enter/page key", k.key, k.id);
        }
        if !ids.insert(&k.id) {
            bail!("[[mode.keys]] id {:?} appears twice in {name}", k.id);
        }
    }
    Ok(())
}

/// A `[mouser]` line command a plugin may claim: lowercase word, not a Mouser command.
pub fn valid_command(c: &str) -> bool {
    crate::engines::valid_name(c) && !RESERVED_COMMANDS.contains(&c)
}

/// Mouser's own `[mouser]` line words (plugins can't take them).
pub const RESERVED_COMMANDS: [&str; 16] = [
    "theme",
    "mode",
    "edit-theme",
    "font-size",
    "font",
    "open-sidebar",
    "close-sidebar",
    "del-conn",
    "edit-conn",
    "set",
    "save-conn",
    "connect",
    "yes",
    "no",
    "commander",
    "c",
];

/// The compiled-in plugins that have a full manifest (modes and engines).
pub const BUILTIN_MANIFESTS: [(&str, &str); 4] = [
    ("commander", include_str!("builtin/commander.toml")),
    ("chat", include_str!("builtin/chat.toml")),
    ("mouser", include_str!("builtin/mouser.toml")),
    ("skia", include_str!("builtin/skia.toml")),
];

fn builtin_parse(s: &str) -> Manifest {
    toml::from_str(s).expect("built-in plugin.toml")
}

/// The compiled-in manifest of a built-in mode or engine.
pub fn builtin_manifest(name: &str) -> Option<Manifest> {
    BUILTIN_MANIFESTS.iter().find(|(n, _)| *n == name).map(|(_, s)| builtin_parse(s))
}

/// The one plugin that can't be disabled: the permanent engine fallback.
pub fn can_disable(name: &str) -> bool {
    name != crate::engines::DEFAULT
}

/// Built-in providers that are not engines, in their default order (modes first).
pub const BUILTINS: [(&str, Kind, &str); 9] = [
    ("commander", Kind::Mode, "file commander mode (built in)"),
    ("chat", Kind::Mode, "agent chat mode (built in)"),
    ("openai", Kind::ModelProvider, "OpenAI (built in; mouser model add openai)"),
    ("xai", Kind::ModelProvider, "xAI Grok (built in, OpenAI-compatible)"),
    ("openrouter", Kind::ModelProvider, "OpenRouter (built in, OpenAI-compatible)"),
    ("moonshot", Kind::ModelProvider, "Moonshot Kimi (built in, OpenAI-compatible)"),
    ("anthropic", Kind::ModelProvider, "Anthropic Messages (built in)"),
    ("google", Kind::ModelProvider, "Google Gemini (built in)"),
    ("gemma-local", Kind::ModelProvider, "Gemma 4 E2B/E4B on llama.cpp (stub; P4b)"),
];

pub fn dir() -> PathBuf {
    storage::resolve_home().join("plugins")
}

#[derive(Debug, Clone)]
pub struct Installed {
    pub manifest: Manifest,
    pub dir: PathBuf,
}

impl Installed {
    /// argv; a program starting with `./` is resolved in the plugin directory, others on PATH.
    pub fn argv(&self) -> Vec<String> {
        let mut v = self.manifest.run.command.clone();
        if let Some(p) = v.first_mut()
            && (p.starts_with("./") || p.starts_with("../") || p.starts_with(".\\"))
        {
            *p = self.dir.join(&*p).display().to_string();
        }
        v
    }
}

fn read_manifest(d: &Path) -> Result<Manifest> {
    let f = d.join("plugin.toml");
    Manifest::parse(&fs::read_to_string(&f).with_context(|| format!("reading {}", f.display()))?)
}

/// Installed plugins (broken ones as errors), sorted by directory name.
pub fn installed() -> Vec<std::result::Result<Installed, (String, String)>> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir()) else { return out };
    let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for d in dirs {
        let name = d.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        out.push(read_manifest(&d).map(|manifest| Installed { manifest, dir: d.clone() }).map_err(|e| (name, format!("{e:#}"))));
    }
    out
}

pub fn find(name: &str) -> Option<Installed> {
    installed().into_iter().flatten().find(|i| i.manifest.name == name)
}

/// Copy a plugin directory (its `plugin.toml` and the files beside it, one level
/// of subdirectories deep) into the plugins directory, replacing an older copy.
pub fn install(src: &Path) -> Result<Installed> {
    let d = if src.is_dir() { src.to_path_buf() } else { src.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(".")) };
    let m = read_manifest(&d)?;
    for i in installed().into_iter().flatten() {
        if i.manifest.name != m.name {
            for c in &m.run.commands {
                if i.manifest.run.commands.contains(c) {
                    bail!("command {c:?} is already provided by plugin {}", i.manifest.name);
                }
            }
        }
    }
    let dest = dir().join(&m.name);
    let tmp = dir().join(format!(".{}.tmp", m.name));
    let _ = fs::remove_dir_all(&tmp);
    copy_tree(&d, &tmp, 2)?;
    let _ = fs::remove_dir_all(&dest);
    fs::rename(&tmp, &dest)?;
    let mut st = State::load();
    if !st.priority.contains(&m.name) {
        // A new mode goes after the built-in modes, so the default mode (the
        // first) stays commander until the user moves it.
        if m.provides.iter().any(|p| p.kind == Kind::Mode) {
            for (b, k, _) in BUILTINS {
                if k == Kind::Mode && !st.priority.iter().any(|n| n == b) {
                    st.priority.push(b.to_string());
                }
            }
        }
        st.priority.push(m.name.clone());
        st.save()?;
    }
    Ok(Installed { manifest: m, dir: dest })
}

fn copy_tree(from: &Path, to: &Path, depth: u32) -> Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)?.flatten() {
        let p = e.path();
        let n = e.file_name();
        if n.to_string_lossy().starts_with('.') {
            continue;
        }
        let ft = e.file_type()?;
        if ft.is_dir() && depth > 0 {
            copy_tree(&p, &to.join(&n), depth - 1)?;
        } else if ft.is_file() {
            fs::copy(&p, to.join(&n)).with_context(|| format!("copying {}", p.display()))?;
        }
    }
    Ok(())
}

pub fn uninstall(name: &str) -> Result<()> {
    let Some(i) = find(name) else { bail!("no plugin named {name} (see `mouser plugin list`)") };
    fs::remove_dir_all(&i.dir)?;
    let mut st = State::load();
    st.priority.retain(|n| n != name);
    st.disabled.retain(|n| n != name);
    st.save()?;
    crate::permissions::forget(name)?;
    Ok(())
}

/// `$MOUSER_HOME/plugins/state.toml`: order and the disabled set.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct State {
    #[serde(default)]
    pub priority: Vec<String>,
    #[serde(default)]
    pub disabled: Vec<String>,
}

impl State {
    fn path() -> PathBuf {
        dir().join("state.toml")
    }

    pub fn load() -> Self {
        fs::read_to_string(Self::path()).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        fs::create_dir_all(dir())?;
        let tmp = dir().join(".state.toml.tmp");
        fs::write(&tmp, toml::to_string_pretty(self)?)?;
        fs::rename(tmp, Self::path())?;
        Ok(())
    }
}

fn known(name: &str) -> Result<()> {
    if find(name).is_none() && !BUILTINS.iter().any(|b| b.0 == name) && !is_engine(name) {
        bail!("no plugin named {name} (see `mouser plugin list`)");
    }
    Ok(())
}

/// A built-in or installed engine (engines are plugins of kind `engine`).
pub fn is_engine(name: &str) -> bool {
    crate::engines::is_builtin(name) || crate::engines::find(name).is_some()
}

/// Turn a plugin (built-in or installed, engines included) on or off. The
/// `mouser` engine is the permanent fallback and can't be turned off.
pub fn set_enabled(name: &str, on: bool) -> Result<()> {
    known(name)?;
    if !on && !can_disable(name) {
        bail!("{name} is the permanent engine (the last fallback); it can't be disabled. Move others before it with `mouser engine priority`");
    }
    let mut st = State::load();
    st.disabled.retain(|n| n != name);
    if !on {
        st.disabled.push(name.to_string());
    }
    st.save()
}

pub fn is_enabled(name: &str) -> bool {
    !State::load().disabled.iter().any(|n| n == name)
}

/// Every known plugin name in priority order (saved order first, then the rest by name).
pub fn priority() -> Vec<String> {
    let st = State::load();
    let mut all: Vec<String> = BUILTINS.iter().map(|b| b.0.to_string()).collect();
    all.extend(installed().into_iter().flatten().map(|i| i.manifest.name));
    let mut out: Vec<String> = st.priority.iter().filter(|n| all.contains(n)).cloned().collect();
    for n in all {
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

pub fn set_priority(names: &[String]) -> Result<Vec<String>> {
    for n in names {
        known(n)?;
    }
    let mut l: Vec<String> = Vec::new();
    for n in names.iter().cloned().chain(priority()) {
        if !l.contains(&n) {
            l.push(n);
        }
    }
    let mut st = State::load();
    st.priority = l.clone();
    st.save()?;
    Ok(l)
}

/// Move one plugin to `to` (0-based, clamped).
pub fn move_to(name: &str, to: usize) -> Result<Vec<String>> {
    known(name)?;
    let mut l = priority();
    l.retain(|n| n != name);
    l.insert(to.min(l.len()), name.to_string());
    set_priority(&l)
}

pub fn move_by(name: &str, d: isize) -> Result<Vec<String>> {
    let l = priority();
    let i = l.iter().position(|n| n == name).ok_or_else(|| anyhow::anyhow!("no plugin named {name}"))?;
    move_to(name, (i as isize + d).max(0) as usize)
}

/// One registry entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub version: String,
    pub description: String,
    pub kinds: Vec<Kind>,
    pub transport: Transport,
    pub enabled: bool,
    /// Installed directory (`None`: built in).
    pub dir: Option<PathBuf>,
    pub commands: Vec<String>,
    pub permissions: Vec<String>,
    /// A mode's hotkey (after `ctrl+shift+m`).
    pub hotkey: Option<String>,
}

/// Every plugin and engine, by priority; [`Registry::providers`] answers "who provides kind K".
#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub entries: Vec<Entry>,
    /// Plugin directories that failed to load: (dir name, why).
    pub broken: Vec<(String, String)>,
}

impl Registry {
    pub fn load() -> Self {
        let st = State::load();
        let on = |n: &str| !st.disabled.iter().any(|d| d == n);
        let mut entries = Vec::new();
        for (n, k, d) in BUILTINS {
            let m = builtin_manifest(n);
            entries.push(Entry {
                name: n.into(),
                version: "built-in".into(),
                description: m.as_ref().map_or(d.to_string(), |m| m.description.clone()),
                kinds: vec![k],
                transport: Transport::Builtin,
                enabled: on(n),
                dir: None,
                commands: vec![],
                permissions: m.as_ref().map(|m| m.permissions.clone()).unwrap_or_default(),
                hotkey: m.and_then(|m| m.mode).map(|m| m.hotkey),
            });
        }
        let mut broken = Vec::new();
        for i in installed() {
            match i {
                Ok(i) => entries.push(Entry {
                    name: i.manifest.name.clone(),
                    version: i.manifest.version.clone(),
                    description: i.manifest.description.clone(),
                    kinds: i.manifest.kinds(),
                    transport: i.manifest.run.transport,
                    enabled: on(&i.manifest.name),
                    dir: Some(i.dir.clone()),
                    commands: i.manifest.run.commands.clone(),
                    permissions: i.manifest.permissions.clone(),
                    hotkey: i.manifest.mode.as_ref().map(|m| m.hotkey.clone()),
                }),
                Err(b) => broken.push(b),
            }
        }
        let order = priority();
        entries.sort_by_key(|e| order.iter().position(|o| *o == e.name).unwrap_or(usize::MAX));
        // Engines have their own order (`mouser engine priority`, the saved
        // `engines` preference); listed after, in that order.
        let mut engs = Vec::new();
        for n in crate::engines::BUILTINS {
            let m = builtin_manifest(n).expect("built-in engine manifest");
            engs.push(Entry {
                name: n.into(),
                version: "built-in".into(),
                description: m.description,
                kinds: vec![Kind::Engine],
                transport: Transport::Builtin,
                enabled: !can_disable(n) || on(n),
                dir: None,
                commands: vec![],
                permissions: vec![],
                hotkey: None,
            });
        }
        for e in crate::engines::installed().into_iter().flatten() {
            let m = Manifest::from_engine(&e.manifest);
            engs.push(Entry {
                name: m.name.clone(),
                version: m.version.clone(),
                description: m.description.clone(),
                kinds: vec![Kind::Engine],
                transport: m.run.transport,
                enabled: on(&m.name),
                dir: Some(e.dir.clone()),
                commands: vec![],
                permissions: vec![],
                hotkey: None,
            });
        }
        let eorder = crate::engines::saved_priority();
        engs.sort_by_key(|e| eorder.iter().position(|o| *o == e.name).unwrap_or(usize::MAX));
        entries.extend(engs);
        Registry { entries, broken }
    }

    /// Enabled providers of `kind`, best first.
    pub fn providers(&self, kind: Kind) -> Vec<&Entry> {
        self.entries.iter().filter(|e| e.enabled && e.kinds.contains(&kind)).collect()
    }

    /// Enabled modes, best first: what the mode switcher offers.
    pub fn modes(&self) -> Vec<&Entry> {
        self.providers(Kind::Mode)
    }

    /// Enabled engines in walk order (`mouser` is always among them).
    pub fn engines(&self) -> Vec<&Entry> {
        self.providers(Kind::Engine)
    }

    /// The enabled command plugin that owns `[mouser]` command `word`.
    pub fn command(&self, word: &str) -> Option<&Entry> {
        self.providers(Kind::Command).into_iter().find(|e| e.commands.iter().any(|c| c == word))
    }
}

/// The JSON a command plugin gets on stdin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandRequest {
    pub api: u32,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// The JSON a command plugin prints on stdout.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandReply {
    /// Shown as a Mouser notice.
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Mode plugins: the dock's lines (replaces what it showed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
    /// Mode plugins: the dock title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Longest a command plugin may run.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest reply read from a command plugin.
pub const COMMAND_MAX_REPLY: u64 = 64 * 1024;

/// Split a `[mouser]` line into the command word and its arguments.
pub fn split_line(line: &str) -> (&str, Vec<String>) {
    let line = line.trim().trim_start_matches(':');
    let mut it = line.split_whitespace();
    let w = it.next().unwrap_or("");
    (w, it.map(str::to_string).collect())
}

/// Run a command plugin once. The environment is scrubbed to PATH/HOME/LANG and
/// MOUSER_PLUGIN_* (no tokens or keys leak to plugins).
pub fn run_command(p: &Installed, req: &CommandRequest) -> Result<CommandReply> {
    let argv = p.argv();
    let (prog, args) = argv.split_first().context("empty run.command")?;
    let mut c = Command::new(prog);
    c.args(args).current_dir(&p.dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    c.env_clear();
    for k in ["PATH", "HOME", "LANG", "SystemRoot", "TEMP", "TMP", "USERPROFILE"] {
        if let Some(v) = std::env::var_os(k) {
            c.env(k, v);
        }
    }
    c.env("MOUSER_PLUGIN_API", API.to_string()).env("MOUSER_PLUGIN_NAME", &p.manifest.name);
    let mut child = c.spawn().with_context(|| format!("starting {prog}"))?;
    let body = serde_json::to_vec(req)?;
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(&body);
    }
    let mut out = child.stdout.take().context("stdout")?;
    let reader = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = (&mut out).take(COMMAND_MAX_REPLY).read_to_end(&mut v);
        v
    });
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if start.elapsed() > COMMAND_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{} took longer than {}s; stopped", p.manifest.name, COMMAND_TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let v = reader.join().unwrap_or_default();
    if !status.success() {
        let mut e = String::new();
        if let Some(mut s) = child.stderr.take() {
            let _ = (&mut s).take(4096).read_to_string(&mut e);
        }
        bail!("{} exited with {status}: {}", p.manifest.name, e.trim());
    }
    let r: CommandReply = serde_json::from_slice(&v).with_context(|| format!("{} replied with invalid JSON", p.manifest.name))?;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMD: &str = r#"
name = "hello"
version = "0.1.0"
description = "says hello"
permissions = ["run", "notify"]
[[provides]]
kind = "command"
[run]
transport = "command"
command = ["./hello.sh"]
commands = ["hello"]
"#;

    #[test]
    fn manifest_parse_and_checks() {
        let m = Manifest::parse(CMD).unwrap();
        assert_eq!(m.kinds(), vec![Kind::Command]);
        assert_eq!(m.api, API);
        let bad = |from: &str, to: &str| Manifest::parse(&CMD.replace(from, to)).unwrap_err().to_string();
        assert!(Manifest::parse(&CMD.replace("\"hello\"\nversion", "\"Bad Name\"\nversion")).is_err());
        assert!(bad("[\"hello\"]", "[\"theme\"]").contains("command name") || bad("[\"hello\"]", "[\"theme\"]").contains("plugin.toml"));
        assert!(Manifest::parse(&CMD.replace("\"notify\"", "\"root\"")).is_err());
        assert!(Manifest::parse(&CMD.replace("command = [\"./hello.sh\"]\n", "")).is_err());
        assert!(Manifest::parse(&CMD.replace("kind = \"command\"", "kind = \"engine\"")).is_err());
        assert!(Manifest::parse(&CMD.replace("transport = \"command\"", "transport = \"builtin\"")).is_err());
        assert!(Manifest::parse(&CMD.replace("name = \"hello\"", "name = \"commander\"")).is_err());
        let mp =
            "name = \"m\"\nversion = \"1\"\n[[provides]]\nkind = \"model-provider\"\nid = \"gemma\"\n[run]\ntransport = \"mcp\"\ncommand = [\"x\"]\n";
        assert_eq!(Manifest::parse(mp).unwrap().kinds(), vec![Kind::ModelProvider]);
        let fz = "name = \"f\"\nversion = \"1\"\n[[provides]]\nkind = \"tool-provider\"\n[run]\ntransport = \"fuzzball\"\n";
        assert!(Manifest::parse(fz).is_ok());
        assert!(Manifest::parse(&fz.replace("\"fuzzball\"\n", "\"fuzzball\"\ncommand = [\"x\"]\n")).is_err());
        assert_eq!(split_line(":hello a  b"), ("hello", vec!["a".to_string(), "b".to_string()]));
    }

    fn plugin(root: &Path, name: &str, cmds: &[&str]) -> PathBuf {
        let d = root.join(format!("src-{name}"));
        fs::create_dir_all(&d).unwrap();
        let c: Vec<String> = cmds.iter().map(|c| format!("\"{c}\"")).collect();
        fs::write(
            d.join("plugin.toml"),
            format!("name = \"{name}\"\nversion = \"1\"\n[[provides]]\nkind = \"command\"\n[run]\ntransport = \"command\"\ncommand = [\"./run\"]\ncommands = [{}]\n", c.join(",")),
        )
        .unwrap();
        fs::write(d.join("run"), "x").unwrap();
        d
    }

    #[test]
    #[serial_test::serial]
    fn install_registry_priority_enable() {
        let t = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("MOUSER_HOME", t.path().join("home")) };
        let a = install(&plugin(t.path(), "alpha", &["hi"])).unwrap();
        assert!(a.dir.join("run").is_file());
        install(&plugin(t.path(), "beta", &["yo"])).unwrap();
        assert!(install(&plugin(t.path(), "gamma", &["hi"])).unwrap_err().to_string().contains("already provided"));
        // Installed plugins go first (in install order); built-ins follow until placed.
        assert_eq!(priority()[..3], ["alpha", "beta", "commander"]);
        let r = Registry::load();
        assert_eq!(r.command("hi").unwrap().name, "alpha");
        assert!(r.providers(Kind::Engine).iter().any(|e| e.name == "mouser"));
        assert_eq!(r.providers(Kind::Mode)[0].name, "commander");
        assert_eq!(move_to("beta", 0).unwrap()[..3], ["beta", "alpha", "commander"]);
        assert_eq!(move_to("commander", 0).unwrap()[..3], ["commander", "beta", "alpha"]);
        assert_eq!(move_by("alpha", -1).unwrap()[..3], ["commander", "alpha", "beta"]);
        set_enabled("alpha", false).unwrap();
        assert!(!is_enabled("alpha"));
        assert!(Registry::load().command("hi").is_none());
        set_enabled("alpha", true).unwrap();
        assert!(Registry::load().command("hi").is_some());
        assert!(set_enabled("nope", false).is_err());
        // P6: built-ins can be turned off, except the permanent engine.
        set_enabled("commander", false).unwrap();
        assert!(Registry::load().modes().iter().all(|e| e.name != "commander"));
        set_enabled("commander", true).unwrap();
        assert!(set_enabled("mouser", false).unwrap_err().to_string().contains("permanent"));
        // A broken manifest is listed, not fatal.
        fs::create_dir_all(dir().join("broken")).unwrap();
        fs::write(dir().join("broken/plugin.toml"), "name = 1").unwrap();
        assert_eq!(Registry::load().broken.len(), 1);
        uninstall("alpha").unwrap();
        assert!(find("alpha").is_none());
        assert!(!State::load().priority.contains(&"alpha".to_string()));
        unsafe { std::env::remove_var("MOUSER_HOME") };
    }

    #[test]
    fn builtin_manifests_are_plugin_toml() {
        for (n, _) in BUILTIN_MANIFESTS {
            let m = builtin_manifest(n).unwrap();
            assert_eq!(m.name, n);
            assert_eq!(m.run.transport, Transport::Builtin);
            assert!(m.permissions.iter().all(|c| known_capability(c)), "{n}");
        }
        assert_eq!(builtin_manifest("commander").unwrap().mode.unwrap().hotkey, "c");
        assert_eq!(builtin_manifest("chat").unwrap().mode.unwrap().hotkey, "a");
        let e = builtin_manifest("mouser").unwrap();
        assert_eq!(e.kinds(), [Kind::Engine]);
        assert_eq!(e.engine.unwrap().abi, crate::engines::ABI);
        assert!(builtin_manifest("skia").unwrap().engine.is_some());
        // A plugin can't reuse a built-in name.
        assert!(Manifest::parse(&MODE.replace("\"shout\"", "\"chat\"")).unwrap_err().to_string().contains("built-in"));
    }

    const MODE: &str = r#"
name = "shout"
version = "0.1.0"
provides = [{ kind = "mode" }]
[run]
transport = "command"
command = ["./run"]
[mode]
hotkey = "s"
keys = [{ id = "again", key = "r", describe = "ask again" }]
"#;

    #[test]
    fn mode_and_engine_manifest_checks() {
        let m = Manifest::parse(MODE).unwrap();
        assert_eq!(m.mode.as_ref().unwrap().keys[0].id, "again");
        let bad = |s: String, why: &str| {
            let e = Manifest::parse(&s).unwrap_err().to_string();
            assert!(e.contains(why), "{e}");
        };
        bad(MODE.replace("hotkey = \"s\"", "hotkey = \"c\""), "taken");
        bad(MODE.replace("hotkey = \"s\"", "hotkey = \"p\""), "taken");
        bad(MODE.replace("hotkey = \"s\"", "hotkey = \"Sx\""), "one lowercase");
        bad(MODE.replace("transport = \"command\"", "transport = \"mcp\""), "not supported yet");
        bad(MODE.replace("key = \"r\"", "key = \"ctrl+r\""), "not a single");
        bad(MODE.replace("[mode]\nhotkey = \"s\"\nkeys = [{ id = \"again\", key = \"r\", describe = \"ask again\" }]", ""), "needs a [mode]");
        let eng = r#"
name = "fancy"
version = "1.0"
description = "a test engine"
provides = [{ kind = "engine" }]
[run]
transport = "library"
[engine]
abi = 1
library = "libfancy.so"
"#;
        let m = Manifest::parse(eng).unwrap();
        let e = m.to_engine().unwrap();
        assert_eq!((e.name.as_str(), e.abi, e.library.as_str()), ("fancy", 1, "libfancy.so"));
        // engine.toml read as plugin.toml gives the same manifest back.
        assert_eq!(Manifest::from_engine(&e), Manifest { description: "a test engine".into(), ..m.clone() });
        bad(eng.replace("abi = 1", "abi = 9"), "ABI");
        bad(eng.replace("transport = \"library\"", "transport = \"command\"\ncommand = [\"x\"]"), "library");
        bad(eng.replace("libfancy.so", "../x.so"), "inside");
        bad(MODE.replace("[mode]", "[engine]\nabi = 1\nlibrary = \"x\"\n[mode]"), "only for kind");
    }

    #[test]
    #[serial_test::serial]
    fn engines_are_plugins_disable_and_migrate() {
        let t = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("MOUSER_HOME", t.path().join("home")) };
        unsafe { std::env::remove_var("MOUSER_ENGINE") };
        // Old prefs: `engine = "skia"` (pre-priority) migrates to skia > mouser,
        // and the registry lists the engines in that order.
        storage::update_preferences(|p| p.engine = Some("skia".into())).unwrap();
        assert_eq!(crate::engines::priority(), ["skia", "mouser"]);
        let names = |r: &Registry| r.engines().iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&Registry::load()), ["skia", "mouser"]);
        // An installed engine described by plugin.toml (no engine.toml).
        let d = crate::engines::dir().join("fancy");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("plugin.toml"),
            "name = \"fancy\"\nversion = \"2\"\nprovides = [{ kind = \"engine\" }]\n[run]\ntransport = \"library\"\n[engine]\nabi = 1\nlibrary = \"libfancy.so\"\n",
        )
        .unwrap();
        assert!(crate::engines::is_engine_dir(&d));
        assert_eq!(crate::engines::find("fancy").unwrap().manifest.version, "2");
        crate::engines::select("fancy").unwrap();
        assert_eq!(crate::engines::priority(), ["fancy", "skia", "mouser"]);
        let r = Registry::load();
        assert_eq!(names(&r), ["fancy", "skia", "mouser"]);
        assert_eq!(r.engines()[0].transport, Transport::Library);
        // Disabling drops an engine from the walk but keeps its place.
        set_enabled("skia", false).unwrap();
        assert_eq!(crate::engines::priority(), ["fancy", "mouser"]);
        assert!(crate::engines::select("skia").unwrap_err().to_string().contains("disabled"));
        assert_eq!(names(&Registry::load()), ["fancy", "mouser"]);
        set_enabled("skia", true).unwrap();
        assert_eq!(crate::engines::priority(), ["fancy", "skia", "mouser"]);
        // MOUSER_ENGINE still overrides for one run, as given.
        set_enabled("fancy", false).unwrap();
        unsafe { std::env::set_var("MOUSER_ENGINE", "fancy") };
        assert_eq!(crate::engines::priority(), ["fancy", "mouser"]);
        unsafe { std::env::remove_var("MOUSER_ENGINE") };
        assert!(set_enabled("mouser", false).is_err());
        // Every mode off: no modes (Mouser falls back to the plain terminal).
        set_enabled("commander", false).unwrap();
        set_enabled("chat", false).unwrap();
        assert!(Registry::load().modes().is_empty());
        unsafe { std::env::remove_var("MOUSER_HOME") };
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn command_plugin_json_roundtrip_timeout_and_env() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("MOUSER_HOME", t.path().join("home")) };
        let d = plugin(t.path(), "echo", &["echo"]);
        fs::write(
            d.join("run"),
            "#!/bin/sh\nread -r line\ncase \"$line\" in *slow*) sleep 30;; esac\nprintf '{\"text\":\"got %s secret=%s\"}' \"$MOUSER_PLUGIN_NAME\" \"${SECRET_TOKEN:-none}\"\n",
        )
        .unwrap();
        fs::set_permissions(d.join("run"), fs::Permissions::from_mode(0o755)).unwrap();
        let p = install(&d).unwrap();
        unsafe { std::env::set_var("SECRET_TOKEN", "leak") };
        let r = run_command(&p, &CommandRequest { api: API, command: "echo".into(), args: vec!["a".into()], cwd: None }).unwrap();
        unsafe { std::env::remove_var("SECRET_TOKEN") };
        assert_eq!(r.text, "got echo secret=none");
        let start = Instant::now();
        let e = run_command(&p, &CommandRequest { api: API, command: "echo".into(), args: vec!["slow".into()], cwd: None }).unwrap_err();
        assert!(e.to_string().contains("longer than"), "{e}");
        assert!(start.elapsed() < COMMAND_TIMEOUT + Duration::from_secs(3));
        unsafe { std::env::remove_var("MOUSER_HOME") };
    }
}
