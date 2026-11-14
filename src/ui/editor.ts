// CodeMirror 6 editor host: tabs, dirty tracking, save, language detection.

import { EditorState } from '@codemirror/state';
import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection, hoverTooltip } from '@codemirror/view';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { bracketMatching, indentOnInput, syntaxHighlighting, HighlightStyle, StreamLanguage } from '@codemirror/language';
import { tags as tg } from '@lezer/highlight';
import { javascript } from '@codemirror/lang-javascript';
import { json } from '@codemirror/lang-json';
import { markdown } from '@codemirror/lang-markdown';
import { python } from '@codemirror/lang-python';
import { html } from '@codemirror/lang-html';
import { css as cssLang } from '@codemirror/lang-css';
import { rust } from '@codemirror/lang-rust';
import { go } from '@codemirror/lang-go';
import { cpp } from '@codemirror/lang-cpp';
import { java } from '@codemirror/lang-java';
import { php } from '@codemirror/lang-php';
import { sql } from '@codemirror/lang-sql';
import { xml } from '@codemirror/lang-xml';
import { yaml } from '@codemirror/lang-yaml';
import { shell } from '@codemirror/legacy-modes/mode/shell';
import { ruby } from '@codemirror/legacy-modes/mode/ruby';
import { lua } from '@codemirror/legacy-modes/mode/lua';
import { perl } from '@codemirror/legacy-modes/mode/perl';
import { toml } from '@codemirror/legacy-modes/mode/toml';
import { properties } from '@codemirror/legacy-modes/mode/properties';
import { powerShell } from '@codemirror/legacy-modes/mode/powershell';
import { dockerFile } from '@codemirror/legacy-modes/mode/dockerfile';
import { csharp, kotlin, dart } from '@codemirror/legacy-modes/mode/clike';
import { swift } from '@codemirror/legacy-modes/mode/swift';
import { autocompletion, type Completion, type CompletionContext, type CompletionResult } from '@codemirror/autocomplete';
import { app, activeFile, emit, on } from '../core/state';
import { api } from '../core/api';
import { extMenuRows, isExtMenuValue } from '../core/extmenus';
import { h, basename, clear } from '../core/dom';
import { icon, mark } from './icons';
import { t } from '../core/i18n';
import { toast, confirmDialog, popupMenu, promptDialog } from './toast';
import { LANGUAGES, detectLanguage, languageLabel } from '../core/languages';
import { renderMarkdown } from './markdown';
import type { OpenFile } from '../core/state';
import type { SnippetRow } from '../types';

/* ------------------------------------------------- extension snippets */

const snippetCache = new Map<string, SnippetRow[]>();

/** Installed extensions changed — drop cached snippets. */
function resetSnippetCache() {
  snippetCache.clear();
}

async function snippetsFor(lang: string): Promise<SnippetRow[]> {
  const hit = snippetCache.get(lang);
  if (hit) return hit;
  let rows: SnippetRow[] = [];
  try {
    rows = await api.extensionsSnippets(lang);
  } catch {
    rows = [];
  }
  snippetCache.set(lang, rows);
  return rows;
}

/** Expand a VS Code snippet body: drop tabstops/choices, keep the default text. */
function expandSnippet(body: string, indent: string): string {
  return body
    // ${1|a,b|} → a
    .replace(/\$\{\d+\|([^|]*)\|[^}]*\}/g, (_m, first: string) => first.split(',')[0] ?? '')
    // ${1:label} → label
    .replace(/\$\{\d+:([^}]*)\}/g, '$1')
    // ${TM_FILENAME}, ${1} → ''
    .replace(/\$\{\d+\}/g, '')
    .replace(/\$(\d+)/g, '')
    .replace(/\$\{TM_[A-Z_]+\}/g, '')
    // re-indent continuation lines to where the snippet started
    .split('\n')
    .map((line, i) => (i === 0 ? line : indent + line))
    .join('\n');
}

