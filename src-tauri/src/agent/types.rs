//! IRIXEN Agent — core type definitions shared across the engine.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// One piece of message content. A message carries an ordered list of parts
/// so tool calls / tool results / text can coexist exactly like the wire
/// formats of the big-three APIs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ToolCall { id: String, name: String, arguments: String },
    ToolResult { call_id: String, name: String, output: String, is_error: bool },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<ContentPart>,
}

impl Message {
    pub fn system(text: impl Into<String>) -> Self {
        Message { role: Role::System, parts: vec![ContentPart::Text { text: text.into() }] }
    }
    pub fn user(text: impl Into<String>) -> Self {
        Message { role: Role::User, parts: vec![ContentPart::Text { text: text.into() }] }
    }
    pub fn assistant(text: impl Into<String>) -> Self {
        Message { role: Role::Assistant, parts: vec![ContentPart::Text { text: text.into() }] }
    }
    pub fn text_of(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    pub fn has_tool_calls(&self) -> bool {
        self.parts.iter().any(|p| matches!(p, ContentPart::ToolCall { .. }))
    }
    pub fn has_tool_results(&self) -> bool {
        self.parts.iter().any(|p| matches!(p, ContentPart::ToolResult { .. }))
    }
}

/// Token accounting. `reasoning` is the thinking-channel usage reported by
/// OpenAI (reasoning_tokens), Anthropic (thinking output) and Gemini
/// (thoughtsTokenCount).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl Usage {
    pub fn merge(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.reasoning_tokens += other.reasoning_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
    }
}

/// How hard the model should think. `Auto` means "use the model's default".
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Auto,
    Off,
    Minimal,
    Low,
    Medium,
    High,
}

impl Default for Effort {
    fn default() -> Self {
        Effort::Auto
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<serde_json::Value>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub effort: Effort,
    pub stream: bool,
}

/// Normalized stream events emitted by every provider implementation.
#[derive(Clone, Debug)]
pub enum StreamEvent {
    Text(String),
    Reasoning(String),
    ToolCallStart { index: usize, id: String, name: String },
    ToolCallArgs { index: usize, delta: String },
    Usage(Usage),
}

/// An assembled tool call (after streaming deltas are joined).
#[derive(Clone, Debug)]
pub struct AssembledToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug)]
pub enum ProviderError {
    Http(String),
    Api { status: u16, body: String },
    Parse(String),
    Unsupported(String),
    /// Already human-readable, produced by `net::explain_*`. Printed verbatim.
    Message(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::Http(e) => write!(f, "network error: {e}"),
            ProviderError::Api { status, body } => {
                // Character-safe: provider error bodies are often Chinese, and
                // slicing at a byte offset would panic mid-codepoint (which, with
                // `panic = "abort"` in release, kills the whole app).
                write!(f, "api error {status}: {}", crate::util::cap_chars(body, 600))
            }
            ProviderError::Parse(e) => write!(f, "parse error: {e}"),
            ProviderError::Unsupported(e) => write!(f, "unsupported: {e}"),
            ProviderError::Message(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ProviderError {}

pub type StreamResult = std::pin::Pin<
    Box<dyn futures_util::Stream<Item = Result<StreamEvent, ProviderError>> + Send>,
>;

/// Events pushed to the UI while the agent works.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEvent {
    Status { session: String, text: String },
    Delta { session: String, text: String },
    Reasoning { session: String, text: String },
    ToolCall { session: String, id: String, name: String, arguments: String },
    ToolResult { session: String, id: String, name: String, output: String, is_error: bool },
    Usage { session: String, usage: Usage },
    Compressed { session: String, report: CompressionReport },
    Done { session: String, text: String },
    Error { session: String, text: String },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CompressionReport {
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub tool_results_elided: usize,
    pub messages_dropped: usize,
    pub digest: Option<String>,
}

/// A file the user attached to the conversation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attachment {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
}
