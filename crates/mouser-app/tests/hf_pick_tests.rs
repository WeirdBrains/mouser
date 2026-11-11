//! `mouser model hf pick` in a real PTY against a local mock Hub
//! (`MOUSER_HF_URL`, never the network) with a temporary MOUSER_HOME:
//! keys in, screens out, the install it hands to, and the terminal put back.
#![cfg(unix)]

use mouser_pty::portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ONE: &[u8] = b"GGUF one-file model bytes";
const PART1: &[u8] = b"GGUF part one";
const PART2: &[u8] = b" and part two";
const SHA_ONE: &str = "a6ada85b3a69089898cf04f1df11dd56afafa8d0f5e8450d6eb2fe59e093dd1a";
const SHA_P1: &str = "167d85f77c1fd58b8ac3d7aaddea071da6fff81d02e2af036a2305626cc1959a";
const SHA_P2: &str = "d0c28e518f3f027c0de0ce4748cbd0141a2bac09f94a18320a0619d2a1aca3c2";

fn route(path: &str) -> (u16, Vec<u8>) {
    let repo = serde_json::json!({
        "id": "acme/Tiny-GGUF", "author": "acme", "downloads": 1234, "likes": 56, "pipeline_tag": "text-generation",
        "tags": ["gguf", "license:mit"], "cardData": {"license": "mit"}, "lastModified": "2026-10-01T00:00:00Z",
        "gguf": {"architecture": "llama", "context_length": 4096, "total": 1100000000u64},
        "siblings": [
            {"rfilename": "Tiny-Q4_K_M.gguf", "size": ONE.len(), "lfs": {"sha256": SHA_ONE, "size": ONE.len()}},
            {"rfilename": "mmproj-Tiny-F16.gguf", "size": 5000},
            {"rfilename": "Tiny-imatrix.gguf", "size": 4000},
            {"rfilename": "big/Tiny-F16-00002-of-00002.gguf", "lfs": {"sha256": SHA_P2, "size": PART2.len()}},
            {"rfilename": "big/Tiny-F16-00001-of-00002.gguf", "lfs": {"sha256": SHA_P1, "size": PART1.len()}}
        ]
    });
    match path {
        p if p.starts_with("/api/models?search=tiny&filter=gguf") => (
            200,
            serde_json::json!([{"id": "acme/Tiny-GGUF", "downloads": 1234, "likes": 56, "lastModified": "2026-10-01T00:00:00Z"},
                               {"id": "acme/Gone", "downloads": 3, "likes": 0, "lastModified": "2026-09-01T00:00:00Z"}])
            .to_string()
            .into_bytes(),
        ),
        "/api/models/acme/Tiny-GGUF?blobs=true" => (200, repo.to_string().into_bytes()),
        "/acme/Tiny-GGUF/resolve/main/Tiny-Q4_K_M.gguf" => (200, ONE.to_vec()),
        "/acme/Tiny-GGUF/resolve/main/big/Tiny-F16-00001-of-00002.gguf" => (200, PART1.to_vec()),
        "/acme/Tiny-GGUF/resolve/main/big/Tiny-F16-00002-of-00002.gguf" => (200, PART2.to_vec()),
        _ => (404, b"{\"error\":\"not found\"}".to_vec()),
    }
}

/// A mock Hub on localhost; logs request paths.
fn hub() -> (String, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { break };
            let log = log.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut first = String::new();
                if r.read_line(&mut first).is_err() {
                    return;
                }
                let path = first.split(' ').nth(1).unwrap_or("").to_string();
                let mut range = 0usize;
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = v.trim().trim_end_matches('-').parse().unwrap_or(0);
                    }
                }
                log.lock().unwrap().push(path.clone());
                let (status, body) = route(&path);
                let (status, body) = if status == 200 && range > 0 { (206, body[range..].to_vec()) } else { (status, body) };
                let mut w = s;
                let _ = write!(w, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = w.write_all(&body);
            });
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

struct Session {
    writer: Box<dyn Write + Send>,
    screen: Arc<Mutex<String>>,
    child: Box<dyn mouser_pty::Child + Send + Sync>,
    _master: Box<dyn mouser_pty::MasterPty + Send>,
}

impl Session {
    /// `mouser model hf pick`, then `stty -a` in the same terminal (to see it was put back).
    fn start(home: &std::path::Path, base: &str) -> Session {
        let pair = native_pty_system().openpty(PtySize { rows: 30, cols: 140, pixel_width: 0, pixel_height: 0 }).unwrap();
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", &format!("'{}' model hf pick; echo \"PICK-EXIT=$?\"; stty -a", env!("CARGO_BIN_EXE_mouser-app"))]);
        cmd.env("MOUSER_HOME", home.join("mouser"));
        cmd.env("HOME", home);
        cmd.env("MOUSER_HF_URL", base);
        cmd.env("TERM", "xterm-256color");
        cmd.env_remove("MOUSER_MODELS_DIR");
        let child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);
        let mut r = pair.master.try_clone_reader().unwrap();
        let screen = Arc::new(Mutex::new(String::new()));
        let s2 = screen.clone();
        std::thread::spawn(move || {
            let mut b = [0u8; 4096];
            while let Ok(n) = r.read(&mut b) {
                if n == 0 {
                    break;
                }
                s2.lock().unwrap().push_str(&String::from_utf8_lossy(&b[..n]));
            }
        });
        let writer = pair.master.take_writer().unwrap();
        Session { writer, screen, child, _master: pair.master }
    }

    fn wait_for(&self, what: &str) {
        let end = Instant::now() + Duration::from_secs(30);
        while Instant::now() < end {
            if self.screen.lock().unwrap().contains(what) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {what:?}; screen:\n{}", self.screen.lock().unwrap());
    }

    /// Wait for `what` to appear after everything seen so far.
    fn wait_for_new(&self, what: &str) {
        let from = self.screen.lock().unwrap().len();
        let end = Instant::now() + Duration::from_secs(30);
        while Instant::now() < end {
            if self.screen.lock().unwrap()[from..].contains(what) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for new {what:?}; screen:\n{}", self.screen.lock().unwrap());
    }

    fn send(&mut self, b: &[u8]) {
        self.writer.write_all(b).unwrap();
        self.writer.flush().unwrap();
    }

    fn finish(mut self) -> String {
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            if self.child.try_wait().unwrap().is_some() {
                break;
            }
            assert!(Instant::now() < end, "still running:\n{}", self.screen.lock().unwrap());
            std::thread::sleep(Duration::from_millis(20));
        }
        self.wait_for("speed");
        self.screen.lock().unwrap().clone()
    }
}

