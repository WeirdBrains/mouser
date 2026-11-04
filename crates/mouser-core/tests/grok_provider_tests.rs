//! The `grok` provider: key shapes and hints, redaction, the Cursor agents API
//! (grok.com keys) and the Grok Bot desktop login — mock servers and fake keys only.

use mouser_core::models::{ChatRequest, Collect, Message, ModelProvider, cursor::CursorAgents, keys};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

const FAKE_GROK: &str = "grok_0123456789abcdef0123456789abcdef_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const FAKE_XAI: &str = "xai-FAKEfakeFAKEfake0000";

/// (method, path, auth header, body) of each request.
type Log = Arc<Mutex<Vec<(String, String, String, String)>>>;

/// A tiny HTTP mock: `route(method, path, body, nth call of that path)` -> (status, content type, body).
fn serve(route: impl Fn(&str, &str, &str, usize) -> (u16, &'static str, String) + Send + 'static) -> (String, Log) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let log: Log = Default::default();
    let lg = log.clone();
    std::thread::spawn(move || {
        let mut counts = std::collections::HashMap::<String, usize>::new();
        for s in l.incoming() {
            let Ok(s) = s else { break };
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut first = String::new();
            if r.read_line(&mut first).is_err() {
                continue;
            }
            let mut parts = first.split_whitespace();
            let (m, p) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
            let (mut len, mut auth) = (0, String::new());
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h == "\r\n" || h.is_empty() {
                    break;
                }
                let lower = h.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                if lower.starts_with("authorization:") {
                    auth = h[14..].trim().to_string();
                }
            }
            let mut b = vec![0; len];
            r.read_exact(&mut b).unwrap();
            let body = String::from_utf8(b).unwrap();
            let n = counts.entry(format!("{m} {p}")).or_default();
            let (st, ct, reply) = route(&m, &p, &body, *n);
            *n += 1;
            lg.lock().unwrap().push((m, p, auth, body));
            let mut w = s;
            let _ = write!(w, "HTTP/1.1 {st} X\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
        }
    });
    (url, log)
}

const JSON: &str = "application/json";

#[test]
fn key_shapes_and_wrong_provider_hints() {
    assert_eq!(keys::kind(FAKE_GROK), keys::KeyKind::GrokCom);
    assert!(keys::is_grok_com(FAKE_GROK) && !keys::is_grok_com("grok_short"));
    assert_eq!(keys::kind(FAKE_XAI), keys::KeyKind::Xai);
    assert_eq!(keys::kind("sk-ant-x"), keys::KeyKind::Anthropic);
    // A grok.com key sent to xAI: point at the grok provider, never echo the key.
    let h = keys::wrong_provider_hint("xai", "https://api.x.ai/v1", FAKE_GROK).unwrap();
    assert!(h.contains("mouser model add grok") && h.contains("console.x.ai") && !h.contains(FAKE_GROK), "{h}");
    let h = keys::wrong_provider_hint("openai", "https://api.openai.com/v1", FAKE_GROK).unwrap();
    assert!(h.contains("grok"), "{h}");
    // An xAI key sent to the agents API or OpenAI.
    assert!(keys::wrong_provider_hint("gk", "https://api.cursor.com", FAKE_XAI).unwrap().contains("mouser model add xai"));
    assert!(keys::wrong_provider_hint("openai", "https://api.openai.com/v1", FAKE_XAI).is_some());
    // Fits: no hint. `grok` routes both shapes itself.
    assert!(keys::wrong_provider_hint("xai", "https://api.x.ai/v1", FAKE_XAI).is_none());
    assert!(keys::wrong_provider_hint("grok", "https://api.cursor.com", FAKE_GROK).is_none());
    assert!(keys::wrong_provider_hint("openai", "https://api.openai.com/v1", "sk-proj-abc123456789").is_none());
    assert_eq!(mouser_core::models::grok_target(FAKE_XAI).1, "https://api.x.ai/v1");
    assert_eq!(mouser_core::models::grok_target(FAKE_GROK).1, "https://api.cursor.com");
}

