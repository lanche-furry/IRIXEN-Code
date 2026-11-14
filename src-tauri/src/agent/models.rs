//! Model directory: auto-discovery of vendor model lists plus reasoning
//! ("thinking") capability detection.
//!
//! The directory has three layers:
//! 1. fetched  — whatever the vendor's `/models` endpoint returns
//! 2. heuristic — reasoning-effort support inferred from model id patterns
//! 3. cached    — last successful fetch, kept on disk so the UI stays usable
//!               offline or while the API key is missing

use crate::agent::types::Effort;
use crate::config::Protocol;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThinkingSupport {
    /// Not probed yet.
    Unknown,
    /// Model cannot reason at all.
    Unsupported,
    /// Discrete effort levels (OpenAI o-series / GPT-5 style).
    Effort { levels: Vec<Effort> },
    /// Numeric thinking budget (Anthropic / Gemini / Qwen style).
    Budget { min: u32, max: u32 },
    /// Always-on reasoning, no knob (e.g. deepseek-reasoner).
    AlwaysOn,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    Fetched,
    Heuristic,
    Cached,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub provider_id: String,
    pub display_name: Option<String>,
    pub owned_by: Option<String>,
    pub context_length: Option<u64>,
    pub thinking: ThinkingSupport,
    pub source: ModelSource,
}

/// Infer reasoning capabilities from a model id + protocol.
pub fn detect_thinking(model_id: &str, protocol: Protocol) -> ThinkingSupport {
    let id = model_id.to_ascii_lowercase();
    match protocol {
        Protocol::Openai => {
            // DeepSeek V4 family: thinking is on by default and controlled by a
            // single `reasoning_effort` string — `none` genuinely stops the
            // reasoning channel (verified against deepseek-flash / v4-pro).
            // The legacy `deepseek-reasoner` alias has no knob at all.
            if id.contains("deepseek") {
                if id.contains("reasoner") || id.contains("-r1") {
                    return ThinkingSupport::AlwaysOn;
                }
                return ThinkingSupport::Effort {
                    levels: vec![
                        Effort::Minimal,
                        Effort::Low,
                        Effort::Medium,
                        Effort::High,
                    ],
                };
            }
            if id.contains("qwq") || id.contains("qwen3") && id.contains("thinking") {
                return ThinkingSupport::Budget { min: 0, max: 32_768 };
            }
            if id.contains("glm-z1") || id.contains("glm-4.5") || id.contains("glm-4.6") {
                return ThinkingSupport::Effort { levels: vec![Effort::Low, Effort::High] };
            }
            if id.contains("gpt-5") || id.contains("o1") || id.contains("o3") || id.contains("o4")
            {
                if id.contains("mini") {
                    return ThinkingSupport::Effort { levels: vec![Effort::Low, Effort::Medium, Effort::High] };
                }
                return ThinkingSupport::Effort { levels: vec![
                    Effort::Minimal,
                    Effort::Low,
                    Effort::Medium,
                    Effort::High,
                ] };
            }
            if id.contains("reasoning") || id.contains("thinking") || id.contains("-r1") {
                return ThinkingSupport::Budget { min: 0, max: 32_768 };
            }
            ThinkingSupport::Unsupported
        }
        Protocol::Anthropic => {
            if id.contains("claude-3-7")
                || id.contains("claude-sonnet-4")
                || id.contains("claude-opus-4")
                || id.contains("claude-haiku-4")
                || id.contains("claude-3.7")
            {
                return ThinkingSupport::Budget { min: 1_024, max: 32_000 };
            }
            ThinkingSupport::Unsupported
        }
        Protocol::Gemini => {
            if id.contains("gemini-2.5") || id.contains("gemini-3") {
                return ThinkingSupport::Budget { min: 0, max: 24_576 };
            }
            ThinkingSupport::Unsupported
        }
    }
}

/// UI-facing description of what the effort selector should offer.
pub fn effort_options(thinking: &ThinkingSupport) -> Vec<Effort> {
    match thinking {
        ThinkingSupport::Unknown => vec![
            Effort::Auto,
            Effort::Off,
            Effort::Low,
            Effort::Medium,
            Effort::High,
        ],
        ThinkingSupport::Unsupported => vec![Effort::Auto],
        ThinkingSupport::AlwaysOn => vec![Effort::Auto],
        ThinkingSupport::Effort { levels } => {
            let mut v = vec![Effort::Auto, Effort::Off];
            v.extend(levels.iter().copied());
            v
        }
        ThinkingSupport::Budget { .. } => vec![
            Effort::Auto,
            Effort::Off,
            Effort::Low,
            Effort::Medium,
            Effort::High,
        ],
    }
}

pub fn effort_label(effort: Effort) -> &'static str {
    match effort {
        Effort::Auto => "自动",
        Effort::Off => "关闭",
        Effort::Minimal => "极低",
        Effort::Low => "低",
        Effort::Medium => "中",
        Effort::High => "高",
    }
}

// ---------------------------------------------------------------------------
// disk cache
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModelCache {
    /// provider_id -> (fetched_at_ms, models)
    pub entries: HashMap<String, (u64, Vec<ModelInfo>)>,
}

impl ModelCache {
    pub fn load() -> Self {
        match std::fs::read_to_string(crate::config::model_cache_path()) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => ModelCache::default(),
        }
    }

    pub fn save(&self) {
        if let Some(dir) = crate::config::model_cache_path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string(self) {
            let _ = std::fs::write(crate::config::model_cache_path(), text);
        }
    }

    pub fn get_fresh(&self, provider_id: &str, ttl_ms: u64) -> Option<Vec<ModelInfo>> {
        let (ts, models) = self.entries.get(provider_id)?;
        if crate::util::now_ms().saturating_sub(*ts) < ttl_ms && !models.is_empty() {
            Some(models.clone())
        } else {
            None
        }
    }
}

impl ModelInfo {
    /// Re-derive capability from the id instead of trusting what an older build
    /// wrote into the cache. Detector fixes ("DeepSeek does have an effort
    /// knob") then reach existing installs without forcing a re-fetch.
    pub fn refresh_thinking(&mut self, protocol: Protocol) {
        self.thinking = detect_thinking(&self.id, protocol);
    }
}

/// Merge fetched models with heuristic thinking detection.
pub fn with_thinking(    provider_id: &str,
    protocol: Protocol,
    raw: Vec<crate::agent::provider::RawModel>,
) -> Vec<ModelInfo> {
    raw.into_iter()
        .map(|m| {
            let thinking = detect_thinking(&m.id, protocol);
            ModelInfo {
                id: m.id,
                provider_id: provider_id.to_string(),
                display_name: m.display_name,
                owned_by: m.owned_by,
                context_length: m.context_length,
                thinking,
                source: crate::agent::models::ModelSource::Fetched,
            }
        })
        .collect()
}
