// Toasts and a promise-based confirm dialog.

import { h, clear } from '../core/dom';
import { api, errText } from '../core/api';
import { t } from '../core/i18n';
import { icon } from './icons';

let host: HTMLElement | null = null;

function ensureHost(): HTMLElement {
  if (!host) {
    host = h('div', { class: 'toasts' });
    document.body.append(host);
  }
  return host;
}

export function toast(msg: string, kind: 'ok' | 'err' | 'info' = 'info', ms = 3200) {
  // The icon is the only thing that survives a theme swap intact: `--err` on
  // `--bg-3` is a different contrast in light and dark, so a 3px border alone
  // reads as a random red line rather than "this failed".
  const glyph = kind === 'err' ? 'alert' : kind === 'ok' ? 'check' : 'info';
  const el = h('div', { class: `toast ${kind === 'info' ? '' : kind}` },
    icon(glyph, 13),
    h('span', { class: 'toast-text' }, msg),
  );
  ensureHost().append(el);
  setTimeout(() => {
    el.style.opacity = '0';
    el.style.transition = 'opacity 200ms';
    setTimeout(() => el.remove(), 220);
  }, ms);
}

export function confirmDialog(title: string, message: string): Promise<boolean> {
  return new Promise((resolve) => {
    const overlay = h('div', { class: 'modal', style: { zIndex: '80' } });
    const card = h('div', {
      style: {
        width: 'min(400px, 90vw)',
        background: 'var(--bg-1)',
        border: '1px solid var(--border-strong)',
        borderRadius: '12px',
        padding: '16px 18px',
        display: 'flex',
        flexDirection: 'column',
        gap: '10px',
        boxShadow: 'var(--shadow-1)',
      },
    });
    const close = (v: boolean) => {
      overlay.remove();
      resolve(v);
    };
    card.append(
      h('div', { style: { fontWeight: '650', fontSize: '15.5px' } }, title),
      h('div', { class: 'hint', style: { fontSize: '14.5px', color: 'var(--text-1)' } }, message),
      h(
        'div',
        { class: 'row', style: { justifyContent: 'flex-end', marginTop: '4px' } },
        h('button', { class: 'btn', onClick: () => close(false) }, t('common.cancel')),
        h('button', { class: 'btn primary', onClick: () => close(true) }, t('common.ok')),
      ),
    );
    overlay.append(card);
    overlay.addEventListener('click', (e) => {
      if (e.target === overlay) close(false);
    });
    document.body.append(overlay);
  });
}

export function promptDialog(title: string, placeholder = '', value = ''): Promise<string | null> {
  return new Promise((resolve) => {
    const overlay = h('div', { class: 'modal', style: { zIndex: '80' } });
    const input = h('input', { value, placeholder, style: { width: '100%' } });
    const card = h('div', {
      style: {
        width: 'min(420px, 90vw)',
        background: 'var(--bg-1)',
        border: '1px solid var(--border-strong)',
        borderRadius: '12px',
        padding: '16px 18px',
        display: 'flex',
        flexDirection: 'column',
        gap: '10px',
        boxShadow: 'var(--shadow-1)',
      },
    });
    const close = (v: string | null) => {
      overlay.remove();
      resolve(v);
    };
    card.append(
      h('div', { style: { fontWeight: '650', fontSize: '15.5px' } }, title),
      input,
      h(
        'div',
        { class: 'row', style: { justifyContent: 'flex-end' } },
        h('button', { class: 'btn', onClick: () => close(null) }, t('common.cancel')),
        h('button', { class: 'btn primary', onClick: () => close(input.value.trim() || null) }, t('common.ok')),
      ),
    );
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') close(input.value.trim() || null);
      if (e.key === 'Escape') close(null);
    });
    overlay.append(card);
    document.body.append(overlay);
    setTimeout(() => input.focus(), 30);
  });
}

/** The menu currently on screen, if any. Kept so clicking its own anchor
 *  again closes it instead of silently rebuilding an identical menu. */
let activePopup: { key: string; root: HTMLElement; close: () => void } | null = null;

/** The composer toolbar is rebuilt whenever the model list refreshes, so an
 *  element reference is not a stable identity for the model picker. Class +
 *  position within its row survives that rebuild. */
