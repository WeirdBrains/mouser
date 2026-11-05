//! Fuzzball `s` (docs/protocol/shell.md): the shell integration changes
//! directory silently, keeps the half-typed line and replies. Real zsh and bash
//! in a PTY with a temporary HOME; skipped when a shell is missing.
#![cfg(unix)]

use mouser_core::parser::TerminalParser;
use mouser_core::scrollback::{Scrollback, ScrollbackConfig};
use mouser_core::types::Size;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static ENV: Mutex<()> = Mutex::new(());

struct Run {
    out: Arc<Mutex<Vec<u8>>>,
}

impl Run {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.out.lock().unwrap()).into_owned()
    }
    fn wait(&self, s: &str) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(5) {
            if self.text().contains(s) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        false
    }
}

fn screen(bytes: &[u8]) -> Vec<String> {
    let mut p = TerminalParser::new(Size { cols: 100, rows: 30 }, Arc::new(Scrollback::new(ScrollbackConfig::default())), std::io::sink());
    p.feed(bytes);
    p.screen.visible_lines()
}

fn name_is_old_bash(shell: &str) -> bool {
    if !shell.ends_with("bash") {
        return false;
    }
    let v = std::process::Command::new(shell).args(["-c", "echo ${BASH_VERSINFO[0]} ${BASH_VERSINFO[1]}"]).output().unwrap();
    let v: Vec<u32> = String::from_utf8_lossy(&v.stdout).split_whitespace().filter_map(|x| x.parse().ok()).collect();
    v.len() < 2 || (v[0], v[1]) < (4, 4)
}

fn check(shell: &str, rc_name: &str, prompt: &str, argv: &dyn Fn(&std::path::Path) -> Vec<String>) {
    if !std::path::Path::new(shell).exists() {
        eprintln!("{shell} not installed, skipped");
        return;
    }
    if name_is_old_bash(shell) {
        eprintln!("{shell} is older than 4.4 (the integration needs PS0), skipped");
        return;
    }
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let target = home.path().join("a dir, with=odd;chars");
    std::fs::create_dir(&target).unwrap();
    let name = std::path::Path::new(shell).file_name().unwrap().to_str().unwrap().to_string();
    let rc = home.path().join(rc_name);
    std::fs::write(&rc, format!("{prompt}\n{}", mouser_core::shell_integration::script(&name).unwrap())).unwrap();
    // SAFETY: serialised by ENV; nothing else in this file reads the environment.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("ZDOTDIR", home.path());
    }
    let mut h = mouser_pty::spawn_argv_in(&argv(&rc), Some(home.path()), 100, 30, 0, 0).unwrap();
    let out = Arc::new(Mutex::new(Vec::new()));
    let o2 = out.clone();
    let mut reader = h.reader;
    std::thread::spawn(move || {
        let mut b = [0u8; 4096];
        while let Ok(n) = reader.read(&mut b) {
            if n == 0 {
                break;
            }
            o2.lock().unwrap().extend_from_slice(&b[..n]);
        }
    });
    let r = Run { out };
    let w = &mut h.writer;
    assert!(r.wait("\x1bP=Fzsa;v=0,shell="), "{shell}: advertises\n{:?}", r.text());
    std::thread::sleep(Duration::from_millis(200));
    w.write_all(b"echo HALF_$((20+22))\x02\x02").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let before = r.text().len();
    w.write_all(&mouser_core::fuzzball::shell_cd(7, target.to_str().unwrap().as_bytes())).unwrap();
    assert!(r.wait("\x1bP=Fzsr;id=7,ok=1\x1b\\"), "{shell}: ok reply\n{:?}", r.text());
    std::thread::sleep(Duration::from_millis(200));
    let during = r.text()[before..].to_string();
    assert!(!during.contains("cd "), "{shell}: nothing typed visibly: {during:?}");
    let lines = screen(&r.out.lock().unwrap());
    let prompts: Vec<&String> = lines.iter().filter(|l| l.contains("]$ ")).collect();
    assert_eq!(prompts.len(), 1, "{shell}: one prompt on screen: {lines:#?}");
    assert!(prompts[0].contains("a dir") && prompts[0].contains("echo HALF"), "{shell}: redrawn in place, line kept: {prompts:?}");
    w.write_all(b"\r").unwrap();
    assert!(r.wait("HALF_42"), "{shell}: the half-typed line survived\n{:?}", r.text());
    w.write_all(b"echo PWD_$PWD\r").unwrap();
    let want = format!("PWD_{}", target.display());
    assert!(r.wait(&want), "{shell}: cd happened\n{:?}", r.text());
    // A missing directory is an error reply, and the shell stays put.
    w.write_all(&mouser_core::fuzzball::shell_cd(8, b"/no/such/dir")).unwrap();
    assert!(r.wait("\x1bP=Fzsr;id=8,ok=0,err=ENOENT"), "{shell}: error reply\n{:?}", r.text());
    // The terminal side: the advertisement and replies are recorded.
    let mut p = TerminalParser::new(Size { cols: 100, rows: 30 }, Arc::new(Scrollback::new(ScrollbackConfig::default())), std::io::sink());
    p.feed(&r.out.lock().unwrap());
    assert!(p.fuzzball.shell.as_ref().is_some_and(|s| s.has("cwd") && s.shell == name), "{shell}: {:?}", p.fuzzball.shell);
    assert!(p.fuzzball.shell_replies.contains(&(7, true, String::new())));
    assert!(p.fuzzball.shell_replies.iter().any(|(id, ok, m)| *id == 8 && !ok && m.contains("/no/such/dir")));
    assert!(p.cwd.as_deref().is_some_and(|c| c.contains("a%20dir")), "OSC 7: {:?}", p.cwd);
    let _ = h.child.kill();
}

