//! File changes from the context menu: copy, move, rename, delete, a new
//! folder, paste. Each is described first and runs only after the user
//! said yes on Mouser's approval card; none ever replaces an existing file.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Copy into a folder (or to a new name when there is one item).
    CopyTo {
        from: Vec<PathBuf>,
        to: PathBuf,
    },
    MoveTo {
        from: Vec<PathBuf>,
        to: PathBuf,
    },
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    Delete(Vec<PathBuf>),
    NewFolder(PathBuf),
}

fn names(v: &[PathBuf]) -> String {
    let mut s: Vec<String> = v.iter().take(5).map(|p| p.display().to_string()).collect();
    if v.len() > 5 {
        s.push(format!("… and {} more", v.len() - 5));
    }
    s.join("\n")
}

/// Where `p` lands in `to`: inside it when it is a folder (or several items
/// go), else `to` itself.
fn target(p: &Path, to: &Path, many: bool) -> PathBuf {
    if many || to.is_dir() { to.join(p.file_name().unwrap_or_default()) } else { to.to_path_buf() }
}

impl Op {
    /// The approval card's title and detail.
    pub fn describe(&self) -> (String, String) {
        match self {
            Op::CopyTo { from, to } => (format!("Copy {} item(s)?", from.len()), format!("{}\n→ {}", names(from), to.display())),
            Op::MoveTo { from, to } => (format!("Move {} item(s)?", from.len()), format!("{}\n→ {}", names(from), to.display())),
            Op::Rename { from, to } => ("Rename?".into(), format!("{}\n→ {}", from.display(), to.display())),
            Op::Delete(v) => (
                format!("Delete {} item(s) permanently?", v.len()),
                format!("{}\nFolders are deleted with everything in them. This cannot be undone.", names(v)),
            ),
            Op::NewFolder(p) => ("Create a folder?".into(), p.display().to_string()),
        }
    }

    /// Carry it out (after the user said yes): what happened, for the status bar.
    pub fn run(&self) -> Result<String, String> {
        let e = |p: &Path, err: std::io::Error| format!("{}: {err}", p.display());
        match self {
            Op::CopyTo { from, to } | Op::MoveTo { from, to } => {
                let many = from.len() > 1;
                if many && !to.is_dir() {
                    return Err(format!("{} is not a folder", to.display()));
                }
                let moving = matches!(self, Op::MoveTo { .. });
                for p in from {
                    let t = target(p, to, many);
                    if t.symlink_metadata().is_ok() {
                        return Err(format!("{} already exists", t.display()));
                    }
                    if t.starts_with(p) {
                        return Err(format!("{} is inside {}", t.display(), p.display()));
                    }
                    if moving {
                        if std::fs::rename(p, &t).is_err() {
                            // Another disk: copy, then remove the original.
                            copy_tree(p, &t).map_err(|err| e(p, err))?;
                            remove(p).map_err(|err| e(p, err))?;
                        }
                    } else {
                        copy_tree(p, &t).map_err(|err| e(p, err))?;
                    }
                }
                Ok(format!("{} {} item(s) to {}", if moving { "moved" } else { "copied" }, from.len(), to.display()))
            }
            Op::Rename { from, to } => {
                if to.symlink_metadata().is_ok() {
                    return Err(format!("{} already exists", to.display()));
                }
                std::fs::rename(from, to).map_err(|err| e(from, err))?;
                Ok(format!("renamed to {}", to.display()))
            }
            Op::Delete(v) => {
                for p in v {
                    remove(p).map_err(|err| e(p, err))?;
                }
                Ok(format!("deleted {} item(s)", v.len()))
            }
            Op::NewFolder(p) => {
                std::fs::create_dir(p).map_err(|err| e(p, err))?;
                Ok(format!("created {}", p.display()))
            }
        }
    }
}

fn remove(p: &Path) -> std::io::Result<()> {
    if p.symlink_metadata()?.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) }
}

/// Copy a file, or a folder with everything in it (links are copied as
/// links, never followed).
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let m = from.symlink_metadata()?;
    if m.file_type().is_symlink() {
        #[cfg(unix)]
        return std::os::unix::fs::symlink(std::fs::read_link(from)?, to);
        #[cfg(not(unix))]
        return std::fs::copy(from, to).map(|_| ());
    }
    if m.is_dir() {
        std::fs::create_dir(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_tree(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// A name typed for rename / new folder: one path component, no `..`.
pub fn valid_name(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty() && s != "." && s != ".." && !s.contains('/') && !s.contains('\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_move_rename_delete_never_overwrite() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        std::fs::write(d.join("a.txt"), "a").unwrap();
        std::fs::create_dir(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/b.txt"), "b").unwrap();
        std::fs::create_dir(d.join("dst")).unwrap();
        Op::CopyTo { from: vec![d.join("a.txt"), d.join("sub")], to: d.join("dst") }.run().unwrap();
        assert_eq!(std::fs::read_to_string(d.join("dst/sub/b.txt")).unwrap(), "b");
        assert!(d.join("a.txt").exists(), "copy keeps the original");
        let again = Op::CopyTo { from: vec![d.join("a.txt")], to: d.join("dst") }.run();
        assert!(again.unwrap_err().contains("already exists"));
        Op::Rename { from: d.join("a.txt"), to: d.join("c.txt") }.run().unwrap();
        assert!(Op::Rename { from: d.join("c.txt"), to: d.join("dst") }.run().is_err(), "rename never replaces");
        Op::MoveTo { from: vec![d.join("c.txt")], to: d.join("sub") }.run().unwrap();
        assert!(d.join("sub/c.txt").exists() && !d.join("c.txt").exists());
        assert!(Op::MoveTo { from: vec![d.join("sub")], to: d.join("sub") }.run().is_err(), "not into itself");
        Op::NewFolder(d.join("new")).run().unwrap();
        assert!(d.join("new").is_dir());
        Op::Delete(vec![d.join("sub"), d.join("new")]).run().unwrap();
        assert!(!d.join("sub").exists() && !d.join("new").exists() && d.join("dst/a.txt").exists());
        assert!(Op::Delete(vec![d.join("x")]).describe().0.contains("permanently"));
        assert!(valid_name("x.txt") && !valid_name("../x") && !valid_name(".."));
    }
}
