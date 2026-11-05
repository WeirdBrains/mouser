//! Host calls from mode plugins (`Out::Request`, docs/mode-protocol.md),
//! answered here for `transport = "mode"` plugins. Every call is checked
//! against the plugin's granted permissions first.
//!
//! Answered here (synchronously, or on a thread for the network):
//! - preferences: `prefs.schema`, `prefs.get` (secrets only show whether
//!   they're set), `prefs.set` / `prefs.reset` with `prefs.write`. A secret
//!   is only ever typed into Mouser's own popup (`prefs.secret_prompt`,
//!   handled by [`super::stdio`]), never sent by a plugin;
//! - `permissions.granted`;
//! - local read-only files with `files.read` (`Fs::Read` up to 1 MiB per
//!   call, `Fs::Stat`, `Fs::List`), never inside the secrets deny-list;
//! - `fetch` with `net.fetch`: one HTTP(S) request by Mouser's client;
//! - the shell's remote host with `remote.read`: `remote.location`,
//!   `remote.list`, `remote.read` (1 MiB per call) over its ssh connection;
//! - `agent.models` and `agent.set_default` with `agent.use`.
//!
//! The agent session (`agent.send` ...), `Approve`, `permissions.request`
//! and `prefs.secret_prompt` need Mouser's popup or a session and are
//! handled by [`super::stdio`]. Everything else answers "not available yet".

use mouser_core::prefs_edit::{self, Kind};
use mouser_core::remote::SshTarget;
use mouser_mode_api::host_api::{Call, Fs, Target};
use mouser_mode_api::services;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// Largest `Fs::Read` / `remote.read` answer.
pub const MAX_READ: u64 = 1 << 20;
/// Largest `fetch` body (default 4 MiB).
pub const MAX_FETCH: u64 = 16 << 20;

pub type Answer = Result<Value, String>;

/// A call's answer: now, or later from a worker thread.
pub enum Reply {
    Now(Answer),
    Later(mpsc::Receiver<Answer>),
}

/// What the host knows about the plugin making the call.
#[derive(Default, Clone)]
pub struct Env {
    pub granted: Vec<String>,
    /// The active tab's ssh host, if the shell is remote.
    pub ssh: Option<SshTarget>,
    /// The remote shell's folder.
    pub remote_dir: Option<PathBuf>,
}

fn has(granted: &[String], p: &str) -> bool {
    granted.iter().any(|g| g == p)
}

fn later(f: impl FnOnce() -> Answer + Send + 'static) -> Reply {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    Reply::Later(rx)
}

/// Answer one call for a plugin with these grants (local calls only; the
/// network ones block). Tests and simple callers.
pub fn answer(call: &Call, granted: &[String]) -> Answer {
    match dispatch(call, &Env { granted: granted.to_vec(), ..Default::default() }) {
        Reply::Now(a) => a,
        Reply::Later(rx) => rx.recv().unwrap_or_else(|_| Err("worker stopped".into())),
    }
}

/// Answer one call.
pub fn dispatch(call: &Call, env: &Env) -> Reply {
    let granted = &env.granted;
    match call {
        Call::Service { method, params } => {
            if !services::allowed(method, granted) {
                return Reply::Now(Err(format!("{method} is not allowed (permission missing or unknown method)")));
            }
            service(method, params, env)
        }
        Call::Fs { target, op } => Reply::Now((|| {
            if !matches!(target, Target::Local | Target::Active) {
                return Err("remote files: use remote.list / remote.read".into());
            }
            if op.mutates() {
                return Err("changing files is not available yet".into());
            }
            if !has(granted, "files.read") {
                return Err("needs files.read".into());
            }
            fs(op)
        })()),
        Call::Spawn { spec, .. } => {
            Reply::Now(spec.check_escalation().and(Err("spawn is not available yet; a plugin may start its own processes".into())))
        }
        _ => Reply::Now(Err("not available yet".into())),
    }
}

