//! IRIXEN Agent engine.
//!
//! Owns sessions, drives the tool loop, streams normalized events to the
//! webview, meters usage and schedules context compression.

pub mod context;
pub mod meter;
pub mod models;
pub mod provider;
pub mod tools;
pub mod types;

use crate::config::{AppConfig, CompressionConfig};
use crate::mcp::McpHost;
use crate::net;
use crate::skills::SkillRegistry;
use crate::util::{lock, now_ms, short_id};
use context::Compressor;
use futures_util::StreamExt;
use meter::{UsageMeter, UsageReport};
use models::{ModelCache, ModelInfo};
use provider::ProviderRegistry;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Emitter;
use types::*;

#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct Session {
    pub id: String,
    pub provider_id: String,
    pub model: String,
    pub effort: Effort,
    pub title: String,
    pub messages: Vec<Message>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionMeta {
    pub id: String,
    pub provider_id: String,
    pub model: String,
    pub effort: Effort,
    pub title: String,
    pub message_count: usize,
    pub updated_at: u64,
}

pub struct AgentEngine {
    app: tauri::AppHandle,
    pub config: Arc<Mutex<AppConfig>>,
    pub skills: Arc<SkillRegistry>,
    pub mcp: Arc<McpHost>,
    sessions: Mutex<HashMap<String, Session>>,
    stops: Mutex<HashMap<String, Arc<AtomicBool>>>,
    busy: Mutex<HashMap<String, bool>>,
    pub meter: UsageMeter,
    cache: Mutex<ModelCache>,
    active: Mutex<Option<String>>,
}

impl AgentEngine {
    pub fn new(
        app: tauri::AppHandle,
        config: Arc<Mutex<AppConfig>>,
        skills: Arc<SkillRegistry>,
        mcp: Arc<McpHost>,
    ) -> Arc<Self> {
        let engine = Arc::new(AgentEngine {
            app,
            config,
            skills,
            mcp,
            sessions: Mutex::new(HashMap::new()),
            stops: Mutex::new(HashMap::new()),
            busy: Mutex::new(HashMap::new()),
            meter: UsageMeter::new(),
            cache: Mutex::new(ModelCache::load()),
            active: Mutex::new(None),
        });
        engine.load_persisted();
        engine
    }

    // -- plumbing ---------------------------------------------------------

    fn emit(&self, ev: AgentEvent) {
        let _ = self.app.emit("agent://event", &ev);
    }

    fn load_persisted(&self) {
        let dir = crate::config::sessions_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        let mut loaded: Vec<Session> = Vec::new();
        for e in entries.flatten() {
            if let Ok(text) = std::fs::read_to_string(e.path()) {
                if let Ok(s) = serde_json::from_str::<Session>(&text) {
                    loaded.push(s);
                }
            }
        }
        loaded.sort_by_key(|s| s.updated_at);
        let keep = loaded.split_off(loaded.len().saturating_sub(20));
        let mut map = lock(&self.sessions);
        for s in keep {
            map.insert(s.id.clone(), s);
        }
    }

    fn save_session(&self, s: &Session) {
        let dir = crate::config::sessions_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(text) = serde_json::to_string(s) {
            let _ = std::fs::write(dir.join(format!("{}.json", s.id)), text);
        }
    }

    pub fn is_busy(&self, id: &str) -> bool {
        lock(&self.busy).get(id).copied().unwrap_or(false)
    }

    pub fn sessions(&self) -> Vec<SessionMeta> {
        let map = lock(&self.sessions);
        let mut out: Vec<SessionMeta> = map
            .values()
            .map(|s| SessionMeta {
                id: s.id.clone(),
                provider_id: s.provider_id.clone(),
                model: s.model.clone(),
                effort: s.effort,
                title: s.title.clone(),
                message_count: s.messages.len(),
                updated_at: s.updated_at,
            })
            .collect();
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out
    }

    pub fn active_id(&self) -> Option<String> {
        lock(&self.active).clone()
    }

