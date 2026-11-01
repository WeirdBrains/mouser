//! Google Gemini, native `streamGenerateContent` (SSE). For Gemini's
//! OpenAI-compatible endpoint use the openai api with
//! `https://generativelanguage.googleapis.com/v1beta/openai`.

use super::*;
use serde_json::json;

pub struct Google {
    pub id: String,
    /// Base URL (`https://generativelanguage.googleapis.com`).
    pub url: String,
    pub key: Option<String>,
    pub models: Vec<String>,
    pub ca: Option<String>,
}

pub fn endpoint(base: &str, model: &str) -> String {
    let b = base.trim_end_matches('/');
    let b = if b.ends_with("/v1beta") || b.ends_with("/v1") { b.to_string() } else { format!("{b}/v1beta") };
    format!("{b}/models/{model}:streamGenerateContent?alt=sse")
}

pub fn body(req: &ChatRequest) -> Value {
    let contents: Vec<Value> = req
        .messages
        .iter()
        .map(|m| {
            let parts: Vec<Value> = m
                .parts
                .iter()
                .map(|p| match p {
                    Part::Text(t) => json!({"text": t}),
                    Part::ToolCall { name, args, .. } => json!({"functionCall": {"name": name, "args": args}}),
                    Part::ToolResult { name, content, is_error, .. } => {
                        json!({"functionResponse": {"name": name, "response": if *is_error { json!({"error": content}) } else { json!({"content": content}) }}})
                    }
                })
                .collect();
            json!({"role": match m.role { Role::User => "user", Role::Assistant => "model" }, "parts": parts})
        })
        .collect();
    let mut b = json!({"contents": contents});
    if let Some(s) = &req.system {
        b["systemInstruction"] = json!({"parts": [{"text": s}]});
    }
    if !req.tools.is_empty() {
        b["tools"] = json!([{"functionDeclarations": req.tools.iter().map(|t| json!({"name": t.name, "description": t.description, "parameters": t.schema})).collect::<Vec<_>>()}]);
        b["toolConfig"] = json!({"functionCallingConfig": match &req.tool_choice {
            ToolChoice::Auto => json!({"mode": "AUTO"}),
            ToolChoice::None => json!({"mode": "NONE"}),
            ToolChoice::Required => json!({"mode": "ANY"}),
            ToolChoice::Tool(n) => json!({"mode": "ANY", "allowedFunctionNames": [n]}),
        }});
    }
    let mut g = json!({});
    if let Some(n) = req.max_tokens {
        g["maxOutputTokens"] = json!(n);
    }
    if let Some(t) = req.temperature {
        g["temperature"] = json!(t);
    }
    if g.as_object().is_some_and(|o| !o.is_empty()) {
        b["generationConfig"] = g;
    }
    b
}

impl ModelProvider for Google {
    fn id(&self) -> &str {
        &self.id
    }
    fn models(&self) -> Vec<ModelInfo> {
        infos(&self.models)
    }
    fn chat(&self, req: &ChatRequest, sink: &mut dyn ChatSink) -> Result<()> {
        let mut h = vec![];
        if let Some(k) = &self.key {
            h.push(("x-goog-api-key", k.clone()));
        }
        let (mut calls, mut stop, mut usage) = (0usize, None::<String>, None);
        stream(&endpoint(&self.url, &req.model), &h, &body(req), self.ca.as_deref(), sink, &mut |_, data, sink| {
            let v: Value = serde_json::from_str(data).with_context(|| "bad stream data")?;
            if let Some(e) = v.get("error") {
                bail!("{}", e["message"].as_str().unwrap_or("error"));
            }
            if let Some(c) = v["candidates"].get(0) {
                for p in c["content"]["parts"].as_array().into_iter().flatten() {
                    if let Some(t) = p["text"].as_str() {
                        sink.event(Event::Text(t.into()));
                    } else if let Some(f) = p.get("functionCall") {
                        let index = calls;
                        calls += 1;
                        sink.event(Event::ToolStart {
                            index,
                            id: f["id"].as_str().map_or_else(|| format!("call_{index}"), String::from),
                            name: f["name"].as_str().unwrap_or("").into(),
                        });
                        sink.event(Event::ToolArgs { index, delta: f["args"].to_string() });
                    }
                }
                if let Some(r) = c["finishReason"].as_str() {
                    stop = Some(match r {
                        "STOP" if calls > 0 => "tool_use".into(),
                        "STOP" => "end".into(),
                        "MAX_TOKENS" => "max_tokens".into(),
                        o => o.to_ascii_lowercase(),
                    });
                }
            }
            if let Some(u) = v.get("usageMetadata") {
                usage = Some((u["promptTokenCount"].as_u64().unwrap_or(0), u["candidatesTokenCount"].as_u64().unwrap_or(0)));
            }
            Ok(true)
        })?;
        if !sink.cancelled() {
            if let Some((input, output)) = usage {
                sink.event(Event::Usage { input, output });
            }
            sink.event(Event::Done { stop: stop.unwrap_or_else(|| "end".into()) });
        }
        Ok(())
    }
}
