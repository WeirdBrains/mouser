//! Remote folder listings for commander over an ssh side channel (one
//! multiplexed connection per host, `BatchMode` so it never prompts). Each
//! (host, folder) is fetched on a thread when first asked for and refreshed
//! in the background every few seconds while it is shown.

use mouser_core::commander::Entry;
use mouser_core::remote::{SshTarget, listing_command, parse_ls};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Key = (SshTarget, PathBuf);

#[derive(Default)]
struct Slot {
    got: Option<Result<Vec<Entry>, String>>,
    at: Option<Instant>,
    busy: bool,
}

#[derive(Default, Clone)]
pub struct RemoteLister {
    s: Arc<Mutex<HashMap<Key, Slot>>>,
}

/// Refresh what is shown this often.
const STALE: Duration = Duration::from_secs(5);

fn ssh_bin() -> std::ffi::OsString {
    mouser_core::fetch::ssh_bin()
}

fn control_dir() -> PathBuf {
    mouser_core::fetch::control_dir()
}

/// Run the listing; the error is ssh's last stderr line.
pub fn fetch(t: &SshTarget, dir: &Path) -> Result<Vec<Entry>, String> {
    let cd = control_dir();
    let mut cmd = std::process::Command::new(ssh_bin());
    // The config Mouser manages (tests: a temp one), as the launcher uses it.
    if let Some(cfg) = std::env::var_os("MOUSER_SSH_CONFIG")
        && !t.opts.iter().any(|o| o == "-F")
    {
        cmd.arg("-F").arg(cfg);
    }
    let out = cmd.args(t.argv(&cd, &listing_command(dir))).stdin(std::process::Stdio::null()).output().map_err(|e| format!("ssh: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim().to_string();
        return Err(format!("remote listing over ssh failed: {last} (the side channel needs key/agent auth or a ControlMaster)"));
    }
    Ok(parse_ls(&String::from_utf8_lossy(&out.stdout), dir))
}

impl RemoteLister {
    /// What is known for `dir` on `t` now (`None`: first fetch running);
    /// starts or refreshes a fetch as needed and calls `wake` when it lands.
    pub fn get(&self, t: &SshTarget, dir: &Path, wake: fn()) -> Option<Result<Vec<Entry>, String>> {
        let key = (t.clone(), dir.to_path_buf());
        let mut m = self.s.lock().unwrap();
        let slot = m.entry(key.clone()).or_default();
        let stale = slot.at.is_none_or(|a| a.elapsed() > STALE);
        if stale && !slot.busy {
            slot.busy = true;
            let s = self.s.clone();
            std::thread::spawn(move || {
                let r = fetch(&key.0, &key.1);
                let mut m = s.lock().unwrap();
                let slot = m.entry(key).or_default();
                slot.got = Some(r);
                slot.at = Some(Instant::now());
                slot.busy = false;
                drop(m);
                wake();
            });
        }
        slot.got.clone()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn lists_through_a_fake_ssh() {
        let t = tempfile::tempdir().unwrap();
        let remote = t.path().join("srv");
        std::fs::create_dir_all(remote.join("app")).unwrap();
        std::fs::write(remote.join("notes.txt"), "hi").unwrap();
        // A stand-in ssh: records its args, runs the remote command locally.
        let fake = t.path().join("ssh");
        std::fs::write(&fake, format!("#!/bin/sh\necho \"$@\" > {}/args\nfor a; do last=\"$a\"; done\nexec sh -c \"$last\"\n", t.path().display()))
            .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: this test is the only one in this binary using these variables.
        unsafe {
            std::env::set_var("MOUSER_SSH_BIN", &fake);
            std::env::set_var("MOUSER_HOME", t.path().join("home"));
            std::env::set_var("MOUSER_SSH_CONFIG", t.path().join("ssh_config"));
        }
        let target = mouser_core::remote::ssh_target(&["ssh".into(), "-p".into(), "2222".into(), "me@web1".into()]).unwrap();
        let l = RemoteLister::default();
        fn nop() {}
        assert!(l.get(&target, &remote, nop).is_none(), "first ask: loading");
        let mut got = None;
        for _ in 0..200 {
            std::thread::sleep(Duration::from_millis(10));
            if let Some(r) = l.get(&target, &remote, nop) {
                got = Some(r);
                break;
            }
        }
        let v = got.expect("listing arrives").expect("ok");
        let mut names: Vec<_> = v.iter().map(|e| e.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["app", "notes.txt"]);
        let args = std::fs::read_to_string(t.path().join("args")).unwrap();
        assert!(args.contains("BatchMode=yes") && args.contains("-p 2222") && args.contains("me@web1"), "{args}");
        assert!(args.contains(&format!("-F {}", t.path().join("ssh_config").display())), "the temp config, never ~/.ssh: {args}");
        assert!(args.contains(&format!("ControlPath={}", t.path().join("home/cache/ssh/%C").display())));
        // A failing side channel says why.
        let e = fetch(&target, &t.path().join("missing")).unwrap_err();
        assert!(e.starts_with("remote listing over ssh failed"), "{e}");
    }
}
