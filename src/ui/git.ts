// Source control panel: status, stage/unstage, commit, push/pull, diff viewer.

import { app, emit } from '../core/state';
import { api } from '../core/api';
import { h, clear } from '../core/dom';
import { icon } from './icons';
import { t } from '../core/i18n';
import { toast } from './toast';

let bodyEl: HTMLElement | null = null;
let commitText = '';

export function mountGit(container: HTMLElement) {
  bodyEl = container;
  void refreshGit();
}

export async function refreshGit() {
  if (!bodyEl) return;
  try {
    app.gitRepo = await api.gitIsRepo();
    if (app.gitRepo) {
      app.git = await api.gitStatus();
      app.gitBranch = await api.gitBranch();
    } else {
      app.git = [];
      app.gitBranch = '';
    }
  } catch {
    app.gitRepo = false;
  }
  render();
}

function render() {
  if (!bodyEl) return;
  clear(bodyEl);

  if (!app.gitRepo) {
    bodyEl.append(
      h(
        'div',
        { class: 'side-empty' },
        h('div', { style: { marginBottom: '12px' } }, t('git.notRepo')),
        h('button', { class: 'btn primary', onClick: async () => { try { await api.gitInit(); await refreshGit(); } catch (e) { toast(String(e), 'err'); } } }, icon('git', 14), t('git.init')),
      ),
    );
    return;
  }

  const branchRow = h(
    'div',
    { class: 'git-branch' },
    h('span', { class: 'dot' }),
    h('span', {}, app.gitBranch || '—'),
    h('span', { class: 'spacer' }),
    h('button', { class: 'icon-btn', title: t('git.pull'), onClick: async () => { try { toast(await api.gitPull(), 'ok'); await refreshGit(); } catch (e) { toast(String(e), 'err'); } } }, icon('down', 13)),
    h('button', { class: 'icon-btn', title: t('git.push'), onClick: async () => { try { toast(await api.gitPush(), 'ok'); } catch (e) { toast(String(e), 'err'); } } }, icon('send', 13)),
    h('button', { class: 'icon-btn', title: t('git.refresh'), onClick: () => void refreshGit() }, icon('refresh', 13)),
  );

  const box = h('textarea', {
    placeholder: t('git.commitPlaceholder'),
    onInput: (e: Event) => (commitText = (e.target as HTMLTextAreaElement).value),
    onKeydown: (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') void doCommit();
    },
  });
  box.value = commitText;

  const commitBtn = h(
    'button',
    { class: 'btn primary', onClick: () => void doCommit() },
    icon('check', 13),
    t('git.commit'),
  );

  const staged = app.git.filter((f) => f.staged);
  const unstaged = app.git.filter((f) => !f.staged);

  const wrap = h(
    'div',
    { class: 'git-wrap' },
    branchRow,
    h('div', { class: 'commit-box' }, box, h('div', { class: 'row' }, commitBtn, h('span', { class: 'spacer' }), h('span', { class: 'muted', style: { fontSize: '13.5px' } }, `${app.git.length}`))),
  );

  if (staged.length) {
    wrap.append(
      h('div', { class: 'git-section' }, `${t('git.stagedChanges')} · ${staged.length}`),
      ...staged.map((f) => fileRow(f.path, f.code, true)),
    );
  }
  if (unstaged.length) {
    wrap.append(
      h(
        'div',
        { class: 'git-section' },
        `${t('git.unstagedChanges')} · ${unstaged.length}`,
        h('span', { class: 'spacer' }),
        h('button', { class: 'icon-btn', title: t('git.stageAll'), onClick: async () => { try { await api.gitStage(unstaged.map((f) => f.path)); await refreshGit(); } catch (e) { toast(String(e), 'err'); } } }, icon('plus', 13)),
      ),
      ...unstaged.map((f) => fileRow(f.path, f.code, false)),
    );
  }
  if (!staged.length && !unstaged.length) {
    wrap.append(h('div', { class: 'side-empty', style: { padding: '18px 0' } }, t('git.noChanges')));
  }

  bodyEl.append(wrap);
}

function fileRow(path: string, code: string, staged: boolean): HTMLElement {
  const name = path.split(/[\\/]/).pop() || path;
  const dir = path.slice(0, path.length - name.length).replace(/[\\/]$/, '');
  const cls = code.includes('?') || code.includes('A') ? 'added' : code.includes('D') ? 'deleted' : '';
  return h(
    'div',
    {
      class: 'git-file',
      title: path,
      onClick: () => void showDiff(staged, path),
    },
    h('span', { class: `git-code ${cls}` }, code.trim() || '·'),
    h('span', { class: 'fname' }, name),
    dir ? h('span', { class: 'muted', style: { fontSize: '12.5px', marginLeft: 'auto' } }, dir.split(/[\\/]/).pop() || '') : null,
    h('button', {
      class: 'icon-btn',
      title: staged ? t('git.unstage') : t('git.stage'),
      onClick: async (e: Event) => {
        e.stopPropagation();
        try {
          if (staged) await api.gitUnstage([path]);
          else await api.gitStage([path]);
          await refreshGit();
        } catch (err) {
          toast(String(err), 'err');
        }
      },
    }, icon(staged ? 'close' : 'plus', 12)),
  );
}

async function doCommit() {
  if (!commitText.trim()) {
    toast(t('git.commitPlaceholder'), 'err');
    return;
  }
  try {
    const out = await api.gitCommit(commitText.trim());
    commitText = '';
    toast(out.split('\n')[0] || 'ok', 'ok');
    await refreshGit();
    emit('git:changed');
  } catch (e) {
    toast(String(e), 'err');
  }
}

async function showDiff(staged: boolean, path: string) {
  let diff = '';
  try {
    diff = await api.gitDiff(staged, path);
  } catch (e) {
    toast(String(e), 'err');
    return;
  }
  const overlay = h('div', { class: 'modal' });
  const body = h('div', { class: 'diff-view', style: { maxHeight: '62vh', overflow: 'auto' } });
  for (const line of diff.split('\n')) {
    const cls = line.startsWith('+') && !line.startsWith('+++') ? 'add' : line.startsWith('-') && !line.startsWith('---') ? 'del' : line.startsWith('@@') ? 'hunk' : '';
    body.append(h('div', { class: `diff-line ${cls}` }, line || ' '));
  }
  const card = h(
    'div',
    { style: { width: 'min(920px, 94vw)', background: 'var(--bg-1)', border: '1px solid var(--border-strong)', borderRadius: '12px', overflow: 'hidden', display: 'grid', gridTemplateRows: '40px 1fr' } },
    h(
      'div',
      { class: 'row', style: { padding: '0 12px', borderBottom: '1px solid var(--border)' } },
      h('span', { class: 'mono', style: { fontSize: '14px', color: 'var(--text-1)' } }, path),
      h('span', { class: 'spacer' }),
      h('button', { class: 'icon-btn', title: t('common.close'), onClick: () => overlay.remove() }, icon('close', 14)),
    ),
    body,
  );
  overlay.append(card);
  overlay.addEventListener('click', (e) => {
    if (e.target === overlay) overlay.remove();
  });
  document.body.append(overlay);
}
