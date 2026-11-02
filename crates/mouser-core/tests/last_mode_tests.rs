//! The last mode is remembered in $MOUSER_HOME/state (not preferences).
use mouser_core::modes::{last_mode_path, load_last_mode, save_last_mode};

#[test]
#[serial_test::serial]
fn last_mode_round_trips_including_no_mode() {
    let t = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("MOUSER_HOME", t.path()) };
    assert_eq!(load_last_mode(), None, "first run: nothing saved");
    save_last_mode(Some("chat")).unwrap();
    assert_eq!(load_last_mode(), Some(Some("chat".into())));
    save_last_mode(None).unwrap();
    assert_eq!(load_last_mode(), Some(None), "no mode is remembered too");
    // The last window to close wins.
    save_last_mode(Some("commander")).unwrap();
    assert_eq!(load_last_mode(), Some(Some("commander".into())));
    assert!(last_mode_path().starts_with(t.path().join("state")));
    assert!(!t.path().join("preferences.toml").exists(), "state, not config");
}
