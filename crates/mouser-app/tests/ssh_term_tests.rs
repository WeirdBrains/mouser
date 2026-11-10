//! The `ssh` shim end to end (docs/ssh.md): a link named ssh to the Mouser
//! binary, a fake real ssh that plays the server, in a real PTY.
#![cfg(unix)]

use mouser_pty::portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn script(p: &Path, body: &str) {
    std::fs::write(p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct World {
    t: tempfile::TempDir,
    shim: PathBuf,
    path: std::ffi::OsString,
    server_path: PathBuf,
}

/// shim/ssh -> mouser; sys/ssh: `-G` prints $FAKE_G, otherwise reports what it
/// got and runs a remote command the way sshd would ($SHELL -c, $SHELL a fake
/// login shell that prints TERM).
fn world(infocmp_finds_mouser: bool) -> World {
    let t = tempfile::tempdir().unwrap();
    let shim_dir = t.path().join("shim");
    let shim = mouser_core::ssh_term::install_shim(&shim_dir, Path::new(env!("CARGO_BIN_EXE_mouser-app"))).unwrap();
    let sys = t.path().join("sys");
    let server = t.path().join("server");
    std::fs::create_dir_all(&sys).unwrap();
    std::fs::create_dir_all(&server).unwrap();
    for tool in ["sh", "cat"] {
        let real = ["/bin", "/usr/bin"].iter().map(|d| Path::new(d).join(tool)).find(|p| p.exists()).unwrap();
        std::os::unix::fs::symlink(real, server.join(tool)).unwrap();
    }
    script(&server.join("infocmp"), &format!("exit {}", if infocmp_finds_mouser { 0 } else { 1 }));
    script(&server.join("usershell"), "echo \"REMOTE TERM=$TERM ARGS=$*\"");
    script(
        &sys.join("ssh"),
        r#"if [ "$1" = -G ]; then printf '%b' "$FAKE_G"; exit 0; fi
echo "LOCAL TERM=$TERM WRAPPED=$MOUSER_SSH_WRAPPED ARGC=$# ARGV=$*"
for last; do :; done
case $last in
  "sh -c "*) HOME=/nonexistent SHELL=$SERVER/usershell PATH=$SERVER sh -c "$last" ;;
esac"#,
    );
    let path = std::env::join_paths([shim_dir, sys, PathBuf::from("/usr/bin"), PathBuf::from("/bin")]).unwrap();
    World { shim, path, server_path: server, t }
}

const G: &str = "user me\\nrequesttty auto\\nsessiontype default\\nstdinnull no\\nforkafterauthentication no\\n";

/// Run the shim in a PTY (a local terminal, like a Mouser tab).
fn in_pty(w: &World, args: &[&str], g: &str, env: &[(&str, &str)]) -> String {
    let pair = native_pty_system().openpty(PtySize { rows: 24, cols: 200, pixel_width: 0, pixel_height: 0 }).unwrap();
    let mut cmd = CommandBuilder::new(&w.shim);
    cmd.args(args);
    cmd.env("PATH", &w.path);
    cmd.env("TERM", "mouser");
    cmd.env("FAKE_G", g);
    cmd.env("SERVER", &w.server_path);
    cmd.env_remove("MOUSER_SSH_TERM");
    cmd.env_remove("MOUSER_SSH_WRAPPED");
    cmd.env_remove("MOUSER_SSH_SHARE");
    // ControlPath sockets go under this home, never the real ~/.mouser.
    cmd.env("MOUSER_HOME", w.t.path().join("home"));
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut r = pair.master.try_clone_reader().unwrap();
    let h = std::thread::spawn(move || {
        let mut s = Vec::new();
        let mut b = [0u8; 4096];
        while let Ok(n) = r.read(&mut b) {
            if n == 0 {
                break;
            }
            s.extend_from_slice(&b[..n]);
        }
        String::from_utf8_lossy(&s).replace('\r', "")
    });
    child.wait().unwrap();
    drop(pair.master);
    h.join().unwrap()
}

#[test]
fn first_ssh_to_a_host_without_mouser_terminfo_gets_xterm() {
    let w = world(false);
    let out = in_pty(&w, &["-p", "2222", "web1"], G, &[]);
    // ssh itself still runs with TERM=mouser locally; the server falls back.
    // The session also becomes commander's ControlMaster (6 options added).
    assert!(out.contains("LOCAL TERM=mouser WRAPPED=1 ARGC=11"), "{out}");
    let cp = format!("ControlPath={}", w.t.path().join("home/cache/ssh/%C").display());
    assert!(out.contains(&format!("ARGV=-o ControlMaster=auto -o {cp} -o ControlPersist=60 -t -p 2222 web1 sh -c")), "{out}");
    assert!(out.contains("REMOTE TERM=xterm-256color ARGS=-l"), "{out}");
    // It tells Mouser where the remote shell is: OSC 7, then its pid.
    assert!(out.contains("\x1b]7;file://") && out.contains("\x1b]1337;MouserRemoteShell="), "{out:?}");
    // The shim first clears the last session's pid.
    assert!(out.starts_with("\x1b]1337;MouserRemoteShell=\x07"), "{out:?}");
    let out = in_pty(&w, &["-t", "web1", "tmux", "new", "-A"], &G.replace("auto", "yes"), &[]);
    assert!(out.contains("REMOTE TERM=xterm-256color ARGS=-c tmux new -A"), "{out}");
}

#[test]
fn a_host_with_mouser_terminfo_keeps_mouser() {
    let w = world(true);
    let out = in_pty(&w, &["web1"], G, &[]);
    assert!(out.contains("REMOTE TERM=mouser ARGS=-l"), "{out}");
}

#[test]
fn remote_command_in_config_and_overrides_fall_back_locally() {
    let w = world(true);
    // `RemoteCommand tmux new -A` + `RequestTTY yes` cannot take the check: TERM falls back here.
    let out = in_pty(&w, &["web1"], &format!("{}remotecommand tmux new -A\\n", G.replace("auto", "yes")), &[]);
    assert!(out.contains("LOCAL TERM=xterm-256color WRAPPED=1 ARGC=7"), "{out}");
    let out = in_pty(&w, &["web1"], G, &[("MOUSER_SSH_TERM", "xterm-256color")]);
    assert!(out.contains("LOCAL TERM=xterm-256color WRAPPED=1 ARGC=7"), "{out}");
    // Sharing off, or ssh_config's own ControlMaster: the arguments as typed.
    let out = in_pty(&w, &["web1"], G, &[("MOUSER_SSH_TERM", "xterm-256color"), ("MOUSER_SSH_SHARE", "no")]);
    assert!(out.contains("LOCAL TERM=xterm-256color WRAPPED=1 ARGC=1"), "{out}");
    let out = in_pty(&w, &["-p", "2222", "web1"], &format!("{G}controlmaster auto\\ncontrolpath /tmp/cm-%C\\n"), &[]);
    assert!(out.contains("LOCAL TERM=mouser WRAPPED=1 ARGC=5"), "{out}");
    let out = in_pty(&w, &["web1"], G, &[("MOUSER_SSH_TERM", "mouser")]);
    assert!(out.contains("LOCAL TERM=mouser WRAPPED=1 ARGC=1") && !out.contains("REMOTE"), "{out}");
    // An explicit TERM is sent as is.
    let out = in_pty(&w, &["web1"], G, &[("TERM", "xterm-kitty")]);
    assert!(out.contains("LOCAL TERM=xterm-kitty WRAPPED=1 ARGC=1"), "{out}");
    // Tunnels and control commands are untouched.
    let out = in_pty(&w, &["-N", "-L", "8080:localhost:80", "web1"], G, &[]);
    assert!(out.contains("LOCAL TERM=mouser WRAPPED=1 ARGC=4"), "{out}");
}

#[test]
fn without_a_terminal_ssh_is_passed_through() {
    let w = world(false);
    // scp / git / rsync style: piped stdio, a command.
    let out = std::process::Command::new(&w.shim)
        .args(["-x", "--", "web1", "scp", "-t", "/tmp"])
        .env("PATH", &w.path)
        .env("TERM", "mouser")
        .env("FAKE_G", G)
        .env("SERVER", &w.server_path)
        .env_remove("MOUSER_SSH_TERM")
        .env_remove("MOUSER_SSH_WRAPPED")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "LOCAL TERM=mouser WRAPPED=1 ARGC=6 ARGV=-x -- web1 scp -t /tmp",
        "passed through unchanged"
    );
    // `mouser ssh …` is the same as the shim.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mouser-app"))
        .args(["ssh", "web1", "true"])
        .env("PATH", &w.path)
        .env("TERM", "mouser")
        .env("FAKE_G", G)
        .env("SERVER", &w.server_path)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "LOCAL TERM=mouser WRAPPED=1 ARGC=2 ARGV=web1 true");
}

