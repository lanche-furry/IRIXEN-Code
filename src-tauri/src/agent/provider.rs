//! Multi-vendor LLM providers.
//!
//! Three wire protocols cover essentially the whole market:
//! - `Openai`    → OpenAI, DeepSeek, Kimi, Qwen/DashScope, GLM, SiliconFlow, OpenRouter, …
//! - `Anthropic` → Claude (and Anthropic-compatible gateways)
//! - `Gemini`    → Google Gemini
//!
//! Every provider normalizes its stream into [`StreamEvent`] and can list its
//! models, so the UI gets one uniform view over all vendors.

use crate::agent::types::*;
use crate::net;
use crate::config::{Protocol, ProviderConfig};
use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// A model entry as returned by a vendor's `/models` endpoint.
#[derive(Clone, Debug)]
pub struct RawModel {
    pub id: String,
    pub display_name: Option<String>,
    pub owned_by: Option<String>,
    pub context_length: Option<u64>,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn id(&self) -> &str;
    async fn chat(&self, req: &ChatRequest) -> Result<StreamResult, ProviderError>;
    async fn list_models(&self) -> Result<Vec<RawModel>, ProviderError>;
}

pub struct ProviderRegistry {
    providers: BTreeMap<String, std::sync::Arc<dyn LlmProvider>>,
}

impl ProviderRegistry {
    /// Providers the agent may actually use: enabled and configured.
    pub fn from_config(providers: &[ProviderConfig], proxy: &str) -> Self {
        Self::build(providers, proxy, true)
    }

    /// Every configured provider, including the ones switched off.
    ///
    /// Listing models and running a connectivity check happen *while the user is
    /// editing the provider*, long before it is switched on — `enabled` only
    /// decides whether the agent may use it, so filtering here would make a
    /// perfectly healthy endpoint report "provider is disabled".
    pub fn from_config_any_state(providers: &[ProviderConfig], proxy: &str) -> Self {
        Self::build(providers, proxy, false)
    }

    fn build(providers: &[ProviderConfig], proxy: &str, only_enabled: bool) -> Self {
        let client = net::build_client(proxy);
        let label = net::proxy_label(proxy);
        let mut providers_map = BTreeMap::new();
        for cfg in providers {
            if cfg.base_url.trim().is_empty() || (only_enabled && !cfg.enabled) {
                continue;
            }
            // Pasting a key from a web page usually drags a trailing newline or
            // space along. `probe()` trims it, so an untrimmed key here made the
            // diagnostic pass while the real request came back 401 — trim once,
            // at the only place every protocol copy comes from.
            let mut cfg = cfg.clone();
            cfg.api_key = cfg.api_key.trim().to_string();
            cfg.base_url = cfg.base_url.trim().to_string();
            let cfg = &cfg;
            let p: std::sync::Arc<dyn LlmProvider> = match cfg.protocol {
                Protocol::Openai => std::sync::Arc::new(OpenAiProvider {
                    cfg: cfg.clone(),
                    client: client.clone(),
                    proxy: label.clone(),
                }),
                Protocol::Anthropic => std::sync::Arc::new(AnthropicProvider {
                    cfg: cfg.clone(),
                    client: client.clone(),
                    proxy: label.clone(),
                }),
                Protocol::Gemini => std::sync::Arc::new(GeminiProvider {
                    cfg: cfg.clone(),
                    client: client.clone(),
                    proxy: label.clone(),
                }),
            };
            providers_map.insert(cfg.id.clone(), p);
        }
        ProviderRegistry { providers: providers_map }
    }

    pub fn get(&self, id: &str) -> Option<std::sync::Arc<dyn LlmProvider>> {
        self.providers.get(id).cloned()
    }
}

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

fn base_url(cfg: &ProviderConfig) -> String {
    net::normalize_base(&cfg.base_url)
}

