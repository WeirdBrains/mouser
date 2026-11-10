//! Models from Hugging Face for the local engine (docs/models.md §Hugging
//! Face). Search the Hub for GGUF repositories (the format llama.cpp runs),
//! read one repository's details and files, and install one quantization:
//! its file(s) download into `<models dir>/hf/<owner>--<repo>/` (resumable,
//! size- and SHA256-checked against the Hub's LFS metadata) and it becomes
//! a `gemma-local/<id>` model and the default. Nothing downloads without a
//! [`Consent`], which only a prompt or Mouser's popup creates.
//!
//! The Hub is `https://huggingface.co`; `MOUSER_HF_URL` points elsewhere
//! (a mirror, or a local mock server in tests).

use super::local::{self, human};
use super::{http, prefs, save};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

pub const HUB: &str = "https://huggingface.co";
/// Search results asked for.
pub const SEARCH_LIMIT: usize = 30;
/// Longest card summary shown in the details.
pub const SUMMARY_MAX: usize = 700;

/// The Hub's base URL (`MOUSER_HF_URL`, else [`HUB`]).
pub fn base() -> String {
    std::env::var("MOUSER_HF_URL").ok().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| HUB.into()).trim_end_matches('/').to_string()
}

/// Percent-encode a query value or path segment.
pub fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A repository id: `owner/name`, Hub characters only (never a path trick).
pub fn check_repo(id: &str) -> Result<()> {
    let ok = |p: &str| !p.is_empty() && p != "." && p != ".." && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    match id.split_once('/') {
        Some((o, n)) if ok(o) && ok(n) => Ok(()),
        _ => bail!("{id:?} is not a Hugging Face model id (owner/name)"),
    }
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hit {
    pub id: String,
    pub downloads: u64,
    pub likes: u64,
    pub pipeline: String,
    pub updated: String,
}

/// GGUF repositories matching `query`, most downloaded first.
pub fn search(query: &str, limit: usize) -> Result<Vec<Hit>> {
    let url = format!("{}/api/models?search={}&filter=gguf&sort=downloads&direction=-1&limit={}", base(), encode(query.trim()), limit.clamp(1, 100));
    let v = http::get_json(&url, &[("Accept", "application/json".into())], None).context("searching Hugging Face")?;
    Ok(parse_search(&v))
}

pub fn parse_search(v: &Value) -> Vec<Hit> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(Hit {
                id: m["id"].as_str().or(m["modelId"].as_str())?.to_string(),
                downloads: m["downloads"].as_u64().unwrap_or(0),
                likes: m["likes"].as_u64().unwrap_or(0),
                pipeline: m["pipeline_tag"].as_str().unwrap_or("").into(),
                updated: m["lastModified"].as_str().or(m["createdAt"].as_str()).unwrap_or("").chars().take(10).collect(),
            })
        })
        .filter(|h| check_repo(&h.id).is_ok())
        .collect()
}

/// One file of a quantization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HfFile {
    pub name: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// A quantization: one GGUF, or the parts of a split one (`-00001-of-00003`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quant {
    /// `Q4_K_M`, `F16` … (the file stem when no tag is recognised).
    pub label: String,
    /// The model id it installs as (the file stem without the split suffix).
    pub id: String,
    pub files: Vec<HfFile>,
}

impl Quant {
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// A repository's details.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Details {
    pub id: String,
    pub author: String,
    pub downloads: u64,
    pub likes: u64,
    pub license: String,
    pub pipeline: String,
    pub architecture: String,
    pub context_length: u64,
    /// Parameters (the GGUF's `total`), 0 if unknown.
    pub params: u64,
    pub gated: bool,
    pub updated: String,
    pub tags: Vec<String>,
    /// The first paragraphs of the model card.
    pub summary: String,
    pub quants: Vec<Quant>,
}

