//! Web2API — a local, OpenAI/Anthropic-compatible HTTP endpoint.
//!
//! The point is to make a *web-only* AI service usable from ordinary clients
//! (OpenAI SDK, Cursor, anything that speaks `/v1/chat/completions`) without
//! touching the client. The bridge accepts both wire formats, normalises them
//! into the app's own `ChatRequest`, and reverse-proxies one of several web
//! apps picked by the model name the client asked for:
//!
//! * `web` — the default. `claude-*` → Claude Web, `gpt-*/o1/o3` → ChatGPT Web,
//!   `gemini-*` → Gemini Web, with a user-editable route table and a default
//!   source. Each source owns its session credential and a set of templates
//!   describing the private browser API.
//! * `provider` — debug only. An API provider configured for the *agent*
//!   (DeepSeek, …) is not what Web2API is for; it stays as a toggle so the
//!   bridge itself can be smoke-tested without a browser session.
//!
//! These are private browser endpoints with no stability guarantees: when one
//! changes, the bridge reports the upstream error verbatim and the source can
//! be repaired from the panel without rebuilding.
//!
//! Deliberately dependency-free: a hand-rolled HTTP/1.1 server over
//! `tokio::net`, because adding a web framework for four routes would cost more
//! than it saves (and this crate ships as a single Windows binary).

use crate::agent::provider::ProviderRegistry;
use crate::agent::types::{
    ChatRequest, Effort, Message, ProviderError, Role, StreamEvent, StreamResult, Usage,
};
use crate::config::{AppConfig, BridgeConfig, BridgeUpstream, ModelRoute, WebSource};
use crate::net;
use crate::util::{cap_chars, lock, now_ms};
use crate::websession::{self, BrowserRequest};
use futures_util::{Stream, StreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const MAX_HEADER: usize = 32 * 1024;
const MAX_BODY: usize = 8 * 1024 * 1024;
const LOG_KEEP: usize = 40;

/* ------------------------------------------------------------------ types */

#[derive(Clone, Serialize)]
pub struct LogRow {
    pub at: u64,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub ms: u64,
    pub detail: String,
}

#[derive(Serialize)]
pub struct BridgeStatus {
    pub running: bool,
    pub port: u16,
    pub base_url: String,
    pub upstream: String,
    pub model: String,
    pub sources: usize,
    pub routes: usize,
    pub default_source: String,
    pub requests: u64,
    pub errors: u64,
    pub started_at: u64,
    pub last_error: Option<String>,
    pub log: Vec<LogRow>,
}

struct RunState {
    running: bool,
    port: u16,
    stop: Option<Arc<AtomicBool>>,
    started_at: u64,
    last_error: Option<String>,
}

#[derive(Default)]
struct Stats {
    requests: AtomicU64,
    errors: AtomicU64,
    log: Mutex<VecDeque<LogRow>>,
}

impl Stats {
    fn record(&self, method: &str, path: &str, status: u16, ms: u64, detail: String) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        if status >= 400 {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
        let row = LogRow {
            at: now_ms(),
            method: method.to_string(),
            path: path.to_string(),
            status,
            ms,
            detail,
        };
        let mut log = lock(&self.log);
        log.push_front(row);
        while log.len() > LOG_KEEP {
            log.pop_back();
        }
    }
}

pub struct Bridge {
    config: Arc<Mutex<AppConfig>>,
    stats: Arc<Stats>,
    state: Mutex<RunState>,
}

impl Bridge {
    pub fn new(config: Arc<Mutex<AppConfig>>) -> Self {
        Bridge {
            config,
            stats: Arc::new(Stats::default()),
            state: Mutex::new(RunState {
                running: false,
                port: 0,
                stop: None,
                started_at: 0,
                last_error: None,
            }),
        }
    }

    pub fn status(&self) -> BridgeStatus {
        let bridge = { lock(&self.config).bridge.clone() };
        let state = lock(&self.state);
        let port = if state.running { state.port } else { bridge.port };
        BridgeStatus {
            running: state.running,
            port,
            base_url: format!("http://127.0.0.1:{port}/v1"),
            upstream: upstream_label(bridge.upstream),
            model: self.effective_model(),
            sources: bridge.sources.len(),
            routes: bridge.routes.len(),
            default_source: bridge.default_source.clone(),
            requests: self.stats.requests.load(Ordering::Relaxed),
            errors: self.stats.errors.load(Ordering::Relaxed),
            started_at: state.started_at,
            last_error: state.last_error.clone(),
            log: lock(&self.stats.log).iter().cloned().collect(),
        }
    }

    pub fn log(&self) -> Vec<LogRow> {
        lock(&self.stats.log).iter().cloned().collect()
    }

    fn effective_model(&self) -> String {
        let cfg = lock(&self.config);
        requested_model(&cfg.bridge, None)
    }

    pub async fn start(&self) -> Result<BridgeStatus, String> {
        {
            let state = lock(&self.state);
            if state.running {
                return Ok(self.status());
            }
        }
        let port = lock(&self.config).bridge.port;
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|e| format!("端口 {port} 绑定失败：{e}"))?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_child = stop.clone();
        let config = self.config.clone();
        let stats = self.stats.clone();
        tauri::async_runtime::spawn(async move {
            serve(listener, stop_child, config, stats).await;
        });

        {
            // Scope the guard: `status()` takes the same (non-reentrant) lock.
            let mut state = lock(&self.state);
            state.running = true;
            state.port = port;
            state.stop = Some(stop);
            state.started_at = now_ms();
            state.last_error = None;
        }
        Ok(self.status())
    }

    pub fn stop(&self) -> BridgeStatus {
        {
            let mut state = lock(&self.state);
            if let Some(stop) = state.stop.take() {
                stop.store(true, Ordering::SeqCst);
            }
            state.running = false;
            state.started_at = 0;
        }
        self.status()
    }

    /// Record a startup failure so the panel can show why the port stayed dark.
    pub fn fail(&self, message: String) {
        let mut state = lock(&self.state);
        state.running = false;
        state.started_at = 0;
        state.last_error = Some(message);
    }
}

fn upstream_label(u: BridgeUpstream) -> String {
    match u {
        BridgeUpstream::Web => "web".into(),
        BridgeUpstream::Provider => "provider(debug)".into(),
    }
}

