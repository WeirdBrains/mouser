//! Plugin and engine downloads end to end against a local HTTPS server
//! (docs/downloads.md): signatures and TOFU keys, update and rollback, the
//! official lock and the offline bundle, `gh:` releases, and the download
//! manager on a slow server with failures in parallel (cap 4, 2 per host).

use mouser_core::downloads::{Manager, Op, State};
use mouser_core::install::{self, Kind, Opts, Src};
use serial_test::serial;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A tiny HTTPS file server: path -> body; `slow` sleeps between chunks.
struct Server {
    port: u16,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    slow: Arc<Mutex<Duration>>,
    /// (now, peak) open responses, total and by Host.
    open: Arc<Mutex<HashMap<String, (usize, usize)>>>,
    total: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    _ca: tempfile::TempDir,
}

impl Server {
    fn start() -> Server {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()]).unwrap();
        let ca = tempfile::tempdir().unwrap();
        std::fs::write(ca.path().join("ca.pem"), ck.cert.pem()).unwrap();
        // SAFETY: serial tests.
        unsafe { std::env::set_var("MOUSER_TEST_CA", ca.path().join("ca.pem")) };
        let cert = rustls::pki_types::CertificateDer::from(ck.cert.der().to_vec());
        let key = rustls::pki_types::PrivateKeyDer::try_from(ck.key_pair.serialize_der()).unwrap();
        let cfg = Arc::new(
            rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![cert], key)
                .unwrap(),
        );
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let s = Server {
            port,
            files: Default::default(),
            slow: Arc::new(Mutex::new(Duration::ZERO)),
            open: Default::default(),
            total: Default::default(),
            peak: Default::default(),
            _ca: ca,
        };
        let (files, slow, open, total, peak) = (s.files.clone(), s.slow.clone(), s.open.clone(), s.total.clone(), s.peak.clone());
        std::thread::spawn(move || {
            for c in l.incoming().flatten() {
                let (cfg, files, slow, open, total, peak) = (cfg.clone(), files.clone(), slow.clone(), open.clone(), total.clone(), peak.clone());
                std::thread::spawn(move || {
                    let conn = rustls::ServerConnection::new(cfg).unwrap();
                    let mut t = rustls::StreamOwned::new(conn, c);
                    let mut r = BufReader::new(&mut t);
                    let mut first = String::new();
                    if r.read_line(&mut first).is_err() {
                        return;
                    }
                    let mut host = String::new();
                    loop {
                        let mut l = String::new();
                        if r.read_line(&mut l).is_err() || l.trim().is_empty() {
                            break;
                        }
                        if let Some(h) = l.to_lowercase().strip_prefix("host:") {
                            host = h.trim().split(':').next().unwrap_or("").to_string();
                        }
                    }
                    drop(r);
                    let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let body = files.lock().unwrap().get(&path).cloned();
                    let Some(body) = body else {
                        let _ = t.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found");
                        return;
                    };
                    let n = total.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(n, Ordering::SeqCst);
                    {
                        let mut o = open.lock().unwrap();
                        let e = o.entry(host.clone()).or_default();
                        e.0 += 1;
                        e.1 = e.1.max(e.0);
                    }
                    let _ = t.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
                    let d = *slow.lock().unwrap();
                    for ch in body.chunks(4096) {
                        if !d.is_zero() {
                            std::thread::sleep(d);
                        }
                        if t.write_all(ch).is_err() {
                            break;
                        }
                    }
                    let _ = t.flush();
                    total.fetch_sub(1, Ordering::SeqCst);
                    open.lock().unwrap().get_mut(&host).unwrap().0 -= 1;
                    t.conn.send_close_notify();
                    let _ = t.flush();
                });
            }
        });
        s
    }
    fn url(&self, host: &str, path: &str) -> String {
        format!("https://{host}:{}{path}", self.port)
    }
    fn put(&self, path: &str, body: impl Into<Vec<u8>>) {
        self.files.lock().unwrap().insert(path.into(), body.into());
    }
}

