//! `mouser model list | add | remove | test | default | local …` (docs/models.md).

use anyhow::{Result, bail};
use mouser_core::models::{
    self, Api, ChatRequest, ChatSink, Event, Message, Origin, PRESETS, Part, ProviderEntry, Role, ToolChoice, ToolSpec, local,
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

pub fn key_secret(name: &str) -> String {
    format!("model-{name}")
}

pub fn run(a: &Action, out: &mut dyn Write) -> Result<()> {
    match a {
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
        Action::Add { name, api, url, key, models: ms, ca } => {
            if !mouser_core::engines::valid_name(name) {
                bail!("name must be a-z, 0-9, - or _");
            }
            if name == "gemma-local" {
                bail!(
                    "gemma-local is the built-in local model; see `mouser model local` (switch with `mouser model default gemma-local/gemma-4-e4b`)"
                );
            }
            let mut prefs = models::prefs();
            let old = prefs.providers.iter().position(|p| p.name == *name);
            let preset = PRESETS.iter().find(|p| p.0 == *name);
            let prev = old.map(|i| prefs.providers[i].clone());
            let api = match api.as_deref() {
                Some(s) => Api::parse(s).ok_or_else(|| anyhow::anyhow!("--api must be openai, anthropic or google"))?,
                None => match (&prev, preset) {
                    (Some(p), _) => p.api,
                    (None, Some(p)) => p.1,
                    (None, None) => bail!("{name} is not a built-in provider; give --api openai|anthropic|google and --url"),
                },
            };
            if api == Api::LlamaCpp {
                bail!("llama.cpp is built in as gemma-local; see `mouser model local`");
            }
            let url = match (url, &prev, preset) {
                (Some(u), _, _) => u.clone(),
                (None, Some(p), _) => p.url.clone(),
                (None, None, Some(p)) => p.2.to_string(),
                (None, None, None) => bail!("--url is required for a custom provider"),
            };
            mouser_core::mcp::Target::parse(&url)?;
            let models = if !ms.is_empty() {
                ms.clone()
            } else if let Some(p) = &prev {
                p.models.clone()
            } else {
                preset.map(|p| vec![p.3.to_string()]).unwrap_or_default()
            };
            let key_ref = match key {
                Some(k) => {
                    let st = mouser_core::secrets::set(&key_secret(name), k.trim())?;
                    if let mouser_core::secrets::Store::File(p) = &st {
                        writeln!(out, "warning: no OS keychain available; the key is stored in {} (0600)", p.display())?;
                    }
                    Some(format!("keychain:{}", key_secret(name)))
                }
                None => prev.as_ref().and_then(|p| p.key.clone()),
            };
            let ca = match ca {
                Some(p) => Some(std::fs::canonicalize(p).map_err(|e| anyhow::anyhow!("{p}: {e}"))?.display().to_string()),
                None => prev.as_ref().and_then(|p| p.ca.clone()),
            };
            let e = ProviderEntry { name: name.clone(), api, url, key: key_ref, models, ca };
            match old {
                Some(i) => prefs.providers[i] = e,
                None => prefs.providers.push(e),
            }
            models::save(&prefs)?;
            writeln!(out, "{} model provider {name}; try: mouser model test {name}", if old.is_some() { "updated" } else { "added" })?;
            Ok(())
        }
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
