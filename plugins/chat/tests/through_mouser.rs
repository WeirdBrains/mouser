//! The chat plugin end to end: Mouser's adapter starts the real
//! `mouser-chat` program with an agent session on a test host whose model
//! streams a Markdown reply. Typing goes to the plugin's input, Enter sends
//! through `agent.send`, and the reply comes back as `agent.state` events and
//! is drawn as Markdown rows.

use mouser_core::agent::{AgentHost, AgentTool, Chat, Check, ScreenContext};
use mouser_core::models::{ChatRequest, ChatSink, Event, ModelInfo, ModelProvider};
use mouser_core::modes::Keymap;
use mouser_core::permissions::{Decision, Store};
use mouser_render::modes::prompt::PromptKind;
use mouser_render::modes::stdio::StdioMode;
use mouser_render::modes::{Context, Mode, View};
use mouser_render::ui::PanelStyle;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// MOUSER_HOME is process-global; serialize tests that set it.
static HOME_LOCK: Mutex<()> = Mutex::new(());

struct Echo;
impl ModelProvider for Echo {
    fn id(&self) -> &str {
        "echo"
    }
    fn models(&self) -> Vec<ModelInfo> {
        vec![]
    }
    fn chat(&self, _req: &ChatRequest, sink: &mut dyn ChatSink) -> anyhow::Result<()> {
        for t in ["**Hello** ", "from `echo`"] {
            sink.event(Event::Text(t.into()));
        }
        sink.event(Event::Done { stop: "end".into() });
        Ok(())
    }
}

struct Host;
impl AgentHost for Host {
    fn context(&self) -> ScreenContext {
        Default::default()
    }
    fn tools(&self) -> Vec<AgentTool> {
        vec![]
    }
    fn check(&self, _: &AgentTool, _: &Value) -> Check {
        Check::Allow
    }
    fn call(&self, _: &AgentTool, _: &Value) -> Result<String, String> {
        Err("no tools".into())
    }
    fn model(&self) -> Option<(String, String)> {
        Some(("echo".into(), "e1".into()))
    }
    fn open(&self, _: &str) -> anyhow::Result<Box<dyn ModelProvider>> {
        Ok(Box::new(Echo))
    }
}

fn ctx() -> Context {
    Context {
        viewport: mouser_render::UiViewport { width: 1200, height: 900, cell_w: 8, cell_h: 16 },
        area: mouser_render::ui::Rect { x: 0, y: 0, w: 1200, h: 900 },
        reduce_motion: false,
    }
}