function popupAnchorKey(anchor: HTMLElement): string {
  const parent = anchor.parentElement;
  const idx = parent ? Array.prototype.indexOf.call(parent.children, anchor) : -1;
  return `${anchor.tagName}|${anchor.className}|${idx}|${parent?.className ?? ''}`;
}

/** A minimal popup menu anchored to an element. */
export function popupMenu(
  anchor: HTMLElement,
  items: { label: string; value: string; hint?: string; active?: boolean; icon?: string; group?: string }[],
  onPick: (value: string) => void,
  opts: {
    search?: boolean;
    emptyText?: string;
    placeholder?: string;
    width?: number;
    /** 'above' always opens upward (bottom toolbar pickers); default flips only when needed. */
    side?: 'above' | 'auto';
    /** Open at a point instead of under `anchor` (right-click menus). */
    at?: { x: number; y: number };
  } = {},
) {
  // Toggle: a second click on the same anchor closes the menu. Picking an
  // item, clicking elsewhere, or pressing Escape still closes it as before.
  // A point-anchored menu has no stable key, so it never toggles.
  const key = opts.at ? `at:${opts.at.x},${opts.at.y},${items.length}` : popupAnchorKey(anchor);
  if (!opts.at && activePopup?.key === key && activePopup.root.isConnected) {
    activePopup.close();
    return;
  }
  activePopup?.close();
  document.querySelectorAll('.irixen-popup').forEach((n) => n.remove());
  const box = opts.at
    ? { left: opts.at.x, right: opts.at.x, top: opts.at.y, bottom: opts.at.y, width: 0, height: 0 }
    : anchor.getBoundingClientRect();
  const rect = box as DOMRect;
  const width = opts.width ?? 320;
  // Menus anchored at the bottom of the window (the chat composer) used to be
  // pinned to `innerHeight - 60`, which cropped every entry past the second.
  // Size the menu from the item count and flip it above the anchor when there
  // is no room below.
  // Size from the rows that are actually rendered. The old estimate reserved
  // a search field and 34px rows even for a two-item model picker, so the
  // flipped menu floated a good 50px above its own button.
  const rowH = 28;
  const hasSearch = opts.search !== false && items.length > 8;
  const wantH = Math.min(340, items.length * rowH + 8 + (hasSearch ? 42 : 0));
  const spaceBelow = window.innerHeight - rect.bottom - 12;
  const above = opts.side === 'above';
  // An upward menu must also fit between the anchor and the top of the window;
  // clamp its height so the first row never gets cut off on a short window.
  const menuH = above ? Math.max(60, Math.min(wantH, rect.top - 12)) : wantH;
  const top = above
    ? Math.max(8, rect.top - menuH - 4)
    : spaceBelow >= wantH
      ? rect.bottom + 4
      : Math.max(8, rect.top - wantH - 4);
  const root = h('div', {
    class: 'irixen-popup',
    role: 'menu',
    style: {
      position: 'fixed',
      left: `${Math.max(8, Math.min(rect.left, window.innerWidth - width - 12))}px`,
      top: `${top}px`,
      width: `${width}px`,
      maxHeight: `${menuH}px`,
      background: 'var(--bg-1)',
      border: '1px solid var(--border-strong)',
      borderRadius: '8px',
      zIndex: '70',
      display: 'flex',
      flexDirection: 'column',
      overflow: 'hidden',
    },
  });

  let closed = false;
  // A rebuilt anchor (toolbar redraw while the menu is open) must still count
  // as "its own button", otherwise mousedown closes the menu before the click
  // handler can toggle it.
  const sameAnchor = (target: Node) => {
    // A point-anchored menu (right click) belongs to no element: it was opened
    // at a coordinate, and `anchor` is only a placeholder. Treating that
    // placeholder as an owner made every click "inside the anchor" — with
    // `document.body` as the placeholder the menu never closed at all.
    if (opts.at) return false;
    if (anchor.contains(target)) return true;
    const el = target instanceof Element ? target : target.parentElement;
    const btn = el?.closest('button');
    return !!btn && popupAnchorKey(btn as HTMLElement) === key;
  };
  const onDoc = (e: MouseEvent) => {
    // The anchor's own click is ignored here; popupMenu() handles the toggle.
    if (!root.contains(e.target as Node) && !sameAnchor(e.target as Node)) close();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') close();
  };
  const close = () => {
    if (closed) return;
    closed = true;
    root.remove();
    document.removeEventListener('mousedown', onDoc);
    document.removeEventListener('keydown', onKey);
    if (activePopup?.root === root) activePopup = null;
  };
  activePopup = { key, root, close };

  let filter = '';
  const listEl = h('div', { style: { overflow: 'auto', padding: '4px' } });

  const draw = () => {
    clear(listEl);
    const shown = items.filter((it) => !filter || it.label.toLowerCase().includes(filter) || it.value.toLowerCase().includes(filter));
    if (shown.length === 0) {
      listEl.append(h('div', { class: 'side-empty' }, opts.emptyText || '—'));
      return;
    }
    let prevGroup: string | undefined;
    for (const it of shown) {
      // Rows that come from different sources (built-ins vs. one extension) get
      // a hairline between them; `group` is only set by the extension rows.
      if (prevGroup !== undefined && it.group !== prevGroup) {
        listEl.append(h('div', { style: { height: '1px', margin: '4px 6px', background: 'var(--border)' } }));
      }
      prevGroup = it.group;
      const row = h(
        'div',
        {
          class: 'git-file',
          role: 'menuitem',
          tabIndex: -1,
          style: {
            height: '28px',
            background: it.active ? 'var(--bg-2)' : 'transparent',
            color: it.active ? 'var(--accent)' : 'var(--text-1)',
          },
          onClick: () => {
            close();
            onPick(it.value);
          },
        },
        it.icon
          ? h('img', {
              src: it.icon,
              alt: '',
              style: { width: '14px', height: '14px', objectFit: 'contain', flex: 'none', marginRight: '7px' },
            })
          : null,
        h('span', { class: 'fname', style: { flex: '1' } }, it.label),
        it.hint ? h('span', { class: 'muted', style: { fontSize: '13px', fontFamily: 'var(--font-mono)' } }, it.hint) : null,
        it.active ? h('span', { style: { color: 'var(--accent)' } }, '✓') : null,
      );
      listEl.append(row);
    }
  };
  draw();

  if (hasSearch) {
    const input = h('input', { placeholder: opts.placeholder || '…', style: { margin: '6px', flex: 'none' } });
    input.addEventListener('input', () => {
      filter = input.value.trim().toLowerCase();
      draw();
    });
    root.append(input);
    setTimeout(() => input.focus(), 20);
  }
  root.append(listEl);
  document.body.append(root);

  // Defer so the click that opened the menu cannot immediately close it.
  setTimeout(() => {
    if (!closed) {
      document.addEventListener('mousedown', onDoc);
      document.addEventListener('keydown', onKey);
    }
  }, 0);
}

