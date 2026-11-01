//! Model adapters against local mock HTTP servers (never real APIs or keys).

use mouser_core::models::*;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{Receiver, channel};

const FAKE_KEY: &str = "test-key-not-real-0000";

/// Serve one request: returns (base url, request receiver of (head, body)).
fn mock(status: u16, body: String, chunked: bool) -> (String, Receiver<(String, Value)>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut head = String::new();
        let mut len = 0;
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
        tx.send((head, serde_json::from_slice(&b).unwrap())).unwrap();
        let mut w = s;
        let ct = if status == 200 { "text/event-stream" } else { "application/json" };
        if chunked {
            write!(w, "HTTP/1.1 {status} X\r\nContent-Type: {ct}\r\nTransfer-Encoding: chunked\r\n\r\n").unwrap();
            for piece in body.as_bytes().chunks(7) {
                write!(w, "{:x}\r\n", piece.len()).unwrap();
                w.write_all(piece).unwrap();
                w.write_all(b"\r\n").unwrap();
            }
            w.write_all(b"0\r\n\r\n").unwrap();
        } else {
            write!(w, "HTTP/1.1 {status} X\r\nContent-Type: {ct}\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    (format!("http://127.0.0.1:{port}"), rx)
}

fn sse(events: &[Value]) -> String {
    events.iter().map(|e| format!("data: {e}\n\n")).collect()
}

fn tool_req(model: &str) -> ChatRequest {
    ChatRequest {
        model: model.into(),
        system: Some("be brief".into()),
        messages: vec![
            Message::user("weather?"),
            Message {
                role: Role::Assistant,
                parts: vec![Part::ToolCall { id: "c0".into(), name: "weather".into(), args: json!({"city": "Paris"}) }],
            },
            Message {
                role: Role::User,
                parts: vec![Part::ToolResult { id: "c0".into(), name: "weather".into(), content: "sunny".into(), is_error: false }],
            },
        ],
        tools: vec![ToolSpec {
            name: "weather".into(),
            description: "get weather".into(),
            schema: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        }],
        tool_choice: ToolChoice::Required,
        max_tokens: Some(100),
        temperature: Some(0.5),
    }
}

#[test]
fn openai_streams_text_tool_calls_and_usage() {
    for chunked in [false, true] {
        let body = sse(&[
            json!({"choices": [{"delta": {"content": "Hel"}}]}),
            json!({"choices": [{"delta": {"content": "lo"}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_1", "function": {"name": "weather", "arguments": "{\"ci"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "ty\":\"Oslo\"}"}}]}, "finish_reason": "tool_calls"}]}),
            json!({"choices": [], "usage": {"prompt_tokens": 12, "completion_tokens": 5}}),
        ]) + "data: [DONE]\n\n";
        let (url, rx) = mock(200, body, chunked);
        let p = openai::OpenAi { id: "t".into(), url: format!("{url}/v1"), key: Some(FAKE_KEY.into()), models: vec!["gpt-x".into()], ca: None };
        let mut c = Collect::default();
        p.chat(&tool_req("gpt-x"), &mut c).unwrap();
        assert_eq!(c.text, "Hello");
        assert_eq!(c.calls, vec![("call_1".into(), "weather".into(), "{\"city\":\"Oslo\"}".into())]);
        assert_eq!(c.message().parts[1], Part::ToolCall { id: "call_1".into(), name: "weather".into(), args: json!({"city": "Oslo"}) });
        assert_eq!((c.usage, c.stop.as_deref()), (Some((12, 5)), Some("tool_use")));
        let (head, b) = rx.recv().unwrap();
        assert!(head.starts_with("POST /v1/chat/completions ") && head.contains(&format!("Authorization: Bearer {FAKE_KEY}")), "{head}");
        assert_eq!(b["messages"][0], json!({"role": "system", "content": "be brief"}));
        assert_eq!(b["messages"][2]["tool_calls"][0]["function"]["arguments"], "{\"city\":\"Paris\"}");
        assert_eq!(b["messages"][3], json!({"role": "tool", "tool_call_id": "c0", "content": "sunny"}));
        assert_eq!((b["tool_choice"].clone(), b["temperature"].clone(), b["stream"].clone()), (json!("required"), json!(0.5), json!(true)));
        assert_eq!(b["tools"][0]["function"]["name"], "weather");
    }
}

