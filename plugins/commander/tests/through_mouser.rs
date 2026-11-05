//! The commander plugin end to end, against the built-in mode it replaces:
//! Mouser's adapter (`modes::stdio`) starts the real `mouser-commander`, and
//! for the same folder and window the plugin must draw what the built-in
//! commander draws (list, grid, carousel, search, the file view, images),
//! take the same keys, and ask Mouser for the same effects.

mod common;

use common::*;
use mouser_core::commander::BINDINGS;
use mouser_core::permissions::{Decision, Store};
use mouser_render::modes::commander::CommanderMode;
use mouser_render::modes::stdio::StdioMode;
use mouser_render::modes::{Effect, Mode};

#[test]
fn draws_and_acts_like_the_built_in_commander() {
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

    // The manifest's keys are the built-in's bindings.
    let reg = mouser_core::plugins::Registry::load();
    assert!(reg.broken.is_empty(), "{:?}", reg.broken);
    let e = reg.entries.iter().find(|e| e.name == "commander").unwrap();
    assert_eq!(e.transport, mouser_core::plugins::Transport::Mode, "the plugin replaces the built-in");
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

    let mut b = CommanderMode::new();
    let p = mouser_core::plugins::find("commander").unwrap();
    let mut m = StdioMode::new(p, 3).unwrap();
    for x in [&mut b as &mut dyn Mode, &mut m] {
        x.activate();
        x.cwd_changed(w);
    }
    let v = same(&mut b, &mut m, "list");
    assert!(v.dock.iter().flat_map(|d| &d.items).any(|i| i.text.contains("notes.md")));

    // Views: grid (thumbnails), carousel (big images), back to list.
    for (step, what) in [("switch_view", "grid"), ("bigger", "bigger grid tiles"), ("switch_view", "carousel"), ("right", "carousel next")] {
        b.action(step, 0);
        m.action(step, 0);
        // Thumbnails load in the background on both sides: wait for them.
        let v = same_when(&mut b, &mut m, what, |v| !v.images.is_empty());
        assert!(!v.images.is_empty(), "{what}: images shown");
    }
    b.action("switch_view", 0);
    m.action("switch_view", 0);
    same(&mut b, &mut m, "list again");

    // Search: plain typing goes to the plugin, alt keys stay bindings.
    b.action("search", 0);
    m.action("search", 0);
    same(&mut b, &mut m, "search open");
    assert!(b.wants_text() && m.wants_text(), "searching takes typing");
    assert!(!m.wants_key(b"\x1bt"), "alt+t is still a binding while searching");
    for k in [b"n".as_slice(), b"o"] {
        assert!(b.text(k).is_some());
        assert!(m.text(k).is_some());
    }
    assert_eq!(m.text(b"\x1bt"), None, "alt keys fall through to the bindings");
    let v = same(&mut b, &mut m, "search typed");
    assert!(v.dock.iter().flat_map(|d| &d.items).any(|i| i.text.contains("notes.md")));
    for _ in 0..2 {
        b.text(b"\x7f");
        m.text(b"\x7f");
    }
    same(&mut b, &mut m, "search erased");
    assert!(b.escape() && m.escape(), "Escape closes the search in both");
    same(&mut b, &mut m, "search closed");
    assert!(!m.wants_text());

    // The file view: Markdown rendered, then closed with Escape.
    select(&mut b, &mut m, "notes.md");
    assert_eq!(b.action("open", 0), Effect::None);
    m.action("open", 0);
    same(&mut b, &mut m, "Markdown file view");
    assert!(b.escape() && m.escape());
    same(&mut b, &mut m, "file view closed");
    assert!(!m.escape(), "nothing open: Escape is Mouser's");

    // A folder: open asks for a cd.
    select(&mut b, &mut m, "sub");
    let want = b.action("open", 0);
    assert!(matches!(&want, Effect::Cd(d) if d.ends_with("sub")), "{want:?}");
    m.action("open", 0);
    assert_eq!(effects(&mut m, 1), vec![want]);

    // A key the built-in passes on (no player: alt+k) reaches the shell.
    assert_eq!(b.action("play_pause", 0), Effect::Pass);
    m.action("play_pause", 0);
    assert_eq!(effects(&mut m, 1), vec![Effect::Type("\x1bk".into())]);

    // Mark and pipe: the command line is run in the shell.
    for x in [&mut b as &mut dyn Mode, &mut m] {
        x.action("last", 0);
        x.action("mark", 0);
        x.action("pipe", 0);
    }
    same(&mut b, &mut m, "pipe prompt");
    for k in [b"w".as_slice(), b"c"] {
        b.text(k);
        m.text(k);
    }
    let want = b.text(b"\r").unwrap();
    assert!(matches!(&want, Effect::Run(l) if l.ends_with("| wc")), "{want:?}");
    m.text(b"\r");
    assert_eq!(effects(&mut m, 1), vec![want]);
    drop(m);
}