/// The model name a client asked for, or a sane web-side fallback. This is also
/// what gets echoed back in the response, so a client hardcoded to `gpt-4o`
/// sees `gpt-4o` even though ChatGPT Web was the actual upstream.
fn requested_model(cfg: &BridgeConfig, body: Option<&Value>) -> String {
    if let Some(m) = body
        .and_then(|b| b.get("model"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return m.to_string();
    }
    if !cfg.model.trim().is_empty() {
        return cfg.model.trim().to_string();
    }
    cfg.source(&cfg.default_source)
        .map(|s| s.model.clone())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "claude-sonnet-4-5".to_string())
}

/// Pick the web source for a model name: first matching prefix wins, otherwise
/// the configured default. Kept separate from the network call so the request
/// log can name the route even when the upstream fails.
fn source_id_for(cfg: &BridgeConfig, requested: &str) -> String {
    let lower = requested.to_lowercase();
    cfg.routes
        .iter()
        .find(|r: &&ModelRoute| {
            !r.prefix.trim().is_empty() && lower.starts_with(&r.prefix.trim().to_lowercase())
        })
        .map(|r| r.source_id.clone())
        .unwrap_or_else(|| cfg.default_source.clone())
}

fn resolve_source(cfg: &BridgeConfig, requested: &str) -> Result<WebSource, String> {
    let id = source_id_for(cfg, requested);
    cfg.sources
        .iter()
        .find(|s| s.id == id)
        .cloned()
        .ok_or_else(|| {
            format!("模型 '{requested}' 路由到网页源 '{id}'，但该源不存在；请检查 Web2API 路由表")
        })
}

/// A source may only advertise models once its session is actually usable.
/// `http` sources need a pasted credential; `browser` sources need a page
/// whose login probe succeeded. An unlogged-in reverse proxy stays out of the
/// agent's model picker entirely (see the user-facing `/v1/models` route).
fn source_ready(src: &WebSource) -> bool {
    if src.mode.eq_ignore_ascii_case("http") {
        !src.session_key.trim().is_empty()
    } else {
        websession::logged_in(&src.id)
    }
}

/// Vendor bot guards answer with a small JSON `detail` blob instead of a
/// useful status line. Turn that into an instruction the user can act on
/// instead of leaking a raw 502 into the client.
pub fn friendly_upstream_error(src_label: &str, status: u16, raw: &str) -> String {
    let text = cap_chars(raw, 300).trim().to_string();
    let low = text.to_ascii_lowercase();
    let guarded = [
        "unusual activity",
        "unusual traffic",
        "cloudflare",
        "cf-challenge",
        "attention required",
        "verify you are human",
        "captcha",
        "b2591d70",
    ]
    .iter()
    .any(|needle| low.contains(needle));
    if guarded {
        return format!(
            "{src_label} 返回 {status}：官网风控拦截（{text}）。请在「打开官网」窗口里完成人机验证 / 稍后重试；页面驱动模式可绕过私有接口的风控。"
        );
    }
    format!("{src_label} 返回 {status}：{text}（请确认已在官网窗口登录）")
}

fn route_label(cfg: &BridgeConfig, requested: &str) -> String {
    match cfg.upstream {
        BridgeUpstream::Provider => "provider(debug)".into(),
        BridgeUpstream::Web => match resolve_source(cfg, requested) {
            Ok(src) => format!("web:{}", src.id),
            Err(_) => format!("web:{}?", cfg.default_source),
        },
    }
}

/* ---------------------------------------------------------------- server */

async fn serve(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    config: Arc<Mutex<AppConfig>>,
    stats: Arc<Stats>,
) {
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        match tokio::time::timeout(Duration::from_millis(400), listener.accept()).await {
            Ok(Ok((stream, _peer))) => {
                let config = config.clone();
                let stats = stats.clone();
                tokio::spawn(async move {
                    handle(stream, config, stats).await;
                });
            }
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

async fn handle(mut stream: TcpStream, config: Arc<Mutex<AppConfig>>, stats: Arc<Stats>) {
    let started = now_ms();
    let req = match read_request(&mut stream).await {
        Ok(r) => r,
        Err(_) => {
            let _ = write_json(&mut stream, 400, &error_body("malformed HTTP request")).await;
            return;
        }
    };
    let (status, detail) = route(&mut stream, &req, &config).await;
    stats.record(
        &req.method,
        &req.path,
        status,
        now_ms().saturating_sub(started),
        detail,
    );
}

async fn read_request(stream: &mut TcpStream) -> Result<Request, String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end;
    loop {
        let n = stream.read(&mut chunk).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("connection closed".into());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            header_end = pos + 4;
            break;
        }
        if buf.len() > MAX_HEADER {
            return Err("headers too large".into());
        }
    }

    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_ascii_uppercase();
    let path = parts.next().unwrap_or("/").to_string();

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().to_string();
            if key == "content-length" {
                content_length = val.parse().unwrap_or(0);
            }
            headers.push((key, val));
        }
    }
    if content_length > MAX_BODY {
        return Err("body too large".into());
    }

    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);

    Ok(Request { method, path, headers, body })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/* ---------------------------------------------------------------- routes */

