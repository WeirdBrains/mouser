//! Remote file fetch layer: ranged, parallel reads of a file on an ssh host
//! into a sparse local cache, with a filled-range map, so a viewer can show
//! the start (or any range it asks for) long before the whole file is here.
//!
//! - [`RangeMap`]: which byte ranges of the cache file are filled, and which
//!   chunk to fetch next (from the priority offset on, then wrapping).
//! - [`Remote`]: the ssh side channel (the session's own `ssh` options,
//!   `BatchMode`, multiplexed with a ControlMaster under
//!   `$MOUSER_HOME/cache/ssh`), remote commands and which tools the host has.
//! - [`open`] -> [`Handle`]: a cached transfer of one [`RemoteFile`] (key:
//!   host + path + size + mtime, so an unchanged file is never fetched twice).
//!   Up to [`SEGMENTS`] concurrent ranged reads (`tail -c +N | head -c L`
//!   over the shared connection); [`Handle::prioritize`] moves the next reads
//!   to where a seek or scroll needs data; [`Handle::url`] serves the file
//!   over a local HTTP server with Range support (ffmpeg reads remote video
//!   through it: seeks become prioritised ranges).
//! - When the last handle goes (off-screen, closed) the transfer stops,
//!   unless the file is small enough to finish into the cache
//!   (`MOUSER_REMOTE_COMPLETE_MB`, default 64): then rsync (both ends have
//!   it) or scp completes it, else the ranged reads carry on.
//! - The cache (`$MOUSER_HOME/cache/remote`) is trimmed least-recently-used
//!   first to `MOUSER_REMOTE_CACHE_MB` (default 2048).
//!
//! Nothing here is specific to commander: a later local<->remote sync can
//! reuse the range map, cache keys, backends and the transfer queue.

use crate::remote::SshTarget;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

/// Concurrent ranged reads per file.
pub const SEGMENTS: usize = 3;
/// Files fetching at the same time (the transfer queue's width).
pub const CONCURRENT_FILES: usize = 3;
/// The first read: enough for an image header, the start of a text file or
/// a video's first frames.
pub const HEAD: u64 = 256 << 10;
/// Later reads.
pub const CHUNK: u64 = 2 << 20;

// ---------------------------------------------------------------- range map

/// Filled byte ranges, half-open, sorted and merged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RangeMap {
    r: Vec<(u64, u64)>,
}

impl RangeMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ranges(&self) -> &[(u64, u64)] {
        &self.r
    }

    pub fn insert(&mut self, a: u64, b: u64) {
        if b <= a {
            return;
        }
        let (mut a, mut b) = (a, b);
        let mut out = Vec::with_capacity(self.r.len() + 1);
        let mut placed = false;
        for &(x, y) in &self.r {
            if y < a {
                out.push((x, y));
            } else if b < x {
                if !placed {
                    out.push((a, b));
                    placed = true;
                }
                out.push((x, y));
            } else {
                a = a.min(x);
                b = b.max(y);
            }
        }
        if !placed {
            out.push((a, b));
        }
        out.sort_unstable();
        self.r = out;
    }

    /// `[a, b)` is all filled.
    pub fn contains(&self, a: u64, b: u64) -> bool {
        b <= a || self.r.iter().any(|&(x, y)| x <= a && b <= y)
    }

    /// Bytes filled from 0 without a hole.
    pub fn prefix(&self) -> u64 {
        self.r.first().filter(|r| r.0 == 0).map_or(0, |r| r.1)
    }

    pub fn filled(&self) -> u64 {
        self.r.iter().map(|(a, b)| b - a).sum()
    }

    /// The holes in `[a, b)`.
    pub fn gaps(&self, a: u64, b: u64) -> Vec<(u64, u64)> {
        let mut v = Vec::new();
        let mut at = a;
        for &(x, y) in &self.r {
            if y <= at {
                continue;
            }
            if x >= b {
                break;
            }
            if x > at {
                v.push((at, x.min(b)));
            }
            at = at.max(y);
            if at >= b {
                break;
            }
        }
        if at < b {
            v.push((at, b));
        }
        v
    }

    /// The next read for a `size`-byte file: the first hole at or after
    /// `from` (wrapping to the start), skipping what `busy` reads cover; at
    /// most `chunk` long (the very first read: [`HEAD`]).
    pub fn next(&self, from: u64, size: u64, chunk: u64, busy: &[(u64, u64)]) -> Option<(u64, u64)> {
        let mut taken = self.clone();
        for &(a, b) in busy {
            taken.insert(a, b);
        }
        let from = from.min(size);
        let g = taken.gaps(from, size).into_iter().next().or_else(|| taken.gaps(0, from).into_iter().next())?;
        let len = if g.0 < HEAD { (HEAD - g.0).min(chunk) } else { chunk };
        Some((g.0, (g.0 + len.max(1)).min(g.1)))
    }

    pub fn to_text(&self) -> String {
        self.r.iter().map(|(a, b)| format!("{a} {b}\n")).collect()
    }

    pub fn from_text(s: &str) -> Self {
        let mut m = Self::new();
        for l in s.lines() {
            let mut it = l.split_whitespace().filter_map(|n| n.parse::<u64>().ok());
            if let (Some(a), Some(b)) = (it.next(), it.next()) {
                m.insert(a, b);
            }
        }
        m
    }
}

