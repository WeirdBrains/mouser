//! The commander plugin end to end, against the built-in mode it replaces:
//! Mouser's adapter (`modes::stdio`) starts the real `mouser-commander`, and
//! for the same folder and window the plugin must draw what the built-in
//! commander draws (list, grid, carousel, search, the file view, images),
//! take the same keys, and ask Mouser for the same effects.

use mouser_core::commander::BINDINGS;
use mouser_core::modes::Keymap;
use mouser_core::permissions::{Decision, Store};
use mouser_render::modes::commander::CommanderMode;
use mouser_render::modes::stdio::StdioMode;
use mouser_render::modes::{Context, Effect, Mode, View};
use std::path::Path;
use std::time::{Duration, Instant};

fn ctx() -> Context {
    Context {
        viewport: mouser_render::UiViewport { width: 1200, height: 900, cell_w: 8, cell_h: 16 },
        area: mouser_render::ui::Rect { x: 0, y: 40, w: 1000, h: 840 },
    }
}

/// What must match: every item (rect, text, style, action) and every
/// image (size, source, place and pixels; keys are handed out in decode
/// order, so only their being distinct counts).
fn shape(v: &View) -> String {
    let mut keys: Vec<u64> = v.images.iter().map(|i| i.key).collect();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), v.images.len(), "one key per image");
    let imgs: Vec<_> =
        v.images.iter().map(|i| (i.width, i.height, i.src, i.rect, i.rgba.len(), i.rgba.iter().map(|&b| b as u64).sum::<u64>())).collect();
    format!("{:#?}\n{:#?}\n{imgs:?}\n{}", v.dock, v.panel, v.fullscreen)
}

/// Draw both until they agree (thumbnails decode on threads in both).
fn same(b: &mut CommanderMode, p: &mut StdioMode, what: &str) -> View {
    let keys = Keymap::new(BINDINGS);
    let end = Instant::now() + Duration::from_secs(20);
    let (mut vb, mut vp);
    loop {
        vb = b.view(&ctx(), &keys);
        vp = p.view(&ctx(), &keys);
        if shape(&vb) == shape(&vp) && vp.dock.is_some() {
            return vp;
        }
        if Instant::now() > end {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let t = std::env::temp_dir();
    let _ = std::fs::write(t.join("commander-builtin.txt"), shape(&vb));
    let _ = std::fs::write(t.join("commander-plugin.txt"), shape(&vp));
    panic!(
        "{what}: the plugin draws differently (both in the temp dir: commander-builtin.txt, commander-plugin.txt)\n--- built-in\n{}\n--- plugin\n{}",
        shape(&vb),
        shape(&vp)
    );
}

/// The plugin's effects (they come back after a round trip).
fn effects(p: &mut StdioMode, want: usize) -> Vec<Effect> {
    let end = Instant::now() + Duration::from_secs(10);
    let mut got = Vec::new();
    while got.len() < want && Instant::now() < end {
        let _ = p.view(&ctx(), &Keymap::new(BINDINGS));
        got.extend(p.take_effects().into_iter().filter(|e| !matches!(e, Effect::Say(_))));
        std::thread::sleep(Duration::from_millis(10));
    }
    got
}

/// Move both down from the top to the row named `name`.
fn select(b: &mut CommanderMode, m: &mut StdioMode, name: &str) {
    for x in [&mut *b as &mut dyn Mode, &mut *m] {
        x.action("first", 0);
    }
    for _ in 0..20 {
        let v = same(b, m, name);
        let sel = v
            .dock
            .iter()
            .flat_map(|d| &d.items)
            .find(|i| matches!(i.style, mouser_render::ui::PanelStyle::Row { selected: true }))
            .map(|i| i.text.clone());
        if sel.as_deref().is_some_and(|t| t.split_whitespace().any(|w| w.trim_end_matches('/') == name)) {
            return;
        }
        b.action("down", 0);
        m.action("down", 0);
    }
    panic!("{name} not in the list");
}

fn png(p: &Path, w: u32, h: u32) {
    image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 7) as u8, (y * 5) as u8, 90, 255])).save(p).unwrap();
}

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
        let v = same(&mut b, &mut m, what);
        if what != "list" {
            assert!(!v.images.is_empty(), "{what}: images shown");
        }
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