struct Signer(minisign::KeyPair);

impl Signer {
    fn new() -> Self {
        Signer(minisign::KeyPair::generate_unencrypted_keypair().unwrap())
    }
    fn public(&self) -> String {
        self.0.pk.to_base64()
    }
    fn sign(&self, data: &[u8]) -> String {
        minisign::sign(Some(&self.0.pk), &self.0.sk, std::io::Cursor::new(data), None, None).unwrap().into_string()
    }
}

/// A mode-less command plugin as a .tar.gz: `<name>/plugin.toml` + `run`.
fn plugin_tgz(name: &str, version: &str, perms: &[&str]) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    {
        let mut b = tar::Builder::new(&mut gz);
        let p: Vec<String> = perms.iter().map(|p| format!("{p:?}")).collect();
        let manifest = format!(
            "name = \"{name}\"\nversion = \"{version}\"\npermissions = [{}]\n[[provides]]\nkind = \"command\"\n[run]\ntransport = \"command\"\ncommand = [\"./run\"]\ncommands = [\"{name}-cmd\"]\n",
            p.join(",")
        );
        for (f, data) in [("plugin.toml", manifest.into_bytes()), ("run", b"#!/bin/sh\necho hi\n".to_vec())] {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            h.set_path(format!("{name}/{f}")).unwrap();
            h.set_cksum();
            b.append(&h, &data[..]).unwrap();
        }
        b.finish().unwrap();
    }
    gz.finish().unwrap()
}

/// An engine as a .tar.gz: `engine.toml` + a placeholder library whose
/// contents are its name (what the fake probe reports).
fn engine_tgz(name: &str, version: &str) -> Vec<u8> {
    let lib = format!("{}{name}{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    {
        let mut b = tar::Builder::new(&mut gz);
        let m = format!("name = \"{name}\"\nversion = \"{version}\"\nabi = 2\nlibrary = \"{lib}\"\n");
        for (f, data) in [("engine.toml".to_string(), m.into_bytes()), (lib.clone(), name.as_bytes().to_vec())] {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_path(&f).unwrap();
            h.set_cksum();
            b.append(&h, &data[..]).unwrap();
        }
        b.finish().unwrap();
    }
    gz.finish().unwrap()
}

fn fake_probe(lib: &Path) -> anyhow::Result<String> {
    Ok(std::fs::read_to_string(lib)?.trim().to_string())
}

fn sha(b: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(b))
}

fn home() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    // SAFETY: serial tests.
    unsafe {
        std::env::set_var("MOUSER_HOME", t.path());
        std::env::set_var("MOUSER_ENGINE_DIRS", "");
        std::env::set_var("MOUSER_PLUGIN_DIRS", "");
        std::env::remove_var("MOUSER_TEST_LOCK");
        std::env::remove_var("MOUSER_TEST_OFFICIAL_KEY");
        std::env::remove_var("MOUSER_OFFLINE_BUNDLE");
    }
    install::set_engine_probe(fake_probe);
    t
}

fn mine() -> String {
    install::targets()[0].clone()
}

fn inst(src: &str, kind: Kind, yes: bool) -> anyhow::Result<install::Installed> {
    let mut asked = |_: &install::Card| yes;
    install::install(&Src::parse(src)?, kind, Opts::default(), &mut |_, _| true, &mut asked)
}