// ---------------------------------------------------------------- ssh side channel

/// The ssh binary (`MOUSER_SSH_BIN` overrides it, for tests).
pub fn ssh_bin() -> std::ffi::OsString {
    std::env::var_os("MOUSER_SSH_BIN").unwrap_or_else(|| "ssh".into())
}

/// Where the ControlMaster sockets live.
pub fn control_dir() -> PathBuf {
    let cd = crate::storage::resolve_home().join("cache/ssh");
    let _ = std::fs::create_dir_all(&cd);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&cd, std::fs::Permissions::from_mode(0o700));
    }
    cd
}

/// What the remote host has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tools {
    pub rsync: bool,
    pub ffmpeg: bool,
}

/// A host reached through the session's ssh options.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Remote {
    pub target: SshTarget,
}

static TOOLS: Lazy<Mutex<HashMap<SshTarget, Tools>>> = Lazy::new(Default::default);

impl Remote {
    pub fn new(target: SshTarget) -> Self {
        Self { target }
    }

    /// `ssh ... -- cmd` with Mouser's config (`MOUSER_SSH_CONFIG`) unless
    /// the session gave its own `-F`.
    pub fn command(&self, remote_cmd: &str) -> Command {
        let mut c = Command::new(ssh_bin());
        if let Some(cfg) = std::env::var_os("MOUSER_SSH_CONFIG")
            && !self.target.opts.iter().any(|o| o == "-F")
        {
            c.arg("-F").arg(cfg);
        }
        c.args(self.target.argv(&control_dir(), remote_cmd)).stdin(Stdio::null());
        c
    }

    /// Run `cmd` remotely; stdout, or ssh's last stderr line.
    pub fn run(&self, cmd: &str) -> Result<Vec<u8>, String> {
        let out = self.command(cmd).output().map_err(|e| format!("ssh: {e}"))?;
        if !out.status.success() {
            let e = String::from_utf8_lossy(&out.stderr);
            return Err(e.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("ssh failed").trim().to_string());
        }
        Ok(out.stdout)
    }

    /// Tools on the host (asked once per host).
    pub fn tools(&self) -> Tools {
        if let Some(t) = TOOLS.lock().unwrap().get(&self.target) {
            return *t;
        }
        let out = self.run("for t in rsync ffmpeg ffprobe; do command -v $t >/dev/null 2>&1 && echo $t; done").unwrap_or_default();
        let s = String::from_utf8_lossy(&out);
        let has = |n: &str| s.lines().any(|l| l.trim() == n);
        let t = Tools { rsync: has("rsync"), ffmpeg: has("ffmpeg") && has("ffprobe") };
        TOOLS.lock().unwrap().insert(self.target.clone(), t);
        t
    }

    /// Read `[a, b)` of `path` (POSIX `tail`/`head`, GNU and BSD).
    pub fn read_range(&self, path: &Path, a: u64, b: u64) -> Result<Vec<u8>, String> {
        let q = crate::commander::shell_quote(&path.to_string_lossy());
        let v = self.run(&format!("tail -c +{} {q} | head -c {}", a + 1, b - a))?;
        if (v.len() as u64) < b - a {
            return Err(format!("{}: short read at {a} ({} of {} bytes; changed or unreadable?)", path.display(), v.len(), b - a));
        }
        Ok(v)
    }

