//! Token / call accounting for the whole app.

use crate::agent::types::Usage;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Default)]
struct MeterState {
    calls: u64,
    errors: u64,
    input_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
    cache_read_tokens: u64,
    by_model: HashMap<String, Usage>,
    by_session: HashMap<String, Usage>,
    started_ms: u64,
}

pub struct UsageMeter {
    state: std::sync::Mutex<MeterState>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelUsage {
    pub model: String,
    pub usage: Usage,
}

#[derive(Clone, Debug, Serialize)]
pub struct UsageReport {
    pub calls: u64,
    pub errors: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_read_tokens: u64,
    pub elapsed_secs: u64,
    pub by_model: Vec<ModelUsage>,
    pub by_session: Vec<ModelUsage>,
}

impl Default for UsageMeter {
    fn default() -> Self {
        UsageMeter::new()
    }
}

impl UsageMeter {
    pub fn new() -> Self {
        UsageMeter {
            state: std::sync::Mutex::new(MeterState {
                started_ms: crate::util::now_ms(),
                ..Default::default()
            }),
        }
    }

    pub fn record(&self, model: &str, session: &str, usage: &Usage) {
        let mut st = crate::util::lock(&self.state);
        st.calls += 1;
        st.input_tokens += usage.input_tokens;
        st.output_tokens += usage.output_tokens;
        st.reasoning_tokens += usage.reasoning_tokens;
        st.cache_read_tokens += usage.cache_read_tokens;
        st.by_model
            .entry(model.to_string())
            .or_default()
            .merge(usage);
        st.by_session
            .entry(session.to_string())
            .or_default()
            .merge(usage);
    }

    pub fn record_error(&self) {
        let mut st = crate::util::lock(&self.state);
        st.errors += 1;
    }

    pub fn reset(&self) {
        let mut st = crate::util::lock(&self.state);
        *st = MeterState {
            started_ms: crate::util::now_ms(),
            ..Default::default()
        };
    }

    pub fn report(&self) -> UsageReport {
        let st = crate::util::lock(&self.state);
        let mut by_model: Vec<ModelUsage> = st
            .by_model
            .iter()
            .map(|(model, usage)| ModelUsage {
                model: model.clone(),
                usage: usage.clone(),
            })
            .collect();
        by_model.sort_by(|a, b| b.usage.output_tokens.cmp(&a.usage.output_tokens));
        let mut by_session: Vec<ModelUsage> = st
            .by_session
            .iter()
            .map(|(model, usage)| ModelUsage {
                model: model.clone(),
                usage: usage.clone(),
            })
            .collect();
        by_session.sort_by(|a, b| b.usage.output_tokens.cmp(&a.usage.output_tokens));
        UsageReport {
            calls: st.calls,
            errors: st.errors,
            input_tokens: st.input_tokens,
            output_tokens: st.output_tokens,
            reasoning_tokens: st.reasoning_tokens,
            cache_read_tokens: st.cache_read_tokens,
            elapsed_secs: (crate::util::now_ms().saturating_sub(st.started_ms)) / 1000,
            by_model,
            by_session,
        }
    }
}