/// The GGUF architectures the llama.cpp bundled with Mouser loads
/// (`LLM_ARCH_NAMES` in llama-cpp-sys-2 0.1.161's `llama-arch.cpp`; update it
/// with that crate). `clip` (vision projectors) is left out.
pub const LLAMA_CPP_ARCHS: &[&str] = &[
    "llama",
    "llama4",
    "deci",
    "falcon",
    "grok",
    "gpt2",
    "gptj",
    "gptneox",
    "mpt",
    "baichuan",
    "starcoder",
    "refact",
    "bert",
    "modern-bert",
    "nomic-bert",
    "nomic-bert-moe",
    "neo-bert",
    "jina-bert-v2",
    "jina-bert-v3",
    "eurobert",
    "bloom",
    "stablelm",
    "qwen",
    "qwen2",
    "qwen2moe",
    "qwen2vl",
    "qwen3",
    "qwen3moe",
    "qwen3next",
    "qwen3vl",
    "qwen3vlmoe",
    "qwen35",
    "qwen35moe",
    "clef",
    "qwen4exp",
    "phi2",
    "phi3",
    "phimoe",
    "plamo",
    "plamo2",
    "plamo3",
    "codeshell",
    "orion",
    "internlm2",
    "minicpm",
    "minicpm3",
    "gemma",
    "gemma2",
    "gemma3",
    "gemma3n",
    "gemma4",
    "gemma4-assistant",
    "gemma-embedding",
    "starcoder2",
    "mamba",
    "mamba2",
    "maple",
    "jamba",
    "falcon-h1",
    "xverse",
    "command-r",
    "cohere2",
    "cohere2moe",
    "dbrx",
    "olmo",
    "olmo2",
    "olmoe",
    "muse-glimmer",
    "openelm",
    "arctic",
    "deepseek",
    "deepseek2",
    "deepseek2-ocr",
    "deepseek32",
    "deepseek4",
    "chatglm",
    "glm4",
    "glm4moe",
    "glm-dsa",
    "bitnet",
    "t5",
    "t5encoder",
    "jais",
    "jais2",
    "nemotron",
    "nemotron_h",
    "nemotron_h_moe",
    "exaone",
    "exaone4",
    "exaone-moe",
    "rwkv6",
    "rwkv6qwen2",
    "rwkv7",
    "arwkv7",
    "granite",
    "granitemoe",
    "granitehybrid",
    "graniteswitch",
    "granite_swa",
    "chameleon",
    "wavtokenizer-dec",
    "plm",
    "bailingmoe",
    "bailingmoe2",
    "bailingmoe3",
    "dots1",
    "dots3note",
    "arcee",
    "afmoe",
    "laguna",
    "ernie4_5",
    "ernie4_5-moe",
    "hunyuan-moe",
    "hunyuan-dense",
    "hunyuan_vl",
    "hy_v3",
    "hy_v4",
    "smollm3",
    "gpt-oss",
    "lfm2",
    "lfm2moe",
    "dream",
    "smallthinker",
    "llada",
    "llada-moe",
    "seed_oss",
    "grovemoe",
    "apertus",
    "minimax-01",
    "hrm_text",
    "minimax-m2",
    "minimax-m3",
    "cogvlm",
    "rnd1",
    "pangu-embedded",
    "mistral3",
    "eagle3",
    "dflash",
    "mistral4",
    "paddleocr",
    "mimo2",
    "step35",
    "spark2_5",
    "llama-embed",
    "maincoder",
    "kimi-linear",
    "kimi-k3",
    "glm5-next",
    "talkie",
    "mellum",
    "nanbeige",
    "qwen3tts",
    "pockettts",
    ".",
    "unknown",
    "tensor",
    "name",
    "for",
    "tensor",
    "id",
    "%d",
    ".",
    "unknown",
];

/// Architectures llama.cpp loads that are not chat models: embedding,
/// encoder-only, speech and codec models. (architecture, what it is).
pub const NOT_CHAT: &[(&str, &str)] = &[
    ("bert", "an embedding model"),
    ("modern-bert", "an embedding model"),
    ("nomic-bert", "an embedding model"),
    ("nomic-bert-moe", "an embedding model"),
    ("neo-bert", "an embedding model"),
    ("jina-bert-v2", "an embedding model"),
    ("jina-bert-v3", "an embedding model"),
    ("eurobert", "an embedding model"),
    ("gemma-embedding", "an embedding model"),
    ("llama-embed", "an embedding model"),
    ("pangu-embedded", "an embedding model"),
    ("t5encoder", "a text encoder"),
    ("wavtokenizer-dec", "an audio codec"),
    ("qwen3tts", "a text-to-speech model"),
    ("pockettts", "a text-to-speech model"),
    ("eagle3", "a speculative-decoding draft head"),
    ("dflash", "a speculative-decoding draft head"),
    ("gemma4-assistant", "a speculative-decoding draft head"),
];

