//! A headless live run of the chat agent (docs/chat.md §Testing):
//! `cargo run --release -p mouser-app --features local-llm --example chat_live -- "PROMPT"`.
//! Uses the default model (`mouser model default`), a fake screen with a fake
//! token, and one sandboxed tool (`sh -c` in a temp dir with a temp HOME).
//! Approval cards are printed and answered "allow once".

use mouser_core::agent::{AgentHost, AgentTool, Answer, Chat, Check, Entry, ScreenContext};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Sandbox(std::path::PathBuf);

impl AgentHost for Sandbox {
    fn context(&self) -> ScreenContext {
        ScreenContext {
            place: format!("shell — {}", self.0.display()),
            screen: "$ export DEMO_TOKEN=sk-test-0123456789abcdefghij\n$ ls\nnotes.txt  todo.md\n$".into(),
            scrollback: String::new(),
        }
    }
    fn tools(&self) -> Vec<AgentTool> {
        vec![AgentTool {
            name: "mouser__shell_run".into(),
            display: "mouser.shell_run".into(),
            description: "Run a command line in the user's shell and return its output. Asks the user first.".into(),
            schema: json!({"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}),
            always_ask: true,
        }]
    }
    fn check(&self, _: &AgentTool, a: &Value) -> Check {
        Check::Ask(vec![format!("command: {}", a["command"].as_str().unwrap_or(""))])
    }
    fn call(&self, _: &AgentTool, a: &Value) -> Result<String, String> {
        let o = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(a["command"].as_str().unwrap_or(""))
            .current_dir(&self.0)
            .env_clear()
            .env("HOME", &self.0)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .map_err(|e| e.to_string())?;
        Ok(format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
    }
}

fn main() {
    #[cfg(feature = "local-llm")]
    mouser_llm::register();
    let prompt = std::env::args().nth(1).unwrap_or_else(|| "What files are here? Use the shell to list them, hidden ones too.".into());
    let dir = tempfile::tempdir().unwrap();
    for f in ["notes.txt", "todo.md", ".hidden-config"] {
        std::fs::write(dir.path().join(f), "x").unwrap();
    }
    let chat = Chat::new(Arc::new(Sandbox(dir.path().to_path_buf())));
    let t0 = Instant::now();
    chat.send(&prompt);
    while chat.running() {
        if let Some(p) = chat.state.lock().unwrap().pending.clone() {
            println!("[card] approve {}: {}  → allow once", p.display, p.lines.join(" | "));
        }
        if chat.state.lock().unwrap().pending.is_some() {
            chat.answer(Answer::Once);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let s = chat.state.lock().unwrap();
    println!("model: {}  ({:.1}s total)", s.model, t0.elapsed().as_secs_f64());
    for e in &s.entries {
        match e {
            Entry::User(t) => println!("you:  {t}"),
            Entry::Assistant(t) => println!("ai:   {}", t.trim()),
            Entry::Tool { display, args, state, output } => {
                println!("tool: {display} {args} [{state:?}]\n      {}", output.trim().replace('\n', " | "))
            }
            Entry::Note(t) => println!("note: {t}"),
            Entry::Error(t) => println!("err:  {t}"),
        }
    }
}
