//! All fixtures live in temp dirs; the real ~/.ssh is never read or written.
use mouser_core::connections::{Connection, ConnectionKind, SshConnection, list_connections, save_connection};
use mouser_core::ssh_config::*;
use serial_test::serial;
use std::fs;
use std::path::{Path, PathBuf};

const FIXTURE: &str = r#"# Personal hosts
Include config.d/*.conf

Host *
    ServerAliveInterval 30

# Production box
Host prod
	HostName prod.example.com
	User deploy
	Port=2222
	IdentityFile ~/.ssh/id_prod
	ForwardAgent yes   

Host web1 web2
    HostName web.example.com

Host *.internal !secret
    User ops

Match host bastion exec "true"
    User admin

Host jump
    HostName jump.example.com
    ProxyJump bastion
"#;

fn setup() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let ssh = tmp.path().join(".ssh");
    fs::create_dir_all(ssh.join("config.d")).unwrap();
    let root = ssh.join("config");
    fs::write(&root, FIXTURE).unwrap();
    fs::write(ssh.join("config.d/work.conf"), "Host work\n  HostName work.corp\n  User me\n").unwrap();
    fs::write(ssh.join("config.d/ignored.txt"), "Host nope\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o600)).unwrap();
    }
    (tmp, root)
}

fn get(root: &Path, alias: &str) -> SshHostEntry {
    load_hosts(root).into_iter().find(|h| h.alias == alias).unwrap_or_else(|| panic!("no {alias}"))
}

#[test]
fn parses_hosts_includes_and_skips_wildcards() {
    let (_t, root) = setup();
    let names: Vec<String> = load_hosts(&root).into_iter().map(|h| h.alias).collect();
    assert_eq!(names, vec!["work", "prod", "web1", "web2", "jump"]);
    let prod = get(&root, "prod");
    assert_eq!(prod.conn.host, "prod.example.com");
    assert_eq!(prod.conn.user.as_deref(), Some("deploy"));
    assert_eq!(prod.conn.port, Some(2222));
    assert_eq!(prod.conn.identity_file.as_deref(), Some("~/.ssh/id_prod"));
    assert_eq!(prod.conn.extra_options.get("ForwardAgent").map(String::as_str), Some("yes"));
    assert!(prod.read_only.is_none());
    assert_eq!(get(&root, "jump").conn.proxy_jump.as_deref(), Some("bastion"));
    assert!(get(&root, "work").file.ends_with("config.d/work.conf"));
    // Several patterns on one Host line: read-only with a reason.
    assert!(get(&root, "web1").read_only.as_deref().unwrap().contains("several patterns"));
    let files = config_files(&root);
    assert_eq!(files.len(), 2);
}

#[test]
fn unchanged_upsert_is_byte_identical_and_writes_nothing() {
    let (_t, root) = setup();
    let prod = get(&root, "prod");
    upsert_host(&root, "prod", &prod.conn).unwrap();
    assert_eq!(fs::read_to_string(&root).unwrap(), FIXTURE);
    assert!(!backup_path(&root).exists(), "no write, no backup");
}

#[test]
fn edits_in_place_preserving_everything_else() {
    let (_t, root) = setup();
    let mut c = get(&root, "prod").conn;
    c.user = Some("admin".into());
    c.port = None;
    c.proxy_jump = Some("jump".into());
    upsert_host(&root, "prod", &c).unwrap();
    let text = fs::read_to_string(&root).unwrap();
    let expected = FIXTURE
        .replace("\tUser deploy\n", "\tUser admin\n")
        .replace("\tPort=2222\n", "")
        .replace("\tForwardAgent yes   \n", "\tForwardAgent yes   \n\tProxyJump jump\n");
    assert_eq!(text, expected);
    assert_eq!(fs::read_to_string(backup_path(&root)).unwrap(), FIXTURE);
    let p = get(&root, "prod");
    assert_eq!(p.conn.user.as_deref(), Some("admin"));
    assert_eq!(p.conn.proxy_jump.as_deref(), Some("jump"));
    assert_eq!(p.conn.port, None);
}

#[test]
fn edits_hosts_in_included_files_there() {
    let (_t, root) = setup();
    let inc = root.parent().unwrap().join("config.d/work.conf");
    let mut c = get(&root, "work").conn;
    c.port = Some(2200);
    upsert_host(&root, "work", &c).unwrap();
    assert_eq!(fs::read_to_string(&inc).unwrap(), "Host work\n  HostName work.corp\n  User me\n  Port 2200\n");
    assert_eq!(fs::read_to_string(&root).unwrap(), FIXTURE);
}

#[test]
fn appends_new_hosts_and_refuses_duplicates() {
    let (_t, root) = setup();
    let c = SshConnection { host: "db.example.com".into(), user: Some("pg".into()), ..Default::default() };
    add_host(&root, "db", &c).unwrap();
    let text = fs::read_to_string(&root).unwrap();
    assert_eq!(text, format!("{FIXTURE}\nHost db\n    HostName db.example.com\n    User pg\n"));
    assert!(matches!(add_host(&root, "db", &c), Err(SshConfigError::Exists(_))));
    assert!(matches!(add_host(&root, "bad alias", &c), Err(SshConfigError::BadAlias(_))));
    assert!(matches!(add_host(&root, "w*", &c), Err(SshConfigError::BadAlias(_))));
    // Creating a config from scratch.
    let fresh = root.with_file_name("fresh");
    add_host(&fresh, "a", &c).unwrap();
    assert_eq!(fs::read_to_string(&fresh).unwrap(), "Host a\n    HostName db.example.com\n    User pg\n");
}

#[test]
fn deletes_blocks_and_renames() {
    let (_t, root) = setup();
    delete_host(&root, "prod").unwrap();
    let text = fs::read_to_string(&root).unwrap();
    assert!(!text.contains("prod"));
    assert!(!text.contains("Production box"));
    assert!(text.contains("    ServerAliveInterval 30\n\nHost web1 web2\n"), "{text}");
    delete_host(&root, "jump").unwrap();
    let text = fs::read_to_string(&root).unwrap();
    assert!(text.ends_with("    User admin\n"), "{text:?}");
    assert!(matches!(delete_host(&root, "web1"), Err(SshConfigError::ReadOnly(_))));
    assert!(matches!(delete_host(&root, "nope"), Err(SshConfigError::NotFound(_))));
    let c = get(&root, "work").conn;
    rename_host(&root, "work", "office", &c).unwrap();
    assert!(fs::read_to_string(root.parent().unwrap().join("config.d/work.conf")).unwrap().starts_with("Host office\n"));
}

#[test]
fn read_only_blocks_are_never_written() {
    let (_t, root) = setup();
    let c = get(&root, "web1").conn;
    assert!(matches!(upsert_host(&root, "web1", &c), Err(SshConfigError::ReadOnly(_))));
    assert_eq!(fs::read_to_string(&root).unwrap(), FIXTURE);
}

#[cfg(unix)]
#[test]
fn permissions_are_kept_and_backup_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let (_t, root) = setup();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let mut c = get(&root, "prod").conn;
    c.user = Some("x".into());
    upsert_host(&root, "prod", &c).unwrap();
    assert_eq!(mode(&root), 0o600);
    assert_eq!(mode(&backup_path(&root)), 0o600);
    fs::set_permissions(&root, fs::Permissions::from_mode(0o644)).unwrap();
    c.user = Some("y".into());
    upsert_host(&root, "prod", &c).unwrap();
    assert_eq!(mode(&root), 0o644);
    let fresh = root.with_file_name("fresh");
    add_host(&fresh, "a", &c).unwrap();
    assert_eq!(mode(&fresh), 0o600);
    // Only the config, the backup, fresh and config.d exist: no stray temp files.
    let mut names: Vec<String> = fs::read_dir(root.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    assert_eq!(names, vec!["config", "config.d", "config.mouser.bak", "fresh"]);
}

#[test]
fn fingerprint_changes_when_an_included_file_changes() {
    let (_t, root) = setup();
    let before = fingerprint(&root);
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(root.parent().unwrap().join("config.d/work.conf"), "Host work\n  HostName work.corp\nHost lab\n  HostName lab\n").unwrap();
    assert_ne!(before, fingerprint(&root));
    assert!(load_hosts(&root).iter().any(|h| h.alias == "lab"));
    // A newly matching include file is picked up too.
    fs::write(root.parent().unwrap().join("config.d/new.conf"), "Host newbox\n").unwrap();
    assert!(load_hosts(&root).iter().any(|h| h.alias == "newbox"));
}

#[test]
#[serial]
fn migration_moves_only_ssh_entries_on_request() {
    let (t, root) = setup();
    unsafe {
        std::env::set_var("MOUSER_HOME", t.path().join("mouser"));
    }
    save_connection(&Connection {
        name: "old".into(),
        kind: ConnectionKind::Ssh(SshConnection { host: "old.example".into(), ..Default::default() }),
    })
    .unwrap();
    save_connection(&Connection { name: "prod".into(), kind: ConnectionKind::Ssh(SshConnection { host: "dup".into(), ..Default::default() }) })
        .unwrap();
    save_connection(&Connection { name: "wp".into(), kind: ConnectionKind::Waypipe(Default::default()) }).unwrap();
    assert_eq!(mouser_ssh_connections().len(), 2);
    let m = migrate_mouser_ssh(&root);
    assert_eq!(m.moved, vec!["old".to_string()]);
    assert!(m.skipped.contains_key("prod"));
    assert_eq!(get(&root, "old").conn.host, "old.example");
    assert_eq!(get(&root, "prod").conn.host, "prod.example.com");
    let left: Vec<String> = list_connections().unwrap().into_iter().map(|c| c.name).collect();
    assert!(left.contains(&"wp".to_string()) && left.contains(&"prod".to_string()) && !left.contains(&"old".to_string()));
}

#[test]
fn ssh_form_validates_and_saves() {
    use mouser_core::appearance::{EditKey, Outcome};
    let (_t, root) = setup();
    let mut f = SshForm::new(None, &SshConnection::default(), None);
    assert_eq!(f.handle(EditKey::Save), Outcome::Continue);
    assert!(f.error.as_deref().unwrap().contains("Alias"));
    for c in "db x*".chars() {
        f.handle(EditKey::Char(c));
    }
    assert_eq!(f.fields[0].value, "dbx", "spaces and wildcards are ignored");
    f.handle(EditKey::Next);
    for c in "db.example.com".chars() {
        f.handle(EditKey::Char(c));
    }
    f.focus = 3;
    for c in "99999a".chars() {
        f.handle(EditKey::Char(c));
    }
    assert_eq!(f.fields[3].value, "99999");
    assert_eq!(f.handle(EditKey::Save), Outcome::Continue);
    assert!(f.error.as_deref().unwrap().contains("Port"));
    f.handle(EditKey::Backspace);
    assert_eq!(f.handle(EditKey::Enter), Outcome::Save);
    assert_eq!(f.save(&root).unwrap(), "dbx");
    let h = get(&root, "dbx");
    assert_eq!(h.conn.host, "db.example.com");
    assert_eq!(h.conn.port, Some(9999));
    // Editing keeps options the form does not show.
    let prod = get(&root, "prod");
    let mut f = SshForm::new(Some("prod"), &prod.conn, None);
    f.focus = 2;
    f.handle(EditKey::Char('2'));
    assert_eq!(f.save(&root).unwrap(), "prod");
    let p = get(&root, "prod");
    assert_eq!(p.conn.user.as_deref(), Some("deploy2"));
    assert_eq!(p.conn.extra_options.get("ForwardAgent").map(String::as_str), Some("yes"));
    // Read-only forms only view.
    let w = get(&root, "web1");
    let mut f = SshForm::new(Some("web1"), &w.conn, w.read_only.clone());
    f.handle(EditKey::Char('x'));
    assert_eq!(f.fields[0].value, "web1");
    assert!(f.save(&root).is_err());
    assert_eq!(f.handle(EditKey::Enter), Outcome::Close);
}