#[test]
fn shell_integration_defines_ssh_only_inside_mouser() {
    for (sh, script) in
        [("bash", mouser_core::shell_integration::BASH), ("zsh", mouser_core::shell_integration::ZSH), ("fish", mouser_core::shell_integration::FISH)]
    {
        assert!(script.contains("MOUSER_SSH_WRAPPER"), "{sh}");
    }
    let Ok(bash) = which("bash") else { return };
    let w = world(false);
    let t = tempfile::tempdir().unwrap();
    let rc = t.path().join("rc");
    std::fs::write(&rc, mouser_core::shell_integration::BASH).unwrap();
    let run = |wrapper: Option<&Path>| {
        let mut c = std::process::Command::new(&bash);
        c.args(["--norc", "--noprofile", "-i", "-c", &format!("source {}; type -t ssh", rc.display())]).env("TERM", "mouser").env("PATH", &w.path);
        match wrapper {
            Some(p) => c.env("MOUSER_SSH_WRAPPER", p),
            None => c.env_remove("MOUSER_SSH_WRAPPER"),
        };
        String::from_utf8_lossy(&c.stdin(std::process::Stdio::null()).output().unwrap().stdout).trim().to_string()
    };
    assert_eq!(run(Some(&w.shim)), "function");
    assert_eq!(run(None), "file");
}

fn which(name: &str) -> Result<PathBuf, ()> {
    ["/usr/bin", "/bin"].iter().map(|d| Path::new(d).join(name)).find(|p| p.exists()).ok_or(())
}