async fn route(
    stream: &mut TcpStream,
    req: &Request,
    config: &Arc<Mutex<AppConfig>>,
) -> (u16, String) {
    let path = req.path.split('?').next().unwrap_or("/").to_string();

    if req.method == "OPTIONS" {
        let _ = write_status_only(stream, 204).await;
        return (204, "preflight".into());
    }

    if path == "/" || path == "/health" {
        let (upstream, port) = {
            let cfg = lock(config);
            (upstream_label(cfg.bridge.upstream), cfg.bridge.port)
        };
        let body = json!({
            "ok": true,
            "service": "irixen-web2api",
            "version": env!("CARGO_PKG_VERSION"),
            "upstream": upstream,
            "port": port,
        });
        let _ = write_json(stream, 200, &body).await;
        return (200, "health".into());
    }

    let api_key = lock(config).bridge.api_key.trim().to_string();
    // Model discovery carries no data beyond ids and is what every OpenAI
    // client probes first. Gate the chat endpoints, not `/v1/models`: a client
    // that has not sent its token yet used to get a hard 401 on discovery and
    // never reached the actual authenticated call.
    let public_route = matches!(path.as_str(), "/v1/models" | "/models");
    if !public_route && !api_key.is_empty() && !authorized(req, &api_key) {
        let _ = write_json(
            stream,
            401,
            &error_body("invalid api key — send `Authorization: Bearer <key>`"),
        )
        .await;
        return (401, "unauthorized".into());
    }

    match path.as_str() {
        "/v1/models" | "/models" => {
            let ids = {
                let cfg = lock(config);
                let mut ids: Vec<String> = Vec::new();
                // Debug mode has no browser session to gate on.
                let debug = cfg.bridge.upstream == BridgeUpstream::Provider;
                let ready = |id: &str| {
                    debug
                        || cfg
                            .bridge
                            .source(id)
                            .map(source_ready)
                            .unwrap_or(false)
                };
                let fallback = requested_model(&cfg.bridge, None);
                if ready(&source_id_for(&cfg.bridge, &fallback)) {
                    ids.push(fallback);
                }
                for src in &cfg.bridge.sources {
                    if (debug || source_ready(src)) && !src.model.trim().is_empty() {
                        ids.push(src.model.clone());
                    }
                }
                // Aliases exist so a client hardcoded to a flagship model id
                // still resolves; only advertise one when its reverse proxy
                // is actually logged in.
                for alias in ["gpt-4o", "claude-sonnet-4-5", "gemini-2.5-pro"] {
                    if ready(&source_id_for(&cfg.bridge, alias)) {
                        ids.push(alias.to_string());
                    }
                }
                ids.sort();
                ids.dedup();
                ids
            };
            let data: Vec<Value> = ids
                .iter()
                .map(|id| json!({
                    "id": id,
                    "object": "model",
                    "created": 0,
                    "owned_by": "irixen-web2api",
                }))
                .collect();
            let _ = write_json(stream, 200, &json!({ "object": "list", "data": data })).await;
            (200, format!("models · {} ids", data.len()))
        }
        "/v1/chat/completions" | "/chat/completions" => {
            let (code, detail) = openai_chat(stream, req, config).await;
            (code, detail_with_route(config, req, detail))
        }
        "/v1/messages" | "/messages" => {
            let (code, detail) = anthropic_chat(stream, req, config).await;
            (code, detail_with_route(config, req, detail))
        }
        _ => {
            let _ = write_json(stream, 404, &error_body("unknown endpoint")).await;
            (404, path)
        }
    }
}

fn authorized(req: &Request, key: &str) -> bool {
    if let Some(auth) = req.header("authorization") {
        if let Some(token) = auth.strip_prefix("Bearer ").or_else(|| auth.strip_prefix("bearer ")) {
            if token.trim() == key {
                return true;
            }
        }
    }
    req.header("x-api-key").map(|v| v.trim() == key).unwrap_or(false)
}

/// Name the route in the request log even when the upstream call failed.
fn detail_with_route(config: &Arc<Mutex<AppConfig>>, req: &Request, detail: String) -> String {
    let body = serde_json::from_slice::<Value>(&req.body).ok();
    let cfg = lock(config);
    let model = requested_model(&cfg.bridge, body.as_ref());
    format!("{detail} · {}", route_label(&cfg.bridge, &model))
}

/* -------------------------------------------------------- OpenAI surface */

async fn openai_chat(
    stream: &mut TcpStream,
    req: &Request,
    config: &Arc<Mutex<AppConfig>>,
) -> (u16, String) {
    let body: Value = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => {
            let _ = write_json(stream, 400, &error_body(&format!("invalid JSON: {e}"))).await;
            return (400, "bad json".into());
        }
    };

    let messages = openai_messages(&body);
    if messages.is_empty() {
        let _ = write_json(stream, 400, &error_body("`messages` is required")).await;
        return (400, "no messages".into());
    }

    let model = requested_model(&lock(config).bridge, Some(&body));

    let want_stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    let max_tokens = body
        .get("max_tokens")
        .or_else(|| body.get("max_completion_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(4096) as u32;
    let temperature = body
        .get("temperature")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);

    let up = match upstream_stream(config, messages, model.clone(), max_tokens, temperature).await {
        Ok(s) => s,
        Err(e) => {
            let _ = write_json(stream, 502, &error_body(&e)).await;
            return (502, e.chars().take(120).collect());
        }
    };

    let id = completion_id();
    if want_stream {
        stream_openai(stream, up, &model, &id).await
    } else {
        collect_openai(stream, up, &model, &id).await
    }
}

fn openai_messages(body: &Value) -> Vec<Message> {
    let mut out = Vec::new();
    let Some(arr) = body.get("messages").and_then(|v| v.as_array()) else {
        return out;
    };
    for m in arr {
        let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("user");
        let text = content_text(m.get("content"));
        if text.trim().is_empty() {
            continue;
        }
        match role {
            "system" | "developer" => out.push(Message::system(text)),
            "assistant" => out.push(Message::assistant(text)),
            // Tool traffic cannot be honoured by a text-only upstream; dropping
            // it beats sending a message the model cannot interpret.
            "tool" | "function" => continue,
            _ => out.push(Message::user(text)),
        }
    }
    out
}

fn content_text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

async fn collect_openai(
    stream: &mut TcpStream,
    mut up: StreamResult,
    model: &str,
    id: &str,
) -> (u16, String) {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut usage = Usage::default();
    let mut failure: Option<String> = None;

    while let Some(item) = up.next().await {
        match item {
            Ok(StreamEvent::Text(t)) => text.push_str(&t),
            Ok(StreamEvent::Reasoning(t)) => reasoning.push_str(&t),
            Ok(StreamEvent::Usage(u)) => usage.merge(&u),
            Ok(_) => {}
            Err(e) => {
                failure = Some(e.to_string());
                break;
            }
        }
    }

    if let Some(e) = failure {
        let _ = write_json(stream, 502, &error_body(&e)).await;
        return (502, e.chars().take(120).collect());
    }

    let content = if text.trim().is_empty() { reasoning.clone() } else { text.clone() };
    let mut message = json!({ "role": "assistant", "content": content });
    if !reasoning.is_empty() {
        message["reasoning_content"] = json!(reasoning);
    }
    let body = json!({
        "id": id,
        "object": "chat.completion",
        "created": now_ms() / 1000,
        "model": model,
        "choices": [{ "index": 0, "message": message, "finish_reason": "stop" }],
        "usage": {
            "prompt_tokens": usage.input_tokens,
            "completion_tokens": usage.output_tokens,
            "total_tokens": usage.input_tokens + usage.output_tokens,
        },
    });
    let _ = write_json(stream, 200, &body).await;
    (200, format!("{model} · {} chars", text.chars().count()))
}

