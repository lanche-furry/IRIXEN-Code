// The integrated terminal.
//
// The old panel was an input box: one line in, one process spawned, its output
// printed. That is not a terminal — no prompt of its own, no `cd` that sticks,
// no history, no tab completion, no `ctrl-c`, no colour, no `vim`, and no way
// for a program to negotiate its output encoding.
//
// This is a real one. The shell is already running when the tab opens (a PTY
// through ConPTY on Windows, `openpty` elsewhere), its own prompt is what you
// see, and every keystroke goes straight down the wire. Nothing in the middle
// interprets or waits for Enter.

import { Terminal, type ITheme } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebLinksAddon } from '@xterm/addon-web-links';
import '@xterm/xterm/css/xterm.css';
import { listen } from '@tauri-apps/api/event';
import { api, errText } from '../core/api';
import { h, clear } from '../core/dom';
import { t } from '../core/i18n';
import { app } from '../core/state';
import { toast, popupMenu } from './toast';

type ShellRow = {
  id: string;
  label: string;
  program: string;
  args: string[];
  available: boolean;
  recommended: boolean;
};

type Session = {
  id: string;
  label: string;
  shell: string;
  term: Terminal;
  fit: FitAddon;
  wrap: HTMLElement;
  tab: HTMLElement;
  alive: boolean;
};

let hostEl: HTMLElement | null = null;
let root: HTMLElement | null = null;
let barEl: HTMLElement | null = null;
let stackEl: HTMLElement | null = null;
let emptyEl: HTMLElement | null = null;

const sessions: Session[] = [];
let activeId = '';
let shellsCache: ShellRow[] | null = null;
let opening = false;
let subscribed = false;
let observer: ResizeObserver | null = null;
let themeWatch: MutationObserver | null = null;

/* --------------------------------------------------------------- mount */

/**
 * Attach the terminal surface to a host element.
 *
 * Called every time the panel repaints. The xterm instances outlive the panel,
 * so closing and reopening the terminal keeps the shell, its scrollback and its
 * working directory — only the DOM is re-parented.
 */
export function mountTerminal(host: HTMLElement) {
  hostEl = host;
  host.classList.add('term-host');

  if (!root) build();
  if (root!.parentElement !== host) host.append(root!);
  void ensureSession();
  requestFit();
}

function build() {
  barEl = h('div', { class: 'term-bar' });
  stackEl = h('div', { class: 'term-stack' });
  emptyEl = h('div', { class: 'term-empty' }, t('terminal.starting'));
  stackEl.append(emptyEl);
  root = h(
    'div',
    { class: 'term-root' },
    barEl,
    stackEl,
  );

  observer = new ResizeObserver(() => fitActive());
  observer.observe(stackEl);

  // A theme switch rewrites CSS variables on <html>; xterm paints on its own
  // canvas and would keep the old palette until reloaded.
  themeWatch = new MutationObserver(() => retheme());
  themeWatch.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ['data-theme', 'style'],
  });

  void subscribe();

  // A page reload leaves the previous renderer's PTYs behind: they are real
  // shells that keep running with nobody reading them. The renderer is the only
  // consumer of a session, so anything already open when this surface mounts
  // for the first time is a ghost — close it.
  void api
    .termList()
    .then((open) => {
      for (const s of open) void api.termClose(s.id).catch(() => undefined);
    })
    .catch(() => undefined);
}

async function subscribe() {
  if (subscribed) return;
  subscribed = true;
  await listen<{ id: string; data: string }>('term://data', (e) => {
    const s = sessions.find((x) => x.id === e.payload.id);
    if (!s) return;
    // xterm decodes UTF-8 itself and keeps decoder state across writes, so a
    // Chinese character split over two reads is reassembled instead of
    // becoming two replacement glyphs.
    s.term.write(b64bytes(e.payload.data));
  });
  await listen<{ id: string; code: number }>('term://exit', (e) => {
    const s = sessions.find((x) => x.id === e.payload.id);
    if (!s) return;
    s.alive = false;
    s.term.write(`\r\n\x1b[38;5;244m[${t('terminal.exited')} ${e.payload.code}]\x1b[0m\r\n`);
    s.tab.classList.add('dead');
    renderBar();
  });
}

