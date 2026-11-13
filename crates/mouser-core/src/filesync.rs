//! Local and remote file sync for the MCP tools (`file_sync`,
//! `remote_file_sync`, `file_download`, `file_upload`).
//!
//! Direction is `push` (first path → second), `pull` (second → first) or
//! `both`. For a remote sync the local path is first and the remote path is
//! second, so push uploads and pull downloads.
//!
//! Conflict `newer` copies only when the source mtime is strictly greater.
//! Conflict `refuse` copies nothing that already exists with a different size
//! or mtime, and reports it. A file that would be replaced is moved to
//! Mouser's trash first (a remote file's previous bytes are written there);
//! nothing is deleted for good. `dry_run` lists those changes and writes
//! nothing. Directories need `recursive`. Dotfiles are included; symlinks are
//! skipped. At most [`MAX_ENTRIES`] paths, and a single file at most
//! [`MAX_FILE`] bytes.

use crate::fetch::Remote;
use crate::fileops;
use crate::remote::SshTarget;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

pub const MAX_ENTRIES: usize = 5000;
pub const MAX_FILE: u64 = 512 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Push,
    Pull,
    Both,
}

impl Direction {
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        match s {
            "push" => Ok(Self::Push),
            "pull" => Ok(Self::Pull),
            "both" => Ok(Self::Both),
            _ => Err("direction must be push, pull or both".into()),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Push => "push",
            Self::Pull => "pull",
            Self::Both => "both",
        }
    }

    fn to_b(self) -> bool {
        self != Self::Pull
    }

    fn to_a(self) -> bool {
        self != Self::Push
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    Newer,
    Refuse,
}

