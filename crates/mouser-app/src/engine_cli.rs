//! `mouser engine list | install | uninstall | use` (docs/engines.md).

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
        Action::Use(n) => {
            if n == "skia" && !mouser_render::engine::available_builtins().iter().any(|(b, _)| *b == "skia") {
                bail!("this build has no Skia engine");
            }
            engines::select(n)?;
            writeln!(out, "new windows will use the {n} engine (open windows keep theirs until restarted)")?;
            Ok(())
        }
    }
}

fn state_text(name: &str) -> String {
    match engines::state(name) {
        engines::State::Ok => "ok".into(),
        engines::State::Skipped { since } => format!(
            "SKIPPED: crashed Mouser while starting{}; restore: mouser engine unskip {name}",
            since.map(|t| format!(" {}", engines::ago(t))).unwrap_or_default()
        ),
        engines::State::Failed { at, reason } => {
            format!("FAILED {}: {reason}; tried again each start; clear: mouser engine unskip {name}", engines::ago(at))
        }
    }
}

fn list(out: &mut dyn Write) -> Result<()> {
    let chosen = engines::chosen();
    let selected = |n: &str| if n == chosen { "*" } else { " " };
    writeln!(out, "  {:<12} {:<10} {:<8} details", "engine", "version", "state")?;
    for (n, d) in mouser_render::engine::available_builtins() {
        let st = state_text(n);
        let (short, rest) = split_state(&st);
        writeln!(out, "{} {n:<12} {:<10} {short:<8} {d}{rest}", selected(n), "built-in")?;
    }
    for e in engines::installed() {
        match e {
            Ok(i) => {
                let m = &i.manifest;
                let st = state_text(&m.name);
                let (short, rest) = split_state(&st);
                writeln!(out, "{} {:<12} {:<10} {short:<8} {}{rest}", selected(&m.name), m.name, m.version, m.description)?;
            }
            Err((dir, why)) => writeln!(out, "  {dir:<12} {:<10} {:<8} {why}", "?", "broken")?,
        }
    }
    if chosen != "mouser" && !engines::is_builtin(&chosen) && engines::find(&chosen).is_none() {
        writeln!(out, "  (selected engine {chosen} is not installed; new windows use mouser)")?;
    }
    writeln!(out, "* = used by new windows (MOUSER_ENGINE or `mouser engine use`)")?;
    Ok(())
}

/// ("skipped", " — reason...") for the list columns.
fn split_state(st: &str) -> (&'static str, String) {
    if st == "ok" {
        ("ok", String::new())
    } else if let Some(r) = st.strip_prefix("SKIPPED: ") {
        ("skipped", format!("\n      -> {r}"))
    } else {
        ("failed", format!("\n      -> {}", st.strip_prefix("FAILED ").unwrap_or(st)))
    }
}
