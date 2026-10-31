//! Installed GPU rendering engines: manifests, the engines directory, the
//! chosen engine and the crash sentinel. Loading libraries lives in
//! `mouser-render`; see docs/engines.md.

use crate::storage;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// The default engine (wgpu).
pub const DEFAULT: &str = "mouser";
/// ABI version this build loads.
pub const ABI: u32 = 1;
/// Built-in engine names (whether this build has Skia is up to `mouser-render`).
pub const BUILTINS: [&str; 2] = ["mouser", "skia"];

pub fn is_builtin(name: &str) -> bool {
    BUILTINS.contains(&name)
}

/// `engine.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub abi: u32,
    pub library: String,
    #[serde(default)]
    pub description: String,
}

impl Manifest {
    pub fn parse(s: &str) -> Result<Self> {
        let m: Manifest = toml::from_str(s).context("engine.toml")?;
        m.check()?;
        Ok(m)
    }

    pub fn check(&self) -> Result<()> {
        if !valid_name(&self.name) {
            bail!("invalid engine name {:?} (use a-z, 0-9, - and _)", self.name);
        }
        if is_builtin(&self.name) {
            bail!("{:?} is a built-in engine name", self.name);
        }
        if self.abi != ABI {
            bail!("engine {} needs ABI {}, this Mouser supports ABI {ABI}", self.name, self.abi);
        }
        if self.library.is_empty() || self.library.contains('/') || self.library.contains('\\') || self.library.starts_with('.') {
            bail!("library must be a plain file name next to engine.toml");
        }
        Ok(())
    }
}

pub fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

pub fn dir() -> PathBuf {
    storage::resolve_home().join("engines")
}

/// An installed engine.
#[derive(Debug, Clone)]
pub struct Installed {
    pub manifest: Manifest,
    pub dir: PathBuf,
}

impl Installed {
    pub fn library(&self) -> PathBuf {
        self.dir.join(&self.manifest.library)
    }
}

/// Installed engines (bad ones as errors), sorted by directory name.
pub fn installed() -> Vec<std::result::Result<Installed, (String, String)>> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir()) else { return out };
    let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for d in dirs {
        let name = d.file_name().unwrap_or_default().to_string_lossy().into_owned();
        out.push(read_dir_manifest(&d).map(|manifest| Installed { manifest, dir: d.clone() }).map_err(|e| (name, format!("{e:#}"))));
    }
    out
}

fn read_dir_manifest(d: &Path) -> Result<Manifest> {
    let s = fs::read_to_string(d.join("engine.toml")).with_context(|| format!("reading {}", d.join("engine.toml").display()))?;
    Manifest::parse(&s)
}

pub fn find(name: &str) -> Option<Installed> {
    installed().into_iter().flatten().find(|i| i.manifest.name == name)
}

/// What `install` was pointed at: a directory with `engine.toml`, the
/// manifest itself, or a library with `engine.toml` beside it.
pub fn source(path: &Path) -> Result<(Manifest, PathBuf)> {
    let d = if path.is_dir() { path.to_path_buf() } else { path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(".")) };
    let m = read_dir_manifest(&d)?;
    let lib = d.join(&m.library);
    if !lib.is_file() {
        bail!("{} not found (build the engine and put the library next to engine.toml)", lib.display());
    }
    Ok((m, lib))
}

/// Copy an engine into the engines directory (replacing an older install).
pub fn install(m: &Manifest, lib: &Path) -> Result<Installed> {
    m.check()?;
    let dest = dir().join(&m.name);
    let tmp = dir().join(format!(".{}.tmp", m.name));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp)?;
    fs::copy(lib, tmp.join(&m.library)).with_context(|| format!("copying {}", lib.display()))?;
    fs::write(tmp.join("engine.toml"), toml::to_string_pretty(m)?)?;
    let _ = fs::remove_dir_all(&dest);
    fs::rename(&tmp, &dest)?;
    clear_sentinel(&m.name);
    clear_failure(&m.name);
    Ok(Installed { manifest: m.clone(), dir: dest })
}

/// Remove an installed engine (and drop it from the priority list). Returns
/// whether it was first in the list.
pub fn uninstall(name: &str) -> Result<bool> {
    if name == DEFAULT {
        bail!("mouser is the built-in fallback engine and can never be removed (reorder with `mouser engine priority`)");
    }
    if is_builtin(name) {
        bail!("{name} is built in and cannot be uninstalled (move it down with `mouser engine move {name} last`)");
    }
    let inst = find(name).with_context(|| format!("no installed engine named {name}"))?;
    fs::remove_dir_all(&inst.dir)?;
    clear_sentinel(name);
    clear_failure(name);
    let saved = saved_priority();
    let was = saved.first().map(String::as_str) == Some(name);
    if saved.iter().any(|n| n == name) {
        save_priority(saved.into_iter().filter(|n| n != name).collect())?;
    }
    Ok(was)
}