/**
 * Inline, in-panel error: a bordered block instead of a toast. Failures that
 * live in the page (a search that never returned, a list that failed to load)
 * should stay where the user was looking and say what to do next — a toast in
 * the corner disappears before it can be read.
 */
export function errorBox(title: string, detail = '', onRetry?: () => void): HTMLElement {
  return h('div', { class: 'inline-error' },
    icon('alert', 14),
    h('div', { class: 'inline-error-body' },
      h('div', { class: 'inline-error-title' }, title),
      detail ? h('div', { class: 'inline-error-detail' }, detail) : null,
    ),
    onRetry
      ? h('div', { class: 'inline-error-actions' },
          h('button', { class: 'btn small', onClick: onRetry }, t('common.retry')))
      : null,
  );
}

/* ---------------------------------------------------------- problem dialog */
//
// A crashed extension or an uncaught renderer exception used to be a toast
// that vanished in three seconds, leaving no way to recover or to tell anyone
// what happened. This is one window-level dialog instead: it batches the
// error burst, offers a real restart (stop all hosts + autostart again) and
// copies a run log the user can attach to a bug report.

/** Fatal = the IDE itself is broken. Plugin = one extension host misbehaved. */
export type ProblemKind = 'fatal' | 'plugin';

interface ProblemRow {
  source: string;
  detail: string;
  at: number;
  kind: ProblemKind;
  pluginId: string;
}

const problemRows: ProblemRow[] = [];
let problemOverlay: HTMLElement | null = null;
let problemSummary: HTMLPreElement | null = null;
let problemTimer: number | null = null;
let problemKeyHandler: ((e: KeyboardEvent) => void) | null = null;
let problemBusy = false;
let problemKind: ProblemKind = 'fatal';