#[test]
fn kimi_quirks_drop_temperature_and_required() {
    let (url, rx) = mock(200, "data: [DONE]\n\n".into(), false);
    let p = openai::OpenAi { id: "moonshot".into(), url, key: None, models: vec![], ca: None };
    p.chat(&tool_req("kimi-k2.6"), &mut Collect::default()).unwrap();
    let (head, b) = rx.recv().unwrap();
    assert!(!head.contains("Authorization"), "no key, no header");
    assert_eq!(b["tool_choice"], "auto");
    assert!(b.get("temperature").is_none());
}

#[test]
fn errors_carry_the_message_not_the_key() {
    let (url, _rx) = mock(401, json!({"error": {"message": "invalid api key"}}).to_string(), false);
    let p = openai::OpenAi { id: "t".into(), url, key: Some(FAKE_KEY.into()), models: vec![], ca: None };
    let e = format!("{:#}", p.chat(&tool_req("m"), &mut Collect::default()).unwrap_err());
    assert!(e.contains("401") && e.contains("invalid api key") && !e.contains(FAKE_KEY), "{e}");
    // An error inside the stream.
    let (url, _rx) = mock(200, sse(&[json!({"error": {"message": "overloaded"}})]), false);
    let p = openai::OpenAi { id: "t".into(), url, key: None, models: vec![], ca: None };
    assert!(p.chat(&tool_req("m"), &mut Collect::default()).unwrap_err().to_string().contains("overloaded"));
}

#[test]
fn cancel_stops_the_stream() {
    let events: Vec<Value> = (0..50).map(|i| json!({"choices": [{"delta": {"content": format!("w{i} ")}}]})).collect();
    let (url, _rx) = mock(200, sse(&events), false);
    let p = openai::OpenAi { id: "t".into(), url, key: None, models: vec![], ca: None };
    let mut c = Collect { cancel_after: Some(3), ..Default::default() };
    p.chat(&tool_req("m"), &mut c).unwrap();
    assert_eq!(c.text, "w0 w1 w2 ");
    assert!(c.stop.is_none(), "no Done after cancel");
}

#[test]
fn anthropic_messages_stream() {
    let ev = |name: &str, v: Value| format!("event: {name}\ndata: {v}\n\n");
    let body = [
        ev("message_start", json!({"type": "message_start", "message": {"usage": {"input_tokens": 20}}})),
        ev("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})),
        ev("content_block_delta", json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Checking"}})),
        ev(
            "content_block_start",
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "weather"}}),
        ),
        ev(
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"city\":"}}),
        ),
        ev(
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "\"Rome\"}"}}),
        ),
        ev("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 9}})),
        ev("message_stop", json!({"type": "message_stop"})),
    ]
    .concat();
    let (url, rx) = mock(200, body, true);
    let p = anthropic::Anthropic { id: "a".into(), url, key: Some(FAKE_KEY.into()), models: vec![], ca: None };
    let mut c = Collect::default();
    p.chat(&tool_req("claude-x"), &mut c).unwrap();
    assert_eq!(c.text, "Checking");
    assert_eq!(c.calls, vec![("toolu_1".into(), "weather".into(), "{\"city\":\"Rome\"}".into())]);
    assert_eq!((c.usage, c.stop.as_deref()), (Some((20, 9)), Some("tool_use")));
    let (head, b) = rx.recv().unwrap();
    assert!(
        head.starts_with("POST /v1/messages ") && head.contains(&format!("x-api-key: {FAKE_KEY}")) && head.contains("anthropic-version: 2023-06-01"),
        "{head}"
    );
    assert_eq!(b["system"], "be brief");
    assert_eq!(b["messages"][1]["content"][0], json!({"type": "tool_use", "id": "c0", "name": "weather", "input": {"city": "Paris"}}));
    assert_eq!(b["messages"][2]["content"][0]["type"], "tool_result");
    assert_eq!(b["tool_choice"], json!({"type": "any"}));
    assert_eq!(b["tools"][0]["input_schema"]["type"], "object");
    assert_eq!(anthropic::endpoint("https://x.example/v1"), "https://x.example/v1/messages");
    // An `error` event fails the call.
    let (url, _rx) = mock(200, ev("error", json!({"type": "error", "error": {"message": "overloaded_error"}})), false);
    let p = anthropic::Anthropic { id: "a".into(), url, key: None, models: vec![], ca: None };
    assert!(p.chat(&tool_req("m"), &mut Collect::default()).unwrap_err().to_string().contains("overloaded"));
}

