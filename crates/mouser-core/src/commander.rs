//! What Mouser itself still needs of commander now that the mode is the
//! `mouser-commander` plugin (1.4.0): directory listings and entries (for
//! the remote listing and the mode protocol's `shell.remote_changed`), file
//! kinds and badges, shell quoting, and the default key bindings (the
//! defaults for `modes.toml` and the shell-collision guard test).

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

/// The small type badge on a grid tile or carousel item (docs/modes.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Badge {
    Folder,
    Image,
    Video,
    Audio,
    Text,
    Code,
    Markdown,
    Pdf,
    Archive,
    Binary,
    Config,
    Font,
    Document,
    Generic,
}

impl Badge {
    pub const ALL: [Badge; 14] = [
        Badge::Folder,
        Badge::Image,
        Badge::Video,
        Badge::Audio,
        Badge::Text,
        Badge::Code,
        Badge::Markdown,
        Badge::Pdf,
        Badge::Archive,
        Badge::Binary,
        Badge::Config,
        Badge::Font,
        Badge::Document,
        Badge::Generic,
    ];

    /// The short mark drawn on the badge (at most 3 characters; the folder
    /// is drawn as a shape instead).
    pub fn mark(self) -> &'static str {
        match self {
            Badge::Folder => "",
            Badge::Image => "IMG",
            Badge::Video => "VID",
            Badge::Audio => "AUD",
            Badge::Text => "TXT",
            Badge::Code => "</>",
            Badge::Markdown => "MD",
            Badge::Pdf => "PDF",
            Badge::Archive => "ZIP",
            Badge::Binary => "BIN",
            Badge::Config => "{}",
            Badge::Font => "Aa",
            Badge::Document => "DOC",
            Badge::Generic => "…",
        }
    }

    /// Badge colour (0xRRGGBB), one per type.
    pub fn color(self) -> u32 {
        match self {
            Badge::Folder => 0x4F9DDE,
            Badge::Image => 0x3FB27F,
            Badge::Video => 0xE0566B,
            Badge::Audio => 0xB46EE0,
            Badge::Text => 0x8A96A3,
            Badge::Code => 0x2FB8C9,
            Badge::Markdown => 0x5B7BE0,
            Badge::Pdf => 0xD9443A,
            Badge::Archive => 0xC9902B,
            Badge::Binary => 0x6B7280,
            Badge::Config => 0xE0A93A,
            Badge::Font => 0xD0679E,
            Badge::Document => 0x3A7BD5,
            Badge::Generic => 0x7C8794,
        }
    }

    pub fn index(self) -> u8 {
        Badge::ALL.iter().position(|b| *b == self).unwrap_or(13) as u8
    }

    pub fn from_index(i: u8) -> Badge {
        Badge::ALL.get(i as usize).copied().unwrap_or(Badge::Generic)
    }
}

