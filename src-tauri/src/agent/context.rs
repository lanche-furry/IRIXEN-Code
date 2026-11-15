//! Context compression — the token-efficiency core of IRIXEN Agent.
//!
//! Three escalating stages keep long sessions inside the model's context
//! without "dumbing down" the agent:
//!
//! 1. **Tool-result elision** — results older than `tool_result_age`
//!    messages collapse to a one-line placeholder. Raw dumps (file reads,
//!    command output) are the #1 context burner and are rarely needed twice.
//! 2. **Sliding window with task anchor** — the original task statement and
//!    the last `keep_recent` messages always survive verbatim, so the agent
//!    never forgets what it was asked to do or where it currently is.
//! 3. **LLM digest** — the dropped middle is summarized into a compact state
//!    digest (goal / decisions / files touched / pending steps) which is
//!    re-inserted as the first user message. Falls back to a heuristic
//!    extract when no API is reachable.

use crate::agent::provider::LlmProvider;
use crate::agent::types::*;
use crate::config::CompressionConfig;

pub struct Compressor {
    pub cfg: CompressionConfig,
}

impl Compressor {
    pub fn new(cfg: CompressionConfig) -> Self {
        Compressor { cfg }
    }

    /// Rough token estimate: ~4 chars per token plus per-message overhead.
    pub fn estimate_tokens(messages: &[Message]) -> u64 {
        let mut total = 0u64;
        for m in messages {
            total += 4; // role/framing overhead
            for p in &m.parts {
                match p {
                    ContentPart::Text { text } => total += (text.chars().count() as u64) / 4 + 1,
                    ContentPart::ToolCall { arguments, name, .. } => {
                        total += (arguments.chars().count() as u64) / 4 + 2;
                        total += (name.chars().count() as u64) / 4 + 2;
                    }
                    ContentPart::ToolResult { output, .. } => {
                        total += (output.chars().count() as u64) / 4 + 2
                    }
                    // Base64 length says nothing about how many tokens an image
                    // costs: providers rescale it to a fixed tile budget, so a
                    // flat estimate is closer than counting characters.
                    ContentPart::Image { .. } => total += 1_000,
                }
            }
        }
        total
    }

    pub fn budget(&self) -> u64 {
        (self.cfg.max_context_tokens as f64 * self.cfg.threshold) as u64
    }

    pub fn should_compress(&self, messages: &[Message]) -> bool {
        self.cfg.enabled && Self::estimate_tokens(messages) >= self.budget()
    }

    /// Stage 1: elide stale tool results. Returns how many were collapsed.
    pub fn trim_tool_results(&self, messages: &mut [Message]) -> usize {
        let mut elided = 0usize;
        let total = messages.len();
        for (idx, m) in messages.iter_mut().enumerate() {
            if !m.has_tool_results() {
                continue;
            }
            // Age measured from the end of the conversation.
            let age = total.saturating_sub(idx + 1);
            if age < self.cfg.tool_result_age {
                continue;
            }
            for p in &mut m.parts {
                if let ContentPart::ToolResult { output, name, .. } = p {
                    if output.chars().count() > 64 {
                        let n = output.chars().count();
                        *output = format!("[已折叠：{name} 的 {n} 字符历史结果]");
                        elided += 1;
                    }
                }
            }
        }
        elided
    }

    /// Stage 2: split into (kept, dropped-middle). The first user message
    /// (the task anchor) and the last `keep_recent` messages are kept.
    pub fn split_window(&self, messages: &[Message]) -> (Vec<Message>, Vec<Message>) {
        let first_user = messages
            .iter()
            .position(|m| m.role == Role::User)
            .unwrap_or(0);
        let keep_from = messages
            .len()
            .saturating_sub(self.cfg.keep_recent)
            .max(first_user + 1);
        if keep_from >= messages.len() {
            return (messages.to_vec(), Vec::new());
        }
        let mut keep = vec![messages[first_user].clone()];
        keep.extend_from_slice(&messages[keep_from..]);
        let middle = messages[first_user + 1..keep_from].to_vec();
        (keep, middle)
    }

    /// Stage 3: summarize the dropped middle with a cheap model call.
    pub async fn digest(
        &self,
        provider: &dyn LlmProvider,
        model: &str,
        middle: &[Message],
    ) -> Option<String> {
        if middle.is_empty() {
            return None;
        }
        let mut transcript = String::new();
        for m in middle {
            let role = match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };
            let text = m.text_of();
            let calls: Vec<String> = m
                .parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::ToolCall { name, .. } => Some(format!("call:{name}")),
                    ContentPart::ToolResult { name, .. } => Some(format!("result:{name}")),
                    _ => None,
                })
                .collect();
            if !text.is_empty() {
                transcript.push_str(&format!("{role}: {text}\n"));
            }
            if !calls.is_empty() {
                transcript.push_str(&format!("{role}: [{}]\n", calls.join(", ")));
            }
        }
        let digest_prompt = format!(
            "Summarize the conversation below into a compact state digest of at most {} tokens.\n\
             Cover, in this order:\n\
             1. Goal — what the user ultimately wants\n\
             2. Decisions — choices made and why (one line each)\n\
             3. Files — paths touched and what changed in each\n\
             4. State — what currently works / is broken\n\
             5. Pending — the next concrete steps\n\
             Be telegraphic. No preamble, no markdown fences.\n\n\
             --- TRANSCRIPT ---\n{}",
            self.cfg.digest_max_tokens,
            crate::util::cap_chars(&transcript, 24_000)
        );
        let req = ChatRequest {
            model: model.to_string(),
            messages: vec![
                Message::system("You are a precise conversation summarizer for a coding agent."),
                Message::user(digest_prompt),
            ],
            tools: Vec::new(),
            temperature: Some(0.2),
            max_tokens: Some(self.cfg.digest_max_tokens),
            effort: Effort::Low,
            stream: false,
        };
        let mut stream = provider.chat(&req).await.ok()?;
        let mut out = String::new();
        use futures_util::StreamExt;
        while let Some(item) = stream.next().await {
            match item {
                Ok(StreamEvent::Text(t)) => out.push_str(&t),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let digest = out.trim().to_string();
        if digest.is_empty() {
            Some(heuristic_digest(middle))
        } else {
            Some(digest)
        }
    }
}

/// No-API fallback: extract the most recent user asks and tool usage.
fn heuristic_digest(middle: &[Message]) -> String {
    let mut asks: Vec<String> = Vec::new();
    let mut touched: Vec<String> = Vec::new();
    for m in middle {
        match m.role {
            Role::User => {
                let t = crate::util::one_line(&m.text_of(), 160);
                if !t.is_empty() {
                    asks.push(t);
                }
            }
            Role::Assistant | Role::Tool => {
                for p in &m.parts {
                    match p {
                        ContentPart::ToolCall { name, .. } => touched.push(name.clone()),
                        ContentPart::ToolResult { name, .. } => touched.push(name.clone()),
                        _ => {}
                    }
                }
            }
            Role::System => {}
        }
    }
    asks.truncate(6);
    let mut out = String::from("[启发式摘要]\n近期请求:\n");
    for a in asks {
        out.push_str(&format!("- {a}\n"));
    }
    if !touched.is_empty() {
        touched.sort();
        touched.dedup();
        out.push_str(&format!("使用过的工具: {}\n", touched.join(", ")));
    }
    out
}