#[test]
fn google_native_stream() {
    let body = sse(&[
        json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "Let me "}]}}]}),
        json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "check."}, {"functionCall": {"name": "weather", "args": {"city": "Lima"}}}]}, "finishReason": "STOP"}], "usageMetadata": {"promptTokenCount": 7, "candidatesTokenCount": 4}}),
    ]);
    let (url, rx) = mock(200, body, false);
    let p = google::Google { id: "g".into(), url, key: Some(FAKE_KEY.into()), models: vec![], ca: None };
    let mut c = Collect::default();
    p.chat(&tool_req("gemini-x"), &mut c).unwrap();
    assert_eq!(c.text, "Let me check.");
    assert_eq!(c.calls, vec![("call_0".into(), "weather".into(), "{\"city\":\"Lima\"}".into())]);
    assert_eq!((c.usage, c.stop.as_deref()), (Some((7, 4)), Some("tool_use")));
    let (head, b) = rx.recv().unwrap();
    assert!(
        head.starts_with("POST /v1beta/models/gemini-x:streamGenerateContent?alt=sse ") && head.contains(&format!("x-goog-api-key: {FAKE_KEY}")),
        "{head}"
    );
    assert_eq!(b["systemInstruction"]["parts"][0]["text"], "be brief");
    assert_eq!(b["contents"][1]["role"], "model");
    assert_eq!(b["contents"][2]["parts"][0]["functionResponse"], json!({"name": "weather", "response": {"content": "sunny"}}));
    assert_eq!(b["toolConfig"]["functionCallingConfig"]["mode"], "ANY");
    assert_eq!(b["generationConfig"], json!({"maxOutputTokens": 100, "temperature": 0.5}));
}

#[test]
#[serial_test::serial]
fn registry_presets_stub_and_default() {
    let t = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("MOUSER_HOME", t.path()) };
    let all = all();
    for n in ["openai", "xai", "openrouter", "moonshot", "anthropic", "google", "gemma-local"] {
        assert!(all.iter().any(|p| p.entry.name == n), "{n}");
    }
    let g = find("gemma-local").unwrap();
    assert_eq!(g.entry.models, vec!["gemma-4-e2b", "gemma-4-e4b"]);
    assert_eq!(g.origin, Origin::Local);
    let e = open("gemma-local")
        .unwrap()
        .chat(&ChatRequest { model: "gemma-4-e2b".into(), ..Default::default() }, &mut Collect::default())
        .unwrap_err()
        .to_string();
    assert!(e.contains("not downloaded") && e.contains("2.84 GB") && e.contains("mouser model local download gemma-4-e2b"), "{e}");
    assert!(open("anthropic").err().unwrap().to_string().contains("--key-stdin"));
    assert!(default_choice().is_none());
    let mut m = ModelsPrefs::default();
    m.providers.push(ProviderEntry {
        name: "local".into(),
        api: Api::OpenAi,
        url: "http://127.0.0.1:8080/v1".into(),
        key: None,
        models: vec!["qwen".into()],
        ca: None,
    });
    save(&m).unwrap();
    assert_eq!(default_choice(), Some(("local".into(), "qwen".into())));
    m.default = Some("gemma-local/gemma-4-e4b".into());
    save(&m).unwrap();
    assert_eq!(default_choice(), Some(("gemma-local".into(), "gemma-4-e4b".into())));
    assert_eq!(open("local").unwrap().id(), "local");
    m.providers[0].url = "http://example.com/v1".into();
    save(&m).unwrap();
    assert!(open("local").is_err(), "plain http only to loopback");
    unsafe { std::env::remove_var("MOUSER_HOME") };
}

#[test]
#[serial_test::serial]
fn a_model_provider_plugin_is_a_provider() {
    let t = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("MOUSER_HOME", t.path()) };
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/ollama-model");
    let i = mouser_core::plugins::install(&src).unwrap();
    assert_eq!(i.manifest.run.transport, mouser_core::plugins::Transport::Http);
    let p = find("ollama-model").unwrap();
    assert_eq!((p.origin.clone(), p.entry.api, p.default_model()), (Origin::Plugin, Api::OpenAi, Some("llama3.2")));
    assert_eq!(open("ollama-model").unwrap().models()[1].name, "qwen3");
    // http transport needs a [model] table.
    let bad = "name = \"m\"\nversion = \"1\"\n[[provides]]\nkind = \"model-provider\"\n[run]\ntransport = \"http\"\n";
    assert!(mouser_core::plugins::Manifest::parse(bad).is_err());
    // The registry lists the built-in model providers as plugins.
    let r = mouser_core::plugins::Registry::load();
    let names: Vec<&str> = r.providers(mouser_core::plugins::Kind::ModelProvider).iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"anthropic") && names.contains(&"gemma-local") && names.contains(&"ollama-model"), "{names:?}");
    unsafe { std::env::remove_var("MOUSER_HOME") };
}