    /// The ssh options as one `-e` string for rsync (dest and command left off).
    fn rsh(&self) -> String {
        let mut v = vec![ssh_bin().to_string_lossy().into_owned()];
        if let Some(cfg) = std::env::var_os("MOUSER_SSH_CONFIG")
            && !self.target.opts.iter().any(|o| o == "-F")
        {
            v.push("-F".into());
            v.push(cfg.to_string_lossy().into_owned());
        }
        let a = self.target.argv(&control_dir(), "");
        v.extend(a[..a.len() - 3].iter().cloned());
        v.iter().map(|s| crate::commander::shell_quote(s)).collect::<Vec<_>>().join(" ")
    }
}

/// Which tool completes a file that was left (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Rsync,
    Scp,
    Cat,
}

fn local_has(tool: &str) -> bool {
    Command::new(tool).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok()
}

/// rsync when both ends have it, else scp, else ranged `cat` reads.
/// `MOUSER_REMOTE_BACKEND=rsync|scp|cat` forces one (tests, measuring).
pub fn backend(r: &Remote) -> Backend {
    match std::env::var("MOUSER_REMOTE_BACKEND").as_deref() {
        Ok("rsync") => return Backend::Rsync,
        Ok("scp") => return Backend::Scp,
        Ok("cat") => return Backend::Cat,
        _ => {}
    }
    if local_has("rsync") && r.tools().rsync {
        Backend::Rsync
    } else if local_has("scp") && std::env::var_os("MOUSER_SSH_BIN").is_none() {
        Backend::Scp
    } else {
        Backend::Cat
    }
}

// ---------------------------------------------------------------- transfers

/// A file on a host, as listed (size and mtime make the cache key).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemoteFile {
    pub remote: Remote,
    pub path: PathBuf,
    pub size: u64,
    /// Seconds since the epoch (0: unknown).
    pub mtime: i64,
}

impl RemoteFile {
    /// Host + path + size + mtime.
    pub fn key(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&self.remote.target, &self.path, self.size, self.mtime).hash(&mut h);
        format!("{:016x}", h.finish())
    }
}

pub fn cache_dir() -> PathBuf {
    crate::storage::resolve_home().join("cache/remote")
}

fn env_mb(k: &str, d: u64) -> u64 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d) << 20
}

#[derive(Default)]
struct State {
    map: RangeMap,
    busy: Vec<(u64, u64)>,
    prio: u64,
    /// Fetch only up to here (`size`: all); the largest any handle asked for.
    upto: u64,
    users: usize,
    workers: usize,
    error: Option<String>,
    stop: bool,
    /// Holds one of the [`CONCURRENT_FILES`] slots.
    slot: bool,
    /// Offsets in the order they were read (tests, timing).
    order: Vec<u64>,
    wakers: Vec<(u64, Arc<dyn Fn() + Send + Sync>)>,
    started: Option<Instant>,
}

pub struct Inner {
    pub file: RemoteFile,
    pub cache: PathBuf,
    map_path: PathBuf,
    st: Mutex<State>,
    cv: Condvar,
    http: Mutex<Option<String>>,
}

/// A user of a transfer; when the last one drops the transfer stops (or
/// completes into the cache, see the module docs).
pub struct Handle {
    inner: Arc<Inner>,
    waker: Option<u64>,
}