/**
 * Report a failure. Repeated identical errors collapse into one dialog.
 *
 * `kind` decides which dialog the user sees: an uncaught renderer exception or
 * a rejected IDE promise is `fatal` ("IRIXEN Code 遇到问题"); anything that
 * came out of an extension host — a crash, a TypeError in its logs, a dead
 * webview panel — is `plugin` ("插件故障") and offers to restart just that one
 * extension rather than tearing down every host.
 */
export function reportProblem(
  detail: string,
  source = 'IRIXEN Code',
  kind: ProblemKind = 'fatal',
  pluginId = '',
) {
  const text = String(detail || '').trim();
  if (!text) return;
  const now = Date.now();
  if (problemRows.some((r) => r.detail === text && now - r.at < 15000)) return;
  problemRows.push({ source, detail: text.slice(0, 2000), at: now, kind, pluginId });
  if (problemRows.length > 40) problemRows.shift();
  if (problemOverlay) {
    // A real IDE failure outranks a plugin complaint: upgrade the open dialog
    // instead of silently appending it to a "plugin fault" window.
    if (kind === 'fatal' && problemKind === 'plugin') {
      problemKind = 'fatal';
      closeProblem(false);
      showProblemDialog();
      return;
    }
    paintProblemSummary();
    return;
  }
  // A dying host often emits a dozen errors in the same tick. Batch them so
  // the dialog opens once with a readable summary.
  if (problemTimer === null) {
    problemTimer = window.setTimeout(() => {
      problemTimer = null;
      showProblemDialog();
    }, 150);
  }
}

function paintProblemSummary() {
  if (!problemSummary) return;
  problemSummary.textContent = problemRows
    .slice(-8)
    .map((r) => `[${new Date(r.at).toLocaleTimeString()}] ${r.source}: ${r.detail}`)
    .join('\n');
}

/** Everything a bug report needs, including the extension host transcripts. */
export async function buildRunLog(): Promise<string> {
  const out: string[] = [];
  try {
    const info = await api.appInfo();
    out.push(
      `IRIXEN Code ${info.version || ''}`,
      `semver: ${info.semver || ''}`,
      `platform: ${info.platform || ''}`,
      `config: ${info.config_dir || ''}`,
      `workspace: ${info.workspace || ''}`,
    );
  } catch {
    out.push('IRIXEN Code');
  }
  out.push(`time: ${new Date().toISOString()}`, `userAgent: ${navigator.userAgent}`, '');
  out.push('== reported errors ==');
  for (const r of problemRows) {
    out.push(`[${new Date(r.at).toISOString()}] [${r.kind}][${r.source}] ${r.detail}`);
  }
  out.push('', '== extension hosts ==');
  try {
    for (const host of await api.exthostStatus()) {
      out.push(`[${host.state}] ${host.id} pid=${host.pid} activated=${host.activated} error=${host.error || ''}`);
    }
  } catch (e) {
    out.push(`(unavailable) ${errText(e)}`);
  }
  out.push('', '== extension logs ==');
  try {
    for (const line of await api.exthostLogs()) {
      out.push(`[${new Date(line.ts).toISOString()}] [${line.level}] ${line.id}: ${line.text}`);
    }
  } catch (e) {
    out.push(`(unavailable) ${errText(e)}`);
  }
  return out.join('\n');
}

async function copyRunLog(): Promise<boolean> {
  const text = await buildRunLog();
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    // The async clipboard needs a secure context and user activation; some
    // WebView2 configurations refuse it. Selection-based copy still works.
    const ta = h('textarea', { style: { position: 'fixed', top: '0', left: '0', opacity: '0' } });
    ta.value = text;
    document.body.append(ta);
    ta.select();
    let ok = false;
    try {
      ok = document.execCommand('copy');
    } catch {
      /* clipboard unavailable */
    }
    ta.remove();
    return ok;
  }
}

function closeProblem(clearRows = true) {
  if (problemKeyHandler) {
    window.removeEventListener('keydown', problemKeyHandler);
    problemKeyHandler = null;
  }
  problemOverlay?.remove();
  problemOverlay = null;
  problemSummary = null;
  problemKind = 'fatal';
  // The tier is derived from the accumulated rows. Keeping a fatal row after
  // the user dismissed the dialog would make every later plugin fault show up
  // as "IRIXEN Code 遇到问题" forever.
  if (clearRows) problemRows.length = 0;
}