#[test]
#[serial]
fn signed_plugins_install_update_roll_back_and_keys_are_pinned_on_first_use() {
    let _h = home();
    let s = Server::start();
    let author = Signer::new();
    let t = mine();
    let v1 = plugin_tgz("hello", "1.0.0", &[]);
    let f1 = format!("/r/hello-1.0.0-{t}.tar.gz");
    s.put(&f1, v1.clone());
    s.put(&format!("{f1}.sha256"), format!("{}  x\n", sha(&v1)));
    s.put(&format!("{f1}.minisig"), author.sign(&v1));
    s.put("/r/minisign.pub", format!("untrusted comment: minisign public key\n{}\n", author.public()));

    // The card shows the signer; declining installs nothing.
    let mut seen = Vec::new();
    let mut no = |c: &install::Card| {
        seen = c.lines();
        false
    };
    let e = install::install(&Src::Url(s.url("localhost", &f1)), Kind::Plugin, Opts::default(), &mut |_, _| true, &mut no).unwrap_err();
    assert!(e.to_string().contains("declined"), "{e}");
    assert!(seen.iter().any(|l| l.contains("signed by its author") && l.contains("new")), "{seen:?}");
    assert!(mouser_core::plugins::find("hello").is_none());

    let i = inst(&s.url("localhost", &f1), Kind::Plugin, true).unwrap();
    assert_eq!((i.name.as_str(), i.version.as_str()), ("hello", "1.0.0"));
    assert!(matches!(i.trust, install::Trust::Author { new: true, .. }));
    let found = mouser_core::plugins::find("hello").unwrap();
    assert!(found.dir.ends_with("hello/1.0.0"), "{}", found.dir.display());
    assert_eq!(install::current(Kind::Plugin, "hello").as_deref(), Some("1.0.0"));

    // An update: a new version dir, `current` moves, the old one is kept.
    let v2 = plugin_tgz("hello", "1.1.0", &[]);
    let f2 = format!("/r/hello-1.1.0-{t}.tar.gz");
    s.put(&f2, v2.clone());
    s.put(&format!("{f2}.minisig"), author.sign(&v2));
    let i2 = inst(&s.url("localhost", &f2), Kind::Plugin, true).unwrap();
    assert_eq!(i2.replaced.as_deref(), Some("1.0.0"));
    assert!(matches!(i2.trust, install::Trust::Author { new: false, .. }));
    assert_eq!(mouser_core::plugins::find("hello").unwrap().manifest.version, "1.1.0");
    // `update` from the recorded source: already current.
    let mut yes = |_: &install::Card| true;
    assert!(install::update(Kind::Plugin, "hello", Opts::default(), &mut |_, _| true, &mut yes).unwrap().is_none());
    assert_eq!(install::rollback(Kind::Plugin, "hello").unwrap(), "1.0.0");
    assert_eq!(mouser_core::plugins::find("hello").unwrap().manifest.version, "1.0.0");
    assert_eq!(install::rollback(Kind::Plugin, "hello").unwrap(), "1.1.0");
    // Only the last two versions are kept.
    let v3 = plugin_tgz("hello", "1.2.0", &[]);
    let f3 = format!("/r/hello-1.2.0-{t}.tar.gz");
    s.put(&f3, v3.clone());
    s.put(&format!("{f3}.minisig"), author.sign(&v3));
    inst(&s.url("localhost", &f3), Kind::Plugin, true).unwrap();
    let base = mouser_core::plugins::dir().join("hello");
    assert!(base.join("1.2.0").is_dir() && base.join("1.1.0").is_dir() && !base.join("1.0.0").exists());

    // Same origin, a different key: refused.
    let mallory = Signer::new();
    let v4 = plugin_tgz("hello", "9.0.0", &[]);
    let f4 = format!("/r/hello-9.0.0-{t}.tar.gz");
    s.put(&f4, v4.clone());
    s.put(&format!("{f4}.minisig"), mallory.sign(&v4));
    s.put("/r/minisign.pub", format!("untrusted comment: x\n{}\n", mallory.public()));
    let e = inst(&s.url("localhost", &f4), Kind::Plugin, true).unwrap_err().to_string();
    assert!(e.contains("signing key changed"), "{e}");
    // A tampered file: the signature fails.
    s.put("/r/minisign.pub", format!("untrusted comment: x\n{}\n", author.public()));
    s.put(&format!("{f4}.minisig"), author.sign(b"something else"));
    let e = format!("{:#}", inst(&s.url("localhost", &f4), Kind::Plugin, true).unwrap_err());
    assert!(e.contains("signature check failed"), "{e}");
    assert_eq!(mouser_core::plugins::find("hello").unwrap().manifest.version, "1.2.0");
    // A wrong sha256.
    s.put(&format!("{f4}.sha256"), "00".repeat(32));
    let e = inst(&s.url("localhost", &f4), Kind::Plugin, true).unwrap_err().to_string();
    assert!(e.contains("sha256 mismatch"), "{e}");

    // Unsigned, elsewhere: a warning on the card, installed only on yes.
    let u = plugin_tgz("loose", "0.1.0", &["files.read"]);
    s.put("/u/loose-0.1.0.tar.gz", u);
    let mut lines = Vec::new();
    let mut look = |c: &install::Card| {
        lines = c.lines();
        true
    };
    let i =
        install::install(&Src::Url(s.url("127.0.0.1", "/u/loose-0.1.0.tar.gz")), Kind::Plugin, Opts::default(), &mut |_, _| true, &mut look).unwrap();
    assert_eq!(i.trust, install::Trust::Unsigned);
    assert!(lines.iter().any(|l| l.contains("UNSIGNED")) && lines.iter().any(|l| l.contains("files.read")), "{lines:?}");
    // http:// and oversize are refused.
    assert!(Src::parse("http://x/y").is_err());
    unsafe { std::env::set_var("MOUSER_TEST_MAX_BYTES", "10") };
    let e = inst(&s.url("localhost", &f3), Kind::Plugin, true).unwrap_err().to_string();
    unsafe { std::env::remove_var("MOUSER_TEST_MAX_BYTES") };
    assert!(e.contains("limit"), "{e}");
    // Engines and plugins don't mix up.
    let e = inst(&s.url("localhost", &f3), Kind::Engine, true).unwrap_err().to_string();
    assert!(e.contains("not an engine"), "{e}");
    let audit = std::fs::read_to_string(mouser_core::storage::resolve_home().join("logs/plugins-audit.log")).unwrap();
    assert!(audit.contains("plugin install hello 1.0.0") && audit.contains("rollback hello"), "{audit}");
}

