//! File operations for Mouser's MCP tools (the commander's ops, without UI):
//! list, read, mkdir, copy, move, and trash (into $MOUSER_HOME/trash; nothing is
//! deleted for good).

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

pub const MAX_READ: u64 = 1 << 20;
pub const MAX_LIST: usize = 5000;

pub fn list(dir: &Path, hidden: bool) -> Result<Value> {
    let mut v = Vec::new();
    for e in fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !hidden && name.starts_with('.') {
            continue;
        }
        let m = e.metadata().ok();
        let kind = match e.file_type() {
            Ok(t) if t.is_dir() => "dir",
            Ok(t) if t.is_symlink() => "link",
            _ => "file",
        };
        let mtime = m.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs());
        v.push(json!({"name": name, "kind": kind, "size": m.map(|m| m.len()).unwrap_or(0), "mtime": mtime}));
        if v.len() >= MAX_LIST {
            break;
        }
    }
    v.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(json!({"path": dir.display().to_string(), "entries": v}))
}

/// Up to `max` bytes (capped at [`MAX_READ`]) as text (lossy UTF-8).
pub fn read(p: &Path, max: Option<u64>) -> Result<Value> {
    use std::io::Read;
    let max = max.unwrap_or(MAX_READ).min(MAX_READ);
    let f = fs::File::open(p).with_context(|| format!("opening {}", p.display()))?;
    let size = f.metadata()?.len();
    let mut b = Vec::new();
    f.take(max).read_to_end(&mut b)?;
    Ok(json!({"path": p.display().to_string(), "size": size, "truncated": size > b.len() as u64, "text": String::from_utf8_lossy(&b)}))
}

pub fn mkdir(p: &Path) -> Result<()> {
    fs::create_dir_all(p).with_context(|| format!("creating {}", p.display()))
}

fn no_clobber(to: &Path) -> Result<()> {
    if to.symlink_metadata().is_ok() {
        bail!("{} already exists", to.display());
    }
    Ok(())
}

pub fn copy(from: &Path, to: &Path) -> Result<()> {
    no_clobber(to)?;
    if from.is_dir() {
        copy_dir(from, to)
    } else {
        fs::copy(from, to).with_context(|| format!("copying {}", from.display()))?;
        Ok(())
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)?.flatten() {
        let (s, d) = (e.path(), to.join(e.file_name()));
        if e.file_type()?.is_dir() { copy_dir(&s, &d)? } else { fs::copy(&s, &d).map(|_| ())? }
    }
    Ok(())
}

pub fn rename(from: &Path, to: &Path) -> Result<()> {
    no_clobber(to)?;
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        // Across filesystems: copy, then remove.
        Err(_) => {
            copy(from, to)?;
            if from.is_dir() {
                fs::remove_dir_all(from)?
            } else {
                fs::remove_file(from)?
            }
            Ok(())
        }
    }
}

/// Move into $MOUSER_HOME/trash/<unix time>-<name>; returns where it went.
pub fn trash(p: &Path) -> Result<PathBuf> {
    let name = p.file_name().context("no file name")?.to_string_lossy().into_owned();
    let d = crate::storage::resolve_home().join("trash");
    fs::create_dir_all(&d)?;
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let to = d.join(format!("{t}-{name}"));
    rename(p, &to)?;
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn ops_roundtrip() {
        let t = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("MOUSER_HOME", t.path().join("home")) };
        let r = t.path().join("w");
        mkdir(&r.join("a/b")).unwrap();
        fs::write(r.join("a/f.txt"), "hello").unwrap();
        fs::write(r.join(".h"), "").unwrap();
        let l = list(&r, false).unwrap();
        assert_eq!(l["entries"].as_array().unwrap().len(), 1);
        assert_eq!(list(&r, true).unwrap()["entries"].as_array().unwrap().len(), 2);
        assert_eq!(read(&r.join("a/f.txt"), Some(3)).unwrap()["text"], "hel");
        copy(&r.join("a"), &r.join("c")).unwrap();
        assert!(r.join("c/b").is_dir() && r.join("c/f.txt").is_file());
        assert!(copy(&r.join("a"), &r.join("c")).is_err(), "no clobber");
        rename(&r.join("c/f.txt"), &r.join("g.txt")).unwrap();
        let to = trash(&r.join("g.txt")).unwrap();
        assert!(!r.join("g.txt").exists() && to.is_file() && to.starts_with(t.path().join("home/trash")));
        unsafe { std::env::remove_var("MOUSER_HOME") };
    }
}