fn extra_headers(cfg: &ProviderConfig) -> reqwest::header::HeaderMap {
    let mut map = reqwest::header::HeaderMap::new();
    for (k, v) in &cfg.extra_headers {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(k.as_bytes()),
            reqwest::header::HeaderValue::from_str(v),
        ) {
            map.insert(name, value);
        }
    }
    map
}

/// Merge the user's vendor-specific body tweaks (Qwen `enable_thinking`,
/// GLM `thinking`, …) into a request body.
fn apply_extra_body(cfg: &ProviderConfig, body: &mut Value) {
    if let Some(obj) = body.as_object_mut() {
        for (k, v) in &cfg.extra_body {
            obj.insert(k.clone(), v.clone());
        }
    }
}

/// Parse an SSE response body into a stream of JSON payloads.
pub(crate) fn sse_json_stream(
    resp: reqwest::Response,
) -> impl Stream<Item = Result<Value, ProviderError>> {
    let mut buf = String::new();
    resp.bytes_stream().flat_map(move |chunk| {
        let mut events: Vec<Result<Value, ProviderError>> = Vec::new();
        match chunk {
            Ok(bytes) => {
                buf.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(nl) = buf.find('\n') {
                    let line = buf[..nl].trim_end_matches('\r').to_string();
                    buf.drain(..=nl);
                    if let Some(rest) = line.strip_prefix("data:") {
                        let data = rest.trim();
                        if data.is_empty() || data == "[DONE]" {
                            continue;
                        }
                        match serde_json::from_str::<Value>(data) {
                            Ok(v) => events.push(Ok(v)),
                            Err(e) => events.push(Err(ProviderError::Parse(e.to_string()))),
                        }
                    }
                }
            }
            Err(e) => events.push(Err(ProviderError::Http(e.to_string()))),
        }
        futures_util::stream::iter(events)
    })
}

/// Convert our neutral message list into OpenAI chat/completions format.
fn to_openai_messages(messages: &[Message]) -> Value {
    let mut out: Vec<Value> = Vec::new();
    for m in messages {
        match m.role {
            Role::System => {
                let text = m.text_of();
                if !text.is_empty() {
                    out.push(json!({ "role": "system", "content": text }));
                }
            }
            Role::User => out.push(json!({ "role": "user", "content": m.text_of() })),
            Role::Assistant => {
                let mut msg = serde_json::Map::new();
                msg.insert("role".into(), json!("assistant"));
                let text = m.text_of();
                msg.insert("content".into(), json!(if text.is_empty() { Value::Null } else { Value::String(text) }));
                let tool_calls: Vec<Value> = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::ToolCall { id, name, arguments } => Some(json!({
                            "id": id,
                            "type": "function",
                            "function": { "name": name, "arguments": arguments }
                        })),
                        _ => None,
                    })
                    .collect();
                if !tool_calls.is_empty() {
                    msg.insert("tool_calls".into(), json!(tool_calls));
                }
                out.push(Value::Object(msg));
            }
            Role::Tool => {
                // Each tool result becomes its own `tool` message.
                for p in &m.parts {
                    if let ContentPart::ToolResult { call_id, output, .. } = p {
                        out.push(json!({ "role": "tool", "tool_call_id": call_id, "content": output }));
                    }
                }
            }
        }
    }
    json!(out)
}