#[test]
#[serial]
fn github_releases_engines_and_the_official_lock_with_its_offline_bundle() {
    let h = home();
    let s = Server::start();
    let t = mine();
    // gh:Owner/Repo: the release API, the asset for this target, its .sha256 and .minisig.
    let author = Signer::new();
    let e1 = engine_tgz("glow", "0.3.0");
    let file = format!("glow-0.3.0-{t}.tar.gz");
    s.put(&format!("/dl/{file}"), e1.clone());
    s.put(&format!("/dl/{file}.sha256"), sha(&e1));
    s.put(&format!("/dl/{file}.minisig"), author.sign(&e1));
    s.put("/dl/minisign.pub", author.public());
    let asset = |n: &str| serde_json::json!({"name": n, "browser_download_url": s.url("localhost", &format!("/dl/{n}"))});
    let rel = serde_json::json!({"tag_name": "v0.3.0", "assets": [
        asset(&file), asset(&format!("{file}.sha256")), asset(&format!("{file}.minisig")), asset("minisign.pub"),
        asset("glow-0.3.0-sparc-sun-solaris.tar.gz")]});
    s.put("/api/repos/someone/glow/releases/latest", rel.to_string());
    unsafe { std::env::set_var("MOUSER_TEST_GITHUB_API", s.url("localhost", "/api")) };
    let i = inst("gh:someone/glow", Kind::Engine, true).unwrap();
    assert_eq!((i.name.as_str(), i.version.as_str()), ("glow", "0.3.0"));
    let e = mouser_core::engines::find("glow").unwrap();
    assert!(e.dir.ends_with("glow/0.3.0") && !e.bundled, "{}", e.dir.display());
    assert!(inst("gh:someone/missing", Kind::Engine, true).unwrap_err().to_string().contains("no such release"));
    // Removing it removes every version.
    mouser_core::engines::uninstall("glow").unwrap();
    assert!(!mouser_core::engines::dir().join("glow").exists());

    // The official lock: signed with the (test) official key; no card on first run.
    let official = Signer::new();
    let c = plugin_tgz("greeter", "0.1.0", &["files.read"]);
    let skia = engine_tgz("skia", "1.4.0");
    s.put("/o/greeter.tar.gz", c.clone());
    s.put("/o/skia.tar.gz", skia.clone());
    let lock = format!(
        "[[item]]\nname = \"greeter\"\nkind = \"plugin\"\nversion = \"0.1.0\"\n[item.assets.{t}]\nurl = \"{}\"\nsha256 = \"{}\"\nsize = {}\nminisig = '''{}'''\n\
         [[item]]\nname = \"skia\"\nkind = \"engine\"\nversion = \"1.4.0\"\n[item.assets.{t}]\nurl = \"{}\"\nsha256 = \"{}\"\nsize = {}\nminisig = '''{}'''\n",
        s.url("localhost", "/o/greeter.tar.gz"),
        sha(&c),
        c.len(),
        official.sign(&c),
        s.url("localhost", "/o/skia.tar.gz"),
        sha(&skia),
        skia.len(),
        official.sign(&skia)
    );
    std::fs::write(h.path().join("lock.toml"), lock).unwrap();
    unsafe { std::env::set_var("MOUSER_TEST_LOCK", h.path().join("lock.toml")) };
    // Signed with another key than the pinned WeirdBrains key: refused.
    unsafe { std::env::remove_var("MOUSER_TEST_OFFICIAL_KEY") };
    assert!(inst("skia", Kind::Engine, true).is_err());
    unsafe { std::env::set_var("MOUSER_TEST_OFFICIAL_KEY", official.public()) };
    assert!(inst("skia", Kind::Plugin, true).unwrap_err().to_string().contains("is an official engine"));
    assert!(inst("nope", Kind::Plugin, true).unwrap_err().to_string().contains("not an official"));

    // The offline bundle has skia: it comes from there (no network); greeter downloads.
    let bundle = h.path().join("bundle");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(bundle.join("skia.tar.gz"), &skia).unwrap();
    unsafe { std::env::set_var("MOUSER_OFFLINE_BUNDLE", &bundle) };
    s.files.lock().unwrap().remove("/o/skia.tar.gz");
    let m = Manager::new(false);
    m.set_asker(Arc::new(|_| panic!("official first-run items don't ask")));
    assert_eq!(m.first_run(), 2);
    m.wait();
    let jobs = m.jobs();
    assert!(jobs.iter().all(|j| matches!(j.state, State::Done(_))), "{jobs:?}");
    assert!(jobs.iter().any(|j| j.src == "skia" && matches!(&j.state, State::Done(s) if s.contains("offline bundle"))), "{jobs:?}");
    assert!(mouser_core::plugins::find("greeter").is_some() && mouser_core::engines::find("skia").is_some());
    assert_eq!(install::missing_official().len(), 0);
    assert_eq!(m.first_run(), 0, "nothing left to fetch");
    // A tampered offline copy is refused.
    mouser_core::engines::uninstall("skia").unwrap();
    std::fs::write(bundle.join("skia.tar.gz"), b"tampered").unwrap();
    m.first_run();
    m.wait();
    assert!(m.jobs().iter().any(|j| matches!(&j.state, State::Failed(e) if e.contains("sha256"))), "{:?}", m.jobs());
}