fn kind_json(k: &Kind) -> Value {
    match k {
        Kind::Bool => json!({"type": "bool"}),
        Kind::Int { min, max } => json!({"type": "int", "min": min, "max": max}),
        Kind::Float { min, max } => json!({"type": "float", "min": min, "max": max}),
        Kind::Choice(v) => json!({"type": "choice", "values": v}),
        Kind::Text { url } => json!({"type": "text", "url": url}),
        Kind::Secret(_) => json!({"type": "secret"}),
        Kind::ReadOnly(hint) => json!({"type": "readonly", "hint": hint}),
    }
}

fn field(params: &Value) -> Result<prefs_edit::Field, String> {
    let key = params["key"].as_str().ok_or("params.key missing")?;
    prefs_edit::schema().into_iter().find(|f| f.key == key).ok_or_else(|| format!("no preference {key}"))
}

/// The preference a `prefs.secret_prompt` asks for: (key, label).
pub fn secret_field(params: &Value) -> Result<(String, String), String> {
    let f = field(params)?;
    if !matches!(f.kind, Kind::Secret(_)) {
        return Err(format!("{} is not a secret; use prefs.set", f.key));
    }
    Ok((f.key.to_string(), f.label.to_string()))
}

/// Save a secret typed into Mouser's popup (keychain; the file keeps a reference).
pub fn save_secret(key: &str, value: &str) -> Answer {
    let f = field(&json!({"key": key}))?;
    let mut doc = prefs_edit::load_doc().map_err(|e| format!("{e:#}"))?;
    prefs_edit::set(&mut doc, &f, value).map_err(|e| format!("{e:#}"))?;
    prefs_edit::save(&doc).map_err(|e| format!("{e:#}"))?;
    Ok(json!({"saved": true}))
}

fn service(method: &str, params: &Value, env: &Env) -> Reply {
    let now = |a: Answer| Reply::Now(a);
    match method {
        "permissions.granted" => now(Ok(json!(env.granted))),
        "prefs.schema" => now(Ok(Value::Array(
            prefs_edit::schema()
                .iter()
                .map(|f| json!({"section": f.section, "key": f.key, "label": f.label, "default": f.default, "live": f.live, "kind": kind_json(&f.kind)}))
                .collect(),
        ))),
        "prefs.get" => now((|| {
            let f = field(params)?;
            let doc = prefs_edit::load_doc().map_err(|e| format!("{e:#}"))?;
            // Secrets: whether set ("set (keychain)"), never the value.
            Ok(json!(prefs_edit::get(&doc, &f)))
        })()),
        "prefs.set" => now((|| {
            let f = field(params)?;
            if matches!(f.kind, Kind::Secret(_)) {
                return Err(format!("{} is a secret; ask for it with prefs.secret_prompt (typed into Mouser's popup)", f.key));
            }
            let raw = params["value"].as_str().map(str::to_string).unwrap_or_else(|| params["value"].to_string());
            let mut doc = prefs_edit::load_doc().map_err(|e| format!("{e:#}"))?;
            prefs_edit::set(&mut doc, &f, &raw).map_err(|e| format!("{e:#}"))?;
            prefs_edit::save(&doc).map_err(|e| format!("{e:#}"))?;
            Ok(json!(prefs_edit::get(&doc, &f)))
        })()),
        "prefs.reset" => now((|| {
            let f = field(params)?;
            let mut doc = prefs_edit::load_doc().map_err(|e| format!("{e:#}"))?;
            prefs_edit::reset(&mut doc, &f).map_err(|e| format!("{e:#}"))?;
            prefs_edit::save(&doc).map_err(|e| format!("{e:#}"))?;
            Ok(Value::Null)
        })()),
        "agent.models" => now(Ok(models())),
        "agent.set_default" => now((|| {
            let spec = params["model"].as_str().ok_or("params.model missing")?;
            let spec = mouser_core::models::resolve(spec).map_err(|e| format!("{e:#}"))?;
            let mut m = mouser_core::models::prefs();
            m.default = Some(spec.clone());
            mouser_core::models::save(&m).map_err(|e| format!("{e:#}"))?;
            Ok(json!(spec))
        })()),
        "fetch" => match fetch_args(params) {
            Ok(a) => later(move || fetch(a)),
            Err(e) => now(Err(e)),
        },
        "remote.location" => now(Ok(match &env.ssh {
            Some(t) => json!({"remote": true, "label": t.label(), "dir": env.remote_dir.as_ref().map(|d| d.display().to_string())}),
            None => json!({"remote": false}),
        })),
        "remote.list" | "remote.read" => {
            let Some(t) = env.ssh.clone() else { return now(Err("the shell is not on a remote host".into())) };
            let p = params.clone();
            let m = method.to_string();
            later(move || remote(&m, &p, t))
        }
        m => now(Err(format!("{m} is not available yet"))),
    }
}

