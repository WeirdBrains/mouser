//! `TERM` for ssh sessions started in Mouser (docs/ssh.md).
//!
//! Mouser's shells run with `TERM=mouser`, and ssh sends the shell's `TERM`
//! to the server. A host without Mouser's terminfo then breaks tmux and other
//! curses programs (`missing or unsuitable terminal: mouser`). Mouser puts an
//! `ssh` shim first on its tabs' `PATH` (and the shell integration defines an
//! `ssh` function); both run [`run`], which:
//!
//! - passes everything that is not an interactive session with a terminal
//!   straight to the real ssh (scp, sftp, git, rsync, `-N`/`-W` tunnels,
//!   `-O` control commands, `-G`, `-V`, `-T`, subsystems, no local tty);
//! - for an interactive session, checks on the server, in the same connection,
//!   whether `mouser` resolves there (`infocmp mouser`, else the terminfo
//!   folders) and keeps `TERM=mouser` if so, else uses `xterm-256color`, then
//!   starts the login shell (or the command given with `-t`);
//! - falls back to `TERM=xterm-256color` locally when the session cannot be
//!   wrapped safely (a `RemoteCommand` in ssh_config, a `-t` command with a
//!   backslash or a newline).
//!
//! `MOUSER_SSH_TERM` (`auto`, `mouser`, `xterm-256color`), also per host as
//! `SetEnv MOUSER_SSH_TERM=…` in ssh_config, and an explicit `TERM=… ssh`
//! override the choice.
//!
//! For commander (docs/modes.md §SSH sessions) a wrapped session also:
//!
//! - shares its connection: `ControlMaster=auto` with Mouser's
//!   `ControlPath` ([`share_opts`]), so commander's side channel rides the
//!   login you already made (password, OTP, a key with a passphrase) instead
//!   of opening a second, non-interactive one. Not when ssh_config sets its
//!   own `ControlMaster`/`ControlPath`; `MOUSER_SSH_SHARE=no` (or per host
//!   `SetEnv MOUSER_SSH_SHARE=no`) turns it off;
//! - reports where the remote shell is: an OSC 7 with the starting folder
//!   and `OSC 1337 ; MouserRemoteShell=<pid>` with the login shell's pid
//!   ([`REMOTE_REPORT`]), so commander can follow its folder without any
//!   remote shell integration. The shim clears the pid before each ssh.

use std::path::{Path, PathBuf};

/// The `TERM` a session falls back to.
pub const FALLBACK: &str = "xterm-256color";
/// User override: `auto` (default), `mouser`, `xterm-256color`.
pub const ENV_MODE: &str = "MOUSER_SSH_TERM";
/// Set on the real ssh so a wrapper script that calls `ssh` again is passed through.
pub const ENV_GUARD: &str = "MOUSER_SSH_WRAPPED";
/// Where the shim is (for the shell integration's `ssh` function).
pub const ENV_WRAPPER: &str = "MOUSER_SSH_WRAPPER";
/// `no` / `off`: don't share the session's connection with commander.
pub const ENV_SHARE: &str = "MOUSER_SSH_SHARE";
/// Clears the remote shell pid Mouser keeps for the tab (printed by the shim).
pub const CLEAR_REMOTE_SHELL: &str = "\x1b]1337;MouserRemoteShell=\x07";

/// ssh options that take a value (OpenSSH).
const WITH_VALUE: &str = "BbcDEeFIiJLlmOoPpQRSWw";
/// Options that never open an interactive session: no tunnel-only (-N), stdio
/// forward (-W), control (-O), config dump (-G), version (-V), query (-Q),
/// subsystem (-s), no tty (-T), background (-f), stdin from /dev/null (-n).
const NOT_A_SESSION: &str = "NWOGVQsTfn";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Check the server and fall back when `mouser` is unknown there.
    Auto,
    /// Always send `TERM=mouser` (Mouser does nothing).
    Mouser,
    /// Always send `TERM=xterm-256color` for sessions with a terminal.
    Fallback,
}

impl Mode {
    pub fn parse(s: Option<&str>) -> Mode {
        match s.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("mouser" | "off" | "no" | "never" | "0" | "false") => Mode::Mouser,
            Some("xterm" | "xterm-256color" | "fallback" | "always") => Mode::Fallback,
            _ => Mode::Auto,
        }
    }
}

/// Where the destination and the remote command are in an ssh argv
/// (without argv\[0\]), and every option letter seen.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Args {
    pub dest: Option<usize>,
    pub command: Option<usize>,
    pub flags: String,
}

