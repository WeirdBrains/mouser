//! Commander mode (MODES.md Phase 1): a listing of the shell's working
//! directory shown above the prompt as a list, a grid or a carousel. This is
//! the model; the app builds the view from it and the renderer draws it.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What a file is, for its preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dir,
    Image,
    Video,
    Audio,
    Text,
    Other,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Dir => "folder",
            Kind::Image => "image",
            Kind::Video => "video",
            Kind::Audio => "audio",
            Kind::Text => "text",
            Kind::Other => "file",
        }
    }
}

/// Kind by name (directories are found from the metadata).
pub fn kind_of(name: &str) -> Kind {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => Kind::Image,
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" => Kind::Video,
        "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac" => Kind::Audio,
        "txt" | "md" | "rs" | "toml" | "json" | "yaml" | "yml" | "sh" | "py" | "js" | "ts" | "c" | "h" | "cpp" | "go" | "html" | "css" | "log"
        | "conf" | "ini" | "lock" | "xml" | "csv" => Kind::Text,
        _ if !name.contains('.') || name.starts_with('.') && !name[1..].contains('.') => Kind::Text,
        _ => Kind::Other,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
    /// `ls -l` style, e.g. `drwxr-xr-x`.
    pub mode: String,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    #[default]
    Name,
    Modified,
    Size,
}

/// `ls` flags: `-a`, `-h`, sort. Default `-lah` (all, human sizes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flags {
    pub all: bool,
    pub human: bool,
    pub sort: Sort,
    pub reverse: bool,
}

impl Default for Flags {
    fn default() -> Self {
        Self { all: true, human: true, sort: Sort::Name, reverse: false }
    }
}

impl Flags {
    /// The equivalent `ls` options, e.g. `-lah`.
    pub fn ls(&self) -> String {
        let mut s = String::from("-l");
        if self.all {
            s.push('a');
        }
        if self.human {
            s.push('h');
        }
        match self.sort {
            Sort::Name => {}
            Sort::Modified => s.push('t'),
            Sort::Size => s.push('S'),
        }
        if self.reverse {
            s.push('r');
        }
        s
    }
}

#[cfg(unix)]
pub fn mode_string(m: &std::fs::Metadata) -> String {
    use std::os::unix::fs::PermissionsExt;
    let p = m.permissions().mode();
    let t = if m.file_type().is_symlink() {
        'l'
    } else if m.is_dir() {
        'd'
    } else {
        '-'
    };
    let mut s = String::from(t);
    for (i, c) in "rwxrwxrwx".chars().enumerate() {
        s.push(if p & (1 << (8 - i)) != 0 { c } else { '-' });
    }
    s
}

#[cfg(not(unix))]
pub fn mode_string(m: &std::fs::Metadata) -> String {
    let d = if m.is_dir() { 'd' } else { '-' };
    let w = if m.permissions().readonly() { '-' } else { 'w' };
    format!("{d}r{w}-r--r--")
}

fn entry(name: String, path: PathBuf) -> Entry {
    let meta = std::fs::symlink_metadata(&path).ok();
    let target_dir = std::fs::metadata(&path).is_ok_and(|m| m.is_dir());
    let kind = if target_dir { Kind::Dir } else { kind_of(&name) };
    Entry {
        mode: meta.as_ref().map(mode_string).unwrap_or_else(|| "?---------".into()),
        size: meta.as_ref().map_or(0, |m| m.len()),
        modified: meta.as_ref().and_then(|m| m.modified().ok()),
        name,
        path,
        kind,
    }
}

/// The directory's entries like `ls` with `flags`. `.` and `..` are never
/// listed: going up is the back control (alt+p, alt+Backspace, the header).
pub fn list_dir(dir: &Path, flags: &Flags) -> std::io::Result<Vec<Entry>> {
    let v: Vec<Entry> = std::fs::read_dir(dir)?.flatten().map(|e| entry(e.file_name().to_string_lossy().into_owned(), e.path())).collect();
    Ok(arrange(v, flags))
}

/// Dotfiles (unless `-a`), sort and reverse, like `ls`.
pub fn arrange(v: Vec<Entry>, flags: &Flags) -> Vec<Entry> {
    let mut v: Vec<Entry> = v.into_iter().filter(|e| flags.all || !e.name.starts_with('.')).filter(|e| e.name != "." && e.name != "..").collect();
    match flags.sort {
        Sort::Name => v.sort_by_key(|e| e.name.to_lowercase()),
        Sort::Modified => v.sort_by_key(|e| std::cmp::Reverse(e.modified)),
        Sort::Size => v.sort_by_key(|e| std::cmp::Reverse(e.size)),
    }
    if flags.reverse {
        v.reverse();
    }
    v
}

