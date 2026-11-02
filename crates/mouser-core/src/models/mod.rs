//! Model providers (P4, docs/models.md). One interface, [`ModelProvider`], with
//! streaming and tool calling; three wire adapters (OpenAI-compatible,
//! Anthropic Messages, Google Gemini). Every model is a plugin of kind
//! `model-provider`: built-in presets, `[[models.providers]]` entries (any
//! compatible URL), and installed plugins with a `[model]` table.

pub mod anthropic;
pub mod google;
pub mod http;
pub mod local;
pub mod openai;
pub mod setup;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------- the interface ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub tools: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Part {
    Text(String),
    /// The model asked for a tool (assistant).
    ToolCall {
        id: String,
        name: String,
        args: Value,
    },
    /// Our answer to a tool call (user); `name` is the tool's (Gemini needs it).
    ToolResult {
        id: String,
        name: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
}

impl Message {
    pub fn user(t: &str) -> Self {
        Message { role: Role::User, parts: vec![Part::Text(t.into())] }
    }
    pub fn text(&self) -> String {
        self.parts.iter().filter_map(|p| if let Part::Text(t) = p { Some(t.as_str()) } else { None }).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments.
    pub schema: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    Required,
    Tool(String),
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub tool_choice: ToolChoice,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

/// What a provider streams.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Text(String),
    /// A tool call begins (`index` orders calls within the reply).
    ToolStart {
        index: usize,
        id: String,
        name: String,
    },
    /// More JSON for that call's arguments.
    ToolArgs {
        index: usize,
        delta: String,
    },
    Usage {
        input: u64,
        output: u64,
    },
    /// The reply is complete: why it stopped (`end`, `tool_use`, `max_tokens`, …).
    Done {
        stop: String,
    },
}

pub trait ChatSink {
    fn event(&mut self, e: Event);
    /// Polled between stream events; true stops the request.
    fn cancelled(&self) -> bool {
        false
    }
}

pub trait ModelProvider: Send + Sync {
    fn id(&self) -> &str;
    fn models(&self) -> Vec<ModelInfo>;
    fn chat(&self, req: &ChatRequest, sink: &mut dyn ChatSink) -> Result<()>;
}

/// A sink that assembles the reply (and can cancel after `limit` text events).
#[derive(Debug, Default)]
pub struct Collect {
    pub text: String,
    /// (id, name, raw JSON args)
    pub calls: Vec<(String, String, String)>,
    pub usage: Option<(u64, u64)>,
    pub stop: Option<String>,
    pub events: usize,
    pub cancel_after: Option<usize>,
}

impl ChatSink for Collect {
    fn event(&mut self, e: Event) {
        self.events += 1;
        match e {
            Event::Text(t) => self.text.push_str(&t),
            Event::ToolStart { index, id, name } => {
                while self.calls.len() <= index {
                    self.calls.push(Default::default());
                }
                self.calls[index].0 = id;
                self.calls[index].1 = name;
            }
            Event::ToolArgs { index, delta } => {
                while self.calls.len() <= index {
                    self.calls.push(Default::default());
                }
                self.calls[index].2.push_str(&delta);
            }
            Event::Usage { input, output } => self.usage = Some((input, output)),
            Event::Done { stop } => self.stop = Some(stop),
        }
    }
    fn cancelled(&self) -> bool {
        self.cancel_after.is_some_and(|n| self.events >= n)
    }
}

impl Collect {
    /// The assistant message (text + tool calls with parsed args).
    pub fn message(&self) -> Message {
        let mut parts = vec![];
        if !self.text.is_empty() {
            parts.push(Part::Text(self.text.clone()));
        }
        for (id, name, args) in &self.calls {
            let args = if args.trim().is_empty() {
                Value::Object(Default::default())
            } else {
                serde_json::from_str(args).unwrap_or(Value::String(args.clone()))
            };
            parts.push(Part::ToolCall { id: id.clone(), name: name.clone(), args });
        }
        Message { role: Role::Assistant, parts }
    }
}

// ---------- configuration ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Api {
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "google")]
    Google,
    /// Built into Mouser (P4b; not built yet).
    #[serde(rename = "llama.cpp")]
    LlamaCpp,
}

impl Api {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "openai" | "openai-compatible" => Api::OpenAi,
            "anthropic" | "anthropic-compatible" => Api::Anthropic,
            "google" | "gemini" => Api::Google,
            "llama.cpp" => Api::LlamaCpp,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Api::OpenAi => "openai",
            Api::Anthropic => "anthropic",
            Api::Google => "google",
            Api::LlamaCpp => "llama.cpp",
        }
    }
}

