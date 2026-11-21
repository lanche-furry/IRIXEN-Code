// Global UI state + a tiny event bus. No framework, no proxies — explicit re-renders.

import type {
  AppConfig, AppInfo, ExtensionInfo, FsEntry, GitFileStatus, HostLog, HostStatus, McpServerView,
  ModelInfo, NodeInfo, ProviderConfig, RuntimeCommand, Skill, StatusItem, UsageReport,
  WorkspaceStats, Effort,
} from '../types';

export type ViewId = 'explorer' | 'search' | 'git' | 'skills' | 'stats';

/** Which panel owns the stage. `editor` is the full-bleed file view that covers
 *  the dock; everything else is a dock panel selected from the right rail.
 *  `bridge` (Web2API) is only reachable while the advanced switch is on: the
 *  rail paints its button from the config, and `setSurface` refuses it
 *  otherwise. */
export type Surface = 'chat' | 'review' | 'bridge' | 'extensions' | 'editor';

export interface OpenFile {
  path: string;
  name: string;
  content: string;
  saved: string;
  dirty: boolean;
  lang: string;
}

/** Files one agent turn rewrote, and whether they can still be put back. */
export interface ChatTurn {
  id: number;
  files: string[];
  undoable: boolean;
  reason?: string | null;
  /** Set once the restore ran, so the bar stops offering it. */
  undone?: boolean;
  undoing?: boolean;
}

export interface ChatItem {
  id: string;
  role: 'user' | 'assistant' | 'system';
  text: string;
  reasoning?: string;
  tools?: { id: string; name: string; args: string; output?: string; isError?: boolean; open?: boolean }[];
  note?: string;
  error?: boolean;
  streaming?: boolean;
  turn?: ChatTurn;
  /** Backend turn id, tagged on the prompt and the reply so undo can remove
   *  the whole exchange from the transcript as well as restore its files. */
  turnId?: number;
}

export const app = {
  config: null as AppConfig | null,
  info: null as AppInfo | null,
  view: 'explorer' as ViewId,
  sessionId: '' as string,
  /** Title of the open session, shown in the chat header so a switch is visible. */
  sessionTitle: '' as string,
  busy: false,
  models: [] as ModelInfo[],
  modelsLoading: false,
  providers: [] as ProviderConfig[],
  usage: null as UsageReport | null,
  stats: null as WorkspaceStats | null,
  statsLoading: false,
  skills: [] as Skill[],
  extensions: [] as ExtensionInfo[],
  /** Per-extension Node host processes. */
  hosts: [] as HostStatus[],
  extCommands: [] as RuntimeCommand[],
  extStatus: [] as StatusItem[],
  extLogs: [] as HostLog[],
  node: null as NodeInfo | null,
  mcp: [] as McpServerView[],
  files: [] as OpenFile[],
  activeFile: '' as string,
  chat: [] as ChatItem[],
  contextTokens: 0,
  git: [] as GitFileStatus[],
  gitBranch: '' as string,
  gitRepo: false,
  tree: new Map<string, FsEntry[]>(),
  expanded: new Set<string>(),
  chatOpen: true,
  /** Which panel fills the stage — the dock (chat/review/bridge) or a file. */
  surface: 'chat' as Surface,
  panelOpen: false,
  panelTab: 'terminal' as 'terminal' | 'output' | 'usage',
  terminal: [] as string[],
  statusText: '',
  effort: 'auto' as Effort,
  model: '' as string,
  providerId: '' as string,
  sidebarWidth: 264,
  chatWidth: 400,
};

type Handler = (payload?: any) => void;
const handlers = new Map<string, Set<Handler>>();

export function on(evt: string, fn: Handler): () => void {
  if (!handlers.has(evt)) handlers.set(evt, new Set());
  handlers.get(evt)!.add(fn);
  return () => handlers.get(evt)?.delete(fn);
}

export function emit(evt: string, payload?: any) {
  handlers.get(evt)?.forEach((fn) => {
    try {
      fn(payload);
    } catch (e) {
      console.error('[irixen] handler error', evt, e);
    }
  });
}

export function activeFile(): OpenFile | undefined {
  return app.files.find((f) => f.path === app.activeFile);
}

/** Status-bar text is a *label*, never content: one line, bounded.
 *
 *  A multi-line paragraph here used to widen `.app` to its min-content (the
 *  statusbar does not wrap), which pushed the chat column off-screen and made
 *  the whole layout look broken mid-conversation. Clamp at the source too. */
export function setStatus(text: string) {
  const flat = (text || '').replace(/\s+/g, ' ').trim();
  app.statusText = flat.length > 90 ? `${flat.slice(0, 90)}…` : flat;
  emit('status');
}
