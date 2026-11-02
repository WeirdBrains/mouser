//! The first-run model picker behind `esc :a` (docs/chat.md §Picking a model):
//! when the chat has no usable default model it offers the built-in local
//! Gemma models and the hosted providers, then installs the chosen one (local:
//! consent, download with progress, SHA256, cancel; hosted: an API key into
//! the keychain, an optional model name and a connection test) and makes it
//! the default. This is the state machine; the chat mode draws it.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The secret name of a provider's API key (the same as `mouser model add`).
pub fn key_secret(name: &str) -> String {
    format!("model-{name}")
}

/// The default `(provider, model)` if it can answer now: a configured provider,
/// or the local model when it is built in and downloaded (or a compatible URL
/// is set). A preset without a key, or weights not downloaded, is not usable.
pub fn usable_default() -> Option<(String, String)> {
    let (p, m) = default_choice()?;
    let pr = find(&p)?;
    let ok = match pr.origin {
        Origin::Preset => false,
        Origin::Local => local::prefs().url.is_some() || (local::backend().is_some() && local::weights(&m).is_some_and(|w| w.installed())),
        Origin::Configured | Origin::Plugin => true,
    };
    ok.then_some((p, m))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Local(local::Weights),
    /// A built-in hosted preset (`PRESETS` name).
    Preset(&'static str),
    /// Any OpenAI- or Anthropic-compatible URL.
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opt {
    pub label: String,
    pub detail: String,
    pub kind: Kind,
    /// False: shown greyed out (the local model in a build without local-llm).
    pub available: bool,
}

fn preset_label(n: &str) -> &'static str {
    match n {
        "openai" => "OpenAI",
        "xai" => "xAI (Grok)",
        "openrouter" => "OpenRouter",
        "moonshot" => "Kimi (Moonshot)",
        "anthropic" => "Anthropic (Claude)",
        "google" => "Google (Gemini)",
        _ => "custom",
    }
}

/// What the picker offers, local first (E2B is the default).
pub fn options(local_built: bool) -> Vec<Opt> {
    let mut v = Vec::new();
    for (i, w) in local::CATALOG.iter().enumerate() {
        v.push(Opt {
            label: format!("Gemma 4 {} local{}", if i == 0 { "E2B" } else { "E4B" }, if i == 0 { " (default)" } else { "" }),
            detail: if local_built {
                format!("runs on this machine, ~{} download{}", local::human(w.size), if w.installed() { ", already downloaded" } else { "" })
            } else {
                "not in this build (no local-llm); Enter shows how to get it".into()
            },
            kind: Kind::Local(w.clone()),
            available: local_built,
        });
    }
    for (n, _, _, m, _) in PRESETS {
        v.push(Opt { label: preset_label(n).into(), detail: format!("hosted, API key; default model {m}"), kind: Kind::Preset(n), available: true });
    }
    v.push(Opt {
        label: "Custom URL".into(),
        detail: "any OpenAI- or Anthropic-compatible server (llama-server, Ollama, vLLM, …)".into(),
        kind: Kind::Custom,
        available: true,
    });
    v
}

/// The form fields, in Tab order, for `kind`.
pub fn fields(kind: &Kind) -> &'static [Field] {
    match kind {
        Kind::Custom => &[Field::Url, Field::Api, Field::Key, Field::Model],
        _ => &[Field::Key, Field::Model],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Url,
    /// openai / anthropic (a choice, not typed).
    Api,
    Key,
    Model,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Form {
    pub url: String,
    pub api_anthropic: bool,
    pub key: String,
    pub model: String,
    /// Index into [`fields`].
    pub field: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Choose,
    /// Local weights: the size and where they go; nothing downloads until OK.
    Consent(local::Weights),
    Downloading(local::Weights),
    Form(Kind, Form),
    /// Testing the connection.
    Testing(Kind, Form),
    /// Ready: the default `provider/model`, and a warning (key in a file).
    Done(String, Option<String>),
    /// An error; Esc/Enter goes back to `back`.
    Failed(String, Box<Step>),
}

#[derive(Debug, Default)]
struct Job {
    done: u64,
    total: u64,
    /// Ok((spec, warning)) / Err(message).
    result: Option<Result<(String, Option<String>), String>>,
}

pub struct Setup {
    pub step: Step,
    pub sel: usize,
    pub opts: Vec<Opt>,
    job: Arc<Mutex<Job>>,
    cancel: Arc<AtomicBool>,
    /// Tests: where the weights come from instead of Hugging Face.
    pub download_url: Option<String>,
    /// Tests: base URL for the hosted presets instead of the real API.
    pub preset_url: Option<String>,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Setup {
    pub fn new(local_built: bool) -> Self {
        Setup {
            step: Step::Choose,
            sel: 0,
            opts: options(local_built),
            job: Arc::default(),
            cancel: Arc::default(),
            download_url: None,
            preset_url: None,
            notify: None,
        }
    }

    pub fn set_notify(&mut self, f: Arc<dyn Fn() + Send + Sync>) {
        self.notify = Some(f);
    }

    /// (done, total) of the running download.
    pub fn progress(&self) -> (u64, u64) {
        let j = self.job.lock().unwrap();
        (j.done, j.total)
    }

    pub fn up(&mut self) {
        if self.step == Step::Choose {
            self.sel = self.sel.saturating_sub(1);
        }
    }
    pub fn down(&mut self) {
        if self.step == Step::Choose {
            self.sel = (self.sel + 1).min(self.opts.len() - 1);
        }
    }

    /// Choose option `i` (click) or the selected one (Enter).
    pub fn pick(&mut self, i: usize) {
        if self.step != Step::Choose || i >= self.opts.len() {
            return;
        }
        self.sel = i;
        let o = self.opts[i].clone();
        if !o.available {
            self.step = Step::Failed(local::NOT_BUILT.into(), Box::new(Step::Choose));
            return;
        }
        self.step = match o.kind {
            Kind::Local(w) if w.installed() => {
                self.finish_local(&w);
                return;
            }
            Kind::Local(w) => Step::Consent(w),
            k => Step::Form(k, Form::default()),
        };
    }

    /// Enter: pick, accept the consent, submit the form, or leave an error.
    pub fn enter(&mut self) {
        match self.step.clone() {
            Step::Choose => self.pick(self.sel),
            Step::Consent(w) => self.start_download(w),
            Step::Form(k, f) => self.submit(k, f),
            Step::Failed(_, back) => self.step = *back,
            _ => {}
        }
    }

    /// Esc: cancel a download or test, step back. False at the list (close).
    pub fn back(&mut self) -> bool {
        match self.step.clone() {
            Step::Choose => return false,
            Step::Downloading(_) | Step::Testing(..) => {
                self.cancel.store(true, Ordering::SeqCst);
            }
            Step::Failed(_, back) => self.step = *back,
            Step::Done(..) => return false,
            Step::Consent(_) | Step::Form(..) => self.step = Step::Choose,
        }
        true
    }

    /// Typing into the form: text, Backspace, Tab / Shift+Tab between fields;
    /// Space toggles the API choice. False: not taken.
    pub fn key(&mut self, bytes: &[u8]) -> bool {
        let Step::Form(kind, f) = &mut self.step else { return false };
        let fs = fields(kind);
        match bytes {
            b"\t" => f.field = (f.field + 1) % fs.len(),
            b"\x1b[Z" => f.field = (f.field + fs.len() - 1) % fs.len(),
            b"\x7f" | b"\x08" => {
                if let Some(s) = text_of(f, fs[f.field]) {
                    s.pop();
                }
            }
            b"\x15" => {
                if let Some(s) = text_of(f, fs[f.field]) {
                    s.clear();
                }
            }
            b" " if fs[f.field] == Field::Api => f.api_anthropic = !f.api_anthropic,
            _ => match std::str::from_utf8(bytes) {
                Ok(t) if !t.chars().any(char::is_control) => {
                    if let Some(s) = text_of(f, fs[f.field]) {
                        s.push_str(t.trim_matches(['\r', '\n']));
                    }
                }
                _ => return false,
            },
        }
        true
    }

    /// Focus a form field (click).
    pub fn focus(&mut self, i: usize) {
        if let Step::Form(k, f) = &mut self.step {
            if fields(k).get(i) == Some(&Field::Api) && f.field == i {
                f.api_anthropic = !f.api_anthropic;
            }
            f.field = i.min(fields(k).len() - 1);
        }
    }

    /// Move a finished background job on; call before drawing.
    pub fn poll(&mut self) {
        let r = self.job.lock().unwrap().result.take();
        let Some(r) = r else { return };
        let back = match &self.step {
            Step::Testing(k, f) => Step::Form(k.clone(), f.clone()),
            Step::Downloading(w) => Step::Consent(w.clone()),
            _ => Step::Choose,
        };
        self.step = match r {
            Ok((spec, warn)) => Step::Done(spec, warn),
            Err(e) => Step::Failed(e, Box::new(back)),
        };
    }

    fn spawn(&mut self, f: impl FnOnce(&Mutex<Job>, &AtomicBool) -> Result<(String, Option<String>), String> + Send + 'static) {
        self.cancel.store(false, Ordering::SeqCst);
        *self.job.lock().unwrap() = Job::default();
        let (job, cancel, notify) = (self.job.clone(), self.cancel.clone(), self.notify.clone());
        std::thread::spawn(move || {
            let r = f(&job, &cancel);
            job.lock().unwrap().result = Some(r);
            if let Some(n) = notify {
                n();
            }
        });
    }

    fn finish_local(&mut self, w: &local::Weights) {
        self.step = match set_default(&format!("gemma-local/{}", w.id)) {
            Ok(s) => Step::Done(s, None),
            Err(e) => Step::Failed(format!("{e:#}"), Box::new(Step::Choose)),
        };
    }

    fn start_download(&mut self, w: local::Weights) {
        self.step = Step::Downloading(w.clone());
        let url = self.download_url.clone().unwrap_or_else(|| w.url());
        let notify = self.notify.clone();
        self.spawn(move |job, cancel| {
            let mut last = std::time::Instant::now();
            local::download(local::Consent::given(&w), &url, &mut |d, n| {
                {
                    let mut j = job.lock().unwrap();
                    (j.done, j.total) = (d, n);
                }
                if last.elapsed().as_millis() > 100 {
                    last = std::time::Instant::now();
                    if let Some(f) = &notify {
                        f();
                    }
                }
                !cancel.load(Ordering::SeqCst)
            })
            .map_err(|e| format!("{e:#}"))?;
            set_default(&format!("gemma-local/{}", w.id)).map(|s| (s, None)).map_err(|e| format!("{e:#}"))
        });
    }

    fn submit(&mut self, kind: Kind, f: Form) {
        let (name, api, url, model) = match &kind {
            Kind::Preset(n) => {
                let p = PRESETS.iter().find(|p| p.0 == *n).expect("preset");
                let url = self.preset_url.clone().unwrap_or_else(|| p.2.to_string());
                let m = if f.model.trim().is_empty() { p.3.to_string() } else { f.model.trim().to_string() };
                (n.to_string(), p.1, url, m)
            }
            Kind::Custom => {
                let url = f.url.trim().to_string();
                if crate::mcp::Target::parse(&url).is_err() {
                    self.step = Step::Failed("enter the server's URL (http:// or https://)".into(), Box::new(Step::Form(kind, f)));
                    return;
                }
                if f.model.trim().is_empty() {
                    self.step = Step::Failed("enter the model name to use on that server".into(), Box::new(Step::Form(kind, f)));
                    return;
                }
                ("custom".into(), if f.api_anthropic { Api::Anthropic } else { Api::OpenAi }, url, f.model.trim().to_string())
            }
            Kind::Local(_) => return,
        };
        let key = f.key.trim().to_string();
        if key.is_empty() && kind != Kind::Custom {
            self.step = Step::Failed("paste your API key (it is stored in the OS keychain)".into(), Box::new(Step::Form(kind, f)));
            return;
        }
        self.step = Step::Testing(kind, f);
        self.spawn(move |_, cancel| {
            let k = (!key.is_empty()).then(|| key.clone());
            let entry = ProviderEntry { name: name.clone(), api, url, key: None, models: vec![model.clone()], ca: None };
            test_connection(&entry, k.clone(), &model, cancel).map_err(|e| format!("connection test failed: {e:#}"))?;
            let warn = save_provider(entry, k).map_err(|e| format!("{e:#}"))?;
            let spec = set_default(&format!("{name}/{model}")).map_err(|e| format!("{e:#}"))?;
            Ok((spec, warn))
        });
    }
}

fn text_of(f: &mut Form, field: Field) -> Option<&mut String> {
    match field {
        Field::Url => Some(&mut f.url),
        Field::Key => Some(&mut f.key),
        Field::Model => Some(&mut f.model),
        Field::Api => None,
    }
}

/// `•` for each character of a secret.
pub fn masked(s: &str) -> String {
    "•".repeat(s.chars().count())
}

struct Probe<'a>(Collect, &'a AtomicBool);
impl ChatSink for Probe<'_> {
    fn event(&mut self, e: Event) {
        self.0.event(e)
    }
    fn cancelled(&self) -> bool {
        self.1.load(Ordering::SeqCst) || self.0.cancelled()
    }
}

/// One short request with the key in memory; nothing is saved yet.
pub fn test_connection(e: &ProviderEntry, key: Option<String>, model: &str, cancel: &AtomicBool) -> Result<()> {
    crate::mcp::Target::parse(&e.url).context("url")?;
    let p: Box<dyn ModelProvider> = match e.api {
        Api::OpenAi => Box::new(openai::OpenAi { id: e.name.clone(), url: e.url.clone(), key, models: e.models.clone(), ca: None }),
        Api::Anthropic => Box::new(anthropic::Anthropic { id: e.name.clone(), url: e.url.clone(), key, models: e.models.clone(), ca: None }),
        Api::Google => Box::new(google::Google { id: e.name.clone(), url: e.url.clone(), key, models: e.models.clone(), ca: None }),
        Api::LlamaCpp => bail!("not a hosted provider"),
    };
    let req = ChatRequest { model: model.into(), messages: vec![Message::user("Reply with OK.")], max_tokens: Some(16), ..Default::default() };
    let mut sink = Probe(Collect::default(), cancel);
    p.chat(&req, &mut sink)?;
    if cancel.load(Ordering::SeqCst) {
        bail!("cancelled");
    }
    Ok(())
}

/// Save the provider; the key goes to the OS keychain (or a 0600 file, with
/// the returned warning).
pub fn save_provider(mut e: ProviderEntry, key: Option<String>) -> Result<Option<String>> {
    let mut warn = None;
    if let Some(k) = key {
        let st = crate::secrets::set(&key_secret(&e.name), &k)?;
        if let crate::secrets::Store::File(p) = &st {
            warn = Some(format!("no OS keychain available: the key is in {} (0600)", p.display()));
        }
        e.key = Some(format!("keychain:{}", key_secret(&e.name)));
    }
    let mut prefs = super::prefs();
    match prefs.providers.iter().position(|p| p.name == e.name) {
        Some(i) => prefs.providers[i] = e,
        None => prefs.providers.push(e),
    }
    save(&prefs)?;
    Ok(warn)
}

/// Make `spec` (`provider/model`) the default; returns it.
pub fn set_default(spec: &str) -> Result<String> {
    let mut prefs = super::prefs();
    prefs.default = Some(spec.to_string());
    save(&prefs)?;
    Ok(spec.to_string())
}
