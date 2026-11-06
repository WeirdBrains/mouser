//! Tab completion for Mouser text fields.
//!
//! Path fields complete filesystem entries (relative to a working directory,
//! with `~` expansion). Command fields complete names then arguments. Other
//! fields complete from a list of known values. Tab inserts the common prefix;
//! repeated Tab cycles matches and Shift+Tab cycles backwards. No match leaves
//! the text unchanged and reports `"no matches"` so Tab is never a silent no-op
//! and never inserts a literal tab.

use std::path::{Path, PathBuf};

/// One name in a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEnt {
    pub name: String,
    pub is_dir: bool,
}

/// Where path completion looks up files: the local disk, or a commander's
/// remote listing.
pub trait PathSource {
    fn list(&self, dir: &Path) -> Vec<DirEnt>;
    fn home(&self) -> Option<PathBuf>;
    fn is_dir(&self, path: &Path) -> bool {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return !self.list(path).is_empty();
        };
        let parent = path.parent().unwrap_or(path);
        self.list(parent).iter().any(|e| e.name == name && e.is_dir)
    }
}

/// Local filesystem.
pub struct FsSource;

impl PathSource for FsSource {
    fn list(&self, dir: &Path) -> Vec<DirEnt> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        rd.flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if name == "." || name == ".." {
                    return None;
                }
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false) || e.path().is_dir();
                Some(DirEnt { name, is_dir })
            })
            .collect()
    }

    fn home(&self) -> Option<PathBuf> {
        std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }
}

/// Outcome of one Tab (or Shift+Tab).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub text: String,
    pub candidates: Vec<String>,
    pub selected: Option<usize>,
    pub message: Option<String>,
}

impl Outcome {
    /// A short inline list for a status/hint row (`foo/  [bar/]  baz/`).
    pub fn list_hint(&self, max_chars: usize) -> String {
        if let Some(msg) = &self.message
            && self.candidates.is_empty()
        {
            return msg.clone();
        }
        if self.candidates.is_empty() {
            return String::new();
        }
        let n = self.candidates.len();
        let sel = self.selected.unwrap_or(0);
        let mut parts = Vec::new();
        let mut used = 0usize;
        // Start near the selection so the current match stays visible.
        let start = if n <= 8 { 0 } else { sel.saturating_sub(2) };
        for (i, c) in self.candidates.iter().enumerate().skip(start) {
            let piece = if Some(i) == self.selected { format!("[{c}]") } else { c.clone() };
            let extra = piece.len() + if parts.is_empty() { 0 } else { 2 };
            if used + extra > max_chars && !parts.is_empty() {
                parts.push("…".into());
                break;
            }
            used += extra;
            parts.push(piece);
        }
        parts.join("  ")
    }

    /// True when Tab did something other than report no matches (unique or cycle).
    pub fn changed(&self, previous: &str) -> bool {
        self.text != previous || self.selected.is_some() || self.message.is_some()
    }
}

/// Remembers matches so a later Tab walks them instead of recomputing.
#[derive(Debug, Clone, Default)]
pub struct Engine {
    cycle: Option<Cycle>,
}

#[derive(Debug, Clone)]
struct Cycle {
    stem: String,
    matches: Vec<String>,
    /// `None` means the common prefix is showing and the next Tab starts cycling.
    index: Option<usize>,
    shown: String,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.cycle = None;
    }

    /// Tab-complete `text`. `matches` replace `token`; `text` should be `stem + token`
    /// when starting a new cycle.
    pub fn tab(&mut self, text: &str, stem: &str, token: &str, matches: &[String], reverse: bool) -> Outcome {
        if let Some(c) = &self.cycle
            && text == c.shown
            && !c.matches.is_empty()
            && c.matches == matches
        {
            let n = c.matches.len();
            let i = match c.index {
                None => {
                    if reverse {
                        n - 1
                    } else {
                        0
                    }
                }
                Some(i) => {
                    if reverse {
                        (i + n - 1) % n
                    } else {
                        (i + 1) % n
                    }
                }
            };
            let shown = format!("{}{}", c.stem, c.matches[i]);
            let matches = c.matches.clone();
            self.cycle = Some(Cycle { stem: c.stem.clone(), matches: matches.clone(), index: Some(i), shown: shown.clone() });
            return Outcome { text: shown, candidates: matches, selected: Some(i), message: None };
        }

        if matches.is_empty() {
            self.cycle = None;
            return Outcome { text: text.to_string(), candidates: Vec::new(), selected: None, message: Some("no matches".into()) };
        }

        if matches.len() == 1 {
            let shown = format!("{}{}", stem, matches[0]);
            self.cycle = Some(Cycle { stem: stem.into(), matches: matches.to_vec(), index: Some(0), shown: shown.clone() });
            return Outcome { text: shown, candidates: matches.to_vec(), selected: Some(0), message: None };
        }

        let common = common_prefix(matches);
        if !reverse && common.len() > token.len() {
            let shown = format!("{stem}{common}");
            self.cycle = Some(Cycle { stem: stem.into(), matches: matches.to_vec(), index: None, shown: shown.clone() });
            return Outcome { text: shown, candidates: matches.to_vec(), selected: None, message: None };
        }

        let i = if reverse { matches.len() - 1 } else { 0 };
        let shown = format!("{}{}", stem, matches[i]);
        self.cycle = Some(Cycle { stem: stem.into(), matches: matches.to_vec(), index: Some(i), shown: shown.clone() });
        Outcome { text: shown, candidates: matches.to_vec(), selected: Some(i), message: None }
    }
}

