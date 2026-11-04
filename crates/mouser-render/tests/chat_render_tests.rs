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

/// A message row: its text drawn as Markdown.
fn said(text: &str, style: &PanelStyle, want: &str) -> bool {
    text == want && matches!(style, PanelStyle::Md { .. })
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
    assert!(v.items.iter().any(|i| said(&i.text, &i.style, "hi")));
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
        if d.items.iter().any(|i| said(&i.text, &i.style, "hi")) {
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

/// Two configured providers in a temp MOUSER_HOME, `custom/m` the default.
fn two_models() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("MOUSER_HOME", home.path()) };
    for (n, m) in [("custom", "m"), ("other", "n")] {
        mouser_core::models::setup::save_provider(
            mouser_core::models::ProviderEntry {
                name: n.into(),
                api: mouser_core::models::Api::OpenAi,
                url: "http://127.0.0.1:9/v1".into(),
                key: None,
                models: vec![m.into()],
                ca: None,
            },
            None,
        )
        .unwrap();
    }
    mouser_core::models::setup::set_default("custom/m").unwrap();
    home
}

fn current() -> String {
    let (p, m) = mouser_core::models::default_choice().unwrap();
    format!("{p}/{m}")
}

#[test]
fn alt_m_opens_the_switch_picker_and_picking_switches_and_persists() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let home = two_models();
    let mut m = ChatMode::new(Chat::new(Arc::new(NoModel)));
    let keys = Keymap::new(m.bindings());
    assert_eq!(keys.lookup(b"\x1bm"), Some("model"));
    assert_eq!(keys.label("model"), "alt+m");
    // No clash with the mode's other keys.
    let ids: Vec<_> = m.bindings().iter().map(|b| b.key).collect();
    assert_eq!(ids.iter().filter(|k| **k == "m").count(), 1);
    m.activate();
    assert!(m.setup().is_none(), "a usable model: no first-run picker");
    assert_eq!(m.action("model", 0), Effect::None);
    let s = m.setup().expect("switch picker open");
    assert!(s.switch);
    let cur = &s.opts[s.sel];
    assert_eq!(cur.label, "● custom/m", "the current model is marked and selected");
    let i = s.opts.iter().position(|o| o.label == "other/n").expect("other configured model listed");
    assert!(s.opts.iter().any(|o| o.label.contains("Gemma 4")), "installable local models listed");
    let v = m.view(&ctx(), &keys).dock.unwrap();
    assert!(v.items.iter().any(|i| i.text == "chat · switch model"));
    assert_eq!(m.action("setup_pick", i as i32), Effect::None);
    assert!(m.setup().is_none());
    assert_eq!(current(), "other/n");
    // Persisted in MOUSER_HOME like `mouser model default`.
    let prefs = std::fs::read_to_string(home.path().join("preferences.toml")).unwrap_or_default();
    let all: String = walk(home.path());
    assert!(prefs.contains("other/n") || all.contains("other/n"), "{all}");
    let v = m.view(&ctx(), &keys).dock.unwrap();
    assert!(v.items.iter().any(|i| i.text.starts_with("chat · other/n")), "header shows the new model");
    assert!(v.items.iter().any(|i| i.text.contains("switched to other/n")));
}

fn walk(p: &std::path::Path) -> String {
    let mut out = String::new();
    for e in std::fs::read_dir(p).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out += &walk(&p);
        } else if let Ok(s) = std::fs::read_to_string(&p) {
            out += &s;
        }
    }
    out
}

#[test]
fn esc_closes_the_switch_picker_with_no_change() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let _home = two_models();
    let mut m = ChatMode::new(Chat::new(Arc::new(NoModel)));
    m.activate();
    m.action("model", 0);
    m.action("down", 0);
    assert!(m.setup().is_some());
    assert!(m.escape());
    assert!(m.setup().is_none());
    assert_eq!(current(), "custom/m");
}

#[test]
fn clicking_the_model_name_in_the_header_opens_the_picker() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let _home = two_models();
    let mut m = ChatMode::new(Chat::new(Arc::new(NoModel)));
    let keys = Keymap::new(m.bindings());
    m.activate();
    let d = m.view(&ctx(), &keys).dock.unwrap();
    let h = d.items.iter().find(|i| i.text.starts_with("chat · custom/m")).expect("header with the model name");
    assert_eq!(h.action, UiAction::Mode { action: "model".into(), arg: 0 });
    // The hit region covers the model name's cells.
    let name_end = h.rect.x + ("chat · custom/m".chars().count() as i32) * 9;
    assert!(h.rect.w > 0 && h.rect.x + h.rect.w >= name_end, "{:?}", h.rect);
    let UiAction::Mode { action, arg } = h.action.clone() else { unreachable!() };
    m.action(&action, arg);
    assert!(m.setup().is_some_and(|s| s.switch));
}

const REPLY: &str = "## Plan\n\nRun **this** with `ls`, see [docs](https://mouser.weirdbrains.com/docs):\n\n```sh\nls -la\necho done\n```\n\n- one\n- two\n\n> careful\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";

