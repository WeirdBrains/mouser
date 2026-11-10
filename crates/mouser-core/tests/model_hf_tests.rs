//! Hugging Face search, details and install against a local mock Hub
//! (`MOUSER_HF_URL`; never the network) in a temporary MOUSER_HOME.

use mouser_core::models::{self, hf};
use serde_json::json;
use sha2::Digest;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

static ENV: Mutex<()> = Mutex::new(());

fn sha(b: &[u8]) -> String {
    sha2::Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// A mock Hub: answers every request from `route(path, range_start)`, logs paths.
fn hub(route: impl Fn(&str, u64) -> (u16, Vec<u8>) + Send + Sync + 'static) -> (String, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { break };
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut first = String::new();
            r.read_line(&mut first).unwrap();
            let path = first.split(' ').nth(1).unwrap_or("").to_string();
            let mut range = 0;
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).unwrap() == 0 || h == "\r\n" {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("range: bytes=") {
                    range = v.trim().trim_end_matches('-').parse().unwrap_or(0);
                }
            }
            log.lock().unwrap().push(format!("{path} {range}"));
            let (status, body) = route(&path, range);
            let (status, body) = if status == 200 && range > 0 { (206, body[range as usize..].to_vec()) } else { (status, body) };
            let mut w = s;
            write!(w, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            w.write_all(&body).unwrap();
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

fn with_env(base: &str, f: impl FnOnce(&std::path::Path)) {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    // SAFETY: serialised by ENV.
    unsafe {
        std::env::set_var("MOUSER_HOME", t.path());
        std::env::remove_var("MOUSER_MODELS_DIR");
        std::env::set_var("MOUSER_HF_URL", base);
    }
    f(t.path());
    unsafe {
        std::env::remove_var("MOUSER_HOME");
        std::env::remove_var("MOUSER_HF_URL");
    }
}

const ONE: &[u8] = b"GGUF one-file model bytes";
const PART1: &[u8] = b"GGUF part one";
const PART2: &[u8] = b" and part two";

fn repo_json(bad_sha: bool) -> Vec<u8> {
    json!({
        "id": "acme/Tiny-GGUF", "author": "acme", "downloads": 1234, "likes": 56, "pipeline_tag": "text-generation",
        "tags": ["gguf", "chat", "license:mit", "region:us"], "cardData": {"license": "mit"}, "lastModified": "2026-10-01T00:00:00Z",
        "gguf": {"architecture": "llama", "context_length": 4096, "total": 1100000000u64},
        "siblings": [
            {"rfilename": "README.md", "size": 10},
            {"rfilename": "Tiny-Q4_K_M.gguf", "size": ONE.len(), "lfs": {"sha256": if bad_sha { "00".repeat(32) } else { sha(ONE) }, "size": ONE.len()}},
            {"rfilename": "mmproj-Tiny-F16.gguf", "size": 5},
            {"rfilename": "big/Tiny-F16-00002-of-00002.gguf", "lfs": {"sha256": sha(PART2), "size": PART2.len()}},
            {"rfilename": "big/Tiny-F16-00001-of-00002.gguf", "lfs": {"sha256": sha(PART1), "size": PART1.len()}}
        ]
    })
    .to_string()
    .into_bytes()
}

fn routes(bad_sha: bool) -> impl Fn(&str, u64) -> (u16, Vec<u8>) {
    move |path, _| {
        match path {
        p if p.starts_with("/api/models?search=tiny%20chat&filter=gguf") => (
            200,
            json!([{"id": "acme/Tiny-GGUF", "downloads": 1234, "likes": 56, "pipeline_tag": "text-generation", "lastModified": "2026-10-01T00:00:00Z"},
                   {"id": "../evil", "downloads": 1}])
            .to_string()
            .into_bytes(),
        ),
        "/api/models/acme/Tiny-GGUF?blobs=true" => (200, repo_json(bad_sha)),
        "/acme/Tiny-GGUF/raw/main/README.md" => {
            (200, b"---\nlicense: mit\n---\n# Tiny\n\n<img src=x>\n![logo](x.png)\nA **tiny** chat model\nfor <a href=\"x\">tests</a> (see [the card](https://x)).\n\n```sh\nrun me\n```\nSecond paragraph.".to_vec())
        }
        "/acme/Tiny-GGUF/resolve/main/Tiny-Q4_K_M.gguf" => (200, ONE.to_vec()),
        "/acme/Tiny-GGUF/resolve/main/big/Tiny-F16-00001-of-00002.gguf" => (200, PART1.to_vec()),
        "/acme/Tiny-GGUF/resolve/main/big/Tiny-F16-00002-of-00002.gguf" => (200, PART2.to_vec()),
        _ => (404, b"{\"error\":\"not found\"}".to_vec()),
    }
    }
}

#[test]
fn search_lists_gguf_repos_and_drops_bad_ids() {
    let (base, seen) = hub(routes(false));
    with_env(&base, |_| {
        let hits = hf::search("tiny chat", 30).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!((hits[0].id.as_str(), hits[0].downloads, hits[0].likes, hits[0].updated.as_str()), ("acme/Tiny-GGUF", 1234, 56, "2026-10-01"));
        assert!(seen.lock().unwrap()[0].contains("sort=downloads"));
    });
}

#[test]
fn details_have_the_card_quants_and_numbers() {
    let (base, _) = hub(routes(false));
    with_env(&base, |_| {
        let d = hf::details("acme/Tiny-GGUF").unwrap();
        assert_eq!((d.license.as_str(), d.architecture.as_str(), d.context_length, d.params), ("mit", "llama", 4096, 1_100_000_000));
        assert_eq!(d.summary, "Tiny\n\nA **tiny** chat model for tests (see the card).\n\nSecond paragraph.");
        let labels: Vec<&str> = d.quants.iter().map(|q| q.label.as_str()).collect();
        assert_eq!(labels, ["Q4_K_M", "F16"], "smallest first, no mmproj");
        let f16 = &d.quants[1];
        assert_eq!(f16.id, "Tiny-F16");
        assert_eq!(
            f16.files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["big/Tiny-F16-00001-of-00002.gguf", "big/Tiny-F16-00002-of-00002.gguf"]
        );
        assert!(hf::details("../etc").is_err());
        assert!(hf::details("acme/missing").is_err());
    });
}

#[test]
fn install_downloads_checks_and_makes_it_the_default() {
    let (base, _) = hub(routes(false));
    with_env(&base, |home| {
        let d = hf::details("acme/Tiny-GGUF").unwrap();
        let plan = hf::Plan::new(&d, "Q4_K_M").unwrap();
        assert!(plan.consent_text().iter().any(|l| l.contains("License: mit")));
        let mut seen = vec![];
        let spec = hf::install(hf::Consent::given(&plan), &mut |a, b| {
            seen.push((a, b));
            true
        })
        .unwrap();
        assert_eq!(spec, "gemma-local/Tiny-Q4_K_M");
        assert_eq!(seen.last(), Some(&(ONE.len() as u64, ONE.len() as u64)));
        let f = home.join("models/hf/acme--Tiny-GGUF/Tiny-Q4_K_M.gguf");
        assert_eq!(std::fs::read(&f).unwrap(), ONE);
        // A file already there with the right size but wrong bytes is fetched again.
        let mut bad = ONE.to_vec();
        bad[0] ^= 1;
        std::fs::write(&f, &bad).unwrap();
        hf::install(hf::Consent::given(&plan), &mut |_, _| true).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), ONE, "re-checked, not trusted by size");
        assert_eq!(models::default_choice().unwrap(), ("gemma-local".into(), "Tiny-Q4_K_M".into()));
        assert!(models::find("gemma-local").unwrap().entry.models.contains(&"Tiny-Q4_K_M".to_string()));
        assert_eq!(models::resolve("Tiny-Q4_K_M").unwrap(), "gemma-local/Tiny-Q4_K_M");
        let i = hf::find("Tiny-Q4_K_M").unwrap();
        assert!(i.complete() && i.path() == f);

        // A split model: both parts, in order; the first is what llama.cpp opens.
        let split = hf::Plan::new(&d, "F16").unwrap();
        let spec = hf::install(hf::Consent::given(&split), &mut |_, _| true).unwrap();
        assert_eq!(spec, "gemma-local/Tiny-F16");
        let i = hf::find("Tiny-F16").unwrap();
        assert!(i.complete());
        assert!(i.path().ends_with("Tiny-F16-00001-of-00002.gguf"));

        assert!(hf::remove("Tiny-F16").unwrap());
        assert!(models::default_choice().is_none_or(|d| d.1 != "Tiny-F16"));
        assert!(!i.path().exists() && hf::find("Tiny-F16").is_none());
    });
}

