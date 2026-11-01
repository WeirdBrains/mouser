//! The tools Mouser's MCP server offers (docs/mcp.md): tabs, screen, scrollback,
//! typing/running in a shell, and the commander's file ops. Reads need the token
//! only; typing, running and file changes go through the permission broker and
//! Mouser's approval popup (key `mcp:<client name>`).

use crate::{AppState, approval_ui, tabs, wake};
use mouser_core::fileops;
use mouser_core::permissions::{Broker, Check, Scope};
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a tool call waits for the user to answer the popup.
pub const APPROVAL_WAIT: Duration = Duration::from_secs(300);

pub struct AppTools {
    pub tabs: Arc<Mutex<tabs::Tabs>>,
    pub app_state: Arc<Mutex<AppState>>,
}

fn tool(name: &str, desc: &str, props: Value, required: &[&str], read_only: bool) -> Value {
    json!({
        "name": name,
        "description": desc,
        "inputSchema": {"type": "object", "properties": props, "required": required},
        "annotations": {"readOnlyHint": read_only, "destructiveHint": !read_only && name.contains("trash"), "openWorldHint": false}
    })
}

pub fn descriptors() -> Vec<Value> {
    let tab = json!({"type": "integer", "description": "tab index from tabs_list (default: the active tab)"});
    let path = json!({"type": "string", "description": "absolute, or relative to the tab's local folder"});
    vec![
        tool("tabs_list", "List Mouser's tabs: index, title, active, local folder or remote host.", json!({}), &[], true),
        tool("screen_read", "Visible text of a tab's screen.", json!({"tab": tab}), &[], true),
        tool(
            "scrollback_read",
            "The last lines of a tab's scrollback (above the screen).",
            json!({"tab": tab, "lines": {"type": "integer", "description": "how many (default 200, at most 5000)"}}),
            &[],
            true,
        ),
        tool(
            "shell_type",
            "Type text into a tab as keystrokes (no Enter added). Asks the user in Mouser first.",
            json!({"tab": tab, "text": {"type": "string"}}),
            &["text"],
            false,
        ),
        tool(
            "shell_run",
            "Run a command line in a tab's shell (typed, then Enter). Only when the shell is at its prompt. Asks the user in Mouser first.",
            json!({"tab": tab, "command": {"type": "string"}}),
            &["command"],
            false,
        ),
        tool(
            "files_list",
            "List a folder (name, kind, size, mtime).",
            json!({"tab": tab, "path": path, "hidden": {"type": "boolean"}}),
            &["path"],
            true,
        ),
        tool("files_read", "Read a text file (up to 1 MiB).", json!({"tab": tab, "path": path, "max_bytes": {"type": "integer"}}), &["path"], true),
        tool("files_mkdir", "Create a folder (and parents). Asks first.", json!({"tab": tab, "path": path}), &["path"], false),
        tool(
            "files_copy",
            "Copy a file or folder (never overwrites). Asks first.",
            json!({"tab": tab, "from": path, "to": path}),
            &["from", "to"],
            false,
        ),
        tool("files_move", "Move or rename (never overwrites). Asks first.", json!({"tab": tab, "from": path, "to": path}), &["from", "to"], false),
        tool(
            "files_trash",
            "Move to Mouser's trash ($MOUSER_HOME/trash); nothing is deleted for good. Asks first.",
            json!({"tab": tab, "path": path}),
            &["path"],
            false,
        ),
    ]
}

type R = std::result::Result<Value, String>;

