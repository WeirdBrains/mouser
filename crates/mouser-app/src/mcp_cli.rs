//! `mouser mcp …`: Mouser's MCP server (docs/mcp.md).

use anyhow::{Result, bail};
use mouser_core::storage::{self, McpPrefs, McpServerEntry, McpServerPrefs};
use std::io::Write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// stdio bridge to the running Mouser.
    ServeStdio,
    Status,
    Enable {
        https: bool,
        port: Option<u16>,
        https_port: Option<u16>,
    },
    Disable,
    /// Print the token (for HTTP clients' config).
    TokenShow,
    TokenRotate,
    /// `mouser mcp servers add`: a stdio `command` or an HTTP(S) `url`.
    ServersAdd {
        name: String,
        command: Vec<String>,
        /// `K=V` for a stdio server (V may be `keychain:NAME`).
        env: Vec<String>,
        url: Option<String>,
        /// The bearer token (read from stdin by main; stored in the keychain).
        token: Option<String>,
        ca: Option<String>,
    },
    ServersList,
    ServersRemove(String),
    /// Connect, list tools, optionally call one (through the broker).
    ServersTest {
        name: String,
        call: Option<String>,
        args: Option<String>,
    },
}

/// The keychain secret name for a server's token.
pub fn server_secret(name: &str) -> String {
    format!("mcp-server-{name}")
}

fn servers() -> Vec<McpServerEntry> {
    storage::load_preferences().mcp.map(|m| m.servers).unwrap_or_default()
}

fn save_servers(v: Vec<McpServerEntry>) -> Result<()> {
    storage::update_preferences(|p| p.mcp.get_or_insert_with(McpPrefs::default).servers = v.clone())?;
    Ok(())
}

