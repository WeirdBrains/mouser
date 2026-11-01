//! `mouser model list | add | remove | test | default` (docs/models.md).

use anyhow::{Result, bail};
use mouser_core::models::{self, Api, ChatRequest, ChatSink, Event, Message, Origin, PRESETS, ProviderEntry, ToolChoice, ToolSpec};
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
                    Origin::Stub => "not built yet (P4b)".to_string(),
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
                    "gemma-local is the built-in local model (P4b, not built yet); switch its model with `mouser model default gemma-local/gemma-4-e4b`"
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
                bail!("llama.cpp providers are built in (P4b)");
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
            let (prov, m) = models::split(spec);
            let Some(p) = models::find(prov) else { bail!("no model provider {prov} (see `mouser model list`)") };
            if let Some(m) = m
                && p.origin == Origin::Stub
                && !p.entry.models.iter().any(|x| x == m)
            {
                bail!("{prov} offers {}", p.entry.models.join(", "));
            }
            let mut prefs = models::prefs();
            prefs.default = Some(spec.clone());
            models::save(&prefs)?;
            let (n, m) = models::default_choice().unwrap_or_default();
            writeln!(out, "default model: {n}/{m}")?;
            if p.origin == Origin::Stub {
                writeln!(out, "note: {prov} is not built yet (P4b)")?;
            }
            Ok(())
        }
        Action::Test { spec, prompt, tool } => {
            let (prov, model) = match spec {
                Some(s) => {
                    let (p, m) = models::split(s);
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
            struct Print<'a>(&'a mut dyn Write);
            impl ChatSink for Print<'_> {
                fn event(&mut self, e: Event) {
                    let _ = match e {
                        Event::Text(t) => write!(self.0, "{t}"),
                        Event::ToolStart { name, id, .. } => write!(self.0, "\n[tool call {name} {id}] "),
                        Event::ToolArgs { delta, .. } => write!(self.0, "{delta}"),
                        Event::Usage { input, output } => write!(self.0, "\n[usage: {input} in, {output} out]"),
                        Event::Done { stop } => writeln!(self.0, "\n[done: {stop}]"),
                    };
                    let _ = self.0.flush();
                }
            }
            p.chat(&req, &mut Print(out))
        }
    }
}