/// Providers and the default, never a key.
fn models() -> Value {
    let def = mouser_core::models::default_choice().map(|(p, m)| format!("{p}/{m}"));
    let ps: Vec<Value> = mouser_core::models::all()
        .into_iter()
        .map(|p| {
            json!({"name": p.entry.name, "models": p.entry.models, "description": p.description,
                   "configured": !matches!(p.origin, mouser_core::models::Origin::Preset)})
        })
        .collect();
    json!({"default": def, "providers": ps})
}

struct FetchArgs {
    url: String,
    method: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
    max: u64,
}

fn fetch_args(p: &Value) -> Result<FetchArgs, String> {
    let url = p["url"].as_str().ok_or("params.url missing")?.to_string();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("fetch: an http:// or https:// URL".into());
    }
    let method = p["method"].as_str().unwrap_or("GET").to_ascii_uppercase();
    if !["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"].contains(&method.as_str()) {
        return Err(format!("fetch: method {method} not supported"));
    }
    let mut headers = Vec::new();
    if let Some(h) = p["headers"].as_object() {
        for (k, v) in h {
            let v = v.as_str().ok_or("fetch: header values are strings")?;
            if k.chars().chain(v.chars()).any(|c| c == '\r' || c == '\n')
                || k.eq_ignore_ascii_case("host")
                || k.eq_ignore_ascii_case("content-length")
            {
                return Err(format!("fetch: header {k} not allowed"));
            }
            headers.push((k.clone(), v.to_string()));
        }
    }
    let body = p["body"].as_str().map(str::to_string);
    let max = p["max_bytes"].as_u64().unwrap_or(4 << 20).min(MAX_FETCH);
    Ok(FetchArgs { url, method, headers, body, max })
}

fn fetch(a: FetchArgs) -> Answer {
    use mouser_core::models::http;
    use std::io::Read;
    let hs: Vec<(&str, String)> = a.headers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let r = if a.method == "GET" { http::get(&a.url, &hs, None) } else { http::request(&a.method, &a.url, &hs, a.body.as_deref(), None) }
        .map_err(|e| format!("{e:#}"))?;
    let mut body = Vec::new();
    let mut rd = r.body.take(a.max + 1);
    rd.read_to_end(&mut body).map_err(|e| e.to_string())?;
    let truncated = body.len() as u64 > a.max;
    body.truncate(a.max as usize);
    let headers: serde_json::Map<String, Value> = r.headers.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    let mut out = json!({"status": r.status, "headers": headers, "truncated": truncated});
    match String::from_utf8(body) {
        Ok(t) => out["text"] = json!(t),
        Err(e) => out["bytes"] = json!(e.into_bytes()),
    }
    Ok(out)
}

