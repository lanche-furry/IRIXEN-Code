// Review panel — the workspace's uncommitted changes, file by file, with a
// one-click hand-off to the agent for an actual code review.

import { app, emit } from '../core/state';
import { api } from '../core/api';
import { h, clear } from '../core/dom';
import { icon } from './icons';
import { t } from '../core/i18n';
import { toast } from './toast';
import { sendPrompt } from './chat';
import type { GitFileStatus } from '../types';

let root: HTMLElement | null = null;
let listEl: HTMLElement | null = null;
let diffEl: HTMLElement | null = null;
let headCountEl: HTMLElement | null = null;

let files: GitFileStatus[] = [];
let selected = '';
let loading = false;

/** Number of changed files, for the rail badge. */
export function reviewCount(): number {
  return app.git.length;
}

export function mountReview(dock: HTMLElement) {
  root = h('div', { class: 'review', dataset: { panel: 'review' } });

  headCountEl = h('span', { class: 'chat-title' }, t('review.title'));

  const head = h(
    'div',
    { class: 'chat-head' },
    headCountEl,
    h('div', { class: 'spacer', style: { flex: '1' } }),
    h('button', {
      class: 'picker',
      onClick: () => void askAgent(),
    }, icon('spark', 12), h('span', { class: 'val' }, t('review.askAgent'))),
    h('button', {
      class: 'icon-btn',
      title: t('common.refresh'),
      onClick: () => void refreshReview(),
    }, icon('refresh', 13)),
  );

  listEl = h('div', { class: 'review-list' });
  diffEl = h('div', { class: 'review-diff' });
  diffEl.append(h('div', { class: 'side-empty' }, t('review.pickFile')));

  root.append(head, listEl, diffEl);
  dock.append(root);
}

export async function refreshReview() {
  if (!root) return;
  if (loading) return;
  loading = true;
  try {
    const repo = await api.gitIsRepo();
    app.gitRepo = repo;
    app.git = repo ? await api.gitStatus() : [];
    files = app.git;
    if (repo) app.gitBranch = await api.gitBranch();
  } catch {
    files = [];
    app.git = [];
  } finally {
    loading = false;
  }

  if (headCountEl) {
    const n = files.length;
    headCountEl.textContent = `${t('review.title')}${n ? ` · ${n}` : ''}`;
  }
  paintList();
  emit('review:count', files.length);

  if (!files.length) {
    clear(diffEl!);
    diffEl?.append(h('div', { class: 'side-empty' }, app.gitRepo ? t('review.noChanges') : t('review.notRepo')));
    return;
  }
  const keep = files.find((f) => f.path === selected);
  const next = keep?.path || files[0].path;
  if (next !== selected || !diffEl?.querySelector('.diff-line')) await selectFile(next);
}

function paintList() {
  if (!listEl) return;
  clear(listEl);
  if (!files.length) {
    listEl.append(h('div', { class: 'side-empty', style: { padding: '14px 12px', textAlign: 'left' } },
      app.gitRepo ? t('review.noChanges') : t('review.notRepo')));
    return;
  }
  for (const f of files) {
    const name = f.path.split(/[\\/]/).pop() || f.path;
    const dir = f.path.slice(0, f.path.length - name.length).replace(/[\\/]$/, '');
    const kind = f.code.includes('?') || f.code.includes('A') ? 'added' : f.code.includes('D') ? 'deleted' : '';
    listEl.append(
      h('div', {
        class: `review-row${f.path === selected ? ' on' : ''}`,
        title: f.path,
        onClick: () => void selectFile(f.path),
      },
        h('span', { class: `git-code ${kind}` }, f.code.trim() || '·'),
        h('span', { class: 'fname' }, name),
        dir ? h('span', { class: 'muted', style: { fontSize: '12.5px', marginLeft: 'auto' } }, dir) : null,
      ),
    );
  }
}

async function selectFile(path: string) {
  const f = files.find((x) => x.path === path);
  if (!f || !diffEl) return;
  selected = path;
  paintList();
  clear(diffEl);
  diffEl.append(h('div', { class: 'side-empty' }, t('common.loading')));
  let diff = '';
  try {
    diff = await api.gitDiff(f.staged, path);
  } catch (e) {
    clear(diffEl);
    diffEl.append(h('div', { class: 'side-empty' }, String(e)));
    return;
  }
  clear(diffEl);
  if (!diff.trim()) {
    diffEl.append(h('div', { class: 'side-empty' }, t('review.emptyDiff')));
    return;
  }
  let adds = 0;
  let dels = 0;
  for (const line of diff.split('\n')) {
    if (line.startsWith('+') && !line.startsWith('+++')) adds++;
    else if (line.startsWith('-') && !line.startsWith('---')) dels++;
    const cls = line.startsWith('+') && !line.startsWith('+++') ? 'add'
      : line.startsWith('-') && !line.startsWith('---') ? 'del'
        : line.startsWith('@@') ? 'hunk' : '';
    diffEl.append(h('div', { class: `diff-line ${cls}` }, line || ' '));
  }
  diffEl.scrollTop = 0;
  const stat = h('span', { class: 'review-stat' }, `+${adds} / −${dels}`);
  diffEl.prepend(stat);
}

async function askAgent() {
  if (!files.length) {
    toast(t('review.noChanges'), 'info');
    return;
  }
  const list = files.map((f) => `- ${f.code.trim() || 'M'} ${f.path}`).join('\n');
  const prompt = [
    t('review.promptIntro'),
    '',
    list,
    '',
    t('review.promptRules'),
  ].join('\n');
  await sendPrompt(prompt);
}
