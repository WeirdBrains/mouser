//! Host calls from mode plugins (`Out::Request`, docs/mode-protocol.md),
//! answered here for `transport = "mode"` plugins. Every call is checked
//! against the plugin's granted permissions first.
//!
//! Available now: preferences (`prefs.schema`, `prefs.get`, `prefs.set` with
//! `prefs.write`; secrets are never read or written by plugins),
//! `permissions.granted`, and local read-only files with `files.read`
//! (`Fs::Read` up to 1 MiB per call, `Fs::Stat`, `Fs::List`), never inside the
//! secrets deny-list. Everything else answers "not available yet".

use mouser_core::prefs_edit::{self, Kind};
use mouser_mode_api::host_api::{Call, Fs, Target};
use mouser_mode_api::services;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Largest `Fs::Read` answer.
pub const MAX_READ: u64 = 1 << 20;

fn has(granted: &[String], p: &str) -> bool {
    granted.iter().any(|g| g == p)
}

/// Answer one call for a plugin with these grants.
pub fn answer(call: &Call, granted: &[String]) -> Result<Value, String> {
    match call {
        Call::Service { method, params } => {
            if !services::allowed(method, granted) {
                return Err(format!("{method} is not allowed (permission missing or unknown method)"));
            }
            service(method, params, granted)
        }
        Call::Fs { target, op } => {
            if !matches!(target, Target::Local | Target::Active) {
                return Err("remote files are not available yet".into());
            }
            if op.mutates() {
                return Err("changing files is not available yet".into());
            }
            if !has(granted, "files.read") {
                return Err("needs files.read".into());
            }
            fs(op)
        }
        Call::Spawn { spec, .. } => {
            spec.check_escalation()?;
            Err("spawn is not available yet; a plugin may start its own processes".into())
        }
        _ => Err("not available yet".into()),
    }
}

fn service(method: &str, params: &Value, granted: &[String]) -> Result<Value, String> {
    let fields = prefs_edit::schema();
    let field = |params: &Value| {
        let key = params["key"].as_str().ok_or("params.key missing")?;
        let f = fields.iter().find(|f| f.key == key).ok_or_else(|| format!("no preference {key}"))?;
        if matches!(f.kind, Kind::Secret(_)) {
            return Err(format!("{key} is a secret; plugins can't read or change it"));
        }
        Ok(f.clone())
    };
    match method {
        "permissions.granted" => Ok(json!(granted)),
        "prefs.schema" => Ok(Value::Array(
            fields
                .iter()
                .filter(|f| !matches!(f.kind, Kind::Secret(_)))
                .map(|f| json!({"section": f.section, "key": f.key, "label": f.label, "default": f.default, "live": f.live, "kind": format!("{:?}", f.kind)}))
                .collect(),
        )),
        "prefs.get" => {
            let f = field(params)?;
            let doc = prefs_edit::load_doc().map_err(|e| e.to_string())?;
            Ok(json!(prefs_edit::get(&doc, &f)))
        }
        "prefs.set" => {
            let f = field(params)?;
            let raw = params["value"].as_str().map(str::to_string).unwrap_or_else(|| params["value"].to_string());
            let mut doc = prefs_edit::load_doc().map_err(|e| e.to_string())?;
            prefs_edit::set(&mut doc, &f, &raw).map_err(|e| e.to_string())?;
            prefs_edit::save(&doc).map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        m => Err(format!("{m} is not available yet")),
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
        let e = answer(&svc("prefs.get", json!({"key": "models.local.key"})), &g).unwrap_err();
        assert!(e.contains("secret"), "{e}");
        let s = answer(&svc("prefs.schema", json!({})), &g).unwrap();
        assert!(s.as_array().unwrap().iter().all(|f| f["key"] != "models.local.key"));
    }
}
