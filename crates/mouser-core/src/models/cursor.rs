//! grok.com API keys (`grok_…`): Cursor's agents API at `https://api.cursor.com`.
//! There is no Chat Completions endpoint; a chat is an agent with runs. The
//! first turn creates an agent (`POST /v1/agents`), later turns of the same
//! conversation add runs (`POST /v1/agents/{id}/runs`, waiting out
//! `409 agent_busy`), and every run's reply is read from its SSE stream
//! (`410` → fetch the run instead). Mouser's own tools are not offered: the
//! agent has its own.

use super::*;
use serde_json::json;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::BufRead;
use std::sync::Mutex;
use std::time::Duration;

pub const URL: &str = "https://api.cursor.com";

pub struct CursorAgents {
    pub id: String,
    pub url: String,
    pub key: Option<String>,
    pub models: Vec<String>,
    pub ca: Option<String>,
}

/// conversation fingerprint -> agent id, so a follow-up turn continues the same agent.
static AGENTS: Lazy<Mutex<HashMap<u64, String>>> = Lazy::new(Default::default);
use once_cell::sync::Lazy;

fn fingerprint(provider: &str, model: &str, system: Option<&str>, msgs: &[(Role, String)]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (provider, model, system).hash(&mut h);
    for (r, t) in msgs {
        (*r == Role::User, t).hash(&mut h);
    }
    h.finish()
}

