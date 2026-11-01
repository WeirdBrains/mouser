//! `mouser plugin …` (docs/plugins.md). Mirrors `mouser engine …`; engines are
//! plugins of kind `engine`, so `mouser plugin install <engine dir>` hands off to
//! the engine installer and `mouser engine …` stays as their alias.

use anyhow::{Result, bail};
use mouser_core::permissions::{Decision, Store};
use mouser_core::plugins::{self, Kind, Transport};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    List,
    Install(String),
    Uninstall(String),
    Enable(String),
    Disable(String),
    Priority(Vec<String>),
    Move(String, String),
    /// Show (no setting) or set `cap=allow|ask|never` (`*` for every requested one).
    Permissions(String, Vec<String>),
    /// Every saved permission, by key (plugin, `fuzzball:<name>`, `ssh:<host>/<name>`).
    PermissionsList,
    /// Forget everything saved under a key.
    Revoke(String),
    /// Start an MCP plugin, handshake and list its tools.
    Probe(String),
}

pub fn run(action: &Action, out: &mut dyn Write) -> Result<()> {
    match action {
        Action::List => list(out),
        Action::Install(p) => {
            if p.starts_with("http://") || p.starts_with("https://") {
                bail!("installing from a URL is not supported; download the plugin and run `mouser plugin install <dir>`");
            }
            let path = Path::new(p);
            let d = if path.is_dir() { path.to_path_buf() } else { path.parent().unwrap_or(Path::new(".")).to_path_buf() };
            if d.join("engine.toml").is_file() && !d.join("plugin.toml").is_file() {
                return crate::engine_cli::run(&crate::engine_cli::Action::Install(p.clone()), out);
            }
            let i = plugins::install(path)?;
            let m = &i.manifest;
            writeln!(out, "installed {} {} ({}) in {}", m.name, m.version, kinds(&m.kinds()), i.dir.display())?;
            if !m.permissions.is_empty() {
                writeln!(out, "it asks for:")?;
                for c in &m.permissions {
                    writeln!(out, "  {c:<16} {}", plugins::capability_text(c))?;
                }
                writeln!(out, "Mouser asks you before each is used; `mouser plugin permissions {}` to see or change", m.name)?;
            }
            if !m.run.commands.is_empty() {
                writeln!(out, "[mouser] line commands: {}", m.run.commands.join(", "))?;
            }
            Ok(())
        }
        Action::Uninstall(n) => {
            if plugins::find(n).is_none() && mouser_core::engines::find(n).is_some() {
                return crate::engine_cli::run(&crate::engine_cli::Action::Uninstall(n.clone()), out);
            }
            plugins::uninstall(n)?;
            writeln!(out, "uninstalled {n} (and forgot its permissions)")?;
            Ok(())
        }
        Action::Enable(n) | Action::Disable(n) => {
            let on = matches!(action, Action::Enable(_));
            plugins::set_enabled(n, on)?;
            writeln!(out, "{n} {}", if on { "enabled" } else { "disabled" })?;
            Ok(())
        }
        Action::Priority(names) => {
            let l = plugins::set_priority(names)?;
            writeln!(out, "priority: {}", l.join(" > "))?;
            Ok(())
        }
        Action::Move(n, to) => {
            let l = match to.as_str() {
                "first" | "top" => plugins::move_to(n, 0)?,
                "last" | "bottom" => plugins::move_to(n, usize::MAX)?,
                "up" => plugins::move_by(n, -1)?,
                "down" => plugins::move_by(n, 1)?,
                p => match p.parse::<usize>() {
                    Ok(i) if i >= 1 => plugins::move_to(n, i - 1)?,
                    _ => bail!("position must be first, last, up, down or a number from 1"),
                },
            };
            writeln!(out, "priority: {}", l.join(" > "))?;
            Ok(())
        }
        Action::Permissions(n, sets) => permissions(n, sets, out),
        Action::PermissionsList => {
            let st = Store::load();
            if st.plugins.is_empty() {
                writeln!(out, "no saved permissions (everything asks)")?;
            }
            for (k, caps) in &st.plugins {
                let c: Vec<String> = caps.iter().map(|(c, d)| format!("{c}={}", d.name())).collect();
                writeln!(out, "{k:?}: {}", c.join(", "))?;
            }
            writeln!(out, "revoke with: mouser plugin permissions '<key>' revoke")?;
            Ok(())
        }
        Action::Revoke(k) => {
            if mouser_core::permissions::forget(k)? {
                writeln!(out, "revoked {k}; it asks again")?;
                Ok(())
            } else {
                bail!("nothing saved under {k:?} (see `mouser plugin permissions`)")
            }
        }
        Action::Probe(n) => {
            let Some(i) = plugins::find(n) else { bail!("no plugin named {n}") };
            if i.manifest.run.transport != Transport::Mcp {
                bail!("{n} is a {} plugin; probe starts MCP plugins", i.manifest.run.transport.name());
            }
            let mut c = mouser_core::mcp::Client::start(&i.argv(), &i.dir, n)?;
            let (sn, sv, pv) = c.server.clone();
            writeln!(out, "{n}: MCP server {sn} {sv}, protocol {pv}")?;
            for t in c.tools()? {
                writeln!(out, "  tool {:<16} {}", t.name, t.description)?;
            }
            Ok(())
        }
    }
}