async function snippetSource(ctx: CompletionContext): Promise<CompletionResult | null> {
  if (!ctx.explicit) {
    const word = ctx.matchBefore(/[A-Za-z_][\w-]{1,}$/);
    if (!word) return null;
  }
  const cur = activeFile();
  if (!cur) return null;
  const lang = cur.lang;
  const rows = await snippetsFor(lang);
  if (rows.length === 0) return null;

  const word = ctx.matchBefore(/[A-Za-z_][\w-]*/);
  const from = word ? word.from : ctx.pos;
  const typed = (word?.text || '').toLowerCase();

  const options = rows
    .filter((r) => !typed || r.prefix.toLowerCase().startsWith(typed))
    .slice(0, 40)
    .map((r) => ({
      label: r.prefix,
      detail: r.ext,
      info: r.description || undefined,
      type: 'keyword',
      apply: (view: EditorView, _c: unknown, applyFrom: number, applyTo: number) => {
        const line = view.state.doc.lineAt(applyFrom);
        const indent = /^\s*/.exec(line.text)?.[0] ?? '';
        const text = expandSnippet(r.body, indent);
        view.dispatch({
          changes: { from: applyFrom, to: applyTo, insert: text },
          selection: { anchor: applyFrom + text.length },
        });
      },
    }));

  if (options.length === 0) return null;
  return { from, options };
}

/* --------------------------------------------- extension language features */
//
// Completion, hover and formatting come from real extensions through the Node
// host. Nothing here re-implements a language server: it forwards the buffer and
// the cursor, and applies whatever edits come back.

function hostsWithProviders(): boolean {
  return app.hosts.some((h) => h.state === 'running');
}

function completionType(kind: unknown): string {
  // Map VS Code's CompletionItemKind onto CodeMirror's icon buckets.
  const n = typeof kind === 'number' ? kind : 0;
  if ([2, 3, 4].includes(n)) return 'function';
  if ([5, 6, 7, 8, 9].includes(n)) return 'variable';
  if ([10, 11, 12, 13].includes(n)) return 'property';
  if (n === 14) return 'text';
  if (n === 15) return 'keyword';
  if (n === 16 || n === 17) return 'text';
  if (n === 20) return 'constant';
  if (n === 21) return 'type';
  if (n === 22) return 'class';
  return 'text';
}

function lineCol(view: EditorView, pos: number) {
  const line = view.state.doc.lineAt(pos);
  return { line: line.number - 1, character: pos - line.from };
}

async function extCompletionSource(ctx: CompletionContext): Promise<CompletionResult | null> {
  const file = activeFile();
  if (!file || !hostsWithProviders()) return null;
  const word = ctx.matchBefore(/[\w$.\-]*/);
  if (!ctx.explicit && (!word || word.from === ctx.pos)) return null;
  const at = lineCol(ctx.view ?? view!, ctx.pos);
  let rows: { ext: string; value: unknown }[] = [];
  try {
    rows = await api.extLanguage({
      kind: 'completion',
      path: file.path,
      language: file.lang,
      text: ctx.state.doc.toString(),
      line: at.line,
      character: at.character,
    });
  } catch {
    return null;
  }
  const options: Completion[] = [];
  for (const row of rows) {
    const value = row.value as { items?: unknown[] } | unknown[] | null;
    const list = Array.isArray(value) ? value : value?.items;
    if (!Array.isArray(list)) continue;
    for (const raw of list as Record<string, any>[]) {
      const label = typeof raw?.label === 'string' ? raw.label : raw?.label?.label;
      if (!label) continue;
      const insert =
        typeof raw.insertText === 'string'
          ? raw.insertText
          : typeof raw.textEdit?.newText === 'string'
            ? raw.textEdit.newText
            : label;
      const doc = raw.documentation;
      options.push({
        label,
        detail: typeof raw.detail === 'string' ? raw.detail : undefined,
        info: typeof doc === 'string' ? doc : doc?.value,
        type: completionType(raw.kind),
        apply: insert.replace(/\$\{\d+:?([^}]*)\}/g, '$1').replace(/\$\d+/g, ''),
      });
    }
  }
  if (options.length === 0) return null;
  return { from: word ? word.from : ctx.pos, options: options.slice(0, 300), validFor: /^[\w$.\-]*$/ };
}

const extHover = hoverTooltip(
  async (v, pos) => {
    const file = activeFile();
    if (!file || !hostsWithProviders()) return null;
    const at = lineCol(v, pos);
    let rows: { ext: string; value: unknown }[] = [];
    try {
      rows = await api.extLanguage({
        kind: 'hover',
        path: file.path,
        language: file.lang,
        text: v.state.doc.toString(),
        line: at.line,
        character: at.character,
      });
    } catch {
      return null;
    }
    const parts: string[] = [];
    for (const row of rows) {
      const value = row.value as Record<string, any> | null;
      if (!value) continue;
      const c = value.contents;
      if (typeof c === 'string') parts.push(c);
      else if (Array.isArray(c)) {
        for (const piece of c) parts.push(typeof piece === 'string' ? piece : piece?.value ?? '');
      } else if (c?.value) parts.push(c.value);
    }
    if (parts.length === 0) return null;
    return {
      pos,
      create: () => {
        const dom = h('div', { class: 'cm-hover-doc' });
        dom.append(renderMarkdown(parts.join('\n\n')));
        return { dom };
      },
    };
  },
  { hoverTime: 350 },
);

