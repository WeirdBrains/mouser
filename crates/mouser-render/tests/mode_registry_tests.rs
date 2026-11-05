//! P6 and 1.4.0: the mode switcher comes from the plugin registry (only
//! installed mode plugins; Mouser has no built-in modes), modes can be
//! disabled, and with no mode Mouser is a plain terminal. One test: it moves
//! MOUSER_HOME.

use mouser_core::plugins::{self, Registry};
use mouser_render::modes::Host;
#[cfg(unix)]
use mouser_render::modes::Mode;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use {
    mouser_core::modes::Keymap,
    mouser_core::permissions::{Decision, Store},
    mouser_render::UiViewport,
    mouser_render::modes::Context,
    mouser_render::ui::Rect,
    std::time::{Duration, Instant},
};

fn host() -> Host {
    Host::from_registry(&Registry::load())
}

fn example(n: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins").join(n)
}

#[cfg(unix)]
fn ctx() -> Context {
    Context { viewport: UiViewport { width: 1200, height: 800, cell_w: 9, cell_h: 18 }, area: Rect { x: 0, y: 30, w: 1000, h: 770 } }
}

#[cfg(unix)]
fn dock_text(m: &mut dyn Mode) -> String {
    let keys = Keymap::new(m.bindings());
    m.view(&ctx(), &keys).dock.map(|d| d.items.iter().map(|i| i.text.clone()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

/// Wait until the plugin's reply is in the dock.
#[cfg(unix)]
fn wait_for(m: &mut dyn Mode, want: &str) -> String {
    let t = Instant::now();
    loop {
        let s = dock_text(m);
        if s.contains(want) || t.elapsed() > Duration::from_secs(10) {
            return s;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn modes_come_from_the_registry() {
    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("MOUSER_HOME", home.path()) };
    unsafe { std::env::set_var("MOUSER_PLUGIN_DIRS", "") };
    // Nothing installed: no mode at all (a plain terminal), nothing built in.
    let h = host();
    assert!(h.names().is_empty(), "{:?}", h.names());
    assert_eq!(h.current(), None);

    // A mode plugin over the command transport is the mode.
    plugins::install(&example("hello-mode")).unwrap();
    let mut h = host();
    assert_eq!(h.names(), ["hello-mode"]);
    assert_eq!(h.current(), Some("hello-mode"), "the only mode is the default");
    assert!(h.switch("h").is_ok());
    // Disabled: gone again.
    plugins::set_enabled("hello-mode", false).unwrap();
    assert!(host().names().is_empty());
    plugins::set_enabled("hello-mode", true).unwrap();

    // A second plugin with the same hotkey is left out with a note.
    let dup = home.path().join("dup");
    std::fs::create_dir_all(&dup).unwrap();
    let toml = std::fs::read_to_string(example("hello-mode").join("plugin.toml")).unwrap().replace("name = \"hello-mode\"", "name = \"dup-mode\"");
    std::fs::write(dup.join("plugin.toml"), &toml).unwrap();
    plugins::install(&dup).unwrap();
    let h = host();
    assert!(!h.names().contains(&"dup-mode"));
    assert!(h.notes.iter().any(|n| n.contains("dup-mode") && n.contains("hotkey h")), "{:?}", h.notes);
    plugins::uninstall("dup-mode").unwrap();

    // The official modes' hotkeys stay theirs: another plugin can't take `c`.
    let taker = toml.replace("name = \"dup-mode\"", "name = \"taker\"").replace("hotkey = \"h\"", "hotkey = \"c\"");
    assert!(plugins::Manifest::parse(&taker).is_err(), "c is commander's");
    plugin_mode_runs_with_permission(home.path());
    unsafe { std::env::remove_var("MOUSER_HOME") };
}

#[cfg(not(unix))]
fn plugin_mode_runs_with_permission(_: &Path) {}

#[cfg(unix)]
fn plugin_mode_runs_with_permission(home: &Path) {
    use mouser_render::modes::plugin::PluginMode;
    let p = plugins::find("hello-mode").unwrap();
    let mut m = PluginMode::new(p.clone()).unwrap();
    assert_eq!((m.name(), m.hotkey()), ("hello-mode", 'h'));
    assert_eq!(m.bindings().iter().map(|b| (b.id, b.key)).collect::<Vec<_>>(), [("time", "t"), ("files", "f")]);
    // Nothing runs until `run` is allowed.
    m.activate();
    let s = dock_text(&mut m);
    assert!(s.contains("needs permission to run") && s.contains("run=allow"), "{s}");
    let mut st = Store::load();
    st.set("hello-mode", "run", Decision::Allow);
    st.save().unwrap();
    m.activate();
    let s = wait_for(&mut m, "event: activate");
    assert!(s.contains("hello from hello-mode") && s.contains("not shared"), "no cwd without files.read: {s}");
    let mut st = Store::load();
    st.set("hello-mode", "files.read", Decision::Allow);
    st.save().unwrap();
    // The cwd change arrives while the activate run is busy: queued, and it
    // carries the new directory (not the one from the earlier request).
    m.activate();
    m.cwd_changed(home);
    let s = wait_for(&mut m, &home.display().to_string());
    assert!(s.contains("event: cwd"), "{s}");
    m.action("time", 0);
    let s = wait_for(&mut m, "it is ");
    assert!(s.contains("event: key"), "{s}");
    // Unknown ids are ignored.
    let _ = m.action("nope", 0);
}