    /// Create a session seeded from the current config selection.
    pub fn new_session(&self) -> SessionMeta {
        let cfg = lock(&self.config);
        // Keep the user's explicit choice only if it can actually talk; a stale
        // `active` pointing at a preset provider with no key is exactly how a
        // fresh session ends up 401-ing against `openai`.
        let active_ready = cfg
            .active
            .as_ref()
            .and_then(|a| cfg.provider(&a.provider_id).map(|p| (a, p)))
            .filter(|(_, p)| {
                p.enabled
                    && !p.base_url.trim().is_empty()
                    && (!p.api_key.trim().is_empty() || crate::config::is_loopback(&p.base_url))
            })
            .map(|(a, _)| (a.provider_id.clone(), a.model.clone(), a.effort));
        // New sessions inherit the sticky thinking-effort default from config;
        // otherwise every restart silently reset the knob to Auto.
        let (provider_id, model, effort) = active_ready
            .unwrap_or_else(|| (cfg.default_provider_id(), String::new(), cfg.effort));
        drop(cfg);
        let id = short_id();
        let session = Session {
            id: id.clone(),
            provider_id,
            model,
            effort,
            title: "新会话".to_string(),
            messages: Vec::new(),
            created_at: now_ms(),
            updated_at: now_ms(),
        };
        lock(&self.sessions).insert(id.clone(), session);
        *lock(&self.active) = Some(id.clone());
        self.sessions()
            .into_iter()
            .find(|s| s.id == id)
            .unwrap_or(SessionMeta {
                id,
                provider_id: String::new(),
                model: String::new(),
                effort,
                title: "新会话".into(),
                message_count: 0,
                updated_at: now_ms(),
            })
    }

    pub fn set_active(&self, id: &str) -> bool {
        if lock(&self.sessions).contains_key(id) {
            *lock(&self.active) = Some(id.to_string());
            true
        } else {
            false
        }
    }

    pub fn messages(&self, id: &str) -> Vec<Message> {
        lock(&self.sessions)
            .get(id)
            .map(|s| s.messages.clone())
            .unwrap_or_default()
    }

    pub fn clear(&self, id: &str) {
        if let Some(s) = lock(&self.sessions).get_mut(id) {
            s.messages.clear();
            s.title = "新会话".into();
            s.updated_at = now_ms();
        }
    }

    pub fn delete_session(&self, id: &str) {
        lock(&self.sessions).remove(id);
        let _ = std::fs::remove_file(crate::config::sessions_dir().join(format!("{id}.json")));
    }

    pub fn set_model(
        &self,
        id: &str,
        provider_id: Option<String>,
        model: Option<String>,
        effort: Option<Effort>,
    ) -> Result<(), String> {
        let mut map = lock(&self.sessions);
        let s = map.get_mut(id).ok_or("session not found")?;
        if let Some(p) = provider_id {
            s.provider_id = p;
        }
        if let Some(m) = model {
            s.model = m;
        }
        if let Some(e) = effort {
            s.effort = e;
        }
        s.updated_at = now_ms();
        Ok(())
    }

    pub fn stop(&self, id: &str) {
        if let Some(flag) = lock(&self.stops).get(id).cloned() {
            flag.store(true, Ordering::SeqCst);
        }
    }

    // -- model directory --------------------------------------------------

    pub async fn models(&self, provider_id: &str, refresh: bool) -> Result<Vec<ModelInfo>, String> {
        let (provider_cfg, proxy) = {
            let cfg = lock(&self.config);
            (
                cfg.provider(provider_id)
                    .cloned()
                    .ok_or_else(|| format!("unknown provider '{provider_id}'"))?,
                cfg.proxy.clone(),
            )
        };
        if provider_cfg.base_url.trim().is_empty() {
            return Err("provider has no base URL".into());
        }
        if !refresh {
            let cache = lock(&self.cache);
            if let Some(list) = cache.get_fresh(provider_id, 10 * 60 * 1000) {
                let protocol = provider_cfg.protocol;
                return Ok(list
                    .into_iter()
                    .map(|mut m| {
                        m.refresh_thinking(protocol);
                        m
                    })
                    .collect());
            }
        }
        let registry =
            ProviderRegistry::from_config_any_state(std::slice::from_ref(&provider_cfg), &proxy);
        let provider = registry
            .get(provider_id)
            .ok_or("provider has no base URL")?;
        match provider.list_models().await {
            Ok(raw) => {
                let infos = models::with_thinking(provider_id, provider_cfg.protocol, raw);
                {
                    let mut cache = lock(&self.cache);
                    cache
                        .entries
                        .insert(provider_id.to_string(), (now_ms(), infos.clone()));
                    cache.save();
                }
                Ok(infos)
            }
            Err(e) => {
                // stale cache is better than an empty picker
                let cached = lock(&self.cache)
                    .entries
                    .get(provider_id)
                    .map(|(_, m)| m.clone())
                    .unwrap_or_default();
                if cached.is_empty() {
                    Err(e.to_string())
                } else {
                    Ok(cached
                        .into_iter()
                        .map(|mut m| {
                            m.source = models::ModelSource::Cached;
                            m.refresh_thinking(provider_cfg.protocol);
                            m
                        })
                        .collect())
                }
            }
        }
    }

