//! Persistent application configuration (JSON in the OS config dir).

use crate::agent::types::Effort;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const APP_DIR_NAME: &str = "IRIXEN Code";
pub const CONFIG_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// OpenAI chat/completions — also covers DeepSeek, Kimi, Qwen/DashScope,
    /// GLM, SiliconFlow, OpenRouter and every other OpenAI-compatible vendor.
    Openai,
    /// Anthropic messages API (also covers Bedrock/Vertex-style gateways).
    Anthropic,
    /// Google Gemini generateContent (streamGenerateContent).
    Gemini,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Path appended to base_url for the model-list endpoint (usually /models).
    #[serde(default = "default_models_path")]
    pub models_path: String,
    /// Extra HTTP headers (some vendors need them, e.g. HTTP-Referer).
    #[serde(default)]
    pub extra_headers: Vec<(String, String)>,
    /// Extra JSON body fields merged into every request (vendor quirks such as
    /// Qwen's `enable_thinking` live here).
    #[serde(default)]
    pub extra_body: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub note: String,
}

fn default_true() -> bool {
    true
}
fn default_models_path() -> String {
    "/models".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActiveSelection {
    pub provider_id: String,
    pub model: String,
    #[serde(default)]
    pub effort: Effort,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompressionConfig {
    /// Hard context budget (tokens) before the compressor kicks in.
    #[serde(default = "default_max_context")]
    pub max_context_tokens: u64,
    /// Fraction of the budget that triggers compression.
    #[serde(default = "default_threshold")]
    pub threshold: f64,
    /// Tool results older than this many messages get elided.
    #[serde(default = "default_tool_result_age")]
    pub tool_result_age: usize,
    /// Per-tool-result character cap when kept verbatim.
    #[serde(default = "default_tool_result_cap")]
    pub tool_result_cap: usize,
    /// Recent messages always kept verbatim.
    #[serde(default = "default_keep_recent")]
    pub keep_recent: usize,
    /// Max tokens for the summarization digest.
    #[serde(default = "default_digest_tokens")]
    pub digest_max_tokens: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_max_context() -> u64 {
    128_000
}
fn default_threshold() -> f64 {
    0.75
}
fn default_tool_result_age() -> usize {
    6
}
fn default_tool_result_cap() -> usize {
    4_000
}
fn default_keep_recent() -> usize {
    10
}
fn default_digest_tokens() -> u32 {
    800
}

impl Default for CompressionConfig {
    fn default() -> Self {
        CompressionConfig {
            max_context_tokens: default_max_context(),
            threshold: default_threshold(),
            tool_result_age: default_tool_result_age(),
            tool_result_cap: default_tool_result_cap(),
            keep_recent: default_keep_recent(),
            digest_max_tokens: default_digest_tokens(),
            enabled: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
    #[serde(default)]
    pub compression: CompressionConfig,
    /// Auto-inject skills whose keywords match the prompt.
    #[serde(default = "default_true")]
    pub auto_skills: bool,
}

fn default_max_iterations() -> u32 {
    24
}

pub fn default_system_prompt() -> String {
    [
        "You are IRIXEN Agent, a precise coding agent embedded in IRIXEN Code.",
        "",
        "Operating principles:",
        "- Act, then report. Prefer tool calls over speculation; never invent file contents.",
        "- Never dump whole files. Use targeted line-range reads and minimal diffs.",
        "- Before editing, read enough surrounding context to make the edit unambiguous.",
        "- Keep outputs terse: a short paragraph OR a unified diff, not walls of text.",
        "- Verify after editing: re-read the changed region or run the relevant check.",
        "- Respect the workspace: no writes outside the project root unless asked.",
        "- Match the repo's existing conventions (style, structure, dependencies).",
        "- When a task is ambiguous, state your assumption in one line and proceed.",
        "- Report exact file paths and line numbers so changes are easy to review.",
    ]
    .join("\n")
}

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            max_iterations: default_max_iterations(),
            temperature: None,
            system_prompt: default_system_prompt(),
            compression: CompressionConfig::default(),
            auto_skills: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub auto_start: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillsConfig {
    /// Skill ids the user has switched on.
    #[serde(default)]
    pub enabled: Vec<String>,
    #[serde(default = "default_true")]
    pub auto_match: bool,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        SkillsConfig { enabled: Vec::new(), auto_match: true }
    }
}

/// Extension host + marketplace settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtensionsConfig {
    /// Run real VS Code extensions in a Node child process. Off means only the
    /// declarative parts (themes, snippets, languages) are used.
    #[serde(default = "default_true")]
    pub node_host: bool,
    /// Explicit `node` binary. Empty = auto (bundled → PATH → vendor dirs).
    #[serde(default)]
    pub node_path: String,
    /// Seconds to wait before auto-starting extensions with `onStartupFinished`.
    /// A small delay keeps the first paint from competing with node boot.
    #[serde(default = "default_ext_delay")]
    pub autostart_delay_ms: u64,
    /// Start every installed extension at launch instead of only the ones whose
    /// `activationEvents` say `*` / `onStartupFinished`. Default on: without it
    /// a package like Gradle (which declares `workspaceContains:*`) never runs,
    /// and half the extensions look installed-but-dead.
    #[serde(default = "default_true")]
    pub auto_activate_all: bool,
    /// Gap between two extension hosts coming up. Staggered so N node processes
    /// do not compete with the window's first frames for the disk.
    #[serde(default = "default_ext_stagger")]
    pub autostart_stagger_ms: u64,
}

fn default_ext_stagger() -> u64 {
    260
}

fn default_ext_delay() -> u64 {
    1500
}

impl Default for ExtensionsConfig {
    fn default() -> Self {
        ExtensionsConfig {
            node_host: true,
            node_path: String::new(),
            autostart_delay_ms: default_ext_delay(),
            auto_activate_all: true,
            autostart_stagger_ms: default_ext_stagger(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomTheme {
    pub id: String,
    pub name: String,
    /// CSS custom property overrides, e.g. `--bg-base` → `#0b0e14`.
    pub colors: Vec<(String, String)>,
}

/// How one language is executed.
///
/// Both fields are templates: `{file}` (absolute path), `{stem}`, `{name}`
/// (file name with extension) and `{dir}` are expanded at run time. An empty
/// `command` means "use the built-in default for this language".
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RunnerConfig {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: String,
    /// Extra environment variables, e.g. `PYTHONPATH`.
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BridgeUpstream {
    /// Reverse-proxy a web AI app. The client's model name picks the source.
    /// `claude_web` is the pre-routing value written by older builds.
    #[serde(alias = "claude_web")]
    Web,
    /// Debug only: an API provider configured for the agent (DeepSeek etc.).
    /// Web2API is not a gateway for those — the agent talks to them directly.
    Provider,
}

/// One browser-based AI service. The endpoint / body / extraction fields are
/// templates on purpose: these are private browser APIs that change without
/// notice, so a source must be repairable without rebuilding the app.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebSource {
    pub id: String,
    pub label: String,
    /// `browser` sends the request from inside a logged-in WebView on the
    /// vendor's own origin (real cookies, UA, CSRF/PoW tokens). `http` is the
    /// legacy raw-template request with a manually pasted session key.
    #[serde(default = "default_web_mode")]
    pub mode: String,
    /// Where the browser session window starts, e.g. https://chatgpt.com/.
    #[serde(default)]
    pub home_url: String,
    /// Cookie value, bearer token, or whatever `auth_header` expects.
    pub session_key: String,
    /// Optional organisation id; empty = resolve through `org_endpoint`.
    pub org_id: String,
    /// Model id sent to the web app.
    pub model: String,
    /// URL template: `{org}`, `{uuid}`, `{model}`, `{session}`.
    pub endpoint: String,
    /// One or more `name: value` header templates, separated by newlines.
    pub auth_header: String,
    pub content_type: String,
    /// Raw request body. `{prompt}` is JSON-escaped text without quotes,
    /// `{prompt_json}` includes quotes, `{req}` is a percent-encoded Gemini
    /// style `f.req` payload, `{model}/{uuid}/{org}/{session}` are raw.
    pub body_template: String,
    /// Dotted path into each JSON event, or `auto` for the longest string.
    pub text_path: String,
    /// Optional `dotted.path=value` guard an event must satisfy.
    pub text_guard: String,
    /// Web apps that re-send the whole answer on every event (ChatGPT/Gemini).
    pub cumulative: bool,
    /// Optional GET endpoint used to resolve `{org}`.
    pub org_endpoint: String,
    /// Dotted path into the org response.
    pub org_path: String,
    /// `ui` drives the page's own composer (immune to sentinel/PoW guards on
    /// private endpoints); `api` posts to the private JSON endpoint. Browser
    /// sessions default to `ui` because the raw endpoints now answer 403 /
    /// "Unusual activity has been detected" even from a logged-in page.
    #[serde(default = "default_web_driver")]
    pub driver: String,
    /// How much of IRIXEN's message array is typed into the vendor composer.
    /// `incremental` (default) sends only the newest user turn: a logged-in
    /// page is a stateful conversation that already holds its persona and
    /// history, so re-injecting the whole system prompt + transcript on every
    /// request just duplicates context and clutters the page. `full` restores
    /// the old flatten-everything behaviour. Stateless transports (`http`
    /// mode or the `api` driver) always need `full` regardless of this field.
    #[serde(default = "default_web_prompt_mode")]
    pub prompt_mode: String,
    /// Optional JS body for the UI driver. Empty = the built-in generic
    /// composer/answer scraper. Same repair-without-rebuild idea as the
    /// request templates above.
    #[serde(default)]
    pub ui_script: String,
    /// Same-origin GET used to detect a logged-in session and read the
    /// account label. Empty = fall back to a DOM/cookie heuristic.
    #[serde(default)]
    pub login_endpoint: String,
    /// Open this source's browser session (hidden) when the endpoint starts or
    /// when one of its models is selected. Users pick which reverse proxies
    /// are allowed to boot in the Web2API panel.
    #[serde(default = "default_true")]
    pub auto_open: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelRoute {
    /// Case-insensitive prefix of the client's `model` field.
    pub prefix: String,
    pub source_id: String,
}

fn default_bridge_port() -> u16 {
    8791
}
fn default_claude_model() -> String {
    "claude-sonnet-4-5".to_string()
}
fn default_web_source_id() -> String {
    "claude".to_string()
}
fn default_bridge_upstream() -> BridgeUpstream {
    BridgeUpstream::Web
}

fn default_web_mode() -> String {
    "browser".into()
}
fn default_web_driver() -> String {
    "ui".into()
}
fn default_web_prompt_mode() -> String {
    "incremental".into()
}

fn claude_source() -> WebSource {
    WebSource {
        id: "claude".into(),
        label: "Claude Web".into(),
        mode: default_web_mode(),
        home_url: "https://claude.ai/".into(),
        session_key: String::new(),
        org_id: String::new(),
        model: default_claude_model(),
        endpoint: "https://claude.ai/api/organizations/{org}/chat_conversations/{uuid}/completion".into(),
        auth_header: "cookie: sessionKey={session}".into(),
        content_type: "application/json".into(),
        body_template: r#"{"prompt":"{prompt}","timezone":"Asia/Shanghai","model":"{model}","attachments":[],"files":[]}"#.into(),
        text_path: "completion".into(),
        text_guard: String::new(),
        cumulative: false,
        org_endpoint: "https://claude.ai/api/organizations".into(),
        org_path: "0.uuid".into(),
        driver: default_web_driver(),
        prompt_mode: default_web_prompt_mode(),
        ui_script: String::new(),
        login_endpoint: "https://claude.ai/api/organizations".into(),
        auto_open: true,
    }
}
fn chatgpt_source() -> WebSource {
    WebSource {
        id: "chatgpt".into(),
        label: "ChatGPT Web".into(),
        mode: default_web_mode(),
        home_url: "https://chatgpt.com/".into(),
        session_key: String::new(),
        org_id: String::new(),
        model: "gpt-4o".into(),
        endpoint: "https://chatgpt.com/backend-api/conversation".into(),
        auth_header: "authorization: Bearer {session}\ncookie: {session}".into(),
        content_type: "application/json".into(),
        body_template: r#"{"action":"next","messages":[{"id":"{uuid}","author":{"role":"user"},"content":{"content_type":"text","parts":["{prompt}"]},"metadata":{}}],"conversation_id":null,"parent_message_id":"{uuid}","model":"{model}","timezone_offset_min":-480,"suggestions":[],"history_and_training_disabled":false,"conversation_mode":{"kind":"primary_assistant"},"force_paragen":false,"force_rate_limit":false}"#.into(),
        text_path: "message.content.parts.0".into(),
        text_guard: "message.author.role=assistant".into(),
        cumulative: true,
        org_endpoint: String::new(),
        org_path: String::new(),
        driver: default_web_driver(),
        prompt_mode: default_web_prompt_mode(),
        ui_script: String::new(),
        // `/api/auth/session` answers `{}` when the cookie jar is empty, and a
        // user object when logged in — a cheap, same-origin login probe.
        login_endpoint: "https://chatgpt.com/api/auth/session".into(),
        auto_open: true,
    }
}
fn gemini_source() -> WebSource {
    WebSource {
        id: "gemini".into(),
        label: "Gemini Web".into(),
        mode: default_web_mode(),
        home_url: "https://gemini.google.com/".into(),
        session_key: String::new(),
        org_id: String::new(),
        model: "gemini-2.5-pro".into(),
        endpoint: "https://gemini.google.com/_/BardChatUi/data/assistant.lamda.BardFrontendService/StreamGenerate?hl=zh-CN&rt=c".into(),
        auth_header: "cookie: {session}".into(),
        content_type: "application/x-www-form-urlencoded;charset=UTF-8".into(),
        body_template: "f.req={req}&at={session}".into(),
        text_path: "auto".into(),
        text_guard: String::new(),
        cumulative: true,
        org_endpoint: String::new(),
        org_path: String::new(),
        driver: default_web_driver(),
        prompt_mode: default_web_prompt_mode(),
        ui_script: String::new(),
        // Gemini has no small JSON identity endpoint; the DOM heuristic in
        // websession::probe_login covers it (composer present, no login wall).
        login_endpoint: String::new(),
        auto_open: true,
    }
}

pub fn default_web_sources() -> Vec<WebSource> {
    vec![claude_source(), chatgpt_source(), gemini_source()]
}

/// Scheme + host of an endpoint, used as a login-page fallback for custom
/// sources that have no explicit `home_url`.
fn origin_of(endpoint: &str) -> Option<String> {
    let rest = endpoint
        .strip_prefix("https://")
        .or_else(|| endpoint.strip_prefix("http://"))?;
    let host = rest.split('/').next()?;
    if host.is_empty() {
        None
    } else {
        Some(format!("https://{host}/"))
    }
}

pub fn default_model_routes() -> Vec<ModelRoute> {
    let route = |prefix: &str, id: &str| ModelRoute { prefix: prefix.into(), source_id: id.into() };
    vec![
        route("claude", "claude"),
        route("gpt-", "chatgpt"),
        route("gpt", "chatgpt"),
        route("o1", "chatgpt"),
        route("o3", "chatgpt"),
        route("o4", "chatgpt"),
        route("chatgpt", "chatgpt"),
        route("gemini", "gemini"),
        route("bard", "gemini"),
    ]
}

/// Web2API — a local OpenAI/Anthropic-compatible endpoint over web sessions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeConfig {
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default = "default_bridge_port")]
    pub port: u16,
    /// Local bearer token clients must send. Empty = no auth (loopback only).
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_bridge_upstream")]
    pub upstream: BridgeUpstream,
    /// Provider used in debug mode only. Empty = the ready default.
    #[serde(default)]
    pub provider_id: String,
    /// Fallback model name when a client sends none.
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub sources: Vec<WebSource>,
    #[serde(default)]
    pub routes: Vec<ModelRoute>,
    #[serde(default)]
    pub default_source: String,
    /// When the composer picks a Web2API model, start the endpoint and open the
    /// matching browser session in the background without any manual step.
    #[serde(default = "default_true")]
    pub auto_ensure: bool,
    /// Bumped once when model routing replaced the single Claude upstream.
    /// Old files have no marker, so `migrate()` can still flip them to web mode
    /// even though serde has already filled the new defaults.
    #[serde(default)]
    pub routing_version: u32,
    /// Legacy fields, kept so an older config still deserialises. `migrate()`
    /// folds them into `sources` and leaves them unused afterwards.
    #[serde(default)]
    pub claude_session_key: String,
    #[serde(default)]
    pub claude_org_id: String,
    #[serde(default = "default_claude_model")]
    pub claude_model: String,
}

impl BridgeConfig {
    /// Upgrade a pre-routing config: build the three web presets, carry the old
    /// Claude session over, and switch the default from the API provider to web
    /// routing. Runs once — an explicit `provider` choice made afterwards is
    /// preserved because `sources` is already populated.
    pub fn migrate(&mut self) {
        if self.routing_version < 2 {
            if self.sources.is_empty() {
                let mut sources = default_web_sources();
                if let Some(claude) = sources.iter_mut().find(|s| s.id == "claude") {
                    claude.session_key = std::mem::take(&mut self.claude_session_key);
                    claude.org_id = std::mem::take(&mut self.claude_org_id);
                    if !self.claude_model.trim().is_empty() {
                        claude.model = self.claude_model.clone();
                    }
                }
                self.sources = sources;
            }
            self.routes = default_model_routes();
            self.default_source = default_web_source_id();
            self.auto_ensure = true;
            // An old config's `provider` was the old default, not an explicit
            // debug choice. New configs keep whatever the user selected.
            self.upstream = BridgeUpstream::Web;
            self.model = default_claude_model();
            self.routing_version = 2;
        }
        if self.sources.is_empty() {
            self.sources = default_web_sources();
        }
        if self.routes.is_empty() {
            self.routes = default_model_routes();
        }
        if self.default_source.trim().is_empty() {
            self.default_source = default_web_source_id();
        }
        // Sources saved before browser sessions existed have an empty
        // `home_url` (serde default). Fill it from the built-in preset, or
        // derive the origin from the endpoint, so "open site" is usable.
        let presets = default_web_sources();
        for src in &mut self.sources {
            let preset = presets.iter().find(|p| p.id == src.id);
            if src.home_url.trim().is_empty() {
                src.home_url = preset
                    .map(|p| p.home_url.clone())
                    .or_else(|| origin_of(&src.endpoint))
                    .unwrap_or_default();
            }
            // Sources saved before page-driving existed have an empty
            // `driver`; the private JSON endpoints they used are now blocked by
            // the vendors' bot guards, so default them to the UI driver.
            if src.driver.trim().is_empty() {
                src.driver = default_web_driver();
            }
            if src.login_endpoint.trim().is_empty() {
                src.login_endpoint = preset.map(|p| p.login_endpoint.clone()).unwrap_or_default();
            }
        }
    }

    pub fn source(&self, id: &str) -> Option<&WebSource> {
        self.sources.iter().find(|s| s.id == id)
    }
}

impl Default for BridgeConfig {
    fn default() -> Self {
        BridgeConfig {
            auto_start: false,
            port: default_bridge_port(),
            api_key: String::new(),
            upstream: BridgeUpstream::Web,
            provider_id: String::new(),
            model: default_claude_model(),
            sources: default_web_sources(),
            routes: default_model_routes(),
            default_source: default_web_source_id(),
            auto_ensure: true,
            routing_version: 2,
            claude_session_key: String::new(),
            claude_org_id: String::new(),
            claude_model: default_claude_model(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_providers")]
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub active: Option<ActiveSelection>,
    /// Sticky default for the composer's thinking-effort knob. New sessions and
    /// fresh installs start here; an explicit per-session pick still wins.
    #[serde(default)]
    pub effort: Effort,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    pub skills: SkillsConfig,
    #[serde(default)]
    pub custom_themes: Vec<CustomTheme>,
    /// Proxy for outbound API calls. Empty = auto (env → Windows system proxy).
    /// `none` forces a direct connection.
    #[serde(default)]
    pub proxy: String,
    /// Web2API bridge endpoint.
    #[serde(default)]
    pub bridge: BridgeConfig,
    /// Extension host + marketplace.
    #[serde(default)]
    pub extensions: ExtensionsConfig,
    /// Per-file language overrides, keyed by absolute path. Lets the user say
    /// "this `.txt` is actually Python" without renaming the file.
    #[serde(default)]
    pub file_languages: std::collections::BTreeMap<String, String>,
    /// Per-language runner overrides. Missing entries use the built-in default.
    #[serde(default)]
    pub runners: std::collections::BTreeMap<String, RunnerConfig>,
    /// Shell for the integrated terminal. Empty = auto (`pwsh` → `powershell`
    /// → `cmd` on Windows, `$SHELL` elsewhere). Any executable path is accepted
    /// so a non-standard install still works.
    #[serde(default)]
    pub terminal_shell: String,
    /// First-run setup wizard: `false` on a fresh profile, flipped to `true`
    /// once the user finishes (or skips) it. `#[serde(default)]` means older
    /// config files are treated as "not yet completed" without a migration.
    #[serde(default)]
    pub onboarding_completed: bool,
}

fn default_version() -> u32 {
    CONFIG_VERSION
}
fn default_theme() -> String {
    "light".to_string()
}

/// Perceived brightness of `#rrggbb` (0–255), same weighting as the renderer.
fn hex_luma(hex: &str) -> Option<f32> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()? as f32;
    let g = u8::from_str_radix(&h[2..4], 16).ok()? as f32;
    let b = u8::from_str_radix(&h[4..6], 16).ok()? as f32;
    Some((r * 299.0 + g * 587.0 + b * 114.0) / 1000.0)
}
fn default_language() -> String {
    "zh-CN".to_string()
}

/// Reserved id for the built-in provider that points at the local Web2API
/// bridge. Its endpoint and key are derived from `BridgeConfig`, never typed
/// by hand — see `AppConfig::sync_web2api_provider`.
pub const WEB2API_PROVIDER_ID: &str = "web2api";

/// The Web2API provider is assembled from the bridge settings, so a port or
/// key change in the Web2API panel cannot leave the agent pointing at a stale
/// (and 401-ing) endpoint.
pub fn web2api_provider(bridge: &BridgeConfig) -> ProviderConfig {
    ProviderConfig {
        id: WEB2API_PROVIDER_ID.to_string(),
        name: "Web2API".to_string(),
        protocol: Protocol::Openai,
        base_url: format!("http://127.0.0.1:{}/v1", bridge.port),
        api_key: bridge.api_key.clone(),
        // Off until the bridge is actually started; otherwise a fresh install
        // would pick this loopback endpoint (ready by definition) and every
        // first message would die on a closed port.
        enabled: false,
        models_path: default_models_path(),
        extra_headers: Vec::new(),
        extra_body: serde_json::Map::new(),
        note: String::new(),
    }
}

/// Preset vendor list — keys are filled in by the user in Settings.
pub fn default_providers() -> Vec<ProviderConfig> {
    let mk = |id: &str, name: &str, protocol: Protocol, base_url: &str, note: &str| ProviderConfig {
        id: id.to_string(),
        name: name.to_string(),
        protocol,
        base_url: base_url.to_string(),
        api_key: String::new(),
        enabled: true,
        models_path: default_models_path(),
        extra_headers: Vec::new(),
        extra_body: serde_json::Map::new(),
        note: note.to_string(),
    };
    vec![
        web2api_provider(&BridgeConfig::default()),
        mk("openai", "OpenAI", Protocol::Openai, "https://api.openai.com/v1", "GPT-5 / o-series 官方接口"),
        mk("anthropic", "Anthropic", Protocol::Anthropic, "https://api.anthropic.com/v1", "Claude 官方接口"),
        mk("gemini", "Google Gemini", Protocol::Gemini, "https://generativelanguage.googleapis.com/v1beta", "Gemini 官方接口"),
        ProviderConfig {
            enabled: false,
            ..mk("deepseek", "DeepSeek", Protocol::Openai, "https://api.deepseek.com/v1", "deepseek-chat / deepseek-reasoner")
        },
        ProviderConfig {
            enabled: false,
            ..mk("moonshot", "Moonshot Kimi", Protocol::Openai, "https://api.moonshot.cn/v1", "Kimi 长上下文")
        },
        ProviderConfig {
            enabled: false,
            ..mk("zhipu", "智谱 GLM", Protocol::Openai, "https://open.bigmodel.cn/api/paas/v4", "GLM-4.6 系列")
        },
        ProviderConfig {
            enabled: false,
            ..mk("dashscope", "阿里百炼 Qwen", Protocol::Openai, "https://dashscope.aliyuncs.com/compatible-mode/v1", "Qwen3 / QwQ")
        },
        ProviderConfig {
            enabled: false,
            ..mk("siliconflow", "SiliconFlow", Protocol::Openai, "https://api.siliconflow.cn/v1", "聚合众多开源模型")
        },
        ProviderConfig {
            enabled: false,
            ..mk("openrouter", "OpenRouter", Protocol::Openai, "https://openrouter.ai/api/v1", "聚合国际厂商")
        },
        ProviderConfig {
            enabled: false,
            ..mk("nvidia", "NVIDIA NIM", Protocol::Openai, "https://integrate.api.nvidia.com/v1", "NVIDIA 托管 NIM / 开源模型")
        },
        ProviderConfig {
            enabled: false,
            ..mk("custom", "自定义 (OpenAI 兼容)", Protocol::Openai, "", "任何 OpenAI 兼容端点")
        },
    ]
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            version: CONFIG_VERSION,
            workspace: None,
            theme: default_theme(),
            language: default_language(),
            providers: default_providers(),
            active: None,
            effort: Effort::Auto,
            agent: AgentConfig::default(),
            mcp_servers: Vec::new(),
            skills: SkillsConfig::default(),
            custom_themes: Vec::new(),
            proxy: String::new(),
            bridge: BridgeConfig::default(),
            extensions: ExtensionsConfig::default(),
            file_languages: std::collections::BTreeMap::new(),
            runners: std::collections::BTreeMap::new(),
            terminal_shell: String::new(),
            onboarding_completed: false,
        }
    }
}

/// Where the app keeps config.json / sessions / extensions / skills.
///
/// `IRIXEN_HOME` overrides the OS location verbatim. Test and automation runs
/// must set it (see `scripts/dev/cdp-check.cjs`): a plain release build and a
/// probe instance otherwise share `%APPDATA%\IRIXEN Code`, so a test that
/// finishes the first-run wizard silently marks the *installed* app as set up
/// and its wizard never comes back.
pub fn base_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("IRIXEN_HOME") {
        let p = PathBuf::from(home);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
}

pub fn config_path() -> PathBuf {
    base_dir().join("config.json")
}

/// Empty file the installer drops inside its *own* install directory on a fresh
/// install (see `installer.nsh`). Named here so the two sides cannot drift.
pub const FIRST_RUN_MARKER: &str = "first-run.marker";

/// Reset the profile when this launch is the first one after a fresh install.
///
/// The installer used to delete `%APPDATA%\IRIXEN Code\config.json` itself, and
/// generic antivirus heuristics read exactly that as a hijacker: Avira flagged
/// the signed Beta 0.2 installer as `TR/Hijacker.Gen` while the other 27 engines
/// were clean. An application managing its own profile is unremarkable, so the
/// work moved here — the installer now only writes an empty marker next to its
/// own binary, inside a directory it is already allowed to write.
///
/// Returns true when the profile was reset, so the caller can log it.
pub fn adopt_fresh_install() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    if !consume_marker(dir) {
        return false;
    }
    archive_profile();
    true
}

/// `dir` is the directory holding the running binary: the install directory for
/// a packaged build, the target directory for a dev build (which never has a
/// marker). Deleting it first means a failed archive cannot reset on every boot.
fn consume_marker(dir: &Path) -> bool {
    std::fs::remove_file(dir.join(FIRST_RUN_MARKER)).is_ok()
}

/// Keep the old profile as `config.json.preinstall` — the same name the
/// installer used to write — and drop the live copy so the wizard runs again.
fn archive_profile() {
    let dir = base_dir();
    let live = dir.join("config.json");
    if live.exists() {
        let _ = std::fs::copy(&live, dir.join("config.json.preinstall"));
        let _ = std::fs::remove_file(&live);
    }
    // Cached model lists describe the old provider set; without a profile they
    // are meaningless and would be re-fetched anyway.
    let _ = std::fs::remove_file(dir.join("model-cache.json"));
}

pub fn skills_dir() -> PathBuf {
    base_dir().join("skills")
}

pub fn extensions_dir() -> PathBuf {
    base_dir().join("extensions")
}

pub fn sessions_dir() -> PathBuf {
    base_dir().join("sessions")
}

pub fn model_cache_path() -> PathBuf {
    base_dir().join("model-cache.json")
}

impl AppConfig {
    /// The concrete palette this theme resolves to: `"light"` or `"dark"`.
    ///
    /// Mirrors `baseOf()` in `src/ui/theme.ts`. Kept next to the config because
    /// it is the only place that can read custom themes before the renderer
    /// exists (used to seed `window.__IRIXEN_THEME`).
    #[allow(dead_code)]
    pub fn theme_base(&self) -> &'static str {
        if self.theme == "dark" {
            return "dark";
        }
        if let Some(key) = self.theme.strip_prefix("custom:") {
            if let Some(t) = self.custom_themes.iter().find(|t| t.id == key) {
                let bg = t
                    .colors
                    .iter()
                    .find(|(k, _)| k == "--bg-0")
                    .or_else(|| t.colors.iter().find(|(k, _)| k == "--bg-1"));
                if let Some((_, value)) = bg {
                    if let Some(luma) = hex_luma(value) {
                        return if luma > 150.0 { "light" } else { "dark" };
                    }
                }
            }
        }
        "light"
    }

    /// Rewrite stored paths into the ordinary (non-verbatim) Windows form.
    ///
    /// `canonicalize()` used to be the only way the workspace was recorded, so
    /// existing configs still carry `\\?\E:\dir`. Verbatim paths do not accept
    /// `/` separators — exactly what the explorer builds — so a Chinese folder
    /// opened from an old config could neither list nor open its children.
    pub fn normalize_paths(&mut self) {
        if let Some(ws) = self.workspace.as_mut() {
            *ws = crate::util::plain_path(ws);
        }
        if !self.file_languages.is_empty() {
            let fixed: std::collections::BTreeMap<String, String> = self
                .file_languages
                .iter()
                .map(|(k, v)| (crate::util::plain_path(k), v.clone()))
                .collect();
            self.file_languages = fixed;
        }
    }

    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<AppConfig>(&text) {
                Ok(mut cfg) => {
                    cfg.version = CONFIG_VERSION;
                    cfg.bridge.migrate();
                    cfg.normalize_paths();
                    // Merge in any new presets the user hasn't seen yet.
                    for preset in default_providers() {
                        if !cfg.providers.iter().any(|p| p.id == preset.id) {
                            cfg.providers.push(preset);
                        }
                    }
                    cfg.sync_web2api_provider();
                    cfg.sanitize_active();
                    cfg
                }
                Err(e) => {
                    eprintln!("[irixen] config parse error: {e}; using defaults");
                    AppConfig::default()
                }
            },
            Err(_) => AppConfig::default(),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let dir = base_dir();
        std::fs::create_dir_all(&dir)?;
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(config_path(), text)?;
        Ok(())
    }

    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// Keep the built-in Web2API provider in step with the bridge panel.
    /// The port and bearer token are owned by `bridge`; only `enabled` is the
    /// user's decision. Called on load and on every config save so a stale
    /// key can never produce the "invalid api key" 401 the agent used to hit.
    pub fn sync_web2api_provider(&mut self) {
        let preset = web2api_provider(&self.bridge);
        if let Some(p) = self
            .providers
            .iter_mut()
            .find(|p| p.id == WEB2API_PROVIDER_ID)
        {
            p.base_url = preset.base_url;
            p.api_key = preset.api_key;
            p.protocol = Protocol::Openai;
            p.models_path = preset.models_path;
        } else {
            self.providers.insert(0, preset);
        }
    }

    /// A sticky `active` pointing at a provider that was switched off (or has
    /// no key) makes the next boot resurrect its model name in the picker.
    /// Drop it so the UI asks the user to configure a model instead.
    pub fn sanitize_active(&mut self) {
        let valid = self
            .active
            .as_ref()
            .and_then(|a| self.provider(&a.provider_id))
            .map(|p| {
                p.enabled
                    && !p.base_url.trim().is_empty()
                    && (!p.api_key.trim().is_empty() || is_loopback(&p.base_url))
            })
            .unwrap_or(false);
        if !valid {
            self.active = None;
        }
    }

    /// Enable/disable the built-in provider and persist, used by the bridge
    /// start/stop commands so the agent's model list follows the bridge.
    pub fn set_web2api_enabled(&mut self, on: bool) {
        self.sync_web2api_provider();
        if let Some(p) = self
            .providers
            .iter_mut()
            .find(|p| p.id == WEB2API_PROVIDER_ID)
        {
            p.enabled = on;
        }
        if !on {
            self.sanitize_active();
        }
    }

    /// The provider a fresh session should start on.
    ///
    /// "Enabled with a base URL" is not enough: the preset list ships with
    /// openai/anthropic/gemini enabled and empty keys, so a freshly installed
    /// app would pick `openai`, send the first message without an
    /// `Authorization` header and come back as a bald 401 "鉴权失败". Prefer a
    /// provider that is actually ready to talk — key filled in (or a loopback
    /// endpoint that needs none).
    pub fn default_provider_id(&self) -> String {
        let ready = |p: &&ProviderConfig| {
            p.enabled
                && !p.base_url.trim().is_empty()
                && (!p.api_key.trim().is_empty() || is_loopback(&p.base_url))
        };
        // The built-in Web2API provider is loopback-ready by definition, so it
        // would otherwise win every fresh boot and send the first message into
        // a browser session that was never logged in. It is opt-in through the
        // model picker instead.
        let candidates: Vec<&ProviderConfig> = self
            .providers
            .iter()
            .filter(|p| p.id != WEB2API_PROVIDER_ID)
            .collect();
        candidates
            .iter()
            .find(|p| ready(p))
            .copied()
            .or_else(|| {
                candidates
                    .iter()
                    .find(|p| p.enabled && !p.base_url.trim().is_empty())
                    .copied()
            })
            .map(|p| p.id.clone())
            .unwrap_or_default()
    }
}

/// Local inference servers (ollama, llama.cpp, LM Studio) take no key.
pub fn is_loopback(base_url: &str) -> bool {
    let u = base_url.trim().to_ascii_lowercase();
    u.contains("://localhost")
        || u.contains("://127.")
        || u.contains("://0.0.0.0")
        || u.contains("://[::1]")
        || u.contains("://192.168.")
        || u.starts_with("localhost")
        || u.starts_with("127.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("irixen-cfg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The marker is the whole handshake between installer and app: a dev build
    /// has no install directory, so it must never reset a profile by accident.
    #[test]
    fn the_fresh_install_marker_is_consumed_exactly_once() {
        let dir = scratch("marker");
        assert!(!consume_marker(&dir), "no marker, nothing to do");

        std::fs::write(dir.join(FIRST_RUN_MARKER), "").unwrap();
        assert!(consume_marker(&dir), "a fresh install is detected");
        assert!(
            !dir.join(FIRST_RUN_MARKER).exists(),
            "the marker is removed, not merely read — otherwise every launch resets"
        );
        assert!(!consume_marker(&dir), "the second launch leaves the profile alone");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
