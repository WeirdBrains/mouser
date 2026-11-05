//! The commander plugin on a remote shell: with `remote.read` Mouser tells
//! it the ssh session and the listing, and it previews and views the remote
//! files over that ssh (a fake ssh here, which runs the commands locally).

// The fake ssh is a shell script.
#![cfg(unix)]

mod common;

use common::*;
use mouser_core::commander::BINDINGS;
use mouser_core::modes::Keymap;
use mouser_core::permissions::{Decision, Store};
use mouser_render::modes::Mode;
use mouser_render::modes::stdio::StdioMode;
use std::time::{Duration, Instant};

#[test]
fn remote_folders_preview_and_view_through_mouser() {
    use mouser_core::commander::{Entry, kind_of};
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let t = tempfile::tempdir().unwrap();
    let ssh = t.path().join("fake-ssh");
    std::fs::write(
        &ssh,
        "#!/bin/sh\nwhile [ $# -gt 0 ]; do case \"$1\" in -o|-F|-p|-l|-i|-J) shift 2;; -*) shift;; *) break;; esac; done\nshift\n[ \"$1\" = -- ] && shift\nPATH=/nonexistent:/usr/bin:/bin exec sh -c \"$*\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: the only test in this binary; set before anything reads them.
    unsafe {
        std::env::set_var("MOUSER_HOME", home.path());
        std::env::set_var("MOUSER_PLUGIN_DIRS", "");
        std::env::set_var("MOUSER_SSH_BIN", &ssh);
        std::env::set_var("MOUSER_REMOTE_BACKEND", "cat");
    }
    let dir = home.path().join("plugins/commander");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = env!("CARGO_BIN_EXE_mouser-commander");
    let manifest = include_str!("../plugin.toml").replace("command = [\"./mouser-commander\"]", &format!("command = [\"{exe}\"]"));
    std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
    let mut store = Store::load();
    for c in ["run", "files.read", "shell.type", "remote.read"] {
        store.set("commander", c, Decision::Allow);
    }
    store.save().unwrap();

    // The "remote" files live here; the listing says they are on fakehost.
    let srv = t.path().join("srv");
    std::fs::create_dir(&srv).unwrap();
    image::RgbaImage::from_pixel(300, 100, image::Rgba([0, 200, 0, 255])).save(srv.join("r.png")).unwrap();
    std::fs::write(srv.join("notes.txt"), "line one\nline two\n").unwrap();
    let ent = |n: &str| {
        let p = srv.join(n);
        let m = std::fs::metadata(&p).unwrap();
        Entry { name: n.into(), path: p, kind: kind_of(n), mode: "-rw-r--r--".into(), size: m.len(), modified: m.modified().ok() }
    };
    let target = mouser_core::remote::SshTarget { opts: vec![], dest: "fakehost".into() };

    let p = mouser_core::plugins::find("commander").unwrap();
    let mut m = StdioMode::new(p, 3).unwrap();
    let keys = Keymap::new(BINDINGS);
    m.activate();
    // Mouser calls these every frame; the plugin hears only changes.
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        m.remote_ssh(Some(&target));
        m.remote_changed("fakehost", Some(&srv), None);
        let _ = m.view(&ctx(), &keys);
        m.remote_changed("fakehost", Some(&srv), Some(Ok(vec![ent("notes.txt"), ent("r.png")])));
        let v = m.view(&ctx(), &keys);
        if shows(&v, "r.png") || Instant::now() > end {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let v = settle(&mut m, "remote list");
    assert!(shows(&v, "fakehost"), "the header names the host");
    assert!(shows(&v, "notes.txt") && shows(&v, "r.png"));

    // Grid: the image tile and the text snippet come over "ssh".
    m.action("view", 1);
    settle_when(&mut m, "remote grid with its tile and snippet", |v| {
        !v.images.is_empty() && v.dock.iter().flat_map(|d| &d.items).any(|i| i.text == "line one")
    });

    // The viewer (picked from the list): the image fetched whole, then the text.
    m.action("view", 0);
    select(&mut m, "r.png");
    m.action("open", 0);
    settle_when(&mut m, "remote image view, fetched whole", |v| v.images.first().map(|i| (i.width, i.height)) == Some((300, 100)));
    assert!(m.escape());
    select(&mut m, "notes.txt");
    m.action("open", 0);
    settle_when(&mut m, "remote text view", |v| v.dock.iter().flat_map(|d| &d.items).any(|i| i.text == "line two"));
    drop(m);
}
