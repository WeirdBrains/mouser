//! OpenAI-compatible Chat Completions (OpenAI, xAI, OpenRouter, Moonshot/Kimi,
//! llama-server, Ollama, vLLM, Gemini's compatible endpoint, …).

use super::*;
use serde_json::json;

pub struct OpenAi {
    pub id: String,
    /// Base URL ending before `/chat/completions` (e.g. `https://api.openai.com/v1`).
    pub url: String,
    pub key: Option<String>,
    pub models: Vec<String>,
    pub ca: Option<String>,
}

/// Kimi K2.x fixes sampling and rejects `tool_choice = "required"`.
pub fn kimi(model: &str) -> bool {
    model.to_ascii_lowercase().starts_with("kimi-k2")
}

pub fn body(req: &ChatRequest) -> Value {
    let mut msgs = vec![];
    if let Some(s) = &req.system {
        msgs.push(json!({"role": "system", "content": s}));
    }
    for m in &req.messages {
        match m.role {
            Role::User => {
                let text = m.text();
                if !text.is_empty() {
                    msgs.push(json!({"role": "user", "content": text}));
                }
                for p in &m.parts {
                    if let Part::ToolResult { id, content, .. } = p {
                        msgs.push(json!({"role": "tool", "tool_call_id": id, "content": content}));
                    }
                }
            }
            Role::Assistant => {
                let calls: Vec<Value> = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::ToolCall { id, name, args } => {
                            Some(json!({"id": id, "type": "function", "function": {"name": name, "arguments": args.to_string()}}))
                        }
                        _ => None,
                    })
                    .collect();
                let mut a = json!({"role": "assistant", "content": m.text()});
                if !calls.is_empty() {
                    a["tool_calls"] = Value::Array(calls);
                }
                msgs.push(a);
            }
        }
    }
    let mut b = json!({"model": req.model, "messages": msgs, "stream": true, "stream_options": {"include_usage": true}});
    if !req.tools.is_empty() {
        b["tools"] = req
            .tools
            .iter()
            .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.schema}}))
            .collect();
        let quirk = kimi(&req.model);
        b["tool_choice"] = match &req.tool_choice {
            ToolChoice::Auto => json!("auto"),
            ToolChoice::None => json!("none"),
            ToolChoice::Required if quirk => json!("auto"),
            ToolChoice::Required => json!("required"),
            ToolChoice::Tool(n) => json!({"type": "function", "function": {"name": n}}),
        };
    }
    if let Some(n) = req.max_tokens {
        b["max_tokens"] = json!(n);
    }
    if let Some(t) = req.temperature
        && !kimi(&req.model)
    {
        b["temperature"] = json!(t);
    }
    b
}

impl ModelProvider for OpenAi {
    fn id(&self) -> &str {
        &self.id
    }
    fn models(&self) -> Vec<ModelInfo> {
        infos(&self.models)
    }
    fn chat(&self, req: &ChatRequest, sink: &mut dyn ChatSink) -> Result<()> {
        let mut h = vec![];
        if let Some(k) = &self.key {
            h.push(("Authorization", format!("Bearer {k}")));
        }
        let mut stop = None;
        // Calls by the wire's index -> our index.
        let mut seen: Vec<u64> = vec![];
        stream(&http::join(&self.url, "chat/completions"), &h, &body(req), self.ca.as_deref(), sink, &mut |_, data, sink| {
            if data.trim() == "[DONE]" {
                return Ok(false);
            }
            let v: Value = serde_json::from_str(data).with_context(|| format!("bad stream data: {}", data.chars().take(120).collect::<String>()))?;
            if let Some(e) = v.get("error") {
                bail!("{}", e["message"].as_str().unwrap_or("error"));
            }
            if let Some(c) = v["choices"].get(0) {
                let d = &c["delta"];
                if let Some(t) = d["content"].as_str()
                    && !t.is_empty()
                {
                    sink.event(Event::Text(t.into()));
                }
                for tc in d["tool_calls"].as_array().into_iter().flatten() {
                    let wire = tc["index"].as_u64().unwrap_or(seen.len() as u64);
                    let index = match seen.iter().position(|w| *w == wire) {
                        Some(i) => i,
                        None => {
                            seen.push(wire);
                            let i = seen.len() - 1;
                            sink.event(Event::ToolStart {
                                index: i,
                                id: tc["id"].as_str().unwrap_or("").into(),
                                name: tc["function"]["name"].as_str().unwrap_or("").into(),
                            });
                            i
                        }
                    };
                    if let Some(a) = tc["function"]["arguments"].as_str()
                        && !a.is_empty()
                    {
                        sink.event(Event::ToolArgs { index, delta: a.into() });
                    }
                }
                if let Some(f) = c["finish_reason"].as_str() {
                    stop = Some(match f {
                        "stop" => "end".to_string(),
                        "tool_calls" => "tool_use".to_string(),
                        "length" => "max_tokens".to_string(),
                        o => o.to_string(),
                    });
                }
            }
            if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                sink.event(Event::Usage { input: u["prompt_tokens"].as_u64().unwrap_or(0), output: u["completion_tokens"].as_u64().unwrap_or(0) });
            }
            Ok(true)
        })?;
        if !sink.cancelled() {
            sink.event(Event::Done { stop: stop.unwrap_or_else(|| "end".into()) });
        }
        Ok(())
    }
}