impl Args {
    pub fn has(&self, c: char) -> bool {
        self.flags.contains(c)
    }
}

/// Parse like OpenSSH: options, the destination, options again (ssh accepts
/// `ssh host -p 22`), then the remote command. `--` ends the options. `None`
/// when an option is missing its value.
pub fn parse(args: &[String]) -> Option<Args> {
    let mut out = Args::default();
    let mut i = 0;
    let mut ended = false;
    loop {
        while !ended && i < args.len() {
            let a = &args[i];
            if a == "--" {
                ended = true;
                i += 1;
                break;
            }
            let Some(opts) = a.strip_prefix('-').filter(|o| !o.is_empty()) else { break };
            for (k, c) in opts.char_indices() {
                out.flags.push(c);
                if WITH_VALUE.contains(c) {
                    if k + c.len_utf8() == opts.len() {
                        args.get(i + 1)?;
                        i += 1;
                    }
                    break;
                }
            }
            i += 1;
        }
        if i >= args.len() {
            return Some(out);
        }
        if out.dest.is_none() {
            out.dest = Some(i);
            i += 1;
            // ssh re-reads options after the destination unless `--` came first.
            continue;
        }
        out.command = Some(i);
        return Some(out);
    }
}

/// The parts of `ssh -G` that matter here.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Config {
    pub user: String,
    pub request_tty: String,
    pub session_type: String,
    pub remote_command: bool,
    pub stdin_null: bool,
    pub fork: bool,
    /// `SetEnv MOUSER_SSH_TERM=…` for this host.
    pub mode: Option<Mode>,
    /// `controlmaster` (`false` when unset).
    pub control_master: String,
    /// `controlpath` (`none` when unset).
    pub control_path: String,
    /// `SetEnv MOUSER_SSH_SHARE=…` for this host.
    pub share: Option<bool>,
}

pub fn parse_config(out: &str) -> Config {
    let mut c = Config {
        request_tty: "auto".into(),
        session_type: "default".into(),
        control_master: "false".into(),
        control_path: "none".into(),
        ..Config::default()
    };
    for line in out.lines() {
        let (k, v) = line.split_once(' ').unwrap_or((line, ""));
        let v = v.trim();
        match k.to_ascii_lowercase().as_str() {
            "user" => c.user = v.to_string(),
            "requesttty" => c.request_tty = v.to_ascii_lowercase(),
            "sessiontype" => c.session_type = v.to_ascii_lowercase(),
            "remotecommand" => c.remote_command = !v.is_empty() && !v.eq_ignore_ascii_case("none"),
            "stdinnull" => c.stdin_null = yes(v),
            "forkafterauthentication" => c.fork = yes(v),
            "controlmaster" => c.control_master = v.to_ascii_lowercase(),
            "controlpath" => c.control_path = v.to_string(),
            "setenv" => {
                for t in v.split_whitespace() {
                    let t = t.trim_matches('"');
                    if let Some(m) = t.strip_prefix("MOUSER_SSH_TERM=") {
                        c.mode = Some(Mode::parse(Some(m.trim_matches('"'))));
                    }
                    if let Some(m) = t.strip_prefix("MOUSER_SSH_SHARE=") {
                        c.share = Some(share_on(Some(m.trim_matches('"'))));
                    }
                }
            }
            _ => {}
        }
    }
    c
}

fn yes(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "yes" | "true")
}

/// `MOUSER_SSH_SHARE`: on unless `no`, `off`, `0`, `false` or `never`.
pub fn share_on(v: Option<&str>) -> bool {
    !matches!(v.map(|s| s.trim().to_ascii_lowercase()).as_deref(), Some("no" | "off" | "0" | "false" | "never"))
}

/// Options that make an interactive session the ControlMaster commander's
/// side channel uses (`control_dir`: Mouser's socket folder, the same
/// `ControlPath` as `remote::SshTarget::argv`). The master stays up 60 s
/// after the last session so `exit` never waits for a listing in flight.
/// Nothing when sharing is off or ssh_config already multiplexes the host
/// (its own `ControlMaster` or `ControlPath` is left alone), and nothing
/// when the socket path would be too long for a Unix socket (ssh would
/// refuse the login, so the session just doesn't share).
pub fn share_opts(cfg: &Config, env_on: bool, control_dir: &Path) -> Vec<String> {
    let on = cfg.share.unwrap_or(env_on);
    let own = !matches!(cfg.control_master.as_str(), "false" | "no") || !cfg.control_path.eq_ignore_ascii_case("none");
    if !on || own || !socket_fits(control_dir) {
        return Vec::new();
    }
    [
        "-o".to_string(),
        "ControlMaster=auto".into(),
        "-o".into(),
        format!("ControlPath={}", control_dir.join("%C").display()),
        "-o".into(),
        "ControlPersist=60".into(),
    ]
    .into()
}