/// Whether the chat can run a model, from its GGUF metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    /// A chat architecture the bundled llama.cpp knows. Not a promise
    /// that this file loads: nothing is load-tested before you install it.
    Supported,
    /// The Hub doesn't say which architecture: it may run; llama.cpp says
    /// so on the first message if it can't load it.
    Unverified,
    /// It won't run in the chat, and why.
    No(String),
}

/// Can the chat run `d`? GGUF files only (the format llama.cpp loads; no
/// Python or `trust_remote_code`, nothing from the repository is executed).
pub fn support(d: &Details) -> Support {
    let a = d.architecture.trim().to_ascii_lowercase();
    if d.quants.is_empty() {
        return Support::No(format!("{} has no GGUF files (Mouser runs GGUF models on llama.cpp; safetensors and other formats don't run)", d.id));
    }
    if a.is_empty() {
        return Support::Unverified;
    }
    if a == "clip" || a == "mmproj" {
        return Support::No(format!("{} is a vision projector (clip), not a model on its own", d.id));
    }
    if let Some((_, what)) = NOT_CHAT.iter().find(|(n, _)| *n == a) {
        return Support::No(format!("{} is {what} ({a}), not a chat model", d.id));
    }
    if !LLAMA_CPP_ARCHS.contains(&a.as_str()) {
        return Support::No(format!(
            "{}'s architecture {a} isn't supported by the llama.cpp bundled with this Mouser; a newer Mouser may run it",
            d.id
        ));
    }
    Support::Supported
}

/// A repository's details and files (`/api/models/ID?blobs=true`) and the
/// start of its card (README; best effort).
pub fn details(id: &str) -> Result<Details> {
    check_repo(id)?;
    let (owner, name) = id.split_once('/').context("id")?;
    let path = format!("{}/{}", encode(owner), encode(name));
    let v = http::get_json(&format!("{}/api/models/{path}?blobs=true", base()), &[("Accept", "application/json".into())], None)
        .with_context(|| format!("reading {id} on Hugging Face"))?;
    let mut d = parse_details(&v);
    if d.id.is_empty() {
        bail!("Hugging Face has no model {id}");
    }
    if let Ok(mut r) = http::get(&format!("{}/{path}/raw/main/README.md", base()), &[], None)
        && r.status == 200
    {
        use std::io::Read;
        let mut s = String::new();
        let _ = (&mut r.body).take(256 * 1024).read_to_string(&mut s);
        d.summary = summary(&s);
    }
    Ok(d)
}

pub fn parse_details(v: &Value) -> Details {
    let card = &v["cardData"];
    let license = card["license"]
        .as_str()
        .map(String::from)
        .or_else(|| v["tags"].as_array()?.iter().find_map(|t| t.as_str()?.strip_prefix("license:").map(String::from)))
        .unwrap_or_default();
    let siblings: Vec<HfFile> = v["siblings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(HfFile {
                name: s["rfilename"].as_str()?.to_string(),
                size: s["lfs"]["size"].as_u64().or(s["size"].as_u64()).unwrap_or(0),
                sha256: s["lfs"]["sha256"].as_str().map(String::from),
            })
        })
        .collect();
    Details {
        id: v["id"].as_str().or(v["modelId"].as_str()).unwrap_or("").into(),
        author: v["author"].as_str().unwrap_or("").into(),
        downloads: v["downloads"].as_u64().unwrap_or(0),
        likes: v["likes"].as_u64().unwrap_or(0),
        license,
        pipeline: v["pipeline_tag"].as_str().unwrap_or("").into(),
        architecture: v["gguf"]["architecture"].as_str().unwrap_or("").into(),
        context_length: v["gguf"]["context_length"].as_u64().unwrap_or(0),
        params: v["gguf"]["total"].as_u64().unwrap_or(0),
        gated: v["gated"].as_bool().unwrap_or(false) || v["gated"].is_string(),
        updated: v["lastModified"].as_str().unwrap_or("").chars().take(10).collect(),
        tags: v["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t.as_str())
            .filter(|t| !t.contains(':') && !["gguf", "region", "endpoints_compatible"].contains(t))
            .take(8)
            .map(String::from)
            .collect(),
        summary: String::new(),
        quants: quants(&siblings),
    }
}

