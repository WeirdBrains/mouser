//! `mouser model …` under a temporary MOUSER_HOME, file secret store, mock HTTP only.

use mouser_app::model_cli::{Action, LocalAction, run};
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
    assert!(l.contains("gemma-4-e2b, gemma-4-e4b") && l.contains("not in this build") && l.contains("no default model"), "{l}");
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
    let (url, heads) = mock(3);
    add("local", Some("openai"), Some(&url), None, &["qwen3", "llama3"]).unwrap();
    let o = cli(Action::Test { spec: Some("local".into()), prompt: "hi".into(), tool: false }).unwrap();
    assert!(o.contains("local/qwen3:") && o.contains("Hello from mock") && o.contains("[usage: 3 in, 3 out; ") && o.contains("[done: end]"), "{o}");
    assert!(!heads.recv().unwrap().contains("Authorization"));
    let o = cli(Action::Test { spec: Some("local/llama3".into()), prompt: "time?".into(), tool: true }).unwrap();
    assert!(
        o.contains("[tool call get_time c1] {\"city\":\"Oslo\"}") && o.contains("[done: tool_use]") && o.contains("[tool result: 14:05 (demo)]"),
        "{o}"
    );
    // Defaults.
    assert!(cli(Action::Default("nope".into())).is_err());
    assert!(cli(Action::Default("gemma-local/gemma-9".into())).is_err());
    let o = cli(Action::Default("gemma-local/gemma-4-e4b".into())).unwrap();
    assert!(o.contains("gemma-local/gemma-4-e4b") && o.contains("not downloaded yet (4.59 GB)"), "{o}");
    let e = cli(Action::Test { spec: None, prompt: "hi".into(), tool: false }).unwrap_err().to_string();
    assert!(e.contains("not downloaded yet") && e.contains("mouser model local download gemma-4-e4b"), "{e}");
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

#[test]
#[serial_test::serial]
fn model_local_dir_download_delete_url() {
    let t = tempfile::tempdir().unwrap();
    let big = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("MOUSER_HOME", t.path());
        std::env::set_var("MOUSER_SECRET_STORE", "file");
        std::env::remove_var("MOUSER_MODELS_DIR");
    }
    let l = |a| cli(Action::Local(a));
    let s = l(LocalAction::Status).unwrap();
    assert!(s.contains("not in this build") && s.contains(&t.path().join("models").display().to_string()), "{s}");
    assert!(s.contains("not downloaded (2.84 GB): mouser model local download gemma-4-e2b"), "{s}");

    // The models dir: preference, then env override.
    let o = l(LocalAction::Dir(Some(big.path().display().to_string()))).unwrap();
    assert!(o.contains(&big.path().display().to_string()), "{o}");
    let other = big.path().join("env");
    unsafe { std::env::set_var("MOUSER_MODELS_DIR", &other) };
    assert!(l(LocalAction::Status).unwrap().contains(&other.display().to_string()));
    unsafe { std::env::remove_var("MOUSER_MODELS_DIR") };

    // A short download (a mock serving 10 bytes): kept as .part, reported, deleted.
    let srv = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/w.gguf", srv.local_addr().unwrap().port());
    std::thread::spawn(move || {
        let (mut s, _) = srv.accept().unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        loop {
            let mut h = String::new();
            r.read_line(&mut h).unwrap();
            if h == "\r\n" {
                break;
            }
        }
        write!(s, "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n0123456789").unwrap();
    });
    unsafe { std::env::set_var("MOUSER_MODEL_URL_OVERRIDE", &url) };
    let e = l(LocalAction::Download { id: None, yes: true }).unwrap_err().to_string();
    unsafe { std::env::remove_var("MOUSER_MODEL_URL_OVERRIDE") };
    assert!(e.contains("stopped at 10 bytes of 2.84 GB"), "{e}");
    assert!(big.path().join("gemma-4-E2B-it-Q4_0.gguf.part").exists());
    assert!(l(LocalAction::Status).unwrap().contains("partial 10 bytes of 2.84 GB"));
    assert!(l(LocalAction::Delete("gemma-4-e2b".into())).unwrap().contains("deleted"));
    assert!(!big.path().join("gemma-4-E2B-it-Q4_0.gguf.part").exists());
    assert!(l(LocalAction::Delete("gemma-9".into())).is_err());
    l(LocalAction::Dir(None)).unwrap();

    // A compatible server instead of the built-in engine.
    let (u, _rx) = mock(1);
    let o = l(LocalAction::Url { url: Some(u.clone()), api: None, model: Some("served".into()), key: Some("sk-test-not-real".into()) }).unwrap();
    assert!(o.contains(&u), "{o}");
    assert!(cli(Action::List).unwrap().contains(&format!("via {u}")));
    let o = cli(Action::Test { spec: Some("gemma-local".into()), prompt: "hi".into(), tool: false }).unwrap();
    assert!(o.contains("Hello from mock"), "{o}");
    let p = std::fs::read_to_string(t.path().join("preferences.toml")).unwrap();
    assert!(!p.contains("sk-test-not-real") && p.contains("keychain:model-gemma-local"), "{p}");
    assert!(l(LocalAction::Url { url: Some("ftp://x".into()), api: None, model: None, key: None }).is_err());
    assert!(l(LocalAction::Url { url: Some(u.clone()), api: Some("google".into()), model: None, key: None }).is_err());
    l(LocalAction::Url { url: None, api: None, model: None, key: None }).unwrap();
    assert!(cli(Action::List).unwrap().contains("not in this build"));
}