fn servers_run(a: &Action, out: &mut dyn Write) -> Result<()> {
    use mouser_core::mcp_client;
    match a {
        Action::ServersAdd { name, command, env, url, token, ca } => {
            let mut envs = std::collections::BTreeMap::new();
            for kv in env {
                let Some((k, v)) = kv.split_once('=') else { bail!("--env takes K=V") };
                if !v.starts_with("keychain:") {
                    writeln!(out, "warning: {k} is stored in plain text in preferences.toml; use keychain:NAME for secrets")?;
                }
                envs.insert(k.to_string(), v.to_string());
            }
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
                bail!("name must be letters, digits, - _ or .");
            }
            match (command.is_empty(), url) {
                (true, None) => bail!("give --url URL or a command after --"),
                (false, Some(_)) => bail!("give either --url or a command, not both"),
                _ => {}
            }
            if let Some(u) = url {
                mouser_core::mcp::Target::parse(u)?;
            }
            let ca = match ca {
                Some(p) => Some(std::fs::canonicalize(p).map_err(|e| anyhow::anyhow!("{p}: {e}"))?.display().to_string()),
                None => None,
            };
            let mut v = servers();
            let old = v.iter().position(|e| e.name == *name);
            let token_ref = match token {
                Some(t) => {
                    let st = mouser_core::secrets::set(&server_secret(name), t.trim())?;
                    if let mouser_core::secrets::Store::File(p) = &st {
                        writeln!(out, "warning: no OS keychain available; the token is stored in {} (0600)", p.display())?;
                    }
                    Some(format!("keychain:{}", server_secret(name)))
                }
                None => old.and_then(|i| v[i].token.clone()),
            };
            let e = McpServerEntry { name: name.clone(), command: command.clone(), env: envs, url: url.clone(), token: token_ref, ca, enabled: true };
            match old {
                Some(i) => v[i] = e,
                None => v.push(e),
            }
            save_servers(v)?;
            writeln!(
                out,
                "{} MCP server {name}; Mouser connects to it on its next start (test: mouser mcp servers test {name})",
                if old.is_some() { "updated" } else { "added" }
            )?;
            Ok(())
        }
        Action::ServersList => {
            let all = mcp_client::sources();
            if all.is_empty() {
                writeln!(out, "no MCP servers (add one: mouser mcp servers add NAME --url URL | -- COMMAND…)")?;
            }
            for (id, src) in all {
                match src {
                    mcp_client::Source::Config(e) => {
                        let what = e.url.clone().unwrap_or_else(|| e.command.join(" "));
                        let tok = if e.token.is_some() { " (token in keychain)" } else { "" };
                        writeln!(out, "{id:<16} {what}{tok}{}", if e.enabled { "" } else { " [off]" })?;
                    }
                    mcp_client::Source::Plugin(_, argv) => writeln!(out, "{id:<16} {} (MCP plugin)", argv.join(" "))?,
                }
            }
            Ok(())
        }
        Action::ServersRemove(name) => {
            let mut v = servers();
            let Some(i) = v.iter().position(|e| e.name == *name) else { bail!("no MCP server named {name} (see `mouser mcp servers list`)") };
            v.remove(i);
            save_servers(v)?;
            mouser_core::secrets::delete(&server_secret(name));
            let _ = mouser_core::permissions::forget(&mouser_core::permissions::key(
                &mcp_client::broker_name(name),
                &mouser_core::permissions::Scope::Installed,
            ));
            writeln!(out, "removed MCP server {name} (and its token and permissions)")?;
            Ok(())
        }
        Action::ServersTest { name, call, args } => {
            let Some((id, src)) = mcp_client::sources().into_iter().find(|(id, _)| id == name || *id == format!("plugin:{name}")) else {
                bail!("no MCP server named {name} (see `mouser mcp servers list`)")
            };
            let mut c = mcp_client::connect(&id, &src)?;
            let (sn, sv, pv) = c.server.clone();
            writeln!(out, "{id}: connected to {sn} {sv}, protocol {pv}")?;
            let tools = c.tools()?;
            for t in &tools {
                writeln!(out, "  tool {:<20} {}", t.name, t.description)?;
            }
            if let Some(tool) = call {
                if !tools.iter().any(|t| t.name == *tool) {
                    bail!("{id} has no tool {tool}");
                }
                let args: serde_json::Value = serde_json::from_str(args.as_deref().unwrap_or("{}"))?;
                let caps = vec![mcp_client::tool_cap(tool)];
                let who = mcp_client::broker_name(&id);
                let scope = mouser_core::permissions::Scope::Installed;
                use mouser_core::permissions::{Answer, Broker, Check};
                match Broker::check(&who, &caps, &scope) {
                    Check::Allow => {}
                    Check::Deny => bail!("{tool} is set to never (mouser plugin permissions {who})"),
                    Check::Ask(_) => {
                        use std::io::IsTerminal;
                        if !std::io::stdin().is_terminal() {
                            bail!(
                                "{tool} needs permission; allow it with `mouser plugin permissions {who} {}=allow` or run this in a terminal",
                                caps[0]
                            );
                        }
                        write!(out, "Allow {who} to call {tool}? [y]es once / [a]lways / [N]o: ")?;
                        out.flush()?;
                        let mut l = String::new();
                        std::io::stdin().read_line(&mut l)?;
                        match l.trim() {
                            "y" | "Y" => {}
                            "a" | "A" => Broker::answer(&who, &caps, &scope, Answer::Always)?,
                            _ => bail!("not allowed"),
                        }
                    }
                }
                writeln!(out, "{}", c.call(tool, args)?)?;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

fn prefs() -> McpServerPrefs {
    storage::load_preferences().mcp.and_then(|m| m.server).unwrap_or_default()
}

fn save(s: McpServerPrefs) -> Result<()> {
    storage::update_preferences(|p| p.mcp.get_or_insert_with(McpPrefs::default).server = Some(s.clone()))?;
    Ok(())
}

pub fn run(a: &Action, out: &mut dyn Write) -> Result<()> {
    match a {
        Action::ServersAdd { .. } | Action::ServersList | Action::ServersRemove(_) | Action::ServersTest { .. } => servers_run(a, out),
        Action::ServeStdio => {
            let s = prefs();
            if !s.enabled {
                bail!("the MCP server is off; run `mouser mcp enable` and restart Mouser");
            }
            let (token, _) = mouser_core::secrets::mcp_token()?;
            mouser_core::mcp_server::stdio_bridge(s.port, &token, std::io::stdin().lock(), std::io::stdout().lock())
        }
        Action::Status => {
            let s = prefs();
            writeln!(out, "MCP server: {}", if s.enabled { "enabled" } else { "off (mouser mcp enable)" })?;
            writeln!(out, "  http  : http://127.0.0.1:{}/mcp (loopback only)", s.port)?;
            if s.https {
                writeln!(out, "  https : https://127.0.0.1:{}/mcp", s.https_port)?;
                match mouser_core::mcp_server::tls_config(s.cert.as_deref(), s.key.as_deref()) {
                    Ok((_, cert, fp)) => writeln!(out, "  cert  : {} (SHA256 {fp})", cert.display())?,
                    Err(e) => writeln!(out, "  cert  : error: {e:#}")?,
                }
            } else {
                writeln!(out, "  https : off (mouser mcp enable --https)")?;
            }
            writeln!(out, "  stdio : mouser mcp serve --stdio (bridges to the running Mouser)")?;
            match mouser_core::secrets::get("mcp-token") {
                Some((_, st)) => writeln!(out, "  token : in {} (show: mouser mcp token --show)", st.describe())?,
                None => writeln!(out, "  token : made on first start")?,
            }
            Ok(())
        }
        Action::Enable { https, port, https_port } => {
            let mut s = prefs();
            s.enabled = true;
            s.https |= *https;
            if let Some(p) = port {
                s.port = *p;
            }
            if let Some(p) = https_port {
                s.https_port = *p;
            }
            if s.port == s.https_port {
                bail!("http and https need different ports");
            }
            let (_, st) = mouser_core::secrets::mcp_token()?;
            save(s.clone())?;
            writeln!(
                out,
                "MCP server enabled on 127.0.0.1:{}{}; restart Mouser to start it",
                s.port,
                if s.https { format!(" and https :{}", s.https_port) } else { String::new() }
            )?;
            if let mouser_core::secrets::Store::File(p) = st {
                writeln!(out, "warning: no OS keychain available; the token is stored in {} (0600)", p.display())?;
            }
            Ok(())
        }
        Action::Disable => {
            let mut s = prefs();
            s.enabled = false;
            save(s)?;
            writeln!(out, "MCP server disabled (from the next start)")?;
            Ok(())
        }
        Action::TokenShow => {
            let (t, _) = mouser_core::secrets::mcp_token()?;
            writeln!(out, "{t}")?;
            Ok(())
        }
        Action::TokenRotate => {
            let st = mouser_core::secrets::rotate_mcp_token()?;
            writeln!(out, "new token stored in {}; restart Mouser, then update your clients", st.describe())?;
            Ok(())
        }
    }
}