/// Whether `<dir>/%C` fits a Unix socket address: `%C` is 40 hex digits and
/// ssh binds a temporary `<path>.<16 chars>` first; macOS allows 103 bytes
/// (Linux 107).
pub fn socket_fits(control_dir: &Path) -> bool {
    control_dir.as_os_str().len() + 1 + 40 + 17 <= 103
}

/// What the shim does with one ssh invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Run the real ssh with these exact arguments and environment.
    Pass,
    /// Run it unchanged but with `TERM=xterm-256color`.
    Fallback,
    /// Run it with these arguments (the server-side check added).
    Probe(Vec<String>),
}

/// The local side of the decision.
#[derive(Debug, Clone, Copy)]
pub struct Facts<'a> {
    pub term: Option<&'a str>,
    pub mode: Mode,
    /// Already inside the shim (a wrapper script calling ssh again).
    pub wrapped: bool,
    pub stdin_tty: bool,
}

/// Decide without `ssh -G` where possible: `Err(plan)` is final, `Ok(args)`
/// means the config is needed.
pub fn early(args: &[String], f: &Facts) -> Result<Args, Plan> {
    if f.wrapped || f.term != Some("mouser") || f.mode == Mode::Mouser {
        return Err(Plan::Pass);
    }
    let Some(p) = parse(args) else { return Err(Plan::Pass) };
    if p.dest.is_none() || p.flags.chars().any(|c| NOT_A_SESSION.contains(c)) {
        return Err(Plan::Pass);
    }
    // No local terminal (scripts, pipes, git, rsync, ansible): left alone,
    // even with -tt.
    if !f.stdin_tty {
        return Err(Plan::Pass);
    }
    Ok(p)
}

/// The arguments for `ssh -G`: everything but the remote command.
pub fn config_args(args: &[String], p: &Args) -> Vec<String> {
    args[..p.command.unwrap_or(args.len())].to_vec()
}

/// The full decision, given `ssh -G`'s view of this command line.
pub fn decide(args: &[String], p: &Args, cfg: &Config, f: &Facts) -> Plan {
    let mode = cfg.mode.unwrap_or(f.mode);
    if mode == Mode::Mouser || cfg.session_type != "default" || cfg.stdin_null || cfg.fork {
        return Plan::Pass;
    }
    let has_cmd = p.command.is_some() || cfg.remote_command;
    let tty = match cfg.request_tty.as_str() {
        "force" | "yes" | "true" => true,
        "auto" => !has_cmd,
        _ => false,
    };
    // No remote terminal: TERM is not sent. Forges (git@) never run a shell.
    if !tty || cfg.user == "git" {
        return Plan::Pass;
    }
    if mode == Mode::Fallback || cfg.remote_command {
        return Plan::Fallback;
    }
    match p.command {
        Some(k) => {
            let cmd = args[k..].join(" ");
            if !safe(&cmd) {
                return Plan::Fallback;
            }
            let mut v = args[..k].to_vec();
            v.push(remote_command(&cmd));
            Plan::Probe(v)
        }
        None => {
            let mut v = Vec::with_capacity(args.len() + 2);
            if cfg.request_tty == "auto" {
                // ssh would have made a terminal for the login shell; it does
                // not for a command unless asked.
                v.push("-t".to_string());
            }
            v.extend_from_slice(args);
            v.push(remote_login());
            Plan::Probe(v)
        }
    }
}

/// POSIX sh run on the server: `TERM=mouser` when its terminfo resolves there
/// (`infocmp`, or the usual folders when there is no infocmp), else
/// `xterm-256color`. No `'`, `\` or `!`, so it can sit in single quotes for
/// any login shell (sh, bash, zsh, fish, csh).
pub const REMOTE_CHECK: &str = concat!(
    "t=xterm-256color; ",
    "for d in \"$TERMINFO\" \"$HOME/.terminfo\" /etc/terminfo /lib/terminfo /usr/share/terminfo /usr/lib/terminfo /usr/share/lib/terminfo /usr/local/share/terminfo; do ",
    "if [ -n \"$d\" ] && { [ -e \"$d/m/mouser\" ] || [ -e \"$d/6d/mouser\" ]; }; then t=mouser; fi; done; ",
    "if command -v infocmp >/dev/null 2>&1; then if infocmp mouser >/dev/null 2>&1; then t=mouser; else t=xterm-256color; fi; fi; ",
    "TERM=$t; export TERM"
);