/// Longest shared prefix of `items`.
pub fn common_prefix(items: &[String]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut end = first.len();
    for s in &items[1..] {
        end = first.as_bytes().iter().zip(s.as_bytes()).take_while(|(a, b)| a == b).count().min(end);
        while !first.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
    }
    first[..end].to_string()
}

/// Names whose prefix is `token`. Hidden names (starting with `.`) are dropped
/// unless `token` itself starts with `.` or `drop_hidden` is false.
pub fn prefix_matches(names: &[String], token: &str, ignore_case: bool, drop_hidden: bool) -> Vec<String> {
    let hide = drop_hidden && !token.starts_with('.');
    let mut out: Vec<String> = names
        .iter()
        .filter(|n| {
            if hide && n.starts_with('.') {
                return false;
            }
            if ignore_case { n.to_lowercase().starts_with(&token.to_lowercase()) } else { n.starts_with(token) }
        })
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

fn split_last(s: &str) -> (&str, &str) {
    match s.rfind(|c| c == '/' || (cfg!(windows) && c == '\\')) {
        Some(0) => {
            let sep = &s[0..1];
            (sep, &s[1..])
        }
        Some(i) => (&s[..i], &s[i + 1..]),
        None => ("", s),
    }
}

/// Split a path-typed field into `(stem, token, dir_to_list)`.
/// `stem + token` reconstructs `input` except that a trailing slash after a
/// finished directory lives in `stem` and `token` is empty.
pub fn split_path(input: &str, cwd: &Path, source: &dyn PathSource) -> (String, String, PathBuf) {
    if input == "~" {
        return (String::new(), "~".into(), source.home().unwrap_or_else(|| PathBuf::from("~")));
    }
    if let Some(rest) = input.strip_prefix("~/")
        && let Some(home) = source.home()
    {
        let (dir_rel, name) = split_last(rest);
        let dir = if dir_rel.is_empty() { home } else { home.join(dir_rel) };
        let stem = if dir_rel.is_empty() { "~/".into() } else { format!("~/{dir_rel}/") };
        return (stem, name.to_string(), dir);
    }
    let abs = input.starts_with('/') || (cfg!(windows) && Path::new(input).is_absolute());
    let (dir_s, name) = split_last(input);
    if abs {
        let dir = if dir_s.is_empty() || dir_s == "/" { PathBuf::from("/") } else { PathBuf::from(dir_s) };
        let stem = if dir_s == "/" || dir_s.is_empty() { "/".into() } else { format!("{dir_s}/") };
        return (stem, name.to_string(), dir);
    }
    let dir = if dir_s.is_empty() { cwd.to_path_buf() } else { cwd.join(dir_s) };
    let stem = if dir_s.is_empty() { String::new() } else { format!("{dir_s}/") };
    (stem, name.to_string(), dir)
}

/// Path candidates for `input`. Directories get a trailing `/`.
pub fn path_matches(input: &str, cwd: &Path, source: &dyn PathSource, directories_only: bool) -> (String, String, Vec<String>) {
    if input == "~" {
        if source.home().is_some() {
            return (String::new(), "~".into(), vec!["~/".into()]);
        }
        return (String::new(), "~".into(), Vec::new());
    }
    let (stem, token, dir) = split_path(input, cwd, source);
    let drop_hidden = !token.starts_with('.');
    let mut out = Vec::new();
    for e in source.list(&dir) {
        if drop_hidden && e.name.starts_with('.') {
            continue;
        }
        if directories_only && !e.is_dir {
            continue;
        }
        if !e.name.starts_with(&token) {
            continue;
        }
        let mut v = e.name;
        if e.is_dir {
            v.push('/');
        }
        out.push(v);
    }
    out.sort();
    out.dedup();
    (stem, token, out)
}

/// Complete the whole field from `values` (case-insensitive).
pub fn value_matches(input: &str, values: &[String]) -> (String, String, Vec<String>) {
    (String::new(), input.to_string(), prefix_matches(values, input, true, false))
}

/// How the argument of a command is completed.
#[derive(Debug, Clone)]
pub enum Arg {
    None,
    Values(Vec<String>),
    Path { directories_only: bool },
}

/// A command-line word and what follows it.
#[derive(Debug, Clone)]
pub struct Cmd {
    pub name: String,
    pub arg: Arg,
}

fn split_cmd(input: &str) -> (String, String, bool) {
    match input.find(char::is_whitespace) {
        Some(i) => (input[..i].to_string(), input[i..].trim_start().to_string(), true),
        None => (input.to_string(), String::new(), false),
    }
}

/// Complete a `:` command line: first token from `commands`, later tokens from
/// that command's [`Arg`].
pub fn command_matches(input: &str, commands: &[Cmd], cwd: &Path, source: &dyn PathSource) -> (String, String, Vec<String>) {
    let raw = input.strip_prefix(':').unwrap_or(input);
    let colon = if input.starts_with(':') { ":" } else { "" };
    let (head, arg, has_arg) = split_cmd(raw);
    if !has_arg {
        let names: Vec<String> = commands.iter().map(|c| c.name.clone()).collect();
        let matches = prefix_matches(&names, &head, true, false);
        let matches: Vec<String> = matches
            .into_iter()
            .map(|n| {
                let spacer = commands.iter().find(|c| c.name == n).is_some_and(|c| !matches!(&c.arg, Arg::None));
                if spacer { format!("{colon}{n} ") } else { format!("{colon}{n}") }
            })
            .collect();
        return (String::new(), input.to_string(), matches);
    }
    let Some(cmd) = commands.iter().find(|c| c.name.eq_ignore_ascii_case(&head)) else {
        return (format!("{colon}{head} "), arg, Vec::new());
    };
    let stem = format!("{colon}{head} ");
    match &cmd.arg {
        Arg::None => (stem, arg, Vec::new()),
        Arg::Values(v) => {
            let matches = prefix_matches(v, &arg, true, false);
            (stem, arg, matches)
        }
        Arg::Path { directories_only } => {
            let (pstem, ptoken, matches) = path_matches(&arg, cwd, source, *directories_only);
            (format!("{stem}{pstem}"), ptoken, matches)
        }
    }
}

/// Executable names on `PATH` starting with `prefix`.
pub fn executables(prefix: &str) -> Vec<String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut names = Vec::new();
    for dir in std::env::split_paths(&path) {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with(prefix) || name.starts_with('.') {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let Ok(meta) = e.metadata() else { continue };
                if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            names.push(name);
        }
    }
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "mouser-complete-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tmp() -> (Tmp, PathBuf) {
        let t = Tmp::new();
        let p = t.path().to_path_buf();
        (t, p)
    }

    fn names(dir: &Path, entries: &[(&str, bool)]) {
        for (name, is_dir) in entries {
            let p = dir.join(name);
            if *is_dir {
                fs::create_dir_all(&p).unwrap();
            } else {
                if let Some(parent) = p.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::write(&p, b"").unwrap();
            }
        }
    }

    #[test]
    fn common_prefix_and_no_matches() {
        assert_eq!(common_prefix(&[("docs").into(), ("downloads").into()]), "do");
        assert_eq!(common_prefix(&[("dest_shared_x/").into(), ("dest_shared_y/").into()]), "dest_shared_");
        assert_eq!(common_prefix(&["only".into()]), "only");
        assert_eq!(common_prefix(&[]), "");
        let mut e = Engine::new();
        let o = e.tab("zzz", "", "zzz", &[], false);
        assert_eq!(o.text, "zzz");
        assert_eq!(o.message.as_deref(), Some("no matches"));
        assert!(o.candidates.is_empty());
    }

    #[test]
    fn unique_match_and_cycle() {
        let mut e = Engine::new();
        let o = e.tab("de", "", "de", &["dest_a/".into(), "dest_b/".into()], false);
        assert_eq!(o.text, "dest_");
        assert_eq!(o.candidates.len(), 2);
        assert_eq!(o.selected, None);
        let o = e.tab(&o.text, "", "dest_", &["dest_a/".into(), "dest_b/".into()], false);
        assert_eq!(o.text, "dest_a/");
        assert_eq!(o.selected, Some(0));
        let o = e.tab(&o.text, "", "dest_a/", &["dest_a/".into(), "dest_b/".into()], false);
        assert_eq!(o.text, "dest_b/");
        assert_eq!(o.selected, Some(1));
        let o = e.tab(&o.text, "", "dest_b/", &["dest_a/".into(), "dest_b/".into()], true);
        assert_eq!(o.text, "dest_a/");
        let o = e.tab("dest_a/", "", "dest_a/", &["dest_a/".into()], false);
        assert_eq!(o.text, "dest_a/");
        assert_eq!(o.selected, Some(0));
    }

    #[test]
    fn path_tilde_hidden_dirs_and_relative() {
        let (_t, root) = tmp();
        names(&root, &[("docs", true), ("downloads", true), ("notes.md", false), (".secret", false), (".hidden_dir", true)]);
        let home = root.join("homeuser");
        fs::create_dir_all(home.join("Documents")).unwrap();
        fs::write(home.join("Documents").join("a.txt"), b"").unwrap();
        let old_home = std::env::var_os("HOME");
        // SAFETY: restored below; this crate's tests do not read HOME elsewhere.
        unsafe { std::env::set_var("HOME", &home) };

        let src = FsSource;
        let (stem, token, matches) = path_matches("do", &root, &src, false);
        assert_eq!(stem, "");
        assert_eq!(token, "do");
        assert_eq!(matches, vec!["docs/".to_string(), "downloads/".to_string()]);

        let (_, _, files) = path_matches("n", &root, &src, false);
        assert_eq!(files, vec!["notes.md".to_string()]);
        let (_, _, only_dirs) = path_matches("n", &root, &src, true);
        assert!(only_dirs.is_empty(), "notes.md is not a directory");

        let (_, _, vis) = path_matches("", &root, &src, false);
        assert!(!vis.iter().any(|n| n.starts_with('.')), "hidden skipped without a dot prefix: {vis:?}");
        let (_, _, hid) = path_matches(".", &root, &src, false);
        assert!(hid.contains(&".secret".to_string()) && hid.contains(&".hidden_dir/".to_string()), "{hid:?}");

        let (stem, token, matches) = path_matches("~/Do", &root, &src, false);
        assert_eq!(stem, "~/");
        assert_eq!(token, "Do");
        assert_eq!(matches, vec!["Documents/".to_string()]);

        let mut e = Engine::new();
        let o = e.tab("~", "", "~", &["~/".into()], false);
        assert_eq!(o.text, "~/");

        let (stem, token, matches) = path_matches("docs/", &root, &src, false);
        assert_eq!((stem.as_str(), token.as_str()), ("docs/", ""));
        let _ = matches;

        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn command_name_then_path_arg() {
        let (_t, root) = tmp();
        names(&root, &[("themes", true)]);
        let cmds = vec![
            Cmd { name: "theme".into(), arg: Arg::Values(vec!["Omarchy".into(), "Red Star".into()]) },
            Cmd { name: "import-theme".into(), arg: Arg::Path { directories_only: true } },
            Cmd { name: "edit-theme".into(), arg: Arg::None },
        ];
        let src = FsSource;
        let (_, _, m) = command_matches("imp", &cmds, &root, &src);
        assert_eq!(m, vec!["import-theme ".to_string()]);
        let (_, _, m) = command_matches("theme Om", &cmds, &root, &src);
        assert_eq!(m, vec!["Omarchy".to_string()]);
        let (stem, token, m) = command_matches("import-theme th", &cmds, &root, &src);
        assert_eq!(stem, "import-theme ");
        assert_eq!(token, "th");
        assert_eq!(m, vec!["themes/".to_string()]);
        let (_, _, m) = command_matches("edit-theme", &cmds, &root, &src);
        assert_eq!(m, vec!["edit-theme".to_string()]);
    }

    #[test]
    fn list_hint_marks_selection() {
        let o = Outcome { text: "dest_b/".into(), candidates: vec!["dest_a/".into(), "dest_b/".into()], selected: Some(1), message: None };
        let h = o.list_hint(80);
        assert!(h.contains("[dest_b/]") && h.contains("dest_a/"), "{h}");
        let empty = Outcome { text: "z".into(), candidates: vec![], selected: None, message: Some("no matches".into()) };
        assert_eq!(empty.list_hint(40), "no matches");
    }
}
