use crate::storage::{connections_dir, ensure_dirs};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
// no path usage for now

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConnectionKind {
    Ssh(SshConnection),
    Waypipe(WaypipeConnection),
    Tunnel(TunnelConnection),
    X11(X11Connection),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Connection {
    pub name: String,
    pub kind: ConnectionKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SshConnection {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    pub compression: Option<bool>,
    pub server_alive_interval: Option<u32>,
    pub server_alive_count_max: Option<u32>,
    pub strict_host_key_checking: Option<String>,
    pub user_known_hosts_file: Option<String>,
    pub preferred_authentications: Option<String>,
    pub ciphers: Option<String>,
    pub macs: Option<String>,
    pub kex_algorithms: Option<String>,
    pub extra_options: HashMap<String, String>, // -o key=value
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WaypipeConnection {
    pub host: String,
    pub ssh_user: Option<String>,
    pub ssh_port: Option<u16>,
    pub compression: Option<String>,
    pub video: Option<String>,
    pub socket: Option<String>,
    pub args: Vec<String>, // pass-through args
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum TunnelType {
    #[default]
    Local, // -L
    Remote,  // -R
    Dynamic, // -D
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TunnelSpec {
    pub tunnel_type: TunnelType,
    pub spec: String, // e.g. "127.0.0.1:8080:internal:80" or "1080"
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TunnelConnection {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub forwards: Vec<TunnelSpec>,
    pub extra_options: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct X11Connection {
    pub host: String,
    pub user: Option<String>,
    pub trusted: bool, // -Y if true else -X
    pub extra_options: HashMap<String, String>,
}

pub fn save_connection(conn: &Connection) -> std::io::Result<()> {
    ensure_dirs()?;
    let path = connections_dir().join(format!("{}.toml", sanitize_name(&conn.name)));
    let s = toml::to_string_pretty(conn).unwrap();
    fs::write(path, s)
}

pub fn delete_connection(name: &str) -> std::io::Result<()> {
    let path = connections_dir().join(format!("{}.toml", sanitize_name(name)));
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn list_connections() -> std::io::Result<Vec<Connection>> {
    let dir = connections_dir();
    let mut out = Vec::new();
    if dir.exists() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry.path().extension().and_then(|s| s.to_str()) == Some("toml")
                && let Ok(s) = fs::read_to_string(entry.path())
                && let Ok(conn) = toml::from_str::<Connection>(&s)
            {
                out.push(conn);
            }
        }
    }
    Ok(out)
}

fn sanitize_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

pub fn build_ssh_args(cfg: &SshConnection) -> Vec<String> {
    let mut args = vec!["ssh".to_string()];
    if let Some(port) = cfg.port {
        args.push("-p".into());
        args.push(port.to_string());
    }
    if let Some(id) = &cfg.identity_file {
        args.push("-i".into());
        args.push(id.clone());
    }
    if let Some(pj) = &cfg.proxy_jump {
        args.push("-J".into());
        args.push(pj.clone());
    }
    if let Some(true) = cfg.compression {
        args.push("-C".into());
    }
    if let Some(v) = cfg.server_alive_interval {
        args.push("-o".into());
        args.push(format!("ServerAliveInterval={v}"));
    }
    if let Some(v) = cfg.server_alive_count_max {
        args.push("-o".into());
        args.push(format!("ServerAliveCountMax={v}"));
    }
    if let Some(v) = &cfg.strict_host_key_checking {
        args.push("-o".into());
        args.push(format!("StrictHostKeyChecking={v}"));
    }
    if let Some(v) = &cfg.user_known_hosts_file {
        args.push("-o".into());
        args.push(format!("UserKnownHostsFile={v}"));
    }
    if let Some(v) = &cfg.preferred_authentications {
        args.push("-o".into());
        args.push(format!("PreferredAuthentications={v}"));
    }
    if let Some(v) = &cfg.ciphers {
        args.push("-o".into());
        args.push(format!("Ciphers={v}"));
    }
    if let Some(v) = &cfg.macs {
        args.push("-o".into());
        args.push(format!("MACs={v}"));
    }
    if let Some(v) = &cfg.kex_algorithms {
        args.push("-o".into());
        args.push(format!("KexAlgorithms={v}"));
    }
    for (k, v) in &cfg.extra_options {
        args.push("-o".into());
        args.push(format!("{k}={v}"));
    }
    let target = if let Some(user) = &cfg.user { format!("{}@{}", user, cfg.host) } else { cfg.host.clone() };
    args.push(target);
    args
}

pub fn build_waypipe_args(cfg: &WaypipeConnection) -> Vec<String> {
    let mut args = vec!["waypipe".to_string(), "ssh".to_string()];
    if let Some(port) = cfg.ssh_port {
        args.push("-p".into());
        args.push(port.to_string());
    }
    if let Some(user) = &cfg.ssh_user {
        args.push("-l".into());
        args.push(user.clone());
    }
    if let Some(c) = &cfg.compression {
        args.push("--compress".into());
        args.push(c.clone());
    }
    if let Some(v) = &cfg.video {
        args.push("--video".into());
        args.push(v.clone());
    }
    if let Some(s) = &cfg.socket {
        args.push("--socket".into());
        args.push(s.clone());
    }
    args.extend(cfg.args.clone());
    args.push(cfg.host.clone());
    args
}

pub fn build_tunnel_args(cfg: &TunnelConnection) -> Vec<String> {
    let mut args = vec!["ssh".to_string(), "-N".to_string()];
    for f in &cfg.forwards {
        match f.tunnel_type {
            TunnelType::Local => {
                args.push("-L".into());
            }
            TunnelType::Remote => {
                args.push("-R".into());
            }
            TunnelType::Dynamic => {
                args.push("-D".into());
            }
        }
        args.push(f.spec.clone());
    }
    for (k, v) in &cfg.extra_options {
        args.push("-o".into());
        args.push(format!("{k}={v}"));
    }
    let target = if let Some(user) = &cfg.user { format!("{}@{}", user, cfg.host) } else { cfg.host.clone() };
    if let Some(port) = cfg.port {
        args.push("-p".into());
        args.push(port.to_string());
    }
    args.push(target);
    args
}

pub fn build_x11_args(cfg: &X11Connection) -> Vec<String> {
    let mut args = vec!["ssh".to_string()];
    args.push(if cfg.trusted { "-Y".into() } else { "-X".into() });
    for (k, v) in &cfg.extra_options {
        args.push("-o".into());
        args.push(format!("{k}={v}"));
    }
    let target = if let Some(user) = &cfg.user { format!("{}@{}", user, cfg.host) } else { cfg.host.clone() };
    args.push(target);
    args
}