/// `-00001-of-00003` at the end of a stem: (stem without it, part number).
fn split_part(stem: &str) -> Option<(&str, u32)> {
    let (head, of) = stem.rsplit_once("-of-")?;
    let (base, part) = head.rsplit_once('-')?;
    (of.len() == 5 && part.len() == 5 && of.chars().all(|c| c.is_ascii_digit())).then_some((base, part.parse().ok()?))
}

/// The quantization tag in a file stem (`Q4_K_M`, `IQ3_XS`, `BF16`, `Q8_0`).
pub fn quant_label(stem: &str) -> String {
    let up = stem.to_ascii_uppercase();
    for sep in ['-', '.', '_'] {
        let parts: Vec<&str> = up.split(sep).collect();
        for n in (1..=parts.len().min(3)).rev() {
            let tail = parts[parts.len() - n..].join(&sep.to_string());
            let t = tail.trim_start_matches(['-', '.', '_']);
            let known = ["F32", "F16", "BF16"].contains(&t)
                || ((t.starts_with('Q') || t.starts_with("IQ") || t.starts_with("TQ"))
                    && t.chars().nth(if t.starts_with('Q') { 1 } else { 2 }).is_some_and(|c| c.is_ascii_digit())
                    && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
            if known {
                return t.to_string();
            }
        }
    }
    stem.to_string()
}

/// The GGUF quantizations among a repository's files, smallest first.
/// Vision projectors (`mmproj`) and importance matrices (`imatrix`, not a model) are left out; a split file is one entry
/// whose parts are in order.
pub fn quants(files: &[HfFile]) -> Vec<Quant> {
    let mut out: Vec<Quant> = Vec::new();
    for f in files {
        let lower = f.name.to_ascii_lowercase();
        let unsafe_path = f.name.contains("..") || f.name.starts_with('/') || f.name.contains('\\') || f.name.contains(':');
        if !lower.ends_with(".gguf") || lower.contains("mmproj") || lower.contains("imatrix") || unsafe_path {
            continue;
        }
        let file = f.name.rsplit('/').next().unwrap_or(&f.name);
        let stem = &file[..file.len() - 5];
        let (id, _) = split_part(stem).unwrap_or((stem, 1));
        let key = format!("{}{id}", &f.name[..f.name.len() - file.len()]);
        match out.iter_mut().find(|q| q.id == key) {
            Some(q) => q.files.push(f.clone()),
            None => out.push(Quant { label: quant_label(id), id: key, files: vec![f.clone()] }),
        }
    }
    for q in &mut out {
        q.files.sort_by_key(|f| split_part(f.name.trim_end_matches(".gguf")).map_or(0, |p| p.1));
        // The id is the stem only (folders in the repo are not part of it).
        q.id = q.id.rsplit('/').next().unwrap_or(&q.id).to_string();
    }
    out.sort_by_key(|q| q.size());
    out
}

/// The first paragraphs of a model card: no front matter, HTML, images,
/// badges or tables; headings as plain lines.
pub fn summary(readme: &str) -> String {
    let mut text = readme.trim_start_matches('\u{feff}');
    if let Some(rest) = text.strip_prefix("---")
        && let Some(end) = rest.find("\n---")
    {
        text = &rest[end + 4..];
    }
    let mut out = String::new();
    let mut fence = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("```") {
            fence = !fence;
            continue;
        }
        if fence || l.starts_with('<') || l.starts_with("![") || l.starts_with("[![") || l.starts_with('|') || l.starts_with("---") {
            continue;
        }
        let l = l.trim_start_matches('#').trim();
        if l.is_empty() {
            if !out.ends_with("\n\n") && !out.is_empty() {
                out.push_str("\n\n");
            }
            continue;
        }
        let l = plain(l);
        if l.is_empty() {
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push(' ');
        }
        out.push_str(&l);
        if out.chars().count() >= SUMMARY_MAX {
            break;
        }
    }
    let out = out.trim().to_string();
    if out.chars().count() > SUMMARY_MAX {
        let cut: String = out.chars().take(SUMMARY_MAX).collect();
        format!("{}…", cut.trim_end())
    } else {
        out
    }
}

