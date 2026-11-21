// Typed bindings for the Rust command surface + agent event stream.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type {
  AgentEvent, AppConfig, AppInfo, AppUpdate, BridgeLogRow, BridgeStatus, Effort, ExtDetail,
  ExtInspect, ExtSearchRow, ExtUiRequest, ExtUpdate, ExtensionInfo, FsEntry, GitFileStatus, HostLog,
  HostStatus, McpServerConfig, McpServerView, McpToolInfo, Message, ModelInfo,
  NodeInfo, ProviderConfig, RuntimeCommand, SearchHit, Skill, SnippetRow, StatusItem,
  UsageReport, WorkspaceStats, Attachment, SessionMeta, ProbeReport,
  WebSource, WebTestRow, WebSessionInfo, RunnerOption, RunInfo, WebviewPanel,
  Downloaded, DownloadProgress, AgentAccess,
  TermShell, TermInfo, TreeItemRow, PreviewProbe,
} from '../types';

export interface AttachmentRef { path: string; name: string; isDir: boolean }

/** Bare base64 plus the mime type the encoder produced. */
export interface SentImage { mime: string; data: string }

export const api = {
  // config
  getConfig: () => invoke<AppConfig>('get_config'),
  saveConfig: (config: AppConfig) => invoke<AppConfig>('save_config', { config }),
  configReset: () => invoke<AppConfig>('config_reset'),
  appInfo: () => invoke<AppInfo>('app_info'),
  /** Reveal the skills/extensions folder; the path comes from config, never
   *  from the renderer. */
  openAddonsFolder: () => invoke<string>('open_addons_folder'),
  appCheckUpdate: (force = false) => invoke<AppUpdate>('app_check_update', { force }),
  downloadUpdate: () => invoke<Downloaded>('app_download_update'),
  // Returns only after the installer is running; the app is gone by the time
  // the promise settles, so callers must not expect to do anything after it.
  applyUpdate: (path: string) => invoke<void>('app_apply_update', { path }),
  setWorkspace: (path: string) => invoke<string>('set_workspace', { path }),

  // providers / models
  providerUpsert: (provider: ProviderConfig) =>
    invoke<ProviderConfig[]>('provider_upsert', { provider }),
  providerDelete: (id: string) => invoke<ProviderConfig[]>('provider_delete', { id }),
  providerTest: (id: string) => invoke<number>('provider_test', { id }),
  providerProbe: (id: string) => invoke<ProbeReport>('provider_probe', { id }),
  modelsList: (providerId: string, refresh: boolean) =>
    invoke<ModelInfo[]>('models_list', { providerId, refresh }),

  // sessions
  sessions: () => invoke<SessionMeta[]>('agent_sessions'),
  newSession: () => invoke<SessionMeta>('agent_new_session'),
  setActive: (id: string) => invoke<boolean>('agent_set_active', { id }),
  active: () => invoke<string | null>('agent_active'),
  messages: (id: string) => invoke<Message[]>('agent_messages', { id }),
  send: (id: string, text: string, attachments: AttachmentRef[], images: SentImage[] = []) =>
    invoke<void>('agent_send', {
      id,
      text,
      attachments: attachments.map((a) => ({ path: a.path, name: a.name, is_dir: a.isDir })),
      images,
    }),
  stop: (id: string) => invoke<void>('agent_stop', { id }),
  clear: (id: string) => invoke<void>('agent_clear', { id }),
  deleteSession: (id: string) => invoke<void>('agent_delete', { id }),
  undo: (turn: number) => invoke<string[]>('agent_undo_turn', { turn }),
  setModel: (id: string, providerId?: string, model?: string, effort?: Effort) =>
    invoke<void>('agent_set_model', { id, providerId, model, effort }),
  setDefaultEffort: (effort: Effort) => invoke<void>('set_default_effort', { effort }),
  setAgentAccess: (access: AgentAccess) => invoke<void>('set_agent_access', { access }),
  /** Answer a parked tool call. False means it already expired. */
  approve: (id: string, allow: boolean) => invoke<boolean>('agent_approve', { id, allow }),
  busy: (id: string) => invoke<boolean>('agent_busy', { id }),
  usage: () => invoke<UsageReport>('agent_usage'),
  resetUsage: () => invoke<void>('agent_reset_usage'),

  // fs
  list: (path: string) => invoke<FsEntry[]>('fs_list', { path }),
  read: (path: string) => invoke<string>('fs_read', { path }),
  write: (path: string, content: string) => invoke<number>('fs_write', { path, content }),
  createFile: (path: string) => invoke<string>('fs_create_file', { path }),
  createDir: (path: string) => invoke<string>('fs_create_dir', { path }),
  remove: (path: string) => invoke<void>('fs_delete', { path }),
  rename: (from: string, to: string) => invoke<void>('fs_rename', { from, to }),
  search: (query: string, glob?: string) =>
    invoke<SearchHit[]>('fs_search', { query, glob, caseSensitive: false, maxResults: 200 }),

  // shell
  run: (command: string, cwd?: string) => invoke<string>('shell_run', { command, cwd }),

  // language + running files
  setFileLanguage: (path: string, language: string) =>
    invoke<void>('language_set', { path, language }),
  runnerOptions: (lang: string) => invoke<RunnerOption[]>('runner_options', { lang }),
  runnerSave: (lang: string, command: string, args: string) =>
    invoke<void>('runner_save', { lang, command, args }),
  runnerReset: (lang: string) => invoke<void>('runner_reset', { lang }),
  runFile: (path: string, language: string, command?: string, args?: string) =>
    invoke<RunInfo>('file_run', { path, language, command: command ?? null, args: args ?? null }),
  runStop: (id: string) => invoke<void>('file_run_stop', { id }),

  // git
  gitIsRepo: () => invoke<boolean>('git_is_repo'),
  gitStatus: () => invoke<GitFileStatus[]>('git_status'),
  gitDiff: (staged: boolean, path?: string) => invoke<string>('git_diff', { staged, path }),
  gitStage: (paths: string[]) => invoke<string>('git_stage', { paths }),
  gitUnstage: (paths: string[]) => invoke<string>('git_unstage', { paths }),
  gitCommit: (message: string) => invoke<string>('git_commit', { message }),
  gitBranch: () => invoke<string>('git_branch'),
  gitBranches: () => invoke<string[]>('git_branches'),
  gitCheckout: (branch: string) => invoke<string>('git_checkout', { branch }),
  gitLog: (limit = 20) => invoke<string>('git_log', { limit }),
  gitInit: () => invoke<string>('git_init'),
  gitPush: () => invoke<string>('git_push'),
  gitPull: () => invoke<string>('git_pull'),

  // stats
  stats: () => invoke<WorkspaceStats>('workspace_stats'),

  // mcp
  mcpServers: () => invoke<McpServerView[]>('mcp_servers'),
  mcpUpsert: (server: McpServerConfig) => invoke<McpServerConfig[]>('mcp_upsert', { server }),
  mcpDelete: (id: string) => invoke<McpServerConfig[]>('mcp_delete', { id }),
  mcpStart: (id: string) => invoke<McpToolInfo[]>('mcp_start', { id }),
  mcpStop: (id: string) => invoke<void>('mcp_stop', { id }),
  mcpTools: () => invoke<McpToolInfo[]>('mcp_tools'),

  // skills
  skills: () => invoke<Skill[]>('skills_list'),
  skillInstall: (source: string) => invoke<Skill>('skills_install', { source }),
  skillUninstall: (id: string) => invoke<void>('skills_uninstall', { id }),
  skillToggle: (id: string, enabled: boolean) =>
    invoke<void>('skills_toggle', { id, enabled }),
  skillRead: (id: string) => invoke<string>('skills_read', { id }),

  // window
  mainLoaded: () => invoke<void>('main_loaded'),
  windowControl: (action: 'minimize' | 'maximize' | 'close') =>
    invoke<void>('window_control', { action }),

  // web2api bridge
  web2apiStatus: () => invoke<BridgeStatus>('web2api_status'),
  web2apiStart: () => invoke<BridgeStatus>('web2api_start'),
  web2apiStop: () => invoke<BridgeStatus>('web2api_stop'),
  web2apiLog: () => invoke<BridgeLogRow[]>('web2api_log'),
  web2apiOpenSession: (sourceId: string, homeUrl: string) =>
    invoke<void>('web2api_open_session', { sourceId, homeUrl }),
  web2apiHideSession: (sourceId: string) =>
    invoke<boolean>('web2api_hide_session', { sourceId }),
  web2apiSessionStatus: () => invoke<WebSessionInfo[]>('web2api_session_status'),
  web2apiProbe: () => invoke<WebSessionInfo[]>('web2api_probe'),
  /** Start the endpoint (if down) and open the matching reverse proxy in the
   *  background. `model` narrows it to the source that model routes to. */
  web2apiEnsure: (model?: string | null) =>
    invoke<BridgeStatus>('web2api_ensure', { model: model ?? null }),
  web2apiTest: (sources: WebSource[]) =>
    invoke<WebTestRow[]>('web2api_test', { sources }),

  // extensions
  extensionsList: () => invoke<ExtensionInfo[]>('extensions_list'),
  extensionsInstall: (source: string) =>
    invoke<ExtensionInfo>('extensions_install', { source }),
  extensionsUninstall: (id: string) => invoke<void>('extensions_uninstall', { id }),
  extensionsToggle: (id: string, enabled: boolean) =>
    invoke<ExtensionInfo>('extensions_toggle', { id, enabled }),
  extensionsSearch: (query: string, category = '', sort = 'downloads', offset = 0) =>
    invoke<ExtSearchRow[]>('extensions_search', { query, category, sort, offset }),
  extensionsDetail: (id: string) => invoke<ExtDetail>('extensions_detail', { id }),
  extensionsInspect: (id: string) => invoke<ExtInspect>('extensions_inspect', { id }),
  extensionsCheckUpdates: (force = false) =>
    invoke<ExtUpdate[]>('extensions_check_updates', { force }),
  extensionsTheme: (id: string, label: string) =>
    invoke<Record<string, string>>('extensions_theme', { id, label }),
  extensionsSnippets: (language: string) =>
    invoke<SnippetRow[]>('extensions_snippets', { language }),

  // extension host (Node)
  exthostNode: () => invoke<NodeInfo>('exthost_node'),
  exthostInstallNode: () => invoke<NodeInfo>('exthost_install_node'),
  exthostStatus: () => invoke<HostStatus[]>('exthost_status'),
  exthostStart: (id: string) => invoke<HostStatus>('exthost_start', { id }),
  exthostStop: (id: string) => invoke<void>('exthost_stop', { id }),
  exthostStopAll: () => invoke<void>('exthost_stop_all'),
  exthostRestartAll: () => invoke<void>('exthost_restart_all'),
  exthostRestart: (id: string) => invoke<HostStatus>('exthost_restart', { id }),
  previewProbe: (url: string) => invoke<PreviewProbe>('preview_probe', { url }),
  exthostLogs: (id?: string) => invoke<HostLog[]>('exthost_logs', { id: id ?? null }),
  exthostClearLogs: (id: string) => invoke<void>('exthost_clear_logs', { id }),
  exthostCommands: () => invoke<RuntimeCommand[]>('exthost_commands'),
  exthostStatusbar: () => invoke<StatusItem[]>('exthost_statusbar'),
  exthostRun: (command: string) => invoke<unknown>('exthost_run', { command }),
  // A cancelled dialog resolves to `undefined`; Tauri rejects a missing key
  // outright, which leaves the extension's request hanging forever.
  exthostReply: (id: number, value: unknown) => invoke<void>('exthost_reply', { id, value: value ?? null }),
  exthostDoc: (doc: {
    kind: 'open' | 'change' | 'save' | 'close' | 'active';
    path: string;
    language: string;
    text: string;
    dirty: boolean;
    version: number;
    selection?: unknown;
  }) => invoke<void>('exthost_doc', doc),
  /** Ask extensions for a language feature. Returns `[{ ext, value }]`. */
  extLanguage: (args: {
    kind: 'completion' | 'hover' | 'format' | 'rangeFormat';
    path: string;
    language: string;
    text: string;
    line?: number;
    character?: number;
    options?: unknown;
  }) =>
    invoke<{ ext: string; value: unknown }[]>('ext_language', {
      kind: args.kind,
      path: args.path,
      language: args.language,
      text: args.text,
      line: args.line ?? null,
      character: args.character ?? null,
      options: args.options ?? null,
    }),
  webviewPost: (id: string, message: unknown) =>
    invoke<void>('webview_post', { id, message }),
  webviewClose: (id: string) => invoke<void>('webview_close', { id }),
  webviewList: () => invoke<WebviewPanel[]>('webview_list'),

  /* --- extension-contributed sidebar views (tree data providers) --- */
  exttreeViews: () => invoke<string[]>('exttree_views'),
  exttreeChildren: (view: string, key?: string) =>
    invoke<TreeItemRow[]>('exttree_children', { view, key: key || null }),
  exttreeCommand: (view: string, key: string, command?: string) =>
    invoke<boolean>('exttree_command', { view, key, command: command || null }),
  exttreeRefresh: (view: string) => invoke<boolean>('exttree_refresh', { view }),
  /** Run a command from the output panel; output streams as `run://event`. */
  shellStream: (command: string, cwd?: string) =>
    invoke<RunInfo>('shell_stream', { command, cwd: cwd || null }),

  /* --- integrated terminal (real PTY) --- */
  termShells: () => invoke<TermShell[]>('term_shells'),
  termOpen: (args: { cwd?: string; shell?: string; cols?: number; rows?: number }) =>
    invoke<TermInfo>('term_open', {
      cwd: args.cwd || null,
      shell: args.shell || null,
      cols: args.cols ?? 100,
      rows: args.rows ?? 24,
    }),
  /** Keystrokes go down the PTY as-is; nothing here waits for Enter. */
  termWrite: (id: string, data: string) => invoke<void>('term_write', { id, data }),
  termResize: (id: string, cols: number, rows: number) =>
    invoke<void>('term_resize', { id, cols, rows }),
  termClose: (id: string) => invoke<void>('term_close', { id }),
  /** Catch up with a PTY an extension created before the panel existed. */
  termAttach: (id: string) =>
    invoke<{ id: string; shell: string; cwd: string; replay: string }>('term_attach', { id }),
  termList: () => invoke<TermInfo[]>('term_list'),
  /** Hand the final HTML to the `irixen-wv` scheme; returns the URL to load. */
  webviewPublish: (id: string, html: string) =>
    invoke<string>('webview_publish', { id, html, userAgent: navigator.userAgent }),
  webviewForget: (id: string) => invoke<void>('webview_forget', { id }),
  webviewBase: () => invoke<string>('webview_base', { userAgent: navigator.userAgent }),
  openExternal: (url: string) => invoke<void>('open_external', { url }),
};

/** Extension-host events (status / commands / output / prompts). */
export function onExtHostEvent<T>(name: string, cb: (payload: T) => void): Promise<() => void> {
  return listen<T>(`exthost://${name}`, (e) => cb(e.payload));
}

/** Subscribe to the agent's normalized event stream. Returns an unlisten fn. */
export function onAgentEvent(cb: (ev: AgentEvent) => void): Promise<() => void> {
  return listen<AgentEvent>('agent://event', (e) => cb(e.payload));
}

export function onUpdateProgress(cb: (p: DownloadProgress) => void): Promise<() => void> {
  return listen<DownloadProgress>('update:progress', (e) => cb(e.payload));
}

export function errText(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}
