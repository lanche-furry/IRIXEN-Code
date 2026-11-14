// Mirrors of the Rust types crossing the IPC boundary.

export type Protocol = 'openai' | 'anthropic' | 'gemini';
export type Effort = 'auto' | 'off' | 'minimal' | 'low' | 'medium' | 'high';
export type Role = 'system' | 'user' | 'assistant' | 'tool';

export interface ProviderConfig {
  id: string;
  name: string;
  protocol: Protocol;
  base_url: string;
  api_key: string;
  enabled: boolean;
  models_path: string;
  extra_headers: [string, string][];
  extra_body: Record<string, unknown>;
  note: string;
}

export interface ActiveSelection {
  provider_id: string;
  model: string;
  effort: Effort;
}

export interface CompressionConfig {
  max_context_tokens: number;
  threshold: number;
  tool_result_age: number;
  tool_result_cap: number;
  keep_recent: number;
  digest_max_tokens: number;
  enabled: boolean;
}

export interface AgentConfig {
  max_iterations: number;
  temperature: number | null;
  system_prompt: string;
  compression: CompressionConfig;
  auto_skills: boolean;
}

export interface McpServerConfig {
  id: string;
  name: string;
  command: string;
  args: string[];
  env: [string, string][];
  enabled: boolean;
  auto_start: boolean;
}

export interface SkillsConfig {
  enabled: string[];
  auto_match: boolean;
}

export interface CustomTheme {
  id: string;
  name: string;
  colors: [string, string][];
}

export type BridgeUpstream = 'web' | 'provider';

/** A browser-based AI service proxied by Web2API. All wire details are
 *  templates so a broken private API can be repaired without a rebuild. */
export interface WebSource {
  id: string;
  label: string;
  /** `browser` = request from the vendor's logged-in page; `http` = raw template. */
  mode: 'browser' | 'http' | string;
  /** Where the browser session window starts. */
  home_url: string;
  session_key: string;
  org_id: string;
  model: string;
  endpoint: string;
  auth_header: string;
  content_type: string;
  body_template: string;
  text_path: string;
  text_guard: string;
  cumulative: boolean;
  org_endpoint: string;
  org_path: string;
  /** `ui` drives the page's own composer; `api` posts to the private endpoint. */
  driver: 'ui' | 'api' | string;
  /** `incremental` sends only the newest user turn to the stateful page;
   *  `full` re-injects the whole system prompt + history (old behaviour).
   *  Stateless transports (`http` / `api` driver) always use full. */
  prompt_mode: 'incremental' | 'full' | string;
  /** Custom UI driver script; empty = built-in generic page driver. */
  ui_script: string;
  /** Same-origin endpoint used to detect a logged-in session. */
  login_endpoint: string;
  /** Open this reverse proxy's hidden browser session automatically. */
  auto_open: boolean;
}

/** Cached login state of one Web2API reverse proxy. */
export interface WebSessionInfo {
  id: string;
  label: string;
  open: boolean;
  logged_in: boolean;
  account: string;
  detail: string;
  checked_at: number;
}

export interface WebTestRow {
  id: string;
  label: string;
  mode: string;
  ok: boolean;
  status: number;
  detail: string;
}

/** Case-insensitive prefix of a client's `model` → which web source handles it. */
export interface ModelRoute {
  prefix: string;
  source_id: string;
}

/** Web2API: a local OpenAI/Anthropic-compatible endpoint over web sessions. */
export interface BridgeConfig {
  auto_start: boolean;
  port: number;
  api_key: string;
  upstream: BridgeUpstream;
  /** Debug only: an API provider already configured for the agent. */
  provider_id: string;
  /** Fallback model name when a client sends none. */
  model: string;
  sources: WebSource[];
  routes: ModelRoute[];
  default_source: string;
  /** Start the endpoint + its browser session when a Web2API model is picked. */
  auto_ensure: boolean;
  /** Set by the Rust migration; never edit from the UI. */
  routing_version: number;
  /** Legacy fields preserved for old config files; the UI does not use them. */
  claude_session_key: string;
  claude_org_id: string;
  claude_model: string;
}

export interface BridgeLogRow {
  at: number;
  method: string;
  path: string;
  status: number;
  ms: number;
  detail: string;
}

export interface BridgeStatus {
  running: boolean;
  port: number;
  base_url: string;
  upstream: string;
  model: string;
  sources: number;
  routes: number;
  default_source: string;
  requests: number;
  errors: number;
  started_at: number;
  last_error: string | null;
  log: BridgeLogRow[];
}

