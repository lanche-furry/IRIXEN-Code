// Agent chat panel — streaming transcript, reasoning blocks, tool cards, composer.

import { h, clear, basename, fmtNum } from '../core/dom';
import { icon } from './icons';
import { t } from '../core/i18n';
import { api, errText, onAgentEvent, type AttachmentRef } from '../core/api';
import { app, emit, setStatus, type ChatItem, type ChatTurn } from '../core/state';
import { pickDefaultProvider, providerBlocker, providerReady } from '../core/providers';
import { effortChoices, effortLabel, effortLevel } from '../core/effort';
import { renderMarkdown } from './markdown';
import { toast, popupMenu } from './toast';
import { openHistory } from './history';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import type { AgentEvent, BridgeStatus, Effort, Message, ModelInfo, ProviderConfig } from '../types';

/* ------------------------------------------------------------------ labels */

const THINK_HINT: Record<string, string> = {
  unknown: '',
  unsupported: '',
  always_on: 'always-on',
  effort: 'effort',
  budget: 'budget',
};

/** Three ascending bars — the composer's at-a-glance effort gauge.
 *  auto → all bars muted (the server decides), off → none, levels → accent. */
function effortMeter(level: Effort): HTMLElement {
  const n = effortLevel(level);
  const el = h('span', {
    class: `effort-meter${level === 'off' ? ' is-off' : ''}${level === 'auto' ? ' is-auto' : ''}`,
  });
  for (let i = 1; i <= 3; i++) el.append(h('i', { class: i <= n ? 'on' : '' }));
  return el;
}

/* ------------------------------------------------------------------- state */

interface ToolView {
  card: HTMLElement;
  body: HTMLElement;
  dot: HTMLElement;
  args: HTMLElement;
}

interface MsgView {
  root: HTMLElement;
  text: HTMLElement;
  reason: HTMLElement | null;
  note: HTMLElement | null;
  tools: Map<string, ToolView>;
  /** Footer offering "undo this turn"; only assistant turns that wrote files. */
  turnBar: HTMLElement | null;
}

const views = new Map<string, MsgView>();

let logEl: HTMLElement | null = null;
let inputEl: HTMLTextAreaElement | null = null;
let headEl: HTMLElement | null = null;
let toolbarEl: HTMLElement | null = null;
let attachRowEl: HTMLElement | null = null;
let hintEl: HTMLElement | null = null;
let sendBtn: HTMLButtonElement | null = null;

let attachments: AttachmentRef[] = [];
let streamingId: string | null = null;
let seq = 0;
let unlisten: (() => void) | null = null;
let stick = true;

function nextId(prefix: string): string {
  seq += 1;
  return `${prefix}-${Date.now().toString(36)}-${seq}`;
}

/* ------------------------------------------------------------------ render */

export function mountChat(container: HTMLElement) {
  const chat = h('div', { class: 'chat', dataset: { panel: 'chat' } });

  headEl = h('div', { class: 'chat-head' });
  logEl = h('div', { class: 'chat-log' });
  logEl.addEventListener('scroll', () => {
    if (!logEl) return;
    const gap = logEl.scrollHeight - logEl.scrollTop - logEl.clientHeight;
    stick = gap < 60;
  });

  attachRowEl = h('div', { class: 'attach-row' });
  toolbarEl = h('div', { class: 'chat-toolbar' });

  inputEl = h('textarea', {
    placeholder: t('chat.placeholder'),
    rows: 1,
    onInput: () => autoGrow(),
    onKeydown: (e: KeyboardEvent) => {
      if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
        e.preventDefault();
        void send();
      }
    },
  }) as HTMLTextAreaElement;

  sendBtn = h(
    'button',
    { class: 'send-btn', title: t('chat.send'), onClick: () => void onSendClick() },
    icon('send', 14),
  ) as HTMLButtonElement;

  const box = h('div', { class: 'chat-box' }, inputEl, sendBtn);
  hintEl = h('div', { class: 'chat-hint' });
  const inputWrap = h(
    'div',
    { class: 'chat-input-wrap' },
    attachRowEl,
    toolbarEl,
    box,
    hintEl,
  );

  chat.append(headEl, logEl, inputWrap);
  container.append(chat);

  renderHead();
  renderToolbar();
  renderAttachments();
  renderLog();

  if (!unlisten) {
    void onAgentEvent(handleEvent).then((fn) => {
      unlisten = fn;
    });
  }
}

export function unmountChat() {
  unlisten?.();
  unlisten = null;
}

/** Rebuild the whole transcript (session switch, clear, first paint). */
export function renderLog() {
  if (!logEl) return;
  clear(logEl);
  views.clear();
  streamingId = null;
  if (app.chat.length === 0) {
    logEl.append(emptyState());
    return;
  }
  for (const item of app.chat) {
    logEl.append(buildMessage(item).root);
  }
  scrollDown(true);
}

