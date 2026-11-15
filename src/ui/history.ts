// Session history.
//
// The transcript was already persisted and `loadSession` already existed, but
// nothing in the UI listed sessions: closing the app (or hitting "new") made
// every earlier conversation unreachable. This is that missing list — search
// by title, switch, delete.
//
// Reuses the command palette's shell (`.palette*` classes) on purpose: same
// overlay, same keyboard contract, one set of styles to keep correct.

import { h, clear } from '../core/dom';
import { icon } from './icons';
import { t } from '../core/i18n';
import { api, errText } from '../core/api';
import { app } from '../core/state';
import { toast } from './toast';
import { loadSession, newSession } from './chat';
import type { SessionMeta } from '../types';

let open = false;

/** "3 minutes ago" — enough to tell two sessions apart, short enough for a row. */
function ago(ms: number): string {
  const mins = Math.round((Date.now() - ms) / 60_000);
  if (mins < 1) return t('history.now');
  if (mins < 60) return t('history.minutes', { n: String(mins) });
  const hours = Math.round(mins / 60);
  if (hours < 24) return t('history.hours', { n: String(hours) });
  return t('history.days', { n: String(Math.round(hours / 24)) });
}

export function openHistory() {
  if (open) return;
  open = true;

  const input = h('input', {
    class: 'palette-input',
    placeholder: t('history.search'),
  }) as HTMLInputElement;
  const list = h('div', { class: 'palette-list' });
  const back = h('div', { class: 'palette-back' });
  const card = h(
    'div',
    { class: 'palette' },
    h('div', { class: 'palette-head' }, icon('history', 13), input),
    list,
  );
  back.append(card);
  document.body.append(back);

  let all: SessionMeta[] = [];
  let shown: SessionMeta[] = [];
  let cursor = 0;

  const close = () => {
    open = false;
    back.remove();
    document.removeEventListener('keydown', onKey, true);
  };

  const pick = async (meta?: SessionMeta) => {
    if (!meta) return;
    close();
    if (meta.id === app.sessionId) return;
    await loadSession(meta.id);
  };

  const remove = async (meta: SessionMeta) => {
    const title = meta.title || t('history.untitled');
    if (!window.confirm(t('history.deleteConfirm', { title }))) return;
    try {
      await api.deleteSession(meta.id);
      all = all.filter((s) => s.id !== meta.id);
      // Deleting the open session leaves the panel pointing at nothing.
      if (meta.id === app.sessionId) await newSession();
      filter();
      toast(t('history.deleted'), 'ok', 1600);
    } catch (e) {
      toast(errText(e), 'err');
    }
  };

  const draw = () => {
    clear(list);
    if (shown.length === 0) {
      list.append(h('div', { class: 'palette-empty' }, all.length ? t('palette.empty') : t('history.empty')));
      return;
    }
    shown.forEach((meta, i) => {
      const title = meta.title || t('history.untitled');
      const del = h(
        'button',
        {
          class: 'palette-del',
          title: t('history.delete'),
          onClick: (e: MouseEvent) => {
            e.stopPropagation();
            void remove(meta);
          },
        },
        icon('trash', 12),
      );
      list.append(
        h(
          'div',
          {
            class: `palette-row${i === cursor ? ' on' : ''}`,
            onMouseenter: () => {
              cursor = i;
              draw();
            },
            onClick: () => void pick(meta),
          },
          h('span', { class: 'history-label' }, title),
          h('span', { class: 'palette-hint' }, t('history.messages', { n: String(meta.message_count) })),
          h('span', { class: 'palette-group' }, meta.id === app.sessionId ? t('history.current') : ago(meta.updated_at)),
          del,
        ),
      );
    });
    (list.children[cursor] as HTMLElement | undefined)?.scrollIntoView({ block: 'nearest' });
  };

  const filter = () => {
    const q = input.value.trim().toLowerCase();
    shown = !q ? all : all.filter((s) => (s.title || '').toLowerCase().includes(q));
    cursor = 0;
    draw();
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      cursor = Math.min(cursor + 1, Math.max(shown.length - 1, 0));
      draw();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      cursor = Math.max(cursor - 1, 0);
      draw();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      void pick(shown[cursor]);
    }
  };

  input.addEventListener('input', filter);
  document.addEventListener('keydown', onKey, true);
  back.addEventListener('click', (e) => {
    if (e.target === back) close();
  });

  draw();
  setTimeout(() => input.focus(), 20);

  void api
    .sessions()
    .then((rows) => {
      all = rows.sort((a, b) => b.updated_at - a.updated_at);
      filter();
    })
    .catch((e) => {
      clear(list);
      list.append(h('div', { class: 'palette-empty' }, errText(e)));
    });
}