/// Convert our neutral message list into Anthropic messages format.
/// Returns (system_prompt, messages).
fn to_anthropic_messages(messages: &[Message]) -> (String, Value) {
    let mut system = String::new();
    let mut out: Vec<Value> = Vec::new();
    for m in messages {
        match m.role {
            Role::System => {
                let text = m.text_of();
                if !text.is_empty() {
                    if !system.is_empty() {
                        system.push_str("\n\n");
                    }
                    system.push_str(&text);
                }
            }
            Role::User => out.push(json!({
                "role": "user",
                "content": [{ "type": "text", "text": m.text_of() }]
            })),
            Role::Assistant => {
                let mut blocks: Vec<Value> = Vec::new();
                let text = m.text_of();
                if !text.is_empty() {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
                for p in &m.parts {
                    if let ContentPart::ToolCall { id, name, arguments } = p {
                        let input: Value =
                            serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
                        blocks.push(json!({
                            "type": "tool_use", "id": id, "name": name, "input": input
                        }));
                    }
                }
                if !blocks.is_empty() {
                    out.push(json!({ "role": "assistant", "content": blocks }));
                }
            }
            Role::Tool => {
                // Anthropic expects tool results as a user message with
                // tool_result blocks, grouped together.
                let mut blocks: Vec<Value> = Vec::new();
                for p in &m.parts {
                    if let ContentPart::ToolResult { call_id, output, is_error, .. } = p {
                        blocks.push(json!({
                            "type": "tool_result",
                            "tool_use_id": call_id,
                            "content": output,
                            "is_error": is_error
                        }));
                    }
                }
                if !blocks.is_empty() {
                    out.push(json!({ "role": "user", "content": blocks }));
                }
            }
        }
    }
    (system, json!(out))
}

/// Convert our neutral message list into Gemini contents format.
fn to_gemini_messages(messages: &[Message]) -> (Option<String>, Value) {
    let mut system: Option<String> = None;
    let mut out: Vec<Value> = Vec::new();
    for m in messages {
        match m.role {
            Role::System => {
                let text = m.text_of();
                if !text.is_empty() {
                    system = Some(match system {
                        Some(s) => format!("{s}\n\n{text}"),
                        None => text,
                    });
                }
            }
            Role::User => out.push(json!({
                "role": "user",
                "parts": [{ "text": m.text_of() }]
            })),
            Role::Assistant => {
                let mut parts: Vec<Value> = Vec::new();
                let text = m.text_of();
                if !text.is_empty() {
                    parts.push(json!({ "text": text }));
                }
                for p in &m.parts {
                    if let ContentPart::ToolCall { name, arguments, .. } = p {
                        let args: Value =
                            serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
                        parts.push(json!({ "functionCall": { "name": name, "args": args } }));
                    }
                }
                if !parts.is_empty() {
                    out.push(json!({ "role": "model", "parts": parts }));
                }
            }
            Role::Tool => {
                let mut parts: Vec<Value> = Vec::new();
                for p in &m.parts {
                    if let ContentPart::ToolResult { name, output, .. } = p {
                        parts.push(json!({
                            "functionResponse": { "name": name, "response": { "content": output } }
                        }));
                    }
                }
                if !parts.is_empty() {
                    out.push(json!({ "role": "user", "parts": parts }));
                }
            }
        }
    }
    (system, json!(out))
}

fn openai_tools(tools: &[Value]) -> Value {
    json!(tools)
}

fn anthropic_tools(tools: &[Value]) -> Value {
    // tools arrive in OpenAI function format; reshape to Anthropic's schema.
    let decls: Vec<Value> = tools
        .iter()
        .filter_map(|t| {
            let f = t.get("function")?;
            Some(json!({
                "name": f.get("name")?,
                "description": f.get("description").cloned().unwrap_or(json!("")),
                "input_schema": f.get("parameters").cloned().unwrap_or(json!({
                    "type": "object", "properties": {}
                }))
            }))
        })
        .collect();
    if decls.is_empty() {
        Value::Null
    } else {
        json!(decls)
    }
}

fn gemini_tools(tools: &[Value]) -> Value {
    let decls: Vec<Value> = tools
        .iter()
        .filter_map(|t| {
            let f = t.get("function")?;
            Some(json!({
                "name": f.get("name")?,
                "description": f.get("description").cloned().unwrap_or(json!("")),
                "parameters": f.get("parameters").cloned().unwrap_or(json!({
                    "type": "object", "properties": {}
                }))
            }))
        })
        .collect();
    if decls.is_empty() {
        Value::Null
    } else {
        json!([{ "functionDeclarations": decls }])
    }
}

// ---------------------------------------------------------------------------
// OpenAI-compatible
// ---------------------------------------------------------------------------

pub struct OpenAiProvider {
    cfg: ProviderConfig,
    client: reqwest::Client,
    /// Resolved proxy, only used to word error messages.
    proxy: String,
}

/// Chat Completions expresses thinking effort as one string. `Off` maps to
/// `"none"`, which DeepSeek (and any vendor forwarding to it) honours as a real
/// switch, not a hint. `Auto` sends nothing and lets the server decide.
fn reasoning_effort(effort: Effort) -> Option<&'static str> {
    match effort {
        Effort::Auto => None,
        Effort::Off => Some("none"),
        Effort::Minimal => Some("minimal"),
        Effort::Low => Some("low"),
        Effort::Medium => Some("medium"),
        Effort::High => Some("high"),
    }
}