/** Format the buffer through a registered provider (Prettier and friends). */
export async function formatDocument(): Promise<void> {
  const file = activeFile();
  if (!file || !view) return;
  if (!hostsWithProviders()) {
    toast(t('editor.noFormatter'), 'info', 1600);
    return;
  }
  let rows: { ext: string; value: unknown }[] = [];
  try {
    rows = await api.extLanguage({
      kind: 'format',
      path: file.path,
      language: file.lang,
      text: view.state.doc.toString(),
      options: { tabSize: 2, insertSpaces: true },
    });
  } catch (e) {
    toast(String(e), 'err');
    return;
  }
  const doc = view.state.doc;
  const offset = (p: { line: number; character: number }) => {
    const line = doc.line(Math.min(Math.max(1, p.line + 1), doc.lines));
    return Math.min(line.from + p.character, line.to);
  };
  for (const row of rows) {
    const edits = row.value as { range?: any; newText?: string }[] | null;
    if (!Array.isArray(edits) || edits.length === 0) continue;
    const changes = edits
      .filter((e) => e && e.range && e.range.start && e.range.end)
      .map((e) => ({ from: offset(e.range.start), to: offset(e.range.end), insert: e.newText ?? '' }))
      .sort((a, b) => b.from - a.from);
    if (!changes.length) continue;
    view.dispatch({ changes });
    file.content = view.state.doc.toString();
    toast(`${t('editor.formatted')} · ${row.ext}`, 'ok', 1400);
    return;
  }
  toast(t('editor.noFormatter'), 'info', 1600);
}

const theme = EditorView.theme({
  '&': { backgroundColor: 'var(--bg-0)', color: 'var(--text-0)', height: '100%' },
  '.cm-content': { padding: '8px 0', caretColor: 'var(--accent)' },
  '.cm-line': { padding: '0 12px 0 6px' },
  '.cm-gutters': { backgroundColor: 'var(--bg-0)', color: 'var(--text-2)', border: 'none', paddingLeft: '4px' },
  '.cm-activeLine': { backgroundColor: 'rgba(56,189,248,0.045)' },
  '.cm-activeLineGutter': { backgroundColor: 'transparent', color: 'var(--text-1)' },
  '.cm-selectionBackground, ::selection': { backgroundColor: 'var(--accent-soft) !important' },
  '.cm-cursor': { borderLeftColor: 'var(--accent)' },
  '.cm-panels': { backgroundColor: 'var(--bg-1)', color: 'var(--text-1)' },
  '.cm-tooltip': { backgroundColor: 'var(--bg-1)', border: '1px solid var(--border-strong)' },
});

const highlight = HighlightStyle.define([
  { tag: tg.keyword, color: 'var(--accent-2)' },
  { tag: [tg.name, tg.deleted, tg.character, tg.propertyName, tg.macroName], color: 'var(--text-0)' },
  { tag: [tg.function(tg.variableName), tg.labelName], color: 'var(--accent)' },
  { tag: [tg.color, tg.constant(tg.name), tg.standard(tg.name)], color: 'var(--warn)' },
  { tag: [tg.definition(tg.name), tg.separator], color: 'var(--text-1)' },
  { tag: [tg.typeName, tg.className, tg.number, tg.changed, tg.annotation, tg.modifier, tg.self, tg.namespace], color: 'var(--warn)' },
  { tag: [tg.operator, tg.operatorKeyword, tg.url, tg.escape, tg.regexp, tg.link], color: 'var(--text-1)' },
  { tag: [tg.meta, tg.comment, tg.strong], color: 'var(--text-2)', fontStyle: 'italic' },
  { tag: [tg.string, tg.inserted], color: 'var(--ok)' },
  { tag: tg.invalid, color: 'var(--err)' },
  { tag: [tg.heading, tg.emphasis], fontWeight: '600', color: 'var(--text-0)' },
]);

let view: EditorView | null = null;
let hostEl: HTMLElement | null = null;
let tabsEl: HTMLElement | null = null;

/* ------------------------------------------------- extension host sync */
//
// `window.activeTextEditor` in an extension is only worth having if it reflects
// this editor. The host process keeps its own copy, so every open / change /
// save / close is published — throttled, because a keystroke is not a
// round-trip's worth of news, and skipped entirely when no host is running.

