//! `mouser model list | add | remove | test | default | local …` (docs/models.md).

use anyhow::{Result, bail};
use mouser_core::models::{
    self, Api, ChatRequest, ChatSink, Event, Message, Origin, PRESETS, Part, ProviderEntry, Role, ToolChoice, ToolSpec, hf, local,
};
use std::io::Write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    List,
    /// Add or update a provider. A preset name (openai, anthropic, …) fills `api`/`url`.
    Add {
        name: String,
        api: Option<String>,
        url: Option<String>,
        /// The API key (read from stdin by main; stored in the keychain).
        key: Option<String>,
        models: Vec<String>,
        ca: Option<String>,
        /// Use the Grok Bot desktop app's saved login (opt-in; `grok --desktop`).
        desktop: bool,
    },
    Remove(String),
    /// Stream one reply: `spec` is `provider[/model]` (default: the default model).
    Test {
        spec: Option<String>,
        prompt: String,
        /// Offer a demo tool and require a call.
        tool: bool,
    },
    /// Set the default `provider[/model]`.
    Default(String),
    /// The built-in local model (gemma-local).
    Local(LocalAction),
    /// GGUF models from Hugging Face.
    Hf(HfAction),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HfAction {
    Search(String),
    Info(String),
    /// Without `yes` it shows the size and asks on the terminal.
    Install {
        id: String,
        quant: Option<String>,
        yes: bool,
    },
    List,
    Remove(String),
}

