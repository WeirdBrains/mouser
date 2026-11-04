//! The Grok Bot desktop login (opt-in, like Grokbot-cli's `/login desktop`):
//! reuse the session the signed-in Grok Bot desktop app saved on this machine
//! (`gateway-descriptor.json`, or the Cursor token in `sand-secrets.json`),
//! through the `grokbot_api` crate. Nothing is read until the user runs
//! `mouser model add grok --desktop`, and again on each chat. Credentials only
//! go to Cursor hosts (the crate's policy); a 401/417 re-reads the desktop
//! files once. A "model" here is one of your Grok Bot agents: Mouser sends the
//! turn with `sendPrompt` and polls the agent's transcript for the reply.

use super::*;
use grokbot_api::desktop::{DesktopConfig, DesktopCredential, Platform};
use grokbot_api::{Client, ClientOptions, Entry, Policy, SendPrompt};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// The URL recorded for desktop providers (the real gateway comes from the desktop files).
pub const URL: &str = "grokbot-desktop:";

pub struct GrokDesktop {
    pub id: String,
    /// Agent ids (first = default).
    pub models: Vec<String>,
}

fn config() -> Result<DesktopConfig> {
    let platform = Platform::current().context("the Grok Bot desktop login is not supported on this OS")?;
    let mut cfg = DesktopConfig::new(platform);
    // Tests (and unusual setups) point this at another home; never the real one in tests.
    if let Some(h) = std::env::var_os("MOUSER_GROKBOT_HOME") {
        cfg.home = h.into();
        // Only that home: never fall back to the real XDG_CONFIG_HOME / APPDATA.
        cfg.env = Box::new(|k| if matches!(k, "XDG_CONFIG_HOME" | "APPDATA") { None } else { std::env::var(k).ok() });
    }
    Ok(cfg)
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread().enable_all().build()?)
}

fn poll_every() -> Duration {
    Duration::from_millis(std::env::var("MOUSER_GROK_POLL_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(1500))
}

fn err(e: grokbot_api::Error) -> anyhow::Error {
    anyhow::anyhow!("Grok Bot: {}", keys::redact(&e.to_string(), &[]))
}

/// Connect with whatever the desktop app saved (read fresh every call).
async fn connect() -> Result<Client> {
    let cfg = config()?;
    let opts = ClientOptions { policy: Policy::from_env(), ..ClientOptions::default() };
    match cfg.load_credential(None, &opts.policy).map_err(err)? {
        DesktopCredential::Session(s) => Client::new(s, opts).map_err(err),
        DesktopCredential::CursorToken(t) => Client::connect_cursor(&t, opts).await.map_err(err),
    }
}

/// Run `f` with a client; on 401/417 re-read the desktop files and retry once.
async fn with_client<T, F, Fut>(f: F) -> Result<T>
where
    F: Fn(Client) -> Fut,
    Fut: std::future::Future<Output = grokbot_api::Result<T>>,
{
    match f(connect().await?).await {
        Err(e) if e.is_unauthorized() => f(connect().await?).await.map_err(|e| {
            if e.is_unauthorized() {
                anyhow::anyhow!("Grok Bot: the desktop session was rejected; sign in again in the Grok Bot app")
            } else {
                err(e)
            }
        }),
        r => r.map_err(err),
    }
}

/// Whether a desktop session is saved on this machine (reads nothing secret).
pub fn present() -> bool {
    config().is_ok_and(|c| c.present() || c.sand_present())
}

/// Your Grok Bot agents: (id, label). Groups are skipped.
pub fn agents() -> Result<Vec<(String, String)>> {
    let list = runtime()?.block_on(with_client(|c| async move { c.list_agents().await }))?;
    Ok(list
        .into_iter()
        .filter(|a| !a.is_group && a.hidden_from_sidebar != Some(true))
        .map(|a| {
            let label = if a.display_name().is_empty() { a.id.clone() } else { format!("{} ({})", a.display_name(), a.id) };
            (a.id, label)
        })
        .collect())
}

fn reply_text(e: &Entry) -> Option<String> {
    if e.is_from_user() || e.text.trim().is_empty() {
        return None;
    }
    // Bot messages are `send-message` with a text body; tool events and cards are skipped.
    let r = e.role.to_lowercase();
    let ok = if r == "send-message" {
        e.raw["message"]["type"].as_str().is_none_or(|t| t == "text")
    } else {
        r.contains("assistant") || r.contains("bot")
    };
    ok.then(|| grokbot_api::sanitize_text(&e.text))
}

impl ModelProvider for GrokDesktop {
    fn id(&self) -> &str {
        &self.id
    }
    fn models(&self) -> Vec<ModelInfo> {
        self.models.iter().map(|m| ModelInfo { name: m.clone(), tools: false }).collect()
    }
    fn chat(&self, req: &ChatRequest, sink: &mut dyn ChatSink) -> Result<()> {
        let agent = if req.model.is_empty() { self.models.first().cloned().unwrap_or_default() } else { req.model.clone() };
        if agent.is_empty() {
            bail!("pick one of your Grok Bot agents: mouser model add {} --desktop", self.id);
        }
        let prompt = req.messages.iter().rev().find(|m| m.role == Role::User).map(|m| m.text()).unwrap_or_default();
        if prompt.trim().is_empty() {
            bail!("nothing to send");
        }
        let rt = runtime()?;
        let a = agent.clone();
        let seen: HashSet<String> = rt
            .block_on(with_client(|c| {
                let a = a.clone();
                async move { c.transcript_tail(&a, 50, None).await }
            }))?
            .entries
            .into_iter()
            .filter_map(|e| e.id)
            .collect();
        let (a, p) = (agent.clone(), prompt.clone());
        rt.block_on(with_client(|c| {
            let (a, p) = (a.clone(), p.clone());
            async move { c.send_prompt(&SendPrompt::new(a, p)).await }
        }))?;
        let mut seen = seen;
        let mut got = false;
        let mut quiet = 0;
        let start = Instant::now();
        let every = poll_every();
        loop {
            if sink.cancelled() {
                return Ok(());
            }
            std::thread::sleep(every);
            let a = agent.clone();
            let page = rt.block_on(with_client(|c| {
                let a = a.clone();
                async move { c.transcript_tail(&a, 50, None).await }
            }))?;
            let mut fresh = false;
            for e in page.entries {
                let Some(id) = e.id.clone() else { continue };
                if !seen.insert(id) {
                    continue;
                }
                fresh = true;
                if let Some(t) = reply_text(&e) {
                    if got {
                        sink.event(Event::Text("\n\n".into()));
                    }
                    got = true;
                    sink.event(Event::Text(t));
                }
            }
            quiet = if fresh { 0 } else { quiet + 1 };
            // A reply, then a few quiet polls: the turn is over.
            if got && quiet >= 2 {
                break;
            }
            if start.elapsed() > Duration::from_secs(900) {
                bail!("Grok Bot did not answer within 15 minutes");
            }
        }
        sink.event(Event::Done { stop: "end".into() });
        Ok(())
    }
}
