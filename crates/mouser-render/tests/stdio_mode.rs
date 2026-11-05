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
use mouser_render::modes::prompt::{Prompt, PromptAnswer, PromptKind};
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
            listeners: ["mode.*", "ui.action", "shell.cwd_changed", "echo-test.*", "input.text", "agent.state"]
                .iter()
                .map(|e| Listener::on(e))
                .collect(),
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
                Some("ask") => {
                    host.request(mouser_mode_api::host_api::Call::Service { method: "permissions.granted".into(), params: json!({}) }).map(|_| ())
                }
                Some("approve") => host
                    .request(mouser_mode_api::host_api::Call::Approve { title: "Delete 3 files?".into(), detail: "a\nb".into(), options: vec![] })
                    .map(|_| ()),
                Some("key") => host
                    .request(mouser_mode_api::host_api::Call::Service {
                        method: "prefs.secret_prompt".into(),
                        params: json!({"key": "models.local.key"}),
                    })
                    .map(|_| ()),
                Some("fetch") => host
                    .request(mouser_mode_api::host_api::Call::Service {
                        method: "fetch".into(),
                        params: json!({"url": format!("http://127.0.0.1:{}/x", e.payload["arg"])}),
                    })
                    .map(|_| ()),
                Some("chat") => {
                    host.request(mouser_mode_api::host_api::Call::Service { method: "agent.send".into(), params: json!({"text": "hi"}) }).map(|_| ())
                }
                Some("field") => {
                    let r = Rect { x: 0, y: 0, w: 400, h: 40 };
                    let it =
                        PanelItem { rect: r, text: String::new(), style: PanelStyle::Field { focused: true, valid: true }, action: Action::None };
                    host.view(Some(Panel { rect: r, items: vec![it] }), None, vec![], false)
                }
                Some("crash") => std::process::exit(9),
                _ => Ok(()),
            },
            "echo-test.tick" => host.view(Some(dock("tick")), None, vec![], false),
            "input.text" => host.effect(Effect::Say(format!("typed {}", e.payload["utf8"].as_str().unwrap_or("")))),
            "agent.state" if e.payload["running"] == false => {
                host.effect(Effect::Say(format!("agent {}", e.payload["entries"].as_array().map_or(0, |v| v.len()))))
            }
            "shell.cwd_changed" => host.effect(Effect::Say(format!("cwd {}", e.payload["dir"].as_str().unwrap_or("")))),
            _ => Ok(()),
        };
        Handled::Handled
    }

    fn response(&mut self, _id: u64, result: Result<serde_json::Value, String>, host: &Host) {
        let _ = host.effect(Effect::Say(format!("granted {}", result.map(|v| v.to_string()).unwrap_or_else(|e| e))));
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

/// Draw until the adapter hands Mouser a popup question.
fn wait_prompt(m: &mut StdioMode, effects: &mut Vec<HostEffect>) -> Prompt {
    let keys = Keymap::new(&[]);
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        let _ = m.view(&ctx(), &keys);
        effects.extend(m.take_effects());
        if let Some(p) = m.take_prompts().pop() {
            return p;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("no prompt; effects {effects:?}");
}

/// An agent host with no model: a turn ends in an error entry.
struct NoModel;
impl mouser_core::agent::AgentHost for NoModel {
    fn context(&self) -> mouser_core::agent::ScreenContext {
        Default::default()
    }
    fn tools(&self) -> Vec<mouser_core::agent::AgentTool> {
        vec![]
    }
    fn check(&self, _: &mouser_core::agent::AgentTool, _: &serde_json::Value) -> mouser_core::agent::Check {
        mouser_core::agent::Check::Allow
    }
    fn call(&self, _: &mouser_core::agent::AgentTool, _: &serde_json::Value) -> Result<String, String> {
        Err("no".into())
    }
    fn model(&self) -> Option<(String, String)> {
        None
    }
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
            "name = \"echo-test\"\nversion = \"0.1.0\"\npermissions = [\"files.read\", \"shell.type\", \"prefs.write\", \"net.fetch\", \"agent.use\"]\n[[provides]]\nkind = \"mode\"\n[run]\ntransport = \"mode\"\ncommand = [\"{exe}\", \"--as-plugin\"]\n[mode]\nhotkey = \"z\"\n"
        ),
    )
    .unwrap();
    let mut store = Store::load();
    store.set("echo-test", "run", Decision::Allow);
    store.set("echo-test", "files.read", Decision::Allow);
    for c in ["prefs.write", "net.fetch", "agent.use"] {
        store.set("echo-test", c, Decision::Allow);
    }
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

    m.action("ask", 0);
    until(&mut m, &mut fx, "a host call answered", |_, fx| {
        fx.iter().any(|e| matches!(e, HostEffect::Say(s) if s.starts_with("granted [\"files.read\"")))
    });

    // Approve and the key prompt become questions for Mouser's popup.
    m.action("approve", 0);
    let p = wait_prompt(&mut m, &mut fx);
    assert!(matches!(&p.kind, PromptKind::Approve { title, detail } if title == "Delete 3 files?" && detail.len() == 2), "{p:?}");
    p.reply.send(PromptAnswer::Deny).unwrap();
    until(&mut m, &mut fx, "approve answered", |_, fx| fx.contains(&HostEffect::Say("granted {\"decision\":\"deny\"}".into())));
    m.action("key", 0);
    let p = wait_prompt(&mut m, &mut fx);
    assert!(matches!(&p.kind, PromptKind::Secret { key, .. } if key == "models.local.key"), "{p:?}");
    p.reply.send(PromptAnswer::Deny).unwrap();
    until(&mut m, &mut fx, "key prompt cancelled", |_, fx| fx.contains(&HostEffect::Say("granted cancelled".into())));

    // fetch runs on a worker thread and answers when done.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port() as i32;
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut s, _) = l.accept().unwrap();
        let mut b = [0u8; 1024];
        let _ = s.read(&mut b);
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
    });
    m.action("fetch", port);
    until(&mut m, &mut fx, "fetch answered", |_, fx| fx.iter().any(|e| matches!(e, HostEffect::Say(s) if s.contains("\"text\":\"ok\""))));

    // Typing reaches the plugin only while its view has a focused field.
    assert!(!m.wants_text());
    assert_eq!(m.text(b"q"), None);
    m.action("field", 0);
    let end = Instant::now() + Duration::from_secs(5);
    while !m.wants_text() && Instant::now() < end {
        let _ = m.view(&ctx(), &keys);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(m.wants_text());
    assert_eq!(m.text(b"q"), Some(HostEffect::None));
    until(&mut m, &mut fx, "typed text", |_, fx| fx.contains(&HostEffect::Say("typed q".into())));

    // An agent session runs in Mouser; the plugin hears agent.state.
    m.set_agent(std::sync::Arc::new(|| mouser_core::agent::Chat::new(std::sync::Arc::new(NoModel))));
    m.action("chat", 0);
    until(&mut m, &mut fx, "agent state", |_, fx| fx.iter().any(|e| matches!(e, HostEffect::Say(s) if s.starts_with("agent ") && s != "agent 0")));

    fx.clear();
    m.action("crash", 0);
    until(&mut m, &mut fx, "crash notice", |v, _| dock_text(v).is_some_and(|t| t.contains("crashed, restarting")));
    // Back after the 1 s wait, and told again that it is the current mode.
    until(&mut m, &mut fx, "dock after restart", |v, _| dock_text(v) == Some("hello"));
    drop(m);
    println!("stdio_mode: ok");
}