export function renderHead() {
  if (!headEl) return;
  clear(headEl);
  const title = h('span', { class: 'chat-title' }, t('chat.title'));
  const spacer = h('div', { class: 'spacer', style: { flex: '1' } });

  const btnNew = h(
    'button',
    { class: 'icon-btn', title: t('chat.new'), onClick: () => void newSession() },
    icon('plus', 14),
  );
  const btnHistory = h(
    'button',
    { class: 'icon-btn', title: t('history.title'), onClick: () => openHistory() },
    icon('history', 14),
  );
  const btnClear = h(
    'button',
    { class: 'icon-btn', title: t('chat.clear'), onClick: () => void clearSession() },
    icon('trash', 14),
  );
  // The Agent page is a first-class surface, not a closable dock: a "close"
  // button here either did nothing (no file open) or silently swapped panes,
  // so it read as a dead control. Closing belongs to the titlebar / Ctrl+Shift+C.
  headEl.append(title, spacer, btnHistory, btnNew, btnClear);
}

function renderToolbar() {
  if (!toolbarEl) return;
  clear(toolbarEl);

  const model = app.models.find((m) => m.id === app.model) || null;
  const provider = app.config?.providers.find((p) => p.id === app.providerId);
  const ready = providerReady(provider);

  // With no usable provider there is nothing to pick. Say what to do next
  // instead of showing a stale model name from a provider that was switched
  // off — the picker used to keep offering DeepSeek after DeepSeek was closed.
  const modelPicker = h(
    'button',
    {
      class: 'picker',
      title: ready ? t('chat.model') : t('chat.configureModels'),
      onClick: () => (ready ? pickModel(modelPicker) : emit('settings:open', 'providers')),
    },
    icon('cpu', 12),
    h('span', { class: 'val' }, ready
      ? (model?.display_name || app.model || t('chat.configureModels'))
      : t('chat.configureModels')),
  );

  const choices = effortChoices(model);
  const level = choices.includes(app.effort) ? app.effort : 'auto';
  const unsupported = !model || model.thinking.type === 'unsupported';
  const effortPicker = h(
    'button',
    {
      class: `picker${level !== 'auto' ? ' is-set' : ''}`,
      title:
        t('chat.effort') +
        (model ? ` · ${THINK_HINT[model.thinking.type] || ''}` : '') +
        (unsupported ? `\n${t('chat.effortBestEffort')}` : ''),
      onClick: () => {
        popupMenu(
          effortPicker,
          choices.map((e) => ({ label: effortLabel(e), value: e, active: e === app.effort })),
          (v) => void setEffort(v as Effort),
          { search: false, width: 168, side: 'above' },
        );
      },
    },
    effortMeter(level),
    h('span', { class: 'val' }, effortLabel(level)),
  );

  const attachBtn = h(
    'button',
    { class: 'picker', title: t('chat.attach'), onClick: () => void pickAttachments() },
    icon('clip', 12),
    h('span', { class: 'val' }, t('chat.attach')),
  );

  const ctx = h('span', {
    class: 'hint',
    style: { marginLeft: 'auto', fontFamily: 'var(--font-mono)' },
    title: t('chat.context'),
  }, `${fmtNum(app.contextTokens)} ${t('status.tokens')}`);

  toolbarEl.append(modelPicker, effortPicker, attachBtn, ctx);
  if ((provider && ready && !app.model) || !ready) {
    modelPicker.style.borderColor = 'var(--warn)';
  }
}

