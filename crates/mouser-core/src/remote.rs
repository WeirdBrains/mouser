//! Commander in SSH sessions: where the shell is (OSC 7 `file://host/path`
//! against the local hostname), how to reach it (the foreground `ssh`
//! command's own options, through a multiplexed side connection), and the
//! remote `ls -la` parsed into commander entries.

use crate::commander::{Entry, Kind, kind_of};
use std::path::{Path, PathBuf};

/// Where commander should look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    Local(PathBuf),
    /// A shell on another host. `dir` is `None` without a remote OSC 7;
    /// `ssh` is the side channel (`None`: no ssh in the foreground to copy).
    Remote {
        label: String,
        dir: Option<PathBuf>,
        ssh: Option<SshTarget>,
    },
}

/// How to open a side connection to the host of a running `ssh`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SshTarget {
    /// Options copied from the running command (`-p 22`, `-F cfg`, ...).
    pub opts: Vec<String>,
    pub dest: String,
}

impl SshTarget {
    /// `user@host` for the header (the destination as typed).
    pub fn label(&self) -> String {
        let user = self.opts.windows(2).find(|w| w[0] == "-l").map(|w| w[1].clone());
        match user {
            Some(u) if !self.dest.contains('@') => format!("{u}@{}", self.dest),
            _ => self.dest.clone(),
        }
    }

    /// The side channel's argv (after the ssh binary): the same options, no
    /// prompts (`BatchMode`), multiplexed through `control_dir` so repeated
    /// listings reuse one connection, then the remote command.
    pub fn argv(&self, control_dir: &Path, remote_cmd: &str) -> Vec<String> {
        let mut v = self.opts.clone();
        v.extend(
            [
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                "-o",
                "ControlMaster=auto",
                "-o",
                &format!("ControlPath={}", control_dir.join("%C").display()),
                "-o",
                "ControlPersist=120",
                "-T",
            ]
            .map(String::from),
        );
        v.push(self.dest.clone());
        v.push("--".into());
        v.push(remote_cmd.into());
        v
    }
}

/// `file://host/a%20b` -> (`host`, `/a b`).
pub fn osc7(u: &str) -> Option<(String, PathBuf)> {
    let rest = u.strip_prefix("file://")?;
    let slash = rest.find('/')?;
    let host = rest[..slash].to_string();
    let b = &rest.as_bytes()[slash..];
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some((host, PathBuf::from(String::from_utf8_lossy(&out).into_owned())))
}

/// Same machine? Empty host and `localhost` are local; otherwise the short
/// names are compared case-insensitively (`box` == `box.lan`).
pub fn is_local_host(host: &str, local: &str) -> bool {
    let short = |h: &str| h.split('.').next().unwrap_or("").to_ascii_lowercase();
    host.is_empty() || host == "localhost" || short(host) == short(local)
}

/// This machine's hostname (cached).
pub fn local_hostname() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| {
        #[cfg(unix)]
        {
            let mut b = [0u8; 256];
            // SAFETY: the buffer is valid for its length; the result is NUL-terminated or truncated.
            if unsafe { libc::gethostname(b.as_mut_ptr().cast(), b.len()) } == 0 {
                let n = b.iter().position(|c| *c == 0).unwrap_or(b.len());
                return String::from_utf8_lossy(&b[..n]).into_owned();
            }
            String::new()
        }
        #[cfg(not(unix))]
        {
            std::env::var("COMPUTERNAME").unwrap_or_default()
        }
    })
}

/// Where the shell is, from the last OSC 7 (`osc7_url`) and the argv of the
/// terminal's foreground process (`fg`, if known). A foreground `ssh` means
/// remote even before (or without) a remote OSC 7, so a stale local folder is
/// never shown; a remote OSC 7 counts only when its host is not this one and
/// the local shell is not back in the foreground (`shell_fg`: after `exit`
/// the last report is the remote one, stale).
pub fn place(osc7_url: Option<&str>, fg: Option<&[String]>, shell_fg: bool, local_dir: Option<PathBuf>) -> Option<Place> {
    let ssh = fg.and_then(ssh_target);
    let remote_dir = osc7_url.and_then(osc7).filter(|(h, _)| !is_local_host(h, local_hostname())).filter(|_| ssh.is_some() || !shell_fg);
    match (ssh, remote_dir) {
        (Some(t), dir) => Some(Place::Remote { label: t.label(), dir: dir.map(|d| d.1), ssh: Some(t) }),
        (None, Some((host, d))) => Some(Place::Remote { label: host, dir: Some(d), ssh: None }),
        (None, None) => local_dir.map(Place::Local),
    }
}

