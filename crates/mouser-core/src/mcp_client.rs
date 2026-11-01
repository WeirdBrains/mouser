//! MCP client manager (P3): keeps a connection to every `[[mcp.servers]]`
//! entry and every enabled MCP plugin, reconnecting with backoff. Tool calls
//! are routed through the permission broker by the caller ([`broker_name`],
//! [`tool_cap`]); MCP plugins also need their `run` permission to start.

use crate::mcp::{Client, Tool};
use crate::permissions::{Broker, Check, Scope};
use crate::storage::McpServerEntry;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub const PING_EVERY: Duration = Duration::from_secs(30);
pub const MAX_BACKOFF: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Config(McpServerEntry),
    /// A P1 plugin with `transport = "mcp"`: (dir, argv).
    Plugin(PathBuf, Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Connected,
    /// Why, and seconds until the next try.
    Failed(String, u64),
    /// An MCP plugin waiting for its `run` permission (click to ask).
    NeedsApproval,
    Denied,
    Disabled,
}

impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Status::Connecting => "connecting",
            Status::Connected => "connected",
            Status::Failed(..) => "failed",
            Status::NeedsApproval => "ask",
            Status::Denied => "denied",
            Status::Disabled => "off",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// `name` for config servers, `plugin:name` for plugins.
    pub id: String,
    pub source: Source,
    pub status: Status,
    pub tools: Vec<Tool>,
    /// Server name/version once connected.
    pub info: String,
}

/// The broker "plugin" name for a server id.
pub fn broker_name(id: &str) -> String {
    match id.strip_prefix("plugin:") {
        Some(p) => p.to_string(),
        None => format!("mcp-server:{id}"),
    }
}

/// The capability for calling `tool`.
pub fn tool_cap(tool: &str) -> String {
    format!("tool.{tool}")
}

/// Check (without asking) whether `id` may call `tool`.
pub fn check_tool(id: &str, tool: &str) -> Check {
    Broker::check(&broker_name(id), &[tool_cap(tool)], &Scope::Installed)
}

/// Every server Mouser should connect to: config entries, then MCP plugins.
pub fn sources() -> Vec<(String, Source)> {
    let mut out: Vec<(String, Source)> = crate::storage::load_preferences()
        .mcp
        .map(|m| m.servers)
        .unwrap_or_default()
        .into_iter()
        .map(|e| (e.name.clone(), Source::Config(e)))
        .collect();
    for i in crate::plugins::installed().into_iter().flatten() {
        if i.manifest.run.transport == crate::plugins::Transport::Mcp && crate::plugins::is_enabled(&i.manifest.name) {
            out.push((format!("plugin:{}", i.manifest.name), Source::Plugin(i.dir.clone(), i.argv())));
        }
    }
    out
}

/// Resolve a `keychain:NAME` token reference.
pub fn resolve_token(r: &str) -> Result<String> {
    let name = r.strip_prefix("keychain:").context("token must be a keychain:NAME reference")?;
    crate::secrets::get(name).map(|(t, _)| t).with_context(|| format!("no secret {name} in the keychain (run `mouser mcp servers add` again)"))
}

/// Connect once (no permission checks).
pub fn connect(id: &str, src: &Source) -> Result<Client> {
    match src {
        Source::Plugin(dir, argv) => Client::start(argv, dir, id.trim_start_matches("plugin:")),
        Source::Config(e) => {
            if let Some(url) = &e.url {
                let token = e.token.as_deref().map(resolve_token).transpose()?;
                Client::connect(url, token, e.ca.as_deref().map(std::path::Path::new))
            } else if !e.command.is_empty() {
                let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let mut env: Vec<(String, String)> = Vec::new();
                for (k, v) in &e.env {
                    env.push((k.clone(), if v.starts_with("keychain:") { resolve_token(v)? } else { v.clone() }));
                }
                if let Some(r) = e.token.as_deref() {
                    env.push(("MCP_TOKEN".into(), resolve_token(r)?));
                }
                Client::start_env(&e.command, &dir, &e.name, &env)
            } else {
                bail!("{}: needs a command or a url", e.name)
            }
        }
    }
}

struct Slot {
    view: Mutex<View>,
    client: Mutex<Option<Client>>,
    /// (kicked, stop, run approved once)
    flags: Mutex<(bool, bool, bool)>,
    cv: Condvar,
}

