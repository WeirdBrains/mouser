//! The first-run model picker (models::setup) against mock servers: fake
//! weights, a fake key, a file secret store; never a real download or API.
use mouser_core::models::local::{self, Weights};
use mouser_core::models::setup::*;
use mouser_core::models::{self, Api};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

const FAKE_KEY: &str = "test-key-not-real-0000";

struct Env(tempfile::TempDir);
impl Env {
    fn new() -> Self {
        let t = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("MOUSER_HOME", t.path());
            std::env::set_var("MOUSER_SECRET_STORE", "file");
            std::env::remove_var("MOUSER_MODELS_DIR");
        }
        Env(t)
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("MOUSER_HOME");
            std::env::remove_var("MOUSER_SECRET_STORE");
        }
    }
}

/// An OpenAI-compatible server answering `n` requests with `status`; returns
/// (url, the Authorization headers it saw).
fn chat_server(status: u16, n: usize) -> (String, std::sync::mpsc::Receiver<String>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/v1", l.local_addr().unwrap().port());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..n {
            let (s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let (mut len, mut auth) = (0, String::new());
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h == "\r\n" {
                    break;
                }
                let lo = h.to_ascii_lowercase();
                if let Some(v) = lo.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                if lo.starts_with("authorization:") {
                    auth = h.trim().to_string();
                }
            }
            let mut b = vec![0; len];
            r.read_exact(&mut b).unwrap();
            let _ = tx.send(auth);
            let mut w = s;
            if status == 200 {
                let body = "data: {\"choices\":[{\"delta\":{\"content\":\"OK\"}}]}\n\ndata: [DONE]\n\n";
                let _ = write!(w, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            } else {
                let body = "{\"error\":{\"message\":\"bad key\"}}";
                let _ = write!(w, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            }
        }
    });
    (url, rx)
}

fn file_server(data: Vec<u8>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        while let Ok((s, _)) = l.accept() {
            let mut r = BufReader::new(s.try_clone().unwrap());
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                    break;
                }
            }
            let mut w = s;
            let _ = write!(w, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", data.len()).and_then(|_| {
                for c in data.chunks(64 * 1024) {
                    w.write_all(c)?;
                    std::thread::sleep(Duration::from_millis(2));
                }
                Ok(())
            });
        }
    });
    format!("http://127.0.0.1:{port}/file.gguf")
}

fn wait(s: &mut Setup, what: &str, f: impl Fn(&Step) -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(20) {
        s.poll();
        if f(&s.step) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}: {:?}", s.step);
}

fn typed(s: &mut Setup, t: &str) {
    assert!(s.key(t.as_bytes()));
}

#[test]
#[serial_test::serial]
fn options_list_local_then_hosted_and_say_when_local_is_missing() {
    let _e = Env::new();
    let o = options(true);
    let labels: Vec<_> = o.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels[0], "Gemma 4 E2B local (default)");
    assert!(labels[1].starts_with("Gemma 4 E4B local"));
    for want in ["OpenAI", "xAI (Grok)", "OpenRouter", "Kimi (Moonshot)", "Anthropic (Claude)", "Google (Gemini)", "Custom URL"] {
        assert!(labels.contains(&want), "{want} in {labels:?}");
    }
    assert!(o[0].detail.contains("2.84 GB") && o[1].detail.contains("4.59 GB"));
    // Without local-llm the local rows say so and lead back to the list.
    let mut s = Setup::new(false);
    assert!(!s.opts[0].available && s.opts[0].detail.contains("no local-llm"));
    s.enter();
    assert!(matches!(&s.step, Step::Failed(m, _) if m.contains("--no-default-features") && m.contains("hosted")));
    s.enter();
    assert_eq!(s.step, Step::Choose);
    assert_eq!(usable_default(), None, "nothing configured");
}

#[test]
#[serial_test::serial]
fn local_choice_asks_consent_downloads_with_progress_verifies_and_sets_default() {
    let e = Env::new();
    unsafe { std::env::set_var("MOUSER_MODELS_DIR", e.0.path().join("big")) };
    let data: Vec<u8> = (0..2_000_000u32).map(|i| (i % 253) as u8).collect();
    let sha: String = {
        use sha2::Digest;
        sha2::Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect()
    };
    let w = Weights {
        id: "gemma-4-e2b",
        label: "test weights",
        repo: "t/r",
        file: "e2b-test.gguf",
        size: data.len() as u64,
        sha256: Box::leak(sha.into_boxed_str()),
    };
    let mut s = Setup::new(true);
    s.opts[0].kind = Kind::Local(w.clone());
    s.download_url = Some(file_server(data.clone()));
    s.enter();
    let Step::Consent(c) = &s.step else { panic!("consent first: {:?}", s.step) };
    // The consent shows the size and the location (MOUSER_MODELS_DIR).
    let text = c.consent_text().join("\n");
    assert!(text.contains("2.0 MB") && text.contains("big"), "{text}");
    assert!(!w.path().exists(), "nothing downloaded before the OK");
    // Esc at the consent goes back; then OK.
    assert!(s.back());
    assert_eq!(s.step, Step::Choose);
    s.enter();
    s.enter();
    assert!(matches!(s.step, Step::Downloading(_)));
    wait(&mut s, "progress", |_| true);
    wait(&mut s, "done", |st| matches!(st, Step::Done(..)));
    assert_eq!(s.step, Step::Done("gemma-local/gemma-4-e2b".into(), None));
    assert_eq!(std::fs::read(w.path()).unwrap(), data);
    assert_eq!(s.progress(), (data.len() as u64, data.len() as u64));
    assert_eq!(models::prefs().default.as_deref(), Some("gemma-local/gemma-4-e2b"));
    unsafe { std::env::remove_var("MOUSER_MODELS_DIR") };
}