#[test]
fn cancel_keeps_the_part_and_a_retry_resumes() {
    let (base, seen) = hub(routes(false));
    with_env(&base, |home| {
        let d = hf::details("acme/Tiny-GGUF").unwrap();
        let plan = hf::Plan::new(&d, "Q4_K_M").unwrap();
        let dir = home.join("models/hf/acme--Tiny-GGUF");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Tiny-Q4_K_M.gguf.part"), &ONE[..5]).unwrap();
        let e = hf::install(hf::Consent::given(&plan), &mut |_, _| false).unwrap_err().to_string();
        assert!(e.contains("cancelled"), "{e}");
        assert!(dir.join("Tiny-Q4_K_M.gguf.part").exists());
        assert!(models::default_choice().is_none(), "nothing became the default");
        hf::install(hf::Consent::given(&plan), &mut |_, _| true).unwrap();
        assert!(seen.lock().unwrap().iter().any(|l| l.ends_with("Tiny-Q4_K_M.gguf 5")), "resumed with Range: {:?}", seen.lock().unwrap());
        assert_eq!(std::fs::read(dir.join("Tiny-Q4_K_M.gguf")).unwrap(), ONE);
    });
}

#[test]
fn a_checksum_mismatch_deletes_the_file() {
    let (base, _) = hub(routes(true));
    with_env(&base, |home| {
        let d = hf::details("acme/Tiny-GGUF").unwrap();
        let plan = hf::Plan::new(&d, "Q4_K_M").unwrap();
        let e = hf::install(hf::Consent::given(&plan), &mut |_, _| true).unwrap_err().to_string();
        assert!(e.contains("checksum mismatch"), "{e}");
        let dir = home.join("models/hf/acme--Tiny-GGUF");
        assert!(!dir.join("Tiny-Q4_K_M.gguf").exists() && !dir.join("Tiny-Q4_K_M.gguf.part").exists());
        assert!(hf::find("Tiny-Q4_K_M").is_none());
    });
}

