//! `mouser mcp enable/status/disable/token` under a temporary MOUSER_HOME (file secret store).

use mouser_app::mcp_cli::{Action, run};

fn cli(a: Action) -> anyhow::Result<String> {
    let mut v = Vec::new();
    run(&a, &mut v)?;
    Ok(String::from_utf8(v).unwrap())
}

#[test]
#[serial_test::serial]
fn enable_status_token_and_disable() {
    let t = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("MOUSER_HOME", t.path());
        std::env::set_var("MOUSER_SECRET_STORE", "file");
    }
    assert!(cli(Action::Status).unwrap().contains("off"));
    assert!(cli(Action::Enable { https: false, port: Some(17000), https_port: Some(17000) }).is_err(), "same ports");
    let o = cli(Action::Enable { https: true, port: Some(17001), https_port: Some(17002) }).unwrap();
    assert!(o.contains("17001") && o.contains("warning: no OS keychain"), "{o}");
    let s = cli(Action::Status).unwrap();
    assert!(s.contains("enabled") && s.contains("https://127.0.0.1:17002/mcp") && s.contains("SHA256"), "{s}");
    let tok = cli(Action::TokenShow).unwrap();
    assert!(!s.contains(tok.trim()), "status never shows the token");
    cli(Action::TokenRotate).unwrap();
    assert_ne!(cli(Action::TokenShow).unwrap(), tok);
    let p = mouser_core::storage::load_preferences().mcp.unwrap().server.unwrap();
    assert!(p.enabled && p.https && p.port == 17001);
    cli(Action::Disable).unwrap();
    assert!(!mouser_core::storage::load_preferences().mcp.unwrap().server.unwrap().enabled);
    assert!(cli(Action::ServeStdio).is_err(), "stdio refuses while off");
    unsafe {
        std::env::remove_var("MOUSER_SECRET_STORE");
        std::env::remove_var("MOUSER_HOME");
    }
}

#[cfg(unix)]
#[test]
#[serial_test::serial]
fn servers_add_list_test_call_and_remove() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("MOUSER_HOME", t.path());
        std::env::set_var("MOUSER_SECRET_STORE", "file");
    }
    let srv = t.path().join("srv");
    std::fs::write(
        &srv,
        r#"#!/bin/sh
while read -r l; do
  id=$(printf '%s' "$l" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$l" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"9"}}}\n' "$id";;
    *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"greet","description":"say hi","inputSchema":{"type":"object"}}]}}\n' "$id";;
    *'"tools/call"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"hi from fake"}]}}\n' "$id";;
  esac
done
"#,
    )
    .unwrap();
    std::fs::set_permissions(&srv, std::fs::Permissions::from_mode(0o755)).unwrap();
    let add = |name: &str, command: Vec<String>, url: Option<&str>, token: Option<&str>| {
        cli(Action::ServersAdd { name: name.into(), command, env: vec![], url: url.map(Into::into), token: token.map(Into::into), ca: None })
    };
    assert!(add("bad name", vec!["x".into()], None, None).is_err());
    assert!(add("x", vec![], None, None).is_err(), "needs a command or url");
    assert!(add("x", vec![], Some("http://example.com/mcp"), None).unwrap_err().to_string().contains("only allowed"));
    add("fake", vec![srv.display().to_string()], None, None).unwrap();
    let secret = "s3cret-value-never-in-prefs";
    let o = add("remote", vec![], Some("https://mcp.example.com/mcp"), Some(secret)).unwrap();
    assert!(o.contains("warning: no OS keychain") && !o.contains(secret), "{o}");
    let prefs = std::fs::read_to_string(t.path().join("preferences.toml")).unwrap();
    assert!(prefs.contains("[[mcp.servers]]") && prefs.contains("keychain:mcp-server-remote") && !prefs.contains(secret), "{prefs}");
    let l = cli(Action::ServersList).unwrap();
    assert!(l.contains("fake") && l.contains("mcp.example.com") && l.contains("token in keychain") && !l.contains(secret), "{l}");
    let o = cli(Action::ServersTest { name: "fake".into(), call: None, args: None }).unwrap();
    assert!(o.contains("connected to fake 9") && o.contains("greet"), "{o}");
    // A call goes through the broker: no terminal, so it can't ask.
    let e = cli(Action::ServersTest { name: "fake".into(), call: Some("greet".into()), args: None }).unwrap_err().to_string();
    assert!(e.contains("mcp-server:fake tool.greet=allow"), "{e}");
    mouser_app::plugin_cli::run(
        &mouser_app::plugin_cli::Action::Permissions("mcp-server:fake".into(), vec!["tool.greet=allow".into()]),
        &mut Vec::new(),
    )
    .unwrap();
    let o = cli(Action::ServersTest { name: "fake".into(), call: Some("greet".into()), args: Some("{}".into()) }).unwrap();
    assert!(o.contains("hi from fake"), "{o}");
    mouser_app::plugin_cli::run(
        &mouser_app::plugin_cli::Action::Permissions("mcp-server:fake".into(), vec!["tool.greet=never".into()]),
        &mut Vec::new(),
    )
    .unwrap();
    assert!(cli(Action::ServersTest { name: "fake".into(), call: Some("greet".into()), args: None }).unwrap_err().to_string().contains("never"));
    // Updating keeps the stored token; removing drops it and the permissions.
    add("remote", vec![], Some("https://mcp.example.com/v2"), None).unwrap();
    assert!(std::fs::read_to_string(t.path().join("preferences.toml")).unwrap().contains("keychain:mcp-server-remote"));
    assert!(mouser_core::secrets::get("mcp-server-remote").is_some());
    cli(Action::ServersRemove("remote".into())).unwrap();
    assert!(mouser_core::secrets::get("mcp-server-remote").is_none());
    cli(Action::ServersRemove("fake".into())).unwrap();
    assert!(!std::fs::read_to_string(t.path().join("plugins/permissions.toml")).unwrap_or_default().contains("mcp-server:fake"));
    assert!(cli(Action::ServersRemove("fake".into())).is_err());
    let o = cli(Action::ServersAdd {
        name: "envy".into(),
        command: vec!["x".into()],
        env: vec!["A=plain".into(), "B=keychain:b".into()],
        url: None,
        token: None,
        ca: None,
    })
    .unwrap();
    assert!(o.contains("A is stored in plain text") && !o.contains("B is stored"), "{o}");
    assert!(
        cli(Action::ServersAdd { name: "envy".into(), command: vec!["x".into()], env: vec!["nokv".into()], url: None, token: None, ca: None })
            .is_err()
    );
    cli(Action::ServersRemove("envy".into())).unwrap();
    // The server prefs survive `mcp enable` (they share [mcp]).
    add("fake", vec![srv.display().to_string()], None, None).unwrap();
    cli(Action::Enable { https: false, port: Some(17011), https_port: Some(17012) }).unwrap();
    assert_eq!(mouser_core::storage::load_preferences().mcp.unwrap().servers.len(), 1);
    unsafe {
        std::env::remove_var("MOUSER_SECRET_STORE");
        std::env::remove_var("MOUSER_HOME");
    }
}
