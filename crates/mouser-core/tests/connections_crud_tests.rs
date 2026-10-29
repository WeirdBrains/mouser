use mouser_core::connections::*;
use once_cell::sync::Lazy;
use std::env;
use std::sync::Mutex;

static ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[test]
fn ssh_add_edit_delete() {
    let _g = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        env::set_var("MOUSER_HOME", tmp.path());
    }
    let ssh = SshConnection { host: "example.com".into(), ..Default::default() };
    let conn = Connection { name: "ssh1".into(), kind: ConnectionKind::Ssh(ssh) };
    save_connection(&conn).unwrap();
    let loaded = load_connection("ssh1").unwrap().unwrap();
    assert_eq!("ssh1", loaded.name);
    let mut edited = load_connection("ssh1").unwrap().unwrap();
    if let ConnectionKind::Ssh(ref mut s) = edited.kind {
        s.port = Some(2222);
    }
    save_connection(&edited).unwrap();
    let re = load_connection("ssh1").unwrap().unwrap();
    if let ConnectionKind::Ssh(s) = re.kind {
        assert_eq!(Some(2222), s.port);
    } else {
        panic!();
    }
    delete_connection("ssh1").unwrap();
    assert!(load_connection("ssh1").unwrap().is_none());
}

#[test]
fn waypipe_add_edit_delete() {
    let _g = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        env::set_var("MOUSER_HOME", tmp.path());
    }
    let wp = WaypipeConnection { host: "remote".into(), ..Default::default() };
    let conn = Connection { name: "wp1".into(), kind: ConnectionKind::Waypipe(wp) };
    save_connection(&conn).unwrap();
    let mut edited = load_connection("wp1").unwrap().unwrap();
    if let ConnectionKind::Waypipe(ref mut w) = edited.kind {
        w.ssh_port = Some(2200);
    }
    save_connection(&edited).unwrap();
    let re = load_connection("wp1").unwrap().unwrap();
    if let ConnectionKind::Waypipe(w) = re.kind {
        assert_eq!(Some(2200), w.ssh_port);
    } else {
        panic!();
    }
    delete_connection("wp1").unwrap();
    assert!(load_connection("wp1").unwrap().is_none());
}

#[test]
fn tunnels_add_edit_delete() {
    let _g = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        env::set_var("MOUSER_HOME", tmp.path());
    }
    let mut t = TunnelConnection { host: "db.internal".into(), ..Default::default() };
    t.forwards.push(TunnelSpec { tunnel_type: TunnelType::Local, spec: "127.0.0.1:5433:db.internal:5432".into() });
    let conn = Connection { name: "tun1".into(), kind: ConnectionKind::Tunnel(t) };
    save_connection(&conn).unwrap();
    let mut edited = load_connection("tun1").unwrap().unwrap();
    if let ConnectionKind::Tunnel(ref mut tt) = edited.kind {
        tt.forwards.push(TunnelSpec { tunnel_type: TunnelType::Dynamic, spec: "1080".into() });
    }
    save_connection(&edited).unwrap();
    let re = load_connection("tun1").unwrap().unwrap();
    if let ConnectionKind::Tunnel(tt) = re.kind {
        assert!(tt.forwards.iter().any(|f| matches!(f.tunnel_type, TunnelType::Dynamic)));
    } else {
        panic!();
    }
    delete_connection("tun1").unwrap();
    assert!(load_connection("tun1").unwrap().is_none());
}

#[test]
fn x11_add_edit_delete() {
    let _g = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        env::set_var("MOUSER_HOME", tmp.path());
    }
    let x11 = X11Connection { host: "gpu".into(), ..Default::default() };
    let conn = Connection { name: "x11a".into(), kind: ConnectionKind::X11(x11) };
    save_connection(&conn).unwrap();
    let mut edited = load_connection("x11a").unwrap().unwrap();
    if let ConnectionKind::X11(ref mut x) = edited.kind {
        x.trusted = true;
    }
    save_connection(&edited).unwrap();
    let re = load_connection("x11a").unwrap().unwrap();
    if let ConnectionKind::X11(x) = re.kind {
        assert!(x.trusted);
    } else {
        panic!();
    }
    delete_connection("x11a").unwrap();
    assert!(load_connection("x11a").unwrap().is_none());
}