/* -------------------------------------------------------------- sessions */

async function shellRows(): Promise<ShellRow[]> {
  if (shellsCache) return shellsCache;
  try {
    shellsCache = await api.termShells();
  } catch {
    shellsCache = [];
  }
  return shellsCache;
}

async function ensureSession() {
  if (sessions.length || opening) return;
  await newSession();
}

async function newSession(shell?: string) {
  if (opening) return;
  opening = true;
  try {
    const rows = await shellRows();
    const pick =
      shell ||
      app.config?.terminal_shell ||
      rows.find((r) => r.recommended && r.available)?.id ||
      rows.find((r) => r.available)?.id ||
      undefined;

    const info = await api.termOpen({
      cwd: app.config?.workspace || undefined,
      shell: pick,
      cols: 100,
      rows: 24,
    });
    attachView(info.id, info.shell || pick || t('panel.terminal'), pick || '');
  } catch (e) {
    emptyEl?.remove();
    emptyEl = null;
    if (stackEl) {
      stackEl.append(h('div', { class: 'term-empty err' }, errText(e)));
    }
    toast(errText(e), 'err');
  } finally {
    opening = false;
  }
}

/**
 * Paint a session the core already owns.
 *
 * An extension can `createTerminal()` and `sendText()` while the panel is
 * closed. The core keeps the PTY (and its output) alive, so attaching replays
 * what happened instead of showing an empty box.
 */
export async function attachTerminal(id: string) {
  if (sessions.some((s) => s.id === id)) {
    activate(id);
    renderBar();
    return;
  }
  try {
    const view = await api.termAttach(id);
    attachView(view.id, view.shell || t('panel.terminal'), '', b64bytes(view.replay));
  } catch (e) {
    toast(errText(e), 'err');
  }
}

/** Close the UI view of a session (the shell is already gone). */
export function forgetTerminal(id: string) {
  const idx = sessions.findIndex((s) => s.id === id);
  if (idx < 0) return;
  const [s] = sessions.splice(idx, 1);
  s.term.dispose();
  s.wrap.remove();
  s.tab.remove();
  if (activeId === id) {
    const next = sessions[Math.min(idx, sessions.length - 1)];
    if (next) activate(next.id);
    else activeId = '';
  }
  renderBar();
}

