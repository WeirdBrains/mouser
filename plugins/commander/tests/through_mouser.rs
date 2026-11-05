//! The commander plugin end to end: Mouser's adapter (`modes::stdio`)
//! starts the real `mouser-commander`, and for a folder and window it must
//! draw the list, grid, carousel, search, the file view and images, take its
//! keys, and ask Mouser for the right effects (Mouser has no built-in
//! commander since 1.4.0; the plugin is the mode).

mod common;

use common::*;
use mouser_core::commander::BINDINGS;
use mouser_core::permissions::{Decision, Store};
use mouser_render::modes::stdio::StdioMode;
use mouser_render::modes::{Effect, Mode};

#[test]
fn draws_and_acts_through_mouser() {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: the only test in this binary; set before anything reads it.
    unsafe { std::env::set_var("MOUSER_HOME", home.path()) };
    unsafe { std::env::set_var("MOUSER_PLUGIN_DIRS", "") };
    let dir = home.path().join("plugins/commander");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = env!("CARGO_BIN_EXE_mouser-commander").replace('\\', "\\\\");
    let manifest = include_str!("../plugin.toml").replace("command = [\"./mouser-commander\"]", &format!("command = [\"{exe}\"]"));
    std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
    let mut store = Store::load();
    for c in ["run", "files.read", "shell.type"] {
        store.set("commander", c, Decision::Allow);
    }
    store.save().unwrap();

    // The manifest's keys are Mouser's commander bindings (`modes.toml`).
    let reg = mouser_core::plugins::Registry::load();
    assert!(reg.broken.is_empty(), "{:?}", reg.broken);
    let e = reg.entries.iter().find(|e| e.name == "commander").unwrap();
    assert_eq!(e.transport, mouser_core::plugins::Transport::Mode, "the plugin is the commander mode");
    let man = mouser_core::plugins::Manifest::parse(include_str!("../plugin.toml")).unwrap();
    let decl: Vec<(String, String)> = man.mode.as_ref().unwrap().keys.iter().map(|k| (k.id.clone(), k.key.clone())).collect();
    let builtin: Vec<(String, String)> = BINDINGS.iter().map(|b| (b.id.to_string(), b.key.to_string())).collect();
    assert_eq!(decl, builtin);

    let work = tempfile::tempdir().unwrap();
    let w = work.path();
    std::fs::create_dir(w.join("sub")).unwrap();
    std::fs::write(w.join("notes.md"), "# Title\n\nSome *text* and `code`.\n\n```rust\nfn main() {}\n```\n").unwrap();
    std::fs::write(w.join("plain.txt"), "one\ntwo\n").unwrap();
    png(&w.join("a.png"), 64, 48);
    png(&w.join("b.png"), 40, 80);

    let p = mouser_core::plugins::find("commander").unwrap();
    let mut m = StdioMode::new(p, 3).unwrap();
    m.activate();
    m.cwd_changed(w);
    let v = settle(&mut m, "list");
    assert!(shows(&v, "notes.md") && shows(&v, "sub") && shows(&v, "a.png"));

    // Views: grid (thumbnails), carousel (big images), back to list.
    for (step, what) in [("switch_view", "grid"), ("bigger", "bigger grid tiles"), ("switch_view", "carousel"), ("right", "carousel next")] {
        m.action(step, 0);
        // Thumbnails load in the background: wait for them.
        let v = settle_when(&mut m, what, |v| !v.images.is_empty());
        assert!(!v.images.is_empty(), "{what}: images shown");
    }
    m.action("switch_view", 0);
    let v = settle(&mut m, "list again");
    assert!(v.images.is_empty() || shows(&v, "notes.md"), "the list");

    // Search: plain typing goes to the plugin, alt keys stay bindings.
    m.action("search", 0);
    settle(&mut m, "search open");
    assert!(m.wants_text(), "searching takes typing");
    assert!(!m.wants_key(b"\x1bt"), "alt+t is still a binding while searching");
    for k in [b"n".as_slice(), b"o"] {
        assert!(m.text(k).is_some());
    }
    assert_eq!(m.text(b"\x1bt"), None, "alt keys fall through to the bindings");
    let v = settle(&mut m, "search typed");
    assert!(shows(&v, "notes.md"), "notes.md matches \"no\"");
    assert!(!shows(&v, "plain.txt"), "plain.txt does not");
    for _ in 0..2 {
        m.text(b"\x7f");
    }
    let v = settle(&mut m, "search erased");
    assert!(shows(&v, "plain.txt"));
    assert!(m.escape(), "Escape closes the search");
    settle(&mut m, "search closed");
    assert!(!m.wants_text());

    // The file view: Markdown rendered, then closed with Escape.
    select(&mut m, "notes.md");
    m.action("open", 0);
    let v = settle_when(&mut m, "Markdown file view", |v| shows(v, "Title"));
    assert!(!shows(&v, "# Title"), "the heading is rendered, not shown raw");
    assert!(m.escape());
    settle(&mut m, "file view closed");
    assert!(!m.escape(), "nothing open: Escape is Mouser's");

    // A folder: open browses it (the shell stays); sync asks for the cd.
    select(&mut m, "sub");
    m.action("open", 0);
    settle_when(&mut m, "browsed into sub", |v| !shows(v, "notes.md"));
    m.action("sync", 0);
    let got = effects(&mut m, 1);
    assert!(matches!(got.as_slice(), [Effect::Cd(d)] if d.ends_with("sub")), "{got:?}");

    // A key with nothing to do (no player: play_pause) types nothing into
    // the shell: commander has the keyboard.
    m.action("play_pause", 0);
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(!effects(&mut m, 1).iter().any(|e| matches!(e, Effect::Type(_))));

    // Mark and pipe: the command line is run in the shell.
    m.action("parent", 0);
    settle_when(&mut m, "back in the folder", |v| shows(v, "notes.md"));
    m.action("last", 0);
    m.action("mark", 0);
    m.action("pipe", 0);
    settle(&mut m, "pipe prompt");
    for k in [b"w".as_slice(), b"c"] {
        m.text(k);
    }
    m.text(b"\r");
    let got = effects(&mut m, 1);
    assert!(matches!(got.as_slice(), [Effect::Run(l)] if l.ends_with("| wc")), "{got:?}");
    drop(m);
}
