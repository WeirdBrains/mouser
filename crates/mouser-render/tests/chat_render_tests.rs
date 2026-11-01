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

#[test]
fn the_dock_has_a_header_an_input_and_shows_the_transcript() {
    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("MOUSER_HOME", home.path()) };
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