    pub async fn test_provider(&self, provider_id: &str) -> Result<usize, String> {
        let list = self.models(provider_id, true).await?;
        Ok(list.len())
    }

    /// Step-by-step connectivity check. "Cannot reach the provider" is never
    /// actionable on its own, so this reports the normalised URL, the proxy
    /// actually used, the HTTP status and the real transport error.
    pub async fn probe(&self, provider_id: &str) -> Result<ProbeReport, String> {
        let (cfg, proxy, protocol) = {
            let c = lock(&self.config);
            let p = c
                .provider(provider_id)
                .cloned()
                .ok_or_else(|| format!("未知供应商 '{provider_id}'"))?;
            let proto = p.protocol;
            (p, c.proxy.clone(), proto)
        };

        let base = net::normalize_base(&cfg.base_url);
        let path = {
            let p = cfg.models_path.trim();
            if p.is_empty() { "/models".to_string() } else { p.to_string() }
        };
        let url = net::join(&base, &path);
        let proxy_label = net::proxy_label(&proxy);

        let mut steps = Vec::new();
        let typed = cfg.base_url.trim().trim_end_matches('/').to_string();
        steps.push(ProbeStep::new(
            "Base URL",
            !base.is_empty(),
            if base.is_empty() {
                "为空 —— 请填写形如 https://api.openai.com/v1 的地址".to_string()
            } else if base != typed {
                format!("{typed}  →  {base}（已自动修正）")
            } else {
                base.clone()
            },
        ));
        steps.push(ProbeStep::new("代理", true, proxy_label.clone()));
        steps.push(ProbeStep::new(
            "请求地址",
            url.starts_with("http"),
            url.clone(),
        ));
        // `enabled` does not gate the request below — a provider still being
        // configured has to be testable — but it does gate the agent, so the
        // report states which side of that line this one is on.
        steps.push(ProbeStep::new(
            "启用",
            cfg.enabled,
            if cfg.enabled {
                "已启用 —— 可在聊天中选用".to_string()
            } else {
                "未启用 —— 测试照常可用，打开本卡片右上角的开关才能在聊天里选它".to_string()
            },
        ));

        let key = cfg.api_key.trim();
        if key.is_empty() {
            steps.push(ProbeStep::new("API Key", false, "未填写".to_string()));
        } else {
            let n = key.chars().count();
            let head: String = key.chars().take(4).collect();
            let tail: String = key.chars().skip(n.saturating_sub(4)).collect();
            let masked = if n <= 8 { "****".to_string() } else { format!("{head}…{tail}") };
            steps.push(ProbeStep::new("API Key", true, format!("{masked}（{n} 字符）")));
        }

        if base.is_empty() {
            return Ok(ProbeReport {
                url,
                proxy: proxy_label,
                ok: false,
                steps,
                models: 0,
                error: Some("Base URL 为空，无法发起请求。".to_string()),
            });
        }

        let client = net::build_client(&proxy);
        let mut req = client.get(&url);
        req = match protocol {
            crate::config::Protocol::Openai => req.bearer_auth(key),
            crate::config::Protocol::Anthropic => req
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01"),
            crate::config::Protocol::Gemini => req.header("x-goog-api-key", key),
        };
        for (k, v) in &cfg.extra_headers {
            if let (Ok(name), Ok(value)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(v),
            ) {
                req = req.header(name, value);
            }
        }

        let t0 = std::time::Instant::now();
        match req.send().await {
            Err(e) => {
                let ms = t0.elapsed().as_millis();
                steps.push(ProbeStep::new(
                    "传输",
                    false,
                    format!("{ms}ms · 连接未建立"),
                ));
                let msg = net::explain_transport(&e, &url, &proxy_label);
                Ok(ProbeReport { url, proxy: proxy_label, ok: false, steps, models: 0, error: Some(msg) })
            }
            Ok(resp) => {
                let status = resp.status().as_u16();
                let ms = t0.elapsed().as_millis();
                let body = resp.text().await.unwrap_or_default();
                if !(200..300).contains(&status) {
                    // Raw server answer next to the URL: the UI only shows the
                    // mapped sentence, which cannot tell a wrong key from a
                    // missing one or from a gateway rejecting the origin.
                    let peek: String = body.chars().take(300).collect();
                    eprintln!("[irixen] probe {url} -> {status} ({ms}ms) {peek}");
                    steps.push(ProbeStep::new("HTTP", false, format!("{status} · {ms}ms")));
                    let msg = net::explain_status(status, &body, &url);
                    return Ok(ProbeReport {
                        url,
                        proxy: proxy_label,
                        ok: false,
                        steps,
                        models: 0,
                        error: Some(msg),
                    });
                }
                let n = count_models(&body);
                eprintln!("[irixen] probe {url} -> {status} ({ms}ms), {n} models");
                steps.push(ProbeStep::new("HTTP", true, format!("{status} · {ms}ms")));
                steps.push(ProbeStep::new(
                    "响应解析",
                    n > 0,
                    if n > 0 { format!("{n} 个模型") } else { "返回 200，但没有识别到模型列表（协议或路径可能不匹配）".to_string() },
                ));
                Ok(ProbeReport { url, proxy: proxy_label, ok: true, steps, models: n, error: None })
            }
        }
    }