/// `ls -h` size.
pub fn human_size(n: u64) -> String {
    let units = ["", "K", "M", "G", "T"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        n.to_string()
    } else if v < 10.0 {
        format!("{v:.1}{}", units[i])
    } else {
        format!("{v:.0}{}", units[i])
    }
}

/// `Oct 31 02:02` in local time.
pub fn format_time(t: SystemTime) -> String {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let (mon, day, h, m) = local_parts(secs);
    const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {day:2} {h:02}:{m:02}", M[mon.min(11)])
}

#[cfg(unix)]
fn local_parts(secs: i64) -> (usize, u32, u32, u32) {
    // SAFETY: localtime_r writes only into `tm`.
    unsafe {
        let t = secs as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        (tm.tm_mon as usize, tm.tm_mday as u32, tm.tm_hour as u32, tm.tm_min as u32)
    }
}

#[cfg(not(unix))]
fn local_parts(secs: i64) -> (usize, u32, u32, u32) {
    let dt = time::OffsetDateTime::from_unix_timestamp(secs).unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    (u8::from(dt.month()) as usize - 1, dt.day() as u32, dt.hour() as u32, dt.minute() as u32)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    List,
    Grid,
    Carousel,
}

impl View {
    pub fn next(self) -> Self {
        match self {
            View::List => View::Grid,
            View::Grid => View::Carousel,
            View::Carousel => View::List,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            View::List => "list",
            View::Grid => "grid",
            View::Carousel => "carousel",
        }
    }
}

/// Grid tile sizes (larger tiles: fewer per row).
pub const TILE_SIZES: [i32; 4] = [96, 128, 176, 240];

/// Commander's actions; each is a [`crate::modes::Binding`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SwitchView,
    Search,
    Up,
    Down,
    Left,
    Right,
    Open,
    Bigger,
    Smaller,
    Hidden,
    Options,
    Man,
    PageUp,
    PageDown,
    First,
    Last,
    Parent,
    Close,
    Mark,
    MarkAll,
    Pipe,
}

/// Default bindings. They avoid readline's alt keys (alt+b, alt+f, alt+.,
/// alt+d, alt+/ ...) except `t` (MODES.md), which takes transpose-words while
/// the mode is on; remap it in `modes.toml` if you need it.
pub const BINDINGS: &[crate::modes::Binding] = &[
    b("switch_view", "t", "switch view (list / grid / carousel)"),
    b("search", "s", "search this folder"),
    b("up", "up", "previous row"),
    b("down", "down", "next row"),
    b("left", "left", "previous item"),
    b("right", "right", "next item"),
    b("open", "enter", "open (folder: cd into it; file: full-width view)"),
    b("bigger", "z", "larger grid tiles"),
    b("smaller", "x", "smaller grid tiles"),
    b("hidden", "a", "show / hide dotfiles (ls -a)"),
    b("options", "o", "options window"),
    b("man", "m", "manual for commander mode"),
    b("page_up", "pageup", "page up"),
    b("page_down", "pagedown", "page down"),
    b("first", "home", "first item"),
    b("last", "end", "last item"),
    b("parent", "p", "back: parent folder (cd ..)"),
    b("back", "backspace", "back: parent folder (cd ..)"),
    b("close", "q", "close the file view / window"),
    b("mark", "space", "mark / unmark the item (for pipe)"),
    b("mark_all", "*", "mark all / clear marks"),
    b("pipe", "|", "pipe the marked (or selected) paths into a command"),
    b("play_pause", "k", "player: play / pause"),
    b("seek_back", "j", "player: back 10 s"),
    b("seek_forward", "l", "player: forward 10 s"),
    b("volume_down", "-", "player: volume down"),
    b("volume_up", "=", "player: volume up"),
    b("zoom_in", "=", "image view: zoom in (same key as volume up in the player)"),
    b("zoom_out", "-", "image view: zoom out (same key as volume down in the player)"),
    b("zoom_fit", "0", "image view: fit"),
    b("zoom_actual", "1", "image view: 100% (actual pixels)"),
    b("menu", "i", "header overflow menu (\u{2022}\u{2022}\u{2022}) when the window is narrow"),
];

const fn b(id: &'static str, key: &'static str, describe: &'static str) -> crate::modes::Binding {
    crate::modes::Binding { id, key, describe }
}

