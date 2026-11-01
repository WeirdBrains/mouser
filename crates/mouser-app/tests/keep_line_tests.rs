//! cd / pipe from commander keep the half-typed line (bash and zsh, emacs
//! keymap; fish too if installed). Runs real shells in a PTY.
#![cfg(unix)]

use std::io::{Read, Write};
use std::time::{Duration, Instant};

fn run(argv: &[&str]) -> Option<String> {
    let prog = argv[0];
    if !std::path::Path::new(prog).exists() {
        eprintln!("{prog} not installed, skipped");
        return None;
    }
    let home = tempfile::tempdir().unwrap();
    // SAFETY: tests in this file do not read the environment concurrently.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("EDITOR", "nano");
        std::env::set_var("VISUAL", "nano");
        std::env::set_var("PS1", "$ ");
        std::env::set_var("PROMPT", "$ ");
    }
    let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let mut h = mouser_pty::spawn_argv_in(&argv, Some(home.path()), 200, 40, 0, 0).unwrap();
    let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
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
    let w = &mut h.writer;
    let pause = |ms| std::thread::sleep(Duration::from_millis(ms));
    pause(800);
    // Half-typed, cursor moved left into the middle.
    w.write_all(b"echo HALF_$((20+22))\x02\x02").unwrap();
    pause(200);
    w.write_all(&mouser_core::modes::keep_line("cd -- '/'")).unwrap();
    pause(500);
    w.write_all(mouser_core::modes::RESTORE_LINE).unwrap();
    pause(300);
    w.write_all(b"\r").unwrap();
    pause(300);
    // An empty line stays empty (no stale kill is yanked).
    w.write_all(&mouser_core::modes::keep_line("printf '%s\\n' x | tr x Y")).unwrap();
    pause(500);
    w.write_all(mouser_core::modes::RESTORE_LINE).unwrap();
    pause(200);
    w.write_all(b"echo PWD_$PWD\r").unwrap();
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(5) && !String::from_utf8_lossy(&out.lock().unwrap()).contains("PWD_/") {
        pause(50);
    }
    let _ = h.child.kill();
    Some(String::from_utf8_lossy(&out.lock().unwrap()).into_owned())
}

fn check(argv: &[&str]) {
    let Some(o) = run(argv) else { return };
    assert!(o.contains("HALF_42"), "{argv:?}: the typed line ran after the cd\n{o}");
    assert!(o.contains("PWD_/\r") || o.contains("PWD_/\n"), "{argv:?}: cd happened; empty line stayed empty\n{o}");
    assert!(o.contains("\nY") || o.contains("Y\r"), "{argv:?}: pipe ran\n{o}");
}

#[test]
fn bash_zsh_fish_keep_the_line() {
    check(&["/bin/bash", "--norc", "--noprofile", "-i"]);
    check(&["/bin/zsh", "-f", "-i"]);
    for f in ["/usr/bin/fish", "/opt/homebrew/bin/fish"] {
        check(&[f, "--no-config", "-i"]);
    }
}