/// Does this rejection look like "I don't know that reasoning field"?
/// Deliberately narrow: a 401/429/500 must never be retried, or a real error
/// would be reported twice and the retry would burn a second request.
fn rejects_thinking(status: u16, body: &str) -> bool {
    if status != 400 && status != 422 {
        return false;
    }
    let b = body.to_ascii_lowercase();
    ["reasoning", "reasoning_effort", "thinking", "unsupported parameter", "unknown field", "unrecognized"]
        .iter()
        .any(|k| b.contains(k))
}

impl OpenAiProvider {
    async fn post(&self, url: &str, body: &Value) -> Result<reqwest::Response, ProviderError> {
        self.client
            .post(url)
            .bearer_auth(&self.cfg.api_key)
            .headers(extra_headers(&self.cfg))
            .json(body)
            .send()
            .await
            .map_err(|e| ProviderError::Message(net::explain_transport(&e, url, &self.proxy)))
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn id(&self) -> &str {
        &self.cfg.id
    }

    async fn chat(&self, req: &ChatRequest) -> Result<StreamResult, ProviderError> {
        let mut body = json!({
            "model": req.model,
            "messages": to_openai_messages(&req.messages),
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        if let Some(t) = req.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(mt) = req.max_tokens {
            body["max_tokens"] = json!(mt);
        } else {
            body["max_tokens"] = json!(8192);
        }
        if !req.tools.is_empty() {
            body["tools"] = openai_tools(&req.tools);
        }
        // Thinking effort. Chat Completions spells this as a top-level
        // `reasoning_effort` string; `off` is `"none"` (verified against
        // DeepSeek: it drops reasoning_tokens to zero). OpenAI-compatible
        // vendors that don't know the field are handled by the retry below,
        // so a knob the vendor never implements costs one request, not a turn.
        if let Some(level) = reasoning_effort(req.effort) {
            body["reasoning_effort"] = json!(level);
        }
        apply_extra_body(&self.cfg, &mut body);

        let url = net::join(&base_url(&self.cfg), "/chat/completions");
        let mut resp = self.post(&url, &body).await?;

        // A model that doesn't implement reasoning parameters usually answers
        // 400/422 naming the offending key. Drop the knob and try once more so
        // the user still gets an answer instead of a dead end.
        if !resp.status().is_success() && body.get("reasoning_effort").is_some() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            if rejects_thinking(status, &text) {
                eprintln!("[irixen] {url} -> {status} rejected reasoning_effort; retrying without it");
                let mut retry = body.clone();
                retry.as_object_mut().map(|o| o.remove("reasoning_effort"));
                resp = self.post(&url, &retry).await?;
            } else {
                return Err(ProviderError::Message(net::explain_status(status, &text, &url)));
            }
        }

        check_status(resp, &url)
            .await
            .map(|resp| Box::pin(openai_event_stream(sse_json_stream(resp))) as StreamResult)
    }

    async fn list_models(&self) -> Result<Vec<RawModel>, ProviderError> {
        let url = net::join(&base_url(&self.cfg), &self.cfg.models_path);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.cfg.api_key)
            .headers(extra_headers(&self.cfg))
            .send()
            .await
            .map_err(|e| ProviderError::Message(net::explain_transport(&e, &url, &self.proxy)))?;
        let resp = check_status(resp, &url).await?;
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        let mut out = Vec::new();
        if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
            for m in arr {
                if let Some(id) = m.get("id").and_then(|i| i.as_str()) {
                    out.push(RawModel {
                        id: id.to_string(),
                        display_name: None,
                        owned_by: m
                            .get("owned_by")
                            .and_then(|o| o.as_str())
                            .map(str::to_string),
                        context_length: m
                            .get("context_length")
                            .or_else(|| m.get("context_window"))
                            .and_then(|c| c.as_u64()),
                    });
                }
            }
        }
        Ok(out)
    }
}

