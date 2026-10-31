//! Session server / UI process split (docs/engines.md §8): the UI process can
//! die (SIGKILL, or a crash inside an engine) while the server keeps the
//! shells; a new UI shows the same screen, scrollback, tabs and images, and a
//! crashed engine is skipped. Uses the headless test UI (`MOUSER_UI=headless`),
//! so no display is needed. Unix only (Windows stays single-process).
#![cfg(unix)]

use serial_test::serial;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

struct Server {
    child: Child,
    dump: PathBuf,
    _home: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const SHELL: &str = r#"#!/bin/sh
for i in $(seq 1 60); do echo "history line $i"; done
# A 2x2 RGBA image through the kitty graphics protocol.
printf '\033_Ga=T,f=32,s=2,v=2;/wAA/wD/AP8AAP///////w==\033\\'
echo
echo "shell pid $$"
i=0; while :; do i=$((i+1)); printf '\rtick %d' $i; sleep 0.1; done
"#;

fn start(home: tempfile::TempDir, env: &[(&str, &str)]) -> Server {
    let shell = home.path().join("shell.sh");
    std::fs::write(&shell, SHELL).unwrap();
    Command::new("chmod").arg("+x").arg(&shell).status().unwrap();
    std::fs::write(home.path().join("ssh_config"), "").unwrap();
    let dump = home.path().join("dump.txt");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mouser-app"));
    cmd.env("HOME", home.path())
        .env_remove("MOUSER_HOME")
        .env("MOUSER_SSH_CONFIG", home.path().join("ssh_config"))
        .env("SHELL", &shell)
        .env("MOUSER_UI", "headless")
        .env("MOUSER_TEST_DUMP", &dump)
        .env_remove("MOUSER_ENGINE")
        .env_remove("MOUSER_TEST_CRASH_ENGINE")
        .env_remove("MOUSER_RENDERER")
        .env_remove("MOUSER_SPLIT");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = cmd.spawn().unwrap();
    Server { child, dump, _home: home }
}

#[derive(Debug, Default, Clone)]
struct Dump {
    ui_pid: u32,
    engine: String,
    tabs: usize,
    images: usize,
    bytes: usize,
    shell_pid: Option<u32>,
    tick: u32,
    screen: String,
    scrollback: String,
}

fn read(path: &Path) -> Option<Dump> {
    let t = std::fs::read_to_string(path).ok()?;
    let (head, rest) = t.split_once("--- screen\n")?;
    let (screen, back) = rest.split_once("--- scrollback\n")?;
    let mut d = Dump { screen: screen.into(), scrollback: back.into(), ..Default::default() };
    for l in head.lines() {
        if let Some(v) = l.strip_prefix("ui_pid=") {
            d.ui_pid = v.parse().ok()?;
        } else if let Some(v) = l.strip_prefix("engine=") {
            d.engine = v.into();
        } else if let Some(v) = l.strip_prefix("tabs=") {
            d.tabs = v.split(' ').next()?.parse().ok()?;
        } else if let Some(v) = l.strip_prefix("images=") {
            let (n, b) = v.split_once(" bytes=")?;
            (d.images, d.bytes) = (n.parse().ok()?, b.parse().ok()?);
        }
    }
    d.shell_pid = screen.lines().find_map(|l| l.trim().strip_prefix("shell pid ").and_then(|p| p.trim().parse().ok()));
    d.tick = screen.lines().find_map(|l| l.trim().strip_prefix("tick ").and_then(|p| p.trim().parse().ok())).unwrap_or(0);
    Some(d)
}

fn wait_for(path: &Path, what: &str, ok: impl Fn(&Dump) -> bool) -> Dump {
    let end = Instant::now() + Duration::from_secs(30);
    let mut last = None;
    while Instant::now() < end {
        if let Some(d) = read(path) {
            if ok(&d) {
                return d;
            }
            last = Some(d);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}; last dump: {last:#?}");
}

fn alive(pid: u32) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).status().is_ok_and(|s| s.success())
}

#[test]
#[serial]
fn sessions_survive_a_killed_ui_and_a_new_ui_shows_them_again() {
    let s = start(tempfile::tempdir().unwrap(), &[("MOUSER_TEST_UI_ACTIONS", "NewTab,SelectTab:0")]);
    let a = wait_for(&s.dump, "the first UI to show the shell", |d| d.shell_pid.is_some() && d.tick > 2 && d.tabs == 2 && d.images == 1);
    assert!(a.bytes > 0, "image pixels reach the UI");
    let shell = a.shell_pid.unwrap();
    assert!(Command::new("kill").args(["-9", &a.ui_pid.to_string()]).status().unwrap().success());
    // While no UI is attached the shell keeps running and its output is kept.
    std::thread::sleep(Duration::from_millis(600));
    assert!(alive(shell), "shell {shell} survives the UI");
    let b = wait_for(&s.dump, "a new UI", |d| d.ui_pid != a.ui_pid && d.shell_pid.is_some());
    assert_eq!(b.shell_pid, Some(shell), "same shell (same PTY pid) after reattach");
    assert!(b.tick > a.tick, "output written while detached is there: tick {} -> {}", a.tick, b.tick);
    assert!(b.screen.contains("history line 60"), "screen restored: {}", b.screen);
    assert_eq!(b.tabs, 2, "tabs restored");
    assert_eq!((b.images, b.bytes), (1, a.bytes), "the image is shown again with all its pixels");
    assert!(b.scrollback.contains("history line"), "scrollback restored: {}", b.scrollback);
    assert_eq!(b.engine, a.engine, "a kill from outside is not the engine's fault");
}

#[cfg(feature = "skia")]
#[test]
#[serial]
fn a_crash_in_a_builtin_engine_skips_it_and_the_new_ui_uses_the_next() {
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().to_path_buf();
    let s = start(home, &[("MOUSER_ENGINE", "skia,mouser"), ("MOUSER_TEST_CRASH_ENGINE", "skia:15")]);
    let a = wait_for(&s.dump, "the skia UI", |d| d.engine == "skia" && d.shell_pid.is_some());
    let b = wait_for(&s.dump, "the UI after the crash", |d| d.ui_pid != a.ui_pid && d.shell_pid.is_some());
    assert_eq!(b.engine, "mouser", "next engine in priority");
    assert_eq!(b.shell_pid, a.shell_pid, "same shell after the engine crash");
    let marker = std::fs::read_to_string(home_path.join(".mouser/engines/.starting-skia")).unwrap();
    assert!(marker.contains("reason=the UI process crashed (signal 6 (SIGABRT)) while drawing with skia"), "{marker}");
}

#[test]
#[serial]
fn a_crash_in_mouser_falls_to_the_cpu_renderer() {
    let s = start(tempfile::tempdir().unwrap(), &[("MOUSER_ENGINE", "mouser"), ("MOUSER_TEST_CRASH_ENGINE", "mouser:10")]);
    let a = wait_for(&s.dump, "the mouser UI", |d| d.engine == "mouser" && d.shell_pid.is_some());
    let b = wait_for(&s.dump, "the CPU UI", |d| d.ui_pid != a.ui_pid && d.shell_pid.is_some());
    assert_eq!(b.engine, "cpu");
    assert_eq!(b.shell_pid, a.shell_pid);
}

#[test]
#[serial]
fn the_server_ends_with_its_last_session() {
    let home = tempfile::tempdir().unwrap();
    let shell = home.path().join("quick.sh");
    std::fs::write(&shell, "#!/bin/sh\necho bye\nsleep 0.5\n").unwrap();
    Command::new("chmod").arg("+x").arg(&shell).status().unwrap();
    let mut s = start(home, &[]);
    // Replace the ticking shell: start a server whose shell exits on its own.
    let _ = s.child.kill();
    let _ = s.child.wait();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mouser-app"));
    cmd.env("HOME", s._home.path())
        .env("MOUSER_SSH_CONFIG", s._home.path().join("ssh_config"))
        .env("SHELL", &shell)
        .env("MOUSER_UI", "headless")
        .env("MOUSER_TEST_DUMP", &s.dump);
    let mut c = cmd.spawn().unwrap();
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(st) = c.try_wait().unwrap() {
            assert!(st.success(), "{st}");
            break;
        }
        assert!(Instant::now() < end, "server still running after its shell exited");
        std::thread::sleep(Duration::from_millis(50));
    }
}