let docVersion = 1;
let syncTimer: number | undefined;

function hostRunning(): boolean {
  return app.hosts.some((h) => h.state === 'running' || h.state === 'starting');
}

function selectionOf(): { start: { line: number; character: number }; end: { line: number; character: number } } {
  const fallback = { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } };
  if (!view) return fallback;
  const sel = view.state.selection.main;
  const a = view.state.doc.lineAt(sel.from);
  const b = view.state.doc.lineAt(sel.to);
  return {
    start: { line: a.number - 1, character: sel.from - a.from },
    end: { line: b.number - 1, character: sel.to - b.from },
  };
}

/** Publish the current file to every running host. `kind` is the event name. */
export function publishDoc(kind: 'open' | 'change' | 'save' | 'close' | 'active', file?: OpenFile) {
  const target = file || activeFile();
  if (!target) return;
  if (!hostRunning()) return;
  // A 2 MB source file is not worth shipping on every keystroke; extensions get
  // the head of it, which is what editors do with huge buffers too.
  const text = target.content.length > 2_000_000 ? target.content.slice(0, 2_000_000) : target.content;
  void api
    .exthostDoc({
      kind,
      path: target.path,
      language: target.lang === 'text' ? 'plaintext' : target.lang,
      text,
      dirty: target.dirty,
      version: ++docVersion,
      selection: selectionOf(),
    })
    .catch(() => undefined);
}

function scheduleSync() {
  if (syncTimer !== undefined) return;
  syncTimer = window.setTimeout(() => {
    syncTimer = undefined;
    publishDoc('change');
  }, 220);
}

function langExtension(lang: string) {
  switch (lang) {
    case 'javascript':
      return javascript({ jsx: false });
    case 'javascriptreact':
      return javascript({ jsx: true });
    case 'typescript':
      return javascript({ typescript: true });
    case 'typescriptreact':
      return javascript({ typescript: true, jsx: true });
    case 'json':
    case 'jsonc':
      return json();
    case 'markdown':
      return markdown();
    case 'python':
      return python();
    case 'html':
    case 'vue':
    case 'svelte':
      return html();
    case 'css':
    case 'scss':
    case 'less':
      return cssLang();
    case 'rust':
      return rust();
    case 'go':
      return go();
    case 'c':
    case 'cpp':
      return cpp();
    case 'java':
      return java();
    case 'php':
      return php();
    case 'sql':
      return sql();
    case 'xml':
      return xml();
    case 'yaml':
      return yaml();
    case 'toml':
      return StreamLanguage.define(toml);
    case 'shellscript':
      return StreamLanguage.define(shell);
    case 'powershell':
      return StreamLanguage.define(powerShell);
    case 'ruby':
      return StreamLanguage.define(ruby);
    case 'perl':
      return StreamLanguage.define(perl);
    case 'lua':
      return StreamLanguage.define(lua);
    case 'ini':
      return StreamLanguage.define(properties);
    case 'dockerfile':
      return StreamLanguage.define(dockerFile);
    case 'csharp':
      return StreamLanguage.define(csharp);
    case 'kotlin':
      return StreamLanguage.define(kotlin);
    case 'swift':
      return StreamLanguage.define(swift);
    case 'dart':
      return StreamLanguage.define(dart);
    default:
      return [];
  }
}

/** Kept for callers that only have a file name; the catalogue owns the table. */
export function detectLang(name: string): string {
  return detectLanguage(name);
}

export function mountEditor(container: HTMLElement) {
  on('extensions:count', resetSnippetCache);
  on('run:event', (ev: any) => handleRunEvent(ev));
  hostEl = h('div', { class: 'editor-host' });
  tabsEl = h('div', { class: 'tabs' });
  // Outside the scroller on purpose: the tab strip scrolls horizontally, and a
  // "back to chat" button that scrolls away is not a button.
  const back = h('button', {
    class: 'icon-btn tabs-back',
    title: t('editor.backToDock'),
    onClick: () => emit('editor:cover', false),
  }, icon('chat', 13));
  container.append(h('div', { class: 'tabs-bar' }, tabsEl, back), hostEl);
  renderTabs();
  renderEditor();
}

