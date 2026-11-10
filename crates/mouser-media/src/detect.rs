//! Media named in text: Markdown images and links (`![alt](/a/b.png)`,
//! `[clip](file:///a/b.mp4)`) and bare local paths (`/tmp/out.wav`,
//! `~/Pictures/cat.jpg`) with a picture, video or sound extension. Used by
//! the chat to show players under messages and tool output; any view that
//! shows text can use it.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Image,
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MediaRef {
    pub path: PathBuf,
    pub kind: Kind,
}

const IMAGE: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];
const VIDEO: &[&str] = &["mp4", "m4v", "mkv", "webm", "mov", "avi", "ogv"];
const AUDIO: &[&str] = &["mp3", "wav", "flac", "ogg", "oga", "opus", "m4a", "aac"];

/// The kind of `path` by its extension.
pub fn kind_of(path: &Path) -> Option<Kind> {
    let e = path.extension()?.to_str()?.to_ascii_lowercase();
    if IMAGE.contains(&e.as_str()) {
        Some(Kind::Image)
    } else if VIDEO.contains(&e.as_str()) {
        Some(Kind::Video)
    } else if AUDIO.contains(&e.as_str()) {
        Some(Kind::Audio)
    } else {
        None
    }
}

/// A local path from a link target or word: absolute, `~/…` (with `home`) or
/// `file://…`. Web URLs and relative paths are not media here.
fn local(target: &str, home: Option<&Path>) -> Option<PathBuf> {
    let t = target.trim();
    let t = t.strip_prefix("file://").unwrap_or(t);
    if let Some(rest) = t.strip_prefix("~/") {
        return home.map(|h| h.join(rest));
    }
    let p = PathBuf::from(percent_decode(t));
    (p.is_absolute() && !t.contains("://")).then_some(p)
}

fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The media in `text`, in order, each once. `home` expands `~/`.
pub fn find(text: &str, home: Option<&Path>) -> Vec<MediaRef> {
    let mut out: Vec<MediaRef> = Vec::new();
    let mut add = |p: PathBuf| {
        if let Some(kind) = kind_of(&p)
            && !out.iter().any(|m| m.path == p)
        {
            out.push(MediaRef { path: p, kind });
        }
    };
    // Markdown link targets: `](target)` or `](<target with spaces>)`.
    let mut rest = text;
    while let Some(i) = rest.find("](") {
        let after = &rest[i + 2..];
        let (target, len) = if let Some(a) = after.strip_prefix('<') {
            match a.find('>') {
                Some(j) => (&a[..j], j + 2),
                None => break,
            }
        } else {
            match after.find(')') {
                // A title after the target: `(path "title")`.
                Some(j) => (after[..j].split_whitespace().next().unwrap_or(""), j + 1),
                None => break,
            }
        };
        if let Some(p) = local(target, home) {
            add(p);
        }
        rest = &after[len.min(after.len())..];
    }
    // Bare words.
    for w in text.split(|c: char| c.is_whitespace() || c == '`' || c == '"' || c == '\'' || c == '<' || c == '>') {
        let w = w.trim_start_matches(['(', '[']).trim_end_matches([')', ']', ',', ';', ':', '.', '!', '?']);
        if w.contains("](") {
            continue;
        }
        if let Some(p) = local(w, home) {
            add(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Unix paths: `/tmp/…` is not absolute on Windows (no drive).
    #[cfg(unix)]
    #[test]
    fn finds_markdown_images_links_and_bare_paths_once() {
        let home = Path::new("/home/u");
        let t = "Here: ![plot](/tmp/plot.png) and [clip](file:///tmp/a%20b.mp4 \"title\").\n\
                 Saved to `/tmp/plot.png`, ~/Music/song.flac; see https://x.org/a.png and ./rel.png or /etc/passwd. \
                 Also (/tmp/voice.WAV).";
        let m = find(t, Some(home));
        let got: Vec<(String, Kind)> = m.iter().map(|m| (m.path.display().to_string(), m.kind)).collect();
        assert_eq!(
            got,
            [
                ("/tmp/plot.png".to_string(), Kind::Image),
                ("/tmp/a b.mp4".to_string(), Kind::Video),
                ("/home/u/Music/song.flac".to_string(), Kind::Audio),
                ("/tmp/voice.WAV".to_string(), Kind::Audio),
            ]
        );
        assert!(find("no media here: /usr/bin/ls", None).is_empty());
        assert!(find("~/a.png", None).is_empty(), "no home, no ~");
    }

    #[cfg(windows)]
    #[test]
    fn finds_windows_paths() {
        let t = r"Saved ![plot](C:\tmp\plot.png) and `D:/clips/a.mp4`; not \rel\b.wav or C:\Windows\notepad.exe.";
        let got: Vec<(PathBuf, Kind)> = find(t, None).into_iter().map(|m| (m.path, m.kind)).collect();
        assert_eq!(got, [(PathBuf::from(r"C:\tmp\plot.png"), Kind::Image), (PathBuf::from("D:/clips/a.mp4"), Kind::Video)]);
    }
}