fn run_hf(a: &HfAction, out: &mut dyn Write) -> Result<()> {
    match a {
        HfAction::Search(q) => {
            let hits = hf::search(q, hf::SEARCH_LIMIT)?;
            if hits.is_empty() {
                writeln!(out, "no GGUF models match {q:?}")?;
            }
            for h in hits {
                writeln!(out, "{:<56} ↓{:>7} ♥{:>6}  {}", h.id, hf::count(h.downloads), hf::count(h.likes), h.updated)?;
            }
            writeln!(out, "details: mouser model hf info ID   install: mouser model hf install ID [QUANT]")?;
            Ok(())
        }
        HfAction::Info(id) => {
            let d = hf::details(id)?;
            for l in hf::details_lines(&d) {
                writeln!(out, "{l}")?;
            }
            Ok(())
        }
        HfAction::Install { id, quant, yes } => {
            if local::backend().is_none() && local::prefs().url.is_none() {
                bail!("{}", local::NOT_BUILT);
            }
            let d = hf::details(id)?;
            let q = match quant {
                Some(q) => q.clone(),
                None => hf::default_quant(&d).map(|q| q.label.clone()).ok_or_else(|| anyhow::anyhow!("{id} has no GGUF files"))?,
            };
            let plan = hf::Plan::new(&d, &q)?;
            if !yes && !ask_yes(&plan.consent_text(), out)? {
                writeln!(out, "not downloaded")?;
                return Ok(());
            }
            let mut last = u64::MAX;
            let r = hf::install(hf::Consent::given(&plan), &mut |d, t| {
                let pct = d * 100 / t.max(1);
                if pct != last {
                    last = pct;
                    eprint!("\r{} {pct}% ({} of {})   ", plan.quant.label, local::human(d), local::human(t));
                }
                true
            });
            eprintln!();
            writeln!(out, "installed; default model: {}", r?)?;
            Ok(())
        }
        HfAction::List => {
            let all = hf::installed();
            if all.is_empty() {
                writeln!(out, "no Hugging Face models installed (mouser model hf search QUERY)")?;
            }
            for i in all {
                let st = if i.complete() { "ready" } else { "incomplete (install again to resume)" };
                writeln!(out, "gemma-local/{:<40} {:<40} {:>9}  {st}", i.id, i.repo, local::human(i.size()))?;
            }
            Ok(())
        }
        HfAction::Remove(id) => {
            if hf::remove(id)? {
                writeln!(out, "removed {id}")?;
            } else {
                writeln!(out, "{id} is not installed (mouser model hf list)")?;
            }
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalAction {
    Status,
    /// Download weights (default: the selected gemma model). Without `yes` it
    /// shows the size and asks on the terminal.
    Download {
        id: Option<String>,
        yes: bool,
    },
    Delete(String),
    /// Use an OpenAI/Anthropic-compatible server instead (None clears).
    Url {
        url: Option<String>,
        api: Option<String>,
        model: Option<String>,
        key: Option<String>,
    },
    /// Where model files go (None: the default).
    Dir(Option<String>),
    /// Turn gemma-local on (download with consent if needed) and make it the default.
    Enable {
        id: Option<String>,
        yes: bool,
    },
    /// Hide gemma-local (downloads are kept).
    Disable,
    /// Move downloaded weights to DIR (SHA256-checked) and use DIR from now on.
    Move(String),
}

fn selected_weights(id: Option<&str>) -> Result<&'static local::Weights> {
    let id = match id {
        Some(i) => i.to_string(),
        None => match models::default_choice() {
            Some((p, m)) if p == "gemma-local" => m,
            _ => local::CATALOG[0].id.to_string(),
        },
    };
    local::weights(&id)
        .ok_or_else(|| anyhow::anyhow!("unknown local model {id}; one of: {}", local::CATALOG.iter().map(|w| w.id).collect::<Vec<_>>().join(", ")))
}

fn ask_yes(lines: &[String], out: &mut dyn Write) -> Result<bool> {
    use std::io::IsTerminal;
    for l in lines {
        writeln!(out, "{l}")?;
    }
    if !std::io::stdin().is_terminal() {
        bail!("not downloading: confirm on a terminal, or pass --yes");
    }
    write!(out, "Download it? [y/N] ")?;
    out.flush()?;
    let mut a = String::new();
    std::io::stdin().read_line(&mut a)?;
    Ok(matches!(a.trim(), "y" | "Y" | "yes" | "Yes"))
}

fn run_local(a: &LocalAction, out: &mut dyn Write) -> Result<()> {
    match a {
        LocalAction::Status => {
            let lp = local::prefs();
            match local::backend() {
                Some(b) => writeln!(out, "engine: {} (built in)", b.describe())?,
                None => writeln!(out, "engine: not in this build (source build with --no-default-features)")?,
            }
            if let Some(u) = &lp.url {
                writeln!(out, "server: {u} ({}) — used instead of the built-in engine", lp.api.unwrap_or(Api::OpenAi).name())?;
            }
            if lp.disabled {
                writeln!(out, "state: disabled (mouser model local enable)")?;
            }
            writeln!(out, "models dir: {}", local::dir().display())?;
            writeln!(out, "  change it: mouser model local dir DIR (or [models.local] dir = \"DIR\" in config.toml);")?;
            writeln!(out, "  move what is downloaded: mouser model local move DIR")?;
            for w in &local::CATALOG {
                let st = if w.installed() {
                    "downloaded".to_string()
                } else if let Ok(m) = std::fs::metadata(w.path().with_extension("gguf.part")) {
                    format!("partial {} of {} (download resumes)", local::human(m.len()), local::human(w.size))
                } else {
                    format!("not downloaded ({}): mouser model local download {}", local::human(w.size), w.id)
                };
                writeln!(out, "  {:<12} {:<24} {st}", w.id, w.label)?;
            }
            Ok(())
        }
        LocalAction::Download { id, yes } => {
            let w = selected_weights(id.as_deref())?;
            if w.installed() {
                writeln!(out, "{} is already downloaded: {}", w.id, w.path().display())?;
                return Ok(());
            }
            fetch(w, *yes, out).map(|_| ())
        }
        LocalAction::Enable { id, yes } => {
            if local::backend().is_none() && local::prefs().url.is_none() {
                bail!("{}", local::NOT_BUILT);
            }
            let w = local::pick(id.as_deref())?;
            if !w.installed() && local::prefs().url.is_none() && !fetch(w, *yes, out)? {
                writeln!(out, "not enabled (nothing downloaded)")?;
                return Ok(());
            }
            let spec = local::enable(w)?;
            writeln!(out, "local model enabled; default model: {spec}")?;
            writeln!(out, "try: mouser model test   (or open the chat: esc :a)")?;
            Ok(())
        }
        LocalAction::Disable => {
            let was = local::disable()?;
            writeln!(out, "local model disabled (downloads kept in {}); enable again: mouser model local enable", local::dir().display())?;
            if was {
                writeln!(out, "it was the default model, so there is no default now (mouser model default PROVIDER[/MODEL])")?;
            }
            Ok(())
        }
        LocalAction::Move(d) => {
            let from = local::dir();
            let moved = local::move_to(std::path::Path::new(d), &mut |w| {
                eprintln!("moving {} ({}) and checking it…", w.file, local::human(w.size));
            })?;
            for p in &moved {
                writeln!(out, "moved and verified: {p}", p = p.display())?;
            }
            if moved.is_empty() {
                writeln!(out, "nothing to move from {}", from.display())?;
            }
            writeln!(out, "models dir: {} (saved as [models.local] dir)", local::dir().display())?;
            Ok(())
        }
        _ => run_local_rest(a, out),
    }
}

/// Ask (unless `yes`), then download and verify `w`. False if the user said no.
fn fetch(w: &local::Weights, yes: bool, out: &mut dyn Write) -> Result<bool> {
    if !yes && !ask_yes(&w.consent_text(), out)? {
        writeln!(out, "not downloaded")?;
        return Ok(false);
    }
    let url = std::env::var("MOUSER_MODEL_URL_OVERRIDE").unwrap_or_else(|_| w.url());
    let mut last = u64::MAX;
    let p = local::download(local::Consent::given(w), &url, &mut |d, t| {
        let pct = d * 100 / t.max(1);
        if pct != last {
            last = pct;
            eprint!("\r{} {pct}% ({} of {})   ", w.id, local::human(d), local::human(t));
        }
        true
    });
    eprintln!();
    let p = p?;
    writeln!(out, "downloaded and verified: {}", p.display())?;
    Ok(true)
}

fn run_local_rest(a: &LocalAction, out: &mut dyn Write) -> Result<()> {
    match a {
        LocalAction::Delete(id) => {
            let w = selected_weights(Some(id))?;
            if local::remove(w) {
                writeln!(out, "deleted {}", w.path().display())?;
            } else {
                writeln!(out, "{} was not downloaded", w.id)?;
            }
            Ok(())
        }
        LocalAction::Url { url, api, model, key } => {
            let mut prefs = models::prefs();
            let mut lp = prefs.local.clone().unwrap_or_default();
            match url {
                None => {
                    lp.url = None;
                    lp.api = None;
                    lp.model = None;
                    lp.key = None;
                    mouser_core::secrets::delete(&key_secret("gemma-local"));
                    writeln!(out, "gemma-local uses the built-in engine")?;
                }
                Some(u) => {
                    mouser_core::mcp::Target::parse(u)?;
                    let api = match api.as_deref() {
                        None => Api::OpenAi,
                        Some(s) => match Api::parse(s) {
                            Some(a @ (Api::OpenAi | Api::Anthropic)) => a,
                            _ => bail!("--api must be openai or anthropic"),
                        },
                    };
                    lp.url = Some(u.clone());
                    lp.api = Some(api);
                    lp.model = model.clone();
                    if let Some(k) = key {
                        mouser_core::secrets::set(&key_secret("gemma-local"), k.trim())?;
                        lp.key = Some(format!("keychain:{}", key_secret("gemma-local")));
                    }
                    writeln!(out, "gemma-local now uses {u} ({}); try: mouser model test gemma-local", api.name())?;
                }
            }
            prefs.local = Some(lp);
            models::save(&prefs)
        }
        LocalAction::Status | LocalAction::Download { .. } | LocalAction::Enable { .. } | LocalAction::Disable | LocalAction::Move(_) => {
            unreachable!("handled by run_local")
        }
        LocalAction::Dir(d) => {
            let mut prefs = models::prefs();
            let mut lp = prefs.local.clone().unwrap_or_default();
            lp.dir = d.clone();
            prefs.local = Some(lp);
            models::save(&prefs)?;
            writeln!(out, "models dir: {}", local::dir().display())?;
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn add(
    name: &str,
    api: Option<&str>,
    url: Option<&str>,
    key: Option<&str>,
    ms: &[String],
    ca: Option<&str>,
    desktop: bool,
    out: &mut dyn Write,
    input: Option<&mut dyn std::io::BufRead>,
) -> Result<()> {
    use mouser_core::models::{cursor, grokbot, keys};
    if !mouser_core::engines::valid_name(name) {
        bail!("name must be a-z, 0-9, - or _");
    }
    if name == "gemma-local" {
        bail!("gemma-local is the built-in local model; see `mouser model local` (switch with `mouser model default gemma-local/gemma-4-e4b`)");
    }
    let key = key.map(str::trim).filter(|k| !k.is_empty());
    if desktop && key.is_some() {
        bail!("--desktop uses the Grok Bot app's login; leave out --key-stdin");
    }
    let mut prefs = models::prefs();
    let old = prefs.providers.iter().position(|p| p.name == name);
    let preset = PRESETS.iter().find(|p| p.0 == name);
    let prev = old.map(|i| prefs.providers[i].clone());
    // `grok` picks its API from the key's shape (or the desktop login).
    let grok = if name == "grok" && api.is_none() && url.is_none() {
        if desktop { Some((Api::GrokDesktop, grokbot::URL)) } else { key.map(models::grok_target) }
    } else {
        None
    };
    if name == "grok" && grok.is_none() && prev.is_none() && api.is_none() {
        bail!(
            "grok needs a key or the desktop login:\n  printf %s \"$KEY\" | mouser model add grok --key-stdin   # grok_… (grok.com/dashboard/api-keys) or xai-… (console.x.ai)\n  mouser model add grok --desktop   # the signed-in Grok Bot desktop app"
        );
    }
    let api = match api {
        Some(s) => Api::parse(s).ok_or_else(|| anyhow::anyhow!("--api must be openai, anthropic, google or cursor-agents"))?,
        None if desktop => Api::GrokDesktop,
        None => match (grok, &prev, preset) {
            (Some((a, _)), _, _) => a,
            (None, Some(p), _) => p.api,
            (None, None, Some(p)) => p.1,
            (None, None, None) => bail!("{name} is not a built-in provider; give --api openai|anthropic|google and --url"),
        },
    };
    if api == Api::LlamaCpp {
        bail!("llama.cpp is built in as gemma-local; see `mouser model local`");
    }
    let url = match (url, grok, &prev, preset) {
        _ if api == Api::GrokDesktop => grokbot::URL.to_string(),
        (Some(u), ..) => u.to_string(),
        (None, Some((_, u)), ..) => u.to_string(),
        (None, None, Some(p), _) if p.api == api => p.url.clone(),
        (None, None, _, Some(p)) if p.1 == api => p.2.to_string(),
        (None, None, _, _) if api == Api::CursorAgents => cursor::URL.to_string(),
        (None, None, _, _) if name == "grok" && api == Api::OpenAi => models::XAI_URL.to_string(),
        _ => bail!("--url is required for a custom provider"),
    };
    if api != Api::GrokDesktop {
        mouser_core::mcp::Target::parse(&url)?;
    }
    // An explicit Cursor agents API takes grok_ keys at any URL (a proxy, a mock).
    if let Some(k) = key
        && !(api == Api::CursorAgents && k.starts_with("grok_"))
        && let Some(h) = keys::wrong_provider_hint(name, &url, k)
    {
        bail!("{h}");
    }
    if api == Api::GrokDesktop && !grokbot::present() {
        bail!("no Grok Bot desktop login found on this machine: install the Grok Bot app and sign in, then run this again");
    }
    let ca = match ca {
        Some(p) => Some(std::fs::canonicalize(p).map_err(|e| anyhow::anyhow!("{p}: {e}"))?.display().to_string()),
        None => prev.as_ref().and_then(|p| p.ca.clone()),
    };
    let same_target = prev.as_ref().is_some_and(|p| p.api == api && p.url == url);
    let preset_default = preset.filter(|p| p.1 == api || name == "grok").map(|p| p.3.to_string()).filter(|_| api != Api::CursorAgents);
    let models = if !ms.is_empty() {
        ms.to_vec()
    } else if input.is_some() || api == Api::GrokDesktop {
        // Ask the provider what it offers (key in memory only) and pick.
        let probe = ProviderEntry { name: name.to_string(), api, url: url.clone(), key: None, models: vec![], ca: ca.clone() };
        let stored = if key.is_none() && same_target {
            prev.as_ref().and_then(|p| p.key.as_deref()).map(mouser_core::mcp_client::resolve_token).transpose().ok().flatten()
        } else {
            None
        };
        let k = key.map(String::from).or(stored);
        writeln!(out, "asking {name} which models it offers…")?;
        match models::list_models(&probe, k.as_deref()) {
            Ok(list) if !list.is_empty() => {
                let want = prev.as_ref().filter(|_| same_target).and_then(|p| p.models.first().cloned()).or(preset_default.clone());
                let d = want.and_then(|w| list.iter().position(|m| m.0 == w)).unwrap_or(0);
                match input {
                    Some(i) => vec![pick_model(&list, d, out, i)?],
                    None => {
                        writeln!(
                            out,
                            "using {} (pass --model to choose; offered: {})",
                            list[d].0,
                            list.iter().map(|m| m.0.as_str()).collect::<Vec<_>>().join(", ")
                        )?;
                        vec![list[d].0.clone()]
                    }
                }
            }
            Ok(_) if api == Api::GrokDesktop => bail!("no Grok Bot agents found for this login"),
            Err(e) if api == Api::GrokDesktop => return Err(e),
            r => {
                let why = match r {
                    Err(e) => format!("{e:#}"),
                    Ok(_) => "the list is empty".into(),
                };
                let fallback = prev
                    .as_ref()
                    .filter(|_| same_target)
                    .map(|p| p.models.clone())
                    .filter(|m| !m.is_empty())
                    .or(preset_default.clone().map(|m| vec![m]));
                match fallback {
                    Some(f) => {
                        writeln!(out, "could not list the models ({why}); using {}", f[0])?;
                        f
                    }
                    None => bail!("could not list {name}'s models ({why}); name one with --model MODEL"),
                }
            }
        }
    } else if let Some(p) = prev.as_ref().filter(|_| same_target) {
        p.models.clone()
    } else {
        preset_default.map(|m| vec![m]).unwrap_or_default()
    };
    let key_ref = match key {
        Some(k) => {
            let st = mouser_core::secrets::set(&key_secret(name), k)?;
            if let mouser_core::secrets::Store::File(p) = &st {
                writeln!(out, "warning: no OS keychain available; the key is stored in {} (0600)", p.display())?;
            }
            Some(format!("keychain:{}", key_secret(name)))
        }
        None if api == Api::GrokDesktop => {
            mouser_core::secrets::delete(&key_secret(name));
            None
        }
        None => prev.as_ref().and_then(|p| p.key.clone()),
    };
    let e = ProviderEntry { name: name.to_string(), api, url, key: key_ref, models, ca };
    let how = match (api, keys::kind(key.unwrap_or(""))) {
        (Api::GrokDesktop, _) => " (Grok Bot desktop login)".to_string(),
        (Api::CursorAgents, _) => " (grok.com key → api.cursor.com agents)".to_string(),
        (Api::OpenAi, keys::KeyKind::Xai) if name == "grok" => " (xAI key → api.x.ai)".to_string(),
        _ => String::new(),
    };
    match old {
        Some(i) => prefs.providers[i] = e.clone(),
        None => prefs.providers.push(e.clone()),
    }
    models::save(&prefs)?;
    if let Some(m) = e.models.first() {
        writeln!(out, "model: {m}")?;
    }
    writeln!(out, "{} model provider {name}{how}; try: mouser model test {name}", if old.is_some() { "updated" } else { "added" })?;
    Ok(())
}

pub fn key_secret(name: &str) -> String {
    format!("model-{name}")
}

/// The terminal to ask on, when there is one: stdin if it is a terminal, else
/// the controlling terminal (`--key-stdin` pipes the key in).
pub fn terminal_input() -> Option<std::io::BufReader<Box<dyn std::io::Read>>> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        return Some(std::io::BufReader::new(Box::new(std::io::stdin())));
    }
    if !std::io::stdout().is_terminal() {
        return None;
    }
    #[cfg(unix)]
    let tty = std::fs::File::open("/dev/tty").ok();
    #[cfg(windows)]
    let tty = std::fs::File::open("CONIN$").ok();
    #[cfg(not(any(unix, windows)))]
    let tty: Option<std::fs::File> = None;
    tty.map(|f| std::io::BufReader::new(Box::new(f) as Box<dyn std::io::Read>))
}

/// Ask which model to use: a numbered list, Enter takes `default`.
pub fn pick_model(list: &[(String, String)], default: usize, out: &mut dyn Write, input: &mut dyn std::io::BufRead) -> Result<String> {
    writeln!(out, "Models offered:")?;
    for (i, (_, label)) in list.iter().enumerate() {
        writeln!(out, "{}{:>3}) {label}", if i == default { "*" } else { " " }, i + 1)?;
    }
    loop {
        write!(out, "Pick a model [1-{}, Enter = {}]: ", list.len(), default + 1)?;
        out.flush()?;
        let mut a = String::new();
        if input.read_line(&mut a)? == 0 {
            bail!("no model picked (pass --model MODEL)");
        }
        let a = a.trim();
        if a.is_empty() {
            return Ok(list[default].0.clone());
        }
        if let Ok(n) = a.parse::<usize>()
            && (1..=list.len()).contains(&n)
        {
            return Ok(list[n - 1].0.clone());
        }
        if let Some(m) = list.iter().find(|m| m.0 == a) {
            return Ok(m.0.clone());
        }
        writeln!(out, "not one of the list: {a}")?;
    }
}

pub fn run(a: &Action, out: &mut dyn Write) -> Result<()> {
    run_io(a, out, None)
}

/// [`run`] with a terminal to ask on: `mouser model add P` without `--model`
/// then lists the provider's models and asks which one to use.
pub fn run_io(a: &Action, out: &mut dyn Write, input: Option<&mut dyn std::io::BufRead>) -> Result<()> {
    if let Action::Add { name, api, url, key, models: ms, ca, desktop } = a {
        return add(name, api.as_deref(), url.as_deref(), key.as_deref(), ms, ca.as_deref(), *desktop, out, input);
    }
    match a {
        Action::Hf(h) => run_hf(h, out),
        Action::List => {
            let d = models::default_choice();
            for p in models::all() {
                let e = &p.entry;
                let state = match p.origin {
                    Origin::Configured if e.key.is_some() => "ready (key in keychain)".to_string(),
                    Origin::Configured => "ready (no key)".to_string(),
                    Origin::Preset => format!("add a key: mouser model add {} --key-stdin", e.name),
                    Origin::Plugin => "plugin".to_string(),
                    Origin::Local => {
                        let lp = local::prefs();
                        match (&lp.url, local::backend()) {
                            (Some(u), _) => format!("via {u}"),
                            (None, None) => "not in this build (--no-default-features); or mouser model local url URL".to_string(),
                            (None, Some(_)) => {
                                let have: Vec<&str> = local::CATALOG.iter().filter(|w| w.installed()).map(|w| w.id).collect();
                                if have.is_empty() {
                                    "built in; download: mouser model local download".to_string()
                                } else {
                                    format!("built in; downloaded: {}", have.join(", "))
                                }
                            }
                        }
                    }
                };
                let mark = if d.as_ref().is_some_and(|(n, _)| *n == e.name) { "*" } else { " " };
                writeln!(out, "{mark} {:<12} {:<9} {:<44} {state}", e.name, e.api.name(), if e.url.is_empty() { "(built in)" } else { &e.url })?;
                let models: Vec<String> = e
                    .models
                    .iter()
                    .map(|m| if d.as_ref().is_some_and(|(n, dm)| *n == e.name && dm == m) { format!("{m} (default)") } else { m.clone() })
                    .collect();
                if !models.is_empty() {
                    writeln!(out, "    models: {}", models.join(", "))?;
                }
            }
            if d.is_none() {
                writeln!(out, "no default model yet (mouser model default PROVIDER[/MODEL])")?;
            }
            Ok(())
        }
        Action::Add { .. } => unreachable!("handled by run_io"),
        Action::Remove(name) => {
            let mut prefs = models::prefs();
            let Some(i) = prefs.providers.iter().position(|p| p.name == *name) else {
                bail!("no configured provider {name} (see `mouser model list`)")
            };
            prefs.providers.remove(i);
            if prefs.default.as_deref().is_some_and(|d| models::split(d).0 == name) {
                prefs.default = None;
            }
            models::save(&prefs)?;
            mouser_core::secrets::delete(&key_secret(name));
            writeln!(out, "removed {name} (and its key)")?;
            Ok(())
        }
        Action::Default(spec) => {
            let spec = &models::resolve(spec)?;
            let (prov, m) = models::split(spec);
            let Some(p) = models::find(prov) else { bail!("no model provider {prov} (see `mouser model list`)") };
            if let Some(m) = m
                && p.origin == Origin::Local
                && !p.entry.models.iter().any(|x| x == m)
            {
                bail!("{prov} offers {}", p.entry.models.join(", "));
            }
            let mut prefs = models::prefs();
            prefs.default = Some(spec.clone());
            models::save(&prefs)?;
            let (n, m) = models::default_choice().unwrap_or_default();
            writeln!(out, "default model: {n}/{m}")?;
            if p.origin == Origin::Local
                && local::prefs().url.is_none()
                && let Some(w) = local::weights(&m)
                && !w.installed()
            {
                writeln!(out, "note: {} is not downloaded yet ({}): mouser model local download {}", w.id, local::human(w.size), w.id)?;
            }
            Ok(())
        }
        Action::Local(l) => run_local(l, out),
        Action::Test { spec, prompt, tool } => {
            let (prov, model) = match spec {
                Some(s) => {
                    let s = models::resolve(s)?;
                    let (p, m) = models::split(&s);
                    let m = match m {
                        Some(m) => m.to_string(),
                        None => models::find(p).and_then(|x| x.default_model().map(String::from)).unwrap_or_default(),
                    };
                    (p.to_string(), m)
                }
                None => models::default_choice().ok_or_else(|| anyhow::anyhow!("no default model; name one: mouser model test PROVIDER[/MODEL]"))?,
            };
            if model.is_empty() {
                bail!("{prov} lists no models; name one: mouser model test {prov}/MODEL");
            }
            let p = models::open(&prov)?;
            let mut req = ChatRequest { model: model.clone(), messages: vec![Message::user(prompt)], max_tokens: Some(256), ..Default::default() };
            if *tool {
                req.tools = vec![ToolSpec {
                    name: "get_time".into(),
                    description: "Get the current time in a city".into(),
                    schema: serde_json::json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}),
                }];
                req.tool_choice = ToolChoice::Required;
            }
            writeln!(out, "{prov}/{model}:")?;
            struct Print<'a>(&'a mut dyn Write, std::time::Instant, models::Collect);
            impl ChatSink for Print<'_> {
                fn event(&mut self, e: Event) {
                    self.2.event(e.clone());
                    let _ = match e {
                        Event::Text(t) => write!(self.0, "{t}"),
                        Event::ToolStart { name, id, .. } => write!(self.0, "\n[tool call {name} {id}] "),
                        Event::ToolArgs { delta, .. } => write!(self.0, "{delta}"),
                        Event::Usage { input, output } => {
                            write!(self.0, "\n[usage: {input} in, {output} out; {:.2}s]", self.1.elapsed().as_secs_f64())
                        }
                        Event::Done { stop } => writeln!(self.0, "\n[done: {stop}]"),
                    };
                    let _ = self.0.flush();
                }
            }
            let mut pr = Print(out, std::time::Instant::now(), models::Collect::default());
            p.chat(&req, &mut pr)?;
            // With --tool, answer the call (a demo time) and print the model's reply.
            if *tool && !pr.2.calls.is_empty() {
                req.messages.push(pr.2.message());
                let results =
                    pr.2.calls
                        .iter()
                        .map(|(id, name, _)| Part::ToolResult { id: id.clone(), name: name.clone(), content: "14:05 (demo)".into(), is_error: false })
                        .collect();
                req.messages.push(Message { role: Role::User, parts: results });
                req.tool_choice = ToolChoice::Auto;
                writeln!(pr.0, "[tool result: 14:05 (demo)]")?;
                pr.1 = std::time::Instant::now();
                pr.2 = models::Collect::default();
                p.chat(&req, &mut pr)?;
            }
            Ok(())
        }
    }
}
