//! `mouser plugin …` with the example plugins (examples/plugins), under a temporary MOUSER_HOME.

use mouser_app::plugin_cli::{Action, run};
use std::path::PathBuf;

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins")
}

fn cli(a: Action) -> anyhow::Result<String> {
    let mut v = Vec::new();
    run(&a, &mut v)?;
    Ok(String::from_utf8(v).unwrap())
}

fn have(p: &str) -> bool {
    std::process::Command::new(p).arg("--version").output().is_ok_and(|o| o.status.success())
}

#[test]
#[serial_test::serial]
fn install_list_permissions_priority_and_examples_run() {
    let t = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("MOUSER_HOME", t.path()) };
    for p in ["hello-command", "hello-mcp", "hello-fuzzball"] {
        let o = cli(Action::Install(examples().join(p).display().to_string())).unwrap();
        assert!(o.starts_with(&format!("installed {p} 0.1.0")), "{o}");
    }
    assert!(cli(Action::Install("https://example.com/p".into())).is_err());
    let l = cli(Action::List).unwrap();
    for want in [
        "hello-command",
        "via",
        "command",
        "commands: hello",
        "hello-mcp",
        "mcp",
        "tool-provider",
        "commander",
        "engines (kind engine",
        "mouser",
        "model-provider:",
    ] {
        assert!(l.contains(want), "{want} missing in\n{l}");
    }
    // Permissions: shown, set, persisted, `*` for all requested.
    let p = cli(Action::Permissions("hello-command".into(), vec![])).unwrap();
    assert!(p.contains("run") && p.contains("ask"), "{p}");
    let p = cli(Action::Permissions("hello-command".into(), vec!["run=allow".into()])).unwrap();
    assert!(p.lines().any(|l| l.contains("run") && l.contains("allow")), "{p}");
    let p = cli(Action::Permissions("hello-command".into(), vec!["*=never".into()])).unwrap();
    assert!(!p.contains("allow"), "{p}");
    assert!(cli(Action::Permissions("hello-command".into(), vec!["run=maybe".into()])).is_err());
    assert!(cli(Action::Permissions("hello-command".into(), vec!["root=allow".into()])).is_err());
    let p = cli(Action::Permissions("fuzzball:hello-fuzzball".into(), vec!["notify=allow".into()])).unwrap();
    assert!(p.contains("notify") && p.contains("allow"));
    // Saved remote grants are listed and revocable.
    let h = mouser_core::permissions::Scope::FuzzballRemote { host: Some("m@127.0.0.1:22422 SHA256:x".into()) };
    mouser_core::permissions::Broker::answer("hello-fuzzball", &["notify".into()], &h, mouser_core::permissions::Answer::Always).unwrap();
    let l = cli(Action::PermissionsList).unwrap();
    assert!(l.contains("\"ssh:m@127.0.0.1:22422 SHA256:x/hello-fuzzball\": notify=allow"), "{l}");
    let o = cli(Action::Revoke("ssh:m@127.0.0.1:22422 SHA256:x/hello-fuzzball".into())).unwrap();
    assert!(o.contains("revoked"));
    assert!(!cli(Action::PermissionsList).unwrap().contains("ssh:"));
    assert!(cli(Action::Revoke("ssh:nope".into())).is_err());
    // Priority and enable/disable mirror the engines CLI.
    assert!(cli(Action::Move("hello-mcp".into(), "first".into())).unwrap().contains("priority: hello-mcp > "));
    assert!(cli(Action::Move("hello-mcp".into(), "0".into())).is_err());
    assert!(cli(Action::Disable("hello-command".into())).unwrap().contains("disabled"));
    assert!(cli(Action::List).unwrap().contains("[disabled]"));
    assert!(mouser_core::plugins::Registry::load().command("hello").is_none());
    cli(Action::Enable("hello-command".into())).unwrap();
    assert!(mouser_core::plugins::Registry::load().command("hello").is_some());
    // The command example answers JSON.
    #[cfg(unix)]
    {
        let p = mouser_core::plugins::find("hello-command").unwrap();
        let r = mouser_core::plugins::run_command(
            &p,
            &mouser_core::plugins::CommandRequest { api: 1, command: "hello".into(), args: vec!["Michael".into()], cwd: None },
        )
        .unwrap();
        assert_eq!(r.text, "Hello, Michael! (from hello-command)");
    }
    // The MCP example: handshake and its tool (needs python3; CI has it).
    if have("python3") {
        let o = cli(Action::Probe("hello-mcp".into())).unwrap();
        assert!(o.contains("MCP server hello-mcp 0.1.0") && o.contains("tool greet"), "{o}");
        let p = mouser_core::plugins::find("hello-mcp").unwrap();
        let mut c = mouser_core::mcp::Client::start(&p.argv(), &p.dir, "hello-mcp").unwrap();
        assert_eq!(c.call("greet", serde_json::json!({"name": "Mouser"})).unwrap(), "Hello, Mouser!");
    }
    assert!(cli(Action::Probe("hello-command".into())).is_err());
    let o = cli(Action::Uninstall("hello-command".into())).unwrap();
    assert!(o.contains("forgot its permissions"));
    assert!(!mouser_core::permissions::Store::load().plugins.contains_key("hello-command"));
    unsafe { std::env::remove_var("MOUSER_HOME") };
}