/// ssh options that take a value (OpenSSH).
const WITH_VALUE: &str = "BbcDEeFIiJLlmOoPpQRSWw";

/// The destination and the options of an `ssh` argv (`argv[0]` is ssh).
/// `None` if it is not ssh or has no destination.
pub fn ssh_target(argv: &[String]) -> Option<SshTarget> {
    let prog = Path::new(argv.first()?).file_name()?.to_string_lossy().into_owned();
    if prog != "ssh" {
        return None;
    }
    let mut opts = Vec::new();
    let mut i = 1;
    while i < argv.len() {
        let a = &argv[i];
        if a == "--" {
            i += 1;
            break;
        }
        if let Some(flags) = a.strip_prefix('-').filter(|f| !f.is_empty()) {
            // -p22, -l user, -4v ...
            let first = flags.chars().next()?;
            if WITH_VALUE.contains(first) {
                if flags.len() > 1 {
                    opts.extend([format!("-{first}"), flags[1..].to_string()]);
                } else {
                    opts.extend([a.clone(), argv.get(i + 1)?.clone()]);
                    i += 1;
                }
            } else if flags.chars().all(|c| "46AaCKkXxYy".contains(c)) {
                // Flags that matter for reaching the host; -t/-N/-f/-v etc. dropped.
                opts.extend(flags.chars().filter(|c| "46".contains(*c)).map(|c| format!("-{c}")));
            }
            i += 1;
            continue;
        }
        break;
    }
    let dest = argv.get(i)?.clone();
    // Only the options needed to connect (not -L/-R/-D forwards, -W, -N).
    let mut keep = Vec::new();
    let mut j = 0;
    while j < opts.len() {
        let o = opts[j].as_str();
        match o {
            "-4" | "-6" => {
                keep.push(opts[j].clone());
                j += 1;
            }
            "-p" | "-l" | "-i" | "-F" | "-J" | "-o" | "-c" | "-m" | "-b" => {
                keep.extend([opts[j].clone(), opts[j + 1].clone()]);
                j += 2;
            }
            _ => j += 2,
        }
    }
    Some(SshTarget { opts: keep, dest })
}

/// POSIX `sh` for the remote side: the directory's long listing with epoch
/// times where `ls` can (GNU), plain `ls -la` elsewhere (BSD/macOS).
pub fn listing_command(dir: &Path) -> String {
    let q = crate::commander::shell_quote(&dir.to_string_lossy());
    format!("cd -- {q} && {{ LC_ALL=C ls -la --time-style=+%s 2>/dev/null || LC_ALL=C ls -la; }}")
}

/// Parse `ls -la` output (GNU with `+%s` times, or BSD) into entries under
/// `dir`; `.` and `..` are skipped.
pub fn parse_ls(out: &str, dir: &Path) -> Vec<Entry> {
    let mut v = Vec::new();
    for line in out.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() < 6 || t[0].starts_with("total") || t[0].len() < 10 {
            continue;
        }
        // mode links user group size then either <epoch> or <Mon dd hh:mm|year>.
        let epoch = t[5].len() >= 9 && t[5].chars().all(|c| c.is_ascii_digit());
        let skip = if epoch { 6 } else { 8 };
        let Some(name) = nth_rest(line, skip) else { continue };
        let name = name.split(" -> ").next().unwrap_or(name).to_string();
        if name == "." || name == ".." || name.is_empty() {
            continue;
        }
        let mode = t[0].to_string();
        let modified = epoch.then(|| t[5].parse::<u64>().ok()).flatten().map(|s| std::time::UNIX_EPOCH + std::time::Duration::from_secs(s));
        let dirish = mode.starts_with('d');
        let kind = if dirish { Kind::Dir } else { kind_of(&name) };
        // Remote paths are POSIX whatever the local OS.
        let path = PathBuf::from(format!("{}/{name}", dir.to_string_lossy().trim_end_matches('/')));
        v.push(Entry { path, name, kind, mode, size: t[4].parse().unwrap_or(0), modified });
    }
    v
}