/// `mouser` is always present; names are de-duplicated (first wins).
pub fn normalize(list: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in list {
        let n = n.trim().to_string();
        if !n.is_empty() && !out.contains(&n) {
            out.push(n);
        }
    }
    if !out.iter().any(|n| n == DEFAULT) {
        out.push(DEFAULT.to_string());
    }
    out
}

/// The saved order: `engines`, else the old single `engine` key, else `[mouser]`.
pub fn saved_priority() -> Vec<String> {
    let p = storage::load_preferences();
    normalize(p.engines.or_else(|| p.engine.map(|e| vec![e])).unwrap_or_default())
}

/// The order this run walks: `MOUSER_ENGINE=a,b,c` overrides the saved one.
pub fn priority() -> Vec<String> {
    if let Ok(v) = std::env::var("MOUSER_ENGINE")
        && !v.trim().is_empty()
    {
        return normalize(v.split(',').map(str::to_string));
    }
    saved_priority()
}

fn save_priority(list: Vec<String>) -> Result<()> {
    let list = normalize(list);
    storage::update_preferences(|p| {
        p.engine = None;
        p.engines = if list == [DEFAULT] { None } else { Some(list.clone()) };
    })?;
    Ok(())
}

fn known(name: &str) -> Result<()> {
    if !is_builtin(name) && find(name).is_none() {
        bail!("no engine named {name} (see `mouser engine list`)");
    }
    Ok(())
}

/// Set the whole order (`mouser` is appended if left out).
pub fn set_priority(names: &[String]) -> Result<Vec<String>> {
    for n in names {
        known(n)?;
    }
    save_priority(names.to_vec())?;
    Ok(saved_priority())
}

/// Move `name` to index `to` (0 = first; clamped), adding it if absent.
pub fn move_to(name: &str, to: usize) -> Result<Vec<String>> {
    known(name)?;
    let mut l: Vec<String> = saved_priority().into_iter().filter(|n| n != name).collect();
    l.insert(to.min(l.len()), name.to_string());
    save_priority(l)?;
    Ok(saved_priority())
}

/// Move one step (`-1` up, `1` down).
pub fn move_by(name: &str, delta: isize) -> Result<Vec<String>> {
    known(name)?;
    let l = saved_priority();
    let i = l.iter().position(|n| n == name).unwrap_or(l.len()) as isize;
    move_to(name, (i + delta).max(0) as usize)
}

/// Put an engine first (and unskip it).
pub fn select(name: &str) -> Result<()> {
    known(name)?;
    clear_sentinel(name);
    clear_failure(name);
    move_to(name, 0)?;
    Ok(())
}

/// The engine new windows try first.
pub fn chosen() -> String {
    priority().into_iter().next().unwrap_or_else(|| DEFAULT.to_string())
}

fn sentinel(name: &str) -> PathBuf {
    dir().join(format!(".starting-{name}"))
}

fn failure_path(name: &str) -> PathBuf {
    dir().join(format!(".failed-{name}"))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Mark an engine as starting; if Mouser dies before [`clear_sentinel`], the
/// next start skips it.
pub fn set_sentinel(name: &str) {
    if name == DEFAULT {
        return; // mouser is never skipped: it is the last GPU fallback.
    }
    let _ = fs::create_dir_all(dir());
    let _ = fs::write(sentinel(name), format!("pid={}\nstarted={}\n", std::process::id(), now_secs()));
}

/// Mark an engine skipped now, with why (an isolated engine crashed, or a
/// built-in one panicked); Mouser keeps running on the next engine.
pub fn mark_skipped(name: &str, reason: &str) {
    if name == DEFAULT {
        return;
    }
    let _ = fs::create_dir_all(dir());
    let _ = fs::write(sentinel(name), format!("started={}\nreason={}\n", now_secs(), reason.replace('\n', " ")));
}

/// Why an engine is skipped, if known (default: it crashed Mouser while starting).
pub fn skip_reason(name: &str) -> Option<String> {
    fs::read_to_string(sentinel(name)).ok()?.lines().find_map(|l| l.strip_prefix("reason=").map(str::to_string))
}

pub fn clear_sentinel(name: &str) {
    let _ = fs::remove_file(sentinel(name));
}

/// True if the engine crashed Mouser last time it started (it is skipped).
pub fn crashed_last_time(name: &str) -> bool {
    sentinel(name).exists()
}

/// Unix time the skipped engine was last started (the start that crashed).
pub fn skipped_since(name: &str) -> Option<u64> {
    let s = fs::read_to_string(sentinel(name)).ok()?;
    s.lines()
        .find_map(|l| l.strip_prefix("started="))
        .and_then(|v| v.trim().parse().ok())
        .or_else(|| fs::metadata(sentinel(name)).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs()))
}