    // -- sending ----------------------------------------------------------

    pub fn send(
        self: &Arc<Self>,
        session_id: &str,
        text: &str,
        attachments: Vec<Attachment>,
    ) -> Result<(), String> {
        {
            let map = lock(&self.sessions);
            if !map.contains_key(session_id) {
                return Err("session not found".into());
            }
        }
        if self.is_busy(session_id) {
            return Err("agent is already running in this session".into());
        }
        let flag = Arc::new(AtomicBool::new(false));
        lock(&self.stops).insert(session_id.to_string(), flag.clone());
        lock(&self.busy).insert(session_id.to_string(), true);
        let engine = self.clone();
        let sid = session_id.to_string();
        let prompt = text.to_string();
        tauri::async_runtime::spawn(async move {
            // run_turn takes `self: Arc<Self>`, so hand it a clone and keep ours
            // for the bookkeeping below.
            engine
                .clone()
                .run_turn(sid.clone(), prompt, attachments, flag)
                .await;
            lock(&engine.busy).insert(sid.clone(), false);
            lock(&engine.stops).remove(&sid);
            if let Some(s) = lock(&engine.sessions).get(&sid).cloned() {
                engine.save_session(&s);
            }
        });
        Ok(())
    }

    async fn run_turn(
        self: Arc<Self>,
        session_id: String,
        prompt: String,
        attachments: Vec<Attachment>,
        stop: Arc<AtomicBool>,
    ) {
        let (provider_id, model, effort) = {
            let map = lock(&self.sessions);
            match map.get(&session_id) {
                Some(s) => (s.provider_id.clone(), s.model.clone(), s.effort),
                None => {
                    self.emit(AgentEvent::Error {
                        session: session_id,
                        text: "会话不存在".into(),
                    });
                    return;
                }
            }
        };

        if provider_id.is_empty() {
            self.emit(AgentEvent::Error {
                session: session_id,
                text: "尚未配置 API 供应商。打开 设置 → 供应商，填入 Base URL 与 API Key。".into(),
            });
            return;
        }
        if model.is_empty() {
            self.emit(AgentEvent::Error {
                session: session_id,
                text: "尚未选择模型。在输入框上方选择供应商与模型（会自动拉取模型列表）。".into(),
            });
            return;
        }

        // Refuse before the socket is opened. An empty key used to travel all
        // the way to the vendor and come back as a bare 401 "鉴权失败", which
        // reads like a wrong key instead of a missing one.
        {
            let cfg = lock(&self.config);
            if let Some(p) = cfg.provider(&provider_id) {
                if p.api_key.trim().is_empty() && !crate::config::is_loopback(&p.base_url) {
                    let name = if p.name.trim().is_empty() { p.id.clone() } else { p.name.clone() };
                    self.emit(AgentEvent::Error {
                        session: session_id,
                        text: format!(
                            "供应商「{name}」还没有填 API Key。打开 设置 → 供应商，粘贴 Key，点「测试连接」确认通过后再发送。"
                        ),
                    });
                    return;
                }
            }
        }

        // ---- user message (prompt + attachments) ----
        let mut body = prompt.clone();
        let mut consumed = 0usize;
        for a in &attachments {
            if consumed > 24_000 {
                break;
            }
            let path = std::path::Path::new(&a.path);
            if a.is_dir {
                let listing = crate::agent::tools::execute_native(
                    "list_dir",
                    &serde_json::json!({ "path": a.name, "depth": 2 }),
                    std::path::Path::new(&a.path),
                )
                .await
                .unwrap_or_default();
                body.push_str(&format!("\n\n--- 目录: {} ---\n{}", a.path, listing));
                consumed += listing.len();
            } else if let Ok(content) = std::fs::read_to_string(path) {
                let capped = crate::util::cap_chars(&content, 8_000);
                consumed += capped.len();
                body.push_str(&format!("\n\n--- 文件: {} ---\n{}", a.path, capped));
            }
        }

        {
            let mut map = lock(&self.sessions);
            if let Some(s) = map.get_mut(&session_id) {
                if s.messages.is_empty() {
                    s.title = crate::util::one_line(&prompt, 32);
                }
                s.messages.push(Message::user(body));
                s.updated_at = now_ms();
            }
        }

        // ---- tool list + skills ----
        let system_prompt = {
            let cfg = lock(&self.config);
            cfg.agent.system_prompt.clone()
        };
        let auto_skills = lock(&self.config).agent.auto_skills;
        let matched = self.skills.match_for(&prompt, auto_skills);
        let mut system = system_prompt;
        if !matched.is_empty() {
            system.push_str("\n\n# 当前激活的 Skill 指引（必须遵守）\n");
            for s in &matched {
                system.push_str(&format!("\n## {}\n{}\n", s.name, s.body.trim()));
            }
            let names: Vec<&str> = matched.iter().map(|s| s.name.as_str()).collect();
            self.emit(AgentEvent::Status {
                session: session_id.clone(),
                text: format!("已加载 skill: {}", names.join(", ")),
            });
        }

        let max_iter = lock(&self.config).agent.max_iterations.max(1);
        let temperature = lock(&self.config).agent.temperature;
        let max_tokens = Some(8192u32);

        let mut iteration = 0u32;
        loop {
            if stop.load(Ordering::SeqCst) {
                self.emit(AgentEvent::Done {
                    session: session_id.clone(),
                    text: "已停止".into(),
                });
                return;
            }
            if iteration >= max_iter {
                self.emit(AgentEvent::Error {
                    session: session_id.clone(),
                    text: format!("达到最大工具轮次 ({max_iter})，已停止。"),
                });
                return;
            }
            iteration += 1;

            // ---- compression ----
            self.maybe_compress(&session_id, &provider_id, &model).await;

            // ---- request ----
            let registry = {
                let cfg = lock(&self.config);
                ProviderRegistry::from_config(&cfg.providers, &cfg.proxy)
            };
            let provider = match registry.get(&provider_id) {
                Some(p) => p,
                None => {
                    self.emit(AgentEvent::Error {
                        session: session_id.clone(),
                        text: format!("供应商 '{provider_id}' 未启用或缺少 Base URL"),
                    });
                    return;
                }
            };

            let mut messages = vec![Message::system(system.clone())];
            messages.extend(self.messages(&session_id));

            let req = ChatRequest {
                model: model.clone(),
                messages,
                tools: self.tool_specs(),
                temperature,
                max_tokens,
                effort,
                stream: true,
            };

            self.emit(AgentEvent::Status {
                session: session_id.clone(),
                text: "思考中…".into(),
            });

            let mut stream = match provider.chat(&req).await {
                Ok(s) => s,
                Err(e) => {
                    self.meter.record_error();
                    self.emit(AgentEvent::Error {
                        session: session_id.clone(),
                        text: e.to_string(),
                    });
                    return;
                }
            };

            let mut text = String::new();
            let mut tool_calls: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
            let mut failed: Option<String> = None;

            while let Some(item) = stream.next().await {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                match item {
                    Ok(StreamEvent::Text(t)) => {
                        text.push_str(&t);
                        self.emit(AgentEvent::Delta {
                            session: session_id.clone(),
                            text: t,
                        });
                    }
                    Ok(StreamEvent::Reasoning(t)) => {
                        self.emit(AgentEvent::Reasoning {
                            session: session_id.clone(),
                            text: t,
                        });
                    }
                    Ok(StreamEvent::ToolCallStart { index, id, name }) => {
                        let entry = tool_calls.entry(index).or_default();
                        if !id.is_empty() {
                            entry.0 = id;
                        }
                        if !name.is_empty() {
                            entry.1 = name;
                        }
                    }
                    Ok(StreamEvent::ToolCallArgs { index, delta }) => {
                        tool_calls.entry(index).or_default().2.push_str(&delta);
                    }
                    Ok(StreamEvent::Usage(u)) => {
                        self.meter.record(&format!("{provider_id}/{model}"), &session_id, &u);
                        self.emit(AgentEvent::Usage {
                            session: session_id.clone(),
                            usage: u,
                        });
                    }
                    Err(e) => {
                        failed = Some(e.to_string());
                        break;
                    }
                }
            }

            if let Some(err) = failed {
                self.meter.record_error();
                self.emit(AgentEvent::Error {
                    session: session_id.clone(),
                    text: err,
                });
                return;
            }

            if stop.load(Ordering::SeqCst) {
                self.emit(AgentEvent::Done {
                    session: session_id.clone(),
                    text: "已停止".into(),
                });
                return;
            }

            // ---- no tools → final answer ----
            if tool_calls.is_empty() {
                let mut map = lock(&self.sessions);
                if let Some(s) = map.get_mut(&session_id) {
                    if !text.trim().is_empty() {
                        s.messages.push(Message::assistant(text.clone()));
                    }
                    s.updated_at = now_ms();
                }
                drop(map);
                self.emit(AgentEvent::Done {
                    session: session_id.clone(),
                    text,
                });
                return;
            }

            // ---- assistant message carrying tool calls ----
            let mut parts: Vec<ContentPart> = Vec::new();
            if !text.trim().is_empty() {
                parts.push(ContentPart::Text { text: text.clone() });
            }
            for (_, (id, name, args)) in &tool_calls {
                parts.push(ContentPart::ToolCall {
                    id: if id.is_empty() { short_id() } else { id.clone() },
                    name: name.clone(),
                    arguments: if args.trim().is_empty() {
                        "{}".into()
                    } else {
                        args.clone()
                    },
                });
            }
            {
                let mut map = lock(&self.sessions);
                if let Some(s) = map.get_mut(&session_id) {
                    s.messages.push(Message {
                        role: Role::Assistant,
                        parts: parts.clone(),
                    });
                    s.updated_at = now_ms();
                }
            }

            // ---- execute tools ----
            let mut results: Vec<ContentPart> = Vec::new();
            for p in parts.iter() {
                if let ContentPart::ToolCall { id, name, arguments } = p {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    self.emit(AgentEvent::ToolCall {
                        session: session_id.clone(),
                        id: id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    });
                    let args: serde_json::Value =
                        serde_json::from_str(arguments).unwrap_or_else(|_| serde_json::json!({}));
                    let (output, is_error) = self.execute_tool(name, &args).await;
                    self.emit(AgentEvent::ToolResult {
                        session: session_id.clone(),
                        id: id.clone(),
                        name: name.clone(),
                        output: crate::util::cap_chars(&output, 2_000),
                        is_error,
                    });
                    results.push(ContentPart::ToolResult {
                        call_id: id.clone(),
                        name: name.clone(),
                        output,
                        is_error,
                    });
                }
            }
            {
                let mut map = lock(&self.sessions);
                if let Some(s) = map.get_mut(&session_id) {
                    s.messages.push(Message {
                        role: Role::Tool,
                        parts: results,
                    });
                    s.updated_at = now_ms();
                }
            }
        }
    }

