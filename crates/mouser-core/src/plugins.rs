//! Plugins (docs/plugins.md): the `plugin.toml` manifest, the plugins
//! directory, enable/disable and priority state, the registry by kind, and the
//! command transport (an executable behind `[mouser]` line commands).
//!
//! Engines keep `engine.toml` and their own loader (`crate::engines`); the
//! registry lists them as `engine` providers so P6 can move them here.

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

/// How Mouser reaches a plugin. No native libraries in v1 (engines keep their ABI).
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
}

impl Transport {
    pub fn name(self) -> &'static str {
        match self {
            Transport::Builtin => "builtin",
            Transport::Mcp => "mcp",
            Transport::Fuzzball => "fuzzball",
            Transport::Command => "command",
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
    CAPABILITIES.iter().find(|(c, _)| *c == cap).map_or("(unknown capability)", |(_, t)| t)
}

pub fn known_capability(cap: &str) -> bool {
    CAPABILITIES.iter().any(|(c, _)| *c == cap)
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
        if self.provides.iter().any(|p| p.kind == Kind::Engine) {
            bail!("engines are installed with engine.toml (`mouser engine install`)");
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
            _ => {}
        }
        let has_cmd_kind = self.provides.iter().any(|p| p.kind == Kind::Command);
        if self.run.transport == Transport::Command {
            if self.run.commands.is_empty() || !has_cmd_kind {
                bail!("a command plugin lists run.commands and provides kind = \"command\"");
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

    pub fn kinds(&self) -> Vec<Kind> {
        let s: BTreeSet<Kind> = self.provides.iter().map(|p| p.kind).collect();
        s.into_iter().collect()
    }
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

/// Built-in providers, listed so the registry already has the P6 shape.
pub const BUILTINS: [(&str, Kind, &str); 1] = [("commander", Kind::Mode, "file commander mode (built in)")];

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
    if find(name).is_none() && !BUILTINS.iter().any(|b| b.0 == name) {
        bail!("no plugin named {name} (see `mouser plugin list`)");
    }
    Ok(())
}

pub fn set_enabled(name: &str, on: bool) -> Result<()> {
    known(name)?;
    if BUILTINS.iter().any(|b| b.0 == name) {
        bail!("{name} is built in; turning built-ins off comes with P6 (for modes: `esc :c` switches commander off)");
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
            entries.push(Entry {
                name: n.into(),
                version: "built-in".into(),
                description: d.into(),
                kinds: vec![k],
                transport: Transport::Builtin,
                enabled: on(n),
                dir: None,
                commands: vec![],
                permissions: vec![],
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
                }),
                Err(b) => broken.push(b),
            }
        }
        let order = priority();
        entries.sort_by_key(|e| order.iter().position(|o| *o == e.name).unwrap_or(usize::MAX));
        // Engines have their own order (`mouser engine priority`); listed after.
        for n in crate::engines::BUILTINS {
            entries.push(Entry {
                name: n.into(),
                version: "built-in".into(),
                description: "rendering engine (built in)".into(),
                kinds: vec![Kind::Engine],
                transport: Transport::Builtin,
                enabled: true,
                dir: None,
                commands: vec![],
                permissions: vec![],
            });
        }
        for e in crate::engines::installed().into_iter().flatten() {
            entries.push(Entry {
                name: e.manifest.name.clone(),
                version: e.manifest.version.clone(),
                description: e.manifest.description.clone(),
                kinds: vec![Kind::Engine],
                transport: Transport::Builtin,
                enabled: true,
                dir: Some(e.dir.clone()),
                commands: vec![],
                permissions: vec![],
            });
        }
        Registry { entries, broken }
    }

    /// Enabled providers of `kind`, best first.
    pub fn providers(&self, kind: Kind) -> Vec<&Entry> {
        self.entries.iter().filter(|e| e.enabled && e.kinds.contains(&kind)).collect()
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
        assert_eq!(priority(), vec!["alpha", "beta", "commander"]);
        let r = Registry::load();
        assert_eq!(r.command("hi").unwrap().name, "alpha");
        assert!(r.providers(Kind::Engine).iter().any(|e| e.name == "mouser"));
        assert_eq!(r.providers(Kind::Mode)[0].name, "commander");
        assert_eq!(move_to("beta", 0).unwrap(), vec!["beta", "alpha", "commander"]);
        assert_eq!(move_to("commander", 0).unwrap(), vec!["commander", "beta", "alpha"]);
        assert_eq!(move_by("alpha", -1).unwrap(), vec!["commander", "alpha", "beta"]);
        set_enabled("alpha", false).unwrap();
        assert!(!is_enabled("alpha"));
        assert!(Registry::load().command("hi").is_none());
        set_enabled("alpha", true).unwrap();
        assert!(Registry::load().command("hi").is_some());
        assert!(set_enabled("nope", false).is_err());
        assert!(set_enabled("commander", false).is_err());
        // A broken manifest is listed, not fatal.
        fs::create_dir_all(dir().join("broken")).unwrap();
        fs::write(dir().join("broken/plugin.toml"), "name = 1").unwrap();
        assert_eq!(Registry::load().broken.len(), 1);
        uninstall("alpha").unwrap();
        assert!(find("alpha").is_none());
        assert!(!State::load().priority.contains(&"alpha".to_string()));
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