/// A lock naming `greeter` at `version` (signed by `key`, served by `s`).
fn greeter_lock(h: &Path, s: &Server, key: &Signer, version: &str, perms: &[&str]) {
    let t = mine();
    let c = plugin_tgz("greeter", version, perms);
    let path = format!("/o/greeter-{version}.tar.gz");
    s.put(&path, c.clone());
    let lock = format!(
        "[[item]]\nname = \"greeter\"\nkind = \"plugin\"\nversion = \"{version}\"\n[item.assets.{t}]\nurl = \"{}\"\nsha256 = \"{}\"\nsize = {}\nminisig = '''{}'''\n",
        s.url("localhost", &path),
        sha(&c),
        c.len(),
        key.sign(&c)
    );
    std::fs::write(h.join("lock.toml"), lock).unwrap();
}

#[test]
#[serial]
fn a_newer_mouser_brings_its_official_plugins_up_to_the_pin() {
    let h = home();
    let s = Server::start();
    let official = Signer::new();
    unsafe {
        std::env::set_var("MOUSER_TEST_LOCK", h.path().join("lock.toml"));
        std::env::set_var("MOUSER_TEST_OFFICIAL_KEY", official.public());
    }
    let current = || install::current(Kind::Plugin, "greeter");
    // Mouser 1: greeter 0.1.9 on first run.
    greeter_lock(h.path(), &s, &official, "0.1.9", &["agent.use"]);
    let m = Manager::new(false);
    let asked = Arc::new(AtomicUsize::new(0));
    let a = asked.clone();
    m.set_asker(Arc::new(move |_| {
        a.fetch_add(1, Ordering::SeqCst);
        true
    }));
    assert_eq!(m.first_run(), 1);
    m.wait();
    assert_eq!(current().as_deref(), Some("0.1.9"));
    assert!(install::outdated_official().is_empty(), "at the pin");
    // Mouser 2 pins 0.1.10 (numerically newer), which also asks for files.read.
    greeter_lock(h.path(), &s, &official, "0.1.10", &["agent.use", "files.read"]);
    assert_eq!(install::outdated_official().len(), 1);
    assert_eq!(m.first_run(), 1, "queued as an update");
    m.wait();
    assert_eq!(current().as_deref(), Some("0.1.10"), "{:?}", m.jobs());
    assert_eq!(asked.load(Ordering::SeqCst), 1, "an added permission still asks");
    assert_eq!(m.first_run(), 0);
    // Rolled back from the pin by hand: left alone.
    install::rollback(Kind::Plugin, "greeter").unwrap();
    assert_eq!(current().as_deref(), Some("0.1.9"));
    assert!(install::outdated_official().is_empty(), "a rollback is the user's choice");
    // Installed from somewhere else (not the lock): never replaced.
    let rec = install::record(Kind::Plugin, "greeter").unwrap();
    let path = h.path().join("plugins/greeter/source.toml");
    let text = std::fs::read_to_string(&path).unwrap().replace(&format!("src = {:?}", rec.src), "src = \"gh:someone/greeter\"");
    std::fs::write(&path, text).unwrap();
    greeter_lock(h.path(), &s, &official, "0.2.0", &["agent.use", "files.read"]);
    assert!(install::outdated_official().is_empty(), "gh: installs keep their source");
}