    fn tool_specs(&self) -> Vec<serde_json::Value> {
        let mut tools: Vec<serde_json::Value> = tools::native_tool_specs()
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters
                    }
                })
            })
            .collect();
        for m in self.mcp.tools_all() {
            tools.push(serde_json::json!({
                "type": "function",
                "function": {
                    "name": m.qualified_name,
                    "description": format!("[MCP · {}] {}", m.server_name, m.description),
                    "parameters": m.input_schema
                }
            }));
        }
        tools
    }

    async fn execute_tool(&self, name: &str, args: &serde_json::Value) -> (String, bool) {
        if name.starts_with("mcp__") {
            match self.mcp.call(name, args).await {
                Ok(out) => (out, false),
                Err(e) => (e, true),
            }
        } else {
            let workspace = {
                let cfg = lock(&self.config);
                cfg.workspace.clone().unwrap_or_default()
            };
            if workspace.is_empty() {
                return ("没有打开工作区。请先在左上角打开一个文件夹。".into(), true);
            }
            match tools::execute_native(name, args, std::path::Path::new(&workspace)).await {
                Ok(out) => (out, false),
                Err(e) => (e, true),
            }
        }
    }

    async fn maybe_compress(&self, session_id: &str, provider_id: &str, model: &str) {
        let cfg = lock(&self.config).agent.compression.clone();
        if !cfg.enabled {
            return;
        }
        let compressor = Compressor::new(cfg);
        let mut report = CompressionReport::default();
        let mut middle: Option<Vec<Message>> = None;

        // Phase 1 — decide and trim while holding the session lock briefly.
        {
            let mut map = lock(&self.sessions);
            let s = match map.get_mut(session_id) {
                Some(s) => s,
                None => return,
            };
            if !compressor.should_compress(&s.messages) {
                return;
            }
            report.tokens_before = Compressor::estimate_tokens(&s.messages);
            report.tool_results_elided = compressor.trim_tool_results(&mut s.messages);
            if Compressor::estimate_tokens(&s.messages) >= compressor.budget() {
                let (keep, mid) = compressor.split_window(&s.messages);
                report.messages_dropped = s.messages.len().saturating_sub(keep.len());
                s.messages = keep;
                middle = Some(mid);
            }
            report.tokens_after = Compressor::estimate_tokens(&s.messages);
        }

        // Phase 2 — summarize the dropped middle (network call, lock released).
        if let Some(mid) = middle {
            if !mid.is_empty() {
                self.emit(AgentEvent::Status {
                    session: session_id.to_string(),
                    text: format!(
                        "上下文压缩：折叠 {} 条历史消息，生成摘要中…",
                        report.messages_dropped
                    ),
                });
                let registry = {
                    let c = lock(&self.config);
                    ProviderRegistry::from_config(&c.providers, &c.proxy)
                };
                let digest = match registry.get(provider_id) {
                    Some(p) => compressor.digest(p.as_ref(), model, &mid).await,
                    None => None,
                };
                if let Some(d) = digest {
                    report.digest = Some(d.clone());
                    let mut map = lock(&self.sessions);
                    if let Some(s) = map.get_mut(session_id) {
                        s.messages.insert(
                            0,
                            Message::user(format!(
                                "[上下文压缩摘要 — 之前对话的要点，供你继续工作]\n{d}"
                            )),
                        );
                        report.tokens_after = Compressor::estimate_tokens(&s.messages);
                    }
                }
            }
        }

        self.emit(AgentEvent::Compressed {
            session: session_id.to_string(),
            report,
        });
    }

    pub fn usage(&self) -> UsageReport {
        self.meter.report()
    }

    pub fn compression_config(&self) -> CompressionConfig {
        lock(&self.config).agent.compression.clone()
    }
}

// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct ProbeStep {
    pub label: String,
    pub ok: bool,
    pub detail: String,
}

impl ProbeStep {
    fn new(label: &str, ok: bool, detail: String) -> Self {
        ProbeStep { label: label.to_string(), ok, detail }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProbeReport {
    pub url: String,
    pub proxy: String,
    pub ok: bool,
    pub steps: Vec<ProbeStep>,
    pub models: usize,
    /// Populated when `ok` is false: a sentence that names the likely cause.
    pub error: Option<String>,
}

fn count_models(body: &str) -> usize {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return 0;
    };
    let arr = v
        .get("data")
        .and_then(|d| d.as_array())
        .or_else(|| v.get("models").and_then(|d| d.as_array()));
    match arr {
        Some(a) => a
            .iter()
            .filter(|m| {
                m.get("id").is_some()
                    || m.get("name").is_some()
                    || m.get("supportedGenerationMethods").is_some()
            })
            .count(),
        None => 0,
    }
}
