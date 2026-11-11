//! `mouser shell-integration --install / --uninstall` through the real binary,
//! in a temporary HOME (never the user's own startup files).

use std::process::Command;

fn mouser(home: &std::path::Path, shell: &str, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mouser-app"))
        .arg("shell-integration")
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("SHELL", shell)
        .env_remove("ZDOTDIR")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap()
}

#[test]
fn install_detects_the_shell_edits_once_and_uninstall_undoes_it() {
    let home = tempfile::tempdir().unwrap();
    let rc = home.path().join(".zshrc");
    std::fs::write(&rc, "export EDITOR=vi\n").unwrap();

    let o = mouser(home.path(), "/bin/zsh", &["--install"]);
    assert!(o.status.success(), "{o:?}");
    let said = String::from_utf8_lossy(&o.stdout);
    assert!(said.contains("zsh integration added to") && said.contains("backup:"), "{said}");
    let text = std::fs::read_to_string(&rc).unwrap();
    assert!(text.starts_with("export EDITOR=vi\n\n# >>> mouser shell integration >>>\n"), "{text}");
    assert!(text.contains("eval \"$(mouser shell-integration zsh)\""));

    let o = mouser(home.path(), "/bin/zsh", &["--install"]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("already installed"), "{o:?}");
    assert_eq!(std::fs::read_to_string(&rc).unwrap(), text);

    let o = mouser(home.path(), "/bin/zsh", &["--uninstall"]);
    assert!(o.status.success(), "{o:?}");
    assert_eq!(std::fs::read_to_string(&rc).unwrap(), "export EDITOR=vi\n");
}

#[test]
fn an_explicit_shell_and_rc_override_detection() {
    let home = tempfile::tempdir().unwrap();
    let rc = home.path().join("my rc");
    let o = mouser(home.path(), "/bin/tcsh", &["fish", "--install", "--rc", rc.to_str().unwrap()]);
    assert!(o.status.success(), "{o:?}");
    assert!(std::fs::read_to_string(&rc).unwrap().contains("mouser shell-integration fish | source"));
    assert!(!home.path().join(".config").exists());
}

#[test]
fn dry_run_and_unknown_shell_write_nothing() {
    let home = tempfile::tempdir().unwrap();
    let o = mouser(home.path(), "/bin/bash", &["--install", "--dry-run"]);
    assert!(o.status.success(), "{o:?}");
    assert!(String::from_utf8_lossy(&o.stdout).contains("dry run"));
    let o = mouser(home.path(), "/bin/tcsh", &["--install"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("can't tell the shell"), "{o:?}");
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}

#[test]
fn printing_the_script_is_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let o = mouser(home.path(), "/bin/zsh", &["bash"]);
    assert!(o.status.success());
    assert_eq!(String::from_utf8_lossy(&o.stdout), mouser_core::shell_integration::BASH);
    // A shell is still required when only printing.
    assert!(!mouser(home.path(), "/bin/zsh", &[]).status.success());
    // --rc / --dry-run need --install or --uninstall.
    assert!(!mouser(home.path(), "/bin/zsh", &["zsh", "--dry-run"]).status.success());
}
