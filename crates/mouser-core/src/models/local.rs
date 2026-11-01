//! The local model (P4b): Gemma 4 on llama.cpp built into Mouser behind the
//! `local-llm` cargo feature (crate mouser-llm registers a [`LocalBackend`]).
//! This module is always compiled: the weights catalog, the consent-gated
//! download, the compatible-URL alternative, prompt rendering and tool-call
//! parsing (pure, tested without a model).

use super::*;
use sha2::Digest;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// A downloadable GGUF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Weights {
    /// The model id (`gemma-4-e2b`).
    pub id: &'static str,
    pub label: &'static str,
    pub repo: &'static str,
    pub file: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// Pinned files (ggml-org, Q4_0; sizes and SHA256 from the Hugging Face API, 2026-11-01).
pub const CATALOG: [Weights; 2] = [
    Weights {
        id: "gemma-4-e2b",
        label: "Gemma 4 E2B instruct (Q4_0)",
        repo: "ggml-org/gemma-4-E2B-it-GGUF",
        file: "gemma-4-E2B-it-Q4_0.gguf",
        size: 2_841_481_184,
        sha256: "8e30dff3ac4c8434c49a7036fa15564bdbb6044e42bf04550bf1a096ad7e6a52",
    },
    Weights {
        id: "gemma-4-e4b",
        label: "Gemma 4 E4B instruct (Q4_0)",
        repo: "ggml-org/gemma-4-E4B-it-GGUF",
        file: "gemma-4-E4B-it-Q4_0.gguf",
        size: 4_590_807_392,
        sha256: "a555b900214b477d8880e7832e0b8925e139b0159640036b09fe472b6f2097f2",
    },
];

pub fn weights(model: &str) -> Option<&'static Weights> {
    CATALOG.iter().find(|w| w.id == model)
}

impl Weights {
    pub fn url(&self) -> String {
        format!("https://huggingface.co/{}/resolve/main/{}", self.repo, self.file)
    }
    pub fn path(&self) -> PathBuf {
        dir().join(self.file)
    }
    /// Downloaded and complete (size matches; the hash was checked when it finished).
    pub fn installed(&self) -> bool {
        std::fs::metadata(self.path()).is_ok_and(|m| m.len() == self.size)
    }
    /// What the consent prompt shows.
    pub fn consent_text(&self) -> Vec<String> {
        vec![
            format!("Download {} — {} ({} bytes)", self.label, human(self.size), self.size),
            format!("from https://huggingface.co/{}", self.repo),
            format!("to {}", self.path().display()),
            "Nothing is downloaded without your OK; the file is checked (SHA256) when done.".into(),
        ]
    }
}

/// Where model files live: `$MOUSER_MODELS_DIR`, else `[models.local] dir`,
/// else `$MOUSER_HOME/models` (big files can go on another drive).
pub fn dir() -> PathBuf {
    if let Some(d) = std::env::var_os("MOUSER_MODELS_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    if let Some(d) = prefs().dir.filter(|d| !d.is_empty()) {
        return expand(&d);
    }
    crate::storage::resolve_home().join("models")
}

fn expand(d: &str) -> PathBuf {
    match d.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(rest),
        None => PathBuf::from(d),
    }
}

/// `2.84 GB` (decimal, like the download pages).
pub fn human(b: u64) -> String {
    if b >= 1_000_000_000 {
        format!("{:.2} GB", b as f64 / 1e9)
    } else if b >= 1_000_000 {
        format!("{:.1} MB", b as f64 / 1e6)
    } else {
        format!("{b} bytes")
    }
}

/// The user said yes to downloading these weights (from a consent prompt).
/// [`download`] needs one, so nothing downloads without an explicit OK.
pub struct Consent<'a>(&'a Weights);

impl<'a> Consent<'a> {
    /// Call only after showing [`Weights::consent_text`] and getting a yes.
    pub fn given(w: &'a Weights) -> Self {
        Consent(w)
    }
}

/// Download `c`'s weights from `url` (normally [`Weights::url`]) into
/// [`Weights::path`]: resumes a `.part`, verifies size and SHA256, then renames.
/// `progress(done, total)` returns false to cancel (the `.part` is kept).
pub fn download(c: Consent<'_>, url: &str, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<PathBuf> {
    let w = c.0;
    let dest = w.path();
    std::fs::create_dir_all(dest.parent().context("dir")?)?;
    let part = dest.with_extension("gguf.part");
    let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let headers: Vec<(&str, String)> = if have > 0 && have < w.size { vec![("Range", format!("bytes={have}-"))] } else { vec![] };
    let mut r = http::get(url, &headers, None)?;
    let mut done = match r.status {
        206 => have,
        200 => 0,
        s => {
            let mut b = String::new();
            let _ = (&mut r.body).take(4096).read_to_string(&mut b);
            return Err(http::HttpError { status: s, body: b }.into());
        }
    };
    let mut f = std::fs::OpenOptions::new().create(true).write(true).append(done > 0).truncate(done == 0).open(&part)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if !progress(done, w.size) {
            bail!("download cancelled at {} of {} (run it again to resume)", human(done), human(w.size));
        }
        let n = r.body.read(&mut buf)?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n])?;
        done += n as u64;
        if done > w.size {
            break;
        }
    }
    f.flush()?;
    drop(f);
    if done != w.size {
        if done > w.size {
            let _ = std::fs::remove_file(&part);
        }
        bail!("download stopped at {} of {} (run it again to resume)", human(done), human(w.size));
    }
    let mut h = sha2::Sha256::new();
    std::io::copy(&mut std::fs::File::open(&part)?, &mut h)?;
    let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if got != w.sha256 {
        let _ = std::fs::remove_file(&part);
        bail!("{}: checksum mismatch (got {got}); the file was deleted", w.file);
    }
    std::fs::rename(&part, &dest)?;
    progress(done, w.size);
    Ok(dest)
}

