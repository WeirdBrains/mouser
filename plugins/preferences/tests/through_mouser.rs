//! The preferences plugin end to end: Mouser's adapter (`modes::stdio`)
//! starts the real `mouser-preferences` program, which lists every
//! preference, changes one through `prefs.set` (written to preferences.toml)
//! and asks for an API key through Mouser's popup.

use mouser_core::modes::Keymap;
use mouser_core::permissions::{Decision, Store};
use mouser_core::prefs_edit::{self, Kind};
use mouser_render::modes::prompt::{PromptAnswer, PromptKind};
use mouser_render::modes::stdio::StdioMode;
use mouser_render::modes::{Context, Mode, View};
use mouser_render::ui::PanelStyle;
use std::time::{Duration, Instant};

fn ctx() -> Context {
    Context {
        viewport: mouser_render::UiViewport { width: 1200, height: 900, cell_w: 8, cell_h: 16 },
        area: mouser_render::ui::Rect { x: 0, y: 0, w: 1200, h: 900 },
    }
}

fn until(m: &mut StdioMode, what: &str, mut f: impl FnMut(&View, &mut StdioMode) -> bool) {
    let keys = Keymap::new(&[]);
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        let v = m.view(&ctx(), &keys);
        if f(&v, m) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

fn texts(v: &View) -> Vec<String> {
    v.dock.iter().flat_map(|d| &d.items).map(|i| i.text.clone()).collect()
}

#[test]
fn lists_changes_and_asks_for_keys_through_mouser() {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: the only test in this binary; set before anything reads it.
    unsafe { std::env::set_var("MOUSER_HOME", home.path()) };
    unsafe { std::env::set_var("MOUSER_PLUGIN_DIRS", "") };
    let dir = home.path().join("plugins/preferences");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = env!("CARGO_BIN_EXE_mouser-preferences").replace('\\', "\\\\");
    let manifest = include_str!("../plugin.toml").replace("command = [\"./mouser-preferences\"]", &format!("command = [\"{exe}\"]"));
    std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
    let mut store = Store::load();
    store.set("preferences", "run", Decision::Allow);
    store.set("preferences", "prefs.write", Decision::Allow);
    store.save().unwrap();

    let reg = mouser_core::plugins::Registry::load();
    let e = reg.entries.iter().find(|e| e.name == "preferences").unwrap();
    assert_eq!(e.transport, mouser_core::plugins::Transport::Mode, "the plugin replaces the built-in");

    let p = mouser_core::plugins::find("preferences").unwrap();
    let mut m = StdioMode::new(p, 3).unwrap();
    m.activate();
    let schema = prefs_edit::schema();
    until(&mut m, "every preference listed", |v, _| texts(v).iter().any(|t| t.contains(&format!("of {}", schema.len()))));
    let t = texts(&m.view(&ctx(), &Keymap::new(&[])));
    assert!(t.iter().any(|t| t == "Preferences"), "{t:?}");
    assert!(t.iter().any(|t| t == schema[0].label), "{t:?}");

    // A bool: clicking its value flips it and Mouser writes preferences.toml.
    let (i, f) = schema.iter().enumerate().find(|(_, f)| f.kind == Kind::Bool).expect("a bool preference");
    m.action("value", i as i32);
    let path = mouser_core::storage::preferences_path();
    until(&mut m, "the change written", |_, _| std::fs::read_to_string(&path).is_ok_and(|s| s.contains(f.key.rsplit('.').next().unwrap())));
    until(&mut m, "the status line", |v, _| texts(v).iter().any(|t| t.starts_with(&format!("{}: ", f.label))));

    // A text field: Enter opens it, typing goes to the plugin, Enter saves.
    if let Some((i, f)) = schema.iter().enumerate().find(|(_, f)| matches!(f.kind, Kind::Int { .. })) {
        m.action("value", i as i32);
        until(&mut m, "a focused field", |v, _| {
            v.dock.iter().flat_map(|d| &d.items).any(|i| matches!(i.style, PanelStyle::Field { focused: true, .. }))
        });
        assert!(m.wants_text());
        m.text(b"\x15");
        let Kind::Int { min, .. } = f.kind else { unreachable!() };
        let field_text =
            |v: &View| v.dock.iter().flat_map(|d| &d.items).find(|i| matches!(i.style, PanelStyle::Field { .. })).map(|i| i.text.clone());
        // Clear what is there, then type the minimum.
        until(&mut m, "field shown", |v, _| field_text(v).is_some());
        let cur = field_text(&m.view(&ctx(), &Keymap::new(&[]))).unwrap();
        for _ in 0..cur.chars().count() {
            m.text(b"\x7f");
        }
        m.text(min.to_string().as_bytes());
        m.text(b"\r");
        let leaf = f.key.rsplit('.').next().unwrap().to_string();
        until(&mut m, "the typed value written", |_, _| {
            std::fs::read_to_string(&path).is_ok_and(|s| s.lines().any(|l| l.trim() == format!("{leaf} = {min}")))
        });
    }

    // A secret: Enter asks Mouser for its popup; the plugin never gets the value.
    let (i, f) = schema.iter().enumerate().find(|(_, f)| matches!(f.kind, Kind::Secret(_))).expect("a secret preference");
    m.action("value", i as i32);
    let end = Instant::now() + Duration::from_secs(10);
    let p = loop {
        let _ = m.view(&ctx(), &Keymap::new(&[]));
        if let Some(p) = m.take_prompts().pop() {
            break p;
        }
        assert!(Instant::now() < end, "no key prompt");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(matches!(&p.kind, PromptKind::Secret { key, .. } if key == f.key), "{p:?}");
    // Cancel (saving would write the real OS keychain).
    p.reply.send(PromptAnswer::Deny).unwrap();
    until(&mut m, "cancel noted", |v, _| texts(v).iter().any(|t| t.contains("no key entered")));
    drop(m);
}