impl Slot {
    /// Wait for a kick (true) or the timeout (false).
    fn wait(&self, d: Duration) -> bool {
        let g = self.flags.lock().unwrap();
        let (mut g, _) = self.cv.wait_timeout_while(g, d, |f| !f.0 && !f.1).unwrap();
        std::mem::take(&mut g.0)
    }
    fn stopped(&self) -> bool {
        self.flags.lock().unwrap().1
    }
    fn kick(&self) {
        self.flags.lock().unwrap().0 = true;
        self.cv.notify_all();
    }
}

type Wake = Arc<dyn Fn() + Send + Sync>;

pub struct Manager {
    slots: Mutex<Vec<Arc<Slot>>>,
    wake: Wake,
}

impl Manager {
    /// Load [`sources`] and connect to each in the background; `wake` runs on every change.
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        let m = Arc::new(Manager { slots: Mutex::new(Vec::new()), wake: Arc::new(wake) });
        m.reload();
        m
    }

    /// Re-read the sources; unchanged servers keep their connection.
    pub fn reload(&self) {
        let mut slots = self.slots.lock().unwrap();
        let new = sources();
        let mut keep = Vec::new();
        for (id, src) in new {
            if let Some(s) = slots.iter().find(|s| {
                let v = s.view.lock().unwrap();
                v.id == id && v.source == src
            }) {
                keep.push(s.clone());
                continue;
            }
            let disabled = matches!(&src, Source::Config(e) if !e.enabled);
            let slot = Arc::new(Slot {
                view: Mutex::new(View {
                    id,
                    source: src,
                    status: if disabled { Status::Disabled } else { Status::Connecting },
                    tools: vec![],
                    info: String::new(),
                }),
                client: Mutex::new(None),
                flags: Mutex::new((false, false, false)),
                cv: Condvar::new(),
            });
            if !disabled {
                let (s, w) = (slot.clone(), self.wake.clone());
                std::thread::Builder::new().name("mcp-client".into()).spawn(move || run(s, w)).ok();
            }
            keep.push(slot);
        }
        for s in slots.iter() {
            if !keep.iter().any(|k| Arc::ptr_eq(k, s)) {
                s.flags.lock().unwrap().1 = true;
                s.cv.notify_all();
            }
        }
        *slots = keep;
        drop(slots);
        (self.wake)();
    }

    pub fn snapshot(&self) -> Vec<View> {
        self.slots.lock().unwrap().iter().map(|s| s.view.lock().unwrap().clone()).collect()
    }

    fn slot(&self, id: &str) -> Option<Arc<Slot>> {
        self.slots.lock().unwrap().iter().find(|s| s.view.lock().unwrap().id == id).cloned()
    }

    /// Drop the connection (if any) and connect again now.
    pub fn reconnect(&self, id: &str) {
        if let Some(s) = self.slot(id) {
            {
                let mut v = s.view.lock().unwrap();
                if !matches!(v.status, Status::Disabled | Status::NeedsApproval | Status::Denied) {
                    v.status = Status::Connecting;
                }
            }
            s.kick();
            (self.wake)();
        }
    }

    /// The user allowed an MCP plugin to run (this session); connect now.
    pub fn approve_run(&self, id: &str) {
        if let Some(s) = self.slot(id) {
            s.flags.lock().unwrap().2 = true;
            s.kick();
        }
    }

    /// The tool's descriptor (with `inputSchema`), if connected.
    pub fn tool_json(&self, id: &str, tool: &str) -> Option<Value> {
        let s = self.slot(id)?;
        let mut c = s.client.lock().unwrap();
        c.as_mut()?.tools_json().ok()?.into_iter().find(|t| t["name"] == tool)
    }

    /// Call a tool (blocking). The caller must have the broker's permission.
    pub fn call(&self, id: &str, tool: &str, args: Value) -> Result<String> {
        self.call_cancellable(id, tool, args, None)
    }

    /// [`call`](Self::call) that stops when `cancel` is set (the chat's cancel):
    /// the server is told, and a stdio server that keeps running is stopped and
    /// reconnected.
    pub fn call_cancellable(&self, id: &str, tool: &str, args: Value, cancel: Option<&std::sync::atomic::AtomicBool>) -> Result<String> {
        let s = self.slot(id).with_context(|| format!("no MCP server {id}"))?;
        let mut g = s.client.lock().unwrap();
        let c = g.as_mut().with_context(|| format!("{id} is not connected"))?;
        let r = c.call_cancellable(tool, args, cancel);
        if c.is_dead() {
            *g = None;
            drop(g);
            set(&s, &self.wake, |v| v.status = Status::Connecting);
            s.kick();
            return r;
        }
        if let Err(e) = &r {
            let m = e.to_string();
            if m.contains("exited") || m.contains("connecting") || m.contains("expired") || m.contains("no reply") {
                drop(g);
                s.kick();
            }
        }
        r
    }
}

