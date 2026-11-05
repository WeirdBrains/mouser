//! The verbs `mouser plugin` and `mouser engine` share (plan §17, the command
//! consistency rule): one implementation, parameterised by kind, behind the
//! CLI, the `esc :plugin …`/`esc :engine …` line and `--install`
//! (docs/downloads.md).

use anyhow::{Context, Result, bail};
use mouser_core::downloads;
use mouser_core::install::{self, Kind, Src};
use std::io::{IsTerminal, Write};
use std::path::Path;

/// Every verb both surfaces have, in help order.
pub const VERBS: [&str; 7] = ["install", "update", "rollback", "remove", "list", "retry", "cancel"];

/// Short `esc :` forms: `:pi` = `:plugin install`, `:eu` = `:engine update`, ….
pub const SHORT: [(&str, &str); 7] =
    [("i", "install"), ("u", "update"), ("rb", "rollback"), ("rm", "remove"), ("l", "list"), ("r", "retry"), ("c", "cancel")];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    Install(String),
    /// `None`: every item installed with `install`.
    Update(Option<String>),
    Rollback(String),
    Remove(String),
    List,
    Retry,
    /// `None`: all.
    Cancel(Option<u32>),
}

/// Flags every verb accepts (`--yes`, `--version`, `--wait`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flags {
    pub yes: bool,
    pub version: Option<String>,
    /// The CLI always waits; accepted for the same shape as `--install`.
    pub wait: bool,
}

static CLI_FLAGS: std::sync::Mutex<Option<Flags>> = std::sync::Mutex::new(None);

/// The flags of this CLI run (`--yes`, `--version`), for the plugin and engine actions.
pub fn set_cli_flags(f: Flags) {
    *CLI_FLAGS.lock().unwrap() = Some(f);
}

pub fn cli_flags() -> Flags {
    CLI_FLAGS.lock().unwrap().clone().unwrap_or_default()
}

/// Parse `install SRC --yes`, `update [NAME|--all]`, … (the `esc :` line).
pub fn parse(words: &[&str]) -> Result<(Verb, Flags)> {
    let mut f = Flags::default();
    let mut rest = Vec::new();
    let mut it = words.iter();
    while let Some(w) = it.next() {
        match *w {
            "--yes" | "-y" => f.yes = true,
            "--wait" => f.wait = true,
            "--all" => {}
            "--version" => f.version = Some(it.next().context("--version needs a value")?.to_string()),
            w => rest.push(w),
        }
    }
    let Some((&v, args)) = rest.split_first() else { bail!("verbs: {}", VERBS.join(", ")) };
    let v = SHORT.iter().find(|(s, _)| *s == v).map(|(_, l)| *l).unwrap_or(v);
    let one = |what: &str| -> Result<String> { args.first().map(|s| s.to_string()).with_context(|| format!("{v} needs {what}")) };
    let verb = match v {
        "install" => Verb::Install(one("a name, gh:Owner/Repo, an https:// URL, a path or crate:NAME")?),
        "update" => Verb::Update(args.first().map(|s| s.to_string())),
        "rollback" => Verb::Rollback(one("a name")?),
        "remove" | "uninstall" => Verb::Remove(one("a name")?),
        "list" => Verb::List,
        "retry" => Verb::Retry,
        "cancel" => Verb::Cancel(match args.first() {
            None | Some(&"all") => None,
            Some(id) => Some(id.trim_start_matches('#').parse().with_context(|| format!("cancel takes a job id or all, not {id}"))?),
        }),
        v => bail!("unknown verb {v:?}; verbs: {}", VERBS.join(", ")),
    };
    Ok((verb, f))
}

/// Run one verb now (the CLI). `kind` None: `mouser plugin` without
/// `--kind`, where an engine folder still installs as an engine.
pub fn run(kind: Kind, verb: &Verb, flags: &Flags, out: &mut dyn Write) -> Result<()> {
    install::set_engine_probe(probe_isolated);
    match verb {
        Verb::Install(s) => {
            let src = Src::parse(s)?;
            let kind = match (&src, kind) {
                (Src::Path(p), Kind::Plugin) if is_engine_source(p) => Kind::Engine,
                _ => kind,
            };
            let r = install::install(&src, kind, opts(flags), &mut progress(), &mut confirm(flags, out));
            let i = r?;
            report(&i, out)
        }
        Verb::Update(name) => {
            let names = match name {
                Some(n) => vec![n.clone()],
                None => install::updatable(kind),
            };
            if names.is_empty() {
                writeln!(out, "nothing to update: no {} was installed with `mouser {} install`", kind.name(), kind.name())?;
            }
            let mut failed = 0;
            for n in names {
                let r = install::update(kind, &n, opts(flags), &mut progress(), &mut confirm(flags, out));
                match r {
                    Ok(Some(i)) => report(&i, out)?,
                    Ok(None) => writeln!(out, "{n} is up to date ({})", install::current(kind, &n).unwrap_or_default())?,
                    Err(e) => {
                        failed += 1;
                        writeln!(out, "{n}: {e:#}")?;
                    }
                }
            }
            if failed > 0 {
                bail!("{failed} update(s) failed");
            }
            Ok(())
        }
        Verb::Rollback(n) => {
            let v = install::rollback(kind, n)?;
            writeln!(out, "{n} rolled back to {v}")?;
            Ok(())
        }
        Verb::Remove(n) => match kind {
            Kind::Engine => crate::engine_cli::run(&crate::engine_cli::Action::Uninstall(n.clone()), out),
            Kind::Plugin => crate::plugin_cli::run(&crate::plugin_cli::Action::Uninstall(n.clone()), out),
        },
        Verb::List => match kind {
            Kind::Engine => crate::engine_cli::run(&crate::engine_cli::Action::List, out),
            Kind::Plugin => crate::plugin_cli::run(&crate::plugin_cli::Action::List, out),
        },
        Verb::Retry => {
            let failed: Vec<downloads::Job> =
                downloads::saved_jobs().into_iter().filter(|j| j.kind == kind && matches!(j.state, downloads::State::Failed(_))).collect();
            if failed.is_empty() {
                writeln!(out, "no failed {} downloads to retry", kind.name())?;
                return Ok(());
            }
            let m = downloads::Manager::new(false);
            let f = flags.clone();
            m.set_asker(std::sync::Arc::new(move |_| f.yes));
            for j in &failed {
                m.enqueue(j.kind, &j.src, j.op.clone());
            }
            m.wait();
            for j in m.jobs() {
                writeln!(out, "{}", j.line())?;
            }
            Ok(())
        }
        Verb::Cancel(id) => {
            let jobs: Vec<downloads::Job> =
                downloads::saved_jobs().into_iter().filter(|j| j.kind == kind && !j.state.finished() && id.is_none_or(|i| i == j.id)).collect();
            if jobs.is_empty() {
                bail!("no running {} download{}", kind.name(), id.map(|i| format!(" #{i}")).unwrap_or_default());
            }
            for j in jobs {
                downloads::request_cancel(j.id)?;
                writeln!(out, "asked Mouser to cancel #{} {}", j.id, j.src)?;
            }
            Ok(())
        }
    }
}

