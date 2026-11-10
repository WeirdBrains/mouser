//! Run one command line on this computer and collect what it prints: the
//! chat's `terminal_run` tool (docs/chat.md). The command runs in your shell
//! (`$SHELL -c`, `sh` without one; `cmd /C` on Windows) in the given folder,
//! with no terminal and stdin closed, so prompts fail fast instead of
//! hanging. It stops at the timeout or when `cancel` is set (the whole
//! process group on Unix). Only ever called after the user allowed it.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Default and longest timeout (seconds).
pub const TIMEOUT: u64 = 120;
pub const MAX_TIMEOUT: u64 = 900;
/// Output kept (bytes, stdout and stderr together; the rest is counted).
pub const MAX_OUTPUT: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// The exit code (`None`: killed by a signal, the timeout or a cancel).
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    /// Bytes past [`MAX_OUTPUT`] that were dropped.
    pub dropped: usize,
}

impl Output {
    /// What the model gets: the output and how it ended.
    pub fn report(&self) -> String {
        let mut s = String::new();
        if !self.stdout.is_empty() {
            s.push_str(&self.stdout);
            if !s.ends_with('\n') {
                s.push('\n');
            }
        }
        if !self.stderr.is_empty() {
            s.push_str("[stderr]\n");
            s.push_str(&self.stderr);
            if !s.ends_with('\n') {
                s.push('\n');
            }
        }
        if self.dropped > 0 {
            s.push_str(&format!("[{} more bytes of output not shown]\n", self.dropped));
        }
        s.push_str(&match (self.code, self.timed_out, self.cancelled) {
            (_, true, _) => "[stopped: timed out]".to_string(),
            (_, _, true) => "[stopped: cancelled]".to_string(),
            (Some(c), ..) => format!("[exit code {c}]"),
            (None, ..) => "[killed by a signal]".to_string(),
        });
        s
    }
}

fn shell(command: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        let mut c = crate::process::command("cmd");
        c.args(["/C", command]);
        c
    }
    #[cfg(not(windows))]
    {
        let sh = std::env::var("SHELL").ok().filter(|s| !s.is_empty() && Path::new(s).is_absolute()).unwrap_or_else(|| "/bin/sh".into());
        let mut c = crate::process::command(sh);
        c.args(["-c", command]);
        use std::os::unix::process::CommandExt;
        // Its own process group, so a stop reaches what it started too.
        c.process_group(0);
        c
    }
}