/// The terminal was put back: off the alternate screen, cursor shown, cooked mode.
fn restored(out: &str) {
    let leave = out.rfind("\x1b[?1049l").expect("left the alternate screen");
    assert!(out.rfind("\x1b[?1049h").unwrap() < leave);
    assert!(out[..leave + 8].contains("\x1b[?25h"), "cursor shown again");
    let stty = &out[out.find("PICK-EXIT=").expect("exit")..];
    let flags: Vec<&str> = stty.split_whitespace().collect();
    for flag in ["icanon", "echo", "isig", "icrnl", "opost"] {
        assert!(flags.contains(&flag), "stty after pick should have {flag} (not -{flag}): {stty}");
    }
}

#[test]
fn choose_a_split_quantization_and_it_installs_complete() {
    let (base, seen) = hub();
    let home = tempfile::tempdir().unwrap();
    let mut s = Session::start(home.path(), &base);
    s.wait_for("Search:");
    s.send(b"tiny");
    s.send(b"\r");
    // Results with the size column: Q4_K_M (25 bytes) to the split F16 (13 + 13);
    // the repo that 404s is unknown.
    s.wait_for("25–26 bytes");
    s.wait_for("unknown");
    s.send(b"\r");
    s.wait_for("(default)");
    s.wait_for("2 files");
    s.send(b"\x1b[B"); // down to F16
    s.send(b"\r");
    s.wait_for("Download it? [Y/n]");
    s.send(b"\r"); // Enter: yes
    s.wait_for("PICK-EXIT=0");
    let out = s.finish();
    restored(&out);
    assert!(out.contains("acme/Tiny-GGUF F16"), "{out}");
    let dir = home.path().join("mouser/models/hf/acme--Tiny-GGUF");
    assert_eq!(std::fs::read(dir.join("Tiny-F16-00001-of-00002.gguf")).unwrap(), PART1);
    assert_eq!(std::fs::read(dir.join("Tiny-F16-00002-of-00002.gguf")).unwrap(), PART2);
    assert!(!dir.join("Tiny-Q4_K_M.gguf").exists(), "only the chosen quantization");
    let seen = seen.lock().unwrap();
    assert!(!seen.iter().any(|p| p.contains("mmproj") || p.contains("imatrix")), "{seen:?}");
}

#[test]
fn esc_goes_back_and_ctrl_c_leaves_with_nothing_downloaded() {
    let (base, seen) = hub();
    let home = tempfile::tempdir().unwrap();
    let mut s = Session::start(home.path(), &base);
    s.wait_for("Search:");
    s.send(b"tiny\r");
    s.wait_for("25–26 bytes");
    s.send(b"\r");
    s.wait_for("(default)");
    s.send(b"\x1b"); // back to the results
    s.wait_for_new("Enter quantizations");
    s.send(b"\x03"); // Ctrl-C
    s.wait_for("PICK-EXIT=0");
    let out = s.finish();
    restored(&out);
    assert!(out.contains("nothing chosen; nothing downloaded"), "{out}");
    assert!(!out.contains("Download it?"));
    assert!(!home.path().join("mouser/models/hf").exists());
    assert!(!seen.lock().unwrap().iter().any(|p| p.contains("/resolve/")), "no weights fetched");
}

#[test]
fn declining_the_download_question_downloads_nothing() {
    let (base, seen) = hub();
    let home = tempfile::tempdir().unwrap();
    let mut s = Session::start(home.path(), &base);
    s.wait_for("Search:");
    s.send(b"tiny\r");
    s.wait_for("25–26 bytes");
    s.send(b"\r");
    s.wait_for("(default)");
    s.send(b"\r");
    s.wait_for("Download it? [Y/n]");
    s.send(b"n\r");
    s.wait_for("PICK-EXIT=0");
    let out = s.finish();
    assert!(out.contains("not downloaded"), "{out}");
    assert!(!seen.lock().unwrap().iter().any(|p| p.contains("/resolve/")));
}

#[test]
fn without_a_terminal_it_refuses_at_once() {
    let t = Instant::now();
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_mouser-app"))
        .args(["model", "hf", "pick", "tiny"])
        .env("MOUSER_HOME", tempfile::tempdir().unwrap().path())
        .env("MOUSER_HF_URL", "http://127.0.0.1:9")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("needs a terminal"), "{o:?}");
    assert!(t.elapsed() < Duration::from_secs(10));
}