mod local_model {
    use super::*;
    use mouser_core::models::local::*;

    fn fake(size: u64, sha: &'static str) -> Weights {
        Weights { id: "gemma-4-e2b", label: "test weights", repo: "test/repo", file: "test.gguf", size, sha256: sha }
    }

    /// Serves `data` (honouring Range) after one redirect; returns the URL.
    fn file_server(data: Vec<u8>, conns: usize) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..conns {
                let (s, _) = l.accept().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                let mut range = None;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h == "\r\n" {
                        break;
                    }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = v.trim().trim_end_matches('-').parse::<usize>().ok();
                    }
                }
                // Write errors are expected: a cancelled download hangs up mid-body.
                let mut w = s;
                let _ = if first.contains("/start") {
                    write!(w, "HTTP/1.1 302 Found\r\nLocation: /file.gguf\r\nContent-Length: 0\r\n\r\n")
                } else if let Some(from) = range {
                    write!(w, "HTTP/1.1 206 Partial\r\nContent-Length: {}\r\n\r\n", data.len() - from).and_then(|_| w.write_all(&data[from..]))
                } else {
                    write!(w, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", data.len()).and_then(|_| w.write_all(&data))
                };
            }
        });
        format!("http://127.0.0.1:{port}/start")
    }

    fn sha(d: &[u8]) -> &'static str {
        use sha2::Digest;
        Box::leak(sha2::Sha256::digest(d).iter().map(|b| format!("{b:02x}")).collect::<String>().into_boxed_str())
    }

    #[test]
    #[serial_test::serial]
    fn download_needs_consent_follows_redirects_resumes_and_verifies() {
        let t = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("MOUSER_HOME", t.path()) };
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        let w = fake(data.len() as u64, sha(&data));
        assert!(!w.installed());
        assert!(w.consent_text()[0].contains("3.0 MB") && w.consent_text()[2].contains("test.gguf"));
        // Cancel part-way (the .part stays), then resume with Range.
        let url = file_server(data.clone(), 4);
        let mut seen = 0;
        let e = download(Consent::given(&w), &url, &mut |d, _| {
            seen = d;
            d < 1_000_000
        })
        .unwrap_err();
        assert!(e.to_string().contains("cancelled"), "{e}");
        let part = w.path().with_extension("gguf.part");
        assert!(std::fs::metadata(&part).unwrap().len() >= 1_000_000);
        let mut last = (0, 0);
        let p = download(Consent::given(&w), &url, &mut |d, n| {
            last = (d, n);
            true
        })
        .unwrap();
        assert_eq!(last, (data.len() as u64, data.len() as u64));
        assert_eq!(std::fs::read(&p).unwrap(), data);
        assert!(w.installed() && !part.exists());
        assert!(remove(&w) && !w.installed());
        // A wrong checksum deletes the file.
        let bad = fake(data.len() as u64, "00");
        let e = download(Consent::given(&bad), &file_server(data.clone(), 2), &mut |_, _| true).unwrap_err();
        assert!(e.to_string().contains("checksum mismatch") && !bad.path().exists() && !part.exists(), "{e}");
        unsafe { std::env::remove_var("MOUSER_HOME") };
    }

    #[test]
    fn catalog_is_gemma_e2b_default_and_e4b() {
        let (d, all) = gemma_models();
        assert_eq!((d.as_str(), all), ("gemma-4-e2b", vec!["gemma-4-e2b".to_string(), "gemma-4-e4b".to_string()]));
        let e2b = weights("gemma-4-e2b").unwrap();
        assert_eq!((human(e2b.size), e2b.sha256.len()), ("2.84 GB".to_string(), 64));
        assert_eq!(e2b.url(), "https://huggingface.co/ggml-org/gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-Q4_0.gguf");
        assert_eq!(human(weights("gemma-4-e4b").unwrap().size), "4.59 GB");
    }

    #[test]
    fn prompts_render_with_the_model_template() {
        let req = ChatRequest {
            model: "gemma-4-e2b".into(),
            system: Some("be brief".into()),
            messages: vec![
                Message::user("time in Oslo?"),
                Message {
                    role: Role::Assistant,
                    parts: vec![Part::ToolCall { id: "c".into(), name: "get_time".into(), args: json!({"city": "Oslo"}) }],
                },
                Message {
                    role: Role::User,
                    parts: vec![Part::ToolResult { id: "c".into(), name: "get_time".into(), content: "12:00".into(), is_error: false }],
                },
            ],
            tools: vec![ToolSpec { name: "get_time".into(), description: "time".into(), schema: json!({"type": "object"}) }],
            tool_choice: ToolChoice::Required,
            ..Default::default()
        };
        let t = turns(&req);
        assert_eq!(t[0].0, "system");
        assert!(t[0].1.contains("be brief") && t[0].1.contains("<tool_call>") && t[0].1.contains("must call a tool"), "{}", t[0].1);
        assert_eq!(t[2].1, "<tool_call>{\"arguments\":{\"city\":\"Oslo\"},\"name\":\"get_time\"}</tool_call>");
        assert!(t[3].1.starts_with("<tool_result name=\"get_time\">12:00"));
        // A Gemma-style Jinja template (system folded into the first user turn).
        let tmpl = "{{ bos_token }}{% if messages[0]['role'] == 'system' %}{% set sys = messages[0]['content'] + '\n\n' %}{% set msgs = messages[1:] %}{% else %}{% set sys = '' %}{% set msgs = messages %}{% endif %}{% for m in msgs %}{% set role = 'model' if m['role'] == 'assistant' else m['role'] %}<start_of_turn>{{ role }}\n{% if loop.first %}{{ sys }}{% endif %}{{ m['content'] | trim }}<end_of_turn>\n{% endfor %}{% if add_generation_prompt %}<start_of_turn>model\n{% endif %}";
        let p = render(tmpl, &t, "<bos>").unwrap();
        assert!(
            p.starts_with("<bos><start_of_turn>user\nbe brief")
                && p.ends_with("<start_of_turn>model\n")
                && p.contains("<start_of_turn>model\n<tool_call>"),
            "{p}"
        );
        assert!(render("{{ raise_exception('no') }}", &t, "").is_err());
        let f = render_fallback(&t);
        assert!(f.starts_with("<start_of_turn>user\nbe brief") && f.ends_with("<start_of_turn>model\n"), "{f}");
    }

    #[test]
    fn tool_calls_are_lifted_from_streamed_text() {
        let mut c = Collect::default();
        let mut p = ToolParser::default();
        for piece in ["Let me check", ". <tool", "_call>{\"name\": \"get_", "time\", \"arguments\": {\"city\": \"Oslo\"}}</tool_", "call> done"] {
            p.feed(piece, &mut c);
        }
        p.finish(&mut c);
        assert_eq!(c.text, "Let me check.  done");
        assert_eq!(c.calls, vec![("call_0".into(), "get_time".into(), "{\"city\":\"Oslo\"}".into())]);
        // Text that only looks like the start of a tag is passed through.
        let mut c = Collect::default();
        let mut p = ToolParser::default();
        p.feed("a <to", &mut c);
        p.feed("p> b <", &mut c);
        p.finish(&mut c);
        assert_eq!((c.text.as_str(), c.calls.len()), ("a <top> b <", 0));
    }

    #[test]
    #[serial_test::serial]
    fn local_url_uses_a_compatible_server_instead() {
        let t = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("MOUSER_HOME", t.path()) };
        let (url, rx) = mock(200, "data: {\"choices\":[{\"delta\":{\"content\":\"from llama-server\"}}]}\n\ndata: [DONE]\n\n".into(), false);
        let mut m = mouser_core::models::prefs();
        m.local = Some(LocalPrefs { url: Some(format!("{url}/v1")), model: Some("served-name".into()), ..Default::default() });
        save(&m).unwrap();
        let mut c = Collect::default();
        open("gemma-local")
            .unwrap()
            .chat(&ChatRequest { model: "gemma-4-e4b".into(), messages: vec![Message::user("hi")], ..Default::default() }, &mut c)
            .unwrap();
        assert_eq!(c.text, "from llama-server");
        let (head, b) = rx.recv().unwrap();
        assert!(head.starts_with("POST /v1/chat/completions "));
        assert_eq!(b["model"], "served-name");
        unsafe { std::env::remove_var("MOUSER_HOME") };
    }
}