fn set(s: &Slot, w: &Wake, f: impl FnOnce(&mut View)) {
    f(&mut s.view.lock().unwrap());
    w();
}

fn run(s: Arc<Slot>, w: Wake) {
    let mut backoff = Duration::from_secs(1);
    loop {
        if s.stopped() {
            return;
        }
        let (id, src) = {
            let v = s.view.lock().unwrap();
            (v.id.clone(), v.source.clone())
        };
        if matches!(src, Source::Plugin(..)) && !s.flags.lock().unwrap().2 {
            match Broker::check(&broker_name(&id), &["run".to_string()], &Scope::Installed) {
                Check::Allow => {}
                c => {
                    set(&s, &w, |v| v.status = if c == Check::Deny { Status::Denied } else { Status::NeedsApproval });
                    s.wait(Duration::from_secs(3600));
                    continue;
                }
            }
        }
        set(&s, &w, |v| v.status = Status::Connecting);
        let r = connect(&id, &src).and_then(|mut c| {
            let tools = c.tools()?;
            Ok((c, tools))
        });
        match r {
            Ok((c, tools)) => {
                let info = format!("{} {}", c.server.0, c.server.1).trim().to_string();
                *s.client.lock().unwrap() = Some(c);
                set(&s, &w, |v| {
                    v.status = Status::Connected;
                    v.tools = tools;
                    v.info = info;
                });
                backoff = Duration::from_secs(1);
                loop {
                    if s.wait(PING_EVERY) || s.stopped() {
                        break;
                    }
                    let ok = s.client.lock().unwrap().as_mut().is_some_and(|c| c.ping().is_ok());
                    if !ok {
                        break;
                    }
                }
                *s.client.lock().unwrap() = None;
            }
            Err(e) => {
                let secs = backoff.as_secs();
                set(&s, &w, |v| {
                    v.status = Status::Failed(format!("{e:#}"), secs);
                    v.tools.clear();
                });
                s.wait(backoff);
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(broker_name("files"), "mcp-server:files");
        assert_eq!(broker_name("plugin:hello-mcp"), "hello-mcp");
        assert_eq!(tool_cap("echo"), "tool.echo");
        assert!(resolve_token("plain").is_err());
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn manager_connects_calls_and_reconnects() {
        let t = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("MOUSER_HOME", t.path()) };
        let srv = crate::mcp::fake_server(t.path());
        crate::storage::update_preferences(|p| {
            p.mcp.get_or_insert_with(Default::default).servers = vec![
                McpServerEntry {
                    name: "fake".into(),
                    env: Default::default(),
                    command: vec![srv.display().to_string()],
                    url: None,
                    token: None,
                    ca: None,
                    enabled: true,
                },
                McpServerEntry {
                    name: "gone".into(),
                    env: Default::default(),
                    command: vec![],
                    url: Some("http://127.0.0.1:1/mcp".into()),
                    token: None,
                    ca: None,
                    enabled: true,
                },
                McpServerEntry {
                    name: "off".into(),
                    env: Default::default(),
                    command: vec!["x".into()],
                    url: None,
                    token: None,
                    ca: None,
                    enabled: false,
                },
            ]
        })
        .unwrap();
        let m = Manager::start(|| {});
        let wait = |id: &str, f: &dyn Fn(&Status) -> bool| {
            for _ in 0..100 {
                if m.snapshot().iter().any(|v| v.id == id && f(&v.status)) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            panic!("{id}: {:?}", m.snapshot());
        };
        wait("fake", &|s| *s == Status::Connected);
        wait("gone", &|s| matches!(s, Status::Failed(..)));
        wait("off", &|s| *s == Status::Disabled);
        assert_eq!(m.snapshot()[0].tools[0].name, "greet");
        assert_eq!(m.call("fake", "greet", serde_json::json!({})).unwrap(), "hi from fake");
        assert!(m.call("off", "greet", serde_json::json!({})).is_err());
        m.reconnect("fake");
        wait("fake", &|s| *s == Status::Connected);
        assert_eq!(m.call("fake", "greet", serde_json::json!({})).unwrap(), "hi from fake");
        // Removing it from the config drops it.
        crate::storage::update_preferences(|p| p.mcp.as_mut().unwrap().servers.truncate(1)).unwrap();
        m.reload();
        assert_eq!(m.snapshot().len(), 1);
        unsafe { std::env::remove_var("MOUSER_HOME") };
    }
}
