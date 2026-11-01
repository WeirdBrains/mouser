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