function renderTabs() {
  if (!tabsEl) return;
  clear(tabsEl);
  for (const f of app.files) {
    const active = f.path === app.activeFile;
    const closeBtn = h('div', {
      class: 'tab-x',
      title: t('common.close'),
      onClick: (e: Event) => {
        e.stopPropagation();
        closeFile(f.path);
      },
    });
    closeBtn.append(icon('close', 12));
    const tab = h(
      'div',
      { class: `tab ${active ? 'active' : ''}`, onClick: () => openFile(f.path), title: f.path },
      f.dirty ? h('span', { class: 'tab-dot' }) : null,
      h('span', { class: 'tab-name' }, f.name),
      closeBtn,
    );
    tabsEl.append(tab);
  }

  // The rail shows a "文件" button only while tabs exist, and the stage falls
  // back to the dock once the last tab closes — both hang off this event.
  tabsEl.append(h('div', { class: 'tabs-fill' }));
  emit('editor:tabs');
}

export async function openFile(path: string, line?: number) {
  let file = app.files.find((f) => f.path === path);
  if (!file) {
    try {
      const content = await api.read(path);
      file = {
        path,
        name: basename(path),
        content,
        saved: content,
        dirty: false,
        // An explicit pick wins over the extension: that is the whole point of
        // "set language for this file".
        lang: app.config?.file_languages?.[path] || detectLanguage(path),
      };
      app.files.push(file);
    } catch (e) {
      toast(String(e), 'err');
      return;
    }
  }
  app.activeFile = path;
  renderTabs();
  renderEditor(line);
  // Opening a file covers the dock (the "file view replaces the chat" rule).
  emit('editor:cover', true);
}

function closeFile(path: string) {
  const idx = app.files.findIndex((f) => f.path === path);
  if (idx < 0) return;
  publishDoc('close', app.files[idx]);
  app.files.splice(idx, 1);
  if (app.activeFile === path) {
    app.activeFile = app.files[Math.min(idx, app.files.length - 1)]?.path || '';
  }
  renderTabs();
  renderEditor();
}

export function closeActiveFile() {
  if (app.activeFile) closeFile(app.activeFile);
}

function renderEditor(scrollToLine?: number) {
  if (!hostEl) return;
  const file = activeFile();
  if (view) {
    view.destroy();
    view = null;
  }
  clear(hostEl);

  if (!file) {
    hostEl.append(welcome());
    return;
  }

  const host = h('div', { class: 'editor-cm' });
  hostEl.append(host);
  hostEl.append(editorFoot(file));

  const state = EditorState.create({
    doc: file.content,
    extensions: [
      lineNumbers(),
      highlightActiveLineGutter(),
      highlightActiveLine(),
      drawSelection(),
      history(),
      bracketMatching(),
      indentOnInput(),
      syntaxHighlighting(highlight),
      theme,
      EditorView.lineWrapping,
      langExtension(file.lang),
      // Snippets contributed by installed VS Code extensions show up as you
      // type the prefix (Ctrl+Space forces the list).
      autocompletion({ override: [snippetSource, extCompletionSource], activateOnTyping: true }),
      extHover,
      keymap.of([
        {
          key: 'Mod-s',
          preventDefault: true,
          run: () => {
            void saveActive();
            return true;
          },
        },
        {
          key: 'Mod-Shift-i',
          preventDefault: true,
          run: () => {
            void formatDocument();
            return true;
          },
        },
        {
          key: 'F5',
          preventDefault: true,
          run: () => {
            const f = activeFile();
            if (f) void toggleRun(f);
            return true;
          },
        },
        ...defaultKeymap,
        ...historyKeymap,
        indentWithTab,
      ]),
      EditorView.updateListener.of((u) => {
        if (u.selectionSet || u.docChanged) updateCursorPos(u.state);
        if (!u.docChanged) return;
        const cur = activeFile();
        if (!cur) return;
        cur.content = u.state.doc.toString();
        const dirty = cur.content !== cur.saved;
        if (dirty !== cur.dirty) {
          cur.dirty = dirty;
          renderTabs();
        }
        emit('editor:dirty', cur.path);
        scheduleSync();
      }),
      EditorView.domEventHandlers({
        keydown: (e) => {
          if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
            e.preventDefault();
            void saveActive();
            return true;
          }
          return false;
        },
        // Right-click: our own menu (cut/copy/paste) plus whatever extensions
        // contribute to `editor/context`. Without this the webview shows either
        // nothing or its own browser menu, and extension actions were unreachable.
        contextmenu: (e) => {
          e.preventDefault();
          openEditorMenu(e.clientX, e.clientY);
          return true;
        },
      }),
    ],
  });

  view = new EditorView({ state, parent: host });
  updateCursorPos(state);
  publishDoc('open', file);
  if (scrollToLine) {
    const line = view.state.doc.line(Math.min(scrollToLine, view.state.doc.lines));
    view.dispatch({ selection: { anchor: line.from }, scrollIntoView: true });
    view.focus();
  } else {
    view.focus();
  }
  emit('editor:file', file.path);
}

