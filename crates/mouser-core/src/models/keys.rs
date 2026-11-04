//! API key shapes: which provider a key belongs to, a hint when it is sent to
//! the wrong one, and redaction of keys/tokens in error text and logs.

use once_cell::sync::Lazy;
use regex::Regex;

/// What a pasted key looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    /// `xai-…` from console.x.ai: OpenAI-compatible inference at `https://api.x.ai/v1`.
    Xai,
    /// `grok_<32hex>_<64hex>` from grok.com/dashboard/api-keys: a Cursor API key
    /// (agents API at `https://api.cursor.com`).
    GrokCom,
    /// `sk-ant-…` (Anthropic).
    Anthropic,
    /// `sk-or-…` (OpenRouter).
    OpenRouter,
    /// `AIza…` (Google).
    Google,
    /// `sk-…` (OpenAI and others).
    OpenAiLike,
    Unknown,
}

static GROK_COM: Lazy<Regex> = Lazy::new(|| Regex::new(r"^grok_[0-9a-fA-F]{32}_[0-9a-fA-F]{64}$").unwrap());

pub fn kind(key: &str) -> KeyKind {
    let k = key.trim();
    if k.starts_with("xai-") {
        KeyKind::Xai
    } else if GROK_COM.is_match(k) || k.starts_with("grok_") {
        KeyKind::GrokCom
    } else if k.starts_with("sk-ant-") {
        KeyKind::Anthropic
    } else if k.starts_with("sk-or-") {
        KeyKind::OpenRouter
    } else if k.starts_with("AIza") {
        KeyKind::Google
    } else if k.starts_with("sk-") {
        KeyKind::OpenAiLike
    } else {
        KeyKind::Unknown
    }
}

/// True for a well-formed grok.com key (`grok_<32hex>_<64hex>`).
pub fn is_grok_com(key: &str) -> bool {
    GROK_COM.is_match(key.trim())
}

fn host(url: &str) -> String {
    crate::mcp::Target::parse(url).map(|t| t.host.to_ascii_lowercase()).unwrap_or_default()
}

/// A hint when `key` clearly belongs to another provider than `provider` (at
/// `url`); None when it fits or we cannot tell. Never includes the key.
pub fn wrong_provider_hint(provider: &str, url: &str, key: &str) -> Option<String> {
    let h = host(url);
    let k = kind(key);
    let xai_host = h == "api.x.ai";
    let cursor_host = h == "api.cursor.com";
    match k {
        KeyKind::GrokCom if !cursor_host && provider != "grok" => Some(format!(
            "that is a grok.com key (grok_…, from grok.com/dashboard/api-keys); it does not work with {provider}{}. Use the grok provider: mouser model add grok --key-stdin",
            if xai_host { " (api.x.ai takes xai-… keys from console.x.ai)" } else { "" }
        )),
        KeyKind::Xai if !xai_host && provider != "grok" => Some(format!(
            "that is an xAI key (xai-…, from console.x.ai); it does not work with {provider}. Use: mouser model add xai --key-stdin (or mouser model add grok --key-stdin)"
        )),
        KeyKind::Xai if cursor_host => Some(
            "that is an xAI key (xai-…, from console.x.ai); api.cursor.com takes grok.com keys (grok_…). Use: mouser model add xai --key-stdin"
                .into(),
        ),
        KeyKind::Anthropic if xai_host || cursor_host => Some(format!(
            "that looks like an Anthropic key (sk-ant-…); it does not work with {provider}. Use: mouser model add anthropic --key-stdin"
        )),
        KeyKind::OpenRouter if xai_host || cursor_host => Some(format!(
            "that looks like an OpenRouter key (sk-or-…); it does not work with {provider}. Use: mouser model add openrouter --key-stdin"
        )),
        KeyKind::Google if xai_host || cursor_host => {
            Some(format!("that looks like a Google key (AIza…); it does not work with {provider}. Use: mouser model add google --key-stdin"))
        }
        _ => None,
    }
}

static SECRETISH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?x)
        (?i:bearer|basic)\s+[A-Za-z0-9._~+/=:-]{6,}
      | xai-[A-Za-z0-9*_-]{4,}
      | grok_[A-Za-z0-9*_]{4,}
      | sk-[A-Za-z0-9*_-]{8,}
      | AIza[A-Za-z0-9_-]{8,}
      | ([?&](?:key|api_key|token)=)[^&\s]+",
    )
    .unwrap()
});

/// Replace anything that looks like a key or bearer token, plus each of
/// `secrets` (exact), with `<redacted>`.
pub fn redact(text: &str, secrets: &[&str]) -> String {
    let mut s = text.to_string();
    for sec in secrets.iter().map(|s| s.trim()).filter(|s| s.len() >= 4) {
        s = s.replace(sec, "<redacted>");
    }
    SECRETISH
        .replace_all(&s, |c: &regex::Captures| {
            let m = &c[0];
            if let Some(q) = c.get(1) {
                format!("{}<redacted>", q.as_str())
            } else if let Some((scheme, _)) = m.split_once(char::is_whitespace) {
                format!("{scheme} <redacted>")
            } else {
                "<redacted>".to_string()
            }
        })
        .into_owned()
}
