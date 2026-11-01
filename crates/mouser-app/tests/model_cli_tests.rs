//! `mouser model …` under a temporary MOUSER_HOME, file secret store, mock HTTP only.

use mouser_app::model_cli::{Action, run};
use std::io::{BufRead, BufReader, Read, Write};

fn cli(a: Action) -> anyhow::Result<String> {
    let mut v = Vec::new();
    run(&a, &mut v)?;
    Ok(String::from_utf8(v).unwrap())
}

/// An OpenAI-compatible mock answering `n` requests; returns (base url, captured heads).
fn mock(n: usize) -> (String, std::sync::mpsc::Receiver<String>) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/v1", l.local_addr().unwrap().port());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..n {
            let (s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let (mut head, mut len) = (String::new(), 0);
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h == "\r\n" {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                head.push_str(&h);
            }
            let mut b = vec![0; len];
            r.read_exact(&mut b).unwrap();
            let body = String::from_utf8(b).unwrap();
            let reply = if body.contains("\"tools\"") {
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"get_time\",\"arguments\":\"{\\\"city\\\":\\\"Oslo\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n"
            } else {
                "data: {\"choices\":[{\"delta\":{\"content\":\"Hello from mock\"},\"finish_reason\":\"stop\"}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":3}}\n\ndata: [DONE]\n\n"
            };
            let mut w = s;
            write!(w, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{reply}", reply.len()).unwrap();
            tx.send(head).unwrap();
        }
    });
    (url, rx)
}

#[test]
#[serial_test::serial]
fn model_add_list_test_default_remove() {
    let t = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("MOUSER_HOME", t.path());
        std::env::set_var("MOUSER_SECRET_STORE", "file");
    }
    let add = |name: &str, api: Option<&str>, url: Option<&str>, key: Option<&str>, models: &[&str]| {
        cli(Action::Add {
            name: name.into(),
            api: api.map(Into::into),
            url: url.map(Into::into),
            key: key.map(Into::into),
            models: models.iter().map(|m| m.to_string()).collect(),
            ca: None,
        })
    };
    let l = cli(Action::List).unwrap();
    for n in ["openai", "xai", "openrouter", "moonshot", "anthropic", "google", "gemma-local"] {
        assert!(l.contains(n), "{n}: {l}");
    }
    assert!(l.contains("gemma-4-e2b, gemma-4-e4b") && l.contains("not built yet (P4b)") && l.contains("no default model"), "{l}");
    assert!(add("custom", None, None, None, &[]).is_err(), "custom needs --api/--url");
    assert!(add("x", Some("openai"), Some("http://example.com/v1"), None, &[]).is_err(), "plain http to a remote host");
    assert!(add("gemma-local", None, None, None, &[]).is_err());
    // A preset fills api/url; the key goes to the (file) keychain only.
    let fake = "sk-fake-test-key-0000";
    let o = add("anthropic", None, None, Some(fake), &[]).unwrap();
    assert!(!o.contains(fake), "{o}");
    let prefs = std::fs::read_to_string(t.path().join("preferences.toml")).unwrap();
    assert!(
        prefs.contains("[[models.providers]]")
            && prefs.contains("keychain:model-anthropic")
            && prefs.contains("https://api.anthropic.com")
            && !prefs.contains(fake),
        "{prefs}"
    );
    // A local OpenAI-compatible server (llama-server / Ollama style) against the mock.
    let (url, heads) = mock(2);
    add("local", Some("openai"), Some(&url), None, &["qwen3", "llama3"]).unwrap();
    let o = cli(Action::Test { spec: Some("local".into()), prompt: "hi".into(), tool: false }).unwrap();
    assert!(o.contains("local/qwen3:") && o.contains("Hello from mock") && o.contains("[usage: 3 in, 3 out]") && o.contains("[done: end]"), "{o}");
    assert!(!heads.recv().unwrap().contains("Authorization"));
    let o = cli(Action::Test { spec: Some("local/llama3".into()), prompt: "time?".into(), tool: true }).unwrap();
    assert!(o.contains("[tool call get_time c1] {\"city\":\"Oslo\"}") && o.contains("[done: tool_use]"), "{o}");
    // Defaults.
    assert!(cli(Action::Default("nope".into())).is_err());
    assert!(cli(Action::Default("gemma-local/gemma-9".into())).is_err());
    let o = cli(Action::Default("gemma-local/gemma-4-e4b".into())).unwrap();
    assert!(o.contains("gemma-local/gemma-4-e4b") && o.contains("not built yet"), "{o}");
    let e = cli(Action::Test { spec: None, prompt: "hi".into(), tool: false }).unwrap_err().to_string();
    assert!(e.contains("P4b"), "{e}");
    cli(Action::Default("local/llama3".into())).unwrap();
    assert!(cli(Action::List).unwrap().contains("* local"));
    // Remove drops the key and a default pointing at it.
    cli(Action::Remove("anthropic".into())).unwrap();
    assert!(mouser_core::secrets::get("model-anthropic").is_none());
    cli(Action::Remove("local".into())).unwrap();
    assert!(mouser_core::models::prefs().default.is_none());
    assert!(cli(Action::Remove("local".into())).is_err());
    unsafe {
        std::env::remove_var("MOUSER_SECRET_STORE");
        std::env::remove_var("MOUSER_HOME");
    }
}