/// Replies (and your messages) render as Markdown; links are clickable;
/// a reply streaming in keeps the rows it already has.
#[test]
fn messages_render_markdown_with_links() {
    use mouser_core::agent::Entry;
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("MOUSER_HOME", home.path()) };
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
    {
        let mut s = chat.state.lock().unwrap();
        s.entries.push(Entry::User("what does *ls* do?".into()));
        s.entries.push(Entry::Assistant(REPLY.into()));
    }
    let mut m = ChatMode::new(chat.clone());
    let keys = Keymap::new(m.bindings());
    // The app's geometry: the area starts under the padding (no tab strip).
    let top = mouser_render::frame::PADDING;
    let c = Context { viewport: UiViewport { width: 1000, height: 900, cell_w: 9, cell_h: 18 }, area: Rect { x: 0, y: top, w: 1000, h: 900 - top } };
    let d = m.view(&c, &keys).dock.unwrap();
    let md = |t: &str| d.items.iter().find(|i| i.text == t && matches!(i.style, PanelStyle::Md { .. })).cloned();
    assert_eq!(md("Plan").expect("heading").style, PanelStyle::Md { flags: 1, heading: 2 });
    let styled = |t: &str, f: u8| d.items.iter().any(|i| i.text == t && i.style == PanelStyle::Md { flags: f, heading: 0 });
    assert!(styled("ls", 2), "your message: italic");
    assert!(styled("ls", 4), "inline code");
    assert!(styled("this", 1), "bold");
    assert!(md("ls -la").is_some() && md("echo done").is_some(), "code block rows");
    assert!(d.items.iter().any(|i| matches!(i.style, PanelStyle::MdCode { top: true, bottom: false })));
    assert!(d.items.iter().any(|i| matches!(i.style, PanelStyle::MdCode { top: false, bottom: true })));
    assert!(d.items.iter().any(|i| i.style == PanelStyle::MdQuote));
    assert!(!d.items.iter().any(|i| i.text.contains("**") || i.text.contains("```")), "no raw markup");
    let link = md("docs").expect("link");
    assert_eq!(link.action, UiAction::Mode { action: "link".into(), arg: 0 });
    assert_eq!(m.action("link", 0), Effect::OpenLink("https://mouser.weirdbrains.com/docs".into()));
    assert!(d.items.iter().any(|i| i.text == "you  ") && d.items.iter().any(|i| i.text == "ai   "));
    if let Some(dir) = std::env::var_os("MOUSER_TEST_PNG_DIR") {
        save_png(&d, &c, &std::path::Path::new(&dir).join("chat-markdown.png"));
    }
    // Streaming: as the reply grows, finished rows above stay as they were.
    let rows = |t: &str| mouser_render::modes::chat::rows(&[Entry::Assistant(t.into())], &Default::default(), 60);
    let mut prev = rows("");
    for n in (1..=REPLY.len()).filter(|n| REPLY.is_char_boundary(*n)) {
        let cur = rows(&REPLY[..n]);
        // All but the last two rows (the block being typed and the gap before it).
        let keep = prev.len().saturating_sub(2);
        let changed = (0..keep).filter(|&i| prev[i] != cur[i]).count();
        assert!(changed <= 1, "at {n}: rows above reflowed\n{prev:#?}\n{cur:#?}");
        prev = cur;
    }
}

fn save_png(d: &mouser_render::ui::Panel, c: &Context, path: &std::path::Path) {
    use mouser_render::{Canvas, FontStack, Frame, render_frame, ui};
    let (w, h) = (c.viewport.width as usize, c.viewport.height as usize);
    let mut layout = ui::without_sidebar(w as i32, h as i32);
    let dock = ui::Dock { panel: d.clone(), inset: d.rect.h + 8, fullscreen: false };
    // The grid starts below the dock, so the prompt never sits on the panel.
    let grid_top = mouser_render::frame::PADDING + dock.inset;
    assert!(grid_top >= d.rect.y + d.rect.h, "grid top {grid_top} inside the dock {:?}", d.rect);
    layout.dock = Some(dock);
    let lines = vec!["$ ".to_string()];
    let mut fonts = FontStack::embedded(15.0);
    let mut buf = vec![0u32; w * h];
    let fr = Frame {
        lines: &lines,
        cursor: Some((2, 0)),
        bg: 0x0B1E2A,
        fg: 0xD8E6EE,
        ui: Some(&layout),
        status: None,
        accent: Some(0x2FD8E5),
        cursor_color: None,
        images: &[],
        styles: &[],
        cursor_shape: Default::default(),
        tabs: None,
    };
    render_frame(&mut Canvas::new(&mut buf, w, h), &mut fonts, &fr);
    let rgba: Vec<u8> = buf.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]).collect();
    image::save_buffer(path, &rgba, w as u32, h as u32, image::ColorType::Rgba8).unwrap();
}