fn s(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

/// Control characters shown visibly in the popup.
fn visible(t: &str) -> String {
    let v: String = t
        .chars()
        .map(|c| {
            if c == '\r' || c == '\n' {
                '⏎'
            } else if c.is_control() {
                '·'
            } else {
                c
            }
        })
        .collect();
    if v.chars().count() > 200 { format!("{}…", v.chars().take(200).collect::<String>()) } else { v }
}

impl AppTools {
    fn session(&self, a: &Value) -> std::result::Result<(usize, Arc<tabs::Session>), String> {
        let t = self.tabs.lock().unwrap();
        let i = match a.get("tab").and_then(Value::as_u64) {
            Some(i) => i as usize,
            None => t.list.iter().position(|s| Some(s.id) == t.active().map(|a| a.id)).unwrap_or(0),
        };
        t.list.get(i).cloned().map(|s| (i, s)).ok_or_else(|| format!("no tab {i} (see tabs_list)"))
    }

    fn path(&self, a: &Value, key: &str) -> std::result::Result<PathBuf, String> {
        let p = PathBuf::from(a.get(key).and_then(Value::as_str).ok_or(format!("{key} is required"))?);
        if p.is_absolute() {
            return Ok(p);
        }
        let (_, sess) = self.session(a)?;
        match sess.place() {
            Some(mouser_core::remote::Place::Local(d)) => Ok(d.join(p)),
            _ => Err("relative paths need a tab with a local folder; give an absolute path".into()),
        }
    }

    /// The broker, then the popup (blocking this call until the user answers).
    fn ask(&self, client: &str, cap: &str, detail: Vec<String>) -> std::result::Result<(), String> {
        let who = format!("mcp:{client}");
        let caps = vec![cap.to_string()];
        match Broker::check(&who, &caps, &Scope::Installed) {
            Check::Allow => Ok(()),
            Check::Deny => Err(format!("the user set {cap} to never for {who}")),
            Check::Ask(_) => {
                let (tx, rx) = std::sync::mpsc::channel();
                self.app_state.lock().unwrap().approvals.push(approval_ui::Request::new(
                    &who,
                    caps,
                    detail,
                    Scope::Installed,
                    approval_ui::Origin::Mcp { reply: tx },
                ));
                wake();
                match rx.recv_timeout(APPROVAL_WAIT) {
                    Ok(true) => Ok(()),
                    Ok(false) => Err("the user denied it".into()),
                    Err(_) => Err("no answer from the user in time".into()),
                }
            }
        }
    }

    fn may_read(&self, client: &str, cap: &str) -> std::result::Result<(), String> {
        match Broker::check(&format!("mcp:{client}"), &[cap.to_string()], &Scope::Installed) {
            Check::Deny => Err(format!("the user set {cap} to never for mcp:{client}")),
            _ => Ok(()),
        }
    }
}

impl mouser_core::mcp_server::Tools for AppTools {
    fn list(&self) -> Vec<Value> {
        descriptors()
    }

    fn call(&self, client: &str, name: &str, a: &Value) -> R {
        match name {
            "tabs_list" => {
                let t = self.tabs.lock().unwrap();
                let active = t.active().map(|a| a.id);
                let v: Vec<Value> = t
                    .list
                    .iter()
                    .enumerate()
                    .map(|(i, x)| {
                        let (folder, remote) = match x.place() {
                            Some(mouser_core::remote::Place::Local(d)) => (Some(d.display().to_string()), None),
                            Some(mouser_core::remote::Place::Remote { label, .. }) => (None, Some(label)),
                            None => (None, None),
                        };
                        json!({"index": i, "title": x.title(), "active": Some(x.id) == active, "folder": folder, "remote": remote, "at_prompt": x.shell_in_foreground()})
                    })
                    .collect();
                Ok(json!({"tabs": v}))
            }
            "screen_read" => {
                self.may_read(client, "screen.read")?;
                let (i, x) = self.session(a)?;
                let lines = x.parser.lock().unwrap().screen.visible_lines();
                let mut lines: Vec<String> = lines.into_iter().map(|l| l.trim_end().to_string()).collect();
                while lines.last().is_some_and(String::is_empty) {
                    lines.pop();
                }
                Ok(json!({"tab": i, "text": lines.join("\n")}))
            }
            "scrollback_read" => {
                self.may_read(client, "scrollback.read")?;
                let (i, x) = self.session(a)?;
                let n = a.get("lines").and_then(Value::as_u64).unwrap_or(200).min(5000) as usize;
                let total = x.scrollback.total_lines();
                let start = total.saturating_sub(n);
                let lines: Vec<String> = x.scrollback.get_lines(start, total - start).into_iter().map(|l| l.trim_end().to_string()).collect();
                Ok(json!({"tab": i, "first_line": start, "total_lines": total, "text": lines.join("\n")}))
            }
            "shell_type" | "shell_run" => {
                let (i, x) = self.session(a)?;
                let run = name == "shell_run";
                let text = a.get(if run { "command" } else { "text" }).and_then(Value::as_str).ok_or("text/command is required")?.to_string();
                if run && (text.contains('\n') || text.contains('\r')) {
                    return Err("one command line, without newlines".into());
                }
                if run && x.shell_in_foreground() == Some(false) {
                    return Err("another program is running in that tab; not typing the command".into());
                }
                let what = if run { "Run" } else { "Type" };
                self.ask(client, "shell.type", vec![format!("{what} in tab {i} ({}):", x.title()), format!("  {}", visible(&text))])?;
                let mut w = x.writer.clone();
                let bytes = if run { format!("{text}\r") } else { text };
                w.write_all(bytes.as_bytes()).and_then(|_| w.flush()).map_err(s)?;
                Ok(json!({"tab": i, "sent": true}))
            }
            "files_list" => {
                self.may_read(client, "files.read")?;
                fileops::list(&self.path(a, "path")?, a["hidden"] == true).map_err(s)
            }
            "files_read" => {
                self.may_read(client, "files.read")?;
                fileops::read(&self.path(a, "path")?, a.get("max_bytes").and_then(Value::as_u64)).map_err(s)
            }
            "files_mkdir" => {
                let p = self.path(a, "path")?;
                self.ask(client, "files.write", vec![format!("Create folder {}", p.display())])?;
                fileops::mkdir(&p).map_err(s)?;
                Ok(json!({"created": p.display().to_string()}))
            }
            "files_copy" | "files_move" => {
                let (f, t) = (self.path(a, "from")?, self.path(a, "to")?);
                let verb = if name == "files_copy" { "Copy" } else { "Move" };
                self.ask(client, "files.write", vec![format!("{verb} {}", f.display()), format!("  to {}", t.display())])?;
                if name == "files_copy" { fileops::copy(&f, &t) } else { fileops::rename(&f, &t) }.map_err(s)?;
                Ok(json!({"from": f.display().to_string(), "to": t.display().to_string()}))
            }
            "files_trash" => {
                let p = self.path(a, "path")?;
                self.ask(client, "files.write", vec![format!("Move {} to Mouser's trash", p.display())])?;
                let to = fileops::trash(&p).map_err(s)?;
                Ok(json!({"trashed": p.display().to_string(), "now_at": to.display().to_string()}))
            }
            _ => Err(format!("unknown tool {name}")),
        }
    }
}

/// Start the server per `[mcp.server]`; returns notices for the user.
pub fn start(tabs: Arc<Mutex<tabs::Tabs>>, app_state: Arc<Mutex<AppState>>) -> Vec<String> {
    let Some(cfg) = mouser_core::storage::load_preferences().mcp.and_then(|m| m.server).filter(|s| s.enabled) else { return vec![] };
    let mut notes = Vec::new();
    let (token, store) = match mouser_core::secrets::mcp_token() {
        Ok(t) => t,
        Err(e) => return vec![format!("[mouser] MCP server not started: no token ({e:#})")],
    };
    if let mouser_core::secrets::Store::File(p) = &store {
        notes.push(format!("[mouser] warning: no OS keychain; the MCP token is in {} (0600)", p.display()));
    }
    let server = mouser_core::mcp_server::Server::new(Arc::new(AppTools { tabs, app_state }), token, env!("CARGO_PKG_VERSION"));
    match mouser_core::mcp_server::bind(cfg.port) {
        Ok(l) => {
            let s = server.clone();
            std::thread::spawn(move || mouser_core::mcp_server::serve(l, None, s));
            notes.push(format!("[mouser] MCP server on http://127.0.0.1:{}/mcp", cfg.port));
        }
        Err(e) => notes.push(format!("[mouser] MCP HTTP: {e:#}")),
    }
    if cfg.https {
        match mouser_core::mcp_server::tls_config(cfg.cert.as_deref(), cfg.key.as_deref())
            .and_then(|t| mouser_core::mcp_server::bind(cfg.https_port).map(|l| (t, l)))
        {
            Ok(((tls, _, _), l)) => {
                std::thread::spawn(move || mouser_core::mcp_server::serve(l, Some(tls), server));
                notes.push(format!("[mouser] MCP server on https://127.0.0.1:{}/mcp", cfg.https_port));
            }
            Err(e) => notes.push(format!("[mouser] MCP HTTPS: {e:#}")),
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    #[test]
    fn descriptors_are_valid_and_mutating_ones_say_they_ask() {
        let d = super::descriptors();
        assert_eq!(d.len(), 11);
        for t in &d {
            let n = t["name"].as_str().unwrap();
            assert!(n.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{n}");
            assert_eq!(t["inputSchema"]["type"], "object");
            if t["annotations"]["readOnlyHint"] == false {
                assert!(t["description"].as_str().unwrap().contains("Asks"), "{n}");
            }
        }
        assert_eq!(super::visible("ls\r\x1b[A"), "ls⏎·[A");
    }
}