#[test]
fn zsh_changes_directory_silently() {
    check("/bin/zsh", ".zshrc", "PROMPT='[%~]$ '", &|_| vec!["/bin/zsh".into(), "-i".into()]);
}

#[test]
fn bash_changes_directory_silently() {
    check("/bin/bash", "bashrc", "PS1='[\\w]$ '", &|rc| {
        vec!["/bin/bash".into(), "--noprofile".into(), "--rcfile".into(), rc.display().to_string(), "-i".into()]
    });
}

/// Neither the silent cd nor anything around it lands in the history.
#[test]
fn bash_sync_adds_nothing_to_the_history() {
    let shell = "/bin/bash";
    if !std::path::Path::new(shell).exists() || name_is_old_bash(shell) {
        eprintln!("bash 4.4+ not installed, skipped");
        return;
    }
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let target = home.path().join("t");
    std::fs::create_dir(&target).unwrap();
    let rc = home.path().join("bashrc");
    let hist = home.path().join("hist");
    std::fs::write(&rc, format!("PS1='[\\w]$ '\nHISTFILE={}\n{}", hist.display(), mouser_core::shell_integration::BASH)).unwrap();
    unsafe { std::env::set_var("HOME", home.path()) };
    let argv: Vec<String> = vec![shell.into(), "--noprofile".into(), "--rcfile".into(), rc.display().to_string(), "-i".into()];
    let mut h = mouser_pty::spawn_argv_in(&argv, Some(home.path()), 100, 30, 0, 0).unwrap();
    let out = Arc::new(Mutex::new(Vec::new()));
    let o2 = out.clone();
    let mut reader = h.reader;
    std::thread::spawn(move || {
        let mut b = [0u8; 4096];
        while let Ok(n) = reader.read(&mut b) {
            if n == 0 {
                break;
            }
            o2.lock().unwrap().extend_from_slice(&b[..n]);
        }
    });
    let r = Run { out };
    let w = &mut h.writer;
    assert!(r.wait("\x1bP=Fzsa;v=0,shell="), "advertises\n{:?}", r.text());
    w.write_all(b"echo FIRST\r").unwrap();
    assert!(r.wait("FIRST\r\n"));
    std::thread::sleep(Duration::from_millis(200));
    w.write_all(&mouser_core::fuzzball::shell_cd(3, target.to_str().unwrap().as_bytes())).unwrap();
    assert!(r.wait("\x1bP=Fzsr;id=3,ok=1\x1b\\"));
    std::thread::sleep(Duration::from_millis(200));
    w.write_all(b"history | sed 's/^ *[0-9]* *//' | tr '\\n' '|'; echo END_OF_HISTORY\r").unwrap();
    assert!(r.wait("END_OF_HISTORY\r\n"), "{:?}", r.text());
    let t = r.text();
    let line = t.lines().map(|l| l.trim_start_matches(|c: char| c != 'e')).find(|l| l.starts_with("echo FIRST|")).unwrap_or("").to_string();
    assert!(line.starts_with("echo FIRST|history"), "only what was typed is in the history: {line:?}\n{t:?}");
    assert!(!line.contains("cd "), "{line:?}");
    let _ = h.child.kill();
}