impl Conflict {
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        match s {
            "newer" => Ok(Self::Newer),
            "refuse" => Ok(Self::Refuse),
            _ => Err("conflict must be newer or refuse".into()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Newer => "newer",
            Self::Refuse => "refuse",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SyncOpts {
    pub direction: Direction,
    pub conflict: Conflict,
    pub recursive: bool,
    pub dry_run: bool,
}

impl Default for SyncOpts {
    fn default() -> Self {
        Self { direction: Direction::Both, conflict: Conflict::Newer, recursive: true, dry_run: false }
    }
}

#[derive(Clone)]
struct Ent {
    mtime: u64,
    size: u64,
    dir: bool,
}

enum Act {
    Copy { rel: String, a_to_b: bool, bytes: u64, mtime: u64, replace: bool },
    Mkdir { rel: String, a_to_b: bool },
    Skip { rel: String, reason: &'static str },
}

struct Report {
    copied: u64,
    trashed: u64,
    skipped: u64,
    unchanged: u64,
    bytes: u64,
    changes: Vec<Value>,
}

impl Report {
    fn finish(self, opts: &SyncOpts) -> Value {
        let mut changes = self.changes;
        let truncated = changes.len() > 400;
        changes.truncate(400);
        json!({
            "direction": opts.direction.as_str(),
            "conflict": opts.conflict.as_str(),
            "dry_run": opts.dry_run,
            "recursive": opts.recursive,
            "copied": self.copied,
            "trashed": self.trashed,
            "skipped": self.skipped,
            "unchanged": self.unchanged,
            "bytes": self.bytes,
            "truncated": truncated,
            "changes": changes,
        })
    }
}

/// Sync two local paths. `from` is side A, `to` is side B.
pub fn sync_local(from: &Path, to: &Path, opts: &SyncOpts) -> Result<Value> {
    if !from.exists() && !to.exists() {
        bail!("neither path exists");
    }
    if opts.direction != Direction::Pull && !from.exists() {
        bail!("{} does not exist", from.display());
    }
    if opts.direction != Direction::Push && !to.exists() {
        bail!("{} does not exist", to.display());
    }
    if !opts.recursive && ((opts.direction.to_b() && from.is_dir()) || (opts.direction.to_a() && to.is_dir())) {
        bail!("a directory needs recursive: true");
    }
    let a = index_local(from)?;
    let b = index_local(to)?;
    let (ops, unchanged) = plan(&a, &b, opts.direction, opts.conflict);
    let mut rep = Report { copied: 0, trashed: 0, skipped: 0, unchanged, bytes: 0, changes: Vec::new() };
    root_mkdir(from, to, opts, &mut rep)?;
    for op in ops {
        match op {
            Act::Mkdir { rel, a_to_b } => {
                let p = local_join(if a_to_b { to } else { from }, &rel);
                rep.changes.push(json!({"action": "mkdir", "path": p.display().to_string()}));
                if !opts.dry_run {
                    fs::create_dir_all(&p)?;
                }
            }
            Act::Copy { rel, a_to_b, bytes, mtime, replace } => {
                let (src, dst) = if a_to_b { (local_join(from, &rel), local_join(to, &rel)) } else { (local_join(to, &rel), local_join(from, &rel)) };
                if replace && dst.exists() {
                    if opts.dry_run {
                        rep.trashed += 1;
                        rep.changes.push(json!({"action": "trash", "path": dst.display().to_string(), "dry_run": true}));
                    } else {
                        let now = fileops::trash(&dst)?;
                        rep.trashed += 1;
                        rep.changes.push(json!({"action": "trash", "path": dst.display().to_string(), "now_at": now.display().to_string()}));
                    }
                }
                if !opts.dry_run {
                    if let Some(parent) = dst.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::copy(&src, &dst).with_context(|| format!("copying {}", src.display()))?;
                    set_mtime(&dst, mtime);
                }
                rep.copied += 1;
                rep.bytes += bytes;
                rep.changes.push(json!({"action": "copy", "from": src.display().to_string(), "to": dst.display().to_string(), "bytes": bytes}));
            }
            Act::Skip { rel, reason } => {
                rep.skipped += 1;
                rep.changes.push(json!({"action": "skip", "path": rel, "reason": reason}));
            }
        }
    }
    Ok(rep.finish(opts))
}

/// Sync a local path with a path on `target`. Side A is local, side B is remote,
/// so `push` uploads and `pull` downloads. Download and upload are this with
/// one direction.
pub fn sync_remote(local: &Path, remote: &str, target: &SshTarget, opts: &SyncOpts) -> Result<Value> {
    check_remote_path(remote)?;
    let (remote_exists, b) = index_remote(target, remote)?;
    if !local.exists() && !remote_exists {
        bail!("neither path exists");
    }
    if opts.direction != Direction::Pull && !local.exists() {
        bail!("{} does not exist", local.display());
    }
    if opts.direction != Direction::Push && !remote_exists {
        bail!("remote path {remote} does not exist");
    }
    let local_dir = local.is_dir();
    // A remote directory lists its children, not itself. A remote file is key "".
    let remote_is_file = b.get("").is_some_and(|e| !e.dir);
    if !opts.recursive && ((opts.direction.to_b() && local_dir) || (opts.direction.to_a() && remote_exists && !remote_is_file)) {
        bail!("a directory needs recursive: true");
    }
    let a = index_local(local)?;
    let (ops, unchanged) = plan(&a, &b, opts.direction, opts.conflict);
    let mut rep = Report { copied: 0, trashed: 0, skipped: 0, unchanged, bytes: 0, changes: Vec::new() };
    if opts.direction.to_b() && local_dir {
        note_mkdir(&mut rep, remote, opts.dry_run);
        if !opts.dry_run {
            remote_mkdir(target, remote)?;
        }
    }
    if opts.direction.to_a() && remote_exists && !remote_is_file && !local.exists() {
        rep.changes.push(json!({"action": "mkdir", "path": local.display().to_string()}));
        if !opts.dry_run {
            fs::create_dir_all(local)?;
        }
    }
    for op in ops {
        match op {
            Act::Mkdir { rel, a_to_b } => {
                if a_to_b {
                    let p = join_remote(remote, &rel);
                    note_mkdir(&mut rep, &p, opts.dry_run);
                    if !opts.dry_run {
                        remote_mkdir(target, &p)?;
                    }
                } else {
                    let p = local_join(local, &rel);
                    rep.changes.push(json!({"action": "mkdir", "path": p.display().to_string()}));
                    if !opts.dry_run {
                        fs::create_dir_all(&p)?;
                    }
                }
            }
            Act::Copy { rel, a_to_b, bytes, mtime, replace } => {
                if bytes > MAX_FILE {
                    bail!("{rel} is {bytes} bytes; the limit is {MAX_FILE}");
                }
                let lp = local_join(local, &rel);
                let rp = join_remote(remote, &rel);
                if a_to_b {
                    if replace && !opts.dry_run {
                        let old = remote_read(target, &rp)?;
                        let now = fileops::trash_bytes(&rel_name(&rel), &old)?;
                        rep.trashed += 1;
                        rep.changes.push(json!({"action": "trash", "path": rp, "now_at": now.display().to_string()}));
                    } else if replace {
                        rep.trashed += 1;
                        rep.changes.push(json!({"action": "trash", "path": rp, "dry_run": true}));
                    }
                    if !opts.dry_run {
                        let data = fs::read(&lp).with_context(|| format!("reading {}", lp.display()))?;
                        remote_write(target, &rp, &data)?;
                        remote_touch(target, &rp, mtime);
                    }
                    rep.changes.push(json!({"action": "copy", "from": lp.display().to_string(), "to": rp, "bytes": bytes}));
                } else {
                    if replace && lp.exists() {
                        if opts.dry_run {
                            rep.trashed += 1;
                            rep.changes.push(json!({"action": "trash", "path": lp.display().to_string(), "dry_run": true}));
                        } else {
                            let now = fileops::trash(&lp)?;
                            rep.trashed += 1;
                            rep.changes.push(json!({"action": "trash", "path": lp.display().to_string(), "now_at": now.display().to_string()}));
                        }
                    }
                    if !opts.dry_run {
                        if let Some(parent) = lp.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        let data = remote_read(target, &rp)?;
                        fs::write(&lp, &data).with_context(|| format!("writing {}", lp.display()))?;
                        set_mtime(&lp, mtime);
                    }
                    rep.changes.push(json!({"action": "copy", "from": rp, "to": lp.display().to_string(), "bytes": bytes}));
                }
                rep.copied += 1;
                rep.bytes += bytes;
            }
            Act::Skip { rel, reason } => {
                rep.skipped += 1;
                rep.changes.push(json!({"action": "skip", "path": rel, "reason": reason}));
            }
        }
    }
    Ok(rep.finish(opts))
}

fn note_mkdir(rep: &mut Report, path: &str, _dry: bool) {
    rep.changes.push(json!({"action": "mkdir", "path": path}));
}

fn root_mkdir(from: &Path, to: &Path, opts: &SyncOpts, rep: &mut Report) -> Result<()> {
    let mk = |src: &Path, dst: &Path, rep: &mut Report| -> Result<()> {
        if !src.is_dir() {
            return Ok(());
        }
        if dst.is_file() {
            bail!("{} is a file and {} is a directory", dst.display(), src.display());
        }
        if !dst.exists() {
            rep.changes.push(json!({"action": "mkdir", "path": dst.display().to_string()}));
            if !opts.dry_run {
                fs::create_dir_all(dst)?;
            }
        }
        Ok(())
    };
    if opts.direction.to_b() {
        mk(from, to, rep)?;
    }
    if opts.direction.to_a() {
        mk(to, from, rep)?;
    }
    Ok(())
}

fn plan(a: &BTreeMap<String, Ent>, b: &BTreeMap<String, Ent>, dir: Direction, conflict: Conflict) -> (Vec<Act>, u64) {
    let mut keys = BTreeSet::new();
    keys.extend(a.keys().cloned());
    keys.extend(b.keys().cloned());
    let mut ops = Vec::new();
    let mut unchanged = 0u64;
    for rel in keys {
        let ea = a.get(&rel);
        let eb = b.get(&rel);
        match (ea, eb) {
            (Some(x), Some(y)) if x.dir && y.dir => {}
            (Some(x), None) if x.dir && dir.to_b() => ops.push(Act::Mkdir { rel, a_to_b: true }),
            (None, Some(y)) if y.dir && dir.to_a() => ops.push(Act::Mkdir { rel, a_to_b: false }),
            (Some(x), Some(y)) if x.dir != y.dir => ops.push(Act::Skip { rel, reason: "file and directory" }),
            (Some(x), None) if !x.dir && dir.to_b() => ops.push(copy(&rel, true, x, false)),
            (None, Some(y)) if !y.dir && dir.to_a() => ops.push(copy(&rel, false, y, false)),
            (Some(x), Some(y)) if !x.dir && !y.dir => {
                if x.size == y.size && x.mtime == y.mtime {
                    unchanged += 1;
                    continue;
                }
                match conflict {
                    Conflict::Refuse => ops.push(Act::Skip { rel, reason: "conflict" }),
                    Conflict::Newer => {
                        if x.mtime > y.mtime && dir.to_b() {
                            ops.push(copy(&rel, true, x, true));
                        } else if y.mtime > x.mtime && dir.to_a() {
                            ops.push(copy(&rel, false, y, true));
                        } else {
                            ops.push(Act::Skip { rel, reason: "not newer" });
                        }
                    }
                }
            }
            _ => {}
        }
    }
    (ops, unchanged)
}

fn copy(rel: &str, a_to_b: bool, src: &Ent, replace: bool) -> Act {
    Act::Copy { rel: rel.to_string(), a_to_b, bytes: src.size, mtime: src.mtime, replace }
}

fn index_local(root: &Path) -> Result<BTreeMap<String, Ent>> {
    let mut m = BTreeMap::new();
    if !root.exists() {
        return Ok(m);
    }
    if root.is_file() {
        m.insert(String::new(), ent_of(root)?);
        return Ok(m);
    }
    if root.is_dir() {
        walk(root, root, &mut m)?;
    }
    Ok(m)
}

fn walk(root: &Path, dir: &Path, m: &mut BTreeMap<String, Ent>) -> Result<()> {
    for e in fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))? {
        let e = e?;
        let p = e.path();
        let ft = e.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        let rel = rel_of(root, &p);
        if ft.is_dir() {
            m.insert(rel, Ent { mtime: 0, size: 0, dir: true });
            if m.len() > MAX_ENTRIES {
                bail!("more than {MAX_ENTRIES} entries");
            }
            walk(root, &p, m)?;
        } else if ft.is_file() {
            m.insert(rel, ent_of(&p)?);
            if m.len() > MAX_ENTRIES {
                bail!("more than {MAX_ENTRIES} entries");
            }
        }
    }
    Ok(())
}

fn ent_of(p: &Path) -> Result<Ent> {
    let meta = fs::metadata(p).with_context(|| format!("reading {}", p.display()))?;
    Ok(Ent { mtime: mtime_secs(&meta), size: meta.len(), dir: false })
}

fn mtime_secs(meta: &fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

fn set_mtime(p: &Path, secs: u64) {
    let t = UNIX_EPOCH + Duration::from_secs(secs);
    if let Ok(f) = File::options().write(true).open(p) {
        let _ = f.set_modified(t);
    }
}

fn rel_of(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

fn local_join(root: &Path, rel: &str) -> PathBuf {
    if rel.is_empty() {
        return root.to_path_buf();
    }
    rel.split('/').filter(|s| !s.is_empty()).fold(root.to_path_buf(), |p, c| p.join(c))
}

fn join_remote(root: &str, rel: &str) -> String {
    if rel.is_empty() {
        return root.to_string();
    }
    let root = root.trim_end_matches('/');
    format!("{root}/{rel}")
}

fn rel_name(rel: &str) -> String {
    rel.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("file").to_string()
}

fn check_remote_path(path: &str) -> Result<()> {
    if path.is_empty() || path.contains('\n') || path.contains('\0') {
        bail!("remote path is empty or contains a newline");
    }
    Ok(())
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn index_remote(target: &SshTarget, path: &str) -> Result<(bool, BTreeMap<String, Ent>)> {
    let out = Remote::new(target.clone()).run(&list_script(path)).map_err(anyhow::Error::msg)?;
    let text = String::from_utf8_lossy(&out);
    parse_listing(&text)
}

fn list_script(path: &str) -> String {
    let q = sh_quote(path);
    format!(
        r#"p={q}
if [ ! -e "$p" ]; then printf 'x\n'; exit 0; fi
printf 'e\n'
if [ -f "$p" ]; then
  m=$(stat -c %Y "$p" 2>/dev/null || stat -f %m "$p")
  z=$(wc -c < "$p" | tr -d ' ')
  printf 'f\037%s\037%s\037.\n' "$m" "$z"
  exit 0
fi
if [ ! -d "$p" ]; then echo 'not a file or directory' >&2; exit 1; fi
cd "$p" || exit 1
find . -print | while IFS= read -r f; do
  [ "$f" = . ] && continue
  if [ -L "$f" ]; then
    continue
  elif [ -d "$f" ]; then
    printf 'd\037%s\n' "$f"
  elif [ -f "$f" ]; then
    m=$(stat -c %Y "$f" 2>/dev/null || stat -f %m "$f")
    z=$(wc -c < "$f" | tr -d ' ')
    printf 'f\037%s\037%s\037%s\n' "$m" "$z" "$f"
  fi
done
"#
    )
}

fn parse_listing(text: &str) -> Result<(bool, BTreeMap<String, Ent>)> {
    let mut exists = false;
    let mut map = BTreeMap::new();
    for line in text.lines() {
        if line == "x" {
            return Ok((false, map));
        }
        if line == "e" {
            exists = true;
            continue;
        }
        let mut p = line.split('\u{1f}');
        match p.next() {
            Some("d") => {
                let rel = norm_rel(p.next().unwrap_or(""));
                if rel_ok(&rel) {
                    map.insert(rel, Ent { mtime: 0, size: 0, dir: true });
                }
            }
            Some("f") => {
                let mtime = p.next().unwrap_or("0").parse().unwrap_or(0);
                let size = p.next().unwrap_or("0").parse().unwrap_or(0);
                let rel = norm_rel(p.next().unwrap_or(""));
                if rel_ok(&rel) {
                    map.insert(rel, Ent { mtime, size, dir: false });
                }
            }
            _ => {}
        }
        if map.len() > MAX_ENTRIES {
            bail!("more than {MAX_ENTRIES} entries");
        }
    }
    Ok((exists, map))
}

fn norm_rel(s: &str) -> String {
    let s = s.strip_prefix("./").unwrap_or(s);
    if s == "." { String::new() } else { s.trim_start_matches('/').to_string() }
}

fn rel_ok(rel: &str) -> bool {
    rel.is_empty() || !rel.split('/').any(|c| c.is_empty() || c == "." || c == "..")
}

fn remote_mkdir(target: &SshTarget, path: &str) -> Result<()> {
    Remote::new(target.clone()).run(&format!("mkdir -p {}", sh_quote(path))).map_err(anyhow::Error::msg)?;
    Ok(())
}

fn remote_read(target: &SshTarget, path: &str) -> Result<Vec<u8>> {
    let out = Remote::new(target.clone()).run(&format!("cat {}", sh_quote(path))).map_err(anyhow::Error::msg)?;
    if out.len() as u64 > MAX_FILE {
        bail!("{path} is larger than {MAX_FILE} bytes");
    }
    Ok(out)
}

fn remote_write(target: &SshTarget, path: &str, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = parent_remote(path) {
        remote_mkdir(target, parent)?;
    }
    let mut cmd = Remote::new(target.clone()).command(&format!("cat > {}", sh_quote(path)));
    cmd.stdin(std::process::Stdio::piped());
    let mut child = cmd.spawn().with_context(|| format!("ssh upload {path}"))?;
    child.stdin.take().unwrap().write_all(bytes)?;
    let st = child.wait()?;
    if !st.success() {
        bail!("upload of {path} failed ({st})");
    }
    Ok(())
}

fn remote_touch(target: &SshTarget, path: &str, mtime: u64) {
    let q = sh_quote(path);
    let _ = Remote::new(target.clone()).run(&format!("touch -d @{mtime} {q}"));
}

fn parent_remote(path: &str) -> Option<&str> {
    let p = path.trim_end_matches('/');
    match p.rfind('/') {
        Some(0) => Some("/"),
        Some(i) => Some(&p[..i]),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().join("home");
        unsafe { std::env::set_var("MOUSER_HOME", &h) };
        (t, h)
    }

    fn mtime(p: &Path, secs: u64) {
        set_mtime(p, secs);
    }

    #[test]
    #[serial_test::serial]
    fn file_sync_directions_conflict_and_dry_run() {
        let (_t, home) = home();
        let a = _t.path().join("a");
        let b = _t.path().join("b");
        fs::create_dir_all(a.join("sub")).unwrap();
        fs::write(a.join("only.txt"), "only-a").unwrap();
        fs::write(a.join("sub/n.txt"), "new").unwrap();
        fs::write(a.join("same.txt"), "same").unwrap();
        fs::create_dir_all(b.join("sub")).unwrap();
        fs::write(b.join("only.txt"), "old").unwrap();
        fs::write(b.join("sub/n.txt"), "older").unwrap();
        fs::write(b.join("same.txt"), "same").unwrap();
        fs::write(b.join("keep.txt"), "stay").unwrap();
        mtime(&a.join("only.txt"), 200);
        mtime(&b.join("only.txt"), 100);
        mtime(&a.join("sub/n.txt"), 100);
        mtime(&b.join("sub/n.txt"), 300);
        mtime(&a.join("same.txt"), 50);
        mtime(&b.join("same.txt"), 50);

        let dry = SyncOpts { direction: Direction::Both, conflict: Conflict::Newer, recursive: true, dry_run: true };
        let plan = sync_local(&a, &b, &dry).unwrap();
        assert_eq!(plan["copied"], 3);
        assert_eq!(plan["dry_run"], true);
        assert!(!a.join("keep.txt").exists());
        assert_eq!(fs::read_to_string(b.join("only.txt")).unwrap(), "old");

        let opts = SyncOpts { dry_run: false, ..dry };
        let rep = sync_local(&a, &b, &opts).unwrap();
        assert_eq!(rep["copied"], 3);
        assert_eq!(rep["trashed"], 2, "{rep}");
        assert_eq!(fs::read_to_string(b.join("only.txt")).unwrap(), "only-a");
        assert_eq!(fs::read_to_string(a.join("sub/n.txt")).unwrap(), "older");
        assert_eq!(fs::read_to_string(a.join("keep.txt")).unwrap(), "stay");
        assert_eq!(rep["unchanged"], 1);
        let trash = fs::read_dir(home.join("trash")).unwrap().flatten().count();
        assert!(trash >= 1);

        fs::write(a.join("only.txt"), "again").unwrap();
        mtime(&a.join("only.txt"), 400);
        let refused = sync_local(&a, &b, &SyncOpts { conflict: Conflict::Refuse, ..opts }).unwrap();
        assert_eq!(refused["copied"], 0);
        assert!(refused["skipped"].as_u64().unwrap() >= 1);
        assert_eq!(fs::read_to_string(b.join("only.txt")).unwrap(), "only-a");

        let err = sync_local(&a, &b, &SyncOpts { recursive: false, ..opts }).unwrap_err();
        assert!(err.to_string().contains("recursive"), "{err}");
        unsafe { std::env::remove_var("MOUSER_HOME") };
    }

    #[test]
    #[serial_test::serial]
    fn file_sync_push_and_pull_one_file() {
        let (_t, _) = home();
        let a = _t.path().join("f.txt");
        let b = _t.path().join("g.txt");
        fs::write(&a, "hi").unwrap();
        mtime(&a, 10);
        let push = SyncOpts { direction: Direction::Push, conflict: Conflict::Newer, recursive: false, dry_run: false };
        let rep = sync_local(&a, &b, &push).unwrap();
        assert_eq!(rep["copied"], 1);
        assert_eq!(fs::read_to_string(&b).unwrap(), "hi");
        fs::write(&b, "newer-b").unwrap();
        mtime(&b, 20);
        let pull = SyncOpts { direction: Direction::Pull, ..push };
        sync_local(&a, &b, &pull).unwrap();
        assert_eq!(fs::read_to_string(&a).unwrap(), "newer-b");
        unsafe { std::env::remove_var("MOUSER_HOME") };
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn remote_file_sync_upload_download_and_both() {
        let (_t, _) = home();
        let remote_root = _t.path().join("remote");
        fs::create_dir_all(remote_root.join("d")).unwrap();
        let ssh = _t.path().join("fake-ssh");
        let mut f = File::create(&ssh).unwrap();
        writeln!(
            f,
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"--\" ]; then shift; exec sh -c \"$1\"; fi\n  shift\ndone\necho 'no command' >&2\nexit 1\n"
        )
        .unwrap();
        drop(f);
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755)).unwrap();
        unsafe { std::env::set_var("MOUSER_SSH_BIN", &ssh) };

        let local = _t.path().join("local");
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join("up.txt"), "uploaded").unwrap();
        mtime(&local.join("up.txt"), 10);
        let target = SshTarget { opts: vec![], dest: "fake".into() };
        let remote = remote_root.join("box");
        let push = SyncOpts { direction: Direction::Push, conflict: Conflict::Newer, recursive: true, dry_run: false };
        let up = sync_remote(&local, &remote.to_string_lossy(), &target, &push).unwrap();
        assert_eq!(up["copied"], 1, "{up}");
        assert_eq!(fs::read_to_string(remote.join("up.txt")).unwrap(), "uploaded");

        fs::create_dir_all(remote.join("d")).unwrap();
        fs::write(remote.join("d/down.txt"), "from-remote").unwrap();
        mtime(&remote.join("d/down.txt"), 30);
        let both = SyncOpts { direction: Direction::Both, ..push };
        let rep = sync_remote(&local, &remote.to_string_lossy(), &target, &both).unwrap();
        assert!(rep["copied"].as_u64().unwrap() >= 1, "{rep}");
        assert_eq!(fs::read_to_string(local.join("d/down.txt")).unwrap(), "from-remote");

        let dest = _t.path().join("got.txt");
        let pull = SyncOpts { direction: Direction::Pull, recursive: false, ..push };
        sync_remote(&dest, &remote.join("up.txt").to_string_lossy(), &target, &pull).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "uploaded");

        fs::write(&dest, "local-newer").unwrap();
        mtime(&dest, 90);
        let again = sync_remote(&dest, &remote.join("up.txt").to_string_lossy(), &target, &pull).unwrap();
        assert_eq!(again["copied"], 0, "{again}");
        assert_eq!(fs::read_to_string(&dest).unwrap(), "local-newer");

        unsafe {
            std::env::remove_var("MOUSER_SSH_BIN");
            std::env::remove_var("MOUSER_HOME");
        }
    }
}