/// Remember why an engine could not be used (load, start or draw error). It
/// is only shown (`mouser engine list`); Mouser still tries the engine on the
/// next start.
pub fn record_failure(name: &str, reason: &str) {
    let _ = fs::create_dir_all(dir());
    let one_line = reason.replace('\n', " ");
    let _ = fs::write(failure_path(name), format!("at={}\nreason={one_line}\n", now_secs()));
}

/// The last recorded failure: (unix time, reason).
pub fn last_failure(name: &str) -> Option<(u64, String)> {
    let s = fs::read_to_string(failure_path(name)).ok()?;
    let at = s.lines().find_map(|l| l.strip_prefix("at=")).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    let reason = s.lines().find_map(|l| l.strip_prefix("reason=")).unwrap_or("").to_string();
    Some((at, reason))
}

pub fn clear_failure(name: &str) {
    let _ = fs::remove_file(failure_path(name));
}

/// An engine's state for `mouser engine list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Ok,
    /// Crashed (Mouser while starting, or its engine host) at this unix time;
    /// skipped until unskipped.
    Skipped {
        since: Option<u64>,
        reason: Option<String>,
    },
    /// Last attempt failed (unix time, reason); still tried on each start.
    Failed {
        at: u64,
        reason: String,
    },
}

pub fn state(name: &str) -> State {
    if crashed_last_time(name) {
        State::Skipped { since: skipped_since(name), reason: skip_reason(name) }
    } else if let Some((at, reason)) = last_failure(name) {
        State::Failed { at, reason }
    } else {
        State::Ok
    }
}

/// Clear an engine's skip marker and failure record without selecting it.
/// Returns whether there was anything to clear.
pub fn unskip(name: &str) -> Result<bool> {
    if !is_builtin(name) && find(name).is_none() {
        bail!("no engine named {name} (see `mouser engine list`)");
    }
    let had = state(name) != State::Ok;
    clear_sentinel(name);
    clear_failure(name);
    Ok(had)
}

/// Unskip every engine; returns the names that were restored.
pub fn unskip_all() -> Vec<String> {
    let mut names: Vec<String> = BUILTINS.iter().map(|s| s.to_string()).collect();
    names.extend(installed().into_iter().flatten().map(|i| i.manifest.name));
    names.into_iter().filter(|n| unskip(n).unwrap_or(false)).collect()
}