fn openai_event_stream<S>(s: S) -> impl Stream<Item = Result<StreamEvent, ProviderError>>
where
    S: Stream<Item = Result<Value, ProviderError>>,
{
    s.flat_map(|item| {
        let mut out: Vec<Result<StreamEvent, ProviderError>> = Vec::new();
        match item {
            Err(e) => out.push(Err(e)),
            Ok(v) => {
                if let Some(err) = v.get("error") {
                    out.push(Err(ProviderError::Api {
                        status: 200,
                        body: err.to_string(),
                    }));
                    return futures_util::stream::iter(out);
                }
                if let Some(usage) = v.get("usage").filter(|u| !u.is_null()) {
                    out.push(Ok(StreamEvent::Usage(Usage {
                        input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
                        output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
                        reasoning_tokens: usage["completion_tokens_details"]["reasoning_tokens"]
                            .as_u64()
                            .unwrap_or(0),
                        cache_read_tokens: usage["prompt_tokens_details"]["cached_tokens"]
                            .as_u64()
                            .unwrap_or(0),
                        ..Default::default()
                    })));
                }
                if let Some(delta) = v
                    .get("choices")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("delta"))
                {
                    if let Some(text) = delta.get("content").and_then(|t| t.as_str()) {
                        if !text.is_empty() {
                            out.push(Ok(StreamEvent::Text(text.to_string())));
                        }
                    }
                    for key in ["reasoning", "reasoning_content"] {
                        if let Some(text) = delta.get(key).and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                out.push(Ok(StreamEvent::Reasoning(text.to_string())));
                            }
                        }
                    }
                    if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc in tcs {
                            let index =
                                tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                            let id = tc.get("id").and_then(|i| i.as_str()).unwrap_or("");
                            let name = tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|n| n.as_str())
                                .unwrap_or("");
                            if !id.is_empty() || !name.is_empty() {
                                out.push(Ok(StreamEvent::ToolCallStart {
                                    index,
                                    id: id.to_string(),
                                    name: name.to_string(),
                                }));
                            }
                            if let Some(args) = tc
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .and_then(|a| a.as_str())
                            {
                                if !args.is_empty() {
                                    out.push(Ok(StreamEvent::ToolCallArgs {
                                        index,
                                        delta: args.to_string(),
                                    }));
                                }
                            }
                        }
                    }
                }
            }
        }
        futures_util::stream::iter(out)
    })
}

// ---------------------------------------------------------------------------
// Anthropic
// ---------------------------------------------------------------------------

pub struct AnthropicProvider {
    cfg: ProviderConfig,
    client: reqwest::Client,
    /// Resolved proxy, only used to word error messages.
    proxy: String,
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn id(&self) -> &str {
        &self.cfg.id
    }