#[test]
fn gated_and_unknown_quants_are_refused() {
    let mut d = hf::Details { id: "acme/x".into(), ..Default::default() };
    assert!(hf::Plan::new(&d, "Q4_0").unwrap_err().to_string().contains("no GGUF"));
    d.gated = true;
    assert!(hf::Plan::new(&d, "Q4_0").unwrap_err().to_string().contains("gated"));
}

#[test]
fn quant_labels_and_encoding() {
    for (stem, want) in [
        ("Qwen2.5-7B-Instruct-Q4_K_M", "Q4_K_M"),
        ("gemma-4-E2B-it-Q4_0", "Q4_0"),
        ("model.IQ3_XS", "IQ3_XS"),
        ("Llama-3.2-1B-Instruct-bf16", "BF16"),
        ("phi-4-f16", "F16"),
        ("weird", "weird"),
    ] {
        assert_eq!(hf::quant_label(stem), want, "{stem}");
    }
    assert_eq!(hf::encode("a b/c&d"), "a%20b%2Fc%26d");
    assert!(hf::check_repo("a/b.c-d_e").is_ok());
    for bad in ["a", "a/b/c", "../b", "a/..", "a/b c"] {
        assert!(hf::check_repo(bad).is_err(), "{bad}");
    }
}

#[test]
fn only_architectures_the_bundled_llama_cpp_runs_as_chat_install() {
    let q = hf::Quant {
        label: "Q4_0".into(),
        id: "m-Q4_0".into(),
        files: vec![hf::HfFile { name: "m-Q4_0.gguf".into(), size: 1, sha256: Some("ab".repeat(32)) }],
    };
    let d = |arch: &str| hf::Details { id: "acme/m".into(), architecture: arch.into(), quants: vec![q.clone()], ..Default::default() };
    for a in ["llama", "qwen2", "gemma3", "Phi3", "mistral3"] {
        assert_eq!(hf::support(&d(a)), hf::Support::Supported, "{a}");
        assert!(hf::Plan::new(&d(a), "Q4_0").is_ok());
    }
    assert_eq!(hf::support(&d("")), hf::Support::Unverified);
    for (a, why) in [("bert", "embedding model"), ("qwen3tts", "text-to-speech"), ("clip", "vision projector"), ("frobnicator9", "isn't supported")] {
        let hf::Support::No(m) = hf::support(&d(a)) else { panic!("{a} should not run") };
        assert!(m.contains(why), "{a}: {m}");
        assert!(hf::Plan::new(&d(a), "Q4_0").unwrap_err().to_string().contains(why), "install refuses {a}");
    }
    let none = hf::Details { id: "acme/st".into(), architecture: "llama".into(), ..Default::default() };
    assert!(matches!(hf::support(&none), hf::Support::No(m) if m.contains("safetensors")));
    let lines = hf::details_lines(&d("bert")).join("\n");
    assert!(lines.contains("won't run"), "{lines}");
    let lines = hf::details_lines(&d("qwen35moe")).join("\n");
    assert!(lines.contains("supported architecture (qwen35moe), not load-tested"), "never claims it runs: {lines}");
    // No published checksum: not downloaded.
    let mut nosha = d("llama");
    nosha.quants[0].files[0].sha256 = None;
    assert!(hf::Plan::new(&nosha, "Q4_0").unwrap_err().to_string().contains("no SHA256"));
}