fn wait() -> Duration {
    Duration::from_millis(std::env::var("MOUSER_GROK_POLL_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(1000))
}

impl CursorAgents {
    fn headers(&self) -> Result<Vec<(&'static str, String)>> {
        let k = self.key.as_deref().context("the grok provider needs a grok.com key: mouser model add grok --key-stdin")?;
        Ok(vec![("Authorization", format!("Bearer {}", k.trim()))])
    }
    fn ca(&self) -> Option<&std::path::Path> {
        self.ca.as_deref().map(std::path::Path::new)
    }
    fn err(&self, e: anyhow::Error) -> anyhow::Error {
        let secrets: Vec<&str> = self.key.as_deref().into_iter().collect();
        let mut msg = keys::redact(&format!("{e:#}"), &secrets);
        if let Some(h) = e.downcast_ref::<http::HttpError>()
            && (h.status == 401 || h.status == 403)
        {
            if let Some(k) = &self.key
                && let Some(hint) = keys::wrong_provider_hint(&self.id, &self.url, k)
            {
                msg.push_str(&format!(" — {hint}"));
            } else {
                msg.push_str(" — check the key at grok.com/dashboard/api-keys (grok_…)");
            }
        }
        anyhow::anyhow!(msg)
    }

    /// `GET /v1/me`: the key works (returns the key's name).
    pub fn me(&self) -> Result<String> {
        let v = http::get_json(&http::join(&self.url, "v1/me"), &self.headers()?, self.ca()).map_err(|e| self.err(e))?;
        Ok(v["apiKeyName"].as_str().unwrap_or("").to_string())
    }

    /// `GET /v1/models` → `items[].id`.
    pub fn list(&self) -> Result<Vec<String>> {
        let v = http::get_json(&http::join(&self.url, "v1/models"), &self.headers()?, self.ca()).map_err(|e| self.err(e))?;
        let items = v["items"].as_array().or(v["models"].as_array()).or(v["data"].as_array());
        Ok(items.into_iter().flatten().filter_map(|m| m["id"].as_str().or(m.as_str()).map(String::from)).collect())
    }

    fn start(&self, req: &ChatRequest, prompt: &str) -> Result<(String, String)> {
        let mut b = json!({"prompt": {"text": prompt}});
        if !req.model.is_empty() {
            b["model"] = json!({"id": req.model});
        }
        let v = http::post_json(&http::join(&self.url, "v1/agents"), &self.headers()?, &b, self.ca())?;
        let agent = v["agent"]["id"].as_str().or(v["run"]["agentId"].as_str()).context("no agent id in the reply")?.to_string();
        let run = v["run"]["id"].as_str().or(v["agent"]["latestRunId"].as_str()).context("no run id in the reply")?.to_string();
        Ok((agent, run))
    }

    fn follow_up(&self, agent: &str, prompt: &str, sink: &dyn ChatSink) -> Result<String> {
        let url = http::join(&self.url, &format!("v1/agents/{agent}/runs"));
        let b = json!({"prompt": {"text": prompt}});
        for _ in 0..600 {
            match http::post_json(&url, &self.headers()?, &b, self.ca()) {
                Ok(v) => return v["run"]["id"].as_str().or(v["id"].as_str()).map(String::from).context("no run id in the reply"),
                Err(e) if e.downcast_ref::<http::HttpError>().is_some_and(|h| h.status == 409) => {
                    if sink.cancelled() {
                        bail!("cancelled");
                    }
                    std::thread::sleep(wait());
                }
                Err(e) => return Err(e),
            }
        }
        bail!("the agent stayed busy")
    }

    /// Read a run's SSE stream into `sink`; returns the stop reason.
    fn read(&self, agent: &str, run: &str, sink: &mut dyn ChatSink) -> Result<String> {
        let mut h = self.headers()?;
        h.push(("Accept", "text/event-stream".into()));
        let url = http::join(&self.url, &format!("v1/agents/{agent}/runs/{run}/stream"));
        let r = http::request("GET", &url, &h, None, self.ca())?;
        if r.status == 410 {
            return self.fetch(agent, run, sink);
        }
        if !(200..300).contains(&r.status) {
            return http::json(r).map(|_| String::new());
        }
        let mut body = r.body;
        let mut sse = http::Sse::new();
        let mut got_text = false;
        let mut stop = None;
        let mut line = String::new();
        loop {
            line.clear();
            let n = match body.read_line(&mut line) {
                Ok(n) => n,
                Err(_) if stop.is_some() => 0,
                Err(e) => return Err(e.into()),
            };
            let l = if n == 0 { "" } else { line.trim_end_matches(['\r', '\n']) };
            if let Some((ev, data)) = sse.line(l) {
                let v: Value = serde_json::from_str(&data).unwrap_or(Value::String(data.clone()));
                let ev = if ev.is_empty() { v["type"].as_str().unwrap_or("").to_string() } else { ev };
                match ev.as_str() {
                    "assistant" => {
                        if let Some(t) = v["text"].as_str().or(v.as_str())
                            && !t.is_empty()
                        {
                            got_text = true;
                            sink.event(Event::Text(t.into()));
                        }
                    }
                    "result" => {
                        if !got_text
                            && let Some(t) = v["text"].as_str()
                            && !t.is_empty()
                        {
                            got_text = true;
                            sink.event(Event::Text(t.into()));
                        }
                        let st = v["status"].as_str().unwrap_or("FINISHED");
                        stop = Some(status_stop(st)?);
                    }
                    "error" => bail!("{}: {}", v["code"].as_str().unwrap_or("error"), v["message"].as_str().unwrap_or("the run failed")),
                    "done" => return Ok(stop.unwrap_or_else(|| "end".into())),
                    _ => {} // status, thinking, tool_call, heartbeat
                }
            }
            if sink.cancelled() {
                let _ =
                    http::post_json(&http::join(&self.url, &format!("v1/agents/{agent}/runs/{run}/cancel")), &self.headers()?, &json!({}), self.ca());
                return Ok("cancelled".into());
            }
            if n == 0 {
                break;
            }
        }
        match stop {
            Some(s) => Ok(s),
            // The stream ended early: ask for the run.
            None => self.fetch(agent, run, sink),
        }
    }

    /// `GET /v1/agents/{id}/runs/{runId}` until it is terminal.
    fn fetch(&self, agent: &str, run: &str, sink: &mut dyn ChatSink) -> Result<String> {
        let url = http::join(&self.url, &format!("v1/agents/{agent}/runs/{run}"));
        loop {
            let v = http::get_json(&url, &self.headers()?, self.ca())?;
            let r = if v["run"].is_object() { &v["run"] } else { &v };
            let st = r["status"].as_str().unwrap_or("");
            if matches!(st, "FINISHED" | "ERROR" | "CANCELLED" | "EXPIRED") {
                let text = r["result"]["text"].as_str().or(r["result"].as_str()).or(r["text"].as_str()).unwrap_or("");
                if !text.is_empty() {
                    sink.event(Event::Text(text.into()));
                }
                return status_stop(st);
            }
            if sink.cancelled() {
                return Ok("cancelled".into());
            }
            std::thread::sleep(wait());
        }
    }
}

fn status_stop(st: &str) -> Result<String> {
    match st {
        "ERROR" => bail!("the agent run failed"),
        "EXPIRED" => bail!("the agent run expired"),
        "CANCELLED" => Ok("cancelled".into()),
        _ => Ok("end".into()),
    }
}

/// The conversation as one prompt (for a new agent).
fn transcript(system: Option<&str>, msgs: &[(Role, String)]) -> String {
    if msgs.len() == 1 && system.is_none() {
        return msgs[0].1.clone();
    }
    let mut s = String::new();
    if let Some(sys) = system {
        s.push_str(&format!("{sys}\n\n"));
    }
    if msgs.len() > 1 {
        s.push_str("Conversation so far:\n");
        for (r, t) in &msgs[..msgs.len() - 1] {
            s.push_str(&format!("{}: {t}\n", if *r == Role::User { "User" } else { "Assistant" }));
        }
        s.push_str("\nReply to the last message:\n");
    }
    s.push_str(&msgs.last().map(|m| m.1.clone()).unwrap_or_default());
    s
}

impl ModelProvider for CursorAgents {
    fn id(&self) -> &str {
        &self.id
    }
    fn models(&self) -> Vec<ModelInfo> {
        self.models.iter().map(|m| ModelInfo { name: m.clone(), tools: false }).collect()
    }
    fn chat(&self, req: &ChatRequest, sink: &mut dyn ChatSink) -> Result<()> {
        let msgs: Vec<(Role, String)> = req
            .messages
            .iter()
            .map(|m| {
                let mut t = m.text();
                for p in &m.parts {
                    if let Part::ToolResult { content, .. } = p {
                        t.push_str(content);
                    }
                }
                (m.role, t)
            })
            .filter(|(_, t)| !t.is_empty())
            .collect();
        let Some((_, last)) = msgs.last().cloned() else { bail!("nothing to send") };
        let prior = fingerprint(&self.id, &req.model, req.system.as_deref(), &msgs[..msgs.len() - 1]);
        let known = AGENTS.lock().unwrap().get(&prior).cloned();
        let go = |sink: &mut dyn ChatSink| -> Result<(String, String)> {
            let (agent, run) = match &known {
                Some(a) => (a.clone(), self.follow_up(a, &last, sink)?),
                None => self.start(req, &transcript(req.system.as_deref(), &msgs))?,
            };
            let mut c = Tee { inner: sink, text: String::new() };
            let stop = self.read(&agent, &run, &mut c)?;
            let mut after = msgs.clone();
            after.push((Role::Assistant, c.text));
            AGENTS.lock().unwrap().insert(fingerprint(&self.id, &req.model, req.system.as_deref(), &after), agent);
            Ok((stop, String::new()))
        };
        let (stop, _) = go(sink).map_err(|e| self.err(e))?;
        if !sink.cancelled() {
            sink.event(Event::Done { stop });
        }
        Ok(())
    }
}

/// Passes events on and keeps the reply text (to fingerprint the conversation).
struct Tee<'a> {
    inner: &'a mut dyn ChatSink,
    text: String,
}
impl ChatSink for Tee<'_> {
    fn event(&mut self, e: Event) {
        if let Event::Text(t) = &e {
            self.text.push_str(t);
        }
        self.inner.event(e)
    }
    fn cancelled(&self) -> bool {
        self.inner.cancelled()
    }
}
