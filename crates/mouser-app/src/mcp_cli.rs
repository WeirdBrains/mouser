//! `mouser mcp …`: Mouser's MCP server (docs/mcp.md).

use anyhow::{Result, bail};
use mouser_core::storage::{self, McpPrefs, McpServerPrefs};
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
}

fn prefs() -> McpServerPrefs {
    storage::load_preferences().mcp.and_then(|m| m.server).unwrap_or_default()
}

fn save(s: McpServerPrefs) -> Result<()> {
    storage::update_preferences(|p| p.mcp = Some(McpPrefs { server: Some(s.clone()) }))?;
    Ok(())
}

pub fn run(a: &Action, out: &mut dyn Write) -> Result<()> {
    match a {
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