#[test]
#[serial_test::serial]
fn a_download_can_be_cancelled_and_a_bad_checksum_fails() {
    let e = Env::new();
    unsafe { std::env::set_var("MOUSER_MODELS_DIR", e.0.path()) };
    let data: Vec<u8> = vec![7; 8_000_000];
    let w = Weights { id: "gemma-4-e2b", label: "t", repo: "t/r", file: "c.gguf", size: data.len() as u64, sha256: "00" };
    let mut s = Setup::new(true);
    s.opts[0].kind = Kind::Local(w.clone());
    s.download_url = Some(file_server(data));
    s.enter();
    s.enter();
    wait(&mut s, "some progress", |_| s_progress_hack());
    assert!(s.back(), "Esc cancels the download");
    wait(&mut s, "cancelled", |st| matches!(st, Step::Failed(m, _) if m.contains("cancelled")));
    assert!(!w.path().exists());
    assert_eq!(models::prefs().default, None);
    // Run it to the end: the checksum is wrong, nothing is installed.
    s.enter();
    assert!(matches!(s.step, Step::Consent(_)));
    s.enter();
    wait(&mut s, "checksum", |st| matches!(st, Step::Failed(m, _) if m.contains("checksum")));
    assert!(!w.path().exists());
    unsafe { std::env::remove_var("MOUSER_MODELS_DIR") };
}

fn s_progress_hack() -> bool {
    std::thread::sleep(Duration::from_millis(50));
    true
}

#[test]
#[serial_test::serial]
fn hosted_choice_tests_the_key_then_saves_it_masked_and_sets_default() {
    let _e = Env::new();
    let (url, seen) = chat_server(401, 1);
    let mut s = Setup::new(false);
    s.preset_url = Some(url);
    let i = s.opts.iter().position(|o| o.kind == Kind::Preset("xai")).unwrap();
    s.pick(i);
    assert!(matches!(&s.step, Step::Form(Kind::Preset("xai"), _)));
    // No key: asked for one.
    s.enter();
    assert!(matches!(&s.step, Step::Failed(m, _) if m.contains("API key")));
    s.enter();
    typed(&mut s, FAKE_KEY);
    let Step::Form(_, f) = &s.step else { panic!() };
    assert_eq!(masked(&f.key), "•".repeat(FAKE_KEY.len()));
    // A rejected key: the error, nothing saved, back to the form with the values.
    s.enter();
    wait(&mut s, "failed test", |st| matches!(st, Step::Failed(..)));
    assert!(seen.recv().unwrap().contains(FAKE_KEY));
    let Step::Failed(m, _) = &s.step else { panic!() };
    assert!(m.contains("connection test failed") && !m.contains(FAKE_KEY), "{m}");
    assert!(models::prefs().providers.is_empty() && models::prefs().default.is_none());
    s.enter();
    // A good server, and a model name (Tab to the field).
    let (url, _seen2) = chat_server(200, 1);
    s.preset_url = Some(url);
    let Step::Form(_, f) = &s.step else { panic!("{:?}", s.step) };
    assert_eq!(f.key, FAKE_KEY);
    typed(&mut s, "\t");
    typed(&mut s, "grok-test");
    s.enter();
    wait(&mut s, "done", |st| matches!(st, Step::Done(..)));
    let Step::Done(spec, warn) = &s.step else { panic!() };
    assert_eq!(spec, "xai/grok-test");
    assert!(warn.as_deref().is_some_and(|w| w.contains("0600")), "file store warns: {warn:?}");
    let p = models::prefs();
    assert_eq!(p.default.as_deref(), Some("xai/grok-test"));
    assert_eq!(p.providers[0].key.as_deref(), Some("keychain:model-xai"));
    assert!(!std::fs::read_to_string(mouser_core::storage::preferences_path()).unwrap().contains(FAKE_KEY), "no key in preferences");
    assert_eq!(mouser_core::secrets::get("model-xai").unwrap().0, FAKE_KEY);
    assert_eq!(usable_default(), Some(("xai".into(), "grok-test".into())));
}

#[test]
#[serial_test::serial]
fn custom_url_needs_url_and_model_and_the_key_is_optional() {
    let _e = Env::new();
    let (url, seen) = chat_server(200, 1);
    let mut s = Setup::new(false);
    s.pick(s.opts.len() - 1);
    s.enter();
    assert!(matches!(&s.step, Step::Failed(m, _) if m.contains("URL")));
    s.enter();
    typed(&mut s, &url);
    typed(&mut s, "\t"); // api
    assert!(s.key(b" "));
    assert!(s.key(b" "), "toggles back to openai");
    typed(&mut s, "\t"); // key (left empty)
    typed(&mut s, "\t"); // model
    s.enter();
    assert!(matches!(&s.step, Step::Failed(m, _) if m.contains("model name")));
    s.enter();
    typed(&mut s, "qwen3");
    s.enter();
    wait(&mut s, "done", |st| matches!(st, Step::Done(..)));
    assert_eq!(seen.recv().unwrap(), "", "no key sent");
    let p = models::prefs();
    assert_eq!((p.default.as_deref(), p.providers[0].api, p.providers[0].key.clone()), (Some("custom/qwen3"), Api::OpenAi, None));
    let _ = local::dir();
}
