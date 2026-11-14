// Tiny, dependency-free markdown renderer for chat output.
// Handles: fenced code, headings, lists, bold/italic, inline code, links, hr.

import { h, copyText } from '../core/dom';
import { t } from '../core/i18n';

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function inline(s: string): string {
  let out = escapeHtml(s);
  out = out.replace(/`([^`]+)`/g, '<code>$1</code>');
  out = out.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  out = out.replace(/(^|[^*])\*([^*\n]+)\*/g, '$1<em>$2</em>');
  out = out.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, '<a href="$2" target="_blank" rel="noreferrer">$1</a>');
  return out;
}

export function renderMarkdown(text: string): DocumentFragment {
  const frag = document.createDocumentFragment();
  const lines = text.split('\n');
  let i = 0;
  let list: HTMLElement | null = null;
  let listTag = '';

  const closeList = () => {
    if (list) {
      frag.append(list);
      list = null;
      listTag = '';
    }
  };

  while (i < lines.length) {
    const line = lines[i];

    // fenced code block
    if (/^```/.test(line)) {
      closeList();
      const lang = line.slice(3).trim();
      const buf: string[] = [];
      i++;
      while (i < lines.length && !/^```/.test(lines[i])) {
        buf.push(lines[i]);
        i++;
      }
      i++; // closing fence
      frag.append(codeBlock(buf.join('\n'), lang));
      continue;
    }

    // heading
    const hm = /^(#{1,4})\s+(.*)$/.exec(line);
    if (hm) {
      closeList();
      const level = Math.min(4, hm[1].length + 1);
      const el = document.createElement(`h${level}`);
      el.innerHTML = inline(hm[2]);
      frag.append(el);
      i++;
      continue;
    }

    // hr
    if (/^\s*(---|\*\*\*)\s*$/.test(line)) {
      closeList();
      frag.append(h('hr', { style: { border: 'none', borderTop: '1px solid var(--border)', margin: '8px 0' } }));
      i++;
      continue;
    }

    // ordered / unordered list
    const lm = /^\s*([-*+]|\d+\.)\s+(.*)$/.exec(line);
    if (lm) {
      const tag = /\d/.test(lm[1]) ? 'ol' : 'ul';
      if (!list || listTag !== tag) {
        closeList();
        list = h(tag);
        listTag = tag;
      }
      const li = h('li');
      li.innerHTML = inline(lm[2]);
      list.append(li);
      i++;
      continue;
    }

    // blank line
    if (line.trim() === '') {
      closeList();
      i++;
      continue;
    }

    // paragraph (merge consecutive non-blank lines)
    closeList();
    const buf = [line];
    i++;
    while (
      i < lines.length &&
      lines[i].trim() !== '' &&
      !/^```/.test(lines[i]) &&
      !/^(#{1,4})\s+/.test(lines[i]) &&
      !/^\s*([-*+]|\d+\.)\s+/.test(lines[i])
    ) {
      buf.push(lines[i]);
      i++;
    }
    const p = h('p');
    p.innerHTML = inline(buf.join('\n')).replace(/\n/g, '<br/>');
    frag.append(p);
  }
  closeList();
  return frag;
}

export function codeBlock(code: string, lang = ''): HTMLElement {
  const copyBtn = h('button', { class: 'code-copy' }, t('common.copy'));
  copyBtn.addEventListener('click', async (e) => {
    e.stopPropagation();
    if (await copyText(code)) {
      copyBtn.textContent = t('common.copied');
      setTimeout(() => (copyBtn.textContent = t('common.copy')), 1200);
    }
  });
  return h(
    'div',
    { class: 'code-block' },
    h('div', { class: 'code-head' }, h('span', {}, lang || 'text'), h('span', { class: 'spacer' }), copyBtn),
    h('pre', {}, h('code', {}, code)),
  );
}