export interface AppConfig {
  version: number;
  workspace: string | null;
  theme: string;
  language: string;
  providers: ProviderConfig[];
  active: ActiveSelection | null;
  /** Sticky default for the thinking-effort picker (new sessions start here). */
  effort: Effort;
  agent: AgentConfig;
  mcp_servers: McpServerConfig[];
  skills: SkillsConfig;
  custom_themes: CustomTheme[];
  /** Proxy for API calls. Empty = auto, `none` = direct. */
  proxy: string;
  /** Web2API bridge endpoint. */
  bridge: BridgeConfig;
  /** Extension host + store. */
  extensions: ExtensionsConfig;
  /** Per-file language overrides, keyed by absolute path. */
  file_languages?: Record<string, string>;
  /** Per-language runner overrides. */
  runners?: Record<string, RunnerConfig>;
  /** Shell for the integrated terminal. Empty = auto-detect. */
  terminal_shell?: string;
  /** Whether the first-run setup wizard has been completed (or skipped). */
  onboarding_completed?: boolean;
}

export interface RunnerConfig {
  command: string;
  args: string;
  env?: [string, string][];
}

export interface RunnerOption {
  id: string;
  label: string;
  command: string;
  args: string;
  available: boolean;
}

export interface RunInfo {
  id: string;
  command: string;
  cwd: string;
  language: string;
}

/** A shell the integrated terminal can start. */
export interface TermShell {
  id: string;
  label: string;
  program: string;
  args: string[];
  available: boolean;
  recommended: boolean;
}

/** A live PTY session. */
export interface TermInfo {
  id: string;
  shell: string;
  cwd: string;
  pid?: number | null;
}

/** Result of asking the Rust side what a preview URL actually served. */
export interface PreviewProbe {
  reason: 'ok' | 'bundler' | 'unreachable' | 'http' | 'not_html';
  status: number;
  dev_url: string;
}

/** A webview panel created by a VS Code extension. */
export interface WebviewPanel {
  id: string;
  ext: string;
  title: string;
  html: string;
  viewType: string;
  options?: Record<string, unknown>;
}

export interface ExtensionsConfig {
  /** Run real VS Code extensions in a Node child process. */
  node_host: boolean;
  /** Explicit node binary; empty = auto-detect. */
  node_path: string;
  autostart_delay_ms: number;
  /** Start every installed extension at launch, not just `onStartupFinished`. */
  auto_activate_all?: boolean;
  /** Gap between extension hosts coming up, in ms. */
  autostart_stagger_ms?: number;
}

export interface ProbeStep {
  label: string;
  ok: boolean;
  detail: string;
}

export interface ProbeReport {
  url: string;
  proxy: string;
  ok: boolean;
  steps: ProbeStep[];
  models: number;
  error: string | null;
}

export type ThinkingSupport =
  | { type: 'unknown' }
  | { type: 'unsupported' }
  | { type: 'always_on' }
  | { type: 'effort'; levels: Effort[] }
  | { type: 'budget'; min: number; max: number };

export interface ModelInfo {
  id: string;
  provider_id: string;
  display_name: string | null;
  owned_by: string | null;
  context_length: number | null;
  thinking: ThinkingSupport;
  source: 'fetched' | 'heuristic' | 'cached';
}

export interface Usage {
  input_tokens: number;
  output_tokens: number;
  reasoning_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
}

export interface CompressionReport {
  tokens_before: number;
  tokens_after: number;
  tool_results_elided: number;
  messages_dropped: number;
  digest: string | null;
}

export type AgentEvent =
  | { kind: 'status'; session: string; text: string }
  | { kind: 'delta'; session: string; text: string }
  | { kind: 'reasoning'; session: string; text: string }
  | { kind: 'tool_call'; session: string; id: string; name: string; arguments: string }
  | { kind: 'tool_result'; session: string; id: string; name: string; output: string; is_error: boolean }
  | { kind: 'usage'; session: string; usage: Usage }
  | { kind: 'compressed'; session: string; report: CompressionReport }
  | { kind: 'done'; session: string; text: string }
  | { kind: 'error'; session: string; text: string };

export interface SessionMeta {
  id: string;
  provider_id: string;
  model: string;
  effort: Effort;
  title: string;
  message_count: number;
  updated_at: number;
}