#[test]
fn file_names_that_could_leave_the_models_dir_are_skipped() {
    let f = |n: &str| hf::HfFile { name: n.into(), size: 1, sha256: None };
    let q = hf::quants(&[f("../evil-Q4_0.gguf"), f("/etc/x-Q4_0.gguf"), f("a\\..\\b-Q4_0.gguf"), f("C:x-Q4_0.gguf"), f("sub/ok-Q8_0.gguf")]);
    let labels: Vec<&str> = q.iter().map(|q| q.label.as_str()).collect();
    assert_eq!(labels, ["Q8_0"]);
}

#[test]
fn imatrix_and_mmproj_files_are_not_quantizations() {
    let f = |n: &str, size: u64| hf::HfFile { name: n.into(), size, sha256: None };
    let q = hf::quants(&[
        f("Qwen_Qwen3.6-35B-A3B-imatrix.gguf", 200),
        f("mmproj-Qwen3.6-F16.gguf", 300),
        f("Qwen_Qwen3.6-35B-A3B-Q4_K_M.gguf", 20_000),
        f("Qwen_Qwen3.6-35B-A3B-IQ2_XS.gguf", 10_000),
        f("README.md", 1),
    ]);
    let labels: Vec<&str> = q.iter().map(|q| q.label.as_str()).collect();
    assert_eq!(labels, ["IQ2_XS", "Q4_K_M"], "smallest first, no imatrix or mmproj");
}