async fn stream_openai(
    stream: &mut TcpStream,
    mut up: StreamResult,
    model: &str,
    id: &str,
) -> (u16, String) {
    if write_sse_head(stream).await.is_err() {
        return (499, "client closed".into());
    }
    let mut chars = 0usize;
    let mut reasoning = String::new();
    let mut failure: Option<String> = None;

    while let Some(item) = up.next().await {
        let delta = match item {
            Ok(StreamEvent::Text(t)) if !t.is_empty() => {
                chars += t.chars().count();
                json!({ "content": t })
            }
            Ok(StreamEvent::Reasoning(t)) if !t.is_empty() => {
                reasoning.push_str(&t);
                json!({ "reasoning_content": t })
            }
            Ok(_) => continue,
            Err(e) => {
                failure = Some(e.to_string());
                break;
            }
        };
        let chunk = json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": now_ms() / 1000,
            "model": model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": Value::Null }],
        });
        if write_sse_data(stream, &chunk.to_string()).await.is_err() {
            return (499, "client closed".into());
        }
    }

    // A reasoning-only finish would hand the client an empty message; most
    // OpenAI clients don't render `reasoning_content` at all.
    if chars == 0 && !reasoning.trim().is_empty() {
        let chunk = json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": now_ms() / 1000,
            "model": model,
            "choices": [{ "index": 0, "delta": { "content": reasoning }, "finish_reason": Value::Null }],
        });
        let _ = write_sse_data(stream, &chunk.to_string()).await;
    }

    if let Some(e) = failure {
        let err = json!({ "error": { "message": e, "type": "upstream_error" } });
        let _ = write_sse_data(stream, &err.to_string()).await;
    }
    let fin = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": now_ms() / 1000,
        "model": model,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
    });
    let _ = write_sse_data(stream, &fin.to_string()).await;
    let _ = write_sse_data(stream, "[DONE]").await;
    let _ = write_sse_end(stream).await;

    (200, format!("{model} · {chars} chars stream"))
}

/* ----------------------------------------------------- Anthropic surface */

async fn anthropic_chat(
    stream: &mut TcpStream,
    req: &Request,
    config: &Arc<Mutex<AppConfig>>,
) -> (u16, String) {
    let body: Value = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => {
            let _ = write_json(stream, 400, &anthropic_error(&format!("invalid JSON: {e}"))).await;
            return (400, "bad json".into());
        }
    };

    let messages = anthropic_messages(&body);
    if messages.is_empty() {
        let _ = write_json(stream, 400, &anthropic_error("`messages` is required")).await;
        return (400, "no messages".into());
    }

    let model = requested_model(&lock(config).bridge, Some(&body));

    let want_stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    let max_tokens = body.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(4096) as u32;
    let temperature = body.get("temperature").and_then(|v| v.as_f64()).map(|v| v as f32);

    let up = match upstream_stream(config, messages, model.clone(), max_tokens, temperature).await {
        Ok(s) => s,
        Err(e) => {
            let _ = write_json(stream, 502, &anthropic_error(&e)).await;
            return (502, e.chars().take(120).collect());
        }
    };

    let id = format!("msg_{}", completion_id().trim_start_matches("chatcmpl-"));
    if want_stream {
        stream_anthropic(stream, up, &model, &id).await
    } else {
        collect_anthropic(stream, up, &model, &id).await
    }
}

fn anthropic_messages(body: &Value) -> Vec<Message> {
    let mut out = Vec::new();
    let system = content_text(body.get("system"));
    if !system.trim().is_empty() {
        out.push(Message::system(system));
    }
    let Some(arr) = body.get("messages").and_then(|v| v.as_array()) else {
        return out;
    };
    for m in arr {
        let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("user");
        let text = content_text(m.get("content"));
        if text.trim().is_empty() {
            continue;
        }
        out.push(if role == "assistant" {
            Message::assistant(text)
        } else {
            Message::user(text)
        });
    }
    out
}

async fn collect_anthropic(
    stream: &mut TcpStream,
    mut up: StreamResult,
    model: &str,
    id: &str,
) -> (u16, String) {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut usage = Usage::default();
    let mut failure: Option<String> = None;

    while let Some(item) = up.next().await {
        match item {
            Ok(StreamEvent::Text(t)) => text.push_str(&t),
            Ok(StreamEvent::Reasoning(t)) => reasoning.push_str(&t),
            Ok(StreamEvent::Usage(u)) => usage.merge(&u),
            Ok(_) => {}
            Err(e) => {
                failure = Some(e.to_string());
                break;
            }
        }
    }
    if let Some(e) = failure {
        let _ = write_json(stream, 502, &anthropic_error(&e)).await;
        return (502, e.chars().take(120).collect());
    }

    // Anthropic's schema has no `reasoning_content`. If a reasoning-heavy model
    // spent the whole budget thinking, an empty content array would look like a
    // broken response to every client — surface the thinking instead.
    let body_text = if text.trim().is_empty() { reasoning } else { text };
    let body = json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [{ "type": "text", "text": body_text }],
        "stop_reason": "end_turn",
        "stop_sequence": Value::Null,
        "usage": {
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
        },
    });
    let _ = write_json(stream, 200, &body).await;
    (200, format!("{model} · {} chars", body_text.chars().count()))
}