#[test]
#[serial]
fn a_slow_server_and_failures_in_parallel_stay_within_the_caps_and_apart() {
    let _h = home();
    let s = Server::start();
    *s.slow.lock().unwrap() = Duration::from_millis(15);
    let t = mine();
    let mut srcs = Vec::new();
    for i in 0..8 {
        let name = format!("p{i}");
        let body = plugin_tgz(&name, "1.0.0", &[]);
        // ~24 chunks of 4 KB, padded with an extra file so each download takes a while.
        let mut padded = body.clone();
        if i % 2 == 0 {
            padded = pad(&name);
        }
        let f = format!("/s/{name}-1.0.0-{t}.tar.gz");
        s.put(&f, padded);
        let host = if i < 4 { "localhost" } else { "127.0.0.1" };
        srcs.push(s.url(host, &f));
    }
    // Two broken ones: a 404 and a wrong sha256.
    srcs.push(s.url("localhost", "/s/missing-1.0.0.tar.gz"));
    let bad = format!("/s/bad-1.0.0-{t}.tar.gz");
    s.put(&bad, plugin_tgz("bad", "1.0.0", &[]));
    s.put(&format!("{bad}.sha256"), "11".repeat(32));
    srcs.push(s.url("127.0.0.1", &bad));

    let m = Manager::new(false);
    m.set_asker(Arc::new(|_| true));
    let ids: Vec<u32> = srcs.iter().map(|u| m.enqueue(Kind::Plugin, u, Op::Install)).collect();
    // Cancel one while it is queued or running.
    m.cancel(Some(ids[7]));
    m.wait();
    let jobs = m.jobs();
    let (total, per_host) = m.peak();
    assert!(total <= 4 && per_host <= 2, "peak {total} total, {per_host} per host");
    assert_eq!(total, 4, "4 at once: 2 per host on 2 hosts");
    assert!(s.peak.load(Ordering::SeqCst) <= 4);
    for (h, (_, p)) in s.open.lock().unwrap().iter() {
        assert!(*p <= 2, "server saw {p} at once for {h}");
    }
    let state = |i: usize| jobs.iter().find(|j| j.id == ids[i]).unwrap().state.clone();
    for i in 0..7 {
        assert!(matches!(state(i), State::Done(_)), "job {i}: {:?}", state(i));
    }
    assert_eq!(state(7), State::Cancelled);
    assert!(matches!(state(8), State::Failed(e) if e.contains("404")), "{:?}", state(8));
    assert!(matches!(state(9), State::Failed(e) if e.contains("sha256")), "{:?}", state(9));
    assert!(mouser_core::plugins::find("p7").is_none() && mouser_core::plugins::find("bad").is_none());
    assert!(m.status().unwrap().contains("2 failed"));
    // retry queues the failed ones again (they fail again, on their own).
    assert_eq!(m.retry().len(), 2);
    m.wait();
    assert_eq!(m.jobs().iter().filter(|j| matches!(j.state, State::Failed(_))).count(), 2);
}