fn until(m: &mut StdioMode, what: &str, mut f: impl FnMut(&View) -> bool) {
    let keys = Keymap::new(&[]);
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        if f(&m.view(&ctx(), &keys)) {
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
fn chats_through_mouser_agent() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    // SAFETY: held under HOME_LOCK; set before anything reads it.
    unsafe { std::env::set_var("MOUSER_HOME", home.path()) };
    let dir = home.path().join("plugins/chat");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = env!("CARGO_BIN_EXE_mouser-chat").replace('\\', "\\\\");
    let manifest = include_str!("../plugin.toml").replace("command = [\"./mouser-chat\"]", &format!("command = [\"{exe}\"]"));
    std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
    let mut store = Store::load();
    store.set("chat", "run", Decision::Allow);
    store.set("chat", "agent.use", Decision::Allow);
    store.save().unwrap();

    let p = mouser_core::plugins::find("chat").expect("chat plugin");
    let mut m = StdioMode::new(p, 3).unwrap();
    m.set_agent(Arc::new(|| Chat::new(Arc::new(Host))));
    m.activate();
    until(&mut m, "the chat's rounded frame", |v| {
        v.dock.iter().flat_map(|d| &d.items).any(|i| matches!(i.style, PanelStyle::Frame { .. }) && i.text.starts_with("Chat: "))
    });
    until(&mut m, "a focused input", |v| v.dock.iter().flat_map(|d| &d.items).any(|i| matches!(i.style, PanelStyle::Field { focused: true, .. })));
    assert!(m.wants_text());
    for b in ["h", "i", "\r"] {
        m.text(b.as_bytes());
    }
    until(&mut m, "the streamed Markdown reply", |v| {
        let t = texts(v);
        t.iter().any(|t| t == "Hello") && t.iter().any(|t| t == "echo") && t.iter().any(|t| t == "hi")
    });
    let v = m.view(&ctx(), &Keymap::new(&[]));
    let bold = v.dock.iter().flat_map(|d| &d.items).find(|i| i.text == "Hello").unwrap();
    assert!(matches!(bold.style, PanelStyle::Md { flags, .. } if flags & mouser_core::markdown::BOLD != 0), "{:?}", bold.style);
    let bubbles: Vec<bool> =
        v.dock.iter().flat_map(|d| &d.items).filter_map(|i| if let PanelStyle::Bubble { mine } = i.style { Some(mine) } else { None }).collect();
    assert_eq!(bubbles, [true, false], "your message and the reply, each in a bubble");
    drop(m);
}

#[test]
fn asks_for_agent_use_instead_of_hard_error() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("MOUSER_HOME", home.path()) };
    let dir = home.path().join("plugins/chat");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = env!("CARGO_BIN_EXE_mouser-chat").replace('\\', "\\\\");
    let manifest = include_str!("../plugin.toml").replace("command = [\"./mouser-chat\"]", &format!("command = [\"{exe}\"]"));
    std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
    let mut store = Store::load();
    store.set("chat", "run", Decision::Allow);
    store.save().unwrap();

    let plug = mouser_core::plugins::find("chat").expect("chat plugin");
    let mut m = StdioMode::new(plug, 3).unwrap();
    m.set_agent(Arc::new(|| Chat::new(Arc::new(Host))));
    m.activate();
    let end = Instant::now() + Duration::from_secs(5);
    let mut saw_prompt = false;
    while Instant::now() < end {
        let prompts = m.take_prompts();
        if prompts.iter().any(|p| matches!(&p.kind, PromptKind::Permission { cap, .. } if cap == "agent.use")) {
            saw_prompt = true;
            break;
        }
        let v = m.view(&ctx(), &Keymap::new(&[]));
        assert!(!texts(&v).iter().any(|t| t.contains("is not allowed")), "hard error while Ask pending: {:?}", texts(&v));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(saw_prompt, "expected agent.use permission prompt on chat welcome");
    drop(m);
}

#[test]
fn pictures_named_in_the_chat_play_in_mousers_host_player() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("MOUSER_HOME", home.path()) };
    let dir = home.path().join("plugins/chat");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = env!("CARGO_BIN_EXE_mouser-chat").replace('\\', "\\\\");
    let manifest = include_str!("../plugin.toml").replace("command = [\"./mouser-chat\"]", &format!("command = [\"{exe}\"]"));
    std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
    let mut store = Store::load();
    for cap in ["run", "agent.use", "files.read"] {
        store.set("chat", cap, Decision::Allow);
    }
    store.save().unwrap();
    let pic = home.path().join("cat.png");
    image::RgbaImage::from_pixel(40, 30, image::Rgba([9, 9, 9, 255])).save(&pic).unwrap();

    let p = mouser_core::plugins::find("chat").expect("chat plugin");
    let mut m = StdioMode::new(p, 3).unwrap();
    m.set_agent(Arc::new(|| Chat::new(Arc::new(Host))));
    m.activate();
    until(&mut m, "a focused input", |v| v.dock.iter().flat_map(|d| &d.items).any(|i| matches!(i.style, PanelStyle::Field { focused: true, .. })));
    for c in format!("see {}\r", pic.display().to_string().replace('\\', "/")).chars() {
        m.text(c.to_string().as_bytes());
    }
    let full = |v: &View| {
        v.dock.iter().flat_map(|d| &d.items).find_map(|i| match &i.action {
            mouser_render::ui::UiAction::Mode { action, arg } if action == "__hosted.media_full" => Some(*arg),
            _ => None,
        })
    };
    // Mouser (not the plugin) loads and draws the picture under the message.
    until(&mut m, "the host-drawn picture", |v| full(v).is_some() && v.images.iter().any(|i| (i.width, i.height) == (40, 30)));
    let v = m.view(&ctx(), &Keymap::new(&[]));
    m.action("__hosted.media_full", full(&v).unwrap());
    until(&mut m, "the host's fullscreen viewer", |v| texts(v).iter().any(|t| t.contains("close esc")));
    assert!(m.escape(), "Esc closes Mouser's viewer first");
    drop(m);
}