/// Remove downloaded weights (and any partial file). True if something was removed.
pub fn remove(w: &Weights) -> bool {
    let a = std::fs::remove_file(w.path()).is_ok();
    let b = std::fs::remove_file(w.path().with_extension("gguf.part")).is_ok();
    a || b
}

// ---------- the backend (crate mouser-llm, feature `local-llm`) ----------

pub trait LocalBackend: Send + Sync {
    /// e.g. `llama.cpp (Vulkan: NVIDIA GeForce RTX 3060)`.
    fn describe(&self) -> String;
    /// Run `req` on the GGUF at `model`.
    fn chat(&self, model: &Path, req: &ChatRequest, sink: &mut dyn ChatSink) -> Result<()>;
}

static BACKEND: OnceLock<Box<dyn LocalBackend>> = OnceLock::new();

/// Called once at startup by builds with `local-llm`.
pub fn register(b: Box<dyn LocalBackend>) {
    let _ = BACKEND.set(b);
}

pub fn backend() -> Option<&'static dyn LocalBackend> {
    BACKEND.get().map(|b| b.as_ref())
}

// ---------- `[models.local]`: use a compatible server instead ----------

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LocalPrefs {
    /// An OpenAI- or Anthropic-compatible URL to use instead of the built-in llama.cpp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<Api>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Model name to send to that server (default: the selected gemma id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Directory for downloaded model files (default `$MOUSER_HOME/models`;
    /// `MOUSER_MODELS_DIR` overrides).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// llama.cpp: layers on the GPU (default: all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_layers: Option<u32>,
    /// llama.cpp: context size in tokens (default 8192).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u32>,
}

pub fn prefs() -> LocalPrefs {
    super::prefs().local.unwrap_or_default()
}

/// The `gemma-local` provider.
pub struct Local {
    pub models: Vec<String>,
}

impl ModelProvider for Local {
    fn id(&self) -> &str {
        "gemma-local"
    }
    fn models(&self) -> Vec<ModelInfo> {
        infos(&self.models)
    }
    fn chat(&self, req: &ChatRequest, sink: &mut dyn ChatSink) -> Result<()> {
        let p = prefs();
        if let Some(url) = &p.url {
            let key = p.key.as_deref().map(crate::mcp_client::resolve_token).transpose()?;
            let mut r = req.clone();
            if let Some(m) = &p.model {
                r.model = m.clone();
            }
            return match p.api.unwrap_or(Api::OpenAi) {
                Api::Anthropic => anthropic::Anthropic { id: "gemma-local".into(), url: url.clone(), key, models: vec![], ca: None }.chat(&r, sink),
                _ => openai::OpenAi { id: "gemma-local".into(), url: url.clone(), key, models: vec![], ca: None }.chat(&r, sink),
            };
        }
        let w = weights(&req.model).with_context(|| format!("gemma-local has no model {} (use {})", req.model, self.models.join(" or ")))?;
        if !w.installed() {
            bail!("{} is not downloaded yet ({}): mouser model local download {} (asks first)", w.label, human(w.size), w.id);
        }
        let b = backend().context(
            "this Mouser was built without the local model (cargo feature local-llm); or use a server: mouser model local url http://127.0.0.1:8080/v1",
        )?;
        b.chat(&w.path(), req, sink)
    }
}

// ---------- prompts and tool calls (shared with the backend) ----------

pub const TOOL_OPEN: &str = "<tool_call>";
pub const TOOL_CLOSE: &str = "</tool_call>";

/// System text describing the tools and the call format.
pub fn tool_prompt(tools: &[ToolSpec], choice: &ToolChoice) -> String {
    if tools.is_empty() || *choice == ToolChoice::None {
        return String::new();
    }
    let mut s = String::from(
        "You can call tools. To call one, reply with only\n<tool_call>{\"name\": \"TOOL\", \"arguments\": {...}}</tool_call>\nand wait for the result. Tools (JSON Schema arguments):\n",
    );
    for t in tools {
        s.push_str(&format!("- {}: {} {}\n", t.name, t.description, t.schema));
    }
    match choice {
        ToolChoice::Required => s.push_str("You must call a tool now.\n"),
        ToolChoice::Tool(n) => s.push_str(&format!("You must call the tool {n} now.\n")),
        _ => {}
    }
    s
}