fn kinds(k: &[Kind]) -> String {
    k.iter().map(|k| k.name()).collect::<Vec<_>>().join(",")
}

fn permissions(n: &str, sets: &[String], out: &mut dyn Write) -> Result<()> {
    let requested: Vec<String> = match plugins::find(n) {
        Some(i) => i.manifest.permissions,
        None if n.starts_with("fuzzball:") || n.starts_with("ssh:") => plugins::CAPABILITIES.iter().map(|c| c.0.to_string()).collect(),
        None => bail!("no plugin named {n} (Fuzzball plugins are named fuzzball:<name>)"),
    };
    let mut st = Store::load();
    if !sets.is_empty() {
        for s in sets {
            let Some((cap, d)) = s.split_once('=') else { bail!("use cap=allow|ask|never") };
            let Some(d) = Decision::parse(d) else { bail!("{d:?}: use allow, ask or never") };
            let caps: Vec<String> = if cap == "*" { requested.clone() } else { vec![cap.to_string()] };
            for c in caps {
                if !plugins::known_capability(&c) {
                    bail!("unknown capability {c:?}");
                }
                st.set(n, &c, d);
            }
        }
        st.save()?;
    }
    writeln!(out, "{n}:")?;
    if requested.is_empty() {
        writeln!(out, "  (asks for nothing)")?;
    }
    for c in &requested {
        writeln!(out, "  {c:<16} {:<6} {}", st.get(n, c).name(), plugins::capability_text(c))?;
    }
    Ok(())
}

fn list(out: &mut dyn Write) -> Result<()> {
    let r = plugins::Registry::load();
    writeln!(out, "  {:<4} {:<14} {:<9} {:<9} {:<16} details", "prio", "plugin", "version", "via", "provides")?;
    let mut i = 0;
    for e in r.entries.iter().filter(|e| !e.kinds.contains(&Kind::Engine)) {
        i += 1;
        let state = if e.enabled { "" } else { " [disabled]" };
        let cmds = if e.commands.is_empty() { String::new() } else { format!("; commands: {}", e.commands.join(", ")) };
        writeln!(out, "  {i:<4} {:<14} {:<9} {:<9} {:<16} {}{cmds}{state}", e.name, e.version, e.transport.name(), kinds(&e.kinds), e.description)?;
    }
    for (d, why) in &r.broken {
        writeln!(out, "  -    {d:<14} {:<9} {:<9} {:<16} broken: {why}", "?", "-", "-")?;
    }
    let engines: Vec<&str> = r.providers(Kind::Engine).iter().map(|e| e.name.as_str()).collect();
    writeln!(out, "engines (kind engine, ordered by `mouser engine priority`): {}", engines.join(", "))?;
    let kinds_with: Vec<String> = Kind::ALL
        .iter()
        .map(|k| format!("{}: {}", k.name(), r.providers(*k).iter().map(|e| e.name.as_str()).collect::<Vec<_>>().join(" > ")))
        .collect();
    writeln!(out, "by kind: {}", kinds_with.join("; "))?;
    Ok(())
}
