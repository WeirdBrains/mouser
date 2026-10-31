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

fn list(out: &mut dyn Write) -> Result<()> {
    let chosen = engines::chosen();
    let mark = |n: &str| if n == chosen { "*" } else { " " };
    for (n, d) in mouser_render::engine::available_builtins() {
        writeln!(out, "{} {n:<12} built-in   {d}", mark(n))?;
    }
    for e in engines::installed() {
        match e {
            Ok(i) => {
                let m = &i.manifest;
                let status = if engines::crashed_last_time(&m.name) { " [crashed last start; `mouser engine use` to retry]" } else { "" };
                writeln!(out, "{} {:<12} {:<10} {}{status}", mark(&m.name), m.name, m.version, m.description)?;
            }
            Err((dir, why)) => writeln!(out, "  {dir:<12} broken     {why}")?,
        }
    }
    Ok(())
}
