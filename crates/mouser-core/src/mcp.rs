//! MCP-process plugins, skeleton (P1): start the plugin's stdio server,
//! `initialize`, `notifications/initialized`, `tools/list`, `tools/call`, stop.
//! Newline-delimited JSON-RPC 2.0 over stdin/stdout (the MCP stdio transport).
//! P3 replaces this with the rmcp client; the surface here stays.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

/// The protocol version Mouser offers (servers may answer an older one).
pub const PROTOCOL: &str = "2025-06-18";
pub const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub name: String,
    pub description: String,
}

pub struct Client {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next: u64,
    /// `serverInfo.name` / `version` and the negotiated protocol.
    pub server: (String, String, String),
}

impl Client {
    /// Start `argv` in `dir` with a scrubbed environment and do the handshake.
    pub fn start(argv: &[String], dir: &std::path::Path, plugin: &str) -> Result<Self> {
        let (prog, args) = argv.split_first().context("empty run.command")?;
        let mut c = Command::new(prog);
        c.args(args).current_dir(dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        c.env_clear();
        for k in ["PATH", "HOME", "LANG", "SystemRoot", "TEMP", "TMP", "USERPROFILE"] {
            if let Some(v) = std::env::var_os(k) {
                c.env(k, v);
            }
        }
        c.env("MOUSER_PLUGIN_API", crate::plugins::API.to_string()).env("MOUSER_PLUGIN_NAME", plugin);
        let mut child = c.spawn().with_context(|| format!("starting {prog}"))?;
        let stdin = child.stdin.take().context("stdin")?;
        let out = child.stdout.take().context("stdout")?;
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for l in BufReader::new(out).lines() {
                let Ok(l) = l else { break };
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        let mut me = Client { child, stdin, lines, next: 1, server: Default::default() };
        let r = me.request(
            "initialize",
            json!({"protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": {"name": "mouser", "version": env!("CARGO_PKG_VERSION")}}),
        )?;
        let s = |v: &Value| v.as_str().unwrap_or("").to_string();
        me.server = (s(&r["serverInfo"]["name"]), s(&r["serverInfo"]["version"]), s(&r["protocolVersion"]));
        me.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(me)
    }

    fn send(&mut self, v: &Value) -> Result<()> {
        let mut b = serde_json::to_vec(v)?;
        b.push(b'\n');
        self.stdin.write_all(&b)?;
        self.stdin.flush()?;
        Ok(())
    }

    /// One request; skips notifications and server requests until our reply.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next;
        self.next += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let l = match self.lines.recv_timeout(left) {
                Ok(l) => l,
                Err(RecvTimeoutError::Timeout) => bail!("{method}: no reply in {}s", TIMEOUT.as_secs()),
                Err(RecvTimeoutError::Disconnected) => bail!("{method}: the plugin exited"),
            };
            let Ok(v) = serde_json::from_str::<Value>(&l) else { continue };
            if v.get("id").and_then(Value::as_u64) != Some(id) || v.get("method").is_some() {
                continue;
            }
            if let Some(e) = v.get("error") {
                bail!("{method}: {}", e["message"].as_str().unwrap_or("error"));
            }
            return Ok(v["result"].clone());
        }
    }

    pub fn tools(&mut self) -> Result<Vec<Tool>> {
        let r = self.request("tools/list", json!({}))?;
        Ok(r["tools"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|t| Tool { name: t["name"].as_str().unwrap_or("").into(), description: t["description"].as_str().unwrap_or("").into() })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Call a tool; returns the text parts of its content.
    pub fn call(&mut self, name: &str, args: Value) -> Result<String> {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}))?;
        let text: Vec<&str> = r["content"].as_array().map(|a| a.iter().filter_map(|c| c["text"].as_str()).collect()).unwrap_or_default();
        if r["isError"].as_bool() == Some(true) {
            bail!("{name}: {}", text.join("\n"));
        }
        Ok(text.join("\n"))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn handshake_list_and_call_with_a_shell_mcp_server() {
        let t = tempfile::tempdir().unwrap();
        // A tiny line-based MCP server: replies by id, sends a notification first.
        let s = r#"#!/bin/sh
while read -r l; do
  id=$(printf '%s' "$l" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$l" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","method":"notifications/message","params":{}}\n{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"9"}}}\n' "$id";;
    *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"greet","description":"say hi","inputSchema":{"type":"object"}}]}}\n' "$id";;
    *'"tools/call"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"hi from fake"}]}}\n' "$id";;
    *'"id"'*) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"nope"}}\n' "$id";;
  esac
done
"#;
        let p = t.path().join("srv");
        std::fs::write(&p, s).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut c = Client::start(&[p.display().to_string()], t.path(), "fake").unwrap();
        assert_eq!(c.server, ("fake".into(), "9".into(), "2025-06-18".into()));
        assert_eq!(c.tools().unwrap(), vec![Tool { name: "greet".into(), description: "say hi".into() }]);
        assert_eq!(c.call("greet", json!({})).unwrap(), "hi from fake");
        assert!(c.request("resources/list", json!({})).unwrap_err().to_string().contains("nope"));
    }
}