static REGISTRY: Lazy<Mutex<HashMap<String, Weak<Inner>>>> = Lazy::new(Default::default);
static SLOTS: Lazy<(Mutex<usize>, Condvar)> = Lazy::new(|| (Mutex::new(0), Condvar::new()));
static NEXT_WAKER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Start (or join) the transfer of `f`, fetching up to `upto` bytes
/// (`None`: all). A complete cached copy with the same key is used as is.
pub fn open(f: &RemoteFile, upto: Option<u64>) -> Handle {
    let key = f.key();
    let upto = upto.unwrap_or(f.size).min(f.size);
    let mut reg = REGISTRY.lock().unwrap();
    let inner = match reg.get(&key).and_then(Weak::upgrade) {
        Some(i) => i,
        None => {
            let dir = cache_dir();
            let _ = std::fs::create_dir_all(&dir);
            let cache = dir.join(format!("{key}.data"));
            let map_path = dir.join(format!("{key}.map"));
            let mut map = std::fs::read_to_string(&map_path).map(|s| RangeMap::from_text(&s)).unwrap_or_default();
            if std::fs::metadata(&cache).map(|m| m.len()).ok() != Some(f.size) {
                map = RangeMap::new();
                let _ = std::fs::File::create(&cache).and_then(|c| c.set_len(f.size));
            }
            // Touch for the LRU.
            let _ = std::fs::File::options().write(true).open(&cache).and_then(|c| c.set_modified(std::time::SystemTime::now()));
            let i = Arc::new(Inner {
                file: f.clone(),
                cache,
                map_path,
                st: Mutex::new(State { map, ..Default::default() }),
                cv: Condvar::new(),
                http: Mutex::new(None),
            });
            reg.insert(key.clone(), Arc::downgrade(&i));
            reg.retain(|_, w| w.strong_count() > 0);
            drop(reg);
            evict(env_mb("MOUSER_REMOTE_CACHE_MB", 2048));
            reg = REGISTRY.lock().unwrap();
            i
        }
    };
    drop(reg);
    {
        let mut s = inner.st.lock().unwrap();
        s.users += 1;
        s.upto = s.upto.max(upto);
        s.stop = false;
    }
    spawn_workers(&inner);
    Handle { inner, waker: None }
}

fn spawn_workers(inner: &Arc<Inner>) {
    let mut s = inner.st.lock().unwrap();
    if s.map.contains(0, s.upto) {
        return;
    }
    while s.workers < SEGMENTS {
        s.workers += 1;
        let i = inner.clone();
        if std::thread::Builder::new().name("remote-fetch".into()).spawn(move || worker(i)).is_err() {
            s.workers -= 1;
            break;
        }
    }
}

fn take_slot(i: &Inner) -> bool {
    {
        if i.st.lock().unwrap().slot {
            return true;
        }
    }
    let (m, cv) = &*SLOTS;
    let mut n = m.lock().unwrap();
    loop {
        {
            let s = i.st.lock().unwrap();
            if s.stop || s.users == 0 {
                return false;
            }
            if s.slot {
                return true;
            }
        }
        if *n < CONCURRENT_FILES {
            *n += 1;
            i.st.lock().unwrap().slot = true;
            return true;
        }
        n = cv.wait_timeout(n, Duration::from_millis(200)).unwrap().0;
    }
}

fn give_slot(s: &mut State) {
    if s.slot {
        s.slot = false;
        let (m, cv) = &*SLOTS;
        *m.lock().unwrap() -= 1;
        cv.notify_all();
    }
}

fn worker(i: Arc<Inner>) {
    if !take_slot(&i) {
        let mut s = i.st.lock().unwrap();
        s.workers -= 1;
        return;
    }
    let size = i.file.size;
    let mut fails = 0;
    loop {
        let job = {
            let mut s = i.st.lock().unwrap();
            let left = s.users == 0;
            if s.stop || s.error.is_some() || s.map.contains(0, s.upto) || (left && size > env_mb("MOUSER_REMOTE_COMPLETE_MB", 64)) {
                None
            } else if left && s.busy.is_empty() && backend(&i.file.remote) != Backend::Cat {
                // Nobody looks any more: let rsync / scp finish it.
                s.busy.push((0, size));
                Some((0, u64::MAX))
            } else {
                let upto = if left { size } else { s.upto };
                let from = s.prio.min(upto);
                let busy = s.busy.clone();
                match s.map.next(from, size, CHUNK, &busy).filter(|r| r.0 < upto) {
                    Some(r) => {
                        s.busy.push(r);
                        Some(r)
                    }
                    None if s.busy.is_empty() => None,
                    // Others are still reading: wait for theirs.
                    None => {
                        let _ = i.cv.wait_timeout(s, Duration::from_millis(100)).unwrap();
                        continue;
                    }
                }
            }
        };
        let Some((a, b)) = job else { break };
        if b == u64::MAX {
            let r = complete(&i);
            let mut s = i.st.lock().unwrap();
            s.busy.retain(|r| *r != (0, size));
            match r {
                Ok(()) => {
                    s.map = RangeMap::new();
                    s.map.insert(0, size);
                    let _ = std::fs::write(&i.map_path, s.map.to_text());
                }
                Err(e) => s.error = Some(e),
            }
            break;
        }
        let r = i.file.remote.read_range(&i.file.path, a, b).and_then(|d| write_at(&i.cache, a, &d).map_err(|e| format!("cache: {e}")));
        let wakers = {
            let mut s = i.st.lock().unwrap();
            s.busy.retain(|r| *r != (a, b));
            match r {
                Ok(()) => {
                    fails = 0;
                    s.map.insert(a, b);
                    s.order.push(a);
                    let _ = std::fs::write(&i.map_path, s.map.to_text());
                }
                Err(e) => {
                    fails += 1;
                    if fails > 2 {
                        s.error = Some(e);
                    }
                }
            }
            s.wakers.iter().map(|w| w.1.clone()).collect::<Vec<_>>()
        };
        i.cv.notify_all();
        for w in wakers {
            w();
        }
    }
    let wakers = {
        let mut s = i.st.lock().unwrap();
        s.workers -= 1;
        if s.workers == 0 {
            give_slot(&mut s);
        }
        s.wakers.iter().map(|w| w.1.clone()).collect::<Vec<_>>()
    };
    i.cv.notify_all();
    for w in wakers {
        w();
    }
}