/// SIGKILL the process group `pid` leads (set at spawn).
fn kill_group(pid: u32) {
    #[cfg(unix)]
    unsafe {
        // SAFETY: signals only the child's own process group.
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

fn kill(child: &mut Child) {
    kill_group(child.id());
    let _ = child.kill();
}

fn reader(mut r: impl Read + Send + 'static) -> std::thread::JoinHandle<(Vec<u8>, usize)> {
    std::thread::spawn(move || {
        let (mut kept, mut dropped) = (Vec::new(), 0usize);
        let mut buf = [0u8; 8192];
        while let Ok(n) = r.read(&mut buf) {
            if n == 0 {
                break;
            }
            let room = MAX_OUTPUT.saturating_sub(kept.len()).min(n);
            kept.extend_from_slice(&buf[..room]);
            dropped += n - room;
        }
        (kept, dropped)
    })
}

/// Run `command` in `dir` for at most `timeout` (clamped to [`MAX_TIMEOUT`]).
pub fn run(command: &str, dir: &Path, timeout: Duration, cancel: &AtomicBool) -> Result<Output, String> {
    if command.trim().is_empty() {
        return Err("an empty command".into());
    }
    let timeout = timeout.min(Duration::from_secs(MAX_TIMEOUT));
    let mut child = shell(command)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("TERM", "dumb")
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat")
        .spawn()
        .map_err(|e| format!("could not start the shell in {}: {e}", dir.display()))?;
    let out = reader(child.stdout.take().expect("piped"));
    let err = reader(child.stderr.take().expect("piped"));
    let start = Instant::now();
    let (mut timed_out, mut cancelled) = (false, false);
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            // What it left running in the background (holding the output
            // pipes open) ends with it: the tool is not a job runner.
            kill_group(child.id());
            break Some(s);
        }
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
        } else if start.elapsed() >= timeout {
            timed_out = true;
        }
        if cancelled || timed_out {
            kill(&mut child);
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let (o, od) = out.join().unwrap_or_default();
    let (e, ed) = err.join().unwrap_or_default();
    Ok(Output {
        code: status.and_then(|s| s.code()),
        stdout: String::from_utf8_lossy(&o).into_owned(),
        stderr: String::from_utf8_lossy(&e).into_owned(),
        timed_out,
        cancelled,
        dropped: od + ed,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn go(cmd: &str, secs: u64) -> Output {
        run(cmd, &std::env::temp_dir(), Duration::from_secs(secs), &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn collects_output_and_the_exit_code() {
        let o = go("echo hi; echo oops >&2; exit 3", 10);
        assert_eq!((o.stdout.as_str(), o.stderr.as_str(), o.code), ("hi\n", "oops\n", Some(3)));
        assert_eq!(o.report(), "hi\n[stderr]\noops\n[exit code 3]");
        assert!(go("pwd", 10).stdout.trim_end().ends_with(std::env::temp_dir().file_name().unwrap().to_str().unwrap()));
    }

    #[test]
    fn stdin_is_closed_and_output_is_capped() {
        assert_eq!(go("cat; echo done", 10).stdout, "done\n", "no terminal input to wait for");
        let o = go("head -c 100000 /dev/zero", 10);
        assert_eq!((o.stdout.len(), o.dropped), (MAX_OUTPUT, 100000 - MAX_OUTPUT));
    }

    #[test]
    fn timeout_and_cancel_stop_the_whole_group() {
        let t = Instant::now();
        let o = run("sleep 30 & sleep 30", &std::env::temp_dir(), Duration::from_millis(300), &AtomicBool::new(false)).unwrap();
        assert!(o.timed_out && t.elapsed() < Duration::from_secs(5) && o.report().ends_with("[stopped: timed out]"));
        let c = AtomicBool::new(true);
        let o = run("sleep 30", &std::env::temp_dir(), Duration::from_secs(60), &c).unwrap();
        assert!(o.cancelled);
        assert!(run(" ", &std::env::temp_dir(), Duration::from_secs(1), &c).is_err());
    }

    fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn wait_dead(pid: i32) -> bool {
        (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(20));
            !alive(pid)
        })
    }

    #[test]
    fn descendants_end_with_a_cancel_or_with_the_command() {
        let d = tempfile::tempdir().unwrap();
        let pidf = d.path().join("pid");
        // A background job of a finished command neither hangs the tool nor lives on.
        let t = Instant::now();
        let o = run(
            &format!("sh -c 'sleep 60' & echo $! > {}; echo started", pidf.display()),
            d.path(),
            Duration::from_secs(30),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(t.elapsed() < Duration::from_secs(5) && o.stdout == "started\n" && o.code == Some(0), "{o:?}");
        let pid: i32 = std::fs::read_to_string(&pidf).unwrap().trim().parse().unwrap();
        assert!(wait_dead(pid), "background job {pid} still running");
        // A cancel mid-run reaches a grandchild.
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        let f2 = pidf.clone();
        std::thread::spawn(move || {
            for _ in 0..200 {
                if std::fs::read_to_string(&f2).is_ok_and(|s| s.trim() != pid.to_string() && !s.trim().is_empty()) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            c2.store(true, Ordering::SeqCst);
        });
        let o = run(&format!("sh -c 'sleep 60 & echo $! > {}; wait'", pidf.display()), d.path(), Duration::from_secs(30), &cancel).unwrap();
        assert!(o.cancelled);
        let gpid: i32 = std::fs::read_to_string(&pidf).unwrap().trim().parse().unwrap();
        assert!(gpid != pid && wait_dead(gpid), "grandchild {gpid} still running");
    }
}