/// `line` from its `n`th whitespace-separated field to the end.
fn nth_rest(line: &str, n: usize) -> Option<&str> {
    let mut s = line.trim_start();
    for _ in 0..n {
        let i = s.find(char::is_whitespace)?;
        s = s[i..].trim_start();
    }
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn osc7_and_hosts() {
        assert_eq!(osc7("file://web1/srv/a%20b"), Some(("web1".into(), "/srv/a b".into())));
        assert_eq!(osc7("file:///tmp"), Some(("".into(), "/tmp".into())));
        assert!(is_local_host("alien-blue.lan", "alien-blue") && is_local_host("", "x") && !is_local_host("web1", "alien-blue"));
    }

    #[test]
    fn place_never_shows_stale_local_files() {
        let local = Some(PathBuf::from("/home/m"));
        let here = format!("file://{}/home/m", local_hostname());
        let ssh = a(&["ssh", "web1"]);
        assert_eq!(place(Some(&here), None, true, local.clone()), Some(Place::Local("/home/m".into())));
        // ssh running, the remote shell has not reported yet: remote, folder unknown.
        let p = place(Some(&here), Some(&ssh), false, local.clone()).unwrap();
        assert!(matches!(&p, Place::Remote { label, dir: None, ssh: Some(_) } if label == "web1"), "{p:?}");
        let p = place(Some("file://web1/srv"), Some(&ssh), false, local.clone()).unwrap();
        assert!(matches!(&p, Place::Remote { dir: Some(d), .. } if d == Path::new("/srv")));
        // A remote OSC 7 without a visible ssh (mosh, nested): remote, no side channel.
        let p = place(Some("file://web1/srv"), Some(&a(&["mosh-client"])), false, local.clone()).unwrap();
        assert!(matches!(p, Place::Remote { ssh: None, .. }));
        // After `exit` the local shell is back: the remote report is stale.
        assert_eq!(place(Some("file://web1/srv"), Some(&a(&["bash"])), true, local), Some(Place::Local("/home/m".into())));
    }

    #[test]
    fn ssh_targets_from_argv() {
        let t = ssh_target(&a(&["ssh", "-p", "2222", "-i", "/k", "-L", "8080:x:80", "-tv", "me@web1"])).unwrap();
        assert_eq!(t.dest, "me@web1");
        assert_eq!(t.opts, a(&["-p", "2222", "-i", "/k"]));
        let t = ssh_target(&a(&["/usr/bin/ssh", "-F", "/tmp/cfg", "-lroot", "-4", "web1", "htop"])).unwrap();
        assert_eq!((t.dest.as_str(), t.label()), ("web1", "root@web1".to_string()));
        assert_eq!(t.opts, a(&["-F", "/tmp/cfg", "-l", "root", "-4"]));
        assert!(ssh_target(&a(&["bash"])).is_none());
        assert!(ssh_target(&a(&["ssh", "-p"])).is_none());
        let argv = t.argv(Path::new("/h/cache/ssh"), "ls");
        assert!(argv.contains(&"BatchMode=yes".to_string()) && argv.contains(&"ControlPath=/h/cache/ssh/%C".to_string()));
        assert_eq!(&argv[argv.len() - 3..], &a(&["web1", "--", "ls"])[..]);
    }

    #[test]
    fn parses_gnu_and_bsd_ls() {
        let gnu = "total 12\ndrwxr-xr-x 2 m m 4096 1700000000 .\ndrwxr-xr-x 9 m m 4096 1700000000 ..\n-rw-r--r-- 1 m m 5 1700000001 my file.png\nlrwxrwxrwx 1 m m 3 1700000002 ln -> x\ndrwxr-xr-x 2 m m 4096 1700000003 sub\n";
        let v = parse_ls(gnu, Path::new("/srv"));
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["my file.png", "ln", "sub"]);
        assert_eq!((v[0].kind, v[0].size, v[0].path.clone()), (Kind::Image, 5, PathBuf::from("/srv/my file.png")));
        assert_eq!(v[2].kind, Kind::Dir);
        assert!(v[0].modified.is_some());
        let bsd = "total 8\ndrwxr-xr-x  3 m  staff   96 Oct 31 20:16 .\n-rw-r--r--  1 m  staff  120 Jan  2  2025 notes.txt\n";
        let v = parse_ls(bsd, Path::new("/Users/m"));
        assert_eq!((v.len(), v[0].name.as_str(), v[0].size), (1, "notes.txt", 120));
        assert!(listing_command(Path::new("/a b")).starts_with("cd -- '/a b' && "));
    }
}