/// A plugin archive with a 100 KB filler file (so a slow download is slow).
fn pad(name: &str) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::none());
    {
        let mut b = tar::Builder::new(&mut gz);
        let manifest = format!(
            "name = \"{name}\"\nversion = \"1.0.0\"\n[[provides]]\nkind = \"command\"\n[run]\ntransport = \"command\"\ncommand = [\"./run\"]\ncommands = [\"{name}-cmd\"]\n"
        );
        let filler: Vec<u8> = (0..100_000u32).map(|i| (i * 7919 % 251) as u8).collect();
        for (f, data) in [("plugin.toml", manifest.into_bytes()), ("run", b"#!/bin/sh\n".to_vec()), ("filler", filler)] {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            h.set_path(format!("{name}/{f}")).unwrap();
            h.set_cksum();
            b.append(&h, &data[..]).unwrap();
        }
        b.finish().unwrap();
    }
    gz.finish().unwrap()
}

#[test]
#[serial]
fn local_paths_and_archives_install_into_version_dirs() {
    let h = home();
    let d = h.path().join("src");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(h.path().join("x.tar.gz"), plugin_tgz("loc", "2.0.0", &[])).unwrap();
    let i = inst(&h.path().join("x.tar.gz").display().to_string(), Kind::Plugin, true).unwrap();
    assert_eq!(i.trust, install::Trust::Local);
    assert!(mouser_core::plugins::find("loc").unwrap().dir.ends_with("loc/2.0.0"));
    mouser_core::plugins::uninstall("loc").unwrap();
    assert!(!mouser_core::plugins::dir().join("loc").exists());
    let e = inst("crate:x", Kind::Engine, true).unwrap_err().to_string();
    assert!(e.contains("mode plugins only"), "{e}");
}