async function restartAction(btn: HTMLButtonElement, label: string, kind: ProblemKind, pluginId: string) {
  if (problemBusy) return;
  problemBusy = true;
  btn.disabled = true;
  btn.textContent = kind === 'plugin' && pluginId ? t('plugin.restarting') : t('problem.restarting');
  try {
    if (kind === 'plugin' && pluginId) {
      await api.exthostRestart(pluginId);
      toast(t('plugin.restarted'), 'ok');
    } else {
      await api.exthostRestartAll();
      toast(t('problem.restarted'), 'ok');
    }
    closeProblem();
  } catch (e) {
    toast(errText(e), 'err', 5000);
    btn.disabled = false;
    btn.textContent = label;
  } finally {
    problemBusy = false;
  }
}

function showProblemDialog() {
  if (problemOverlay) return;
  const summary = h('pre', {
    style: {
      margin: '0',
      padding: '10px 12px',
      maxHeight: '150px',
      overflow: 'auto',
      background: 'var(--bg-2)',
      border: '1px solid var(--border)',
      borderRadius: '8px',
      fontSize: '13px',
      lineHeight: '1.55',
      whiteSpace: 'pre-wrap',
      wordBreak: 'break-word',
      color: 'var(--text-1)',
      fontFamily: 'var(--font-mono)',
    },
  });
  problemSummary = summary;
  paintProblemSummary();

  const copyBtn = h('button', {
    class: 'btn',
    onClick: async () => {
      const ok = await copyRunLog();
      toast(ok ? t('problem.copied') : t('problem.copyFailed'), ok ? 'ok' : 'err');
    },
  }, t('problem.copy'));
  const closeBtn = h('button', { class: 'btn', onClick: () => closeProblem() }, t('common.close'));
  problemKind = problemRows.some((r) => r.kind === 'fatal') ? 'fatal' : 'plugin';
  const isPlugin = problemKind === 'plugin';
  const pluginId = problemRows.find((r) => r.pluginId)?.pluginId || '';
  const title = t(isPlugin ? 'plugin.title' : 'problem.title');
  const body = isPlugin
    ? t(pluginId ? 'plugin.body' : 'plugin.bodyGeneric', { id: pluginId })
    : t('problem.body');
  // Only a plugin we can name gets a single-extension restart. Otherwise the
  // safe action is still "restart everything", with the original label.
  const single = isPlugin && !!pluginId;
  const restartLabel = t(single ? 'plugin.restart' : 'problem.restart');
  const restartBtn = h('button', {
    class: 'btn primary',
    onClick: () => void restartAction(restartBtn, restartLabel, problemKind, pluginId),
  }, restartLabel);

  const card = h('div', {
    style: {
      width: 'min(520px, 92vw)',
      background: 'var(--bg-1)',
      border: '1px solid var(--border-strong)',
      borderRadius: '12px',
      padding: '18px 20px',
      display: 'flex',
      flexDirection: 'column',
      gap: '10px',
      boxShadow: 'var(--shadow-1)',
    },
  },
    h('div', { style: { display: 'flex', alignItems: 'center', gap: '9px' } },
      icon(isPlugin ? 'plug' : 'alert', 16),
      h('div', { style: { fontWeight: '650', fontSize: '15.5px' } }, title)),
    h('div', {
      class: 'hint',
      style: { fontSize: '14.5px', color: 'var(--text-1)', lineHeight: '1.6' },
    }, body),
    h('div', {
      style: { fontSize: '12.5px', color: 'var(--text-2)', letterSpacing: '0.06em' },
    }, t('problem.details')),
    summary,
    h('div', {
      class: 'row',
      style: { justifyContent: 'flex-end', gap: '8px', marginTop: '4px', flexWrap: 'wrap' },
    }, copyBtn, closeBtn, restartBtn),
  );

  const overlay = h('div', {
    class: 'modal',
    style: { zIndex: '95' },
    role: 'dialog',
    'aria-modal': 'true',
    'aria-label': title,
  });
  overlay.append(card);
  overlay.addEventListener('click', (e) => {
    if (e.target === overlay) closeProblem();
  });
  problemKeyHandler = (e: KeyboardEvent) => {
    if (e.key === 'Escape') closeProblem();
  };
  window.addEventListener('keydown', problemKeyHandler);
  problemOverlay = overlay;
  document.body.append(overlay);
  setTimeout(() => restartBtn.focus(), 30);
}