impl Action {
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "switch_view" => Action::SwitchView,
            "search" => Action::Search,
            "up" => Action::Up,
            "down" => Action::Down,
            "left" => Action::Left,
            "right" => Action::Right,
            "open" => Action::Open,
            "bigger" => Action::Bigger,
            "smaller" => Action::Smaller,
            "hidden" => Action::Hidden,
            "options" => Action::Options,
            "man" => Action::Man,
            "page_up" => Action::PageUp,
            "page_down" => Action::PageDown,
            "first" => Action::First,
            "last" => Action::Last,
            "parent" | "back" => Action::Parent,
            "close" => Action::Close,
            "mark" => Action::Mark,
            "mark_all" => Action::MarkAll,
            "pipe" => Action::Pipe,
            _ => return None,
        })
    }
}

/// Floating windows of the mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    Options,
    Man,
    /// Full-width view of a file.
    Preview(PathBuf),
}

/// What an action asks the app to do beyond changing the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Type `cd -- <path>` into the shell.
    Cd(PathBuf),
    /// Hand a file to the desktop's player/opener (video, audio).
    External(PathBuf),
    /// Run this command line in the shell, keeping what is typed (pipe).
    Run(String),
}

#[derive(Debug, Default)]
pub struct Commander {
    pub view: View,
    /// Index into [`TILE_SIZES`].
    pub tile: usize,
    pub flags: Flags,
    pub cwd: Option<PathBuf>,
    pub entries: Vec<Entry>,
    stamp: Option<(PathBuf, Option<SystemTime>, Flags)>,
    pub error: Option<String>,
    pub filter: String,
    pub searching: bool,
    /// Index into [`Commander::visible`].
    pub selected: usize,
    /// First list row shown.
    pub scroll: usize,
    pub overlay: Option<Overlay>,
    /// Scroll of the man page / text preview.
    pub overlay_scroll: usize,
    /// Items per page (set by the view each frame).
    pub page: usize,
    /// Marked paths (multi-select for pipe).
    pub marks: std::collections::BTreeSet<PathBuf>,
    /// The pipe prompt has focus; its text.
    pub piping: bool,
    pub pipe_cmd: String,
    /// `user@host` while the shell is on another machine (entries are remote
    /// paths; files are not read locally).
    pub remote: Option<String>,
    /// A one-line message in the header (e.g. why a remote file did not open).
    pub notice: Option<String>,
    /// The remote listing as received (re-arranged when the flags change).
    remote_raw: Vec<Entry>,
}

impl Commander {
    pub fn new() -> Self {
        Self { tile: 1, ..Default::default() }
    }

    /// Leaving the mode: back to a plain state (the directory is kept).
    pub fn reset(&mut self) {
        self.searching = false;
        self.filter.clear();
        self.overlay = None;
        self.overlay_scroll = 0;
        self.piping = false;
        self.pipe_cmd.clear();
    }

    /// Follow the shell to `dir`; re-list when it, its mtime or the flags change.
    pub fn set_cwd(&mut self, dir: Option<PathBuf>) {
        let Some(dir) = dir else { return };
        let mtime = std::fs::metadata(&dir).and_then(|m| m.modified()).ok();
        let stamp = (dir.clone(), mtime, self.flags);
        if self.remote.is_some() {
            self.remote = None;
            self.stamp = None;
        }
        if self.stamp.as_ref() == Some(&stamp) {
            return;
        }
        let listing = list_dir(&dir, &self.flags).map_err(|e| format!("{}: {e}", dir.display()));
        self.load(dir, listing);
        self.stamp = Some(stamp);
    }

    /// The shell is on `label` (user@host) in `dir` (`None`: unknown), with
    /// this listing (`None`: still loading, `Some(Err)`: failed). `.` and `..`
    /// are dropped; the flags (`-a`, sort, reverse) apply as locally.
    pub fn set_remote(&mut self, label: String, dir: Option<PathBuf>, listing: Option<Result<Vec<Entry>, String>>) {
        let label_changed = self.remote.as_ref() != Some(&label);
        self.remote = Some(label.clone());
        self.stamp = None;
        let Some(dir) = dir else {
            self.entries.clear();
            if label_changed || self.cwd.is_some() {
                self.marks.clear();
                self.selected = 0;
            }
            self.cwd = None;
            self.error = Some(format!(
                "remote ({label}): no directory info. The remote shell does not report its folder (OSC 7) or there is no ssh side channel; see docs/modes.md"
            ));
            return;
        };
        let listing = match listing {
            Some(r) => r.map(|v| {
                self.remote_raw = v.clone();
                arrange(v, &self.flags)
            }),
            None if self.cwd.as_ref() == Some(&dir) && self.error.is_none() && !label_changed => return,
            None => Ok(Vec::new()),
        };
        self.load(dir, listing);
    }

