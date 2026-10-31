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
    Ok(Installed { manifest: m.clone(), dir: dest })
}

/// Remove an installed engine; switches the preference back to the default
/// if it was selected. Returns whether it was selected.
pub fn uninstall(name: &str) -> Result<bool> {
    if is_builtin(name) {
        bail!("{name} is built in and cannot be uninstalled");
    }
    let inst = find(name).with_context(|| format!("no installed engine named {name}"))?;
    fs::remove_dir_all(&inst.dir)?;
    clear_sentinel(name);
    let was = storage::load_preferences().engine.as_deref() == Some(name);
    if was {
        storage::update_preferences(|p| p.engine = None)?;
    }
    Ok(was)
}

/// Select an engine for new windows.
pub fn select(name: &str) -> Result<()> {
    if !is_builtin(name) && find(name).is_none() {
        bail!("no engine named {name} (see `mouser engine list`)");
    }
    clear_sentinel(name);
    storage::update_preferences(|p| p.engine = if name == DEFAULT { None } else { Some(name.to_string()) })?;
    Ok(())
}

/// The engine to use: `MOUSER_ENGINE`, else the preference, else `mouser`.
pub fn chosen() -> String {
    if let Ok(v) = std::env::var("MOUSER_ENGINE")
        && !v.trim().is_empty()
    {
        return v.trim().to_string();
    }
    storage::load_preferences().engine.unwrap_or_else(|| DEFAULT.to_string())
}

fn sentinel(name: &str) -> PathBuf {
    dir().join(format!(".starting-{name}"))
}

/// Mark an engine as starting; if Mouser dies before [`clear_sentinel`], the
/// next start skips it.
pub fn set_sentinel(name: &str) {
    let _ = fs::create_dir_all(dir());
    let _ = fs::write(sentinel(name), std::process::id().to_string());
}

pub fn clear_sentinel(name: &str) {
    let _ = fs::remove_file(sentinel(name));
}

/// True if the engine crashed Mouser last time it started.
pub fn crashed_last_time(name: &str) -> bool {
    sentinel(name).exists()
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
}