fn remote(method: &str, p: &Value, t: SshTarget) -> Answer {
    let path = PathBuf::from(p["path"].as_str().ok_or("params.path missing")?);
    if !path.is_absolute() {
        return Err("use an absolute path".into());
    }
    let r = mouser_core::fetch::Remote::new(t);
    match method {
        "remote.list" => {
            let out = r.run(&mouser_core::remote::listing_command(&path))?;
            let v = mouser_core::remote::parse_ls(&String::from_utf8_lossy(&out), &path);
            Ok(Value::Array(v.into_iter().map(|e| json!({"name": e.name, "kind": format!("{:?}", e.kind).to_lowercase(), "size": e.size})).collect()))
        }
        _ => {
            let off = p["offset"].as_u64().unwrap_or(0);
            let len = p["len"].as_u64().unwrap_or(MAX_READ).min(MAX_READ);
            let data = r.read_range(&path, off, off + len)?;
            Ok(json!({"data": data}))
        }
    }
}

/// Paths no plugin reads (EVENTS.md 13.5): `~/.ssh`, `~/.gnupg`, keychains,
/// Mouser's secrets and config.
pub fn denied(p: &Path) -> bool {
    let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    let mut deny: Vec<PathBuf> = Vec::new();
    if let Some(h) = &home {
        deny.extend([".ssh", ".gnupg", "Library/Keychains", ".local/share/keyrings"].iter().map(|d| h.join(d)));
    }
    let mh = mouser_core::storage::resolve_home();
    deny.extend([mh.join("secrets"), mh.join("config.toml"), mh.join("plugins/permissions.toml")]);
    deny.iter().any(|d| {
        let d = d.canonicalize().unwrap_or_else(|_| d.clone());
        p.starts_with(&d)
    })
}