    /// List again after the flags changed (local: re-read; remote: re-sort).
    pub fn relist(&mut self) {
        if self.remote.is_some() {
            if let Some(d) = self.cwd.clone()
                && self.error.is_none()
            {
                let v = arrange(self.remote_raw.clone(), &self.flags);
                self.load(d, Ok(v));
            }
        } else {
            let d = self.cwd.clone();
            self.set_cwd(d);
        }
    }

    fn load(&mut self, dir: PathBuf, listing: Result<Vec<Entry>, String>) {
        let came_from =
            self.cwd.as_ref().filter(|old| **old != dir).and_then(|old| {
                old.strip_prefix(&dir).ok().and_then(|r| r.components().next()).map(|c| c.as_os_str().to_string_lossy().into_owned())
            });
        if self.cwd.as_ref() != Some(&dir) {
            self.selected = 0;
            self.scroll = 0;
            self.filter.clear();
            self.marks.clear();
        }
        match listing {
            Ok(v) => {
                self.entries = v;
                self.error = None;
            }
            Err(e) => {
                self.entries.clear();
                self.error = Some(e);
            }
        }
        // Going up selects the folder you came from.
        if let Some(name) = came_from
            && let Some(i) = self.visible().iter().position(|e| e.name == name)
        {
            self.selected = i;
        }
        self.cwd = Some(dir);
        self.clamp();
    }

    /// Entries matching the search (case-insensitive substring).
    pub fn visible(&self) -> Vec<&Entry> {
        let f = self.filter.to_lowercase();
        self.entries.iter().filter(|e| f.is_empty() || e.name.to_lowercase().contains(&f)).collect()
    }

    fn clamp(&mut self) {
        let n = self.visible().len();
        self.selected = self.selected.min(n.saturating_sub(1));
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.visible().get(self.selected).copied()
    }

    /// Keep the selected list row inside `rows` visible rows.
    pub fn follow(&mut self, rows: usize) {
        let rows = rows.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
    }

    /// Typed text while the pipe prompt has focus: Enter runs, Esc cancels.
    pub fn pipe_key(&mut self, bytes: &[u8]) -> Option<Effect> {
        if !self.piping {
            return None;
        }
        match bytes {
            b"\r" => {
                self.piping = false;
                let cmd = std::mem::take(&mut self.pipe_cmd);
                if cmd.trim().is_empty() {
                    return Some(Effect::None);
                }
                return Some(Effect::Run(pipe_line(&self.pipe_paths(), &cmd)));
            }
            b"\x1b" => {
                self.piping = false;
                self.pipe_cmd.clear();
            }
            b"\x7f" | b"\x08" => {
                self.pipe_cmd.pop();
            }
            _ => {
                if let Ok(t) = std::str::from_utf8(bytes)
                    && !t.chars().any(char::is_control)
                {
                    self.pipe_cmd.push_str(t);
                }
            }
        }
        Some(Effect::None)
    }

    /// Marked paths in listing order, else the selected one.
    pub fn pipe_paths(&self) -> Vec<PathBuf> {
        if self.marks.is_empty() {
            self.selected_entry().map(|e| e.path.clone()).into_iter().collect()
        } else {
            let v: Vec<PathBuf> = self.entries.iter().filter(|e| self.marks.contains(&e.path)).map(|e| e.path.clone()).collect();
            v
        }
    }

    /// Typed text while the search field has focus; false when not searching.
    pub fn search_key(&mut self, bytes: &[u8]) -> bool {
        if !self.searching {
            return false;
        }
        match bytes {
            b"\r" | b"\x1b" => self.searching = false,
            b"\x7f" | b"\x08" => {
                self.filter.pop();
            }
            _ => {
                if let Ok(s) = std::str::from_utf8(bytes)
                    && !s.starts_with('\x1b')
                    && !s.chars().any(char::is_control)
                {
                    self.filter.push_str(s);
                    self.selected = 0;
                    self.scroll = 0;
                }
            }
        }
        self.clamp();
        true
    }