fn kind(a: Action, k: Option<&str>) -> anyhow::Result<String> {
    let mut v = Vec::new();
    mouser_app::plugin_cli::run_kind(&a, k, &mut v)?;
    Ok(String::from_utf8(v).unwrap())
}

/// P6: engines and modes are plugins; `mouser engine X` = `mouser plugin X --kind engine`.
#[test]
#[serial_test::serial]
fn engines_and_modes_through_the_plugin_cli() {
    let t = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("MOUSER_HOME", t.path()) };
    unsafe { std::env::remove_var("MOUSER_ENGINE") };
    let engine_list = || {
        let mut v = Vec::new();
        mouser_app::engine_cli::run(&mouser_app::engine_cli::Action::List, &mut v).unwrap();
        String::from_utf8(v).unwrap()
    };
    // The same list either way.
    assert_eq!(kind(Action::List, Some("engine")).unwrap(), engine_list());
    assert!(engine_list().contains("mouser"));
    assert!(kind(Action::List, Some("nope")).is_err());
    // The permanent engine can't be disabled; others can, and leave the walk.
    let e = kind(Action::Disable("mouser".into()), Some("engine")).unwrap_err().to_string();
    assert!(e.contains("permanent"), "{e}");
    let o = kind(Action::Disable("skia".into()), Some("engine")).unwrap();
    assert!(o.contains("skia disabled; engines tried: mouser"), "{o}");
    assert!(kind(Action::Use("skia".into()), Some("engine")).unwrap_err().to_string().contains("disabled"));
    if mouser_render::engine::available_builtins().iter().any(|(n, _)| *n == "skia") {
        assert!(engine_list().contains("disabled"), "{}", engine_list());
    }
    // `mouser plugin enable skia` (no --kind) reaches the same engine.
    cli(Action::Enable("skia".into())).unwrap();
    assert!(mouser_core::plugins::is_enabled("skia"));
    assert_eq!(kind(Action::List, Some("engine")).unwrap(), engine_list());
    // Built-in modes: listed with their hotkeys, and can be turned off.
    let l = kind(Action::List, Some("mode")).unwrap();
    assert!(l.contains("commander") && l.contains("switch: esc :c") && l.contains("chat") && !l.contains("openai"), "{l}");
    cli(Action::Disable("commander".into())).unwrap();
    cli(Action::Disable("chat".into())).unwrap();
    let l = cli(Action::List).unwrap();
    assert!(l.contains("[disabled]") && l.contains("plain terminal"), "{l}");
    cli(Action::Enable("commander".into())).unwrap();
    cli(Action::Enable("chat".into())).unwrap();
    // A mode plugin installs like any plugin.
    let o = cli(Action::Install(examples().join("hello-mode").display().to_string())).unwrap();
    assert!(o.contains("installed hello-mode 0.1.0 (mode)") && o.contains("run"), "{o}");
    assert!(kind(Action::List, Some("mode")).unwrap().contains("switch: esc :h"));
    assert!(kind(Action::Use("hello-mode".into()), None).is_err());
    unsafe { std::env::remove_var("MOUSER_HOME") };
}