/// POSIX sh run on the server just before the login shell replaces it (same
/// pid): an OSC 7 with the folder it starts in and `OSC 1337 ;
/// MouserRemoteShell=<pid>`, so commander lists the host and follows the
/// shell's folder (`/proc/<pid>/cwd`, `lsof` on BSD/macOS) with no remote
/// shell integration. Only `\0` escapes (fish and csh keep them in single
/// quotes), no `'` or `!`.
pub const REMOTE_REPORT: &str = "printf \"\\033]7;file://%s%s\\007\\033]1337;MouserRemoteShell=%s\\007\" \"$(uname -n 2>/dev/null)\" \"$PWD\" \"$$\"";

/// The remote command for an interactive login: the check, the report, the
/// message of the day sshd skips for commands (unless `~/.hushlogin`), then
/// the login shell.
pub fn remote_login() -> String {
    format!(
        "sh -c '{REMOTE_CHECK}; {REMOTE_REPORT}; if [ -e \"$HOME/.hushlogin\" ]; then :; else cat /run/motd.dynamic /etc/motd 2>/dev/null; fi; exec \"${{SHELL:-/bin/sh}}\" -l'"
    )
}

/// The remote command for `ssh -t host CMD`: the check and the report, then
/// CMD through the login shell exactly as sshd would run it (`$SHELL -c CMD`).
pub fn remote_command(cmd: &str) -> String {
    format!("sh -c '{REMOTE_CHECK}; {REMOTE_REPORT}; exec \"${{SHELL:-/bin/sh}}\" -c \"$1\"' mouser-ssh {}", quote(cmd))
}

/// Whether `cmd` survives one more layer of single quotes in every common
/// login shell (fish reads `\` inside single quotes; csh dislikes newlines).
fn safe(cmd: &str) -> bool {
    !cmd.contains(['\\', '\n', '\r', '\0'])
}

/// Single-quote for sh (`'` becomes `'\''`).
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The first `ssh` on `path` that is not Mouser itself (`me`: this
/// executable, canonical) or a link to it.
pub fn find_real_ssh(path: Option<&std::ffi::OsStr>, me: Option<&Path>) -> Option<PathBuf> {
    let name = if cfg!(windows) { "ssh.exe" } else { "ssh" };
    let me = me.and_then(|m| std::fs::canonicalize(m).ok());
    std::env::split_paths(path?).map(|d| d.join(name)).find(|c| {
        let Ok(real) = std::fs::canonicalize(c) else { return false };
        real.is_file() && executable(&real) && me.as_deref() != Some(real.as_path())
    })
}

fn executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        true
    }
}