/// A card line without inline HTML tags; `[text](url)` links keep their text.
fn plain(l: &str) -> String {
    let mut out = String::new();
    let mut chars = l.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' => {
                for d in chars.by_ref() {
                    if d == '>' {
                        break;
                    }
                }
            }
            ']' if chars.peek() == Some(&'(') => {
                for d in chars.by_ref() {
                    if d == ')' {
                        break;
                    }
                }
            }
            '[' => {}
            c => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// An installed Hugging Face model (`[[models.local.installed]]`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Installed {
    /// The model id: `gemma-local/<id>`.
    pub id: String,
    pub repo: String,
    pub label: String,
    pub files: Vec<HfFile>,
}

impl Installed {
    pub fn dir(&self) -> PathBuf {
        repo_dir(&self.repo)
    }
    /// What llama.cpp opens (the first part of a split model).
    pub fn path(&self) -> PathBuf {
        self.dir().join(self.files.first().map_or("", |f| file_name(&f.name)))
    }
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
    /// Every file is there with its size.
    pub fn complete(&self) -> bool {
        !self.files.is_empty() && self.files.iter().all(|f| std::fs::metadata(self.dir().join(file_name(&f.name))).is_ok_and(|m| m.len() == f.size))
    }
}

fn file_name(n: &str) -> &str {
    n.rsplit('/').next().unwrap_or(n)
}

/// `<models dir>/hf/<owner>--<name>`.
pub fn repo_dir(repo: &str) -> PathBuf {
    local::dir().join("hf").join(repo.replace('/', "--"))
}

/// The installed Hugging Face models.
pub fn installed() -> Vec<Installed> {
    local::prefs().installed
}

pub fn find(id: &str) -> Option<Installed> {
    installed().into_iter().find(|i| i.id == id)
}

/// What one install would download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub repo: String,
    pub quant: Quant,
    pub license: String,
}

impl Plan {
    /// The plan for `label` (or id) in `d`.
    pub fn new(d: &Details, quant: &str) -> Result<Plan> {
        if d.gated {
            bail!("{} is gated: accept its terms on huggingface.co first (gated downloads are not supported yet)", d.id);
        }
        if let Support::No(why) = support(d) {
            bail!("{why}");
        }
        let q = d.quants.iter().find(|q| q.label == quant || q.id == quant).with_context(|| {
            format!("{} has no GGUF {quant}; one of: {}", d.id, d.quants.iter().map(|q| q.label.as_str()).collect::<Vec<_>>().join(", "))
        })?;
        if let Some(f) = q.files.iter().find(|f| f.sha256.as_deref().is_none_or(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))) {
            bail!("{}: the Hub publishes no SHA256 for {}, so it can't be checked; not downloading it", d.id, f.name);
        }
        Ok(Plan { repo: d.id.clone(), quant: q.clone(), license: d.license.clone() })
    }

    /// The model id it installs as (owner-prefixed when another repo already has it).
    pub fn id(&self) -> String {
        match installed().into_iter().find(|i| i.id == self.quant.id) {
            Some(i) if i.repo != self.repo => format!("{}--{}", self.repo.split('/').next().unwrap_or(""), self.quant.id),
            _ => self.quant.id.clone(),
        }
    }

    /// What the consent prompt shows.
    pub fn consent_text(&self) -> Vec<String> {
        let size = self.quant.size();
        let parts = if self.quant.files.len() > 1 { format!(", {} files", self.quant.files.len()) } else { String::new() };
        let mut v = vec![
            format!("Download {} {} — {} ({size} bytes{parts})", self.repo, self.quant.label, human(size)),
            format!("from {}/{}", base(), self.repo),
            format!("to {}", repo_dir(&self.repo).display()),
        ];
        if !self.license.is_empty() {
            v.push(format!("License: {}", self.license));
        }
        v.push("Nothing is downloaded without your OK; it then becomes the chat's model.".into());
        v
    }
}

/// The user said yes to this plan ([`Plan::consent_text`] was shown).
pub struct Consent<'a>(&'a Plan);