    async fn chat(&self, req: &ChatRequest) -> Result<StreamResult, ProviderError> {
        let (system, messages) = to_anthropic_messages(&req.messages);
        let mut body = json!({
            "model": req.model,
            "max_tokens": req.max_tokens.unwrap_or(8192),
            "messages": messages,
            "stream": true,
        });
        if !system.is_empty() {
            body["system"] = json!(system);
        }
        if let Some(t) = req.temperature {
            body["temperature"] = json!(t);
        }
        let tools = anthropic_tools(&req.tools);
        if !tools.is_null() {
            body["tools"] = tools;
        }
        // Thinking: budget tokens map from effort. Note Anthropic forbids
        // setting temperature while thinking is on.
        match req.effort {
            Effort::Off | Effort::Auto => {}
            other => {
                let budget = match other {
                    Effort::Minimal | Effort::Low => 4_096,
                    Effort::Medium => 16_384,
                    Effort::High => 32_768,
                    _ => 16_384,
                };
                body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
                body.as_object_mut().unwrap().remove("temperature");
            }
        }
        apply_extra_body(&self.cfg, &mut body);

        let url = net::join(&base_url(&self.cfg), "/messages");
        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.cfg.api_key)
            .header("anthropic-version", "2023-06-01")
            .headers(extra_headers(&self.cfg))
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Message(net::explain_transport(&e, &url, &self.proxy)))?;
        check_status(resp, &url)
            .await
            .map(|resp| Box::pin(anthropic_event_stream(sse_json_stream(resp))) as StreamResult)
    }

    async fn list_models(&self) -> Result<Vec<RawModel>, ProviderError> {
        let url = net::join(&base_url(&self.cfg), "/models");
        let resp = self
            .client
            .get(&url)
            .header("x-api-key", &self.cfg.api_key)
            .header("anthropic-version", "2023-06-01")
            .headers(extra_headers(&self.cfg))
            .send()
            .await
            .map_err(|e| ProviderError::Message(net::explain_transport(&e, &url, &self.proxy)))?;
        let resp = check_status(resp, &url).await?;
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        let mut out = Vec::new();
        if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
            for m in arr {
                if let Some(id) = m.get("id").and_then(|i| i.as_str()) {
                    out.push(RawModel {
                        id: id.to_string(),
                        display_name: m
                            .get("display_name")
                            .and_then(|d| d.as_str())
                            .map(str::to_string),
                        owned_by: Some("anthropic".into()),
                        context_length: m.get("context_length").and_then(|c| c.as_u64()),
                    });
                }
            }
        }
        Ok(out)
    }
}