    /// Do `a`; `per_row` is the grid's items per row (1 in the list).
    pub fn apply(&mut self, a: Action, per_row: usize) -> Effect {
        self.notice = None;
        let n = self.visible().len();
        let per_row = if self.view == View::Grid { per_row.max(1) } else { 1 };
        match a {
            Action::SwitchView => self.view = self.view.next(),
            Action::Search => self.searching = !self.searching,
            Action::Up => self.selected = self.selected.saturating_sub(per_row),
            Action::Down => {
                if self.selected + per_row < n {
                    self.selected += per_row
                }
            }
            Action::Left => self.selected = self.selected.saturating_sub(1),
            Action::Right => self.selected = (self.selected + 1).min(n.saturating_sub(1)),
            Action::Bigger => self.tile = (self.tile + 1).min(TILE_SIZES.len() - 1),
            Action::Smaller => self.tile = self.tile.saturating_sub(1),
            Action::Hidden => self.flags.all = !self.flags.all,
            Action::Options => self.overlay = if self.overlay == Some(Overlay::Options) { None } else { Some(Overlay::Options) },
            Action::Man => {
                self.overlay = if self.overlay == Some(Overlay::Man) { None } else { Some(Overlay::Man) };
                self.overlay_scroll = 0;
            }
            Action::Open => return self.open_selected(),
            Action::PageUp => self.selected = self.selected.saturating_sub(self.page.max(1)),
            Action::PageDown => self.selected = (self.selected + self.page.max(1)).min(n.saturating_sub(1)),
            Action::First => self.selected = 0,
            Action::Last => self.selected = n.saturating_sub(1),
            Action::Parent => {
                if let Some(p) = self.cwd.as_ref().and_then(|d| d.parent()) {
                    return Effect::Cd(p.to_path_buf());
                }
            }
            Action::Close => {
                self.overlay = None;
                self.piping = false;
            }
            Action::Mark => {
                if let Some(e) = self.selected_entry().map(|e| e.path.clone())
                    && !self.marks.remove(&e)
                {
                    self.marks.insert(e);
                }
                if self.view != View::Grid || per_row == 1 {
                    self.selected = (self.selected + 1).min(n.saturating_sub(1));
                }
            }
            Action::MarkAll => {
                if self.marks.is_empty() {
                    let all: Vec<PathBuf> = self.visible().iter().map(|e| e.path.clone()).collect();
                    self.marks.extend(all);
                } else {
                    self.marks.clear();
                }
            }
            Action::Pipe => {
                self.piping = !self.piping;
                self.searching = false;
            }
        }
        self.clamp();
        Effect::None
    }

    pub fn open_selected(&mut self) -> Effect {
        let Some(e) = self.selected_entry().cloned() else { return Effect::None };
        match e.kind {
            Kind::Dir => Effect::Cd(e.path),
            // Remote files open like local ones (fetched by `fetch`).
            Kind::Video | Kind::Audio => Effect::External(e.path),
            _ => {
                self.overlay = Some(Overlay::Preview(e.path));
                self.overlay_scroll = 0;
                Effect::None
            }
        }
    }

    /// Items whose previews should be loaded now: the visible grid tiles, or
    /// the carousel's three (focused + its two neighbours). Everything else
    /// is not loaded.
    pub fn wanted_previews(&self, first: usize, count: usize) -> Vec<PathBuf> {
        let vis = self.visible();
        let range = match self.view {
            View::List => 0..0,
            View::Grid => first.min(vis.len())..(first + count).min(vis.len()),
            View::Carousel => self.selected.saturating_sub(1)..(self.selected + 2).min(vis.len()),
        };
        vis[range].iter().filter(|e| e.kind == Kind::Image).map(|e| e.path.clone()).collect()
    }
}

/// POSIX single-quoting (`'` becomes `'\\''`).
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `printf '%s\n' 'a' 'b' | cmd` (run with [`crate::modes::keep_line`]).
pub fn pipe_line(paths: &[PathBuf], cmd: &str) -> String {
    let args: Vec<String> = paths.iter().map(|p| shell_quote(&p.to_string_lossy())).collect();
    format!("printf '%s\\n' {} | {}", args.join(" "), cmd.trim())
}

/// The manual shown by the mode's `man` button.
pub fn manual(k: &crate::modes::Keymap) -> Vec<String> {
    let mut v = vec![
        "COMMANDER MODE (the default mode)".to_string(),
        String::new(),
        "Shows the shell's working directory above the prompt and follows every cd.".into(),
        "Keep typing commands as usual: only the keys below are taken, every other".into(),
        "key (other alt keys too) goes to the shell. esc : c turns modes off / on.".into(),
        String::new(),
        "KEYS".into(),
    ];
    for (id, _, d) in &k.keys {
        v.push(format!("  {:<12} {d}", k.label(id)));
    }
    v.extend([
        String::new(),
        "  ctrl+shift+k  key mappings     ctrl+shift+m  modes".into(),
        String::new(),
        "REMAPPING".into(),
        format!("  {}:", crate::modes::modes_path().display()),
        "    [commander]".into(),
        "    modifier = \"alt\"".into(),
        "    [commander.keys]".into(),
        "    switch_view = \"v\"".into(),
    ]);
    v
}

