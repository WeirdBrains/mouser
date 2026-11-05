//! `transport = "mode"` end to end: Mouser's adapter (`modes::stdio`) starts
//! a real plugin process, which is this test binary run with
//! `--as-plugin` and built on `mouser_mode_api::serve`. Covers the dock, the
//! plugin's own click actions, effects and their permission check,
//! self-delivered emits, `shell.cwd_changed`, and a crash with restart.
//! (`harness = false`: libtest would write to stdout, which is the protocol.)

use mouser_core::modes::Keymap;
use mouser_core::permissions::{Decision, Store};
use mouser_mode_api::events::{Event, EventDecl, Listener};
use mouser_mode_api::protocol::{ApiRange, Handled, Hello, Provide};
use mouser_mode_api::ui::{Action, Panel, PanelItem};
use mouser_mode_api::{Effect, Host, PanelStyle, Rect};
use mouser_render::modes::stdio::StdioMode;
use mouser_render::modes::{Context, Effect as HostEffect, Mode};
use mouser_render::ui::UiAction;
use serde_json::json;
use std::time::{Duration, Instant};

struct Plugin;

fn dock(text: &str) -> Panel {
    let r = Rect { x: 0, y: 0, w: 400, h: 40 };
    Panel {
        rect: r,
        items: vec![PanelItem { rect: r, text: text.into(), style: PanelStyle::Label, action: Action::Mode { action: "go".into(), arg: 7 } }],
    }
}

impl mouser_mode_api::Mode for Plugin {
    fn hello(&self) -> Hello {
        Hello {
            api: ApiRange::CURRENT,
            name: "echo-test".into(),
            version: "0".into(),
            provides: vec![Provide::Mode { hotkey: 'z', bindings: vec![], activate_on: vec![] }],
            listeners: ["mode.*", "ui.action", "shell.cwd_changed", "echo-test.*"].iter().map(|e| Listener::on(e)).collect(),
            emits: vec![EventDecl { name: "echo-test.tick".into(), schema: None }],
            wants_mcp: false,
            shm: true,
        }
    }

    fn event(&mut self, e: &Event, host: &Host) -> Handled {
        let _ = match e.name.as_str() {
            "mode.activated" => host.view(Some(dock("hello")), None, vec![], false),
            "ui.action" => match e.payload["id"].as_str() {
                Some("ping") => {
                    let _ = host.effect(Effect::Say("pong".into()));
                    let _ = host.effect(Effect::Type("ls".into()));
                    host.emit("echo-test.tick", json!({}))
                }
                Some("crash") => std::process::exit(9),
                _ => Ok(()),
            },
            "echo-test.tick" => host.view(Some(dock("tick")), None, vec![], false),
            "shell.cwd_changed" => host.effect(Effect::Say(format!("cwd {}", e.payload["dir"].as_str().unwrap_or("")))),
            _ => Ok(()),
        };
        Handled::Handled
    }
}

fn ctx() -> Context {
    Context { viewport: mouser_render::UiViewport { width: 800, height: 600, cell_w: 8, cell_h: 16 }, area: Rect { x: 0, y: 0, w: 800, h: 600 } }
}

/// Draw until `f` holds (5 s at most), collecting effects.
fn until(m: &mut StdioMode, effects: &mut Vec<HostEffect>, what: &str, mut f: impl FnMut(&mouser_render::modes::View, &[HostEffect]) -> bool) {
    let keys = Keymap::new(&[]);
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        let v = m.view(&ctx(), &keys);
        effects.extend(m.take_effects());
        if f(&v, effects) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}; effects {effects:?}");
}

fn dock_text(v: &mouser_render::modes::View) -> Option<&str> {
    v.dock.as_ref().and_then(|d| d.items.first()).map(|i| i.text.as_str())
}

fn main() {
    if std::env::args().any(|a| a == "--as-plugin") {
        let _ = mouser_mode_api::serve(Plugin);
        return;
    }
    let home = tempfile::tempdir().unwrap();
    // SAFETY: single-threaded here, before anything reads the environment.
    unsafe { std::env::set_var("MOUSER_HOME", home.path()) };
    let dir = home.path().join("plugins/echo-test");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = std::env::current_exe().unwrap().display().to_string().replace('\\', "\\\\");
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            "name = \"echo-test\"\nversion = \"0.1.0\"\npermissions = [\"files.read\", \"shell.type\"]\n[[provides]]\nkind = \"mode\"\n[run]\ntransport = \"mode\"\ncommand = [\"{exe}\", \"--as-plugin\"]\n[mode]\nhotkey = \"z\"\n"
        ),
    )
    .unwrap();
    let mut store = Store::load();
    store.set("echo-test", "run", Decision::Allow);
    store.set("echo-test", "files.read", Decision::Allow);
    store.save().unwrap();

    let installed = mouser_core::plugins::find("echo-test").expect("plugin installed");
    let mut m = StdioMode::new(installed, 3).unwrap();
    let mut fx = Vec::new();
    let keys = Keymap::new(&[]);
    // Activated before its process runs (like the first mode at startup).
    m.activate();
    until(&mut m, &mut fx, "the dock", |v, _| dock_text(v) == Some("hello"));
    let v = m.view(&ctx(), &keys);
    assert_eq!(v.dock.unwrap().items[0].action, UiAction::Mode { action: "go".into(), arg: 7 }, "the plugin's own action, converted");

    m.action("ping", 0);
    until(&mut m, &mut fx, "pong, the refused Type and the self-delivered tick", |v, fx| {
        dock_text(v) == Some("tick")
            && fx.contains(&HostEffect::Say("pong".into()))
            && fx.iter().any(|e| matches!(e, HostEffect::Say(s) if s.contains("needs shell.type")))
    });
    assert!(!fx.iter().any(|e| matches!(e, HostEffect::Type(_))), "Type without shell.type is refused");

    m.cwd_changed(std::path::Path::new("/somewhere"));
    until(&mut m, &mut fx, "cwd event", |_, fx| fx.iter().any(|e| matches!(e, HostEffect::Say(s) if s == "cwd /somewhere")));

    fx.clear();
    m.action("crash", 0);
    until(&mut m, &mut fx, "crash notice", |v, _| dock_text(v).is_some_and(|t| t.contains("crashed, restarting")));
    // Back after the 1 s wait, and told again that it is the current mode.
    until(&mut m, &mut fx, "dock after restart", |v, _| dock_text(v) == Some("hello"));
    drop(m);
    println!("stdio_mode: ok");
}