/// `[[models.providers]]` in preferences.toml.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderEntry {
    pub name: String,
    pub api: Api,
    pub url: String,
    /// `keychain:NAME`; none for local servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Models offered (the first is the default).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    /// PEM to trust for a self-signed https URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca: Option<String>,
}

/// `[models]` in preferences.toml.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModelsPrefs {
    /// `provider/model` (or just `provider`) used when none is named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<ProviderEntry>,
    /// `[models.local]`: the built-in local model's options (P4b).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<local::LocalPrefs>,
}

/// `[model]` in a model-provider plugin's plugin.toml.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSection {
    pub api: Api,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Built-in model-provider plugins: (name, api, url, default model, description).
pub const PRESETS: [(&str, Api, &str, &str, &str); 6] = [
    ("openai", Api::OpenAi, "https://api.openai.com/v1", "gpt-5", "OpenAI (Chat Completions)"),
    ("xai", Api::OpenAi, "https://api.x.ai/v1", "grok-4", "xAI Grok (OpenAI-compatible)"),
    ("openrouter", Api::OpenAi, "https://openrouter.ai/api/v1", "openrouter/auto", "OpenRouter (OpenAI-compatible)"),
    ("moonshot", Api::OpenAi, "https://api.moonshot.ai/v1", "kimi-k2.6", "Moonshot Kimi (OpenAI-compatible)"),
    ("anthropic", Api::Anthropic, "https://api.anthropic.com", "claude-sonnet-4-5", "Anthropic Messages"),
    ("google", Api::Google, "https://generativelanguage.googleapis.com", "gemini-2.5-flash", "Google Gemini (native)"),
];

pub const GEMMA_MANIFEST: &str = include_str!("gemma-local.toml");

/// The stubbed local provider's declared models: (default, all).
pub fn gemma_models() -> (String, Vec<String>) {
    #[derive(Deserialize)]
    struct M {
        model: ModelSection,
    }
    let m: M = toml::from_str(GEMMA_MANIFEST).expect("gemma-local.toml");
    (m.model.default.clone().unwrap_or_default(), m.model.models)
}

pub fn prefs() -> ModelsPrefs {
    crate::storage::load_preferences().models.unwrap_or_default()
}

pub fn save(m: &ModelsPrefs) -> Result<()> {
    crate::storage::update_preferences(|p| p.models = Some(m.clone()))?;
    Ok(())
}

/// Where a provider comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// A built-in preset, not configured yet (needs a key: `mouser model add NAME --key-stdin`).
    Preset,
    /// `[[models.providers]]`.
    Configured,
    /// An installed plugin's `[model]` table.
    Plugin,
    /// The built-in local model (`gemma-local`, P4b).
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub entry: ProviderEntry,
    pub origin: Origin,
    pub description: String,
}

impl Provider {
    pub fn default_model(&self) -> Option<&str> {
        self.entry.models.first().map(String::as_str)
    }
}

/// Every provider Mouser knows, configured first.
pub fn all() -> Vec<Provider> {
    let p = prefs();
    let mut out: Vec<Provider> =
        p.providers.iter().map(|e| Provider { entry: e.clone(), origin: Origin::Configured, description: preset_desc(&e.name) }).collect();
    let have = |out: &Vec<Provider>, n: &str| out.iter().any(|p| p.entry.name == n);
    for (n, api, url, m, d) in PRESETS {
        if !have(&out, n) {
            out.push(Provider {
                entry: ProviderEntry { name: n.into(), api, url: url.into(), key: None, models: vec![m.into()], ca: None },
                origin: Origin::Preset,
                description: d.into(),
            });
        }
    }
    if !have(&out, "gemma-local") && !p.local.as_ref().is_some_and(|l| l.disabled) {
        let (d, all) = gemma_models();
        let mut models = vec![d.clone()];
        models.extend(all.into_iter().filter(|m| *m != d));
        out.push(Provider {
            entry: ProviderEntry { name: "gemma-local".into(), api: Api::LlamaCpp, url: String::new(), key: None, models, ca: None },
            origin: Origin::Local,
            description: "Gemma 4 on llama.cpp, built in (local-llm)".into(),
        });
    }
    for i in crate::plugins::installed().into_iter().flatten() {
        if let Some(m) = &i.manifest.model
            && crate::plugins::is_enabled(&i.manifest.name)
            && !have(&out, &i.manifest.name)
        {
            let mut models = m.models.clone();
            if let Some(d) = &m.default {
                models.retain(|x| x != d);
                models.insert(0, d.clone());
            }
            out.push(Provider {
                entry: ProviderEntry {
                    name: i.manifest.name.clone(),
                    api: m.api,
                    url: m.url.clone().unwrap_or_default(),
                    key: None,
                    models,
                    ca: None,
                },
                origin: Origin::Plugin,
                description: i.manifest.description.clone(),
            });
        }
    }
    out
}