export interface ContentPartText { type: 'text'; text: string }
export interface ContentPartToolCall { type: 'tool_call'; id: string; name: string; arguments: string }
export interface ContentPartToolResult { type: 'tool_result'; call_id: string; name: string; output: string; is_error: boolean }
export type ContentPart = ContentPartText | ContentPartToolCall | ContentPartToolResult;

export interface Message {
  role: Role;
  parts: ContentPart[];
}

export interface Attachment {
  path: string;
  name: string;
  is_dir: boolean;
}

export interface FsEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  modified: number;
}

export interface SearchHit {
  path: string;
  line: number;
  text: string;
}

export interface GitFileStatus {
  path: string;
  code: string;
  staged: boolean;
}

export interface ExtStat { ext: string; files: number; loc: number; bytes: number }
export interface FileStat { path: string; bytes: number; lines: number }
export interface WorkspaceStats {
  root: string;
  files: number;
  dirs: number;
  total_bytes: number;
  total_loc: number;
  by_ext: ExtStat[];
  largest: FileStat[];
  scanned_at: number;
}

export interface ModelUsage { model: string; usage: Usage }
export interface UsageReport {
  calls: number;
  errors: number;
  input_tokens: number;
  output_tokens: number;
  reasoning_tokens: number;
  cache_read_tokens: number;
  elapsed_secs: number;
  by_model: ModelUsage[];
  by_session: ModelUsage[];
}

export interface Skill {
  id: string;
  name: string;
  description: string;
  when: string[];
  body: string;
  builtin: boolean;
  enabled: boolean;
  path: string | null;
}

export interface McpToolInfo {
  server_id: string;
  server_name: string;
  name: string;
  qualified_name: string;
  description: string;
  input_schema: unknown;
}

export interface McpServerView {
  config: McpServerConfig;
  running: boolean;
  tools: number;
  stderr: string[];
}

export interface AppInfo {
  /** Product label, e.g. "Beta 0.1" — use this in UI. */
  version: string;
  /** Machine-readable semver, e.g. "0.1.0-beta.1" — diagnostics only. */
  semver: string;
  config_dir: string;
  platform: string;
  workspace: string | null;
}

/** Result of a GitHub Releases lookup from the About page. */
export interface UpdateAsset {
  name: string;
  size: number;
  download_url: string;
}

export interface AppUpdate {
  current: string;
  current_semver: string;
  latest: string;
  latest_semver: string;
  name: string;
  published_at: string;
  notes: string;
  url: string;
  assets: UpdateAsset[];
  has_update: boolean;
  prerelease: boolean;
  checked_at: number;
  releases_url: string;
  no_release: boolean;
}

/* ------------------------------------------------------------ extensions */

export interface ExtCommand {
  command: string;
  title: string;
  category: string;
  /** Absolute path of the command icon, empty when it has none. */
  icon?: string;
}

export interface ExtTheme {
  id: string;
  label: string;
  path: string;
}

export interface ExtSnippet {
  language: string;
  path: string;
}

export interface ExtLanguage {
  id: string;
  aliases: string[];
  extensions: string[];
}

/** A `contributes.submenus` declaration: an id other menu rows point at. */
export interface ExtSubmenu {
  id: string;
  label: string;
}

export interface ExtContributions {
  commands: ExtCommand[];
  themes: ExtTheme[];
  snippets: ExtSnippet[];
  languages: ExtLanguage[];
  /** `contributes.menus` rows, ready to be merged into our own context menus. */
  menus: ExtMenu[];
  /** Optional for sidecars written before submenu support existed. */
  submenus?: ExtSubmenu[];
  /** `contributes.viewsContainers.*` — sidebar/activity-bar slots. */
  viewContainers?: ExtViewContainer[];
  /** `contributes.views.<container>` — the individual sections inside a slot. */
  views?: ExtView[];
  skills: string[];
}

/** An activity-bar (or panel) container an extension contributes. */
export interface ExtViewContainer {
  id: string;
  title: string;
  /** Absolute path of the container icon, empty when it has none. */
  icon: string;
}

/** One view inside a contributed container. */
export interface ExtView {
  /** Owning container id. */
  container: string;
  id: string;
  name: string;
  /** Raw VS Code `when` clause. */
  when: string;
  /** `collapsed`, `visible`, `hidden`. */
  visibility: string;
}