#[test]
fn redaction() {
    let t = format!("Incorrect API key provided: {FAKE_XAI}; Authorization: Bearer {FAKE_GROK}, sk-proj-abcdefgh1234 url?key=AIzaSecret123&x=1");
    let r = keys::redact(&t, &[]);
    for s in [FAKE_XAI, FAKE_GROK, "sk-proj-abcdefgh1234", "AIzaSecret123"] {
        assert!(!r.contains(s), "{r}");
    }
    assert!(r.contains("Bearer <redacted>") && r.contains("key=<redacted>&x=1"), "{r}");
    assert_eq!(keys::redact("token opaque-secret-value here", &["opaque-secret-value"]), "token <redacted> here");
    let e = mouser_core::models::http::HttpError { status: 401, body: format!("{{\"error\":{{\"message\":\"bad key {FAKE_GROK}\"}}}}") };
    assert!(!e.to_string().contains(FAKE_GROK), "{e}");
}

fn agents(url: &str) -> CursorAgents {
    CursorAgents { id: "gk".into(), url: url.into(), key: Some(FAKE_GROK.into()), models: vec!["grok-4.7".into()], ca: None }
}

#[test]
#[serial_test::serial]
fn cursor_agents_chat_follow_up_busy_and_gone_stream() {
    unsafe { std::env::set_var("MOUSER_GROK_POLL_MS", "5") };
    let (url, log) = serve(|m, p, _body, n| {
        match (m, p) {
        ("GET", "/v1/me") => (200, JSON, r#"{"apiKeyName":"mouser-test"}"#.into()),
        ("GET", "/v1/models") => (200, JSON, r#"{"items":[{"id":"grok-4.7","displayName":"Grok 4.7"},{"id":"grok-4.6"}]}"#.into()),
        ("POST", "/v1/agents") => (200, JSON, r#"{"agent":{"id":"bc-1","latestRunId":"run-1"},"run":{"id":"run-1","agentId":"bc-1","status":"CREATING"}}"#.into()),
        ("GET", "/v1/agents/bc-1/runs/run-1/stream") => (
            200,
            "text/event-stream",
            "event: status\ndata: {\"status\":\"RUNNING\"}\n\nevent: thinking\ndata: {\"text\":\"hmm\"}\n\nevent: assistant\ndata: {\"text\":\"Hello \"}\n\nevent: assistant\ndata: {\"text\":\"Michael\"}\n\nevent: result\ndata: {\"runId\":\"run-1\",\"status\":\"FINISHED\"}\n\nevent: done\ndata: {}\n\n".into(),
        ),
        // The first follow-up finds the agent busy.
        ("POST", "/v1/agents/bc-1/runs") if n == 0 => (409, JSON, r#"{"error":{"code":"agent_busy","message":"busy"}}"#.into()),
        ("POST", "/v1/agents/bc-1/runs") => (200, JSON, r#"{"run":{"id":"run-2","agentId":"bc-1"}}"#.into()),
        ("GET", "/v1/agents/bc-1/runs/run-2/stream") => (410, JSON, r#"{"error":{"code":"stream_expired"}}"#.into()),
        ("GET", "/v1/agents/bc-1/runs/run-2") if n == 0 => (200, JSON, r#"{"id":"run-2","status":"RUNNING"}"#.into()),
        ("GET", "/v1/agents/bc-1/runs/run-2") => (200, JSON, r#"{"id":"run-2","status":"FINISHED","result":{"text":"Second answer"}}"#.into()),
        _ => (404, JSON, "{}".into()),
    }
    });
    let p = agents(&url);
    assert_eq!(p.me().unwrap(), "mouser-test");
    assert_eq!(p.list().unwrap(), ["grok-4.7", "grok-4.6"]);
    let mut req = ChatRequest { model: "grok-4.7".into(), messages: vec![Message::user("hi")], ..Default::default() };
    let mut c = Collect::default();
    p.chat(&req, &mut c).unwrap();
    assert_eq!(c.text, "Hello Michael");
    assert_eq!(c.stop.as_deref(), Some("end"));
    // Same conversation, next turn: a follow-up run on the same agent.
    req.messages.push(c.message());
    req.messages.push(Message::user("again"));
    let mut c = Collect::default();
    p.chat(&req, &mut c).unwrap();
    assert_eq!(c.text, "Second answer");
    let log = log.lock().unwrap();
    let create = log.iter().find(|r| r.1 == "/v1/agents").unwrap();
    let v: serde_json::Value = serde_json::from_str(&create.3).unwrap();
    assert_eq!(v["prompt"]["text"], "hi");
    assert_eq!(v["model"]["id"], "grok-4.7");
    assert!(log.iter().all(|r| r.2 == format!("Bearer {FAKE_GROK}")), "every call carries the key");
    let follow: Vec<_> = log.iter().filter(|r| r.1 == "/v1/agents/bc-1/runs").collect();
    assert_eq!(follow.len(), 2, "busy, then accepted");
    assert!(follow[1].3.contains("\"again\""));
    assert_eq!(log.iter().filter(|r| r.1 == "/v1/agents").count(), 1, "one agent for the conversation");
}

#[test]
fn cursor_agents_errors_are_redacted_and_hinted() {
    let (url, _) = serve(|_, _, _, _| (401, JSON, format!(r#"{{"error":{{"message":"Invalid key {FAKE_GROK}"}}}}"#)));
    let e = format!("{:#}", agents(&url).list().unwrap_err());
    assert!(!e.contains(FAKE_GROK) && e.contains("401") && e.contains("grok.com/dashboard/api-keys"), "{e}");
    let (url, _) = serve(|_, _, _, _| (200, "text/event-stream", "event: error\ndata: {\"code\":\"bad\",\"message\":\"nope\"}\n\n".into()));
    let mut c = Collect::default();
    // POST /v1/agents gets the SSE text: not JSON → a clear error, still no key.
    let e = format!("{:#}", agents(&url).chat(&ChatRequest { messages: vec![Message::user("x")], ..Default::default() }, &mut c).unwrap_err());
    assert!(!e.contains(FAKE_GROK), "{e}");
}

#[test]
#[serial_test::serial]
fn list_models_for_openai_compatible_redacts() {
    let (url, log) = serve(|_, p, _, _| match p {
        "/v1/models" => (200, JSON, r#"{"data":[{"id":"grok-4.7"},{"id":"grok-4.5"},{"id":"grok-4.6"}]}"#.into()),
        _ => (400, JSON, format!(r#"{{"error":"Incorrect API key provided: {FAKE_XAI}"}}"#)),
    });
    let e = mouser_core::models::ProviderEntry {
        name: "xai".into(),
        api: mouser_core::models::Api::OpenAi,
        url: format!("{url}/v1"),
        key: None,
        models: vec![],
        ca: None,
    };
    let l = mouser_core::models::list_models(&e, Some(FAKE_XAI)).unwrap();
    assert_eq!(l.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), ["grok-4.5", "grok-4.6", "grok-4.7"]);
    assert_eq!(log.lock().unwrap()[0].2, format!("Bearer {FAKE_XAI}"));
    let bad = mouser_core::models::ProviderEntry { url: format!("{url}/bad"), ..e };
    let err = format!("{:#}", mouser_core::models::list_models(&bad, Some(FAKE_XAI)).unwrap_err());
    assert!(err.contains("400") && !err.contains(FAKE_XAI), "{err}");
}

/// Write a fake desktop session (Linux v10 "basic text" Safe Storage) for `gateway` under `home`.
#[cfg(target_os = "linux")]
pub fn fake_desktop(home: &std::path::Path, gateway: &str) {
    use aes::Aes128;
    use base64::Engine;
    use cbc::cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2::<hmac::Hmac<sha1::Sha1>>(b"peanuts", b"saltysalt", 1, &mut key).unwrap();
    let clear = serde_json::json!({"baseUrl": gateway, "token": "fake-gateway-token-1234", "headers": {"x-anyrun-network-token": "fake-net-token"}})
        .to_string();
    let ct = cbc::Encryptor::<Aes128>::new(&key.into(), &[0x20u8; 16].into()).encrypt_padded_vec::<Pkcs7>(clear.as_bytes());
    let mut payload = b"v10".to_vec();
    payload.extend(ct);
    let dir = home.join(".config/Grok Bot");
    std::fs::create_dir_all(&dir).unwrap();
    let enc = base64::engine::general_purpose::STANDARD.encode(payload);
    std::fs::write(dir.join("gateway-descriptor.json"), serde_json::json!({"version": 1, "encrypted": enc}).to_string()).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
#[serial_test::serial]
fn grokbot_desktop_agents_and_chat_with_reread_on_417() {
    use mouser_core::models::grokbot;
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("MOUSER_GROKBOT_HOME", home.path());
        std::env::set_var("GROKBOT_ALLOW_LOCAL_GATEWAY", "1");
        std::env::set_var("MOUSER_GROK_POLL_MS", "10");
    }
    assert!(!grokbot::present());
    let tail = Arc::new(Mutex::new(0usize));
    let t2 = tail.clone();
    let (url, log) = serve(move |_, p, _, n| {
        match p {
        // An expired session: 417 once, then fine after re-reading the files.
        "/api/listAgents" if n == 0 => (417, JSON, String::new()),
        "/api/listAgents" => (
            200,
            JSON,
            r#"{"agents":[{"id":"helper","name":"Helper"},{"id":"team","name":"Team","memberAgentIds":["helper"]},{"id":"quiet","name":"Quiet","hiddenFromSidebar":true}]}"#.into(),
        ),
        "/api/sendPrompt" => (200, JSON, r#"{"accepted":true}"#.into()),
        "/api/getAgentTranscriptTail" => {
            let mut k = t2.lock().unwrap();
            *k += 1;
            let mut e = vec![r#"{"id":"e1","kind":"user-prompt","prompt":"old"}"#.to_string()];
            if *k > 1 {
                e.push(r#"{"id":"e2","kind":"user-prompt","prompt":"What is 2 + 2?"}"#.into());
                e.push(r#"{"id":"e3","kind":"send-message","message":{"type":"text","content":"4"}}"#.into());
            }
            if *k > 2 {
                e.push(r#"{"id":"e4","kind":"tool-call","text":"ran ls"}"#.into());
                e.push(r#"{"id":"e5","kind":"send-message","message":{"type":"local-tool-permission","ask":{}}}"#.into());
            }
            (200, JSON, format!(r#"{{"entries":[{}]}}"#, e.join(",")))
        }
        _ => (404, JSON, "{}".into()),
    }
    });
    fake_desktop(home.path(), &url);
    assert!(grokbot::present());
    let a = grokbot::agents().unwrap();
    assert_eq!(a, [("helper".to_string(), "Helper (helper)".to_string())], "groups and hidden bots are skipped");
    let p = grokbot::GrokDesktop { id: "grok".into(), models: vec!["helper".into()] };
    let mut c = Collect::default();
    p.chat(&ChatRequest { messages: vec![Message::user("What is 2 + 2?")], ..Default::default() }, &mut c).unwrap();
    assert_eq!(c.text, "4", "bot text after the prompt; tool events and cards skipped");
    let log = log.lock().unwrap();
    let send = log.iter().find(|r| r.1 == "/api/sendPrompt").unwrap();
    let v: serde_json::Value = serde_json::from_str(&send.3).unwrap();
    assert_eq!((v["agentId"].as_str(), v["prompt"].as_str()), (Some("helper"), Some("What is 2 + 2?")));
    assert!(log.iter().all(|r| r.2 == "Bearer fake-gateway-token-1234"));
    unsafe {
        std::env::remove_var("MOUSER_GROKBOT_HOME");
        std::env::remove_var("GROKBOT_ALLOW_LOCAL_GATEWAY");
    }
}

#[cfg(target_os = "linux")]
#[test]
#[serial_test::serial]
fn grokbot_desktop_refuses_non_cursor_gateways() {
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("MOUSER_GROKBOT_HOME", home.path());
        std::env::remove_var("GROKBOT_ALLOW_LOCAL_GATEWAY");
    }
    fake_desktop(home.path(), "https://evil.example.com");
    let e = format!("{:#}", mouser_core::models::grokbot::agents().unwrap_err());
    assert!(e.to_lowercase().contains("evil.example.com") && !e.contains("fake-gateway-token"), "{e}");
    unsafe { std::env::remove_var("MOUSER_GROKBOT_HOME") };
}