fn write_at(p: &Path, at: u64, d: &[u8]) -> std::io::Result<()> {
    let mut f = std::fs::File::options().write(true).open(p)?;
    f.seek(SeekFrom::Start(at))?;
    f.write_all(d)
}

/// Finish `i` with rsync (in place: what is cached already is not sent
/// again) or scp.
fn complete(i: &Inner) -> Result<(), String> {
    let r = &i.file.remote;
    let src = format!("{}:{}", r.target.dest, i.file.path.display());
    let st = match backend(r) {
        Backend::Rsync => Command::new("rsync")
            .args(["-s", "--inplace", "--no-whole-file", "-e"])
            .arg(r.rsh())
            .arg(&src)
            .arg(&i.cache)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output(),
        _ => {
            let tmp = i.cache.with_extension("scp");
            let mut c = Command::new("scp");
            c.arg("-q");
            if let Some(cfg) = std::env::var_os("MOUSER_SSH_CONFIG") {
                c.arg("-F").arg(cfg);
            }
            c.arg("-o").arg(format!("ControlPath={}", control_dir().join("%C").display())).args(["-o", "BatchMode=yes"]);
            let mut it = r.target.opts.iter();
            while let Some(o) = it.next() {
                match o.as_str() {
                    "-p" => {
                        c.arg("-P").args(it.next());
                    }
                    "-o" | "-i" | "-J" | "-F" => {
                        c.arg(o).args(it.next());
                    }
                    "-l" => {
                        c.arg("-o").arg(format!("User={}", it.next().map_or("", |s| s)));
                    }
                    _ => {}
                }
            }
            let out = c.arg(&src).arg(&tmp).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output();
            if out.as_ref().is_ok_and(|o| o.status.success()) {
                std::fs::rename(&tmp, &i.cache).map_err(|e| e.to_string())?;
            }
            out
        }
    };
    match st {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(format!("completing {}: {}", i.file.path.display(), String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("failed"))),
        Err(e) => Err(format!("completing {}: {e}", i.file.path.display())),
    }
}

/// Trim the cache to `cap` bytes, least recently opened first, sparing
/// transfers in use.
pub fn evict(cap: u64) {
    let Ok(rd) = std::fs::read_dir(cache_dir()) else { return };
    let live: Vec<PathBuf> = REGISTRY.lock().unwrap().values().filter_map(Weak::upgrade).map(|i| i.cache.clone()).collect();
    let mut v: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "data"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = v.iter().map(|x| x.1).sum();
    v.sort();
    for (_, len, p) in v {
        if total <= cap {
            break;
        }
        if live.contains(&p) {
            continue;
        }
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("map"));
        total -= len;
    }
}

impl Handle {
    pub fn file(&self) -> &RemoteFile {
        &self.inner.file
    }

    /// The sparse cache file (holes read as zeros: check [`Handle::map`]).
    pub fn cache_path(&self) -> &Path {
        &self.inner.cache
    }

    pub fn map(&self) -> RangeMap {
        self.inner.st.lock().unwrap().map.clone()
    }

    pub fn complete(&self) -> bool {
        self.inner.st.lock().unwrap().map.contains(0, self.inner.file.size)
    }