function renderAttachments() {
  if (!attachRowEl) return;
  clear(attachRowEl);
  for (const a of attachments) {
    const chip = h(
      'span',
      { class: 'attach-chip' },
      icon(a.isDir ? 'folderOpen' : 'file', 11),
      h('span', { style: { maxWidth: '150px', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' } }, a.name),
      h(
        'button',
        {
          title: t('common.delete'),
          onClick: () => {
            attachments = attachments.filter((x) => x.path !== a.path);
            renderAttachments();
          },
        },
        icon('close', 10),
      ),
    );
    attachRowEl.append(chip);
  }
  attachRowEl.style.display = attachments.length ? 'flex' : 'none';
}

function renderSendState() {
  if (!sendBtn) return;
  clear(sendBtn);
  if (app.busy) {
    sendBtn.className = 'send-btn stop';
    sendBtn.title = t('chat.stop');
    sendBtn.append(icon('stop', 14));
  } else {
    sendBtn.className = 'send-btn';
    sendBtn.title = t('chat.send');
    sendBtn.append(icon('send', 14));
  }
  sendBtn.disabled = !app.busy && !canSend();
  if (hintEl) {
    clear(hintEl);
    hintEl.append(
      h('span', null, app.busy ? t('status.busy') : t('status.ready')),
      h('span', null, `${app.chat.length} ${t('chat.session')}`),
    );
  }
}

function canSend(): boolean {
  return Boolean((inputEl?.value.trim() || attachments.length > 0) && app.sessionId);
}

function autoGrow() {
  if (!inputEl) return;
  inputEl.style.height = 'auto';
  inputEl.style.height = `${Math.min(inputEl.scrollHeight, 168)}px`;
  renderSendState();
}

function scrollDown(force = false) {
  if (!logEl) return;
  if (!force && !stick) return;
  logEl.scrollTop = logEl.scrollHeight;
  stick = true;
}

function emptyState(): HTMLElement {
  const hasProvider = (app.config?.providers || []).some(providerReady);
  const wrap = h(
    'div',
    { class: 'side-empty', style: { padding: '36px 16px', textAlign: 'center', gap: '8px' } },
    h('div', { style: { display: 'grid', placeItems: 'center', marginBottom: '4px' } }, icon('spark', 26)),
    // The greeting and the usage guidance stay: they are functional guidance
    // for the agent, not product introductions.
    h('div', { style: { color: 'var(--text-1)', fontWeight: '600' } }, t('chat.welcome')),
    h('div', { style: { fontSize: '14px', lineHeight: '1.6' } }, t('chat.welcomeHint')),
  );
  if (!hasProvider) {
    wrap.append(
      h(
        'button',
        {
          class: 'btn',
          style: { marginTop: '6px' },
          onClick: () => emit('settings:open', 'providers'),
        },
        `${t('chat.noProvider')} · ${t('chat.configure')}`,
      ),
    );
  }
  return wrap;
}

/* -------------------------------------------------------------- msg nodes */

function buildMessage(item: ChatItem): MsgView {
  const roleLabel =
    item.role === 'user' ? 'YOU' : item.role === 'system' ? 'SYS' : 'AGENT';
  const root = h('div', { class: `msg ${item.role}${item.error ? ' error' : ''}` });
  root.append(h('div', { class: 'msg-role' }, item.role === 'assistant' ? 'IRIXEN' : roleLabel));

  const body = h('div', { class: 'msg-body' });

  let reason: HTMLElement | null = null;
  if (item.reasoning && item.reasoning.trim()) {
    reason = reasoningBlock(item);
    body.append(reason);
  }

  const text = h('div', { class: 'msg-text' });
  body.append(text);

  let note: HTMLElement | null = null;
  if (item.note) {
    note = h('div', { class: 'msg-note' }, item.note);
    body.append(note);
  }

  root.append(body);

  const view: MsgView = { root, text, reason, note, tools: new Map(), turnBar: null };
  views.set(item.id, view);

  if (item.tools) {
    for (const tl of item.tools) body.append(toolCard(item, tl, view));
  }

  paintText(item, view);
  if (item.turn) paintTurnBar(item, view);
  return view;
}

/** Footer under an assistant message: "this turn rewrote N files — undo it".
 *  The snapshot lives in Rust for the lifetime of the process, so this is the
 *  only way to get a batch of edits back without reading `git diff` by hand. */
function paintTurnBar(item: ChatItem, view: MsgView) {
  const turn = item.turn;
  if (!turn) return;
  const existing = view.turnBar;
  const bar = existing ?? h('div', { class: 'turn-bar' });
  clear(bar);

  if (turn.undone) {
    bar.className = 'turn-bar done';
    bar.append(icon('check', 12), h('span', null, t('chat.undone', { n: String(turn.files.length) })));
  } else if (!turn.undoable) {
    // Refusing beats a half restore: say why instead of offering the button.
    bar.className = 'turn-bar blocked';
    bar.append(
      icon('alert', 12),
      h('span', null, t('chat.undoBlocked', { reason: turn.reason || '' })),
    );
  } else {
    bar.className = 'turn-bar';
    const files = turn.files.length;
    const btn = h(
      'button',
      { class: 'turn-undo', type: 'button', title: turn.files.join('\n') },
      icon('refresh', 12),
      h('span', null, turn.undoing ? t('chat.undoing') : t('chat.undoTurn')),
    ) as HTMLButtonElement;
    btn.disabled = Boolean(turn.undoing);
    btn.addEventListener('click', () => void runUndo(item));
    bar.append(
      btn,
      h('span', { class: 'turn-files', title: turn.files.join('\n') }, t('chat.turnFiles', { n: String(files) })),
    );
  }

  if (!existing) {
    view.turnBar = bar;
    view.root.append(bar);
  }
}

async function runUndo(item: ChatItem) {
  const turn = item.turn;
  if (!turn || turn.undone || turn.undoing) return;
  turn.undoing = true;
  const view = views.get(item.id);
  if (view) paintTurnBar(item, view);
  try {
    await api.undo(turn.id);
    turn.undone = true;
    turn.undoing = false;
    if (view) paintTurnBar(item, view);
    // The tree, git status and the review list all describe files that just
    // changed underneath them.
    emit('git:changed');
    emit('workspace');
  } catch (e) {
    turn.undoing = false;
    if (view) paintTurnBar(item, view);
    toast(errText(e), 'err');
  }
}

function paintText(item: ChatItem, view: MsgView) {
  clear(view.text);
  if (!item.text) {
    view.text.style.display = 'none';
    return;
  }
  view.text.style.display = '';
  if (item.streaming) {
    view.text.style.whiteSpace = 'pre-wrap';
    view.text.textContent = item.text;
  } else {
    view.text.style.whiteSpace = '';
    view.text.append(renderMarkdown(item.text));
  }
}

function reasoningBlock(item: ChatItem): HTMLElement {
  const body = h('div', { class: 'thinking-body' });
  const head = h('div', { class: 'thinking-head' });
  let open = Boolean(item.streaming);
  const paint = () => {
    clear(head);
    head.append(icon(open ? 'down' : 'chevron', 11), h('span', null, t('chat.reasoning')));
    clear(body);
    body.style.display = open ? '' : 'none';
    body.textContent = item.reasoning || '';
  };
  head.addEventListener('click', () => {
    open = !open;
    paint();
  });
  paint();
  const root = h('div', { class: 'thinking' }, head, body);
  // expose a repaint hook for streaming updates
  (root as any).__paint = paint;
  (root as any).__setOpen = (v: boolean) => {
    open = v;
    paint();
  };
  return root;
}

function toolCard(
  item: ChatItem,
  tool: { id: string; name: string; args: string; output?: string; isError?: boolean; open?: boolean },
  view: MsgView,
): HTMLElement {
  const dot = h('span', {
    class: `tool-status${tool.output === undefined ? ' running' : tool.isError ? ' err' : ''}`,
  });
  const args = h('span', { class: 'tool-args' }, oneLine(tool.args, 90));
  const chev = h('span', { style: { color: 'var(--text-2)', display: 'grid' } }, icon('chevron', 12));
  const head = h(
    'div',
    { class: 'tool-head' },
    dot,
    h('span', { class: 'tool-name' }, tool.name),
    args,
    chev,
  );
  const body = h('div', { class: 'tool-body' });
  body.style.display = 'none';
  body.textContent = tool.output ?? '';
  if (tool.isError) body.classList.add('err');

  head.addEventListener('click', () => {
    const open = body.style.display === 'none';
    body.style.display = open ? '' : 'none';
    chev.style.transform = open ? 'rotate(90deg)' : '';
  });

  const card = h('div', { class: 'tool-card' }, head, body);
  view.tools.set(tool.id, { card, body, dot, args });
  return card;
}

function oneLine(s: string, max: number): string {
  const flat = s.replace(/\s+/g, ' ').trim();
  return flat.length > max ? `${flat.slice(0, max)}…` : flat;
}

/* ---------------------------------------------------------------- events */

function handleEvent(ev: AgentEvent) {
  if (ev.session !== app.sessionId) return;
  switch (ev.kind) {
    case 'status':
      setStatus(ev.text);
      if (hintEl) {
        clear(hintEl);
        hintEl.append(h('span', null, ev.text));
      }
      break;
    case 'delta': {
      const item = ensureAssistant();
      item.text += ev.text;
      item.streaming = true;
      const view = views.get(item.id);
      if (view) paintText(item, view);
      scrollDown();
      break;
    }
    case 'reasoning': {
      const item = ensureAssistant();
      item.reasoning = (item.reasoning || '') + ev.text;
      item.streaming = true;
      const view = views.get(item.id);
      if (view) {
        if (!view.reason) {
          const node = reasoningBlock(item);
          node.style.display = '';
          view.reason = node;
          const bodyEl = view.root.querySelector('.msg-body');
          bodyEl?.insertBefore(node, view.text);
        } else {
          (view.reason as any).__paint?.();
        }
      }
      scrollDown();
      break;
    }
    case 'tool_call': {
      const item = ensureAssistant();
      item.tools = item.tools || [];
      if (!item.tools.some((tl) => tl.id === ev.id)) {
        const entry = { id: ev.id, name: ev.name, args: ev.arguments };
        item.tools.push(entry);
        const view = views.get(item.id);
        if (view) {
          const card = toolCard(item, entry, view);
          const bodyEl = view.root.querySelector('.msg-body');
          bodyEl?.append(card);
        }
      }
      scrollDown();
      break;
    }
    case 'tool_result': {
      const item = ensureAssistant();
      const tool = item.tools?.find((tl) => tl.id === ev.id);
      if (tool) {
        tool.output = ev.output;
        tool.isError = ev.is_error;
      }
      const view = views.get(item.id);
      const tv = view?.tools.get(ev.id);
      if (tv) {
        tv.body.textContent = ev.output;
        tv.body.classList.toggle('err', ev.is_error);
        tv.dot.className = `tool-status${ev.is_error ? ' err' : ''}`;
        tv.args.textContent = oneLine(tool?.args || '', 90);
      }
      scrollDown();
      break;
    }
    case 'checkpoint': {
      const item = ensureAssistant();
      item.turn = {
        id: ev.turn,
        files: ev.files,
        undoable: ev.undoable,
        reason: ev.reason,
      };
      const view = views.get(item.id);
      if (view) paintTurnBar(item, view);
      scrollDown();
      break;
    }
    case 'usage': {
      app.contextTokens = ev.usage.input_tokens + ev.usage.output_tokens;
      renderToolbar();
      break;
    }
    case 'compressed': {
      const r = ev.report;
      app.contextTokens = r.tokens_after;
      pushNote(
        `${t('chat.compressed')} · ${fmtNum(r.tokens_before)} → ${fmtNum(r.tokens_after)} tokens`,
      );
      renderToolbar();
      break;
    }
    case 'done': {
      const item = streamingId ? app.chat.find((c) => c.id === streamingId) : null;
      if (item) {
        item.streaming = false;
        const view = views.get(item.id);
        if (view) {
          paintText(item, view);
          const rb = view.reason as any;
          if (rb?.__setOpen) rb.__setOpen(false);
        }
      }
      streamingId = null;
      setBusy(false);
      // `Done::text` carries the *whole* answer for short turns ("已停止").
      // Anything long belongs in the transcript, not on a one-line status bar.
      const done = (ev.text || '').replace(/\s+/g, ' ').trim();
      setStatus(done && done.length <= 24 ? done : t('status.done'));
      break;
    }
    case 'error': {
      const item = streamingId ? app.chat.find((c) => c.id === streamingId) : null;
      if (item && item.streaming) {
        item.streaming = false;
        item.error = true;
        item.text = item.text ? `${item.text}\n\n${ev.text}` : ev.text;
        const view = views.get(item.id);
        if (view) paintText(item, view);
        views.get(item.id)?.root.classList.add('error');
      } else {
        pushNote(ev.text, true);
      }
      streamingId = null;
      setBusy(false);
      break;
    }
  }
}

function ensureAssistant(): ChatItem {
  if (streamingId) {
    const found = app.chat.find((c) => c.id === streamingId);
    if (found) return found;
  }
  const item: ChatItem = {
    id: nextId('a'),
    role: 'assistant',
    text: '',
    reasoning: '',
    tools: [],
    streaming: true,
  };
  app.chat.push(item);
  streamingId = item.id;
  if (logEl) {
    const empty = logEl.querySelector('.side-empty');
    if (empty) empty.remove();
    logEl.append(buildMessage(item).root);
  }
  return item;
}

function pushNote(text: string, isError = false) {
  const item: ChatItem = {
    id: nextId('s'),
    role: 'system',
    text: '',
    note: text,
    error: isError,
  };
  app.chat.push(item);
  if (logEl) logEl.append(buildMessage(item).root);
  scrollDown();
}

/* ---------------------------------------------------------------- actions */

function setBusy(busy: boolean) {
  app.busy = busy;
  app.statusText = busy ? t('status.busy') : t('status.ready');
  renderSendState();
  emit('agent:busy', busy);
}

async function onSendClick() {
  if (app.busy) {
    try {
      await api.stop(app.sessionId);
    } catch (e) {
      toast(errText(e), 'err');
    }
    setBusy(false);
    return;
  }
  await send();
}

export async function send() {
  if (!inputEl || app.busy) return;
  const text = inputEl.value.trim();
  if (!text && attachments.length === 0) return;

  // The selection has to be checked against the provider the agent will
  // actually call. `providers.some(p => p.api_key)` only proved that *someone*
  // had a key — not that this session's provider did.
  const provider = await ensureSelection();
  const blocker = providerBlocker(provider);
  if (!provider || blocker) {
    if (blocker === 'key' && provider) {
      toast(t('chat.providerNoKey', { name: provider.name || provider.id }), 'err');
    } else {
      toast(t('chat.noProvider'), 'err');
    }
    emit('settings:open', 'providers');
    return;
  }
  if (!app.model) {
    toast(t('chat.noModel'), 'err');
    return;
  }
  if (!app.sessionId) await newSession();
  if (!app.sessionId) return;

  const userItem: ChatItem = {
    id: nextId('u'),
    role: 'user',
    text,
    note: attachments.length ? attachments.map((a) => a.name).join('  ·  ') : undefined,
  };
  app.chat.push(userItem);
  if (logEl) {
    const empty = logEl.querySelector('.side-empty');
    if (empty) empty.remove();
    logEl.append(buildMessage(userItem).root);
  }
  const sent = attachments.slice();
  attachments = [];
  renderAttachments();
  inputEl.value = '';
  autoGrow();
  scrollDown(true);
  streamingId = null;
  setBusy(true);

  try {
    await api.send(app.sessionId, text, sent);
  } catch (e) {
    setBusy(false);
    pushNote(errText(e), true);
  }
}

/** Hand a prepared prompt to the composer and send it (used by the review
 *  panel). Switching surfaces first means the user watches it stream. */
export async function sendPrompt(text: string) {
  emit('chat:toggle', true);
  if (!inputEl) return;
  inputEl.value = text;
  autoGrow();
  await send();
}

async function setEffort(effort: Effort) {
  app.effort = effort;
  renderToolbar();
  // Two targets on purpose: the live session (what the next request uses) and
  // the sticky config default (what the *next* session starts from).
  if (app.config) app.config.effort = effort;
  void api.setDefaultEffort(effort).catch(() => undefined);
  if (app.sessionId) {
    try {
      await api.setModel(app.sessionId, undefined, undefined, effort);
    } catch (e) {
      toast(errText(e), 'err');
    }
  }
}

function pickModel(anchor: HTMLElement) {
  // Merely opening the picker is not consent to start a local server or
  // hidden browser windows. Web2API models appear after the user enables
  // "start on launch" or explicitly picks one (which starts it on demand).
  const items = app.models.map((m) => ({
    label: m.display_name && m.display_name !== m.id ? m.display_name : m.id,
    // Two providers can expose the same id (gpt-4o is a Web2API alias too).
    // Carry the provider in the value so picking one switches to the right one.
    value: `${m.provider_id}\u0000${m.id}`,
    hint: m.provider_id,
    active: m.id === app.model && m.provider_id === app.providerId,
  }));
  popupMenu(
    anchor,
    items,
    (value) => void chooseModel(value),
    { emptyText: t('chat.noModel'), placeholder: 'model…', side: 'above' },
  );
  if (items.length === 0) void refreshModels();
}

/** Models from *every* provider that can talk, not just the current one.
 *  `chooseModel` already adopts `m.provider_id`, so a single flat list is all
 *  the composer needs to switch between Web2API, DeepSeek, NVIDIA, etc.
 *  One dead endpoint must not hide the others, so failures are per-provider.
 */
async function loadAllModels(refresh: boolean): Promise<ModelInfo[]> {
  const providers = (app.config?.providers ?? []).filter(providerReady);
  const lists = await Promise.all(
    providers.map(async (p) => {
      try {
        return await api.modelsList(p.id, refresh);
      } catch {
        return [] as ModelInfo[];
      }
    }),
  );
  const seen = new Set<string>();
  const out: ModelInfo[] = [];
  for (let i = 0; i < providers.length; i++) {
    for (const m of lists[i]) {
      const item: ModelInfo = { ...m, provider_id: m.provider_id || providers[i].id };
      const key = `${item.provider_id}::${item.id}`;
      if (seen.has(key)) continue;
      seen.add(key);
      out.push(item);
    }
  }
  return out;
}

export async function refreshModels() {
  if (app.modelsLoading) return;
  app.modelsLoading = true;
  try {
    app.models = await loadAllModels(true);
    renderToolbar();
    emit('models:changed');
  } finally {
    app.modelsLoading = false;
  }
}

/* ------------------------------------------------- Web2API auto-enable */

let webSessionSig = '';
let lastEnsureAt = 0;

/** Start the endpoint + hidden browser sessions without any manual step.
 *  `model` narrows the boot to the reverse proxy that model routes to; no
 *  argument opens every source the user allowed in the Web2API panel.
 *
 *  Returns the bridge status so the boot checklist can show the port, or `null`
 *  when there was nothing to start (disabled, or already started this minute). */
export async function autoEnableWeb2Api(
  model?: string,
  manual = false,
): Promise<BridgeStatus | null> {
  // "Start on launch" is the single switch for AUTOMATIC Web2API startup.
  // With it off, booting the IDE must not create a local endpoint, hidden
  // WebView2 windows or a polling loop. `manual` is an explicit user gesture
  // (picking a Web2API model), which may still start the bridge on demand.
  if (!manual && app.config?.bridge.auto_start !== true) return null;
  if (app.config?.bridge.auto_ensure === false) return null;
  const now = Date.now();
  if (now - lastEnsureAt < 20_000) return null;
  lastEnsureAt = now;
  try {
    const status = await api.web2apiEnsure(model ?? null);
    // The backend enabled the built-in provider and saved config; mirror it
    // here or `loadAllModels` would still filter Web2API out as disabled.
    app.config = await api.getConfig();
    app.providers = app.config.providers;
    await refreshModels();
    return status;
  } catch {
    /* plain browser / bridge unavailable — the panel still works manually */
    return null;
  }
}

/** Poll the cached login state so a source that just logged in (or logged
 *  out) appears in / disappears from the picker on its own. */
export function watchWeb2ApiSessions() {
  // Only installed when "start on launch" is on. Less frequent than before,
  // and completely idle while the window is hidden: the Web2API panel has its
  // own on-screen polling, so background wake-ups buy nothing here.
  window.setInterval(async () => {
    if (document.hidden || app.config?.bridge.auto_start !== true) return;
    try {
      const infos = await api.web2apiSessionStatus();
      const sig = infos.map((s) => `${s.id}:${s.logged_in ? 1 : 0}`).join('|');
      if (sig === webSessionSig) return;
      webSessionSig = sig;
      await refreshModels();
    } catch {
      /* no IPC bridge (plain browser) */
    }
  }, 15_000);
}

async function chooseModel(value: string) {
  if (!app.sessionId) return;
  const [pid, ...tail] = value.split('\u0000');
  const modelId = tail.length ? tail.join('\u0000') : value;
  const m = app.models.find((x) => x.id === modelId && x.provider_id === pid)
    || app.models.find((x) => x.id === modelId);
  app.model = modelId;
  if (m) app.providerId = m.provider_id;
  // Selecting a Web2API model makes sure that reverse proxy's hidden browser
  // session is up — no "打开官网" round-trip for the user.
  if (pid === 'web2api') void autoEnableWeb2Api(modelId, true);
  const allowed = effortChoices(m);
  if (!allowed.includes(app.effort)) app.effort = 'auto';
  renderToolbar();
  try {
    await api.setModel(app.sessionId, app.providerId, modelId, app.effort);
  } catch (e) {
    toast(errText(e), 'err');
  }
}

async function pickAttachments() {
  try {
    const picked = await openDialog({ multiple: true, directory: false });
    const list = Array.isArray(picked) ? picked : picked ? [picked] : [];
    for (const p of list) attachToChat(String(p), basename(String(p)), false);
  } catch (e) {
    toast(errText(e), 'err');
  }
}

/** Called by the explorer context menu ("Add to chat"). */
export function attachToChat(path: string, name = basename(path), isDir = false) {
  if (attachments.some((a) => a.path === path)) {
    toast(name, 'info', 1200);
    return;
  }
  attachments.push({ path, name, isDir });
  renderAttachments();
  emit('chat:toggle', true);
  inputEl?.focus();
}

/** Make sure the composer points at a provider that can actually talk and a
 *  model that exists, then push that selection into the session the agent will
 *  use. Returns the provider, or null when nothing is configured.
 *
 *  This is the single place that decides "which provider am I on": the pickers
 *  read `app.providerId`, the agent reads `session.provider_id`, and they are
 *  kept in step here.
 */
export async function ensureSelection(): Promise<ProviderConfig | null> {
  const providers = app.config?.providers ?? [];
  let provider = providers.find((p) => p.id === app.providerId) ?? null;

  if (!providerReady(provider)) {
    const pick = pickDefaultProvider(providers);
    if (!pick) {
      // Nothing usable is configured. Wipe every leftover model name and list
      // so the composer asks the user to set one up instead of still offering
      // a model from a provider that was switched off.
      app.models = [];
      app.model = '';
      app.providerId = '';
      renderToolbar();
      return null;
    }
    if (pick.id !== app.providerId) app.model = '';
    provider = pick;
    app.providerId = pick.id;
  }
  if (!provider) return null;

  if (!app.models.some((m) => m.provider_id === provider!.id)) {
    app.models = await loadAllModels(false);
  }

  // The selected model can belong to a provider that was just disabled.
  if (app.model && !app.models.some((m) => m.id === app.model)) app.model = '';
  if (!app.model && app.models.length) {
    const first = app.models.find((m) => !/embed|rerank|whisper|tts/i.test(m.id)) ?? app.models[0];
    app.model = first.id;
    app.providerId = first.provider_id || app.providerId;
  }

  renderToolbar();

  if (app.sessionId && app.providerId && app.model) {
    try {
      await api.setModel(app.sessionId, app.providerId, app.model, app.effort);
    } catch {
      /* session vanished — the next send creates a fresh one */
    }
  }
  return provider;
}

let providerSig = '';

/** Re-resolve the composer after a config or bridge change.
 *  Settings writes fire on a 500ms debounce while typing, so only react when
 *  something that can actually change the model list has changed.
 */
export async function reconcileProviders(): Promise<void> {
  const providers = app.config?.providers ?? [];
  const sig = `${providers
    .map((p) => `${p.id}:${p.enabled ? 1 : 0}:${p.base_url}:${p.api_key}:${p.models_path}:${p.protocol}`)
    .join('|')}#${app.config?.bridge.port ?? 0}`;
  if (sig === providerSig) return;
  providerSig = sig;
  // Force a fresh hydrate so a provider that was just enabled (Web2API on
  // bridge start) contributes its models too.
  app.models = [];
  if (!providerReady(providers.find((p) => p.id === app.providerId))) {
    app.model = '';
    app.providerId = pickDefaultProvider(providers)?.id ?? '';
  }
  await ensureSelection();
  emit('models:changed');
}

export async function newSession() {
  try {
    const meta = await api.newSession();
    app.sessionId = meta.id;
    app.chat = [];
    app.contextTokens = 0;
    // The agent picks the provider/model for a new session; the composer has to
    // adopt them, otherwise the pickers show one thing and the request uses
    // another. Empty model → `ensureSelection` fills in the provider's first.
    if (meta.provider_id) app.providerId = meta.provider_id;
    app.model = meta.model || '';
    renderLog();
    emit('session:changed', meta.id);
    void ensureSelection();
  } catch (e) {
    toast(errText(e), 'err');
  }
}

export async function clearSession() {
  if (!app.sessionId) return;
  try {
    await api.clear(app.sessionId);
    app.chat = [];
    app.contextTokens = 0;
    renderLog();
    toast(t('chat.clear'), 'ok', 1400);
  } catch (e) {
    toast(errText(e), 'err');
  }
}

/** Load a session's persisted transcript into the panel. */
export async function loadSession(id: string) {
  app.sessionId = id;
  // Tell the engine which session is current, so a restart reopens this one
  // instead of whatever was created last.
  void api.setActive(id).catch(() => undefined);
  // Adopt the session's own provider/model so the toolbar agrees with what the
  // agent will send on this session's behalf.
  try {
    const meta = (await api.sessions()).find((s) => s.id === id);
    if (meta?.provider_id) app.providerId = meta.provider_id;
    if (meta?.model !== undefined) app.model = meta.model;
  } catch {
    /* keep whatever boot resolved */
  }
  try {
    const messages = await api.messages(id);
    app.chat = toChatItems(messages);
  } catch {
    app.chat = [];
  }
  renderLog();
  renderToolbar();
  renderSendState();
  emit('session:changed', id);
  void ensureSelection();
}

function toChatItems(messages: Message[]): ChatItem[] {
  const out: ChatItem[] = [];
  let i = 0;
  for (const m of messages) {
    if (m.role === 'system') continue;
    if (m.role === 'user') {
      const text = m.parts
        .filter((p) => p.type === 'text')
        .map((p) => (p as any).text as string)
        .join('\n');
      out.push({
        id: nextId('u'),
        role: 'user',
        text: stripAttachments(text),
      });
      continue;
    }
    if (m.role === 'assistant') {
      const text = m.parts
        .filter((p) => p.type === 'text')
        .map((p) => (p as any).text as string)
        .join('\n');
      const tools = m.parts
        .filter((p) => p.type === 'tool_call')
        .map((p) => ({
          id: (p as any).id as string,
          name: (p as any).name as string,
          args: (p as any).arguments as string,
        }));
      i += 1;
      out.push({
        id: nextId('a'),
        role: 'assistant',
        text,
        tools: tools.length ? tools : undefined,
      });
      continue;
    }
    if (m.role === 'tool') {
      // attach results to the most recent assistant tool cards
      const last = [...out].reverse().find((c) => c.role === 'assistant');
      if (!last || !last.tools) continue;
      for (const p of m.parts) {
        if (p.type !== 'tool_result') continue;
        const hit = last.tools.find((tl) => tl.id === (p as any).call_id);
        if (hit) {
          hit.output = (p as any).output;
          hit.isError = (p as any).is_error;
        }
      }
    }
  }
  return out;
}

function stripAttachments(text: string): string {
  const cut = text.search(/\n\n--- (文件|目录): /);
  return cut >= 0 ? text.slice(0, cut) : text;
}

/** Refresh the composer (providers/models changed elsewhere in the UI). */
export function refreshComposer() {
  renderToolbar();
  renderSendState();
}