/// A text file for the full-width view: up to 5000 lines of the first
/// 1 MiB; `(lines, error)`.
/// Lines of `b` for the file view (`more`: there is more after it).
pub fn text_from_bytes(b: &[u8], more: bool, name: &str) -> (Vec<String>, Option<String>) {
    if b.contains(&0) {
        return (Vec::new(), Some(format!("{name}: binary file")));
    }
    let mut v: Vec<String> = String::from_utf8_lossy(b).lines().take(5000).map(String::from).collect();
    if more || v.len() == 5000 {
        v.push("[…]".into());
    }
    (v, None)
}

pub fn text_preview(p: &Path) -> (Vec<String>, Option<String>) {
    const MAX: u64 = 1 << 20;
    let read = std::fs::File::open(p).and_then(|f| {
        let mut b = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(f, MAX), &mut b)?;
        Ok(b)
    });
    match read {
        Ok(b) if b.contains(&0) => (Vec::new(), Some(format!("{}: binary file", p.display()))),
        Ok(b) => {
            let mut v: Vec<String> = String::from_utf8_lossy(&b).lines().take(5000).map(String::from).collect();
            if b.len() as u64 >= MAX || v.len() == 5000 {
                v.push("[…]".into());
            }
            (v, None)
        }
        Err(e) => (Vec::new(), Some(format!("{}: {e}", p.display()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("sub")).unwrap();
        std::fs::write(t.path().join("b.png"), b"x").unwrap();
        std::fs::write(t.path().join("a.txt"), b"hello").unwrap();
        std::fs::write(t.path().join(".hidden"), b"").unwrap();
        std::fs::write(t.path().join("clip.mp4"), vec![0; 3000]).unwrap();
        t
    }

    #[test]
    fn lists_like_ls_lah() {
        let t = tree();
        let v = list_dir(t.path(), &Flags::default()).unwrap();
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, [".hidden", "a.txt", "b.png", "clip.mp4", "sub"], "no . or .. even with -a");
        assert_eq!(v[4].kind, Kind::Dir);
        assert!(v[4].mode.starts_with('d'));
        assert_eq!((v[2].kind, v[3].kind, v[1].kind), (Kind::Image, Kind::Video, Kind::Text));
        let no_dot = list_dir(t.path(), &Flags { all: false, ..Default::default() }).unwrap();
        assert!(no_dot.iter().all(|e| !e.name.starts_with('.')));
        let by_size = list_dir(t.path(), &Flags { all: false, sort: Sort::Size, ..Default::default() }).unwrap();
        assert_eq!(by_size.iter().find(|e| e.kind != Kind::Dir).unwrap().name, "clip.mp4");
        assert_eq!(Flags::default().ls(), "-lah");
        assert_eq!(human_size(1536), "1.5K");
        assert_eq!(human_size(900), "900");
        assert_eq!(format_time(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(86400 * 40)).len(), 12);
    }

    #[test]
    fn follows_cwd_search_and_navigation() {
        let t = tree();
        let mut c = Commander::new();
        c.set_cwd(Some(t.path().to_path_buf()));
        assert_eq!(c.visible().len(), 5);
        c.searching = true;
        for b in [&b"p"[..], b"n", b"g"] {
            assert!(c.search_key(b));
        }
        assert_eq!(c.visible().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["b.png"]);
        c.search_key(b"\x7f");
        c.search_key(b"\x7f");
        assert_eq!(c.filter, "p");
        c.search_key(b"\r");
        assert!(!c.searching && !c.search_key(b"q"));
        c.filter.clear();
        c.apply(Action::Down, 1);
        assert_eq!(c.selected, 1);
        c.view = View::Grid;
        c.apply(Action::Down, 3);
        assert_eq!(c.selected, 4);
        c.apply(Action::Down, 3);
        assert_eq!(c.selected, 4, "no row below");
        c.apply(Action::Up, 3);
        assert_eq!(c.selected, 1);
        c.apply(Action::SwitchView, 1);
        assert_eq!(c.view, View::Carousel);
        c.apply(Action::SwitchView, 1);
        assert_eq!(c.view, View::List);
        // A new directory re-lists and resets the selection.
        c.set_cwd(Some(t.path().join("sub")));
        assert_eq!((c.selected, c.visible().len()), (0, 0), "empty folder: no . or ..");
        std::fs::write(t.path().join("sub/.dot"), b"").unwrap();
        c.set_cwd(Some(t.path().join("sub")));
        assert_eq!(c.visible().len(), 1);
        c.apply(Action::Hidden, 1);
        c.set_cwd(Some(t.path().join("sub")));
        assert_eq!(c.visible().len(), 0);
    }

    #[test]
    fn open_by_kind() {
        let t = tree();
        let mut c = Commander::new();
        c.set_cwd(Some(t.path().to_path_buf()));
        let at = |c: &Commander, n: &str| c.visible().iter().position(|e| e.name == n).unwrap();
        c.selected = at(&c, "sub");
        assert_eq!(c.apply(Action::Open, 1), Effect::Cd(t.path().join("sub")));
        c.selected = at(&c, "clip.mp4");
        assert_eq!(c.apply(Action::Open, 1), Effect::External(t.path().join("clip.mp4")));
        c.selected = at(&c, "b.png");
        assert_eq!(c.apply(Action::Open, 1), Effect::None);
        assert_eq!(c.overlay, Some(Overlay::Preview(t.path().join("b.png"))));
    }

    #[test]
    fn previews_only_for_what_is_visible() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..10 {
            std::fs::write(t.path().join(format!("p{i}.png")), b"x").unwrap();
        }
        let mut c = Commander::new();
        c.flags.all = false;
        c.set_cwd(Some(t.path().to_path_buf()));
        assert!(c.wanted_previews(0, 10).is_empty(), "list view loads none");
        c.view = View::Grid;
        let w = c.wanted_previews(4, 3);
        assert_eq!(w, (4..7).map(|i| t.path().join(format!("p{i}.png"))).collect::<Vec<_>>());
        c.view = View::Carousel;
        c.selected = 5;
        assert_eq!(c.wanted_previews(0, 99), (4..7).map(|i| t.path().join(format!("p{i}.png"))).collect::<Vec<_>>(), "focused + two neighbours");
        c.selected = 0;
        assert_eq!(c.wanted_previews(0, 99).len(), 2, "first item has one neighbour");
    }

    #[test]
    fn bindings_cover_every_action() {
        for b in BINDINGS.iter().filter(|b| {
            !matches!(
                b.id,
                "play_pause"
                    | "seek_back"
                    | "seek_forward"
                    | "volume_down"
                    | "volume_up"
                    | "zoom_in"
                    | "zoom_out"
                    | "zoom_fit"
                    | "zoom_actual"
                    | "menu"
            )
        }) {
            assert!(Action::from_id(b.id).is_some(), "{}", b.id);
        }
        let k = crate::modes::Keymap::new(BINDINGS);
        assert!(manual(&k).iter().any(|l| l.contains("alt+t") && l.contains("switch view")));
    }

    #[test]
    fn paging_parent_marks_and_pipe() {
        let t = tree();
        let mut c = Commander::new();
        c.set_cwd(Some(t.path().to_path_buf()));
        c.page = 3;
        c.apply(Action::PageDown, 1);
        assert_eq!(c.selected, 3);
        c.apply(Action::Last, 1);
        assert_eq!(c.selected, 4);
        c.apply(Action::PageUp, 1);
        assert_eq!(c.selected, 1);
        c.apply(Action::First, 1);
        assert_eq!(c.selected, 0);
        assert_eq!(c.apply(Action::Parent, 1), Effect::Cd(t.path().parent().unwrap().to_path_buf()));
        // No marks: the selection is piped.
        c.selected = 1;
        c.apply(Action::Pipe, 1);
        for b in [&b"w"[..], b"c", b" ", b"-", b"l"] {
            c.pipe_key(b);
        }
        let Some(Effect::Run(line)) = c.pipe_key(b"\r") else { panic!() };
        assert_eq!(line, format!("printf '%s\\n' '{}' | wc -l", t.path().join("a.txt").display()));
        // Marks: several files, in listing order (no . or .. to skip any more).
        c.selected = 0;
        c.apply(Action::Mark, 1);
        assert_eq!(c.marks.len(), 1, "the first entry is a real file");
        c.selected = 0;
        c.apply(Action::Mark, 1);
        assert!(c.marks.is_empty());
        c.selected = 2;
        c.apply(Action::Mark, 1);
        c.selected = 1;
        c.apply(Action::Mark, 1);
        assert_eq!(c.pipe_paths(), [t.path().join("a.txt"), t.path().join("b.png")]);
        c.apply(Action::MarkAll, 1);
        assert!(c.marks.is_empty(), "mark-all with marks clears them");
        c.apply(Action::MarkAll, 1);
        assert_eq!(c.marks.len(), 5);
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        c.apply(Action::Pipe, 1);
        c.pipe_key(b"x");
        assert_eq!(c.pipe_key(b"\x1b"), Some(Effect::None));
        assert!(!c.piping && c.pipe_cmd.is_empty());
        c.set_cwd(Some(t.path().join("sub")));
        assert!(c.marks.is_empty(), "marks are per folder");
    }

    #[test]
    fn back_selects_the_folder_you_came_from() {
        let t = tree();
        std::fs::create_dir_all(t.path().join("sub/deep/er")).unwrap();
        let mut c = Commander::new();
        c.set_cwd(Some(t.path().join("sub/deep/er")));
        assert_eq!(c.apply(Action::Parent, 1), Effect::Cd(t.path().join("sub/deep")));
        // The shell went up: the folder we left is selected.
        c.set_cwd(Some(t.path().join("sub")));
        assert_eq!(c.selected_entry().unwrap().name, "deep");
        c.set_cwd(Some(t.path().to_path_buf()));
        assert_eq!(c.selected_entry().unwrap().name, "sub");
        // Going down starts at the top.
        c.set_cwd(Some(t.path().join("sub")));
        assert_eq!(c.selected, 0);
        // alt+p and alt+Backspace are both back, both remappable.
        let k = crate::modes::Keymap::new(BINDINGS);
        assert!(manual(&k).iter().any(|l| l.contains("alt+backspace") && l.contains("back")), "{:?}", manual(&k));
        assert_eq!(Action::from_id("back"), Some(Action::Parent));
        assert_eq!(crate::modes::key_bytes("backspace").unwrap()[0], b"\x1b\x7f");
        // At / there is no parent.
        c.set_cwd(Some(PathBuf::from("/")));
        assert_eq!(c.apply(Action::Parent, 1), Effect::None);
    }

    #[test]
    fn grid_moves_in_2d_and_carousel_by_item() {
        let t = tree();
        let mut c = Commander::new();
        c.set_cwd(Some(t.path().to_path_buf()));
        c.view = View::Grid;
        c.apply(Action::Right, 3);
        c.apply(Action::Down, 3);
        assert_eq!(c.selected, 4);
        c.apply(Action::Left, 3);
        c.apply(Action::Up, 3);
        assert_eq!(c.selected, 0);
        c.view = View::Carousel;
        c.apply(Action::Right, 3);
        c.apply(Action::Right, 3);
        assert_eq!(c.selected, 2);
        c.apply(Action::Left, 3);
        assert_eq!(c.selected, 1);
    }
}