    pub fn error(&self) -> Option<String> {
        self.inner.st.lock().unwrap().error.clone()
    }

    /// Fetch from `at` next (a seek, a scroll).
    pub fn prioritize(&self, at: u64) {
        self.inner.st.lock().unwrap().prio = at;
        self.inner.cv.notify_all();
    }

    /// Fetch everything (a head-only handle that is now opened).
    pub fn want_all(&self) {
        self.inner.st.lock().unwrap().upto = self.inner.file.size;
        spawn_workers(&self.inner);
    }

    /// Read order (tests, timing).
    pub fn order(&self) -> Vec<u64> {
        self.inner.st.lock().unwrap().order.clone()
    }

    /// Called whenever a read lands (or the transfer ends).
    pub fn on_progress(&mut self, w: Arc<dyn Fn() + Send + Sync>) {
        let id = NEXT_WAKER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.st.lock().unwrap().wakers.push((id, w));
        self.waker = Some(id);
    }

    /// Wait until `[a, b)` is here; false on error, timeout or `cancel()`.
    pub fn wait(&self, a: u64, b: u64, timeout: Duration, cancel: &dyn Fn() -> bool) -> bool {
        let b = b.min(self.inner.file.size);
        let end = Instant::now() + timeout;
        let mut s = self.inner.st.lock().unwrap();
        s.started.get_or_insert_with(Instant::now);
        loop {
            if s.map.contains(a, b) {
                return true;
            }
            if s.error.is_some() || cancel() || Instant::now() >= end {
                return false;
            }
            if !s.map.contains(s.prio, (s.prio + 1).min(b)) || s.prio > b {
                // Make sure what we wait for is next.
                if s.map.gaps(a, b).first().is_some_and(|g| !s.busy.iter().any(|r| r.0 <= g.0 && g.0 < r.1)) {
                    s.prio = s.map.gaps(a, b)[0].0;
                }
            }
            if s.workers == 0 {
                s.upto = s.upto.max(b);
                drop(s);
                spawn_workers(&self.inner);
                s = self.inner.st.lock().unwrap();
            }
            s = self.inner.cv.wait_timeout(s, Duration::from_millis(50)).unwrap().0;
        }
    }

    /// Read `[a, b)` once it is here.
    pub fn read(&self, a: u64, b: u64, timeout: Duration) -> Option<Vec<u8>> {
        if !self.wait(a, b, timeout, &|| false) {
            return None;
        }
        let mut f = std::fs::File::open(&self.inner.cache).ok()?;
        f.seek(SeekFrom::Start(a)).ok()?;
        let mut v = vec![0; (b.min(self.inner.file.size) - a) as usize];
        f.read_exact(&mut v).ok()?;
        Some(v)
    }

    /// `http://127.0.0.1:<port>/<name>`: the file with Range support, each
    /// request prioritising and waiting for its bytes (for ffmpeg).
    pub fn url(&self) -> String {
        let mut h = self.inner.http.lock().unwrap();
        if let Some(u) = &*h {
            return u.clone();
        }
        let name = self
            .inner
            .file
            .path
            .file_name()
            .map(|n| n.to_string_lossy().replace(|c: char| !c.is_ascii_alphanumeric() && c != '.', "_"))
            .unwrap_or_default();
        let u = match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(l) => {
                let port = l.local_addr().map(|a| a.port()).unwrap_or(0);
                let w = Arc::downgrade(&self.inner);
                let _ = l.set_nonblocking(true);
                std::thread::spawn(move || serve(l, w));
                format!("http://127.0.0.1:{port}/{name}")
            }
            Err(_) => String::new(),
        };
        *h = Some(u.clone());
        u
    }
}

impl Clone for Handle {
    fn clone(&self) -> Self {
        self.inner.st.lock().unwrap().users += 1;
        Handle { inner: self.inner.clone(), waker: None }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let mut s = self.inner.st.lock().unwrap();
        s.users = s.users.saturating_sub(1);
        if let Some(id) = self.waker {
            s.wakers.retain(|w| w.0 != id);
        }
        let small = self.inner.file.size <= env_mb("MOUSER_REMOTE_COMPLETE_MB", 64);
        if s.users == 0 && small && s.workers == 0 && !s.map.contains(0, self.inner.file.size) && s.error.is_none() {
            // Finish small files into the cache in the background.
            s.upto = self.inner.file.size;
            drop(s);
            spawn_workers(&self.inner);
        } else {
            drop(s);
        }
        self.inner.cv.notify_all();
    }
}