fn is_engine_source(p: &Path) -> bool {
    let d = if p.is_dir() { p.to_path_buf() } else { p.parent().map(Path::to_path_buf).unwrap_or_default() };
    !p.to_string_lossy().ends_with(".tar.gz") && !p.to_string_lossy().ends_with(".zip") && mouser_core::engines::is_engine_dir(&d)
}

fn opts(f: &Flags) -> install::Opts<'_> {
    install::Opts { yes: f.yes, version: f.version.as_deref(), first_run: false }
}

fn progress() -> impl FnMut(u64, Option<u64>) -> bool {
    let mut shown = 0u64;
    move |done, total| {
        if let Some(t) = total.filter(|t| *t > 0)
            && std::io::stderr().is_terminal()
        {
            let pct = done * 100 / t;
            if pct >= shown + 10 || done == t {
                shown = pct;
                eprint!("\r{pct:>3}%");
                if done == t {
                    eprintln!();
                }
            }
        }
        true
    }
}

/// The card on the terminal: `y` installs; without a terminal it needs --yes.
fn confirm<'a>(f: &'a Flags, out: &'a mut dyn Write) -> impl FnMut(&install::Card) -> bool + 'a {
    move |c: &install::Card| {
        if f.yes {
            return true;
        }
        for l in c.lines() {
            let _ = writeln!(out, "{l}");
        }
        if !std::io::stdin().is_terminal() {
            let _ = writeln!(out, "(not a terminal: add --yes to install)");
            return false;
        }
        let _ = write!(out, "Install? [y/N] ");
        let _ = out.flush();
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
        s.trim().eq_ignore_ascii_case("y") || s.trim().eq_ignore_ascii_case("yes")
    }
}

fn report(i: &install::Installed, out: &mut dyn Write) -> Result<()> {
    match i.kind {
        Kind::Engine => {
            writeln!(out, "installed {} {} in {}", i.name, i.version, i.dir.display())?;
            writeln!(out, "use it with: mouser engine use {}", i.name)?;
        }
        Kind::Plugin => {
            let m = mouser_core::plugins::find(&i.name).map(|p| p.manifest);
            let kinds = m.as_ref().map(|m| m.kinds().iter().map(|k| k.name()).collect::<Vec<_>>().join(",")).unwrap_or_default();
            writeln!(out, "installed {} {} ({kinds}) in {}", i.name, i.version, i.dir.display())?;
            if let Some(m) = m {
                if !m.permissions.is_empty() {
                    writeln!(out, "it asks for:")?;
                    for c in &m.permissions {
                        writeln!(out, "  {c:<16} {}", mouser_core::plugins::capability_text(c))?;
                    }
                    writeln!(out, "Mouser asks you before each is used; `mouser plugin permissions {}` to see or change", m.name)?;
                }
                if !m.run.commands.is_empty() {
                    writeln!(out, "[mouser] line commands: {}", m.run.commands.join(", "))?;
                }
            }
        }
    }
    writeln!(out, "{}", i.trust.text())?;
    if let Some(r) = &i.replaced {
        writeln!(out, "replaced {r}; `mouser {} rollback {}` goes back", i.kind.name(), i.name)?;
    }
    for w in &i.warnings {
        writeln!(out, "warning: {w}")?;
    }
    Ok(())
}

/// Probe an engine library in a child process (`mouser __engine-probe LIB`),
/// never in this one: a broken library can't take Mouser down.
pub fn probe_isolated(lib: &Path) -> Result<String> {
    let exe = match std::env::var_os("MOUSER_TEST_PROBE_EXE") {
        Some(e) => std::path::PathBuf::from(e),
        None => std::env::current_exe()?,
    };
    let o = std::process::Command::new(exe).arg("__engine-probe").arg(lib).output().context("starting the engine probe")?;
    if !o.status.success() {
        let e = String::from_utf8_lossy(&o.stderr);
        bail!("{}", e.trim().strip_prefix("mouser engine probe: ").unwrap_or(e.trim()));
    }
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// The `__engine-probe` child: load the library, print its name.
pub fn probe_main(lib: &Path) -> i32 {
    match mouser_render::engine::probe(lib) {
        Ok(n) => {
            println!("{n}");
            0
        }
        Err(e) => {
            eprintln!("mouser engine probe: {e:#}");
            1
        }
    }
}
