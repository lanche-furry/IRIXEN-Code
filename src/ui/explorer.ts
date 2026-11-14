// File explorer: lazy tree, create / rename / delete, open in editor, attach to chat.

import { app, emit } from '../core/state';
import { api, errText } from '../core/api';
import { h, clear, fmtBytes, basename } from '../core/dom';
import { icon } from './icons';
import { t } from '../core/i18n';
import { detectLanguage } from '../core/languages';
import { extMenuRows, isExtMenuValue } from '../core/extmenus';
import { toast, promptDialog, confirmDialog, popupMenu } from './toast';
import { openFile } from './editor';
import { attachToChat } from './chat';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import type { FsEntry } from '../types';

let bodyEl: HTMLElement | null = null;

export function mountExplorer(container: HTMLElement) {
  bodyEl = container;
  render();
}

export async function refreshExplorer() {
  app.tree.clear();
  render();
}

/** Header shortcuts for the explorer view: new file / new folder / collapse.
 *  Mirrors what the context menu does, without hunting for a target row. */
export function explorerActions(): HTMLElement[] {
  return [
    h('button', {
      class: 'icon-btn',
      title: t('explorer.newFile'),
      onClick: () => void createIn(app.config?.workspace || '', 'file'),
    }, icon('file', 13)),
    h('button', {
      class: 'icon-btn',
      title: t('explorer.newFolder'),
      onClick: () => void createIn(app.config?.workspace || '', 'dir'),
    }, icon('folderOpen', 13)),
    h('button', {
      class: 'icon-btn',
      title: t('explorer.collapse'),
      onClick: () => {
        app.expanded.clear();
        render();
      },
    }, icon('down', 13)),
  ];
}

/** Create a file or folder under `parent` and reveal it. */
async function createIn(parent: string, kind: 'file' | 'dir') {
  if (!parent) {
    await pickFolder();
    return;
  }
  const title = kind === 'file' ? t('explorer.newFile') : t('explorer.newFolder');
  const name = await promptDialog(title, kind === 'file' ? 'untitled.ts' : 'new-folder');
  if (!name) return;
  const target = `${parent}/${name}`;
  try {
    if (kind === 'file') {
      const path = await api.createFile(target);
      app.tree.delete(parent);
      render();
      await openFile(path);
    } else {
      await api.createDir(target);
      app.tree.delete(parent);
      app.expanded.add(target);
      render();
    }
  } catch (err) {
    toast(String(err), 'err');
  }
}

export function render() {
  if (!bodyEl) return;
  clear(bodyEl);
  const root = app.config?.workspace;
  if (!root) {
    bodyEl.append(
      h(
        'div',
        { class: 'side-empty' },
        h('div', { style: { marginBottom: '10px' } }, t('explorer.empty')),
        h('button', { class: 'btn primary', onClick: () => void pickFolder() }, icon('folderOpen', 14), t('explorer.openFolder')),
      ),
    );
    return;
  }
  const tree = h('div');
  bodyEl.append(tree);
  void renderDir(tree, root, 0);
}

export async function pickFolder() {
  try {
    const picked = await openDialog({ directory: true, multiple: false, title: t('explorer.openFolder') });
    if (typeof picked === 'string' && picked) {
      const ws = await api.setWorkspace(picked);
      if (app.config) app.config.workspace = ws;
      app.tree.clear();
      app.expanded.clear();
      app.files = [];
      app.activeFile = '';
      emit('workspace');
      render();
      toast(ws, 'ok', 2000);
    }
  } catch (e) {
    toast(String(e), 'err');
  }
}

async function renderDir(container: HTMLElement, dir: string, depth: number) {
  let entries = app.tree.get(dir);
  if (!entries) {
    try {
      entries = await api.list(dir);
      app.tree.set(dir, entries);
    } catch (e) {
      container.append(h('div', { class: 'side-empty' }, String(e)));
      return;
    }
  }
  for (const entry of entries) {
    container.append(row(entry, depth));
    if (entry.is_dir && app.expanded.has(entry.path)) {
      const childBox = h('div');
      container.append(childBox);
      void renderDir(childBox, entry.path, depth + 1);
    }
  }
  if (entries.length === 0) {
    container.append(h('div', { class: 'side-empty', style: { padding: '8px 18px' } }, '—'));
  }
}

function row(entry: FsEntry, depth: number): HTMLElement {
  const isOpen = app.expanded.has(entry.path);
  const caret = h('span', { class: `tree-caret ${isOpen ? 'open' : ''}` });
  if (entry.is_dir) caret.append(icon('chevron', 12));

  const nameEl = h('span', { class: `tree-name ${entry.is_dir ? 'dir' : ''}` }, entry.name);
  const el = h(
    'div',
    {
      class: `tree-row ${app.activeFile === entry.path ? 'active' : ''}`,
      style: { paddingLeft: `${10 + depth * 13}px` },
      title: entry.path,
      onClick: () => void onRowClick(entry),
      onContextmenu: (e: MouseEvent) => {
        e.preventDefault();
        void contextMenu(e, entry);
      },
    },
    caret,
    nameEl,
    entry.is_dir ? null : h('span', { class: 'tree-size' }, fmtBytes(entry.size)),
  );
  return el;
}

async function onRowClick(entry: FsEntry) {
  if (entry.is_dir) {
    if (app.expanded.has(entry.path)) app.expanded.delete(entry.path);
    else app.expanded.add(entry.path);
    render();
  } else {
    await openFile(entry.path);
  }
}

async function contextMenu(e: MouseEvent, entry: FsEntry) {
  const items = [
    { label: t('explorer.newFile'), value: 'newfile' },
    { label: t('explorer.newFolder'), value: 'newdir' },
    { label: t('explorer.rename'), value: 'rename' },
    { label: t('explorer.delete'), value: 'delete' },
    { label: t('chat.attach'), value: 'attach' },
    // Extension rows: `contributes.menus` → `explorer/context`, filtered by
    // their `when` clause against the entry that was clicked.
    ...extMenuRows('explorer/context', {
      path: entry.path,
      lang: entry.is_dir ? undefined : detectLanguage(entry.path),
      isDir: entry.is_dir,
    }),
  ];
  popupMenu(
    document.body,
    items,
    async (action) => {
      if (isExtMenuValue(action)) {
        await api.exthostRun(action.slice(4)).catch((err) => toast(errText(err), 'err', 3200));
        return;
      }
      const parent = entry.is_dir ? entry.path : entry.path.replace(/[\\/][^\\/]+$/, '');
      if (action === 'newfile' || action === 'newdir') {
        await createIn(parent, action === 'newfile' ? 'file' : 'dir');
      } else if (action === 'rename') {
        const name = await promptDialog(t('explorer.rename'), entry.name, entry.name);
        if (!name || name === entry.name) return;
        const to = `${parent}/${name}`;
        try {
          await api.rename(entry.path, to);
          app.tree.clear();
          render();
        } catch (err) {
          toast(String(err), 'err');
        }
      } else if (action === 'delete') {
        const ok = await confirmDialog(t('explorer.delete'), entry.path);
        if (!ok) return;
        try {
          await api.remove(entry.path);
          app.tree.clear();
          render();
          toast(t('explorer.delete'), 'ok');
        } catch (err) {
          toast(String(err), 'err');
        }
      } else if (action === 'attach') {
        attachToChat(entry.path, basename(entry.path), entry.is_dir);
      }
    },
    { search: false, at: { x: e.clientX, y: e.clientY } },
  );
}