async fn stream_anthropic(
    stream: &mut TcpStream,
    mut up: StreamResult,
    model: &str,
    id: &str,
) -> (u16, String) {
    if write_sse_head(stream).await.is_err() {
        return (499, "client closed".into());
    }

    let start = json!({
        "type": "message_start",
        "message": {
            "id": id, "type": "message", "role": "assistant", "model": model,
            "content": [], "stop_reason": Value::Null, "stop_sequence": Value::Null,
            "usage": { "input_tokens": 0, "output_tokens": 0 },
        },
    });
    if write_sse_event(stream, "message_start", &start.to_string()).await.is_err() {
        return (499, "client closed".into());
    }
    let block = json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } });
    let _ = write_sse_event(stream, "content_block_start", &block.to_string()).await;

    let mut chars = 0usize;
    let mut reasoning = String::new();
    let mut usage = Usage::default();
    let mut failure: Option<String> = None;
    while let Some(item) = up.next().await {
        match item {
            Ok(StreamEvent::Text(t)) if !t.is_empty() => {
                chars += t.chars().count();
                let delta = json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": { "type": "text_delta", "text": t },
                });
                if write_sse_event(stream, "content_block_delta", &delta.to_string()).await.is_err() {
                    return (499, "client closed".into());
                }
            }
            Ok(StreamEvent::Reasoning(t)) => reasoning.push_str(&t),
            Ok(StreamEvent::Usage(u)) => usage.merge(&u),
            Ok(_) => continue,
            Err(e) => {
                failure = Some(e.to_string());
                break;
            }
        }
    }

    // Same rule as the non-streaming path: never finish with an empty block.
    if chars == 0 && !reasoning.trim().is_empty() {
        let delta = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": reasoning },
        });
        let _ = write_sse_event(stream, "content_block_delta", &delta.to_string()).await;
    }

    let stop = json!({ "type": "content_block_stop", "index": 0 });
    let _ = write_sse_event(stream, "content_block_stop", &stop.to_string()).await;
    if let Some(e) = failure {
        let err = json!({ "type": "error", "error": { "type": "api_error", "message": e } });
        let _ = write_sse_event(stream, "error", &err.to_string()).await;
    }
    let delta = json!({
        "type": "message_delta",
        "delta": { "stop_reason": "end_turn", "stop_sequence": Value::Null },
        "usage": { "output_tokens": usage.output_tokens },
    });
    let _ = write_sse_event(stream, "message_delta", &delta.to_string()).await;
    let _ = write_sse_event(stream, "message_stop", "{\"type\":\"message_stop\"}").await;
    let _ = write_sse_end(stream).await;
    (200, format!("{model} · {chars} chars stream"))
}

/* -------------------------------------------------------------- upstream */

/// Hard ceiling on "time to the first upstream answer". A browser-mode source
/// awaits the whole page reply before the HTTP response's first byte exists;
/// without this ceiling the provider client hits its own 150 s read timeout
/// first and the user only sees a vague 网络错误 instead of the real reason.
async fn upstream_stream(
    config: &Arc<Mutex<AppConfig>>,
    messages: Vec<Message>,
    requested: String,
    max_tokens: u32,
    temperature: Option<f32>,
) -> Result<StreamResult, String> {
    let src_id = {
        let cfg = lock(config);
        source_id_for(&cfg.bridge, &requested)
    };
    match tokio::time::timeout(
        Duration::from_secs(130),
        upstream_stream_inner(config, messages, requested, max_tokens, temperature),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            crate::websession::reveal(&src_id);
            Err(
                "网页源在 130 秒内未返回回答：官网页面可能未登录、停在风控验证页，或后台窗口被浏览器节流。已弹出官网窗口，请完成登录 / 验证并停留在对话页后重试"
                    .into(),
            )
        }
    }
}

async fn upstream_stream_inner(
    config: &Arc<Mutex<AppConfig>>,
    messages: Vec<Message>,
    requested: String,
    max_tokens: u32,
    temperature: Option<f32>,
) -> Result<StreamResult, String> {
    let (bridge, proxy) = {
        let cfg = lock(config);
        (cfg.bridge.clone(), cfg.proxy.clone())
    };

    match bridge.upstream {
        // Debug only. The agent talks to API providers directly; this path
        // exists so the bridge can be smoke-tested without a browser session.
        BridgeUpstream::Provider => {
            let registry = {
                let cfg = lock(config);
                ProviderRegistry::from_config(&cfg.providers, &cfg.proxy)
            };
            let pid = if bridge.provider_id.trim().is_empty() {
                lock(config).default_provider_id()
            } else {
                bridge.provider_id.clone()
            };
            let provider = registry
                .get(&pid)
                .ok_or_else(|| format!("供应商 '{pid}' 未启用或缺 API Key（仅调试模式）"))?;
            let model = if bridge.model.trim().is_empty() {
                requested
            } else {
                bridge.model.clone()
            };
            let req = ChatRequest {
                model,
                messages,
                tools: Vec::new(),
                temperature,
                max_tokens: Some(max_tokens),
                effort: Effort::Auto,
                stream: true,
            };
            provider.chat(&req).await.map_err(|e| e.to_string())
        }
        BridgeUpstream::Web => {
            let src = resolve_source(&bridge, &requested)?;
            web_source_stream(&src, &proxy, messages).await
        }
    }
}

/* ------------------------------------------------------- web sources */

const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Fill a template. {prompt} is a JSON-escaped string body without quotes,
/// {prompt_json} keeps the quotes, {req} is a percent-encoded Gemini-style
/// f.req payload; the rest are raw identifiers.
fn render_placeholders(tpl: &str, src: &WebSource, org: &str, uuid: &str, prompt: &str) -> String {
    let quoted = serde_json::to_string(prompt).unwrap_or_else(|_| "\"\"".into());
    let escaped = quoted.get(1..quoted.len().saturating_sub(1)).unwrap_or("").to_string();
    let req_json = json!([[[prompt], Value::Null, ["", "", ""]]]).to_string();
    let mut out = tpl.to_string();
    out = out.replace("{prompt_json}", &quoted);
    out = out.replace("{prompt}", &escaped);
    out = out.replace("{req}", &pct_encode(&req_json));
    out = out.replace("{model}", &src.model);
    out = out.replace("{uuid}", uuid);
    out = out.replace("{org}", org);
    out = out.replace("{session}", src.session_key.trim());
    out
}

fn auth_headers(tpl: &str, src: &WebSource, org: &str) -> Vec<(String, String)> {
    tpl.lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((
                name.trim().to_string(),
                render_placeholders(value.trim(), src, org, "", ""),
            ))
        })
        .collect()
}

