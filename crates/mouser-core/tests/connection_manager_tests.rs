use mouser_core::connections::*;
use std::collections::HashMap;

#[test]
fn build_ssh_args_includes_advanced() {
    let mut extra = HashMap::new();
    extra.insert("StrictHostKeyChecking".into(), "no".into());
    let cfg = SshConnection {
        host: "example.com".into(),
        user: Some("alice".into()),
        port: Some(2222),
        identity_file: Some("/home/alice/.ssh/id_ed25519".into()),
        proxy_jump: Some("bastion".into()),
        compression: Some(true),
        server_alive_interval: Some(30),
        server_alive_count_max: Some(3),
        strict_host_key_checking: None,
        user_known_hosts_file: None,
        preferred_authentications: Some("publickey".into()),
        ciphers: None,
        macs: None,
        kex_algorithms: None,
        extra_options: extra,
    };
    let args = build_ssh_args(&cfg);
    let s = args.join(" ");
    assert!(s.contains("ssh -p 2222 -i /home/alice/.ssh/id_ed25519 -J bastion -C -o ServerAliveInterval=30 -o ServerAliveCountMax=3 -o PreferredAuthentications=publickey -o StrictHostKeyChecking=no alice@example.com"));
}

#[test]
fn build_waypipe_args_pass_through() {
    let cfg = WaypipeConnection {
        host: "remote".into(),
        ssh_user: Some("bob".into()),
        ssh_port: Some(2200),
        compression: Some("auto".into()),
        video: Some("h264".into()),
        socket: Some("/tmp/wp.sock".into()),
        args: vec!["--fps".into(), "60".into()],
    };
    let s = build_waypipe_args(&cfg).join(" ");
    assert!(s.contains("waypipe ssh -p 2200 -l bob --compress auto --video h264 --socket /tmp/wp.sock --fps 60 remote"));
}

#[test]
fn build_tunnel_args_multiple_forwards() {
    let cfg = TunnelConnection {
        host: "db.internal".into(),
        user: Some("svc".into()),
        port: Some(22),
        forwards: vec![
            TunnelSpec { tunnel_type: TunnelType::Local, spec: "127.0.0.1:5433:db.internal:5432".into() },
            TunnelSpec { tunnel_type: TunnelType::Dynamic, spec: "1080".into() },
        ],
        extra_options: HashMap::new(),
    };
    let s = build_tunnel_args(&cfg).join(" ");
    assert!(s.contains("ssh -N -L 127.0.0.1:5433:db.internal:5432 -D 1080 -p 22 svc@db.internal"));
}

#[test]
fn build_x11_args_trusted() {
    let cfg = X11Connection { host: "gpu".into(), user: None, trusted: true, extra_options: HashMap::new() };
    let s = build_x11_args(&cfg).join(" ");
    assert!(s.contains("ssh -Y gpu"));
}