/** A serialised `vscode.TreeItem` returned by an extension's data provider. */
export interface TreeItemRow {
  /** Opaque handle used to ask the provider for this node's children. */
  key: string;
  label: string;
  description?: string;
  tooltip?: string;
  /** 0 = none, 1 = collapsed, 2 = expanded. */
  state: number;
  context?: string;
  /** Theme icon id, e.g. `folder`. */
  themeIcon?: string;
  /** Absolute path of a custom icon, ready for `convertFileSrc`. */
  iconPath?: string;
  /** `resourceUri.fsPath`, used by context menus. */
  path?: string;
  /** Command the provider attached to this item. */
  command?: { command: string; title: string };
}

/** One right-click / title-bar entry an extension contributes. */
export interface ExtMenu {
  /** `editor/context`, `explorer/context`, `view/item/context`, `commandPalette`, … */
  menu: string;
  /** Empty for rows that open a submenu instead of running a command. */
  command: string;
  /** `contributes.menus` row pointing at a `submenus` id. */
  submenu?: string;
  group: string;
  /** Raw VS Code `when` clause. */
  when: string;
}

/** An installed third-party package: VS Code extension or DSH plugin. */
/** Token form of the "what actually works here" note. */
export interface ExtNotice {
  kind: 'capabilities' | 'hosted' | 'meta_only' | 'skills' | 'no_skills' | string;
  parts: string[];
  count: number;
}

export interface ExtensionInfo {
  id: string;
  dir: string;
  name: string;
  publisher: string;
  version: string;
  display_name: string;
  description: string;
  /** Absolute path of the extension icon, empty when the package ships none. */
  icon?: string;
  kind: 'vscode' | 'dsh' | string;
  source: 'vsix' | 'online' | 'git' | string;
  enabled: boolean;
  installed_at: number;
  contributes: ExtContributions;
  /** `main` from package.json, relative to the extension directory. */
  entry: string;
  /** `activationEvents`, e.g. `["onStartupFinished", "onCommand:foo"]`. */
  activation: string[];
  /** `main` is declared but missing on disk — the package unpacked incompletely. */
  broken?: boolean;
  /** Derived on the Rust side; the UI turns it into a localized sentence. */
  notice: ExtNotice;
}

/** Local, offline detail for an extension that is already installed. */
export interface ExtInspect {
  info: ExtensionInfo;
  readme: string;
  changelog: string;
  license: string;
  license_text: string;
  homepage: string;
  repository: string;
  categories: string[];
  keywords: string[];
  engines: string;
  size_bytes: number;
  file_count: number;
}

export interface ExtUpdate {
  id: string;
  current: string;
  latest: string;
}

export interface ExtSearchRow {
  id: string;
  namespace: string;
  name: string;
  display_name: string;
  description: string;
  version: string;
  downloads: number;
  rating: number;
  timestamp: string;
  icon: string;
  /** Empty when not installed locally. */
  installed_version: string;
}

/** Everything the store's detail pane needs. */
export interface ExtDetail {
  id: string;
  namespace: string;
  name: string;
  display_name: string;
  description: string;
  version: string;
  versions: string[];
  downloads: number;
  rating: number;
  reviews: number;
  license: string;
  repository: string;
  homepage: string;
  categories: string[];
  readme: string;
  node: boolean;
  installed_version: string;
}

/** State of one Node extension host process. */
export interface HostStatus {
  id: string;
  state: 'stopped' | 'starting' | 'running' | 'stopping' | 'crashed' | string;
  pid: number;
  node: string;
  error: string;
  started_at: number;
  activated: boolean;
  commands: string[];
  entry: string;
}

export interface RuntimeCommand {
  id: string;
  title: string;
  category: string;
  ext: string;
  ext_name: string;
  /** True once the running extension registered it. */
  live: boolean;
}

export interface HostLog {
  id: string;
  ts: number;
  level: string;
  text: string;
}

export interface StatusItem {
  ext: string;
  id: string;
  text: string;
  tooltip: string;
  command: string;
  visible: boolean;
  alignment: number;
  priority: number;
}

export interface NodeInfo {
  found: boolean;
  path: string;
  version: string;
  bundled: boolean;
}

/** A `window.show*` prompt an extension is blocking on. */
export interface ExtUiRequest {
  id: number;
  kind: 'message' | 'quickPick' | 'inputBox';
  ext: string;
  severity?: string;
  message?: string;
  detail?: string;
  items?: { label: string; description?: string; detail?: string }[] | string[];
  modal?: boolean;
  placeholder?: string;
  prompt?: string;
  value?: string;
  placeHolder?: string;
  password?: boolean;
  canPickMany?: boolean;
}

export interface SnippetRow {
  ext: string;
  language: string;
  prefix: string;
  body: string;
  description: string;
}