fn anthropic_event_stream<S>(s: S) -> impl Stream<Item = Result<StreamEvent, ProviderError>>
where
    S: Stream<Item = Result<Value, ProviderError>>,
{
    s.flat_map(|item| {
        let mut out: Vec<Result<StreamEvent, ProviderError>> = Vec::new();
        match item {
            Err(e) => out.push(Err(e)),
            Ok(v) => {
                let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match kind {
                    "error" => out.push(Err(ProviderError::Api {
                        status: 200,
                        body: v.to_string(),
                    })),
                    "message_start" => {
                        if let Some(u) = v.get("message").and_then(|m| m.get("usage")) {
                            out.push(Ok(StreamEvent::Usage(Usage {
                                input_tokens: u["input_tokens"].as_u64().unwrap_or(0),
                                cache_read_tokens: u["cache_read_input_tokens"]
                                    .as_u64()
                                    .unwrap_or(0),
                                cache_write_tokens: u["cache_creation_input_tokens"]
                                    .as_u64()
                                    .unwrap_or(0),
                                ..Default::default()
                            })));
                        }
                    }
                    "content_block_start" => {
                        let index = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        if let Some(block) = v.get("content_block") {
                            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                                out.push(Ok(StreamEvent::ToolCallStart {
                                    index,
                                    id: block
                                        .get("id")
                                        .and_then(|i| i.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    name: block
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                }));
                            }
                        }
                    }
                    "content_block_delta" => {
                        let index = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        if let Some(delta) = v.get("delta") {
                            match delta.get("type").and_then(|t| t.as_str()) {
                                Some("text_delta") => {
                                    if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                                        if !text.is_empty() {
                                            out.push(Ok(StreamEvent::Text(text.to_string())));
                                        }
                                    }
                                }
                                Some("thinking_delta") => {
                                    if let Some(text) = delta.get("thinking").and_then(|t| t.as_str())
                                    {
                                        if !text.is_empty() {
                                            out.push(Ok(StreamEvent::Reasoning(text.to_string())));
                                        }
                                    }
                                }
                                Some("input_json_delta") => {
                                    if let Some(text) =
                                        delta.get("partial_json").and_then(|t| t.as_str())
                                    {
                                        if !text.is_empty() {
                                            out.push(Ok(StreamEvent::ToolCallArgs {
                                                index,
                                                delta: text.to_string(),
                                            }));
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    "message_delta" => {
                        if let Some(u) = v.get("usage") {
                            out.push(Ok(StreamEvent::Usage(Usage {
                                output_tokens: u["output_tokens"].as_u64().unwrap_or(0),
                                ..Default::default()
                            })));
                        }
                    }
                    _ => {}
                }
            }
        }
        futures_util::stream::iter(out)
    })
}

// ---------------------------------------------------------------------------
// Google Gemini
// ---------------------------------------------------------------------------

pub struct GeminiProvider {
    cfg: ProviderConfig,
    client: reqwest::Client,
    /// Resolved proxy, only used to word error messages.
    proxy: String,
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    fn id(&self) -> &str {
        &self.cfg.id
    }

    async fn chat(&self, req: &ChatRequest) -> Result<StreamResult, ProviderError> {
        let (system, contents) = to_gemini_messages(&req.messages);
        let mut generation = serde_json::Map::new();
        if let Some(t) = req.temperature {
            generation.insert("temperature".into(), json!(t));
        }
        generation.insert("maxOutputTokens".into(), json!(req.max_tokens.unwrap_or(8192)));
        match req.effort {
            Effort::Off | Effort::Auto => {}
            other => {
                let budget = match other {
                    Effort::Minimal | Effort::Low => 4_096,
                    Effort::Medium => 12_288,
                    Effort::High => 24_576,
                    _ => 12_288,
                };
                generation.insert(
                    "thinkingConfig".into(),
                    json!({ "thinkingBudget": budget, "includeThoughts": true }),
                );
            }
        }
        let mut body = json!({
            "contents": contents,
            "generationConfig": Value::Object(generation),
        });
        if let Some(system) = system {
            body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
        }
        let tools = gemini_tools(&req.tools);
        if !tools.is_null() {
            body["tools"] = tools;
        }
        apply_extra_body(&self.cfg, &mut body);

        let url = net::join(
            &base_url(&self.cfg),
            &format!("/models/{}:streamGenerateContent?alt=sse", req.model),
        );
        let resp = self
            .client
            .post(&url)
            .header("x-goog-api-key", &self.cfg.api_key)
            .headers(extra_headers(&self.cfg))
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Message(net::explain_transport(&e, &url, &self.proxy)))?;
        check_status(resp, &url)
            .await
            .map(|resp| Box::pin(gemini_event_stream(sse_json_stream(resp))) as StreamResult)
    }

    async fn list_models(&self) -> Result<Vec<RawModel>, ProviderError> {
        let url = net::join(&base_url(&self.cfg), &self.cfg.models_path);
        let resp = self
            .client
            .get(&url)
            .header("x-goog-api-key", &self.cfg.api_key)
            .headers(extra_headers(&self.cfg))
            .send()
            .await
            .map_err(|e| ProviderError::Message(net::explain_transport(&e, &url, &self.proxy)))?;
        let resp = check_status(resp, &url).await?;
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        let mut out = Vec::new();
        if let Some(arr) = v.get("models").and_then(|d| d.as_array()) {
            for m in arr {
                if let Some(name) = m.get("name").and_then(|i| i.as_str()) {
                    // Only list models that can actually generate content.
                    let methods = m
                        .get("supportedGenerationMethods")
                        .and_then(|s| s.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str())
                                .any(|x| x == "generateContent")
                        })
                        .unwrap_or(false);
                    if !methods {
                        continue;
                    }
                    out.push(RawModel {
                        id: name.trim_start_matches("models/").to_string(),
                        display_name: m
                            .get("displayName")
                            .and_then(|d| d.as_str())
                            .map(str::to_string),
                        owned_by: Some("google".into()),
                        context_length: m
                            .get("inputTokenLimit")
                            .and_then(|c| c.as_u64()),
                    });
                }
            }
        }
        Ok(out)
    }
}

fn gemini_event_stream<S>(s: S) -> impl Stream<Item = Result<StreamEvent, ProviderError>>
where
    S: Stream<Item = Result<Value, ProviderError>>,
{
    s.flat_map(|item| {
        let mut out: Vec<Result<StreamEvent, ProviderError>> = Vec::new();
        match item {
            Err(e) => out.push(Err(e)),
            Ok(v) => {
                if let Some(feedback) = v.get("promptFeedback") {
                    if let Some(reason) = feedback.get("blockReason") {
                        out.push(Err(ProviderError::Api {
                            status: 200,
                            body: format!("blocked: {reason}"),
                        }));
                    }
                }
                if let Some(usage) = v.get("usageMetadata") {
                    out.push(Ok(StreamEvent::Usage(Usage {
                        input_tokens: usage["promptTokenCount"].as_u64().unwrap_or(0),
                        output_tokens: usage["candidatesTokenCount"].as_u64().unwrap_or(0),
                        reasoning_tokens: usage["thoughtsTokenCount"].as_u64().unwrap_or(0),
                        cache_read_tokens: usage["cachedContentTokenCount"].as_u64().unwrap_or(0),
                        ..Default::default()
                    })));
                }
                if let Some(parts) = v
                    .get("candidates")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("content"))
                    .and_then(|c| c.get("parts"))
                    .and_then(|p| p.as_array())
                {
                    let mut tool_index = 0usize;
                    for part in parts {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                if part.get("thought").and_then(|t| t.as_bool()).unwrap_or(false) {
                                    out.push(Ok(StreamEvent::Reasoning(text.to_string())));
                                } else {
                                    out.push(Ok(StreamEvent::Text(text.to_string())));
                                }
                            }
                        }
                        if let Some(fc) = part.get("functionCall") {
                            let name = fc
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .to_string();
                            let args = fc
                                .get("args")
                                .cloned()
                                .unwrap_or_else(|| json!({}))
                                .to_string();
                            out.push(Ok(StreamEvent::ToolCallStart {
                                index: tool_index,
                                id: format!("call_{tool_index}"),
                                name,
                            }));
                            out.push(Ok(StreamEvent::ToolCallArgs {
                                index: tool_index,
                                delta: args,
                            }));
                            tool_index += 1;
                        }
                    }
                }
            }
        }
        futures_util::stream::iter(out)
    })
}

// ---------------------------------------------------------------------------

async fn check_status(
    resp: reqwest::Response,
    url: &str,
) -> Result<reqwest::Response, ProviderError> {
    let status = resp.status();
    if status.is_success() {
        Ok(resp)
    } else {
        let body = resp.text().await.unwrap_or_default();
        // The UI shows the mapped sentence; the raw status/URL is what actually
        // identifies the problem, so keep it in the log.
        let peek: String = body.chars().take(300).collect();
        eprintln!("[irixen] {url} -> {} {peek}", status.as_u16());
        Err(ProviderError::Message(net::explain_status(
            status.as_u16(),
            &body,
            url,
        )))
    }
}