fn dotted<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for seg in path.split('.').filter(|s| !s.is_empty()) {
        cur = match cur {
            Value::Object(map) => map.get(seg)?,
            Value::Array(arr) => arr.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

async fn resolve_org(client: &reqwest::Client, src: &WebSource) -> Result<String, String> {
    if !src.org_id.trim().is_empty() {
        return Ok(src.org_id.trim().to_string());
    }
    if src.org_endpoint.trim().is_empty() {
        return Ok(String::new());
    }
    let url = render_placeholders(&src.org_endpoint, src, "", "", "");
    let mut req = client.get(&url);
    for (name, value) in auth_headers(&src.auth_header, src, "") {
        req = req.header(name.as_str(), value.as_str());
    }
    let resp = req
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("无法读取 {} 的组织列表：{e}", src.label))?;
    if !resp.status().is_success() {
        return Err(format!(
            "{} 拒绝了会话凭证（{}）；请重新复制登录态",
            src.label,
            resp.status()
        ));
    }
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    dotted(&v, &src.org_path)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("{} 的会话里没有组织 ID；可在面板手动填写", src.label))
}

fn guard_ok(v: &Value, guard: &str) -> bool {
    let Some((path, want)) = guard.split_once('=') else {
        return true;
    };
    dotted(v, path.trim())
        .and_then(|x| x.as_str())
        .map(|s| s == want.trim())
        .unwrap_or(false)
}

fn collect_strings(v: &Value, exclude: &str, best: &mut String) {
    match v {
        Value::String(s) => {
            let ignored = matches!(
                s.as_str(),
                "user" | "assistant" | "system" | "text" | "wrb.fr" | "error" | "ok"
            );
            if s.len() > best.len() && s.len() >= 2 && !ignored && !exclude.contains(s.as_str()) {
                *best = s.clone();
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_strings(item, exclude, best);
            }
        }
        Value::Object(map) => {
            for (_, item) in map {
                collect_strings(item, exclude, best);
            }
        }
        _ => {}
    }
}

fn extract_text(v: &Value, path: &str, prompt: &str) -> String {
    if path.trim().is_empty() || path.trim() == "auto" {
        let mut best = String::new();
        collect_strings(v, prompt, &mut best);
        return best;
    }
    match dotted(v, path.trim()) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|x| x.as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Private web endpoints are not uniform: some emit SSE data frames, others
/// stream bare JSON arrays (often with an XSSI prefix or a chunk-length line).
/// Accept any line that parses as JSON and let the source's text path decide
/// what counts as text.
fn parse_web_lines(buf: &mut String, chunk: &str) -> Vec<Result<Value, ProviderError>> {
    let mut events = Vec::new();
    buf.push_str(chunk);
    while let Some(nl) = buf.find('\n') {
        let raw = buf[..nl].trim_end_matches('\r').trim().to_string();
        buf.drain(..=nl);
        let mut data = raw.as_str();
        if let Some(rest) = data.strip_prefix("data:") {
            data = rest.trim();
        }
        let data = data.trim_start_matches(")]}'").trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        if data.chars().all(|c| c.is_ascii_digit() || c.is_whitespace()) {
            continue;
        }
        if data.starts_with('{') || data.starts_with('[') {
            if let Ok(v) = serde_json::from_str::<Value>(data) {
                events.push(Ok(v));
            }
        }
    }
    events
}

fn web_value_stream(resp: reqwest::Response) -> impl Stream<Item = Result<Value, ProviderError>> {
    let mut buf = String::new();
    resp.bytes_stream().flat_map(move |chunk| {
        let events = match chunk {
            Ok(bytes) => parse_web_lines(&mut buf, &String::from_utf8_lossy(&bytes)),
            Err(e) => vec![Err(ProviderError::Http(e.to_string()))],
        };
        futures_util::stream::iter(events)
    })
}

/// Turn parsed web events into text deltas. Shared by the raw HTTP path and
/// the browser-session path so both honour `text_path`, `text_guard` and the
/// cumulative-vs-incremental convention identically.
fn map_web_values<S>(
    values: S,
    path: String,
    guard: String,
    cumulative: bool,
    prompt: String,
) -> impl Stream<Item = Result<StreamEvent, ProviderError>>
where
    S: Stream<Item = Result<Value, ProviderError>>,
{
    let mut seen = String::new();
    values.map(move |item| {
        let v = item?;
        if !guard_ok(&v, &guard) {
            return Ok(StreamEvent::Text(String::new()));
        }
        let full = extract_text(&v, &path, &prompt);
        let delta = if cumulative {
            if full.starts_with(&seen) {
                let d = full[seen.len()..].to_string();
                seen = full;
                d
            } else {
                seen = full.clone();
                full
            }
        } else {
            full
        };
        Ok(StreamEvent::Text(delta))
    })
}

async fn web_source_stream(
    src: &WebSource,
    proxy: &str,
    messages: Vec<Message>,
) -> Result<StreamResult, String> {
    if src.mode.eq_ignore_ascii_case("browser") {
        return browser_web_stream(src, messages).await;
    }
    if src.session_key.trim().is_empty() {
        return Err(format!("网页源「{}」缺少会话凭证，请在 Web2API 面板填写", src.label));
    }
    let client = net::build_client(proxy);
    let org = resolve_org(&client, src).await?;
    let prompt = flatten(&messages);
    if prompt.is_empty() {
        return Err("空对话".into());
    }
    let uuid = fake_uuid();
    let url = render_placeholders(&src.endpoint, src, &org, &uuid, &prompt);
    let body = render_placeholders(&src.body_template, src, &org, &uuid, &prompt);

    let mut req = client.post(&url);
    for (name, value) in auth_headers(&src.auth_header, src, &org) {
        req = req.header(name.as_str(), value.as_str());
    }
    req = req
        .header("content-type", src.content_type.as_str())
        .header("accept", "text/event-stream")
        .header("user-agent", BROWSER_UA);
    let resp = req
        .body(body)
        .send()
        .await
        .map_err(|e| format!("请求 {} 失败：{e}", src.label))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "{} 返回 {status}：{}",
            src.label,
            cap_chars(&text, 300).trim()
        ));
    }

    let events = map_web_values(
        web_value_stream(resp),
        src.text_path.clone(),
        src.text_guard.clone(),
        src.cumulative,
        prompt,
    );
    Ok(Box::pin(events))
}