/// The request as (role, content) turns: system (with tools), user/assistant;
/// tool calls and results are written in the tag format.
pub fn turns(req: &ChatRequest) -> Vec<(String, String)> {
    let mut out = vec![];
    let sys = [req.system.clone().unwrap_or_default(), tool_prompt(&req.tools, &req.tool_choice)]
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");
    if !sys.is_empty() {
        out.push(("system".to_string(), sys));
    }
    for m in &req.messages {
        let mut text = String::new();
        for p in &m.parts {
            match p {
                Part::Text(t) => text.push_str(t),
                Part::ToolCall { name, args, .. } => {
                    text.push_str(&format!("{TOOL_OPEN}{}{TOOL_CLOSE}", serde_json::json!({"name": name, "arguments": args})))
                }
                Part::ToolResult { name, content, is_error, .. } => {
                    text.push_str(&format!("<tool_result name=\"{name}\"{}>{content}</tool_result>", if *is_error { " error=\"true\"" } else { "" }))
                }
            }
        }
        out.push((
            match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            }
            .to_string(),
            text,
        ));
    }
    out
}

/// Render turns with the model's own Jinja chat template (GGUF
/// `tokenizer.chat_template`), ending with the generation prompt.
pub fn render(template: &str, turns: &[(String, String)], bos: &str) -> Result<String> {
    let mut env = minijinja::Environment::new();
    minijinja_contrib::add_to_environment(&mut env);
    env.set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);
    env.add_function("raise_exception", |m: String| -> std::result::Result<String, minijinja::Error> {
        Err(minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, m))
    });
    env.add_template("chat", template)?;
    let msgs: Vec<minijinja::Value> = turns.iter().map(|(r, c)| minijinja::context! { role => r, content => c }).collect();
    Ok(env
        .get_template("chat")?
        .render(minijinja::context! { messages => minijinja::Value::from(msgs), add_generation_prompt => true, bos_token => bos, eos_token => "" })?)
}

/// A plain Gemma-style prompt when the model has no usable template.
pub fn render_fallback(turns: &[(String, String)]) -> String {
    let mut s = String::new();
    let mut pending_sys = String::new();
    for (r, c) in turns {
        match r.as_str() {
            "system" => pending_sys = format!("{c}\n\n"),
            "user" => s.push_str(&format!("<start_of_turn>user\n{}{c}<end_of_turn>\n", std::mem::take(&mut pending_sys))),
            _ => s.push_str(&format!("<start_of_turn>model\n{c}<end_of_turn>\n")),
        }
    }
    s.push_str("<start_of_turn>model\n");
    s
}

/// Turns streamed text into [`Event`]s, lifting `<tool_call>…</tool_call>` into tool calls.
#[derive(Debug, Default)]
pub struct ToolParser {
    buf: String,
    in_call: bool,
    pub calls: usize,
}

impl ToolParser {
    pub fn feed(&mut self, piece: &str, sink: &mut dyn ChatSink) {
        self.buf.push_str(piece);
        loop {
            if self.in_call {
                let Some(end) = self.buf.find(TOOL_CLOSE) else { return };
                let body: String = self.buf[..end].to_string();
                self.buf.drain(..end + TOOL_CLOSE.len());
                self.in_call = false;
                self.emit_call(&body, sink);
            } else if let Some(i) = self.buf.find(TOOL_OPEN) {
                if i > 0 {
                    sink.event(Event::Text(self.buf[..i].to_string()));
                }
                self.buf.drain(..i + TOOL_OPEN.len());
                self.in_call = true;
            } else {
                // Keep a possible partial "<tool_call" at the end.
                let keep = (1..TOOL_OPEN.len())
                    .rev()
                    .find(|&k| {
                        self.buf.len() >= k && self.buf.is_char_boundary(self.buf.len() - k) && TOOL_OPEN.starts_with(&self.buf[self.buf.len() - k..])
                    })
                    .unwrap_or(0);
                let cut = self.buf.len() - keep;
                if cut > 0 {
                    sink.event(Event::Text(self.buf[..cut].to_string()));
                    self.buf.drain(..cut);
                }
                return;
            }
        }
    }

    fn emit_call(&mut self, body: &str, sink: &mut dyn ChatSink) {
        let v: Value = serde_json::from_str(body.trim()).unwrap_or(Value::Null);
        let index = self.calls;
        self.calls += 1;
        sink.event(Event::ToolStart { index, id: format!("call_{index}"), name: v["name"].as_str().unwrap_or("").to_string() });
        let args = if v["arguments"].is_null() { Value::Object(Default::default()) } else { v["arguments"].clone() };
        sink.event(Event::ToolArgs { index, delta: args.to_string() });
    }

    /// End of stream: flush text (an unterminated call is parsed as is).
    pub fn finish(&mut self, sink: &mut dyn ChatSink) {
        let rest = std::mem::take(&mut self.buf);
        if self.in_call {
            self.in_call = false;
            self.emit_call(&rest, sink);
        } else if !rest.is_empty() {
            sink.event(Event::Text(rest));
        }
    }
}
