//! The chat mode's dock: header, transcript, input, and that typing stays in the input.

use mouser_core::agent::{AgentHost, AgentTool, Chat, Check, ScreenContext};
use mouser_core::modes::Keymap;
use mouser_render::UiViewport;
use mouser_render::modes::chat::{ChatMode, wrap};
use mouser_render::modes::{Context, Effect, Mode};
use mouser_render::ui::{PanelStyle, Rect, UiAction};
use serde_json::Value;
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

fn ctx() -> Context {
    Context { viewport: UiViewport { width: 1200, height: 800, cell_w: 9, cell_h: 18 }, area: Rect { x: 0, y: 30, w: 1000, h: 770 } }
}

/// The tests share MOUSER_HOME.
static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn the_dock_has_a_header_an_input_and_shows_the_transcript() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("MOUSER_HOME", home.path()) };
    // A configured model, so the chat (not the picker) shows.
    mouser_core::models::setup::save_provider(
        mouser_core::models::ProviderEntry {
            name: "custom".into(),
            api: mouser_core::models::Api::OpenAi,
            url: "http://127.0.0.1:9/v1".into(),
            key: None,
            models: vec!["m".into()],
            ca: None,
        },
        None,
    )
    .unwrap();
    mouser_core::models::setup::set_default("custom/m").unwrap();
    let chat = Chat::new(Arc::new(NoModel));
    let mut m = ChatMode::new(chat.clone());
    let keys = Keymap::new(m.bindings());
    assert_eq!((m.name(), m.hotkey()), ("chat", 'a'));
    let v = m.view(&ctx(), &keys);
    let d = v.dock.expect("docked above the prompt");
    assert!(d.rect.h >= 18 * 12 && d.rect.y == 30, "{:?}", d.rect);
    assert!(d.items.iter().any(|i| i.text.starts_with("chat · ")));
    assert!(d.items.iter().any(|i| matches!(i.style, PanelStyle::Field { focused: false, .. }) && i.text.contains("alt+i")));
    assert!(d.items.iter().any(|i| i.action == UiAction::Mode { action: "clear".into(), arg: 0 }));
    // Opening the mode focuses the input; typing goes there, not to the shell.
    m.activate();
    assert!(m.wants_text());
    for b in [&b"h"[..], b"i", b"!", b"\x7f"] {
        assert_eq!(m.text(b), Some(Effect::None));
    }
    assert_eq!(m.text(b"\x1bt"), None, "alt keys are not text");
    let v = m.view(&ctx(), &keys).dock.unwrap();
    assert!(v.items.iter().any(|i| matches!(i.style, PanelStyle::Field { focused: true, .. }) && i.text == "hi▏"));
    // Enter sends: without a model the turn ends with an error in the transcript.
    assert_eq!(m.text(b"\r"), Some(Effect::None));
    let t = Instant::now();
    while chat.running() {
        assert!(t.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
    let v = m.view(&ctx(), &keys).dock.unwrap();
    assert!(v.items.iter().any(|i| i.text.starts_with("you  hi")));
    assert!(v.items.iter().any(|i| i.style == PanelStyle::Error && i.text.contains("no model configured")), "{:#?}", v.items);
    // Esc lets go of the input (nothing running); a second Esc is not the mode's.
    assert!(m.escape() && !m.wants_text() && !m.escape());
    assert_eq!(m.action("cancel", 0), Effect::Say("[chat] nothing running".into()));
}

#[test]
fn wrapping_keeps_words_and_hard_wraps_long_ones() {
    assert_eq!(wrap("aaa bbb ccc", 8), vec!["aaa bbb", "ccc"]);
    assert_eq!(wrap("x\ny", 8), vec!["x", "y"]);
    assert_eq!(wrap(&"z".repeat(20), 8), vec!["zzzzzzzz", "zzzzzzzz", "zzzz"]);
}

/// An OpenAI-compatible mock that answers every request with "OK".
fn ok_server() -> String {
    use std::io::{BufRead, BufReader, Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/v1", l.local_addr().unwrap().port());
    std::thread::spawn(move || {
        while let Ok((s, _)) = l.accept() {
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut len = 0;
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut b = vec![0; len];
            let _ = r.read_exact(&mut b);
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"OK\"}}]}\n\ndata: [DONE]\n\n";
            let mut w = s;
            let _ = write!(w, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        }
    });
    url
}

/// No usable model: `esc :a` opens the picker, a restore at launch doesn't;
/// a message typed first is held and sent once a model is set up.
#[test]
fn no_model_opens_the_picker_and_the_held_message_follows() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("MOUSER_HOME", home.path());
        std::env::set_var("MOUSER_SECRET_STORE", "file");
    }
    let url = ok_server();
    let chat = Chat::new(Arc::new(NoModel));
    let mut m = ChatMode::new(chat.clone());
    let u = url.clone();
    m.make_setup = Some(Box::new(move || {
        let mut s = mouser_core::models::setup::Setup::new(false);
        s.preset_url = Some(u.clone());
        s
    }));
    let keys = Keymap::new(m.bindings());
    // Restored at launch: no picker.
    m.restore();
    assert!(m.setup().is_none());
    // Typing then Enter: the picker opens and keeps the message.
    for b in [&b"h"[..], b"i", b"\r"] {
        m.text(b);
    }
    let d = m.view(&ctx(), &keys).dock.unwrap();
    assert!(d.items.iter().any(|i| i.text == "chat · pick a model"));
    let rows: Vec<_> = d.items.iter().filter(|i| matches!(i.action, UiAction::Mode { ref action, .. } if action == "setup_pick")).collect();
    assert!(rows.len() >= 9, "local x2, six hosted, custom");
    assert!(rows[0].text.contains("Gemma 4 E2B local (default)") && rows[0].text.contains("no local-llm"));
    assert!(rows.iter().any(|r| r.text.starts_with("xAI")));
    // Esc closes the picker (and drops the held message); esc :a opens it again.
    assert!(m.escape());
    assert!(m.setup().is_none());
    m.activate();
    assert!(m.setup().is_some(), "esc :a with no model shows the picker");
    assert!(m.wants_key(b"\x1b[B") && !m.wants_key(b"\x1bt"), "arrows move in the list");
    assert_eq!(m.text(b"\x1b[B"), Some(Effect::None));
    assert_eq!(m.setup().unwrap().sel, 1);
    m.escape();
    for b in [&b"h"[..], b"i", b"\r"] {
        m.text(b);
    }
    // Click xAI, type a (fake) key: the field shows dots only.
    let xai = rows.iter().find(|r| r.text.starts_with("xAI")).unwrap().action.clone();
    let UiAction::Mode { action, arg } = xai else { panic!() };
    m.action(&action, arg);
    for c in "test-key-not-real".chars() {
        m.text(c.to_string().as_bytes());
    }
    let d = m.view(&ctx(), &keys).dock.unwrap();
    assert!(d.items.iter().any(|i| i.text.starts_with("•••••") && matches!(i.style, PanelStyle::Field { focused: true, .. })));
    assert!(!d.items.iter().any(|i| i.text.contains("test-key")), "never shown");
    // Enter: tested against the mock, saved, default set, the held "hi" sent.
    m.text(b"\r");
    let t = Instant::now();
    loop {
        let d = m.view(&ctx(), &keys).dock.unwrap();
        if d.items.iter().any(|i| i.text.starts_with("you  hi")) {
            assert!(d.items.iter().any(|i| i.text.contains("using xai/grok-4")), "{:#?}", d.items);
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "{:#?}", d.items);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(m.setup().is_none());
    assert_eq!(mouser_core::models::prefs().default.as_deref(), Some("xai/grok-4"));
    unsafe { std::env::remove_var("MOUSER_SECRET_STORE") };
}
