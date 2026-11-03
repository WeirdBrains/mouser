use mouser_core::prefs_edit::*;
use std::sync::Mutex;

static ENV: Mutex<()> = Mutex::new(());

fn home() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
    let g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("MOUSER_HOME", t.path());
        std::env::set_var("MOUSER_SECRET_STORE", "file");
    }
    (g, t)
}

fn field(key: &str) -> Field {
    schema().into_iter().find(|f| f.key == key).unwrap()
}

#[test]
fn validation_rejects_bad_values() {
    assert_eq!(validate(&field("keys.leader_timeout_ms"), " 800 ").unwrap(), "800");
    assert!(validate(&field("keys.leader_timeout_ms"), "50").is_err());
    assert!(validate(&field("keys.leader_timeout_ms"), "abc").is_err());
    assert!(validate(&field("font_size"), "NaN").is_err());
    assert_eq!(validate(&field("mode"), "DARK").unwrap(), "dark");
    assert!(validate(&field("mode"), "purple").is_err());
    assert!(validate(&field("models.local.url"), "ftp://x").is_err());
    assert_eq!(validate(&field("chat.transcripts"), "off").unwrap(), "false");
    assert!(validate(&field("models.local.dir"), "/x").is_err());
}

#[test]
fn write_back_keeps_comments_and_unknown_keys() {
    let (_g, t) = home();
    let p = t.path().join("preferences.toml");
    std::fs::write(&p, "# mine\ntheme = \"Red Star\"\nfuture_key = 3\n\n[keys]\n# wait\nleader_timeout_ms = 1200\n").unwrap();
    let mut d = load_doc().unwrap();
    set(&mut d, &field("keys.leader_timeout_ms"), "900").unwrap();
    set(&mut d, &field("models.local.context"), "16384").unwrap();
    set(&mut d, &field("mode"), "dark").unwrap();
    save(&d).unwrap();
    let s = std::fs::read_to_string(&p).unwrap();
    assert!(s.contains("# mine") && s.contains("# wait") && s.contains("future_key = 3"), "{s}");
    let prefs = mouser_core::storage::load_preferences();
    assert_eq!(prefs.keys.unwrap().leader_timeout_ms, Some(900));
    assert_eq!(prefs.models.unwrap().local.unwrap().context, Some(16384));
    let d = load_doc().unwrap();
    assert_eq!(get(&d, &field("mode")).as_deref(), Some("dark"));
    let mut d = d;
    reset(&mut d, &field("keys.leader_timeout_ms")).unwrap();
    save(&d).unwrap();
    assert_eq!(mouser_core::storage::load_preferences().keys.and_then(|k| k.leader_timeout_ms), None);
}

#[test]
fn secrets_never_land_in_the_file() {
    let (_g, t) = home();
    let mut d = load_doc().unwrap();
    set(&mut d, &field("models.local.key"), "sk-test-123").unwrap();
    save(&d).unwrap();
    let s = std::fs::read_to_string(t.path().join("preferences.toml")).unwrap();
    assert!(!s.contains("sk-test-123") && s.contains("keychain:local-model-key"), "{s}");
    assert_eq!(get(&d, &field("models.local.key")).as_deref(), Some("set (keychain)"));
    reset(&mut d, &field("models.local.key")).unwrap();
    assert!(mouser_core::secrets::get("local-model-key").is_none());
}

#[test]
fn unparsable_file_is_left_alone() {
    let (_g, t) = home();
    let p = t.path().join("preferences.toml");
    std::fs::write(&p, "theme = = broken").unwrap();
    assert!(load_doc().is_err());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "theme = = broken");
}

#[test]
fn every_section_has_fields() {
    let s = schema();
    let secs = sections(&s);
    assert!(secs.len() >= 6);
    for k in s.iter().map(|f| f.key) {
        assert_eq!(s.iter().filter(|f| f.key == k).count(), 1, "{k}");
    }
}

#[test]
fn mcp_lets_encrypt_settings_use_dropdowns_and_keep_comments() {
    let (_g, t) = home();
    let p = t.path().join("preferences.toml");
    std::fs::write(&p, "[mcp.server]\n# my server\nenabled = true\nhttps = true\n\n[mcp.server.tls]\n# public name\ndomains = \"old.example.com\"\n")
        .unwrap();
    assert!(matches!(field("mcp.server.tls.source").kind, Kind::Choice(ref v) if v == &["self-signed", "files", "acme"]));
    assert!(matches!(field("mcp.server.tls.challenge").kind, Kind::Choice(ref v) if v == &["tls-alpn-01", "http-01", "dns-01"]));
    assert!(validate(&field("mcp.server.tls.source"), "letsencrypt").is_err());
    assert!(validate(&field("mcp.server.tls.directory"), "ftp://ca").is_err());
    let mut d = load_doc().unwrap();
    set(&mut d, &field("mcp.server.tls.source"), "ACME").unwrap();
    set(&mut d, &field("mcp.server.tls.domains"), "mcp.example.com").unwrap();
    set(&mut d, &field("mcp.server.tls.email"), "me@example.com").unwrap();
    set(&mut d, &field("mcp.server.tls.staging"), "on").unwrap();
    set(&mut d, &field("mcp.server.tls.challenge"), "http-01").unwrap();
    set(&mut d, &field("mcp.server.tls.http_port"), "80").unwrap();
    set(&mut d, &field("mcp.server.bind"), "0.0.0.0").unwrap();
    save(&d).unwrap();
    let s = std::fs::read_to_string(&p).unwrap();
    assert!(s.contains("# my server") && s.contains("# public name"), "{s}");
    let srv = mouser_core::storage::load_preferences().mcp.unwrap().server.unwrap();
    let tls = srv.tls.clone().unwrap();
    assert_eq!((tls.source.as_deref(), tls.domains.as_deref(), tls.staging, tls.http_port), (Some("acme"), Some("mcp.example.com"), true, Some(80)));
    assert_eq!(srv.bind.as_deref(), Some("0.0.0.0"));
    let acme = mouser_core::mcp_acme::AcmeConfig::from_prefs(&tls).unwrap();
    assert_eq!((acme.challenge.as_str(), acme.ca_name()), ("http-01", "Let's Encrypt staging"));
    assert_eq!(mouser_core::mcp_server::TlsSource::of(&srv).unwrap(), mouser_core::mcp_server::TlsSource::Acme);
}