function attachView(id: string, label: string, shell: string, replay?: Uint8Array) {
  if (!stackEl) return;
  const term = new Terminal({
    fontFamily: 'var(--font-mono), "Cascadia Code", Consolas, monospace',
    fontSize: 14,
    lineHeight: 1.25,
    letterSpacing: 0,
    cursorBlink: true,
    scrollback: 8000,
    allowProposedApi: true,
    theme: themeFor(),
    // The shell decides line endings; translation here would double them.
    convertEol: false,
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.loadAddon(
    new WebLinksAddon((_ev, uri) => {
      void api.openExternal(uri);
    }),
  );

  const wrap = h('div', { class: 'term-wrap' });
  const tab = h('div', { class: 'term-tab', onClick: () => activate(id) });
  const session: Session = {
    id,
    label,
    shell,
    term,
    fit,
    wrap,
    tab,
    alive: true,
  };
  sessions.push(session);
  stackEl.append(wrap);
  emptyEl?.remove();
  emptyEl = null;

  term.open(wrap);
  // Typing goes straight to the shell — this is the whole point.
  term.onData((data) => queueWrite(id, data));
  term.onResize(({ cols, rows: r }) => {
    void api.termResize(id, cols, r).catch(() => undefined);
  });
  // `ctrl+v` is not a control character: route it through the clipboard so
  // pasting a command does not print `^V`.
  term.attachCustomKeyEventHandler((e) => {
    if (e.type !== 'keydown') return true;
    const paste = (e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'v';
    if (!paste) return true;
    void navigator.clipboard
      .readText()
      .then((text) => {
        if (text) queueWrite(id, text.replace(/\r?\n/g, '\r'));
      })
      .catch(() => undefined);
    return false;
  });

  if (replay && replay.length) term.write(replay);

  activate(id);
  renderBar();
  requestFit();
}

function activate(id: string) {
  activeId = id;
  for (const s of sessions) {
    const on = s.id === id;
    s.wrap.classList.toggle('on', on);
    s.tab.classList.toggle('active', on);
    if (on) {
      s.term.focus();
      requestFit();
    }
  }
}

function closeSession(id: string, respawn = true) {
  const idx = sessions.findIndex((s) => s.id === id);
  if (idx < 0) return;
  const [s] = sessions.splice(idx, 1);
  void api.termClose(id).catch(() => undefined);
  s.term.dispose();
  s.wrap.remove();
  s.tab.remove();
  if (activeId === id) {
    const next = sessions[Math.min(idx, sessions.length - 1)];
    if (next) activate(next.id);
    else {
      activeId = '';
      if (stackEl && !stackEl.querySelector('.term-wrap') && !emptyEl) {
        emptyEl = h('div', { class: 'term-empty' }, t('terminal.starting'));
        stackEl.append(emptyEl);
      }
      // Closing the last terminal opens a fresh one, the way every other
      // editor does — except when the caller is already respawning it.
      if (respawn) void ensureSession();
    }
  }
  renderBar();
}

/** Shut every shell down (window close / explicit "kill all"). */
export function disposeTerminals() {
  for (const s of sessions) {
    void api.termClose(s.id).catch(() => undefined);
    s.term.dispose();
  }
  sessions.length = 0;
  activeId = '';
}

/* ---------------------------------------------------------------- writes */

const pending = new Map<string, string>();
let flushQueued = false;

/**
 * Batch keystrokes into one IPC round-trip.
 *
 * A fast typist or an IME commit fires many `onData` callbacks in the same
 * task; sending them separately would order them by IPC latency instead of by
 * when they were typed.
 */
function queueWrite(id: string, data: string) {
  if (!data) return;
  pending.set(id, (pending.get(id) || '') + data);
  if (flushQueued) return;
  flushQueued = true;
  queueMicrotask(() => {
    flushQueued = false;
    for (const [key, text] of pending) {
      pending.delete(key);
      void api.termWrite(key, text).catch((e) => {
        // A dead shell is normal after `exit`; anything else is worth saying.
        const s = sessions.find((x) => x.id === key);
        if (s?.alive) toast(errText(e), 'err');
      });
    }
  });
}

/* ------------------------------------------------------------------- ui */

function renderBar() {
  if (!barEl) return;
  clear(barEl);
  const tabs = h('div', { class: 'term-tabs' });
  for (const s of sessions) {
    s.tab.className = `term-tab${s.id === activeId ? ' active' : ''}${s.alive ? '' : ' dead'}`;
    clear(s.tab);
    s.tab.append(
      h('span', { class: 'term-tab-label', title: s.label }, s.label),
      h('button', {
        class: 'term-tab-x',
        title: t('common.close'),
        'aria-label': t('common.close'),
        onClick: (e: MouseEvent) => {
          e.stopPropagation();
          closeSession(s.id);
        },
      }, '×'),
    );
    tabs.append(s.tab);
  }
  barEl.append(tabs, h('div', { class: 'spacer' }));

  barEl.append(
    h('button', {
      class: 'term-btn',
      title: t('terminal.new'),
      'aria-label': t('terminal.new'),
      onClick: () => void newSession(activeSession()?.shell || undefined),
    }, '+'),
    h('button', {
      class: 'term-btn',
      title: t('terminal.shell'),
      'aria-label': t('terminal.shell'),
      onClick: (e: MouseEvent) => void pickShell(e.currentTarget as HTMLElement),
    }, '⌄'),
    h('button', {
      class: 'term-btn',
      title: t('terminal.clear'),
      'aria-label': t('terminal.clear'),
      onClick: () => activeSession()?.term.clear(),
    }, '⌫'),
    h('button', {
      class: 'term-btn',
      title: t('terminal.restart'),
      'aria-label': t('terminal.restart'),
      onClick: () => restartActive(),
    }, '↻'),
  );
}

async function pickShell(anchor: HTMLElement) {
  const rows = await shellRows();
  if (!rows.length) {
    toast(t('terminal.noShell'), 'err');
    return;
  }
  popupMenu(
    anchor,
    rows.map((r) => ({
      label: r.available ? r.label : `${r.label} — ${t('terminal.missing')}`,
      value: r.id,
      hint: r.program,
      active: r.id === (app.config?.terminal_shell || ''),
    })),
    (id) => {
      void saveShell(id);
      void newSession(id);
    },
    { side: 'above', width: 280 },
  );
}

async function saveShell(id: string) {
  if (!app.config) return;
  app.config.terminal_shell = id;
  try {
    await api.saveConfig(app.config);
  } catch {
    /* the session still opens; only the default is not remembered */
  }
}

async function restartActive() {
  const s = activeSession();
  const shell = s?.shell;
  if (s) closeSession(s.id);
  await newSession(shell);
}

function activeSession(): Session | undefined {
  return sessions.find((s) => s.id === activeId) || sessions[0];
}

/* ---------------------------------------------------------------- layout */

let fitTimer = 0;

function requestFit() {
  window.clearTimeout(fitTimer);
  fitTimer = window.setTimeout(() => fitActive(), 30);
}

function fitActive() {
  const s = activeSession();
  if (!s || !s.wrap.isConnected) return;
  const box = s.wrap.getBoundingClientRect();
  if (box.width < 20 || box.height < 20) return;
  try {
    s.fit.fit();
  } catch {
    /* the element was detached mid-frame */
  }
}

/* ----------------------------------------------------------------- theme */

const LIGHT_ANSI = {
  black: '#0b1018',
  red: '#b32d24',
  green: '#1f7a33',
  yellow: '#8a6100',
  blue: '#0369a1',
  magenta: '#7b1fa2',
  cyan: '#00697a',
  white: '#9aa6b4',
  brightBlack: '#74808f',
  brightRed: '#cf3f33',
  brightGreen: '#2f9247',
  brightYellow: '#a87700',
  brightBlue: '#1a82c4',
  brightMagenta: '#9a3fbf',
  brightCyan: '#00879a',
  brightWhite: '#3c4859',
};

const DARK_ANSI = {
  black: '#1d2530',
  red: '#f2777a',
  green: '#8ed08a',
  yellow: '#e6c07b',
  blue: '#6cb6ff',
  magenta: '#d19aff',
  cyan: '#63d6e0',
  white: '#b0bccb',
  brightBlack: '#7c8798',
  brightRed: '#ff8f92',
  brightGreen: '#a6e6a2',
  brightYellow: '#f2d494',
  brightBlue: '#8ecbff',
  brightMagenta: '#e0b4ff',
  brightCyan: '#8ae4ec',
  brightWhite: '#e3e9f2',
};

function cssv(name: string, fallback: string): string {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return v || fallback;
}

function themeFor(): ITheme {
  const dark = document.documentElement.getAttribute('data-theme') === 'dark';
  const ansi = dark ? DARK_ANSI : LIGHT_ANSI;
  return {
    background: cssv('--bg-1', dark ? '#0f131a' : '#f8f9fb'),
    foreground: cssv('--text-0', dark ? '#e3e9f2' : '#0b1018'),
    cursor: cssv('--accent', dark ? '#38bdf8' : '#0369a1'),
    cursorAccent: cssv('--bg-1', dark ? '#0f131a' : '#f8f9fb'),
    selectionBackground: cssv('--accent-soft', 'rgba(3,105,161,0.2)'),
    ...ansi,
  };
}

function retheme() {
  const theme = themeFor();
  for (const s of sessions) s.term.options.theme = theme;
}

/* -------------------------------------------------------------- helpers */

function b64bytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i += 1) out[i] = bin.charCodeAt(i);
  return out;
}

/** Open the terminal panel with a shell ready to receive `sendText`. */
export function terminalFocus() {
  requestFit();
  activeSession()?.term.focus();
}

/** Another shell tab (Ctrl+Shift+`). */
export function newTerminal(shell?: string) {
  void newSession(shell);
}

/** Clear the scrollback, leaving the shell and its state untouched. */
export function clearTerminal() {
  activeSession()?.term.clear();
}