/// Browser-session path: the request runs inside the vendor's own logged-in
/// page, so it carries the real cookie jar, UA, origin and page-generated
/// tokens. No session key has to be pasted anywhere.
async fn browser_web_stream(src: &WebSource, messages: Vec<Message>) -> Result<StreamResult, String> {
    let prompt = prompt_for_source(src, &messages);
    eprintln!(
        "[irixen] web2api {} -> {} chars ({})",
        src.id,
        prompt.chars().count(),
        if uses_full_prompt(src) { "full prompt" } else { "latest turn only" }
    );
    if prompt.is_empty() {
        return Err("空对话".into());
    }
    if !websession::is_open(&src.id) {
        return Err(format!(
            "网页源「{}」的官网会话未启动；请点「打开官网」登录，或让端点在选中模型时自动启用",
            src.label
        ));
    }
    if !websession::logged_in(&src.id) {
        // The cached probe can predate a login that just completed in this
        // window. Re-probe once so a real session is never refused on stale
        // state, but fail fast (with an actionable message) when the page is
        // genuinely signed out or stuck on a bot-guard challenge.
        let ready = websession::probe_login(&src.id)
            .await
            .map(|i| i.logged_in)
            .unwrap_or(false);
        if !ready {
            return Err(format!(
                "网页源「{}」尚未登录或停在官网验证页：请点「打开官网」完成登录 / 人机验证，就绪后模型会自动出现并可对话。",
                src.label
            ));
        }
    }
    // Default path: let the page answer through its own UI. Vendor private
    // APIs now answer 403 / "Unusual activity has been detected" even from a
    // logged-in page, so the browser's real composer is the reliable surface.
    if !src.driver.eq_ignore_ascii_case("api") {
        let text = websession::run_ui(&src.id, &prompt, &src.ui_script)
            .await
            .map_err(|e| format!("{} 页面驱动失败：{}", src.label, e))?;
        return Ok(Box::pin(futures_util::stream::iter(vec![Ok(
            StreamEvent::Text(text),
        )])));
    }
    let uuid = fake_uuid();
    let url = render_placeholders(&src.endpoint, src, "", &uuid, &prompt);
    let body = render_placeholders(&src.body_template, src, "", &uuid, &prompt);
    // The browser supplies cookie / authorization itself. Passing an empty
    // hand-written one would *override* the real credential.
    let headers = auth_headers(&src.auth_header, src, "")
        .into_iter()
        .filter(|(name, _)| {
            !matches!(
                name.to_ascii_lowercase().as_str(),
                "cookie" | "authorization"
            )
        })
        .collect::<std::collections::HashMap<_, _>>();

    let reply = websession::fetch(
        &src.id,
        BrowserRequest {
            url,
            body,
            headers,
            content_type: src.content_type.clone(),
            org: src.org_id.trim().to_string(),
            org_endpoint: render_placeholders(&src.org_endpoint, src, "", "", ""),
            org_path: src.org_path.clone(),
        },
    )
    .await?;

    if !reply.error.is_empty() {
        return Err(format!("{} 浏览器请求失败：{}", src.label, reply.error));
    }
    if !(200..300).contains(&reply.status) {
        return Err(friendly_upstream_error(&src.label, reply.status, &reply.raw));
    }
    let mut buf = String::new();
    let values = parse_web_lines(&mut buf, &format!("{}\n", reply.raw));
    let events = map_web_values(
        futures_util::stream::iter(values),
        src.text_path.clone(),
        src.text_guard.clone(),
        src.cumulative,
        prompt,
    );
    Ok(Box::pin(events))
}

#[derive(Clone, Serialize)]
pub struct WebTestRow {
    pub id: String,
    pub label: String,
    pub mode: String,
    pub ok: bool,
    pub status: u16,
    pub detail: String,
}

/// Real connectivity check: issue one minimal request per configured source,
/// through the browser session or through the raw template, and report the
/// actual HTTP status. Pinging the local listener alone would hide an expired
/// session — the failure that actually matters here.
pub async fn test_sources(sources: Vec<WebSource>, proxy: &str) -> Vec<WebTestRow> {
    let mut out = Vec::new();
    let client = net::build_client(proxy);
    for src in &sources {
        let row = |ok: bool, status: u16, detail: String| WebTestRow {
            id: src.id.clone(),
            label: src.label.clone(),
            mode: src.mode.clone(),
            ok,
            status,
            detail,
        };
        let browser = src.mode.eq_ignore_ascii_case("browser");
        if browser && !websession::is_open(&src.id) {
            out.push(row(false, 0, "未打开官网窗口；请先点「打开官网」并登录".into()));
            continue;
        }
        // The page driver has no private endpoint to ping, and POSTing a fake
        // "ping" would create a stray conversation. Report the login state
        // instead — that is the only thing that can make a UI source fail.
        if browser && !src.driver.eq_ignore_ascii_case("api") {
            match websession::probe_login_with(&src.id, &src.login_endpoint).await {
                Ok(info) => out.push(row(
                    info.logged_in,
                    200,
                    if info.logged_in {
                        format!("已登录 · 页面驱动就绪{}", info.account)
                    } else {
                        format!("未登录 · {}", info.detail)
                    },
                )),
                Err(e) => out.push(row(false, 0, e)),
            }
            continue;
        }
        let uuid = fake_uuid();
        let url = render_placeholders(&src.endpoint, src, "", &uuid, "ping");
        let body = render_placeholders(&src.body_template, src, "", &uuid, "ping");
        if browser {
            let headers = auth_headers(&src.auth_header, src, "")
                .into_iter()
                .filter(|(name, _)| {
                    !matches!(
                        name.to_ascii_lowercase().as_str(),
                        "cookie" | "authorization" | "content-type" | "content-length"
                    )
                })
                .collect();
            match websession::fetch(
                &src.id,
                BrowserRequest {
                    url,
                    body,
                    headers,
                    content_type: src.content_type.clone(),
                    org: src.org_id.trim().to_string(),
                    org_endpoint: render_placeholders(&src.org_endpoint, src, "", "", ""),
                    org_path: src.org_path.clone(),
                },
            )
            .await
            {
                Ok(r) if !r.error.is_empty() => {
                    out.push(row(false, r.status, format!("浏览器请求失败：{}", r.error)));
                }
                Ok(r) => out.push(row(
                    (200..300).contains(&r.status),
                    r.status,
                    if (200..300).contains(&r.status) {
                        format!("官网同源请求成功 · {} 字节", r.raw.len())
                    } else {
                        friendly_upstream_error(&src.label, r.status, &r.raw)
                    },
                )),
                Err(e) => out.push(row(false, 0, e)),
            }
        } else {
            if src.session_key.trim().is_empty() {
                out.push(row(false, 0, "缺少会话凭证".into()));
                continue;
            }
            let mut req = client.post(&url);
            for (name, value) in auth_headers(&src.auth_header, src, "") {
                req = req.header(name.as_str(), value.as_str());
            }
            req = req
                .header("content-type", src.content_type.as_str())
                .header("accept", "text/event-stream")
                .header("user-agent", BROWSER_UA)
                .body(body);
            match req.send().await {
                Ok(resp) => {
                    let code = resp.status().as_u16();
                    let text = resp.text().await.unwrap_or_default();
                    out.push(row(code < 400, code, cap_chars(&text, 220).trim().to_string()));
                }
                Err(e) => out.push(row(false, 0, e.to_string())),
            }
        }
    }
    out
}