/* ------------------------------------------------- language mode + runner */

/**
 * Editor right-click menu: the four editing actions every text surface needs,
 * then the rows extensions declared for `editor/context`.
 */
function openEditorMenu(x: number, y: number) {
  const file = activeFile();
  const v = view;
  if (!file || !v) return;
  const items = [
    { label: t('common.cut'), value: 'cut' },
    { label: t('common.copy'), value: 'copy' },
    { label: t('common.paste'), value: 'paste' },
    { label: t('common.selectAll'), value: 'selectAll' },
    ...extMenuRows('editor/context', { path: file.path, lang: file.lang, editor: true }),
  ];
  popupMenu(
    document.body,
    items,
    async (value) => {
      if (isExtMenuValue(value)) {
        try {
          await api.exthostRun(value.slice(4));
        } catch (err) {
          toast(String(err), 'err', 3200);
        }
        return;
      }
      const hasSelection = !v.state.selection.main.empty;
      if (value === 'selectAll') {
        v.dispatch({ selection: { anchor: 0, head: v.state.doc.length } });
        v.focus();
      } else if (value === 'copy') {
        if (hasSelection) document.execCommand('copy');
      } else if (value === 'cut') {
        if (hasSelection) document.execCommand('cut');
      } else if (value === 'paste') {
        try {
          const text = await navigator.clipboard.readText();
          if (text) v.dispatch(v.state.replaceSelection(text));
          v.focus();
        } catch {
          toast(t('editor.pasteBlocked'), 'err', 3200);
        }
      }
    },
    { search: false, at: { x, y } },
  );
}

let footLang: HTMLButtonElement | null = null;
let footPos: HTMLElement | null = null;
let footRunner: HTMLButtonElement | null = null;
let footRun: HTMLButtonElement | null = null;
let runningId: string | null = null;

interface RunnerRow {
  id: string;
  label: string;
  command: string;
  args: string;
  available: boolean;
}

const runnerCache = new Map<string, RunnerRow[]>();

/** Default table, mirrored for the button label before the backend answers. */
const RUNNER_HINT: Record<string, string> = {
  python: 'python -u {file}',
  javascript: 'node {file}',
  javascriptreact: 'node {file}',
  typescript: 'node --experimental-strip-types {file}',
  typescriptreact: 'node --experimental-strip-types {file}',
  go: 'go run {file}',
  rust: 'cargo run',
  java: 'java {file}',
  c: 'cc {file}',
  cpp: 'c++ {file}',
  php: 'php {file}',
  ruby: 'ruby {file}',
  perl: 'perl {file}',
  lua: 'lua {file}',
  r: 'Rscript {file}',
  dart: 'dart run {file}',
  shellscript: 'bash {file}',
  powershell: 'powershell -File {file}',
  bat: 'cmd /C {file}',
};

function runnerLabelFor(lang: string): string {
  const saved = app.config?.runners?.[lang];
  if (saved?.command) return `${saved.command} ${saved.args || '{file}'}`;
  return RUNNER_HINT[lang] || t('editor.noRunner');
}

async function runnerList(lang: string): Promise<RunnerRow[]> {
  const hit = runnerCache.get(lang);
  if (hit) return hit;
  let rows: RunnerRow[] = [];
  try {
    rows = await api.runnerOptions(lang);
  } catch {
    rows = [];
  }
  runnerCache.set(lang, rows);
  return rows;
}

function updateCursorPos(state: EditorState) {
  if (!footPos) return;
  const sel = state.selection.main;
  const line = state.doc.lineAt(sel.head);
  footPos.textContent = `Ln ${line.number}, Col ${sel.head - line.from + 1}`;
}

function editorFoot(file: OpenFile): HTMLElement {
  footLang = h(
    'button',
    { class: 'ef-btn', title: t('editor.language'), onClick: () => pickLanguage(file) },
    languageLabel(file.lang),
  );
  footPos = h('span', { class: 'ef-pos' }, 'Ln 1, Col 1');
  footRunner = h(
    'button',
    { class: 'ef-btn ef-runner', title: t('editor.runner'), onClick: () => void pickRunner(file) },
    runnerLabelFor(file.lang),
  );
  footRun = h(
    'button',
    { class: 'ef-btn ef-run', title: `${t('editor.run')} (Ctrl+F5)`, onClick: () => void toggleRun(file) },
    icon('play', 11),
    h('span', {}, t('editor.run')),
  );
  return h(
    'div',
    { class: 'editor-foot' },
    footLang,
    footPos,
    h('span', { class: 'ef-fill' }),
    footRunner,
    footRun,
  );
}

