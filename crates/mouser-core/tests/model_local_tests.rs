//! `mouser model local enable|disable|move` and bare model names (temporary
//! MOUSER_HOME only; nothing is downloaded).

use mouser_core::models::{self, ProviderEntry, local};
use std::sync::Mutex;

static ENV: Mutex<()> = Mutex::new(());

fn with_home(f: impl FnOnce(&std::path::Path)) {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    // SAFETY: serialised by ENV.
    unsafe {
        std::env::set_var("MOUSER_HOME", t.path());
        std::env::remove_var("MOUSER_MODELS_DIR");
    }
    f(t.path());
    unsafe { std::env::remove_var("MOUSER_HOME") };
}

#[test]
fn bare_model_names_resolve_to_their_provider() {
    with_home(|_| {
        let mut p = models::prefs();
        p.providers.push(ProviderEntry {
            name: "lab".into(),
            api: models::Api::OpenAi,
            url: "http://127.0.0.1:1/v1".into(),
            key: None,
            models: vec!["qwen-x".into(), "grok-4".into()],
            ca: None,
        });
        models::save(&p).unwrap();
        assert_eq!(models::resolve("gemma-4-e4b").unwrap(), "gemma-local/gemma-4-e4b");
        assert_eq!(models::resolve("qwen-x").unwrap(), "lab/qwen-x");
        assert_eq!(models::resolve("lab").unwrap(), "lab");
        assert_eq!(models::resolve("lab/anything").unwrap(), "lab/anything");
        let e = models::resolve("grok-4").unwrap_err().to_string();
        assert!(e.contains("lab") && e.contains("xai"), "ambiguous: {e}");
        assert!(models::resolve("nope").is_err());
    });
}

#[test]
fn enable_and_disable_the_local_model() {
    with_home(|_| {
        assert_eq!(local::pick(None).unwrap().id, "gemma-4-e2b", "E2B when nothing is downloaded");
        assert!(local::pick(Some("gemma-9")).is_err());
        let w = local::pick(Some("gemma-4-e4b")).unwrap();
        assert_eq!(local::enable(w).unwrap(), "gemma-local/gemma-4-e4b");
        assert_eq!(models::default_choice().unwrap(), ("gemma-local".into(), "gemma-4-e4b".into()));
        assert!(local::disable().unwrap(), "it was the default");
        assert!(models::default_choice().is_none());
        assert!(models::find("gemma-local").is_none(), "hidden while disabled");
        let e = models::resolve("gemma-4-e2b").unwrap_err().to_string();
        assert!(e.contains("mouser model local enable"), "{e}");
        assert!(!local::disable().unwrap());
        local::enable(local::pick(None).unwrap()).unwrap();
        assert!(models::find("gemma-local").is_some());
        assert_eq!(models::default_choice().unwrap().1, "gemma-4-e2b");
    });
}

#[test]
fn pick_prefers_the_downloaded_model_and_move_checks_it() {
    with_home(|home| {
        let e4b = local::weights("gemma-4-e4b").unwrap();
        std::fs::create_dir_all(local::dir()).unwrap();
        // A sparse stand-in of the right size: "downloaded", but not the real bytes.
        std::fs::File::create(e4b.path()).unwrap().set_len(e4b.size).unwrap();
        assert_eq!(local::pick(None).unwrap().id, "gemma-4-e4b");
        let to = home.join("big-drive/models");
        let old = e4b.path();
        let moved = local::move_to(&to, &mut |_| {}).unwrap();
        assert_eq!(moved, vec![std::fs::canonicalize(&to).unwrap().join(e4b.file)]);
        assert!(!old.exists(), "the old copy is gone");
        assert_eq!(local::dir(), std::fs::canonicalize(&to).unwrap(), "saved as [models.local] dir");
        assert!(e4b.installed());
        // Moving to where the files already are only saves the setting.
        assert!(local::move_to(&to, &mut |_| {}).unwrap().is_empty());
        // MOUSER_MODELS_DIR wins over the setting, so move refuses.
        unsafe { std::env::set_var("MOUSER_MODELS_DIR", home.join("x")) };
        assert!(local::move_to(&to, &mut |_| {}).is_err());
        unsafe { std::env::remove_var("MOUSER_MODELS_DIR") };
    });
}
