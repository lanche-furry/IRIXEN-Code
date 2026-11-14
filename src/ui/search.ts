// Workspace search panel (gitignore-aware, powered by the Rust side).

import { app } from '../core/state';
import { api } from '../core/api';
import { h, clear } from '../core/dom';
import { icon } from './icons';
import { t } from '../core/i18n';
import { toast } from './toast';
import { openFileAtLine } from './editor';
import type { SearchHit } from '../types';

let bodyEl: HTMLElement | null = null;

export function mountSearch(container: HTMLElement) {
  bodyEl = container;
  render([]);
}

export function render(hits: SearchHit[], query = '') {
  if (!bodyEl) return;
  clear(bodyEl);

  const q = h('input', { placeholder: t('search.placeholder'), value: query });
  const g = h('input', { placeholder: t('search.glob'), style: { fontSize: '14px' } });
  const meta = h('div', { class: 'search-meta' });
  const results = h('div');

  const run = async () => {
    const query = q.value.trim();
    if (!query) {
      clear(results);
      meta.textContent = '';
      return;
    }
    try {
      const found = await api.search(query, g.value.trim() || undefined);
      meta.textContent = found.length ? t('search.results', { n: found.length }) : t('search.noResults');
      clear(results);
      for (const hit of found) {
        const name = hit.path.split(/[\\/]/).slice(-2).join('/');
        const text = h('div', { class: 'hit-text' });
        text.innerHTML = highlight(hit.text, query);
        results.append(
          h(
            'div',
            { class: 'hit', onClick: () => void openFileAtLine(hit.path, hit.line) },
            h('div', { class: 'hit-path' }, `${name}:${hit.line}`),
            text,
          ),
        );
      }
    } catch (e) {
      toast(String(e), 'err');
    }
  };

  q.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') void run();
  });
  g.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') void run();
  });

  bodyEl.append(h('div', { class: 'search-wrap' }, q, g), meta, results);
  setTimeout(() => q.focus(), 30);
}

function highlight(text: string, query: string): string {
  const esc = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  if (!query) return esc(text);
  const idx = text.toLowerCase().indexOf(query.toLowerCase());
  if (idx < 0) return esc(text);
  return (
    esc(text.slice(0, idx)) +
    '<mark>' +
    esc(text.slice(idx, idx + query.length)) +
    '</mark>' +
    esc(text.slice(idx + query.length))
  );
}