/// One prompt string: the web app takes a single turn, not a message array.
fn flatten(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        let text = m.text_of();
        if text.trim().is_empty() {
            continue;
        }
        match m.role {
            Role::Assistant => {
                out.push_str("Assistant: ");
                out.push_str(text.trim());
                out.push_str("\n\n");
            }
            Role::Tool => {}
            _ => {
                out.push_str(text.trim());
                out.push_str("\n\n");
            }
        }
    }
    out.trim().to_string()
}

/// A logged-in vendor page is a stateful conversation: it already holds the
/// account's own persona (custom instructions / custom GPT) and every earlier
/// turn. Re-typing IRIXEN's system prompt and the whole transcript on each
/// request duplicates context and shows the user a wall of prompt text.
/// Send just the newest user turn and let the page maintain its own context.
fn latest_user_prompt(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .map(|m| m.text_of().trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Stateless transports need the entire transcript in one shot; only a real
/// browser page can rely on its own conversation history.
fn uses_full_prompt(src: &WebSource) -> bool {
    src.mode.eq_ignore_ascii_case("http")
        || src.driver.eq_ignore_ascii_case("api")
        || src.prompt_mode.eq_ignore_ascii_case("full")
}

/// Choose the text typed into the page (UI driver) or sent to a private API.
fn prompt_for_source(src: &WebSource, messages: &[Message]) -> String {
    if uses_full_prompt(src) {
        flatten(messages)
    } else {
        latest_user_prompt(messages).unwrap_or_else(|| flatten(messages))
    }
}

/* -------------------------------------------------------------- plumbing */

fn fake_uuid() -> String {
    let n = now_ms();
    let mix = n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let hex = format!("{mix:016x}{:08x}", n as u32);
    format!(
        "{}-{}-4{}-a{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..15],
        &hex[15..18],
        &hex[2..14]
    )
}

fn completion_id() -> String {
    format!("chatcmpl-{:x}", now_ms().wrapping_mul(31) & 0xffff_ffff_ffff)
}

fn error_body(msg: &str) -> Value {
    json!({ "error": { "message": msg, "type": "invalid_request_error" } })
}

fn anthropic_error(msg: &str) -> Value {
    json!({ "type": "error", "error": { "type": "api_error", "message": msg } })
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        499 => "Client Closed Request",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        _ => "OK",
    }
}

async fn write_status_only(stream: &mut TcpStream, code: u16) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {code} {}\r\naccess-control-allow-origin: *\r\n\
         access-control-allow-headers: *\r\n\
         access-control-allow-methods: GET,POST,OPTIONS\r\ncontent-length: 0\r\n\
         connection: close\r\n\r\n",
        status_text(code)
    );
    stream.write_all(head.as_bytes()).await?;
    stream.flush().await
}

async fn write_json(stream: &mut TcpStream, code: u16, body: &Value) -> std::io::Result<()> {
    let text = serde_json::to_string(body).unwrap_or_else(|_| "{}".to_string());
    let head = format!(
        "HTTP/1.1 {code} {}\r\ncontent-type: application/json; charset=utf-8\r\n\
         content-length: {}\r\naccess-control-allow-origin: *\r\n\
         access-control-allow-headers: *\r\n\
         access-control-allow-methods: GET,POST,OPTIONS\r\nconnection: close\r\n\r\n",
        status_text(code),
        text.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(text.as_bytes()).await?;
    stream.flush().await
}

async fn write_sse_head(stream: &mut TcpStream) -> std::io::Result<()> {
    let head = "HTTP/1.1 200 OK\r\n\
                content-type: text/event-stream; charset=utf-8\r\n\
                cache-control: no-cache, no-transform\r\n\
                access-control-allow-origin: *\r\n\
                x-accel-buffering: no\r\n\
                connection: keep-alive\r\n\
                transfer-encoding: chunked\r\n\r\n";
    stream.write_all(head.as_bytes()).await?;
    stream.flush().await
}

async fn write_chunk(stream: &mut TcpStream, text: &str) -> std::io::Result<()> {
    stream
        .write_all(format!("{:X}\r\n", text.len()).as_bytes())
        .await?;
    stream.write_all(text.as_bytes()).await?;
    stream.write_all(b"\r\n").await?;
    stream.flush().await
}

async fn write_sse_data(stream: &mut TcpStream, data: &str) -> std::io::Result<()> {
    write_chunk(stream, &format!("data: {data}\n\n")).await
}

async fn write_sse_event(stream: &mut TcpStream, event: &str, data: &str) -> std::io::Result<()> {
    write_chunk(stream, &format!("event: {event}\ndata: {data}\n\n")).await
}

async fn write_sse_end(stream: &mut TcpStream) -> std::io::Result<()> {
    stream.write_all(b"0\r\n\r\n").await?;
    stream.flush().await
}
