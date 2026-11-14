// Statistics panel: workspace size/LOC breakdown + API token usage.

import { app } from '../core/state';
import { api } from '../core/api';
import { h, clear, fmtBytes, fmtNum } from '../core/dom';
import { icon } from './icons';
import { t } from '../core/i18n';
import { toast } from './toast';

let bodyEl: HTMLElement | null = null;

export function mountStats(container: HTMLElement) {
  bodyEl = container;
  render();
  void refreshStats();
}

export async function refreshStats() {
  if (app.config?.workspace) {
    app.statsLoading = true;
    render();
    try {
      app.stats = await api.stats();
    } catch (e) {
      toast(String(e), 'err');
    }
    app.statsLoading = false;
  }
  try {
    app.usage = await api.usage();
  } catch {
    /* ignore */
  }
  render();
}

export function render() {
  if (!bodyEl) return;
  clear(bodyEl);
  const wrap = h('div', { class: 'stats-wrap' });

  // ---- workspace ----
  wrap.append(h('div', { class: 'section-title' }, t('stats.workspace')));
  if (app.statsLoading) {
    wrap.append(h('div', { class: 'side-empty' }, h('span', { class: 'spin' }, icon('refresh', 14)), ' ', t('stats.scanning')));
  } else if (!app.stats) {
    wrap.append(h('div', { class: 'side-empty' }, t('stats.noWorkspace')));
  } else {
    const s = app.stats;
    wrap.append(
      h(
        'div',
        { class: 'stat-grid' },
        statCard(fmtNum(s.files), t('stats.files')),
        statCard(fmtNum(s.total_loc), t('stats.lines')),
        statCard(fmtBytes(s.total_bytes), t('stats.size')),
        statCard(fmtNum(s.dirs), 'dirs'),
      ),
    );
    if (s.by_ext.length) {
      wrap.append(h('div', { class: 'section-title' }, t('stats.byExt')));
      const max = s.by_ext[0]?.loc || s.by_ext[0]?.files || 1;
      for (const e of s.by_ext.slice(0, 8)) {
        const pct = Math.max(3, Math.round(((e.loc || e.files) / max) * 100));
        wrap.append(
          h(
            'div',
            { class: 'ext-row' },
            h('span', { class: 'ext-name' }, `.${e.ext}`),
            h('div', { class: 'ext-bar' }, h('i', { style: { width: `${pct}%` } })),
            h('span', { class: 'ext-num' }, fmtNum(e.loc)),
          ),
        );
      }
    }
    if (s.largest.length) {
      wrap.append(h('div', { class: 'section-title' }, t('stats.largest')));
      for (const f of s.largest.slice(0, 5)) {
        const name = f.path.split(/[\\/]/).pop() || f.path;
        wrap.append(
          h(
            'div',
            { class: 'ext-row', title: f.path },
            h('span', { class: 'ext-name', style: { width: '120px' } }, name),
            h('span', { class: 'ext-num', style: { marginLeft: 'auto' } }, fmtBytes(f.bytes)),
          ),
        );
      }
    }
    wrap.append(
      h(
        'button',
        { class: 'btn sm', style: { marginTop: '6px', justifyContent: 'center' }, onClick: () => void refreshStats() },
        icon('refresh', 12),
        t('stats.scan'),
      ),
    );
  }

  // ---- api usage ----
  wrap.append(h('div', { class: 'section-title' }, t('stats.api')));
  const u = app.usage;
  if (!u) {
    wrap.append(h('div', { class: 'side-empty' }, '—'));
  } else {
    wrap.append(
      h(
        'div',
        { class: 'stat-grid' },
        statCard(String(u.calls), t('stats.calls')),
        statCard(fmtNum(u.input_tokens), t('stats.input')),
        statCard(fmtNum(u.output_tokens), t('stats.output')),
        statCard(fmtNum(u.reasoning_tokens), t('stats.reasoning')),
      ),
    );
    if (u.by_model.length) {
      wrap.append(h('div', { class: 'section-title' }, t('chat.model')));
      for (const m of u.by_model.slice(0, 6)) {
        wrap.append(
          h(
            'div',
            { class: 'ext-row', title: m.model },
            h('span', { class: 'ext-name', style: { width: '130px' } }, m.model.split('/').pop() || m.model),
            h('span', { class: 'ext-num', style: { marginLeft: 'auto', width: 'auto' } }, `${fmtNum(m.usage.input_tokens)} ↑ / ${fmtNum(m.usage.output_tokens)} ↓`),
          ),
        );
      }
    }
    wrap.append(
      h(
        'button',
        {
          class: 'btn sm',
          style: { marginTop: '6px', justifyContent: 'center' },
          onClick: async () => {
            await api.resetUsage();
            await refreshStats();
          },
        },
        icon('trash', 12),
        t('stats.reset'),
      ),
    );
  }

  bodyEl.append(wrap);
}

function statCard(value: string, label: string): HTMLElement {
  return h('div', { class: 'stat-card' }, h('div', { class: 'stat-value' }, value), h('div', { class: 'stat-label' }, label));
}
