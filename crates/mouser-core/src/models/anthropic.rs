//! Anthropic Messages (and any Anthropic-compatible URL).

use super::*;
use serde_json::json;

pub const VERSION: &str = "2023-06-01";
pub const DEFAULT_MAX_TOKENS: u32 = 4096;

pub struct Anthropic {
    /// Base URL (`https://api.anthropic.com`, or one ending in `/v1`).
    pub id: String,
    pub url: String,
    pub key: Option<String>,
    pub models: Vec<String>,
    pub ca: Option<String>,
}

pub fn endpoint(base: &str) -> String {
    let b = base.trim_end_matches('/');
    if b.ends_with("/v1") { format!("{b}/messages") } else { format!("{b}/v1/messages") }
}

pub fn body(req: &ChatRequest) -> Value {
    let msgs: Vec<Value> = req
        .messages
        .iter()
        .map(|m| {
            let content: Vec<Value> = m
                .parts
                .iter()
                .map(|p| match p {
                    Part::Text(t) => json!({"type": "text", "text": t}),
                    Part::ToolCall { id, name, args } => json!({"type": "tool_use", "id": id, "name": name, "input": args}),
                    Part::ToolResult { id, content, is_error, .. } => {
                        json!({"type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error})
                    }
                })
                .collect();
            json!({"role": match m.role { Role::User => "user", Role::Assistant => "assistant" }, "content": content})
        })
        .collect();
    let mut b = json!({"model": req.model, "max_tokens": req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS), "messages": msgs, "stream": true});
    if let Some(s) = &req.system {
        b["system"] = json!(s);
    }
    if !req.tools.is_empty() {
        b["tools"] = req.tools.iter().map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.schema})).collect();
        b["tool_choice"] = match &req.tool_choice {
            ToolChoice::Auto => json!({"type": "auto"}),
            ToolChoice::None => json!({"type": "none"}),
            ToolChoice::Required => json!({"type": "any"}),
            ToolChoice::Tool(n) => json!({"type": "tool", "name": n}),
        };
    }
    if let Some(t) = req.temperature {
        b["temperature"] = json!(t);
    }
    b
}

impl ModelProvider for Anthropic {
    fn id(&self) -> &str {
        &self.id
    }
    fn models(&self) -> Vec<ModelInfo> {
        infos(&self.models)
    }
    fn chat(&self, req: &ChatRequest, sink: &mut dyn ChatSink) -> Result<()> {
        let mut h = vec![("anthropic-version", VERSION.to_string())];
        if let Some(k) = &self.key {
            h.push(("x-api-key", k.clone()));
        }
        // Content block index -> tool call index.
        let mut blocks: Vec<(u64, usize)> = vec![];
        let (mut input, mut output, mut stop) = (0u64, 0u64, None::<String>);
        stream(&endpoint(&self.url), &h, &body(req), self.ca.as_deref(), sink, &mut |ev, data, sink| {
            let v: Value = serde_json::from_str(data).unwrap_or(Value::Null);
            let kind = if ev.is_empty() { v["type"].as_str().unwrap_or("") } else { ev };
            match kind {
                "message_start" => input = v["message"]["usage"]["input_tokens"].as_u64().unwrap_or(0),
                "content_block_start" if v["content_block"]["type"] == "tool_use" => {
                    let i = blocks.len();
                    blocks.push((v["index"].as_u64().unwrap_or(0), i));
                    sink.event(Event::ToolStart {
                        index: i,
                        id: v["content_block"]["id"].as_str().unwrap_or("").into(),
                        name: v["content_block"]["name"].as_str().unwrap_or("").into(),
                    });
                }
                "content_block_delta" => {
                    let d = &v["delta"];
                    match d["type"].as_str() {
                        Some("text_delta") => sink.event(Event::Text(d["text"].as_str().unwrap_or("").into())),
                        Some("input_json_delta") => {
                            let bi = v["index"].as_u64().unwrap_or(0);
                            if let Some((_, i)) = blocks.iter().find(|(b, _)| *b == bi) {
                                sink.event(Event::ToolArgs { index: *i, delta: d["partial_json"].as_str().unwrap_or("").into() });
                            }
                        }
                        _ => {}
                    }
                }
                "message_delta" => {
                    output = v["usage"]["output_tokens"].as_u64().unwrap_or(output);
                    if let Some(s) = v["delta"]["stop_reason"].as_str() {
                        stop = Some(if s == "end_turn" { "end".into() } else { s.into() });
                    }
                }
                "message_stop" => return Ok(false),
                "error" => bail!("{}", v["error"]["message"].as_str().unwrap_or("error")),
                _ => {}
            }
            Ok(true)
        })?;
        if !sink.cancelled() {
            sink.event(Event::Usage { input, output });
            sink.event(Event::Done { stop: stop.unwrap_or_else(|| "end".into()) });
        }
        Ok(())
    }
}
