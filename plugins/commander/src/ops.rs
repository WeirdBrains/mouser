//! File changes from the context menu (and their keys): copy, move, rename,
//! delete, a new folder, paste. Each is described first and runs only after
//! the user said yes on Mouser's approval card; none ever replaces an
//! existing file. In an SSH session they run on the remote host, over the
//! session's side channel ([`Op::run_remote`]), never on local paths.

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

/// POSIX `sh` for a change on the remote host (arguments: see [`Op::remote_argv`]).
/// The same rules as [`Op::run`]: never over an existing file, never into itself.
const REMOTE_SCRIPT: &str = concat!(
    "op=$1; to=$2; many=$3; shift 3; ",
    "case $op in ",
    "copy|move) ",
    "if [ \"$many\" = 1 ]; then if [ -d \"$to\" ]; then :; else echo \"$to is not a folder\" >&2; exit 1; fi; fi; ",
    "for p in \"$@\"; do ",
    "if [ \"$many\" = 1 ] || [ -d \"$to\" ]; then t=\"${to%/}/${p##*/}\"; else t=$to; fi; ",
    "if [ -e \"$t\" ] || [ -L \"$t\" ]; then echo \"$t already exists\" >&2; exit 1; fi; ",
    "case \"$t/\" in \"$p/\"*) echo \"$t is inside $p\" >&2; exit 1;; esac; ",
    "if [ \"$op\" = move ]; then mv -- \"$p\" \"$t\" || exit 1; else cp -RP -- \"$p\" \"$t\" || exit 1; fi; ",
    "done;; ",
    "rename) if [ -e \"$to\" ] || [ -L \"$to\" ]; then echo \"$to already exists\" >&2; exit 1; fi; mv -- \"$1\" \"$to\";; ",
    "delete) rm -rf -- \"$@\";; ",
    "mkdir) mkdir -- \"$to\";; ",
    "*) exit 2;; esac"
);

impl Op {
    /// `sh -c SCRIPT mouser-op OP TO MANY PATHS…` for the remote host.
    pub fn remote_argv(&self) -> Vec<String> {
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let (op, to, many, paths): (&str, String, bool, Vec<String>) = match self {
            Op::CopyTo { from, to } => ("copy", s(to), from.len() > 1, from.iter().map(|p| s(p)).collect()),
            Op::MoveTo { from, to } => ("move", s(to), from.len() > 1, from.iter().map(|p| s(p)).collect()),
            Op::Rename { from, to } => ("rename", s(to), false, vec![s(from)]),
            Op::Delete(v) => ("delete", String::new(), false, v.iter().map(|p| s(p)).collect()),
            Op::NewFolder(p) => ("mkdir", s(p), false, Vec::new()),
        };
        let mut v = vec!["sh".to_string(), "-c".into(), REMOTE_SCRIPT.into(), "mouser-op".into(), op.into(), to, if many { "1" } else { "0" }.into()];
        v.extend(paths);
        v
    }

    /// The remote command line ([`Op::remote_argv`], quoted for the login shell).
    pub fn remote_command(&self) -> String {
        self.remote_argv().iter().map(|a| crate::commander::shell_quote(a)).collect::<Vec<_>>().join(" ")
    }

    /// Carry it out on the remote host (after the user said yes).
    pub fn run_remote(&self, r: &crate::fetch::Remote) -> Result<String, String> {
        r.run(&self.remote_command())?;
        Ok(self.done())
    }

    /// What happened, for the status bar.
    fn done(&self) -> String {
        match self {
            Op::CopyTo { from, to } => format!("copied {} item(s) to {}", from.len(), to.display()),
            Op::MoveTo { from, to } => format!("moved {} item(s) to {}", from.len(), to.display()),
            Op::Rename { to, .. } => format!("renamed to {}", to.display()),
            Op::Delete(v) => format!("deleted {} item(s)", v.len()),
            Op::NewFolder(p) => format!("created {}", p.display()),
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

    /// The remote script, run here with `sh` as the host would.
    #[cfg(unix)]
    #[test]
    fn remote_ops_follow_the_same_rules() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let run = |op: Op| {
            let v = op.remote_argv();
            let o = std::process::Command::new(&v[0]).args(&v[1..]).output().unwrap();
            if o.status.success() { Ok(op.done()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_string()) }
        };
        std::fs::write(d.join("a b.txt"), "a").unwrap();
        std::fs::create_dir(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/b.txt"), "b").unwrap();
        std::fs::create_dir(d.join("dst")).unwrap();
        run(Op::CopyTo { from: vec![d.join("a b.txt"), d.join("sub")], to: d.join("dst") }).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("dst/sub/b.txt")).unwrap(), "b");
        assert!(d.join("a b.txt").exists());
        assert!(run(Op::CopyTo { from: vec![d.join("a b.txt")], to: d.join("dst") }).unwrap_err().contains("already exists"));
        assert!(run(Op::CopyTo { from: vec![d.join("a b.txt"), d.join("sub")], to: d.join("nope") }).unwrap_err().contains("not a folder"));
        // One item to a new name.
        run(Op::CopyTo { from: vec![d.join("a b.txt")], to: d.join("c.txt") }).unwrap();
        assert!(d.join("c.txt").is_file());
        run(Op::Rename { from: d.join("c.txt"), to: d.join("e.txt") }).unwrap();
        assert!(run(Op::Rename { from: d.join("e.txt"), to: d.join("dst") }).is_err(), "rename never replaces");
        run(Op::MoveTo { from: vec![d.join("e.txt")], to: d.join("sub") }).unwrap();
        assert!(d.join("sub/e.txt").exists() && !d.join("e.txt").exists());
        assert!(run(Op::MoveTo { from: vec![d.join("sub")], to: d.join("sub") }).unwrap_err().contains("inside"), "not into itself");
        run(Op::NewFolder(d.join("new it's"))).unwrap();
        assert!(d.join("new it's").is_dir());
        assert_eq!(run(Op::Delete(vec![d.join("sub"), d.join("new it's")])).unwrap(), "deleted 2 item(s)");
        assert!(!d.join("sub").exists() && d.join("dst/a b.txt").exists());
        // Quoted for the login shell: a name with a quote survives.
        let c = Op::NewFolder(d.join("q'x")).remote_command();
        let o = std::process::Command::new("sh").arg("-c").arg(&c).output().unwrap();
        assert!(o.status.success() && d.join("q'x").is_dir(), "{c}");
        assert!(!REMOTE_SCRIPT.contains(['\'', '!', '\\']));
    }

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