function pickLanguage(file: OpenFile) {
  if (!footLang) return;
  const items = LANGUAGES.map((l) => ({
    label: l.label,
    value: l.id,
    hint: l.exts.slice(0, 3).join(' '),
    active: l.id === file.lang,
  }));
  popupMenu(
    footLang,
    items,
    (id) => void setLanguage(file, id),
    { search: true, width: 300, side: 'above', placeholder: t('editor.language') },
  );
}

async function setLanguage(file: OpenFile, lang: string) {
  file.lang = lang;
  if (app.config) {
    app.config.file_languages = { ...(app.config.file_languages || {}), [file.path]: lang };
  }
  try {
    await api.setFileLanguage(file.path, lang);
  } catch (e) {
    toast(String(e), 'err');
  }
  runnerCache.delete(lang);
  // Rebuild the view so the new grammar (and its indentation rules) apply.
  renderEditor();
  // Extensions match documents by language id, so they must be told too.
  publishDoc('open', file);
}

async function pickRunner(file: OpenFile) {
  if (!footRunner) return;
  const rows = await runnerList(file.lang);
  const saved = app.config?.runners?.[file.lang];
  const items = rows.map((r) => ({
    label: r.label,
    value: `use:${r.id}`,
    hint: r.available ? `${r.command} ${r.args}` : t('editor.runnerMissing'),
    active: !!saved && saved.command === r.command,
  }));
  items.push({ label: t('editor.customRunner'), value: 'custom', hint: 'command {file}', active: false });
  if (saved?.command) {
    items.push({ label: t('editor.resetRunner'), value: 'reset', hint: RUNNER_HINT[file.lang] || '', active: false });
  }
  popupMenu(footRunner, items, async (value) => {
    try {
      if (value === 'custom') {
        const current = saved?.command ? `${saved.command} ${saved.args || '{file}'}` : RUNNER_HINT[file.lang] || '';
        const line = await promptDialog(t('editor.customRunner'), current);
        if (!line) return;
        const sp = line.indexOf(' ');
        const command = sp < 0 ? line : line.slice(0, sp);
        const args = sp < 0 ? '{file}' : line.slice(sp + 1);
        await api.runnerSave(file.lang, command, args);
      } else if (value === 'reset') {
        await api.runnerReset(file.lang);
      } else {
        const row = rows.find((r) => `use:${r.id}` === value);
        if (!row) return;
        await api.runnerSave(file.lang, row.command, row.args);
      }
      const cfg = await api.getConfig();
      app.config = cfg;
      runnerCache.delete(file.lang);
      if (footRunner) footRunner.textContent = runnerLabelFor(file.lang);
    } catch (e) {
      toast(String(e), 'err');
    }
  }, { width: 380, side: 'above' });
}

async function toggleRun(file: OpenFile) {
  if (runningId) {
    const id = runningId;
    runningId = null;
    try {
      await api.runStop(id);
    } catch {
      /* already exited */
    }
    return;
  }
  // A runner executes what is on disk; an unsaved buffer would silently run
  // the previous version.
  if (file.dirty) await saveActive();
  if (!app.terminal.length || app.terminal[app.terminal.length - 1] !== '') app.terminal.push('');
  try {
    const info = await api.runFile(file.path, file.lang);
    runningId = info.id;
    if (footRun) {
      footRun.textContent = '';
      footRun.append(icon('stop', 11), h('span', {}, t('editor.stop')));
      footRun.classList.add('busy');
    }
    // A run writes into the output tab, so that is the tab that must be in
    // front — otherwise F5 looks like it did nothing.
    app.panelTab = 'output';
    emit('panel:open');
  } catch (e) {
    app.terminal.push(`! ${String(e)}`);
    app.panelTab = 'output';
    emit('panel:refresh');
    emit('panel:open');
  }
}