fn serve(l: std::net::TcpListener, w: Weak<Inner>) {
    loop {
        match l.accept() {
            Ok((c, _)) => {
                let Some(i) = w.upgrade() else { return };
                let _ = c.set_nonblocking(false);
                std::thread::spawn(move || {
                    let _ = serve_one(c, &i);
                });
            }
            Err(_) => {
                if w.strong_count() == 0 {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// `Range: bytes=a-b` (b inclusive) -> [a, b+1).
pub fn parse_range(req: &str, size: u64) -> (u64, u64, bool) {
    for l in req.lines() {
        let l = l.trim();
        if l.to_ascii_lowercase().starts_with("range:")
            && let Some(r) = l.split_once('=').map(|x| x.1.trim())
        {
            let (a, b) = r.split_once('-').unwrap_or((r, ""));
            let a: u64 = a.trim().parse().unwrap_or(0);
            let b: u64 = b.trim().parse::<u64>().map(|b| b + 1).unwrap_or(size);
            return (a.min(size), b.min(size).max(a.min(size)), true);
        }
    }
    (0, size, false)
}

fn serve_one(mut c: std::net::TcpStream, i: &Arc<Inner>) -> std::io::Result<()> {
    let mut req = Vec::new();
    let mut b = [0u8; 1024];
    while !req.windows(4).any(|w| w == b"\r\n\r\n") && req.len() < 16384 {
        let n = c.read(&mut b)?;
        if n == 0 {
            return Ok(());
        }
        req.extend_from_slice(&b[..n]);
    }
    let req = String::from_utf8_lossy(&req).into_owned();
    let size = i.file.size;
    let (a, e, ranged) = parse_range(&req, size);
    let head = req.starts_with("HEAD");
    let status = if ranged { "206 Partial Content" } else { "200 OK" };
    let mut hdr = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n",
        e - a
    );
    if ranged {
        hdr += &format!("Content-Range: bytes {a}-{}/{size}\r\n", e.saturating_sub(1).max(a));
    }
    hdr += "\r\n";
    c.write_all(hdr.as_bytes())?;
    if head {
        return Ok(());
    }
    let h = Handle { inner: i.clone(), waker: None };
    h.inner.st.lock().unwrap().users += 1;
    h.prioritize(a);
    let mut f = std::fs::File::open(&i.cache)?;
    let mut at = a;
    let mut buf = vec![0u8; 256 << 10];
    while at < e {
        let to = (at + buf.len() as u64).min(e);
        if !h.wait(at, to, Duration::from_secs(60), &|| false) {
            break;
        }
        f.seek(SeekFrom::Start(at))?;
        let n = (to - at) as usize;
        f.read_exact(&mut buf[..n])?;
        c.write_all(&buf[..n])?;
        at = to;
        // Keep reading ahead of the client.
        if !h.inner.st.lock().unwrap().map.contains(at, (at + CHUNK).min(e)) {
            h.prioritize(at);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_map_merges_and_finds_gaps() {
        let mut m = RangeMap::new();
        m.insert(10, 20);
        m.insert(30, 40);
        m.insert(20, 25);
        assert_eq!(m.ranges(), [(10, 25), (30, 40)]);
        m.insert(0, 10);
        assert_eq!(m.prefix(), 25);
        assert!(m.contains(12, 25) && !m.contains(20, 31));
        assert_eq!(m.gaps(0, 50), [(25, 30), (40, 50)]);
        assert_eq!(m.filled(), 35);
        m.insert(5, 45);
        assert_eq!(m.ranges(), [(0, 45)]);
        assert_eq!(RangeMap::from_text(&m.to_text()), m);
    }

    #[test]
    fn next_read_follows_priority_skips_busy_and_wraps() {
        let size = 20 << 20;
        let m = RangeMap::new();
        // First: the head (preview), small.
        assert_eq!(m.next(0, size, CHUNK, &[]), Some((0, HEAD)));
        // A seek near the end goes first, then wraps to the start.
        let at = 18 << 20;
        assert_eq!(m.next(at, size, CHUNK, &[]), Some((at, at + CHUNK)));
        let mut m = RangeMap::new();
        m.insert(at, size);
        assert_eq!(m.next(at, size, CHUNK, &[]), Some((0, HEAD)), "wraps");
        // Busy reads are not repeated.
        assert_eq!(m.next(0, size, CHUNK, &[(0, HEAD)]), Some((HEAD, HEAD + CHUNK)));
        m.insert(0, at);
        assert_eq!(m.next(0, size, CHUNK, &[]), None);
        // Short tail chunk.
        let mut m = RangeMap::new();
        m.insert(0, size - 10);
        assert_eq!(m.next(0, size, CHUNK, &[]), Some((size - 10, size)));
    }

    #[test]
    fn http_range_parse() {
        assert_eq!(parse_range("GET / HTTP/1.1\r\nRange: bytes=100-\r\n", 1000), (100, 1000, true));
        assert_eq!(parse_range("GET / HTTP/1.1\r\nrange: bytes=0-99\r\n", 1000), (0, 100, true));
        assert_eq!(parse_range("GET / HTTP/1.1\r\n", 1000), (0, 1000, false));
    }

    /// A fake ssh: drops the options and the destination, runs the command here.
    #[cfg(unix)]
    pub fn fake_ssh(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("fake-ssh");
        std::fs::write(
            &p,
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do case \"$1\" in -o|-F|-p|-l|-i|-J) shift 2;; -*) shift;; *) break;; esac; done\nshift\n[ \"$1\" = -- ] && shift\nexec sh -c \"$*\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn fake_ssh_ranged_parallel_fetch_priority_and_http() {
        let t = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("MOUSER_HOME", t.path().join("home"));
            std::env::set_var("MOUSER_SSH_BIN", fake_ssh(t.path()));
            std::env::set_var("MOUSER_REMOTE_BACKEND", "cat");
            std::env::remove_var("MOUSER_SSH_CONFIG");
        }
        let data: Vec<u8> = (0..(9u32 << 20)).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let src = t.path().join("big file.bin");
        std::fs::write(&src, &data).unwrap();
        let f = RemoteFile {
            remote: Remote::new(SshTarget { opts: vec![], dest: "fakehost".into() }),
            path: src.clone(),
            size: data.len() as u64,
            mtime: 1,
        };
        let h = open(&f, None);
        // Ask for the end right away: it comes before the middle.
        let end = f.size - 1000;
        h.prioritize(end);
        assert_eq!(h.read(end, f.size, Duration::from_secs(20)).unwrap(), &data[end as usize..]);
        assert!(h.wait(0, f.size, Duration::from_secs(30), &|| false), "{:?}", h.error());
        assert!(h.complete());
        assert_eq!(std::fs::read(h.cache_path()).unwrap(), data);
        let order = h.order();
        let pos_end = order.iter().position(|&o| o + CHUNK > end).unwrap();
        assert!(pos_end <= SEGMENTS + 1, "the prioritised range is among the first reads: {order:?}");
        // The HTTP server serves ranges.
        let url = h.url();
        let addr = url.trim_start_matches("http://").split('/').next().unwrap().to_string();
        let mut c = std::net::TcpStream::connect(addr).unwrap();
        c.write_all(b"GET /x HTTP/1.1\r\nRange: bytes=100-199\r\n\r\n").unwrap();
        let mut got = Vec::new();
        c.read_to_end(&mut got).unwrap();
        let body = &got[got.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4..];
        assert!(String::from_utf8_lossy(&got).starts_with("HTTP/1.1 206"));
        assert_eq!(body, &data[100..200]);
        // Unchanged (same key): a second open finds it complete, nothing read.
        drop(h);
        let h2 = open(&f, None);
        assert!(h2.complete());
        // Head-only handles stop at their limit.
        let g = RemoteFile { size: data.len() as u64, mtime: 2, ..f.clone() };
        let hh = open(&g, Some(4096));
        assert!(hh.wait(0, 4096, Duration::from_secs(10), &|| false));
        assert_eq!(hh.order()[0], 0, "without a seek the head comes first");
        std::thread::sleep(Duration::from_millis(300));
        assert!(hh.map().filled() <= HEAD + 2 * CHUNK, "{:?}", hh.map());
        // LRU: a tiny cap drops the unused entry, keeps the live ones.
        drop(h2);
        evict(1);
        assert!(hh.cache_path().exists());
    }
}