impl<'a> Consent<'a> {
    pub fn given(p: &'a Plan) -> Self {
        Consent(p)
    }
}

/// Download the plan's files (resuming `.part`s, checking sizes and the
/// Hub's SHA256), record the model and make it the default. Returns the
/// spec (`gemma-local/<id>`). `progress(done, total)` returns false to cancel.
pub fn install(c: Consent<'_>, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<String> {
    let p = c.0;
    check_repo(&p.repo)?;
    let dir = repo_dir(&p.repo);
    let total = p.quant.size();
    let mut before = 0u64;
    for f in &p.quant.files {
        let name = file_name(&f.name);
        let url = format!("{}/{}/resolve/main/{}", base(), p.repo, f.name.split('/').map(encode).collect::<Vec<_>>().join("/"));
        local::fetch_file(&url, &dir.join(name), f.size, f.sha256.as_deref(), &mut |d, _| progress(before + d, total))?;
        before += f.size;
    }
    let inst = Installed { id: p.id(), repo: p.repo.clone(), label: p.quant.label.clone(), files: p.quant.files.clone() };
    let spec = format!("gemma-local/{}", inst.id);
    let mut m = prefs();
    let mut lp = m.local.clone().unwrap_or_default();
    lp.installed.retain(|i| i.id != inst.id);
    lp.installed.push(inst);
    lp.disabled = false;
    m.local = Some(lp);
    m.default = Some(spec.clone());
    save(&m)?;
    progress(total, total);
    Ok(spec)
}

/// Delete an installed model's files and forget it. True if it was there;
/// a default pointing at it is cleared.
pub fn remove(id: &str) -> Result<bool> {
    let Some(i) = find(id) else { return Ok(false) };
    for f in &i.files {
        let p = i.dir().join(file_name(&f.name));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("gguf.part"));
    }
    let _ = std::fs::remove_dir(i.dir());
    let mut m = prefs();
    let mut lp = m.local.clone().unwrap_or_default();
    lp.installed.retain(|x| x.id != id);
    m.local = Some(lp);
    if m.default.as_deref() == Some(&format!("gemma-local/{id}")) {
        m.default = None;
    }
    save(&m)?;
    Ok(true)
}

/// The quantization installed when none is named: the smallest Q4 (else the smallest).
pub fn default_quant(d: &Details) -> Option<&Quant> {
    d.quants.iter().find(|q| q.label.starts_with("Q4")).or(d.quants.first())
}

/// `1.2k`, `3.4M`.
pub fn count(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

/// The details as text lines (`mouser model hf info`, the chat's details view).
pub fn details_lines(d: &Details) -> Vec<String> {
    let mut v = vec![format!("{}  ↓{} ♥{}", d.id, count(d.downloads), count(d.likes))];
    let mut facts = vec![];
    for (k, x) in [("license", &d.license), ("task", &d.pipeline), ("architecture", &d.architecture), ("updated", &d.updated)] {
        if !x.is_empty() {
            facts.push(format!("{k}: {x}"));
        }
    }
    if d.params > 0 {
        facts.push(format!("parameters: {:.1}B", d.params as f64 / 1e9));
    }
    if d.context_length > 0 {
        facts.push(format!("context: {}", d.context_length));
    }
    v.push(facts.join(" · "));
    if d.gated {
        v.push("gated: accept its terms on huggingface.co (gated downloads are not supported yet)".into());
    }
    v.push(match support(d) {
        Support::Supported => {
            format!("supported architecture ({}), not load-tested: llama.cpp says so on the first message if this file doesn't load", d.architecture)
        }
        Support::Unverified => "architecture not listed on the Hub: it may not load (llama.cpp says so on the first message)".into(),
        Support::No(why) => format!("won't run: {why}"),
    });
    if !d.summary.is_empty() {
        v.push(String::new());
        v.extend(d.summary.lines().map(String::from));
    }
    v.push(String::new());
    v.push(format!("{} GGUF quantizations:", d.quants.len()));
    for q in &d.quants {
        let parts = if q.files.len() > 1 { format!(" ({} files)", q.files.len()) } else { String::new() };
        v.push(format!("  {:<12} {:>9}{parts}", q.label, human(q.size())));
    }
    v
}