function handleRunEvent(ev: {
  run: string;
  kind: 'start' | 'chunk' | 'exit';
  command?: string;
  cwd?: string;
  stream?: 'stdout' | 'stderr';
  text?: string;
  code?: number;
}) {
  if (ev.kind === 'start') {
    app.terminal.push(`$ ${ev.command || ''}`);
    if (ev.cwd) app.terminal.push(`# ${ev.cwd}`);
  } else if (ev.kind === 'chunk') {
    const line = ev.text ?? '';
    app.terminal.push(ev.stream === 'stderr' ? `! ${line}` : line);
  } else {
    app.terminal.push(`[${t('editor.exitCode')} ${ev.code ?? -1}]`);
    if (runningId === ev.run) {
      runningId = null;
      if (footRun) {
        footRun.textContent = '';
        footRun.append(icon('play', 11), h('span', {}, t('editor.run')));
        footRun.classList.remove('busy');
      }
    }
  }
  if (app.terminal.length > 4000) app.terminal.splice(0, app.terminal.length - 4000);
  emit('panel:refresh');
}

export function chooseLanguage() {
  const f = activeFile();
  if (f) pickLanguage(f);
}

export function runActiveFile() {
  const f = activeFile();
  if (f) void toggleRun(f);
}

export async function saveActive(): Promise<boolean> {
  const file = activeFile();
  if (!file) return false;
  try {
    await api.write(file.path, file.content);
    file.saved = file.content;
    file.dirty = false;
    renderTabs();
    publishDoc('save', file);
    emit('editor:dirty', file.path);
    toast(`${t('editor.saved')}: ${file.name}`, 'ok', 1400);
    return true;
  } catch (e) {
    toast(String(e), 'err');
    return false;
  }
}

export async function openFileAtLine(path: string, line: number) {
  await openFile(path);
  const file = activeFile();
  if (!file || !view) return;
  const target = view.state.doc.line(Math.min(Math.max(1, line), view.state.doc.lines));
  view.dispatch({ selection: { anchor: target.from }, scrollIntoView: true, effects: [] });
  view.focus();
}

export function refreshEditor() {
  renderTabs();
  renderEditor();
}

/**
 * Apply edits an extension asked for (`TextEditor.edit` / `WorkspaceEdit`).
 *
 * Ranges are expressed against the document the *host* knows, which is the one
 * it was last sent. They are converted to offsets here and applied bottom-up in
 * a single CodeMirror transaction so overlapping edits cannot shift each other.
 */
export async function applyExternalEdits(
  path: string,
  edits: { start: { line: number; character: number }; end: { line: number; character: number }; text: string }[],
): Promise<void> {
  if (!edits.length) return;
  const file = app.files.find((f) => f.path === path);
  const source = file ? file.content : await api.read(path);
  const lines = source.split('\n');

  const offsetOf = (pos: { line: number; character: number }) => {
    let off = 0;
    for (let i = 0; i < Math.min(pos.line, lines.length); i++) off += lines[i].length + 1;
    const line = lines[Math.min(pos.line, lines.length - 1)] || '';
    return off + Math.min(pos.character, line.length);
  };

  const ops = edits
    .map((e) => ({ from: offsetOf(e.start), to: offsetOf(e.end), text: e.text }))
    .sort((a, b) => b.from - a.from);

  if (!file) {
    let text = source;
    for (const op of ops) text = text.slice(0, op.from) + op.text + text.slice(op.to);
    await api.write(path, text);
    return;
  }

  if (file.path === app.activeFile && view) {
    view.dispatch({ changes: ops.map((op) => ({ from: op.from, to: op.to, insert: op.text })) });
    view.focus();
    return;
  }

  let text = source;
  for (const op of ops) text = text.slice(0, op.from) + op.text + text.slice(op.to);
  file.content = text;
  file.dirty = file.content !== file.saved;
  renderTabs();
  emit('editor:dirty', file.path);
}

function welcome(): HTMLElement {
  return h(
    'div',
    { class: 'welcome' },
    mark(96),
    h('h2', {}, t('editor.welcome')),
    h('p', {}, t('editor.welcomeHint')),
    h(
      'div',
      { class: 'keys' },
      h('div', {}, h('span', { class: 'kbd' }, 'Ctrl'), h('span', { class: 'kbd' }, 'S'), h('span', {}, t('editor.save'))),
      h('div', {}, h('span', { class: 'kbd' }, 'Ctrl'), h('span', { class: 'kbd' }, 'B'), h('span', {}, t('editor.toggleChat'))),
      h('div', {}, h('span', { class: 'kbd' }, 'Ctrl'), h('span', { class: 'kbd' }, 'J'), h('span', {}, t('panel.terminal'))),
    ),
  );
}