#[test]
fn a_clip_keeps_playing_when_the_chat_scrolls_it_out_of_view() {
    if !mouser_media::player::available() {
        eprintln!("skipped: no ffmpeg / ffprobe on PATH");
        return;
    }
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("MOUSER_HOME", home.path()) };
    let dir = home.path().join("plugins/chat");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = env!("CARGO_BIN_EXE_mouser-chat").replace('\\', "\\\\");
    let manifest = include_str!("../plugin.toml").replace("command = [\"./mouser-chat\"]", &format!("command = [\"{exe}\"]"));
    std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
    let mut store = Store::load();
    for cap in ["run", "agent.use", "files.read"] {
        store.set("chat", cap, Decision::Allow);
    }
    store.save().unwrap();
    let clip = home.path().join("clip.mp4");
    let st = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=96x64:rate=10:duration=20"])
        .args(["-f", "lavfi", "-i", "anullsrc=r=22050:cl=mono", "-t", "20", "-c:v", "mpeg4", "-c:a", "aac", "-shortest"])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(st.success());

    let p = mouser_core::plugins::find("chat").expect("chat plugin");
    let mut m = StdioMode::new(p, 3).unwrap();
    m.set_agent(Arc::new(|| Chat::new(Arc::new(Host))));
    m.activate();
    until(&mut m, "a focused input", |v| v.dock.iter().flat_map(|d| &d.items).any(|i| matches!(i.style, PanelStyle::Field { focused: true, .. })));
    let type_line = |m: &mut StdioMode, s: &str| {
        for c in format!("{s}\r").chars() {
            m.text(c.to_string().as_bytes());
        }
    };
    type_line(&mut m, &format!("see {}", clip.display().to_string().replace('\\', "/")));
    let play = |v: &View| {
        v.dock.iter().flat_map(|d| &d.items).find_map(|i| match &i.action {
            mouser_render::ui::UiAction::Mode { action, arg } if action == "__hosted.media_play" => Some((*arg, i.text.clone())),
            _ => None,
        })
    };
    until(&mut m, "the clip's player", |v| play(v).is_some());
    let v = m.view(&ctx(), &Keymap::new(&[]));
    m.action("__hosted.media_play", play(&v).unwrap().0);
    until(&mut m, "playing", |v| play(v).is_some_and(|p| p.1 == "❚❚"));
    // Enough turns to push the clip off the top.
    let mut turns = 1;
    while play(&m.view(&ctx(), &Keymap::new(&[]))).is_some() {
        assert!(turns < 60, "the clip never scrolled out");
        type_line(&mut m, &format!("turn {turns}"));
        let want = format!("turn {turns}");
        until(&mut m, "the turn", |v| texts(v).iter().any(|t| t == &want));
        until(&mut m, "the reply", |v| !texts(v).iter().any(|t| t.contains("thinking")));
        turns += 1;
    }
    std::thread::sleep(Duration::from_millis(1500));
    for _ in 0..40 {
        m.view(&ctx(), &Keymap::new(&[]));
        std::thread::sleep(Duration::from_millis(10));
    }
    // Back up to it: still playing (it would show ▶ had it stopped).
    for _ in 0..30 {
        m.action("up", 0);
        if play(&m.view(&ctx(), &Keymap::new(&[]))).is_some() {
            break;
        }
    }
    until(&mut m, "the clip still playing", |v| play(v).is_some_and(|p| p.1 == "❚❚"));
    drop(m);
}