fn preset_desc(n: &str) -> String {
    PRESETS.iter().find(|p| p.0 == n).map_or("custom", |p| p.4).to_string()
}

pub fn find(name: &str) -> Option<Provider> {
    all().into_iter().find(|p| p.entry.name == name)
}

/// Split `provider/model` (the model part may itself contain `/`).
pub fn split(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once('/') {
        Some((p, m)) => (p, Some(m)),
        None => (spec, None),
    }
}

/// Resolve `mouser model default|test SPEC`: `provider[/model]`, or a bare
/// model name offered by exactly one provider.
pub fn resolve(spec: &str) -> Result<String> {
    let (p, m) = split(spec);
    if find(p).is_some() {
        return Ok(spec.to_string());
    }
    if m.is_none() {
        let hits: Vec<String> = all().into_iter().filter(|x| x.entry.models.iter().any(|y| y == p)).map(|x| x.entry.name).collect();
        match hits.len() {
            1 => return Ok(format!("{}/{p}", hits[0])),
            0 => {}
            _ => bail!("{p} is offered by {}; name one: PROVIDER/{p}", hits.join(", ")),
        }
        if local::weights(p).is_some() {
            bail!("{p} is the built-in local model, which is disabled: mouser model local enable {p}");
        }
    }
    bail!("no model provider or model named {p} (see `mouser model list`)")
}

/// The default `(provider, model)`: `models.default`, else the first configured provider.
pub fn default_choice() -> Option<(String, String)> {
    let p = prefs();
    let spec = p.default.clone().or_else(|| p.providers.first().map(|e| e.name.clone()))?;
    let (prov, m) = split(&spec);
    let pr = find(prov)?;
    let model = m.map(String::from).or_else(|| pr.default_model().map(String::from))?;
    Some((prov.to_string(), model))
}

/// Build the provider behind `name` (resolving its key).
pub fn open(name: &str) -> Result<Box<dyn ModelProvider>> {
    let p = find(name).with_context(|| format!("no model provider {name} (see `mouser model list`)"))?;
    match p.origin {
        Origin::Local => return Ok(Box::new(local::Local { models: p.entry.models.clone() })),
        Origin::Preset => bail!("{name} needs an API key first: mouser model add {name} --key-stdin"),
        _ => {}
    }
    let key = p.entry.key.as_deref().map(crate::mcp_client::resolve_token).transpose()?;
    let e = p.entry;
    crate::mcp::Target::parse(&e.url).with_context(|| format!("{name}: url"))?;
    Ok(match e.api {
        Api::OpenAi => Box::new(openai::OpenAi { id: e.name, url: e.url, key, models: e.models, ca: e.ca }),
        Api::Anthropic => Box::new(anthropic::Anthropic { id: e.name, url: e.url, key, models: e.models, ca: e.ca }),
        Api::Google => Box::new(google::Google { id: e.name, url: e.url, key, models: e.models, ca: e.ca }),
        Api::LlamaCpp => bail!("{name}: llama.cpp providers are P4b"),
    })
}

pub(crate) fn infos(models: &[String]) -> Vec<ModelInfo> {
    models.iter().map(|m| ModelInfo { name: m.clone(), tools: true }).collect()
}

/// Run one stream of SSE events through `f` until it returns false or the sink cancels.
pub(crate) fn stream(
    url: &str,
    headers: &[(&str, String)],
    body: &Value,
    ca: Option<&str>,
    sink: &mut dyn ChatSink,
    f: &mut dyn FnMut(&str, &str, &mut dyn ChatSink) -> Result<bool>,
) -> Result<()> {
    let mut sse = http::Sse::new();
    let mut err: Option<anyhow::Error> = None;
    let mut go = true;
    let mut feed = |line: &str, sink: &mut dyn ChatSink| -> bool {
        if let Some((ev, data)) = sse.line(line) {
            match f(&ev, &data, sink) {
                Ok(true) => {}
                Ok(false) => go = false,
                Err(e) => {
                    err = Some(e);
                    go = false;
                }
            }
        }
        go && !sink.cancelled()
    };
    http::post_lines(url, headers, &body.to_string(), ca.map(std::path::Path::new), &mut |l| feed(l, sink))?;
    feed("", sink);
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