/// PowerShell: the prompt advertises its sync file; a folder left there is
/// taken at the next prompt with a builtin Set-Location (nothing typed).
#[test]
fn pwsh_takes_the_sync_file_at_the_next_prompt() {
    let Some(pwsh) = ["/usr/bin/pwsh", "/usr/local/bin/pwsh", "/opt/homebrew/bin/pwsh", "/opt/microsoft/powershell/7/pwsh"]
        .into_iter()
        .map(std::path::PathBuf::from)
        .chain(std::env::var_os("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).map(|d| d.join("pwsh")).collect::<Vec<_>>()))
        .find(|p| p.exists())
    else {
        eprintln!("pwsh not installed, skipped");
        return;
    };
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let target = home.path().join("a dir");
    std::fs::create_dir(&target).unwrap();
    let rc = home.path().join("mouser.ps1");
    std::fs::write(&rc, mouser_core::shell_integration::PWSH).unwrap();
    unsafe { std::env::set_var("HOME", home.path()) };
    let argv: Vec<String> =
        vec![pwsh.display().to_string(), "-NoProfile".into(), "-NoLogo".into(), "-NoExit".into(), "-Command".into(), format!(". '{}'", rc.display())];
    let h = mouser_pty::spawn_argv_in(&argv, Some(home.path()), 100, 30, 0, 0).unwrap();
    let mut child = h.child;
    let writer = Arc::new(Mutex::new(h.writer));
    let out = Arc::new(Mutex::new(Vec::new()));
    let o2 = out.clone();
    let w2 = writer.clone();
    let mut reader = h.reader;
    std::thread::spawn(move || {
        let mut b = [0u8; 4096];
        while let Ok(n) = reader.read(&mut b) {
            if n == 0 {
                break;
            }
            // .NET asks where the cursor is (DSR); answer like a terminal.
            for _ in b[..n].windows(4).filter(|w| w == b"\x1b[6n") {
                let _ = w2.lock().unwrap().write_all(b"\x1b[1;1R");
            }
            o2.lock().unwrap().extend_from_slice(&b[..n]);
        }
    });
    let r = Run { out };
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(20) && !r.text().contains("caps=cwdfile") {
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut p = TerminalParser::new(Size { cols: 100, rows: 30 }, Arc::new(Scrollback::new(ScrollbackConfig::default())), std::io::sink());
    p.feed(&r.out.lock().unwrap());
    let caps = p.fuzzball.shell.clone().unwrap_or_else(|| panic!("advertises cwdfile\n{:?}", r.text()));
    assert!(caps.has("cwdfile") && caps.shell == "pwsh", "{caps:?}");
    let file = caps.file.clone().expect("the sync file");
    assert!(file.file_name().unwrap().to_str().unwrap().starts_with("mouser-sync-"), "{file:?}");
    std::fs::write(&file, target.to_str().unwrap()).unwrap();
    let w = |b: &[u8]| writer.lock().unwrap().write_all(b).unwrap();
    w(b"\r");
    std::thread::sleep(Duration::from_millis(500));
    w(b"'PWD_' + (Get-Location).Path\r");
    // macOS temp dirs are under /var, a symlink to /private/var: either form.
    let wants = [format!("PWD_{}", target.display()), format!("PWD_{}", target.canonicalize().unwrap().display())];
    let moved = || wants.iter().any(|w| r.text().contains(w.as_str()));
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(10) && !moved() {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(moved(), "moved at the next prompt\n{:?}", r.text());
    assert!(!file.exists(), "the sync file is used up");
    w(b"(Get-History | % CommandLine) -join '|'\r");
    std::thread::sleep(Duration::from_millis(800));
    assert!(!r.text().contains("Set-Location"), "nothing about the sync in the history");
    let _ = child.kill();
}
