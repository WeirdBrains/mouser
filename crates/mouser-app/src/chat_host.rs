//! What the chat mode's agent can see and do (docs/chat.md): the active tab's
//! screen and scrollback, Mouser's own tools (in-process, the same as the MCP
//! server's), and the tools of connected MCP servers and MCP plugins.
//!
//! Permissions: Mouser's typing/running/file-writing tools ask on every call
//! (the card shows the real command, path and tab; a broker "never" for the
//! `chat` plugin blocks them). Read tools only obey a "never". MCP tools follow
//! the broker entry of their server (`mcp-server:<name>` / plugin, `tool.<name>`),
//! asking on the card when it says ask.

use crate::mcp_tools::{AppTools, descriptors};
use crate::{AppState, mcp_clients, tabs};
use mouser_core::agent::{AgentHost, AgentTool, Check, ScreenContext};
use mouser_core::mcp_server::Tools;
use mouser_core::permissions::{Broker, Scope};
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// The broker's name for the chat's use of Mouser's own tools.
pub const WHO: &str = "chat";
const CLIENT: &str = "mouser-chat";

pub struct ChatHost {
    tools: AppTools,
}

impl ChatHost {
    pub fn new(tabs: Arc<Mutex<tabs::Tabs>>, app_state: Arc<Mutex<AppState>>) -> Self {
        ChatHost { tools: AppTools { tabs, app_state, preapproved: true } }
    }
}

/// A tool name the model APIs accept (`^[a-zA-Z0-9_-]{1,64}$`).
fn model_name(source: &str, tool: &str) -> String {
    let clean = |s: &str| s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect::<String>();
    let n = format!("{}__{}", clean(source), clean(tool));
    n.chars().take(64).collect()
}

fn cap_of(tool: &str) -> &'static str {
    match tool {
        "screen_read" => "screen.read",
        "scrollback_read" => "scrollback.read",
        "files_list" | "files_read" | "tabs_list" => "files.read",
        "shell_type" | "shell_run" => "shell.type",
        _ => "files.write",
    }
}

fn text(v: Value) -> String {
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

impl ChatHost {
    fn tab_title(&self, args: &Value) -> String {
        let t = self.tools.tabs.lock().unwrap();
        let i = match args.get("tab").and_then(Value::as_u64) {
            Some(i) => i as usize,
            None => t.list.iter().position(|s| Some(s.id) == t.active().map(|a| a.id)).unwrap_or(0),
        };
        t.list.get(i).map(|s| format!("tab {i} ({})", s.title())).unwrap_or_else(|| format!("tab {i} (missing)"))
    }
}

impl AgentHost for ChatHost {
    fn context(&self) -> ScreenContext {
        let get = |n: &str, a: Value| self.tools.call(CLIENT, n, &a).ok();
        let place = get("tabs_list", Value::Null)
            .and_then(|v| {
                v["tabs"].as_array()?.iter().find(|t| t["active"] == true).map(|t| {
                    let at =
                        t["folder"].as_str().map(String::from).or_else(|| t["remote"].as_str().map(|r| format!("remote {r}"))).unwrap_or_default();
                    format!("{} — {at}", t["title"].as_str().unwrap_or(""))
                })
            })
            .unwrap_or_default();
        let screen = get("screen_read", Value::Null).and_then(|v| v["text"].as_str().map(String::from)).unwrap_or_default();
        let scrollback = get("scrollback_read", serde_json::json!({"lines": mouser_core::agent::CONTEXT_LINES}))
            .and_then(|v| v["text"].as_str().map(String::from))
            .unwrap_or_default();
        ScreenContext { place, screen, scrollback }
    }

    fn tools(&self) -> Vec<AgentTool> {
        let mut v: Vec<AgentTool> = descriptors()
            .into_iter()
            .map(|d| {
                let n = d["name"].as_str().unwrap_or_default().to_string();
                AgentTool {
                    name: model_name("mouser", &n),
                    display: format!("mouser.{n}"),
                    description: d["description"].as_str().unwrap_or_default().to_string(),
                    schema: d["inputSchema"].clone(),
                    always_ask: d["annotations"]["readOnlyHint"] != true,
                }
            })
            .collect();
        for s in mcp_clients().snapshot() {
            if s.status != mouser_core::mcp_client::Status::Connected {
                continue;
            }
            for t in &s.tools {
                let schema =
                    mcp_clients().tool_json(&s.id, &t.name).map(|j| j["inputSchema"].clone()).unwrap_or(serde_json::json!({"type": "object"}));
                v.push(AgentTool {
                    name: model_name(&s.id.replace("plugin:", ""), &t.name),
                    display: format!("{}.{}", s.id, t.name),
                    description: t.description.clone(),
                    schema,
                    always_ask: false,
                });
            }
        }
        v
    }

    fn check(&self, tool: &AgentTool, args: &Value) -> Check {
        if let Some(n) = tool.display.strip_prefix("mouser.") {
            let cap = cap_of(n).to_string();
            if let mouser_core::permissions::Check::Deny = Broker::check(WHO, std::slice::from_ref(&cap), &Scope::Installed) {
                return Check::Deny(format!("{cap} is set to never for {WHO} (mouser plugin permissions {WHO})"));
            }
            if !tool.always_ask {
                return Check::Allow;
            }
            let mut lines = vec![format!("Target: {}", self.tab_title(args))];
            if let Some(o) = args.as_object() {
                for (k, v) in o {
                    if k == "tab" {
                        continue;
                    }
                    let s = v.as_str().map(String::from).unwrap_or_else(|| v.to_string());
                    lines.push(format!("{k}: {}", s.replace(['\r', '\n'], "⏎")));
                }
            }
            return Check::Ask(lines);
        }
        let (id, name) = tool.display.rsplit_once('.').unwrap_or((&tool.display, ""));
        let who = mouser_core::mcp_client::broker_name(id);
        match Broker::check(&who, &[mouser_core::mcp_client::tool_cap(name)], &Scope::Installed) {
            mouser_core::permissions::Check::Allow => Check::Allow,
            mouser_core::permissions::Check::Deny => Check::Deny(format!("{name} is set to never for {who}")),
            mouser_core::permissions::Check::Ask(_) => Check::Ask(vec![
                format!("Server: {id}"),
                format!("Tool: {name}"),
                format!("Arguments: {}", args.to_string().chars().take(300).collect::<String>()),
            ]),
        }
    }

    fn call(&self, tool: &AgentTool, args: &Value) -> Result<String, String> {
        if let Some(n) = tool.display.strip_prefix("mouser.") {
            return self.tools.call(CLIENT, n, args).map(text);
        }
        let (id, name) = tool.display.rsplit_once('.').ok_or("bad tool")?;
        mcp_clients().call(id, name, args.clone()).map_err(|e| format!("{e:#}"))
    }

    fn call_cancellable(&self, tool: &AgentTool, args: &Value, cancel: &std::sync::atomic::AtomicBool) -> Result<String, String> {
        match tool.display.strip_prefix("mouser.") {
            // Mouser's own tools are short and in-process.
            Some(_) => self.call(tool, args),
            None => {
                let (id, name) = tool.display.rsplit_once('.').ok_or("bad tool")?;
                mcp_clients().call_cancellable(id, name, args.clone(), Some(cancel)).map_err(|e| format!("{e:#}"))
            }
        }
    }
}