#[cfg(test)]
mod remote_tests {
    use super::*;

    fn e(name: &str, dir: bool) -> Entry {
        Entry {
            name: name.into(),
            path: PathBuf::from(format!("/srv/{name}")),
            kind: if dir { Kind::Dir } else { kind_of(name) },
            mode: if dir { "drwxr-xr-x".into() } else { "-rw-r--r--".into() },
            size: 1,
            modified: None,
        }
    }

    #[test]
    fn remote_listing_unknown_dir_and_back_to_local() {
        let mut c = Commander::new();
        c.set_remote("me@web1".into(), None, None);
        assert!(c.entries.is_empty() && c.error.as_deref().unwrap().starts_with("remote (me@web1): no directory info"));
        c.set_remote("me@web1".into(), Some("/srv".into()), None);
        assert!(c.entries.is_empty() && c.error.is_none(), "loading: nothing stale");
        c.set_remote("me@web1".into(), Some("/srv".into()), Some(Ok(vec![e("b.png", false), e(".env", false), e("app", true)])));
        assert_eq!(c.visible().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), [".env", "app", "b.png"]);
        c.flags.all = false;
        c.relist();
        assert_eq!(c.visible().len(), 2, "flags re-sort the remote listing, no local read");
        // Folders cd (over the PTY), files open in the viewer like local ones.
        c.selected = 0;
        assert_eq!(c.open_selected(), Effect::Cd("/srv/app".into()));
        c.selected = 1;
        assert_eq!(c.open_selected(), Effect::None);
        assert_eq!(c.overlay, Some(Overlay::Preview("/srv/b.png".into())));
        c.overlay = None;
        // Up on the remote side selects where we came from.
        c.set_remote("me@web1".into(), Some("/srv/app/x".into()), Some(Ok(vec![])));
        c.set_remote("me@web1".into(), Some("/srv/app".into()), None);
        c.set_remote("me@web1".into(), Some("/srv".into()), Some(Ok(vec![e("app", true), e("zz", true)])));
        assert_eq!(c.selected_entry().unwrap().name, "app");
        // Back home: local listing again.
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("local.txt"), b"").unwrap();
        c.set_cwd(Some(t.path().to_path_buf()));
        assert!(c.remote.is_none());
        assert_eq!(c.visible()[0].name, "local.txt");
    }
}
