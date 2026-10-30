//! `~/.ssh/config` as the source of truth for SSH connections.
//!
//! Parsing keeps the file's exact text so edits touch only the lines they must:
//! comments, blank lines, ordering, indentation and unknown options are kept
//! byte for byte. Writes are atomic (temp file + rename), keep the file's
//! permissions, and first save the previous contents to `config.mouser.bak`
//! next to the root config.

use crate::connections::SshConnection;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The SSH config Mouser manages: `$MOUSER_SSH_CONFIG`, else `$HOME/.ssh/config`.
pub fn ssh_config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("MOUSER_SSH_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".ssh").join("config")
}

/// Backup written before every change, next to the root config.
pub fn backup_path(root: &Path) -> PathBuf {
    root.with_file_name(format!("{}.mouser.bak", root.file_name().and_then(|n| n.to_str()).unwrap_or("config")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHostEntry {
    pub alias: String,
    pub conn: SshConnection,
    /// File that defines this Host block.
    pub file: PathBuf,
    /// 0-based line of the `Host` line in `file`.
    pub line: usize,
    /// Why Mouser will not edit this entry, if it is read-only.
    pub read_only: Option<String>,
}

#[derive(Debug, Clone)]
enum LineKind {
    Other,
    Opt { key: String, value: String },
}

#[derive(Debug, Clone)]
struct Line {
    text: String,
    kind: LineKind,
}

/// Split an option line into (indent, key, separator, value, eol).
fn split_option(text: &str) -> Option<(&str, &str, &str, &str, &str)> {
    let body = text.trim_end_matches(['\n', '\r']);
    let eol = &text[body.len()..];
    let trimmed = body.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let indent = &body[..body.len() - trimmed.len()];
    let key_len = trimmed.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(trimmed.len());
    let key = &trimmed[..key_len];
    let rest = &trimmed[key_len..];
    let mut sep_len = 0;
    let mut seen_eq = false;
    for c in rest.chars() {
        if c.is_whitespace() || (c == '=' && !seen_eq) {
            seen_eq |= c == '=';
            sep_len += c.len_utf8();
        } else {
            break;
        }
    }
    let sep = &rest[..sep_len];
    let value = rest[sep_len..].trim_end();
    Some((indent, key, sep, value, eol))
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') { v[1..v.len() - 1].to_string() } else { v.to_string() }
}

fn quote(v: &str) -> String {
    if v.contains(char::is_whitespace) { format!("\"{v}\"") } else { v.to_string() }
}

fn parse_lines(text: &str) -> Vec<Line> {
    text.split_inclusive('\n')
        .map(|t| {
            let kind = match split_option(t) {
                Some((_, k, _, v, _)) => LineKind::Opt { key: k.to_ascii_lowercase(), value: v.to_string() },
                None => LineKind::Other,
            };
            Line { text: t.to_string(), kind }
        })
        .collect()
}

fn is_header(l: &Line) -> bool {
    matches!(&l.kind, LineKind::Opt { key, .. } if key == "host" || key == "match")
}

/// Host/Match blocks: (header line, last option line).
fn blocks(lines: &[Line]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if is_header(&lines[i]) {
            let mut last = i;
            let mut j = i + 1;
            while j < lines.len() && !is_header(&lines[j]) {
                if matches!(lines[j].kind, LineKind::Opt { .. }) {
                    last = j;
                }
                j += 1;
            }
            out.push((i, last));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

fn has_wildcard(p: &str) -> bool {
    p.contains(['*', '?', '!'])
}

fn split_words(v: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in v.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn known_values(c: &SshConnection) -> Vec<(&'static str, Option<String>)> {
    let yn = |b: Option<bool>| b.map(|b| if b { "yes".to_string() } else { "no".to_string() });
    vec![
        ("HostName", Some(c.host.clone()).filter(|h| !h.is_empty())),
        ("User", c.user.clone()),
        ("Port", c.port.map(|p| p.to_string())),
        ("IdentityFile", c.identity_file.clone()),
        ("ProxyJump", c.proxy_jump.clone()),
        ("Compression", yn(c.compression)),
        ("ServerAliveInterval", c.server_alive_interval.map(|v| v.to_string())),
        ("ServerAliveCountMax", c.server_alive_count_max.map(|v| v.to_string())),
        ("StrictHostKeyChecking", c.strict_host_key_checking.clone()),
        ("UserKnownHostsFile", c.user_known_hosts_file.clone()),
        ("PreferredAuthentications", c.preferred_authentications.clone()),
        ("Ciphers", c.ciphers.clone()),
        ("MACs", c.macs.clone()),
        ("KexAlgorithms", c.kex_algorithms.clone()),
    ]
}

fn block_connection(lines: &[Line], header: usize, last: usize) -> SshConnection {
    let mut c = SshConnection::default();
    let mut seen: Vec<String> = Vec::new();
    for l in &lines[header + 1..=last.max(header)] {
        let LineKind::Opt { key, value } = &l.kind else { continue };
        if seen.contains(key) || key == "include" {
            continue; // ssh uses the first value
        }
        seen.push(key.clone());
        let v = unquote(value);
        match key.as_str() {
            "hostname" => c.host = v,
            "user" => c.user = Some(v),
            "port" => c.port = v.parse().ok(),
            "identityfile" => c.identity_file = Some(v),
            "proxyjump" => c.proxy_jump = Some(v),
            "compression" => c.compression = Some(v.eq_ignore_ascii_case("yes")),
            "serveraliveinterval" => c.server_alive_interval = v.parse().ok(),
            "serveralivecountmax" => c.server_alive_count_max = v.parse().ok(),
            "stricthostkeychecking" => c.strict_host_key_checking = Some(v),
            "userknownhostsfile" => c.user_known_hosts_file = Some(v),
            "preferredauthentications" => c.preferred_authentications = Some(v),
            "ciphers" => c.ciphers = Some(v),
            "macs" => c.macs = Some(v),
            "kexalgorithms" => c.kex_algorithms = Some(v),
            _ => {
                let canon = l.text.trim_start().split(|ch: char| ch.is_whitespace() || ch == '=').next().unwrap_or(key).to_string();
                c.extra_options.insert(canon, v);
            }
        }
    }
    c
}

fn expand_include(pattern: &str, base: &Path) -> Vec<PathBuf> {
    let p = if let Some(rest) = pattern.strip_prefix("~/") {
        dirs::home_dir().unwrap_or_default().join(rest)
    } else if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        base.join(pattern)
    };
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    if !has_wildcard(&name) {
        return if p.is_file() { vec![p] } else { Vec::new() };
    }
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut out: Vec<PathBuf> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|f| f.is_file() && f.file_name().and_then(|n| n.to_str()).is_some_and(|n| glob_match(&name, n)))
        .collect();
    out.sort();
    out
}

fn glob_match(pat: &str, s: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pat.chars().collect(), s.chars().collect());
    fn go(p: &[char], t: &[char]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&p[1..], t) || (!t.is_empty() && go(p, &t[1..])),
            (Some('?'), Some(_)) => go(&p[1..], &t[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &t[1..]),
            _ => false,
        }
    }
    go(&p, &t)
}

fn walk(file: &Path, base: &Path, depth: usize, files: &mut Vec<PathBuf>, hosts: &mut Vec<SshHostEntry>) {
    if depth > 16 || files.iter().any(|f| f == file) {
        return;
    }
    files.push(file.to_path_buf());
    let Ok(text) = fs::read_to_string(file) else { return };
    let lines = parse_lines(&text);
    let blocks = blocks(&lines);
    for (i, l) in lines.iter().enumerate() {
        let LineKind::Opt { key, value } = &l.kind else { continue };
        if key == "include" {
            for pat in split_words(value) {
                for inc in expand_include(&pat, base) {
                    walk(&inc, base, depth + 1, files, hosts);
                }
            }
        } else if key == "host" {
            let pats = split_words(value);
            let plain: Vec<&String> = pats.iter().filter(|p| !has_wildcard(p)).collect();
            if plain.is_empty() {
                continue; // wildcard-only, e.g. `Host *`
            }
            let last = blocks.iter().find(|(h, _)| *h == i).map(|(_, l)| *l).unwrap_or(i);
            let conn = block_connection(&lines, i, last);
            let read_only =
                if pats.len() > 1 { Some(format!("Host line has several patterns ({}); edit it in {}", value.trim(), file.display())) } else { None };
            for alias in plain {
                if hosts.iter().any(|h| &h.alias == alias) {
                    continue; // first definition wins, as in ssh
                }
                hosts.push(SshHostEntry {
                    alias: alias.clone(),
                    conn: conn.clone(),
                    file: file.to_path_buf(),
                    line: i,
                    read_only: read_only.clone(),
                });
            }
        }
    }
}

/// All concrete Host aliases in `root` and its Includes (in ssh order).
pub fn load_hosts(root: &Path) -> Vec<SshHostEntry> {
    let base = root.parent().map(Path::to_path_buf).unwrap_or_default();
    let (mut files, mut hosts) = (Vec::new(), Vec::new());
    walk(root, &base, 0, &mut files, &mut hosts);
    hosts
}

/// `root` and every file it includes (for change watching).
pub fn config_files(root: &Path) -> Vec<PathBuf> {
    let base = root.parent().map(Path::to_path_buf).unwrap_or_default();
    let (mut files, mut hosts) = (Vec::new(), Vec::new());
    walk(root, &base, 0, &mut files, &mut hosts);
    if files.is_empty() {
        files.push(root.to_path_buf());
    }
    files
}

/// Modification-time fingerprint of the config and its includes.
pub fn fingerprint(root: &Path) -> Vec<(PathBuf, Option<std::time::SystemTime>, u64)> {
    config_files(root)
        .into_iter()
        .map(|f| {
            let md = fs::metadata(&f).ok();
            let m = md.as_ref().and_then(|m| m.modified().ok());
            let len = md.map(|m| m.len()).unwrap_or(0);
            (f, m, len)
        })
        .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum SshConfigError {
    #[error("{0}")]
    ReadOnly(String),
    #[error("Host '{0}' already exists")]
    Exists(String),
    #[error("Host '{0}' not found")]
    NotFound(String),
    #[error("invalid alias '{0}': use one word without wildcards")]
    BadAlias(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub fn valid_alias(alias: &str) -> bool {
    !alias.is_empty() && !alias.contains(char::is_whitespace) && !has_wildcard(alias) && !alias.contains(['"', '#', '='])
}

/// Rewrite one Host block so its options match `conn` (other lines untouched).
fn edit_block(text: &str, header: usize, old: &SshConnection, new: &SshConnection) -> String {
    let mut lines = parse_lines(text);
    let last = blocks(&lines).into_iter().find(|(h, _)| *h == header).map(|(_, l)| l).unwrap_or(header);
    let mut desired: Vec<(String, Option<String>)> = known_values(new).into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    let mut extra_keys: Vec<&String> = old.extra_options.keys().chain(new.extra_options.keys()).collect();
    extra_keys.sort();
    extra_keys.dedup();
    for k in extra_keys {
        desired.push((k.clone(), new.extra_options.get(k).cloned()));
    }
    let indent = lines[header + 1..=last.max(header)]
        .iter()
        .find_map(|l| matches!(l.kind, LineKind::Opt { .. }).then(|| split_option(&l.text).map(|(i, ..)| i.to_string())).flatten())
        .unwrap_or_else(|| "    ".to_string());
    let mut remove = Vec::new();
    let mut inserts = Vec::new();
    for (key, want) in desired {
        let lk = key.to_ascii_lowercase();
        let pos = (header + 1..=last).filter(|&i| i < lines.len()).find(|&i| matches!(&lines[i].kind, LineKind::Opt { key, .. } if *key == lk));
        match (pos, want) {
            (Some(i), Some(v)) => {
                let (ind, k, sep, cur, eol) = split_option(&lines[i].text).unwrap();
                if unquote(cur) != v {
                    lines[i].text = format!("{ind}{k}{sep}{}{eol}", quote(&v));
                }
            }
            (Some(i), None) => remove.push(i),
            (None, Some(v)) => inserts.push(format!("{indent}{key} {}\n", quote(&v))),
            (None, None) => {}
        }
    }
    if !inserts.is_empty() {
        let anchor = (header..=last).rev().find(|&i| !remove.contains(&i)).unwrap_or(header);
        if !lines[anchor].text.ends_with('\n') {
            lines[anchor].text.push('\n');
        }
        for (n, t) in inserts.into_iter().enumerate() {
            lines.insert(anchor + 1 + n, Line { text: t, kind: LineKind::Other });
        }
    }
    let mut out = String::with_capacity(text.len() + 64);
    for (i, l) in lines.iter().enumerate() {
        if !remove.contains(&i) {
            out.push_str(&l.text);
        }
    }
    out
}

fn host_block_text(alias: &str, c: &SshConnection) -> String {
    let mut s = format!("Host {alias}\n");
    for (k, v) in known_values(c) {
        if let Some(v) = v {
            s.push_str(&format!("    {k} {}\n", quote(&v)));
        }
    }
    let mut extra: Vec<_> = c.extra_options.iter().collect();
    extra.sort();
    for (k, v) in extra {
        s.push_str(&format!("    {k} {}\n", quote(v)));
    }
    s
}

#[cfg(unix)]
fn mode_of(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).ok().map(|m| m.permissions().mode() & 0o7777)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// Atomically replace `file` with `text`, after backing up its old contents.
fn write_config(root: &Path, file: &Path, old: Option<&str>, text: &str) -> std::io::Result<()> {
    if old == Some(text) {
        return Ok(());
    }
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    let mode = mode_of(file).unwrap_or(0o600);
    if let Some(old) = old {
        let bak = backup_path(root);
        fs::write(&bak, old)?;
        #[cfg(unix)]
        set_mode(&bak, 0o600)?;
    }
    let tmp = file.with_file_name(format!(".{}.mouser.tmp", file.file_name().and_then(|n| n.to_str()).unwrap_or("config")));
    {
        use std::io::Write;
        let mut f = fs::File::create(&tmp)?;
        #[cfg(unix)]
        set_mode(&tmp, mode)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, file)
}

/// Update the Host block for `alias` in place, or append a new block to `root`.
pub fn upsert_host(root: &Path, alias: &str, conn: &SshConnection) -> Result<(), SshConfigError> {
    if !valid_alias(alias) {
        return Err(SshConfigError::BadAlias(alias.to_string()));
    }
    match load_hosts(root).into_iter().find(|h| h.alias == alias) {
        Some(h) => {
            if let Some(r) = h.read_only {
                return Err(SshConfigError::ReadOnly(r));
            }
            let text = fs::read_to_string(&h.file)?;
            let new = edit_block(&text, h.line, &h.conn, conn);
            write_config(root, &h.file, Some(&text), &new)?;
        }
        None => {
            let old = fs::read_to_string(root).ok();
            let mut text = old.clone().unwrap_or_default();
            if !text.is_empty() {
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                if !text.ends_with("\n\n") {
                    text.push('\n');
                }
            }
            text.push_str(&host_block_text(alias, conn));
            write_config(root, root, old.as_deref(), &text)?;
        }
    }
    Ok(())
}

/// Add a new Host; fails if the alias already exists.
pub fn add_host(root: &Path, alias: &str, conn: &SshConnection) -> Result<(), SshConfigError> {
    if load_hosts(root).iter().any(|h| h.alias == alias) {
        return Err(SshConfigError::Exists(alias.to_string()));
    }
    upsert_host(root, alias, conn)
}

/// Rename a host (edit its `Host` line) and update its options.
pub fn rename_host(root: &Path, old_alias: &str, new_alias: &str, conn: &SshConnection) -> Result<(), SshConfigError> {
    if old_alias == new_alias {
        return upsert_host(root, old_alias, conn);
    }
    if !valid_alias(new_alias) {
        return Err(SshConfigError::BadAlias(new_alias.to_string()));
    }
    let hosts = load_hosts(root);
    if hosts.iter().any(|h| h.alias == new_alias) {
        return Err(SshConfigError::Exists(new_alias.to_string()));
    }
    let h = hosts.into_iter().find(|h| h.alias == old_alias).ok_or_else(|| SshConfigError::NotFound(old_alias.to_string()))?;
    if let Some(r) = h.read_only {
        return Err(SshConfigError::ReadOnly(r));
    }
    let text = fs::read_to_string(&h.file)?;
    let mut lines = parse_lines(&text);
    let (ind, k, sep, _, eol) = split_option(&lines[h.line].text).unwrap();
    lines[h.line].text = format!("{ind}{k}{sep}{new_alias}{eol}");
    let renamed: String = lines.iter().map(|l| l.text.as_str()).collect();
    let new = edit_block(&renamed, h.line, &h.conn, conn);
    write_config(root, &h.file, Some(&text), &new)?;
    Ok(())
}

/// Remove the Host block for `alias` (and one blank line after it).
pub fn delete_host(root: &Path, alias: &str) -> Result<(), SshConfigError> {
    let h = load_hosts(root).into_iter().find(|h| h.alias == alias).ok_or_else(|| SshConfigError::NotFound(alias.to_string()))?;
    if let Some(r) = h.read_only {
        return Err(SshConfigError::ReadOnly(r));
    }
    let text = fs::read_to_string(&h.file)?;
    let lines = parse_lines(&text);
    let last = blocks(&lines).into_iter().find(|(hd, _)| *hd == h.line).map(|(_, l)| l).unwrap_or(h.line);
    let mut end = last + 1;
    let blank = |i: usize| lines.get(i).is_some_and(|l| l.text.trim().is_empty());
    // Comment lines directly above the Host line describe it; remove them too.
    let mut start = h.line;
    while start > 0 && lines[start - 1].text.trim_start().starts_with('#') {
        start -= 1;
    }
    if blank(end) && (start == 0 || blank(start - 1) || end + 1 == lines.len()) {
        end += 1;
    }
    let mut out: String = lines[..start].iter().chain(&lines[end.min(lines.len())..]).map(|l| l.text.as_str()).collect();
    if start > 0 && end >= lines.len() {
        // Removed the last block: drop the blank separator left before it.
        while out.ends_with("\n\n") {
            out.pop();
        }
    }
    write_config(root, &h.file, Some(&text), &out)?;
    Ok(())
}

/// Hosts from `root` as `(alias, read-only reason)` for display.
pub fn host_names(root: &Path) -> Vec<(String, Option<String>)> {
    load_hosts(root).into_iter().map(|h| (h.alias, h.read_only)).collect()
}

/// Result of moving Mouser-saved SSH connections into the SSH config.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Migration {
    pub moved: Vec<String>,
    pub skipped: HashMap<String, String>,
}

/// SSH connections still saved under `~/.mouser/connections`.
pub fn mouser_ssh_connections() -> Vec<(String, SshConnection)> {
    crate::connections::list_connections()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|c| match c.kind {
            crate::connections::ConnectionKind::Ssh(s) => Some((c.name, s)),
            _ => None,
        })
        .collect()
}

/// Move Mouser-saved SSH connections into `root`. Only call after the user
/// confirmed. Entries whose alias exists or is invalid are left in place.
pub fn migrate_mouser_ssh(root: &Path) -> Migration {
    let mut m = Migration::default();
    for (name, conn) in mouser_ssh_connections() {
        match add_host(root, &name, &conn) {
            Ok(()) => {
                let _ = crate::connections::delete_connection(&name);
                m.moved.push(name);
            }
            Err(e) => {
                m.skipped.insert(name, e.to_string());
            }
        }
    }
    m
}

use crate::appearance::{EditKey, Field, FieldKind, Outcome};

/// Add/edit form for one SSH Host block.
#[derive(Debug, Clone)]
pub struct SshForm {
    /// Alias being edited (`None` for a new host).
    pub original: Option<String>,
    pub fields: Vec<Field>,
    pub focus: usize,
    pub error: Option<String>,
    /// Set for entries Mouser will not edit; the form is then view-only.
    pub read_only: Option<String>,
    /// Options not shown in the form, kept as they are.
    base: SshConnection,
}

impl SshForm {
    pub fn new(alias: Option<&str>, conn: &SshConnection, read_only: Option<String>) -> Self {
        let fields = vec![
            Field::new("Alias", alias.unwrap_or(""), FieldKind::Alias),
            Field::new("HostName", conn.host.clone(), FieldKind::Text),
            Field::new("User", conn.user.clone().unwrap_or_default(), FieldKind::Text),
            Field::new("Port", conn.port.map(|p| p.to_string()).unwrap_or_default(), FieldKind::Port),
            Field::new("IdentityFile", conn.identity_file.clone().unwrap_or_default(), FieldKind::Text),
            Field::new("ProxyJump", conn.proxy_jump.clone().unwrap_or_default(), FieldKind::Text),
        ];
        Self { original: alias.map(str::to_string), fields, focus: 0, error: None, read_only, base: conn.clone() }
    }

    pub fn handle(&mut self, key: EditKey) -> Outcome {
        let n = self.fields.len();
        match key {
            EditKey::Cancel => return Outcome::Close,
            EditKey::Next | EditKey::Down => self.focus = (self.focus + 1) % n,
            EditKey::Prev | EditKey::Up => self.focus = (self.focus + n - 1) % n,
            _ if self.read_only.is_some() => {
                if key == EditKey::Enter {
                    return Outcome::Close;
                }
            }
            EditKey::Enter | EditKey::Save => match self.validate() {
                Ok(_) => return Outcome::Save,
                Err(e) => self.error = Some(e),
            },
            EditKey::Backspace => {
                self.fields[self.focus].value.pop();
                self.error = None;
            }
            EditKey::Char(c) => {
                if self.fields[self.focus].accepts(c) {
                    self.fields[self.focus].value.push(c);
                    self.error = None;
                }
            }
            EditKey::Left | EditKey::Right => {}
        }
        Outcome::Continue
    }

    /// `(alias, connection)` to write; unknown options of the original are kept.
    pub fn validate(&self) -> Result<(String, SshConnection), String> {
        if let Some(r) = &self.read_only {
            return Err(r.clone());
        }
        let v = |i: usize| self.fields[i].value.trim().to_string();
        let opt = |i: usize| Some(v(i)).filter(|s| !s.is_empty());
        if !valid_alias(&v(0)) {
            return Err("Alias: one word, no spaces or wildcards".into());
        }
        if !self.fields[3].is_valid() {
            return Err("Port must be 1-65535".into());
        }
        let mut c = self.base.clone();
        c.host = v(1);
        c.user = opt(2);
        c.port = opt(3).and_then(|p| p.parse().ok());
        c.identity_file = opt(4);
        c.proxy_jump = opt(5);
        Ok((v(0), c))
    }

    /// Write the form to the SSH config rooted at `root`.
    pub fn save(&self, root: &Path) -> Result<String, String> {
        let (alias, conn) = self.validate()?;
        let r = match &self.original {
            Some(old) => rename_host(root, old, &alias, &conn),
            None => add_host(root, &alias, &conn),
        };
        r.map(|_| alias).map_err(|e| e.to_string())
    }
}