/// The badge for an entry: from its extension (the same names [`kind_of`]
/// knows, plus documents, archives, fonts, PDFs), else its kind; files with
/// an execute bit and no known type are binaries.
pub fn badge_of(name: &str, kind: Kind, mode: &str) -> Badge {
    if kind == Kind::Dir {
        return Badge::Folder;
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let by_ext = match ext.as_str() {
        "md" | "markdown" => Some(Badge::Markdown),
        "pdf" => Some(Badge::Pdf),
        "zip" | "tar" | "gz" | "tgz" | "xz" | "zst" | "bz2" | "7z" | "rar" | "deb" | "rpm" => Some(Badge::Archive),
        "json" | "toml" | "yaml" | "yml" | "ini" | "conf" | "cfg" | "lock" | "xml" | "env" => Some(Badge::Config),
        "ttf" | "otf" | "woff" | "woff2" => Some(Badge::Font),
        "doc" | "docx" | "odt" | "rtf" | "xls" | "xlsx" | "ods" | "csv" | "ppt" | "pptx" | "odp" => Some(Badge::Document),
        "rs" | "sh" | "py" | "js" | "ts" | "c" | "h" | "cpp" | "go" | "html" | "css" | "java" | "rb" | "lua" | "zig" | "swift" | "kt" => {
            Some(Badge::Code)
        }
        "exe" | "dll" | "so" | "dylib" | "bin" | "o" | "a" | "wasm" => Some(Badge::Binary),
        _ => None,
    };
    if let Some(b) = by_ext {
        return b;
    }
    let exec = mode.len() == 10 && mode[1..].contains('x');
    match kind {
        Kind::Image => Badge::Image,
        Kind::Video => Badge::Video,
        Kind::Audio => Badge::Audio,
        _ if exec => Badge::Binary,
        Kind::Text => Badge::Text,
        _ => Badge::Generic,
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

/// Default bindings: plain keys, live only while commander has the keyboard
/// (the focus toggle, `mouser_core::modes::DEFAULT_FOCUS_KEY`); otherwise every
/// key goes to the shell. Remap them in `modes.toml`.
pub const BINDINGS: &[crate::modes::Binding] = &[
    b("switch_view", "v", "switch view (list / grid / carousel)"),
    b("search", "/", "search this folder"),
    b("sync", "s", "sync: move the shell to commander's folder (silently, at its next prompt if busy)"),
    b("up", "up", "previous row"),
    b("down", "down", "next row"),
    b("left", "left", "previous item"),
    b("right", "right", "next item"),
    b("open", "enter", "open (folder: cd into it; file: full-width view)"),
    b("bigger", "z", "larger grid tiles"),
    b("smaller", "x", "smaller grid tiles"),
    b("hidden", "h", "show / hide dotfiles (ls -a)"),
    b("options", "o", "options window"),
    b("man", "m", "manual for commander mode"),
    b("page_up", "pageup", "page up"),
    b("page_down", "pagedown", "page down"),
    b("first", "home", "first item"),
    b("last", "end", "last item"),
    b("parent", "u", "back: where you came from, else the parent folder"),
    b("back", "backspace", "back: where you came from, else the parent folder"),
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
    b("fullscreen", "f", "image / player: fullscreen, the whole window (Esc or the same key leaves)"),
    b("menu", "i", "header overflow menu (\u{2022}\u{2022}\u{2022}) when the window is narrow"),
];

const fn b(id: &'static str, key: &'static str, describe: &'static str) -> crate::modes::Binding {
    crate::modes::Binding { id, key, describe }
}

/// POSIX single-quoting (`'` becomes `'\\''`).
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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
}

#[cfg(test)]
mod badge_tests {
    use super::*;

    #[test]
    fn every_type_has_its_badge() {
        let b = |n: &str| badge_of(n, kind_of(n), "-rw-r--r--");
        assert_eq!(badge_of("src", Kind::Dir, "drwxr-xr-x"), Badge::Folder);
        for (n, want) in [
            ("a.png", Badge::Image),
            ("a.mkv", Badge::Video),
            ("a.flac", Badge::Audio),
            ("notes.txt", Badge::Text),
            ("main.rs", Badge::Code),
            ("README.md", Badge::Markdown),
            ("paper.PDF", Badge::Pdf),
            ("x.tar.gz", Badge::Archive),
            ("app.exe", Badge::Binary),
            ("Cargo.toml", Badge::Config),
            ("a.json", Badge::Config),
            ("Inter.ttf", Badge::Font),
            ("sheet.xlsx", Badge::Document),
            ("a.csv", Badge::Document),
            ("thing.qqq", Badge::Generic),
        ] {
            assert_eq!(b(n), want, "{n}");
        }
        // An executable with no known type is a binary; a plain one is text.
        assert_eq!(badge_of("mouser", kind_of("mouser"), "-rwxr-xr-x"), Badge::Binary);
        assert_eq!(badge_of("Makefile", kind_of("Makefile"), "-rw-r--r--"), Badge::Text);
        for x in Badge::ALL {
            assert_eq!(Badge::from_index(x.index()), x);
            assert!(x.mark().chars().count() <= 3);
        }
    }
}