/// "5m ago" style age of a unix time.
pub fn ago(t: u64) -> String {
    let d = now_secs().saturating_sub(t);
    match d {
        0..60 => format!("{d}s ago"),
        60..3600 => format!("{}m ago", d / 60),
        3600..86400 => format!("{}h {}m ago", d / 3600, d % 3600 / 60),
        _ => format!("{}d ago", d / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn home() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        // SAFETY: tests touching MOUSER_HOME are serial.
        unsafe {
            std::env::set_var("MOUSER_HOME", t.path());
            std::env::remove_var("MOUSER_ENGINE");
        }
        t
    }

    fn fake_engine(root: &Path, name: &str, abi: u32) -> PathBuf {
        let d = root.join(format!("src-{name}"));
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("engine.toml"), format!("name = \"{name}\"\nversion = \"1.0\"\nabi = {abi}\nlibrary = \"lib{name}.so\"\n")).unwrap();
        fs::write(d.join(format!("lib{name}.so")), b"not really").unwrap();
        d
    }

    #[test]
    fn manifest_checks() {
        assert!(Manifest::parse("name='x'\nversion='1'\nabi=1\nlibrary='libx.so'").is_ok());
        assert!(Manifest::parse("name='X Y'\nversion='1'\nabi=1\nlibrary='l.so'").is_err());
        assert!(Manifest::parse("name='skia'\nversion='1'\nabi=1\nlibrary='l.so'").is_err());
        assert!(Manifest::parse("name='x'\nversion='1'\nabi=2\nlibrary='l.so'").unwrap_err().to_string().contains("ABI"));
        assert!(Manifest::parse("name='x'\nversion='1'\nabi=1\nlibrary='../l.so'").is_err());
    }

    #[test]
    #[serial]
    fn install_list_use_uninstall() {
        let t = home();
        assert_eq!(chosen(), "mouser");
        let src = fake_engine(t.path(), "glow", 1);
        let (m, lib) = source(&src).unwrap();
        install(&m, &lib).unwrap();
        assert!(dir().join("glow/libglow.so").is_file());
        assert_eq!(installed().len(), 1);
        select("glow").unwrap();
        assert_eq!(chosen(), "glow");
        // SAFETY: serial test.
        unsafe { std::env::set_var("MOUSER_ENGINE", "skia") };
        assert_eq!(chosen(), "skia");
        unsafe { std::env::remove_var("MOUSER_ENGINE") };
        assert!(select("nope").is_err());
        assert!(uninstall("mouser").is_err());
        assert!(uninstall("glow").unwrap());
        assert_eq!(chosen(), "mouser");
        assert!(uninstall("mouser").unwrap_err().to_string().contains("never be removed"));
        assert!(find("glow").is_none());
    }

    #[test]
    #[serial]
    fn source_needs_library_and_broken_installs_are_reported() {
        let t = home();
        let src = fake_engine(t.path(), "gone", 1);
        fs::remove_file(src.join("libgone.so")).unwrap();
        assert!(source(&src).is_err());
        fs::create_dir_all(dir().join("broken")).unwrap();
        fs::write(dir().join("broken/engine.toml"), "name = 1").unwrap();
        let l = installed();
        assert!(matches!(&l[0], Err((n, _)) if n == "broken"));
    }

    #[test]
    #[serial]
    fn sentinel_round_trip() {
        let _t = home();
        assert!(!crashed_last_time("glow"));
        set_sentinel("glow");
        assert!(crashed_last_time("glow"));
        clear_sentinel("glow");
        assert!(!crashed_last_time("glow"));
    }

    #[test]
    #[serial]
    fn priority_order_compat_and_mouser_always_present() {
        let t = home();
        let src = fake_engine(t.path(), "glow", 1);
        let (m, lib) = source(&src).unwrap();
        install(&m, &lib).unwrap();
        assert_eq!(priority(), ["mouser"]);
        // Old single key is a one-item list (+ mouser).
        storage::update_preferences(|p| p.engine = Some("skia".into())).unwrap();
        assert_eq!(priority(), ["skia", "mouser"]);
        assert_eq!(set_priority(&["glow".into(), "skia".into()]).unwrap(), ["glow", "skia", "mouser"]);
        assert_eq!(storage::load_preferences().engine, None, "old key replaced");
        assert_eq!(set_priority(&["mouser".into(), "glow".into()]).unwrap(), ["mouser", "glow"], "mouser may move up");
        assert!(set_priority(&["nope".into()]).is_err());
        assert_eq!(move_to("skia", 1).unwrap(), ["mouser", "skia", "glow"]);
        assert_eq!(move_by("glow", -1).unwrap(), ["mouser", "glow", "skia"]);
        assert_eq!(move_by("mouser", 1).unwrap(), ["glow", "mouser", "skia"]);
        assert_eq!(move_to("mouser", 99).unwrap(), ["glow", "skia", "mouser"]);
        select("skia").unwrap();
        assert_eq!(saved_priority(), ["skia", "glow", "mouser"]);
        // SAFETY: serial test.
        unsafe { std::env::set_var("MOUSER_ENGINE", "glow, skia,glow") };
        assert_eq!(priority(), ["glow", "skia", "mouser"]);
        unsafe { std::env::remove_var("MOUSER_ENGINE") };
        assert!(!uninstall("glow").unwrap());
        assert_eq!(saved_priority(), ["skia", "mouser"]);
        // mouser can't be skipped.
        set_sentinel("mouser");
        assert_eq!(state("mouser"), State::Ok);
    }

    #[test]
    #[serial]
    fn states_and_unskip() {
        let t = home();
        let src = fake_engine(t.path(), "glow", 1);
        let (m, lib) = source(&src).unwrap();
        install(&m, &lib).unwrap();
        assert_eq!(state("glow"), State::Ok);
        assert!(!unskip("glow").unwrap(), "nothing to clear");
        assert!(unskip("nope").is_err());
        set_sentinel("glow");
        let State::Skipped { since: Some(at), .. } = state("glow") else { panic!("{:?}", state("glow")) };
        assert!(ago(at).ends_with("s ago"));
        assert!(unskip("glow").unwrap());
        assert_eq!(state("glow"), State::Ok);
        assert_eq!(chosen(), "mouser", "unskip does not select");
        mark_skipped("glow", "engine host crashed (signal 6)");
        assert!(matches!(state("glow"), State::Skipped { reason: Some(r), .. } if r.contains("signal 6")));
        unskip("glow").unwrap();
        record_failure("glow", "create failed:\nno GL");
        assert_eq!(state("glow"), State::Failed { at: last_failure("glow").unwrap().0, reason: "create failed: no GL".into() });
        set_sentinel("skia");
        assert_eq!(unskip_all(), vec!["skia".to_string(), "glow".to_string()]);
        assert_eq!(state("glow"), State::Ok);
        record_failure("glow", "x");
        select("glow").unwrap();
        assert_eq!(state("glow"), State::Ok, "use clears it too");
    }
}