/// Make `dir/ssh` a link to `exe` (atomic; left alone when it already is).
/// Returns the shim's path.
#[cfg(unix)]
pub fn install_shim(dir: &Path, exe: &Path) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let shim = dir.join("ssh");
    if std::fs::read_link(&shim).is_ok_and(|t| t == exe) {
        return Ok(shim);
    }
    let tmp = dir.join(format!(".ssh.{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(exe, &tmp)?;
    std::fs::rename(&tmp, &shim)?;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    Ok(shim)
}

/// Run ssh for `args` (without argv\[0\]) as decided above; never returns.
/// The real ssh replaces this process on Unix (same pid, terminal, signals).
pub fn run(args: Vec<String>) -> ! {
    use std::io::IsTerminal;
    let me = std::env::current_exe().ok();
    let Some(real) = find_real_ssh(std::env::var_os("PATH").as_deref(), me.as_deref()) else {
        eprintln!("mouser: ssh not found on PATH");
        std::process::exit(127);
    };
    let term = std::env::var("TERM").ok();
    let mode = std::env::var(ENV_MODE).ok();
    let facts = Facts {
        term: term.as_deref(),
        mode: Mode::parse(mode.as_deref()),
        wrapped: std::env::var_os(ENV_GUARD).is_some(),
        stdin_tty: std::io::stdin().is_terminal(),
    };
    let mut cfg = None;
    let plan = match early(&args, &facts) {
        Err(p) => p,
        Ok(p) => {
            let out = std::process::Command::new(&real)
                .arg("-G")
                .args(config_args(&args, &p))
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output();
            match out {
                Ok(o) if o.status.success() => {
                    let c = parse_config(&String::from_utf8_lossy(&o.stdout));
                    let plan = decide(&args, &p, &c, &facts);
                    cfg = Some(c);
                    plan
                }
                // ssh will report the bad command line itself.
                _ => Plan::Pass,
            }
        }
    };
    // A new session in a Mouser tab: forget the last remote shell's pid.
    if facts.term == Some("mouser") && facts.stdin_tty && std::io::stdout().is_terminal() {
        use std::io::Write;
        let mut o = std::io::stdout();
        let _ = o.write_all(CLEAR_REMOTE_SHELL.as_bytes()).and_then(|_| o.flush());
    }
    let mut cmd = std::process::Command::new(&real);
    cmd.env(ENV_GUARD, "1");
    let share = |cfg: &Config| share_opts(cfg, share_on(std::env::var(ENV_SHARE).ok().as_deref()), &crate::fetch::control_dir());
    match (plan, cfg) {
        (Plan::Pass, _) => cmd.args(&args),
        (Plan::Fallback, c) => cmd.args(c.as_ref().map(share).unwrap_or_default()).args(&args).env("TERM", FALLBACK),
        (Plan::Probe(v), c) => cmd.args(c.as_ref().map(share).unwrap_or_default()).args(v),
    };
    exec(cmd)
}

#[cfg(unix)]
fn exec(mut cmd: std::process::Command) -> ! {
    use std::os::unix::process::CommandExt;
    // argv[0] stays `ssh` (Mouser spots a foreground ssh by its name).
    let e = cmd.arg0("ssh").exec();
    eprintln!("mouser: cannot run ssh: {e}");
    std::process::exit(127);
}

#[cfg(not(unix))]
fn exec(mut cmd: std::process::Command) -> ! {
    match cmd.status() {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("mouser: cannot run ssh: {e}");
            std::process::exit(127);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    const TTY: Facts<'static> = Facts { term: Some("mouser"), mode: Mode::Auto, wrapped: false, stdin_tty: true };

    fn cfg() -> Config {
        parse_config("user me\nrequesttty auto\nsessiontype default\nstdinnull no\nforkafterauthentication no\n")
    }

    /// The plan for `args` with ssh -G reporting `c`.
    fn plan(args: &[&str], c: &Config, f: &Facts) -> Plan {
        let args = a(args);
        match early(&args, f) {
            Err(p) => p,
            Ok(p) => decide(&args, &p, c, f),
        }
    }

    #[test]
    fn modes() {
        assert_eq!(Mode::parse(None), Mode::Auto);
        assert_eq!(Mode::parse(Some("auto")), Mode::Auto);
        assert_eq!(Mode::parse(Some("Mouser")), Mode::Mouser);
        assert_eq!(Mode::parse(Some("off")), Mode::Mouser);
        assert_eq!(Mode::parse(Some("xterm-256color")), Mode::Fallback);
    }

    #[test]
    fn parses_like_openssh() {
        let p = |v: &[&str]| parse(&a(v)).unwrap();
        assert_eq!(p(&["web1"]), Args { dest: Some(0), command: None, flags: "".into() });
        assert_eq!(p(&["-p", "22", "-i", "/k", "web1"]).dest, Some(4));
        assert_eq!(p(&["-p22", "web1"]).dest, Some(1));
        assert_eq!(p(&["-vvtp", "22", "web1"]), Args { dest: Some(2), command: None, flags: "vvtp".into() });
        // Options after the destination are still options.
        assert_eq!(p(&["web1", "-p", "22"]), Args { dest: Some(0), command: None, flags: "p".into() });
        assert_eq!(p(&["web1", "-t", "tmux", "a"]), Args { dest: Some(0), command: Some(2), flags: "t".into() });
        assert_eq!(p(&["web1", "ls", "-l"]).command, Some(1));
        assert_eq!(p(&["web1", "--", "ls"]).command, Some(2));
        // `--` before the destination: no options after it.
        assert_eq!(p(&["--", "web1", "-t"]), Args { dest: Some(1), command: Some(2), flags: "".into() });
        assert_eq!(p(&["-o", "RemoteCommand=x", "-J", "jump", "web1"]).flags, "oJ");
        assert_eq!(p(&[]).dest, None);
        assert!(parse(&a(&["-p"])).is_none());
        assert!(parse(&a(&["web1", "-l"])).is_none());
    }

    #[test]
    fn reads_ssh_g() {
        let c = parse_config(
            "user git\nrequesttty force\nsessiontype none\nremotecommand tmux new -A\nstdinnull yes\nforkafterauthentication yes\nsetenv FOO=1 MOUSER_SSH_TERM=xterm-256color\n",
        );
        assert_eq!(c.user, "git");
        assert_eq!(c.request_tty, "force");
        assert_eq!(c.session_type, "none");
        assert!(c.remote_command && c.stdin_null && c.fork);
        assert_eq!(c.mode, Some(Mode::Fallback));
        let d = cfg();
        assert!(!d.remote_command && d.mode.is_none() && d.request_tty == "auto");
    }

    #[test]
    fn interactive_login_is_checked_on_the_server() {
        let Plan::Probe(v) = plan(&["-p", "2222", "me@web1"], &cfg(), &TTY) else { panic!() };
        assert_eq!(v[..4], a(&["-t", "-p", "2222", "me@web1"]));
        assert_eq!(v.len(), 5);
        assert!(v[4].starts_with("sh -c '") && v[4].contains("infocmp mouser") && v[4].ends_with("-l'"));
        // The wrapped line still parses: same destination, the check is the command.
        let p = parse(&v).unwrap();
        assert_eq!((p.dest, p.command), (Some(3), Some(4)));
        // Options after the host and ProxyJump are kept.
        let Plan::Probe(v) = plan(&["-J", "bastion", "web1", "-A"], &cfg(), &TTY) else { panic!() };
        assert_eq!(v[..5], a(&["-t", "-J", "bastion", "web1", "-A"]));
        // RequestTTY yes/force already gives the command a terminal: no extra -t.
        let mut c = cfg();
        c.request_tty = "yes".into();
        let Plan::Probe(v) = plan(&["-t", "web1"], &c, &TTY) else { panic!() };
        assert_eq!(v[..2], a(&["-t", "web1"]));
    }

    #[test]
    fn tty_command_is_wrapped_and_kept() {
        let mut c = cfg();
        c.request_tty = "yes".into();
        let Plan::Probe(v) = plan(&["-t", "web1", "tmux", "new", "-A", "-s", "it's"], &c, &TTY) else { panic!() };
        assert_eq!(v[..2], a(&["-t", "web1"]));
        assert_eq!(v.len(), 3);
        assert!(v[2].ends_with(r#" mouser-ssh 'tmux new -A -s it'\''s'"#), "{}", v[2]);
        // A backslash would not survive fish's single quotes: fall back instead.
        assert_eq!(plan(&["-t", "web1", r"printf a\\n"], &c, &TTY), Plan::Fallback);
        // A command without a terminal sends no TERM.
        assert_eq!(plan(&["web1", "uptime"], &cfg(), &TTY), Plan::Pass);
    }

    #[test]
    fn non_sessions_pass_through() {
        for args in [
            &["-N", "-L", "8080:localhost:80", "web1"][..],
            &["-W", "%h:%p", "bastion"],
            &["-O", "check", "web1"],
            &["-O", "exit", "web1"],
            &["-G", "web1"],
            &["-V"],
            &["-T", "git@github.com"],
            &["-s", "web1", "sftp"],
            &["-f", "-N", "-D", "1080", "web1"],
            &["-n", "web1", "true"],
            &[],
        ] {
            assert_eq!(plan(args, &cfg(), &TTY), Plan::Pass, "{args:?}");
        }
        // scp / rsync / git run ssh without a terminal.
        let pipe = Facts { stdin_tty: false, ..TTY };
        for args in [
            &["-x", "-oPermitLocalCommand=no", "-oClearAllForwardings=yes", "--", "web1", "scp", "-t", "/tmp"][..],
            &["git@github.com", "git-upload-pack 'me/repo.git'"],
            &["-l", "me", "web1", "rsync", "--server", "-e.LsfxCIvu", ".", "/x"],
            &["-tt", "web1", "sudo", "true"],
            &["web1"],
        ] {
            assert_eq!(plan(args, &cfg(), &pipe), Plan::Pass, "{args:?}");
        }
    }

    #[test]
    fn config_and_overrides() {
        let mut c = cfg();
        c.remote_command = true;
        c.request_tty = "yes".into();
        // RemoteCommand in ssh_config cannot be wrapped: fall back.
        assert_eq!(plan(&["web1"], &c, &TTY), Plan::Fallback);
        let mut c = cfg();
        c.request_tty = "no".into();
        assert_eq!(plan(&["web1"], &c, &TTY), Plan::Pass);
        let mut c = cfg();
        c.session_type = "none".into();
        assert_eq!(plan(&["web1"], &c, &TTY), Plan::Pass);
        let mut c = cfg();
        c.user = "git".into();
        assert_eq!(plan(&["gitlab.com"], &c, &TTY), Plan::Pass);
        // An explicit TERM, MOUSER_SSH_TERM and the guard.
        assert_eq!(plan(&["web1"], &cfg(), &Facts { term: Some("xterm-kitty"), ..TTY }), Plan::Pass);
        assert_eq!(plan(&["web1"], &cfg(), &Facts { term: None, ..TTY }), Plan::Pass);
        assert_eq!(plan(&["web1"], &cfg(), &Facts { mode: Mode::Mouser, ..TTY }), Plan::Pass);
        assert_eq!(plan(&["web1"], &cfg(), &Facts { mode: Mode::Fallback, ..TTY }), Plan::Fallback);
        assert_eq!(plan(&["web1"], &cfg(), &Facts { wrapped: true, ..TTY }), Plan::Pass);
        // Per host: SetEnv MOUSER_SSH_TERM=… wins.
        let mut c = cfg();
        c.mode = Some(Mode::Fallback);
        assert_eq!(plan(&["winbox"], &c, &TTY), Plan::Fallback);
        c.mode = Some(Mode::Mouser);
        assert_eq!(plan(&["web1"], &c, &TTY), Plan::Pass);
    }

    #[test]
    fn sessions_share_their_connection_unless_configured() {
        let d = Path::new("/h/.mouser/cache/ssh");
        let o = share_opts(&cfg(), true, d);
        assert_eq!(
            o,
            a(&["-o", "ControlMaster=auto", "-o", "-", "-o", "ControlPersist=60"])
                .into_iter()
                .enumerate()
                .map(|(i, s)| if i == 3 { format!("ControlPath={}", d.join("%C").display()) } else { s })
                .collect::<Vec<_>>()
        );
        assert!(share_opts(&cfg(), false, d).is_empty(), "MOUSER_SSH_SHARE=no");
        // ssh_config multiplexes the host itself: left alone.
        let own = parse_config("user me\ncontrolmaster auto\ncontrolpath ~/.ssh/cm-%C\n");
        assert_eq!((own.control_master.as_str(), own.control_path.as_str()), ("auto", "~/.ssh/cm-%C"));
        assert!(share_opts(&own, true, d).is_empty());
        assert!(share_opts(&parse_config("controlpath /tmp/x\n"), true, d).is_empty());
        // Per host: SetEnv MOUSER_SSH_SHARE=no wins over the environment, and yes too.
        let off = parse_config("user me\nsetenv MOUSER_SSH_SHARE=no\n");
        assert_eq!(off.share, Some(false));
        assert!(share_opts(&off, true, d).is_empty());
        assert!(!share_opts(&parse_config("setenv MOUSER_SSH_SHARE=yes\n"), false, d).is_empty());
        assert!(share_on(None) && share_on(Some("auto")) && !share_on(Some("Off")) && !share_on(Some("0")));
        assert_eq!(CLEAR_REMOTE_SHELL, "\x1b]1337;MouserRemoteShell=\x07");
        // A MOUSER_HOME so deep the socket can't be bound: the login still works, unshared.
        let deep = Path::new("/mnt/some/very/long/scratch/folder/for/tests/home/cache/ssh");
        assert!(!socket_fits(deep) && share_opts(&cfg(), true, deep).is_empty());
        assert!(socket_fits(Path::new("/home/someone-long-name/.mouser/cache/ssh")));
    }

    #[test]
    fn remote_scripts_are_quote_safe() {
        let login = remote_login();
        let inner = login.strip_prefix("sh -c '").and_then(|s| s.strip_suffix('\'')).unwrap();
        let cmd = remote_command("x");
        let cmd_inner = cmd.strip_prefix("sh -c '").and_then(|s| s.split_once("' mouser-ssh ")).unwrap().0;
        for s in [REMOTE_CHECK, REMOTE_REPORT, inner, cmd_inner] {
            assert!(!s.contains(['\'', '!', '\n']), "{s}");
            // Only `\0…` octal escapes: fish reads `\\` and `\'` in single quotes.
            assert!(s.match_indices('\\').all(|(i, _)| s[i + 1..].starts_with('0')), "{s}");
        }
        assert_eq!(quote("a'b"), r"'a'\''b'");
    }

    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// Run `remote` the way sshd does (`LOGIN_SHELL -c remote`) with a fake
    /// user shell that prints what it got, and `infocmp` answering `found`.
    #[cfg(unix)]
    fn on_server(login_shell: &Path, remote: &str, found: Option<bool>, terminfo: bool) -> String {
        let t = tempfile::tempdir().unwrap();
        let bin = t.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        for tool in ["sh", "cat"] {
            let real = ["/bin", "/usr/bin"].iter().map(|d| Path::new(d).join(tool)).find(|p| p.exists()).unwrap();
            std::os::unix::fs::symlink(real, bin.join(tool)).unwrap();
        }
        if let Some(f) = found {
            script(&bin, "infocmp", &format!("[ \"$1\" = mouser ] && exit {}; exit 1", if f { 0 } else { 1 }));
        }
        let home = t.path().join("home");
        std::fs::create_dir_all(home.join(".terminfo/m")).unwrap();
        if terminfo {
            std::fs::write(home.join(".terminfo/m/mouser"), "").unwrap();
        }
        std::fs::write(home.join(".hushlogin"), "").unwrap();
        let user_shell = script(t.path(), "usershell", "echo \"TERM=$TERM ARGS=$*\"");
        let out = std::process::Command::new(login_shell)
            .current_dir(&home)
            .arg("-c")
            .arg(remote)
            .env_clear()
            .env("PATH", &bin)
            .env("HOME", &home)
            .env("TERM", "mouser")
            .env("SHELL", &user_shell)
            .output()
            .unwrap();
        // stdout only: a login shell's own startup noise (vendor configs) goes to stderr.
        // The report (OSC 7 + the pid) comes first; checked, then dropped.
        let o = String::from_utf8_lossy(&out.stdout).into_owned();
        let report = format!("\x1b]7;file://{}\x07\x1b]1337;MouserRemoteShell=", home.display());
        let rest = o.strip_prefix(&report).unwrap_or_else(|| panic!("no report in {o:?}"));
        let (pid, rest) = rest.split_once('\x07').unwrap();
        assert!(pid.parse::<u32>().is_ok_and(|p| p > 0), "{pid:?}");
        rest.trim().to_string()
    }

    /// Login shells to try the remote command in (those installed here).
    #[cfg(unix)]
    fn login_shells() -> Vec<PathBuf> {
        ["/bin/sh", "/bin/bash", "/usr/bin/bash", "/usr/bin/zsh", "/bin/zsh", "/usr/bin/fish", "/usr/bin/dash", "/bin/tcsh", "/usr/bin/tcsh"]
            .iter()
            .map(PathBuf::from)
            .filter(|p| p.exists())
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn server_side_check_picks_term() {
        for sh in login_shells() {
            let login = remote_login();
            assert_eq!(on_server(&sh, &login, Some(true), false), "TERM=mouser ARGS=-l", "{sh:?}");
            assert_eq!(on_server(&sh, &login, Some(false), false), "TERM=xterm-256color ARGS=-l", "{sh:?}");
            // No infocmp: the terminfo folders decide.
            assert_eq!(on_server(&sh, &login, None, true), "TERM=mouser ARGS=-l", "{sh:?}");
            let cmd = remote_command("tmux new -A -s 'it'\"'\"'s' $HOME");
            assert_eq!(on_server(&sh, &cmd, Some(false), false), "TERM=xterm-256color ARGS=-c tmux new -A -s 'it'\"'\"'s' $HOME", "{sh:?}");
            assert_eq!(on_server(&sh, &cmd, Some(true), false), "TERM=mouser ARGS=-c tmux new -A -s 'it'\"'\"'s' $HOME", "{sh:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn finds_the_real_ssh_not_the_shim() {
        let t = tempfile::tempdir().unwrap();
        let me = script(t.path(), "mouser", "exit 0");
        let shim = install_shim(&t.path().join("shim"), &me).unwrap();
        assert_eq!(std::fs::read_link(&shim).unwrap(), me);
        // Idempotent.
        install_shim(&t.path().join("shim"), &me).unwrap();
        let sys = t.path().join("sys");
        std::fs::create_dir(&sys).unwrap();
        let real = script(&sys, "ssh", "exit 0");
        let path = std::env::join_paths([t.path().join("shim"), t.path().join("missing"), sys.clone()]).unwrap();
        assert_eq!(find_real_ssh(Some(&path), Some(&me)), Some(real));
        let only_shim = std::env::join_paths([t.path().join("shim")]).unwrap();
        assert_eq!(find_real_ssh(Some(&only_shim), Some(&me)), None);
    }
}
