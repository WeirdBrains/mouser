//! Engine actions behind `mouser engine …` = `mouser plugin … --kind engine`
//! (docs/engines.md). Engines are plugins; the list comes from the registry.

use anyhow::{Result, bail};
use mouser_core::engines;
use std::io::Write;
use std::path::Path;

/// One `mouser engine` action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    List,
    Install(String),
    Uninstall(String),
    Use(String),
    /// Clear the crash skip / failure record (`None`: all engines).
    Unskip(Option<String>),
    /// Set the whole priority order.
    Priority(Vec<String>),
    /// Move one engine: `first`, `last`, `up`, `down` or a 1-based position.
    Move(String, String),
}

pub fn run(action: &Action, out: &mut dyn Write) -> Result<()> {
    match action {
        Action::List => list(out),
        Action::Install(p) => {
            if p.starts_with("http://") || p.starts_with("https://") {
                bail!("installing from a URL is not supported; download the engine and run `mouser engine install <dir>`");
            }
            let (m, lib) = engines::source(Path::new(p))?;
            let reported = mouser_render::engine::probe(&lib)?;
            if reported != m.name {
                bail!("library reports engine name {reported:?} but engine.toml says {:?}", m.name);
            }
            let inst = engines::install(&m, &lib)?;
            writeln!(out, "installed {} {} in {}", m.name, m.version, inst.dir.display())?;
            writeln!(out, "use it with: mouser engine use {}", m.name)?;
            Ok(())
        }
        Action::Uninstall(n) => {
            let was = engines::uninstall(n)?;
            writeln!(out, "uninstalled {n}")?;
            if was {
                writeln!(out, "new windows will use the mouser engine")?;
            }
            Ok(())
        }
        Action::Unskip(Some(n)) => {
            if engines::unskip(n)? {
                writeln!(out, "{n} is no longer skipped; Mouser will try it again on the next start")?;
                if engines::chosen() != *n {
                    writeln!(out, "(not selected; `mouser engine use {n}` to use it)")?;
                }
            } else {
                writeln!(out, "{n} was not skipped and has no recorded failure; nothing to do")?;
            }
            Ok(())
        }
        Action::Unskip(None) => {
            let done = engines::unskip_all();
            if done.is_empty() {
                writeln!(out, "no engine was skipped or failed; nothing to do")?;
            } else {
                writeln!(out, "restored: {}", done.join(", "))?;
            }
            Ok(())
        }
        Action::Priority(names) => {
            let l = engines::set_priority(names)?;
            writeln!(out, "priority: {}", l.join(" > "))?;
            Ok(())
        }
        Action::Move(n, to) => {
            let l = match to.as_str() {
                "first" | "top" => engines::move_to(n, 0)?,
                "last" | "bottom" => engines::move_to(n, usize::MAX)?,
                "up" => engines::move_by(n, -1)?,
                "down" => engines::move_by(n, 1)?,
                p => match p.parse::<usize>() {
                    Ok(i) if i >= 1 => engines::move_to(n, i - 1)?,
                    _ => bail!("position must be first, last, up, down or a number from 1"),
                },
            };
            writeln!(out, "priority: {}", l.join(" > "))?;
            Ok(())
        }
        Action::Use(n) => {
            if n == "skia" && !mouser_render::engine::available_builtins().iter().any(|(b, _)| *b == "skia") {
                bail!("this build has no Skia engine");
            }
            engines::select(n)?;
            writeln!(out, "{n} is now first: {}", engines::saved_priority().join(" > "))?;
            Ok(())
        }
    }
}

fn state_text(name: &str) -> String {
    match engines::state(name) {
        engines::State::Ok => "ok".into(),
        engines::State::Skipped { since, reason } => format!(
            "SKIPPED: {}{}; restore: mouser engine unskip {name}",
            reason.unwrap_or_else(|| "crashed Mouser while starting".into()),
            since.map(|t| format!(" {}", engines::ago(t))).unwrap_or_default()
        ),
        engines::State::Failed { at, reason } => {
            format!("FAILED {}: {reason}; tried again each start; clear: mouser engine unskip {name}", engines::ago(at))
        }
    }
}

fn list(out: &mut dyn Write) -> Result<()> {
    let order = engines::priority();
    let pos = |n: &str| order.iter().position(|o| o == n).map(|i| (i + 1).to_string()).unwrap_or_else(|| "-".into());
    let first = mouser_render::engine::first_usable(&order).unwrap_or_default();
    let mark = |n: &str| if n == first { "*" } else { " " };
    writeln!(out, "  {:<4} {:<12} {:<10} {:<8} details", "prio", "engine", "version", "state")?;
    let mut rows: Vec<(String, String, String, String)> = Vec::new();
    let built = mouser_render::engine::available_builtins();
    let reg = mouser_core::plugins::Registry::load();
    for e in reg.entries.iter().filter(|e| e.kinds.contains(&mouser_core::plugins::Kind::Engine)) {
        let n = e.name.as_str();
        let (v, d) = if e.dir.is_none() {
            // Built in: listed only when this build has it.
            let Some((_, d)) = built.iter().find(|(b, _)| *b == n) else { continue };
            (
                e.version.clone(),
                if n == engines::DEFAULT { format!("{d}; always the last fallback, cannot be removed or disabled") } else { d.to_string() },
            )
        } else {
            (e.version.clone(), e.description.clone())
        };
        let st = if e.enabled { state_text(n) } else { format!("DISABLED: enable: mouser engine enable {n}") };
        rows.push((n.to_string(), v, d, st));
    }
    for e in engines::installed() {
        if let Err((dir, why)) = e {
            writeln!(out, "  {:<4} {dir:<12} {:<10} {:<8} {why}", "-", "?", "broken")?;
        }
    }
    // Listed in priority order, then the rest.
    rows.sort_by_key(|r| order.iter().position(|o| *o == r.0).unwrap_or(usize::MAX));
    for (n, v, d, st) in rows {
        let (short, rest) = split_state(&st);
        writeln!(out, "{} {:<4} {n:<12} {v:<10} {short:<8} {d}{rest}", mark(&n), pos(&n))?;
    }
    for n in &order {
        if !engines::is_builtin(n) && engines::find(n).is_none() {
            writeln!(out, "  ({n} is in the priority list but not installed; skipped)")?;
        }
    }
    let src = if std::env::var("MOUSER_ENGINE").is_ok_and(|v| !v.trim().is_empty()) { "MOUSER_ENGINE" } else { "preferences" };
    writeln!(out, "order ({src}): {}; * = what new windows use (skipped ones are passed over)", order.join(" > "))?;
    Ok(())
}

/// ("skipped", " — reason...") for the list columns.
fn split_state(st: &str) -> (&'static str, String) {
    if st == "ok" {
        ("ok", String::new())
    } else if let Some(r) = st.strip_prefix("DISABLED: ") {
        ("disabled", format!("\n      -> {r}"))
    } else if let Some(r) = st.strip_prefix("SKIPPED: ") {
        ("skipped", format!("\n      -> {r}"))
    } else {
        ("failed", format!("\n      -> {}", st.strip_prefix("FAILED ").unwrap_or(st)))
    }
}