fn fs(op: &Fs) -> Result<Value, String> {
    let path = match op {
        Fs::Read { path, .. } | Fs::Stat { path, .. } | Fs::List { path, .. } => PathBuf::from(path),
        _ => return Err("not available yet".into()),
    };
    if !path.is_absolute() {
        return Err("use an absolute path".into());
    }
    if denied(&path) {
        return Err(format!("{} is off limits to plugins", path.display()));
    }
    let io = |e: std::io::Error| e.to_string();
    match op {
        Fs::Read { offset, len, .. } => {
            use std::io::{Read, Seek, SeekFrom};
            let mut f = std::fs::File::open(&path).map_err(io)?;
            let size = f.metadata().map_err(io)?.len();
            f.seek(SeekFrom::Start(*offset)).map_err(io)?;
            let want = len.unwrap_or(MAX_READ).min(MAX_READ);
            let mut data = Vec::new();
            f.take(want).read_to_end(&mut data).map_err(io)?;
            let eof = offset + data.len() as u64 >= size;
            Ok(json!({"data": data, "eof": eof, "size": size}))
        }
        Fs::Stat { follow, .. } => {
            let m = if *follow { std::fs::metadata(&path) } else { std::fs::symlink_metadata(&path) }.map_err(io)?;
            let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs());
            let kind = if m.is_dir() {
                "dir"
            } else if m.file_type().is_symlink() {
                "symlink"
            } else {
                "file"
            };
            Ok(json!({"kind": kind, "size": m.len(), "mtime": mtime, "readonly": m.permissions().readonly()}))
        }
        Fs::List { hidden, .. } => {
            let flags = mouser_core::commander::Flags { all: *hidden, ..Default::default() };
            let v = mouser_core::commander::list_dir(&path, &flags).map_err(io)?;
            Ok(Value::Array(v.into_iter().map(|e| json!({"name": e.name, "kind": format!("{:?}", e.kind).to_lowercase(), "size": e.size})).collect()))
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(method: &str, params: Value) -> Call {
        Call::Service { method: method.into(), params }
    }

    #[test]
    fn calls_are_gated_by_permissions() {
        let none: Vec<String> = vec![];
        assert!(answer(&svc("prefs.set", json!({"key": "theme", "value": "x"})), &none).unwrap_err().contains("not allowed"));
        assert!(answer(&svc("agent.send", json!({})), &none).is_err());
        assert_eq!(answer(&svc("permissions.granted", json!({})), &["files.read".into()]).unwrap(), json!(["files.read"]));
        let read = Call::Fs { target: Target::Local, op: Fs::Stat { path: "/".into(), follow: true } };
        assert!(answer(&read, &none).unwrap_err().contains("files.read"));
        let rm = Call::Fs { target: Target::Local, op: Fs::Remove { path: "/tmp/x".into(), recursive: false, to_trash: true } };
        assert!(answer(&rm, &["files.read".into(), "files.write".into()]).is_err(), "no mutations yet");
        assert!(answer(&svc("fetch", json!({"url": "http://x"})), &none).unwrap_err().contains("not allowed"));
        let net = vec!["net.fetch".to_string()];
        assert!(answer(&svc("fetch", json!({"url": "file:///etc/passwd"})), &net).unwrap_err().contains("http"));
        assert!(answer(&svc("fetch", json!({"url": "http://x", "headers": {"X": "a\r\nB: c"}})), &net).unwrap_err().contains("header"));
        let rr = vec!["remote.read".to_string()];
        assert_eq!(answer(&svc("remote.location", json!({})), &rr).unwrap(), json!({"remote": false}));
        assert!(answer(&svc("remote.list", json!({"path": "/"})), &rr).unwrap_err().contains("not on a remote"));
        let m = answer(&svc("agent.models", json!({})), &["agent.use".into()]).unwrap();
        assert!(m["providers"].as_array().unwrap().iter().all(|p| p.get("key").is_none()));
    }

    #[test]
    fn fetch_reads_a_local_http_server() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut s, _) = l.accept().unwrap();
            let mut b = [0u8; 1024];
            let _ = s.read(&mut b);
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX-Y: z\r\n\r\nhello");
        });
        let r = answer(&svc("fetch", json!({"url": format!("http://127.0.0.1:{port}/a")})), &["net.fetch".into()]).unwrap();
        assert_eq!((r["status"].as_u64(), r["text"].as_str(), r["headers"]["x-y"].as_str()), (Some(200), Some("hello"), Some("z")));
    }

    #[test]
    fn files_read_stat_list_and_deny_list() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("a.txt"), b"hello world").unwrap();
        let g = vec!["files.read".to_string()];
        let f = |op| answer(&Call::Fs { target: Target::Active, op }, &g);
        let p = t.path().join("a.txt").display().to_string();
        let r = f(Fs::Read { path: p.clone(), offset: 6, len: Some(3) }).unwrap();
        assert_eq!(r["data"], json!(b"wor".to_vec()));
        assert_eq!((r["eof"].as_bool(), r["size"].as_u64()), (Some(false), Some(11)));
        assert_eq!(f(Fs::Stat { path: p, follow: true }).unwrap()["kind"], "file");
        let l = f(Fs::List { path: t.path().display().to_string(), glob: None, hidden: false, cursor: None }).unwrap();
        assert_eq!(l[0]["name"], "a.txt");
        assert!(f(Fs::Read { path: "relative".into(), offset: 0, len: None }).is_err());
        let ssh = std::env::var("HOME").map(|h| format!("{h}/.ssh/id_ed25519")).unwrap_or_default();
        if !ssh.is_empty() {
            assert!(f(Fs::Read { path: ssh, offset: 0, len: None }).unwrap_err().contains("off limits"));
        }
    }

    #[test]
    fn secrets_are_never_served() {
        let g = vec!["prefs.write".to_string()];
        let e = answer(&svc("prefs.set", json!({"key": "models.local.key", "value": "sk-x"})), &g).unwrap_err();
        assert!(e.contains("secret_prompt"), "{e}");
        let s = answer(&svc("prefs.schema", json!({})), &g).unwrap();
        let k = s.as_array().unwrap().iter().find(|f| f["key"] == "models.local.key").unwrap();
        assert_eq!(k["kind"]["type"], "secret");
        assert!(secret_field(&json!({"key": "theme"})).is_err());
        assert!(secret_field(&json!({"key": "models.local.key"})).is_ok());
    }
}
