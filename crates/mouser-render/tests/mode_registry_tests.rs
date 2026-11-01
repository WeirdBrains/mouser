//! P6: the mode switcher comes from the plugin registry (built-in commander and
//! chat, plus mode plugins over the command transport), built-ins can be
//! disabled, and with no mode Mouser is a plain terminal. One test: it moves
//! MOUSER_HOME.

use mouser_core::agent::{AgentHost, AgentTool, Chat, Check, ScreenContext};
use mouser_core::modes::Keymap;
use mouser_core::permissions::{Decision, Store};
use mouser_core::plugins::{self, Registry};
use mouser_render::UiViewport;
use mouser_render::modes::{Context, Host, Mode, chat::ChatMode};
use mouser_render::ui::Rect;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct NoModel;
impl AgentHost for NoModel {
    fn context(&self) -> ScreenContext {
        ScreenContext::default()
    }
    fn tools(&self) -> Vec<AgentTool> {
        vec![]
    }
    fn check(&self, _: &AgentTool, _: &Value) -> Check {
        Check::Allow
    }
    fn call(&self, _: &AgentTool, _: &Value) -> Result<String, String> {
        Ok(String::new())
    }
    fn model(&self) -> Option<(String, String)> {
        None
    }
}

fn host() -> Host {
    Host::from_registry(&Registry::load(), |e| (e.name == "chat").then(|| Box::new(ChatMode::new(Chat::new(Arc::new(NoModel)))) as Box<dyn Mode>))
}

fn example(n: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins").join(n)
}

fn ctx() -> Context {
    Context { viewport: UiViewport { width: 1200, height: 800, cell_w: 9, cell_h: 18 }, area: Rect { x: 0, y: 30, w: 1000, h: 770 } }
}

fn dock_text(m: &mut dyn Mode) -> String {
    let keys = Keymap::new(m.bindings());
    m.view(&ctx(), &keys).dock.map(|d| d.items.iter().map(|i| i.text.clone()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

/// Wait until the plugin's reply is in the dock.
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
    // Defaults: commander (the default, active) then chat.
    let h = host();
    assert_eq!(h.names(), ["commander", "chat"]);
    assert_eq!(h.current(), Some("commander"));
    // A build without the chat mode leaves it out with a note.
    let h = Host::from_registry(&Registry::load(), |_| None);
    assert_eq!(h.names(), ["commander"]);
    assert!(h.notes.iter().any(|n| n.contains("chat")), "{:?}", h.notes);
    // Priority order decides the switcher order and the default.
    plugins::move_to("chat", 0).unwrap();
    assert_eq!(host().names(), ["chat", "commander"]);
    plugins::move_to("commander", 0).unwrap();
    // Built-ins can be turned off; with none on, an empty host (plain terminal).
    plugins::set_enabled("commander", false).unwrap();
    let mut h = host();
    assert_eq!(h.names(), ["chat"]);
    assert!(h.switch("c").is_err(), "commander is off");
    plugins::set_enabled("chat", false).unwrap();
    let h = host();
    assert!(h.names().is_empty());
    assert_eq!(h.current(), None);
    plugins::set_enabled("commander", true).unwrap();
    plugins::set_enabled("chat", true).unwrap();

    // A mode plugin over the command transport joins after the built-in modes.
    plugins::install(&example("hello-mode")).unwrap();
    let mut h = host();
    assert_eq!(h.names(), ["commander", "chat", "hello-mode"]);
    assert_eq!(h.current(), Some("commander"), "installing a mode keeps the default");
    assert_eq!(h.switch("h").unwrap(), "hello-mode mode");
    // A second plugin with the same hotkey is left out with a note.
    let dup = home.path().join("dup");
    std::fs::create_dir_all(&dup).unwrap();
    let toml = std::fs::read_to_string(example("hello-mode").join("plugin.toml")).unwrap().replace("name = \"hello-mode\"", "name = \"dup-mode\"");
    std::fs::write(dup.join("plugin.toml"), toml).unwrap();
    plugins::install(&dup).unwrap();
    let h = host();
    assert!(!h.names().contains(&"dup-mode"));
    assert!(h.notes.iter().any(|n| n.contains("dup-mode") && n.contains("hotkey h")), "{:?}", h.notes);
    plugins::uninstall("dup-mode").unwrap();
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
    m.cwd_changed(home);
    let s = wait_for(&mut m, &home.display().to_string());
    assert!(s.contains("event: cwd"), "{s}");
    m.action("time", 0);
    let s = wait_for(&mut m, "it is ");
    assert!(s.contains("event: key"), "{s}");
    // Unknown ids are ignored.
    let _ = m.action("nope", 0);
}
