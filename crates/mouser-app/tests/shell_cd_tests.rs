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

fn check(shell: &str, rc_name: &str, prompt: &str, argv: &dyn Fn(&std::path::Path) -> Vec<String>) {
    if !std::path::Path::new(shell).exists() {
        eprintln!("{shell} not installed, skipped");
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
