//! The tools Mouser's MCP server offers (docs/mcp.md): tabs, screen, scrollback,
//! typing/running in a shell, the commander's file ops, and the modes'
//! actions (the same registry as their keys and context menus). Reads need the token
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
    /// The caller already asked (the chat's approval card); skip the popup.
    pub preapproved: bool,
}

fn tool(name: &str, desc: &str, props: Value, required: &[&str], read_only: bool) -> Value {
    json!({
        "name": name,
        "description": desc,
        "inputSchema": {"type": "object", "properties": props, "required": required},
        "annotations": {"readOnlyHint": read_only, "destructiveHint": tool_destructive(name), "openWorldHint": false}
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
        tool(
            "files_move",
            "Alias of file_move. Move or rename (never overwrites). Asks first.",
            json!({"tab": tab, "from": path, "to": path}),
            &["from", "to"],
            false,
        ),
        tool(
            "file_move",
            "Move or rename a file or folder (never overwrites). files_move is the same tool. Asks first.",
            json!({"tab": tab, "from": path, "to": path}),
            &["from", "to"],
            false,
        ),
        tool(
            "file_sync",
            "Sync two local paths. direction is push (from → to), pull (to → from) or both (default). conflict is newer (default; the strictly newer mtime wins) or refuse (leave both, report the clash). A file that would be replaced is moved to Mouser's trash first. dry_run lists the changes and writes nothing. Directories need recursive (default true). Asks first.",
            json!({
                "tab": tab, "from": path, "to": path,
                "direction": {"type": "string", "description": "push, pull or both (default both)"},
                "conflict": {"type": "string", "description": "newer (default) or refuse"},
                "recursive": {"type": "boolean", "description": "descend into directories (default true)"},
                "dry_run": {"type": "boolean", "description": "list changes and write nothing"}
            }),
            &["from", "to"],
            false,
        ),
        tool(
            "remote_file_sync",
            "Sync a local path with a path on a remote host (the tab's ssh, or host). push uploads, pull downloads, both (default) copies whichever side is missing or newer. Same conflict, trash, dry_run and recursive rules as file_sync. Asks first.",
            json!({
                "tab": tab,
                "local": path,
                "remote": {"type": "string", "description": "path on the remote host"},
                "host": {"type": "string", "description": "user@host or host; omitted: the tab's remote shell"},
                "port": {"type": "integer", "description": "ssh port when host is set"},
                "direction": {"type": "string", "description": "push (upload), pull (download) or both (default)"},
                "conflict": {"type": "string", "description": "newer (default) or refuse"},
                "recursive": {"type": "boolean", "description": "descend into directories (default true)"},
                "dry_run": {"type": "boolean", "description": "list changes and write nothing"}
            }),
            &["local", "remote"],
            false,
        ),
        tool(
            "file_download",
            "Download a remote file or directory (recursive, default true) onto a local path. Same as remote_file_sync direction pull: a newer local file is kept, otherwise the local file goes to the trash and the remote copy replaces it. dry_run lists that and writes nothing. Asks first.",
            json!({
                "tab": tab,
                "remote": {"type": "string", "description": "path on the remote host"},
                "to": path,
                "host": {"type": "string", "description": "user@host or host; omitted: the tab's remote shell"},
                "port": {"type": "integer"},
                "conflict": {"type": "string", "description": "newer (default) or refuse"},
                "recursive": {"type": "boolean", "description": "descend into directories (default true)"},
                "dry_run": {"type": "boolean"}
            }),
            &["remote", "to"],
            false,
        ),
        tool(
            "file_upload",
            "Upload a local file or directory (recursive, default true) onto a remote path. Same as remote_file_sync direction push. A replaced remote file's previous bytes go to Mouser's trash. dry_run lists that and writes nothing. Asks first.",
            json!({
                "tab": tab,
                "from": path,
                "remote": {"type": "string", "description": "path on the remote host"},
                "host": {"type": "string", "description": "user@host or host; omitted: the tab's remote shell"},
                "port": {"type": "integer"},
                "conflict": {"type": "string", "description": "newer (default) or refuse"},
                "recursive": {"type": "boolean", "description": "descend into directories (default true)"},
                "dry_run": {"type": "boolean"}
            }),
            &["from", "remote"],
            false,
        ),
        tool("list_themes", "Every built-in and user theme, and which one is active.", json!({}), &[], true),
        tool(
            "get_theme",
            "One theme's dark and light values (colours, cursor, accent, palette, selection).",
            json!({"name": {"type": "string"}}),
            &["name"],
            true,
        ),
        tool(
            "set_theme",
            "Make a theme the active one: saved in preferences and applied to this window on the next frame. Asks first.",
            json!({"name": {"type": "string"}}),
            &["name"],
            false,
        ),
        tool(
            "create_theme",
            "Create a user theme. name is required. Missing colours fall back to Red Star. variant is dark, light or both (default both). A short palette is padded with the default ANSI colours. A bad colour is an error and nothing is written. Asks first.",
            json!({
                "name": {"type": "string"},
                "variant": {"type": "string", "description": "dark, light or both (default both)"},
                "foreground": {"type": "string", "description": "#rrggbb"},
                "background": {"type": "string"},
                "cursor": {"type": "string"},
                "accent": {"type": "string"},
                "selection_foreground": {"type": "string"},
                "selection_background": {"type": "string"},
                "palette": {"type": "array", "items": {"type": "string"}, "description": "up to 16 #rrggbb colours"}
            }),
            &["name"],
            false,
        ),
        tool(
            "duplicate_theme",
            "Copy a theme. name is the copy's name; omitted, it is \"<theme> copy\" (then \"<theme> copy 2\", …). Asks first.",
            json!({"theme": {"type": "string"}, "name": {"type": "string", "description": "name of the copy"}}),
            &["theme"],
            false,
        ),
        tool(
            "edit_theme",
            "Merge colour fields into a user theme and save it. Omitted fields stay. variant dark, light or both (default both). Built-in themes are read-only: the error says to use duplicate_theme. Asks first.",
            json!({
                "name": {"type": "string"},
                "variant": {"type": "string"},
                "foreground": {"type": "string"},
                "background": {"type": "string"},
                "cursor": {"type": "string"},
                "accent": {"type": "string"},
                "selection_foreground": {"type": "string"},
                "selection_background": {"type": "string"},
                "palette": {"type": "array", "items": {"type": "string"}}
            }),
            &["name"],
            false,
        ),
        tool(
            "delete_theme",
            "Delete a theme. The last remaining theme is refused. A user theme's files go to Mouser's trash. A built-in is hidden (it stays in the program; themes/.hidden lists it) and any copy on disk is trashed. Deleting the active theme switches to the default if that remains, otherwise another theme, and saves that. Asks first.",
            json!({"name": {"type": "string"}}),
            &["name"],
            false,
        ),
        tool(
            "files_trash",
            "Move to Mouser's trash ($MOUSER_HOME/trash); nothing is deleted for good. Asks first.",
            json!({"tab": tab, "path": path}),
            &["path"],
            false,
        ),
        tool(
            "modes_list",
            "List Mouser's modes (commander, chat, preferences, plugins): name, hotkey, which is on and whether it has the keyboard.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "modes_switch",
            "Turn a mode on by name or hotkey (its own name again turns modes off). Asks first.",
            json!({"mode": {"type": "string"}}),
            &["mode"],
            false,
        ),
        tool(
            "mode_actions",
            "Every action of the current mode that applies now to its selected item and view state (a file, folder, image, clip; list or fullscreen view ...): the same list as its right-click menu, with id, arg, label, key, enabled and destructive.",
            json!({}),
            &[],
            true,
        ),
        tool(
            "mode_action",
            "Run one action from mode_actions (id and arg), as its key would. Asks first; destructive file changes (delete, move, rename, paste) also ask again on the mode's own approval card and never overwrite.",
            json!({"id": {"type": "string"}, "arg": {"type": "integer", "description": "from mode_actions (default 0)"}}),
            &["id"],
            false,
        ),
    ]
}

/// How long `mode_actions` waits for a mode plugin's answer.
const ACTIONS_WAIT: Duration = Duration::from_secs(3);

/// Actions that change files: shown as destructive to MCP clients.
fn destructive(id: &str) -> bool {
    let id = id.strip_prefix("menu:").unwrap_or(id);
    ["delete", "trash", "move", "rename", "paste", "cut"].iter().any(|w| id.contains(w))
}

/// Tools that can replace or remove something (the previous bytes go to the trash).
fn tool_destructive(name: &str) -> bool {
    matches!(name, "files_trash" | "file_sync" | "remote_file_sync" | "file_download" | "file_upload" | "edit_theme" | "delete_theme" | "mode_action")
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
        if self.preapproved {
            return Ok(());
        }
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

    /// The current mode's context actions (waits for a plugin's answer).
    fn mode_actions(&self) -> std::result::Result<(String, Vec<mouser_render::modes::MenuEntry>), String> {
        let name = {
            let mut st = self.app_state.lock().unwrap();
            if !st.modes.request_actions() {
                return Err("no mode is on (modes_switch)".into());
            }
            st.modes.current().unwrap_or_default().to_string()
        };
        let t0 = std::time::Instant::now();
        loop {
            if let Some(a) = self.app_state.lock().unwrap().modes.take_actions() {
                return Ok((name, a));
            }
            if t0.elapsed() > ACTIONS_WAIT {
                return Err(format!("{name} did not answer (a mode without a context menu has no actions to list)"));
            }
            wake();
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn may_read(&self, client: &str, cap: &str) -> std::result::Result<(), String> {
        match Broker::check(&format!("mcp:{client}"), &[cap.to_string()], &Scope::Installed) {
            Check::Deny => Err(format!("the user set {cap} to never for mcp:{client}")),
            _ => Ok(()),
        }
    }

    fn sync_opts(a: &Value, default_dir: &str) -> std::result::Result<mouser_core::filesync::SyncOpts, String> {
        use mouser_core::filesync::{Conflict, Direction, SyncOpts};
        Ok(SyncOpts {
            direction: Direction::parse(a.get("direction").and_then(Value::as_str).unwrap_or(default_dir))?,
            conflict: Conflict::parse(a.get("conflict").and_then(Value::as_str).unwrap_or("newer"))?,
            recursive: a.get("recursive").and_then(Value::as_bool).unwrap_or(true),
            dry_run: a.get("dry_run").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// `host` (and optional `port`), or the tab's remote ssh.
    fn ssh_of(&self, a: &Value) -> std::result::Result<mouser_core::remote::SshTarget, String> {
        if let Some(h) = a.get("host").and_then(Value::as_str) {
            if h.is_empty() || h.contains(['\n', '\0']) {
                return Err("host is empty".into());
            }
            let mut opts = Vec::new();
            if let Some(p) = a.get("port").and_then(Value::as_u64) {
                opts.push("-p".into());
                opts.push(p.to_string());
            }
            return Ok(mouser_core::remote::SshTarget { opts, dest: h.to_string() });
        }
        let (_, sess) = self.session(a)?;
        match sess.place() {
            Some(mouser_core::remote::Place::Remote { ssh: Some(t), .. }) => Ok(t),
            _ => Err("give host, or a tab whose shell is on a remote host".into()),
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
            "files_copy" | "files_move" | "file_move" => {
                let (f, t) = (self.path(a, "from")?, self.path(a, "to")?);
                let verb = if name == "files_copy" { "Copy" } else { "Move" };
                self.ask(client, "files.write", vec![format!("{verb} {}", f.display()), format!("  to {}", t.display())])?;
                if name == "files_copy" { fileops::copy(&f, &t) } else { fileops::rename(&f, &t) }.map_err(s)?;
                Ok(json!({"from": f.display().to_string(), "to": t.display().to_string()}))
            }
            "file_sync" => {
                let (f, t) = (self.path(a, "from")?, self.path(a, "to")?);
                let opts = Self::sync_opts(a, "both")?;
                let dry = if opts.dry_run { " (dry run)" } else { "" };
                let dir = opts.direction.as_str();
                self.ask(client, "files.write", vec![format!("Sync ({dir}) {}{dry}", f.display()), format!("  with {}", t.display())])?;
                mouser_core::filesync::sync_local(&f, &t, &opts).map_err(s)
            }
            "remote_file_sync" | "file_download" | "file_upload" => {
                let ssh = self.ssh_of(a)?;
                let (local, remote, default_dir, verb) = if name == "file_download" {
                    (self.path(a, "to")?, a.get("remote").and_then(Value::as_str).ok_or("remote is required")?.to_string(), "pull", "Download")
                } else if name == "file_upload" {
                    (self.path(a, "from")?, a.get("remote").and_then(Value::as_str).ok_or("remote is required")?.to_string(), "push", "Upload")
                } else {
                    (self.path(a, "local")?, a.get("remote").and_then(Value::as_str).ok_or("remote is required")?.to_string(), "both", "Sync")
                };
                let opts = Self::sync_opts(a, default_dir)?;
                let dry = if opts.dry_run { " (dry run)" } else { "" };
                let dir = opts.direction.as_str();
                self.ask(
                    client,
                    "files.write",
                    vec![format!("{verb} ({dir}) {}{dry}", local.display()), format!("  remote {remote} on {}", ssh.dest)],
                )?;
                mouser_core::filesync::sync_remote(&local, &remote, &ssh, &opts).map_err(s)
            }
            "list_themes" => {
                self.may_read(client, "themes.read")?;
                let prefs = mouser_core::storage::load_preferences();
                let active = prefs.theme.clone().unwrap_or_else(|| mouser_core::themes::default_theme_name().to_string());
                let v: Vec<Value> = mouser_core::themes::list_themes()
                    .into_iter()
                    .map(|n| json!({"name": n, "builtin": mouser_core::themes::is_builtin(&n), "active": n == active}))
                    .collect();
                Ok(json!({"themes": v, "active": active}))
            }
            "get_theme" => {
                self.may_read(client, "themes.read")?;
                let name = a.get("name").and_then(Value::as_str).ok_or("name is required")?;
                let mut v = mouser_core::themes::theme_values(name)?;
                let active = mouser_core::storage::load_preferences().theme.unwrap_or_else(|| mouser_core::themes::default_theme_name().into());
                v["active"] = json!(v["name"] == active);
                Ok(v)
            }
            "set_theme" => {
                let name = a.get("name").and_then(Value::as_str).ok_or("name is required")?;
                let name = mouser_core::themes::find_theme(name).ok_or_else(|| format!("no theme named '{name}'"))?;
                self.ask(client, "themes.write", vec![format!("Use the {name} theme")])?;
                let name = mouser_core::themes::set_active_theme(&name)?;
                self.app_state.lock().unwrap().look.reload_theme();
                wake();
                Ok(json!({"theme": name, "active": true}))
            }
            "create_theme" => {
                let name = a.get("name").and_then(Value::as_str).ok_or("name is required")?;
                self.ask(client, "themes.write", vec![format!("Create theme {name}")])?;
                mouser_core::themes::create_from_values(name, a)?;
                Ok(json!({"created": name.trim()}))
            }
            "duplicate_theme" => {
                let theme = a.get("theme").and_then(Value::as_str).ok_or("theme is required")?;
                let name = a.get("name").and_then(Value::as_str);
                let label = name.unwrap_or("(copy)");
                self.ask(client, "themes.write", vec![format!("Duplicate theme {theme} as {label}")])?;
                let created = mouser_core::themes::duplicate_named(theme, name)?;
                Ok(json!({"theme": theme, "created": created}))
            }
            "edit_theme" => {
                let name = a.get("name").and_then(Value::as_str).ok_or("name is required")?;
                let name = mouser_core::themes::find_theme(name).ok_or_else(|| format!("no theme named '{name}'"))?;
                if mouser_core::themes::is_builtin(&name) {
                    return Err(format!("'{name}' is a built-in theme and read-only; use duplicate_theme"));
                }
                self.ask(client, "themes.write", vec![format!("Edit theme {name}")])?;
                mouser_core::themes::edit_user_theme(&name, a)?;
                Ok(json!({"theme": name, "saved": true}))
            }
            "delete_theme" => {
                let name = a.get("name").and_then(Value::as_str).ok_or("name is required")?;
                let name = mouser_core::themes::find_theme(name).ok_or_else(|| format!("no theme named '{name}'"))?;
                if mouser_core::themes::list_themes().len() <= 1 {
                    return Err(format!("'{name}' is the last theme; Mouser keeps at least one"));
                }
                let mut detail = vec![format!("Delete theme {name}")];
                if mouser_core::themes::is_builtin(&name) {
                    detail.push("  a built-in: hidden, not removed from the program".into());
                } else {
                    detail.push("  its files go to Mouser's trash".into());
                }
                let active = mouser_core::storage::load_preferences().theme.unwrap_or_else(|| mouser_core::themes::default_theme_name().into());
                if active.eq_ignore_ascii_case(&name) {
                    detail.push("  it is the active theme; another theme will be saved".into());
                }
                self.ask(client, "themes.write", detail)?;
                let r = mouser_core::themes::delete_theme(&name)?;
                if r.switched {
                    self.app_state.lock().unwrap().look.reload_theme();
                    wake();
                }
                Ok(json!({
                    "theme": r.name,
                    "builtin": r.builtin,
                    "hidden": r.hidden,
                    "trashed": r.trashed.as_ref().map(|p| p.display().to_string()),
                    "active": r.active,
                    "switched": r.switched,
                }))
            }
            "files_trash" => {
                let p = self.path(a, "path")?;
                self.ask(client, "files.write", vec![format!("Move {} to Mouser's trash", p.display())])?;
                let to = fileops::trash(&p).map_err(s)?;
                Ok(json!({"trashed": p.display().to_string(), "now_at": to.display().to_string()}))
            }
            "modes_list" => {
                self.may_read(client, "screen.read")?;
                let st = self.app_state.lock().unwrap();
                let (list, focused) = st.modes.list();
                let v: Vec<Value> = list.iter().map(|(n, k, on)| json!({"name": n, "hotkey": k.to_string(), "on": on})).collect();
                Ok(json!({"modes": v, "focused": focused}))
            }
            "modes_switch" => {
                let m = a.get("mode").and_then(Value::as_str).ok_or("mode is required")?;
                self.ask(client, "modes.control", vec![format!("Switch to the {m} mode")])?;
                let r = self.app_state.lock().unwrap().modes.switch(m)?;
                wake();
                Ok(json!({"result": r}))
            }
            "mode_actions" => {
                self.may_read(client, "screen.read")?;
                let (mode, acts) = self.mode_actions()?;
                let v: Vec<Value> = acts
                    .iter()
                    .map(|e| json!({"id": e.id, "arg": e.arg, "label": e.label, "key": e.key, "enabled": e.enabled, "destructive": destructive(&e.id)}))
                    .collect();
                Ok(json!({"mode": mode, "actions": v}))
            }
            "mode_action" => {
                let id = a.get("id").and_then(Value::as_str).ok_or("id is required")?.to_string();
                let arg = a.get("arg").and_then(Value::as_i64).unwrap_or(0) as i32;
                let (mode, acts) = self.mode_actions()?;
                let label = acts.iter().find(|e| e.id == id && e.arg == arg).map_or(id.clone(), |e| e.label.clone());
                let mut detail = vec![format!("{mode}: {label}")];
                if destructive(&id) {
                    detail.push("  changes files; the mode asks again before it does".into());
                }
                self.ask(client, "modes.control", detail)?;
                let mut st = self.app_state.lock().unwrap();
                let e = st.modes.invoke(&id, arg, &acts)?;
                crate::apply_mode_effect(&mut st, e, &tabs::ActiveWriter(self.tabs.clone()));
                drop(st);
                wake();
                Ok(json!({"mode": mode, "ran": id, "arg": arg, "confirm": destructive(&id)}))
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
    let server =
        mouser_core::mcp_server::Server::new(Arc::new(AppTools { tabs, app_state, preapproved: false }), token.clone(), env!("CARGO_PKG_VERSION"));
    match mouser_core::mcp_server::bind_preferred(cfg.port) {
        Ok((l, port)) => {
            if port != cfg.port {
                // Another Mouser already holds the preferred port — do not toast.
                eprintln!("[mouser] MCP HTTP: port {} in use; using ephemeral {port}", cfg.port);
            }
            let s = server.clone();
            std::thread::spawn(move || mouser_core::mcp_server::serve(l, None, s));
            notes.push(format!("[mouser] MCP server on http://127.0.0.1:{port}/mcp"));
        }
        Err(e) => notes.push(format!("[mouser] MCP HTTP: {e:#}")),
    }
    if cfg.https {
        match start_https(&cfg, &token, server) {
            Ok(n) => notes.push(n),
            Err(e) => notes.push(format!("[mouser] MCP HTTPS: {e:#}")),
        }
    }
    notes
}

/// The HTTPS listener: self-signed, the user's files, or Let's Encrypt.
fn start_https(cfg: &mouser_core::storage::McpServerPrefs, token: &str, server: Arc<mouser_core::mcp_server::Server>) -> anyhow::Result<String> {
    use mouser_core::{mcp_acme, mcp_server};
    let addr = cfg.bind.as_deref().unwrap_or("127.0.0.1");
    let shown = if addr == "127.0.0.1" { "127.0.0.1".to_string() } else { addr.to_string() };
    match mcp_server::TlsSource::of(cfg)? {
        mcp_server::TlsSource::Acme => {
            let acme = mcp_acme::AcmeConfig::from_prefs(cfg.tls.as_ref().expect("acme source implies [mcp.server.tls]"))?;
            let store = Arc::new(mcp_acme::CertStore::default());
            server.allow_hosts(&acme.domains);
            let (l, https_port) = mcp_server::bind_addr_preferred(addr, cfg.https_port, true, token)?;
            if https_port != cfg.https_port {
                eprintln!("[mouser] MCP HTTPS: port {} in use; using ephemeral {https_port}", cfg.https_port);
            }
            let tls = mcp_acme::server_config(store.clone())?;
            std::thread::spawn(move || mcp_server::serve(l, Some(tls), server));
            let mut extra = String::new();
            if acme.challenge == mcp_acme::Challenge::Http01 {
                // Answers only /.well-known/acme-challenge/; never MCP.
                let l = std::net::TcpListener::bind((addr, acme.http_port))
                    .map_err(|e| anyhow::anyhow!("http-01 listener on {addr}:{}: {e}", acme.http_port))?;
                let s = store.clone();
                std::thread::spawn(move || mcp_acme::serve_http01(l, s));
                extra = format!(", http-01 on :{}", acme.http_port);
            }
            let name = acme.domains[0].clone();
            let ca = acme.ca_name().to_string();
            mcp_acme::spawn_renewer(acme, mcp_acme::default_dir(), store);
            Ok(format!("[mouser] MCP server on https://{name}:{https_port}/mcp ({ca} certificate, renewed automatically{extra})"))
        }
        src => {
            let (cert, key) = if src == mcp_server::TlsSource::Files { (cfg.cert.as_deref(), cfg.key.as_deref()) } else { (None, None) };
            let (tls, _, _) = mcp_server::tls_config(cert, key)?;
            server.allow_hosts(&mcp_server::tls_domains(cfg));
            let (l, https_port) = mcp_server::bind_addr_preferred(addr, cfg.https_port, true, token)?;
            if https_port != cfg.https_port {
                eprintln!("[mouser] MCP HTTPS: port {} in use; using ephemeral {https_port}", cfg.https_port);
            }
            std::thread::spawn(move || mcp_server::serve(l, Some(tls), server));
            Ok(format!("[mouser] MCP server on https://{shown}:{https_port}/mcp"))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn descriptors_are_valid_and_mutating_ones_say_they_ask() {
        let d = super::descriptors();
        let names: Vec<&str> = d.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for n in [
            "tabs_list",
            "screen_read",
            "scrollback_read",
            "shell_type",
            "shell_run",
            "files_list",
            "files_read",
            "files_mkdir",
            "files_copy",
            "files_move",
            "file_move",
            "file_sync",
            "remote_file_sync",
            "file_download",
            "file_upload",
            "list_themes",
            "get_theme",
            "set_theme",
            "create_theme",
            "duplicate_theme",
            "edit_theme",
            "delete_theme",
            "modes_list",
            "modes_switch",
            "mode_actions",
            "mode_action",
        ] {
            assert!(names.contains(&n), "missing {n}");
        }
        assert_eq!(names.len(), 27);
        assert!(!super::destructive("open") && super::destructive("delete") && super::destructive("menu:move_to"));
        assert!(super::tool_destructive("file_sync") && super::tool_destructive("file_download") && !super::tool_destructive("file_move"));
        assert!(super::tool_destructive("edit_theme") && super::tool_destructive("delete_theme") && !super::tool_destructive("set_theme"));
        for t in &d {
            let n = t["name"].as_str().unwrap();
            assert!(n.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{n}");
            assert_eq!(t["inputSchema"]["type"], "object");
            if t["annotations"]["readOnlyHint"] == false {
                assert!(t["description"].as_str().unwrap().contains("Asks"), "{n}");
            }
            if n == "files_move" || n == "file_move" {
                assert!(t["description"].as_str().unwrap().contains("file_move") || t["description"].as_str().unwrap().contains("files_move"));
            }
        }
        assert_eq!(super::visible("ls\r\x1b[A"), "ls⏎·[A");
    }
}
