// Sidebar views contributed by extensions (`viewsContainers` + `views`).
//
// A view is declarative until its extension calls `window.createTreeView`.
// Until then there is nothing to draw but the section heading, so the UI must
// still show the container: otherwise the user sees "the extension installed
// but its sidebar never appeared", which is exactly the failure this module
// fixes. Once a provider registers, `exthost://tree` wakes the section up.

import { api, errText, onExtHostEvent } from '../core/api';
import { app, emit } from '../core/state';
import { h, clear } from '../core/dom';
import { t } from '../core/i18n';
import { icon, type IconName } from './icons';
import { evalWhen, extMenuRows, fileUrl, type MenuTarget } from '../core/extmenus';
import { popupMenu, toast } from './toast';
import type { ExtView, TreeItemRow } from '../types';

export interface ExtContainer {
  id: string;
  title: string;
  /** Absolute path of the container icon, or ''. */
  icon: string;
  views: ExtView[];
}

/** View ids whose extension has called `createTreeView` in this session. */
const live = new Set<string>();
const openSections = new Map<string, boolean>();
const expanded = new Set<string>();

let mountedHost: HTMLElement | null = null;
let mountedContainer = '';
let ready = false;
const earlyEvents: { action: string; viewId?: string }[] = [];

/**
 * `when` clauses on views use context keys an extension sets on activation
 * (`gradle:activated`) and mutually exclusive switches (`gradle:defaultView`).
 * Autostart makes the former true; leaving unknown keys false makes the
 * `!…defaultView` branch win, which is the primary view VS Code would show.
 */
function viewVisible(when: string): boolean {
  const clause = (when || '').trim();
  if (!clause) return true;
  const relaxed = clause.replace(/\b[A-Za-z0-9_.-]+:activated\b/gi, 'true');
  return evalWhen(relaxed, {});
}

/** Views declared against a built-in container (`views.explorer`, `views.scm`,
 *  …) have no `viewsContainers` entry of their own. Give them one shared slot
 *  rather than silently dropping them — the majority of VS Code extensions use
 *  this form. */
const EXTRA_CONTAINER = 'irixen:extraViews';

/** Containers with at least one currently-visible declared view. */
export function extContainers(): ExtContainer[] {
  const byId = new Map<string, ExtContainer>();
  for (const info of app.extensions) {
    if (!info.enabled) continue;
    for (const c of info.contributes?.viewContainers || []) {
      if (!c.id) continue;
      byId.set(c.id, { id: c.id, title: c.title || c.id, icon: c.icon || '', views: [] });
    }
  }
  for (const info of app.extensions) {
    if (!info.enabled) continue;
    for (const v of info.contributes?.views || []) {
      if (!viewVisible(v.when)) continue;
      let owner = byId.get(v.container);
      if (!owner) {
        owner = byId.get(EXTRA_CONTAINER);
        if (!owner) {
          owner = { id: EXTRA_CONTAINER, title: t('ext.title'), icon: '', views: [] };
          byId.set(EXTRA_CONTAINER, owner);
        }
      }
      owner.views.push(v);
    }
  }
  return [...byId.values()].filter((c) => c.views.length > 0);
}

export function extContainerTitle(id: string): string {
  return extContainers().find((c) => c.id === id)?.title || id;
}

export function extContainerIcon(id: string): string | undefined {
  return fileUrl(extContainers().find((c) => c.id === id)?.icon);
}

export function extContainerLive(id: string): boolean {
  const views = extContainers().find((c) => c.id === id)?.views || [];
  return views.some((v) => live.has(v.id));
}

// Hosts autostart before the shell mounts. Buffer their registration frames
// just like the status/webview fan-in does, then replay on mount.
void onExtHostEvent<{ action: string; viewId?: string }>('tree', (ev) => {
  if (!ev) return;
  if (!ready) {
    earlyEvents.push(ev);
    return;
  }
  applyTreeEvent(ev);
});

function applyTreeEvent(ev: { action: string; viewId?: string }) {
  const view = ev.viewId || '';
  if (!view) return;
  if (ev.action === 'register') live.add(view);
  else if (ev.action === 'unregister') live.delete(view);
  // `changed` means the provider fired onDidChangeTreeData: the visible rows
  // are stale. A full redraw of the small sidebar is cheaper and more honest
  // than trying to diff opaque element handles.
  if (mountedHost && mountedContainer) {
    const here = extContainers()
      .find((c) => c.id === mountedContainer)
      ?.views.some((v) => v.id === view);
    if (here) draw();
  }
  emit('extview:changed');
}

/** Mount the contributed views of one activity-bar container into `host`. */
export function mountExtViews(host: HTMLElement, containerId: string) {
  mountedHost = host;
  mountedContainer = containerId;
  ready = true;
  for (const ev of earlyEvents.splice(0)) applyTreeEvent(ev);
  void api
    .exttreeViews()
    .then((ids) => {
      for (const id of ids || []) live.add(id);
      emit('extview:changed');
      draw();
    })
    .catch(() => undefined);
  draw();
}

export function unmountExtViews() {
  mountedHost = null;
  mountedContainer = '';
}

/** Refresh button in the sidebar header. */
export async function refreshExtViews(containerId?: string) {
  const id = containerId || mountedContainer;
  const views = extContainers().find((c) => c.id === id)?.views || [];
  for (const v of views) {
    try { await api.exttreeRefresh(v.id); } catch { /* provider not up yet */ }
  }
  expanded.clear();
  if (mountedHost) draw();
}

function draw() {
  const host = mountedHost;
  if (!host) return;
  const container = extContainers().find((c) => c.id === mountedContainer);
  clear(host);
  if (!container) {
    host.append(h('div', { class: 'hint ext-view-empty' }, t('ext.view.missing')));
    return;
  }
  for (const view of container.views) host.append(section(view));
}

/* ------------------------------------------------------------------ section */

function section(view: ExtView): HTMLElement {
  if (!openSections.has(view.id)) openSections.set(view.id, view.visibility !== 'collapsed');
  const body = h('div', { class: 'ext-view-body' });
  const head = h('div', { class: 'ext-view-head' });
  const wrap = h('div', { class: 'ext-view-section', dataset: { view: view.id } }, head, body);

  const paintHead = () => {
    const isOpen = !!openSections.get(view.id);
    clear(head);
    head.append(
      h('button', {
        class: 'ext-view-toggle',
        title: isOpen ? t('ext.view.collapse') : t('ext.view.expand'),
        'aria-label': isOpen ? t('ext.view.collapse') : t('ext.view.expand'),
        onClick: () => {
          openSections.set(view.id, !isOpen);
          paintHead();
          if (!isOpen) void loadRoot(view.id, body);
          else clear(body);
        },
      }, icon(isOpen ? 'down' : 'chevron', 12)),
      h('span', { class: 'ext-view-name' }, view.name || view.id),
      ...(live.has(view.id) ? [] : [h('span', { class: 'ext-view-dot', title: t('ext.view.waiting') })]),
      h('span', { class: 'ext-view-spacer' }),
      h('button', {
        class: 'icon-btn ext-view-tool',
        title: t('common.refresh'),
        onClick: async () => {
          try { await api.exttreeRefresh(view.id); } catch { /* no provider yet */ }
          expanded.clear();
          if (openSections.get(view.id)) void loadRoot(view.id, body);
        },
      }, icon('refresh', 12)),
      h('button', {
        class: 'icon-btn ext-view-tool',
        title: t('ext.view.more'),
        onClick: (e: MouseEvent) => {
          const target: MenuTarget = { view: view.id };
          const items = extMenuRows('view/title', target);
          if (!items.length) return;
          popupMenu(e.currentTarget as HTMLElement, items, (value) => {
            if (value.startsWith('ext:')) void api.exthostRun(value.slice(4)).catch((err) => toast(errText(err), 'err'));
          });
        },
      }, icon('command', 12)),
    );
  };
  paintHead();
  if (openSections.get(view.id)) void loadRoot(view.id, body);
  return wrap;
}

/* --------------------------------------------------------------------- tree */

const THEME_ICONS: [string, IconName][] = [
  ['folder', 'files'], ['rootfolder', 'files'], ['package', 'files'], ['project', 'files'],
  ['file', 'file'], ['document', 'file'], ['book', 'book'], ['library', 'book'],
  ['play', 'play'], ['run', 'play'], ['debug', 'play'], ['terminal', 'terminal'],
  ['refresh', 'refresh'], ['sync', 'refresh'], ['loading', 'refresh'], ['history', 'history'],
  ['gear', 'settings'], ['settings', 'settings'], ['warning', 'alert'], ['error', 'alert'],
  ['check', 'check'], ['pass', 'check'], ['zap', 'zap'], ['sparkle', 'spark'],
  ['puzzle', 'puzzle'], ['extensions', 'puzzle'], ['git', 'branch'], ['branch', 'branch'],
  ['search', 'search'], ['info', 'info'], ['circle', 'dot'], ['layout', 'layout'],
];

function themeIconName(id: string): IconName {
  const bare = id.replace(/^.*\//, '').replace(/~.*$/, '').toLowerCase();
  const exact = THEME_ICONS.find(([key]) => key === bare);
  if (exact) return exact[1];
  const loose = THEME_ICONS.find(([key]) => bare.includes(key) || key.includes(bare));
  return loose ? loose[1] : 'file';
}

function itemIcon(row: TreeItemRow): HTMLElement {
  if (row.iconPath) {
    const url = fileUrl(row.iconPath);
    if (url) {
      const img = h('img', { class: 'ext-view-icon-img', src: url, alt: '' });
      img.addEventListener('error', () => img.remove(), { once: true });
      return img;
    }
  }
  const name = row.themeIcon ? themeIconName(row.themeIcon) : row.state !== 0 ? 'files' : 'file';
  return h('span', { class: 'ext-view-icon' }, icon(name, 13));
}

async function loadRoot(view: string, body: HTMLElement) {
  clear(body);
  body.append(h('div', { class: 'hint ext-view-loading' }, t('ext.view.loading')));
  try {
    const rows = await api.exttreeChildren(view);
    if (!body.isConnected) return; // section was collapsed or replaced meanwhile
    clear(body);
    if (!rows || !rows.length) {
      body.append(h('div', { class: 'hint ext-view-empty' }, t('ext.view.empty')));
      return;
    }
    for (const row of rows) body.append(node(view, row, 0));
  } catch (e) {
    if (!body.isConnected) return;
    clear(body);
    const msg = live.has(view) ? errText(e) : t('ext.view.waiting');
    body.append(h('div', { class: 'hint ext-view-empty', title: msg }, msg));
  }
}

async function toggleNode(view: string, row: TreeItemRow, kids: HTMLElement, caret: HTMLElement, depth: number) {
  const id = `${view}:${row.key}`;
  if (expanded.has(id)) {
    expanded.delete(id);
    clear(kids);
    clear(caret);
    caret.append(icon('chevron', 12));
    return;
  }
  expanded.add(id);
  clear(caret);
  caret.append(icon('down', 12));
  clear(kids);
  kids.append(h('div', { class: 'hint ext-view-loading' }, t('ext.view.loading')));
  try {
    const rows = await api.exttreeChildren(view, row.key);
    if (!kids.isConnected || !expanded.has(id)) return;
    clear(kids);
    if (!rows || !rows.length) {
      kids.append(h('div', { class: 'hint ext-view-empty', style: { paddingLeft: `${20 + depth * 14}px` } }, t('ext.view.empty')));
      return;
    }
    for (const child of rows) kids.append(node(view, child, depth + 1));
  } catch (e) {
    if (!kids.isConnected) return;
    clear(kids);
    kids.append(h('div', { class: 'hint ext-view-empty', title: errText(e) }, errText(e)));
  }
}

function node(view: string, row: TreeItemRow, depth: number): HTMLElement {
  const canExpand = row.state !== 0;
  const kids = h('div', { class: 'ext-view-kids' });
  const caret = h('span', { class: 'ext-view-caret' }, canExpand ? icon('chevron', 12) : null);
  // Placeholder items ("No tasks found") put their text in `description` and
  // leave `label` empty. Showing the opaque handle there reads like a bug.
  const label = row.label || row.description || row.key;
  const description = row.label ? row.description : '';
  const line = h('div', {
    class: 'ext-view-row',
    style: { paddingLeft: `${8 + depth * 14}px` },
    title: row.tooltip || label,
    dataset: { key: row.key, context: row.context || '' },
  },
    h('button', {
      class: `ext-view-caret-btn${canExpand ? '' : ' leaf'}`,
      'aria-label': row.label || '',
      onClick: (e: MouseEvent) => {
        e.stopPropagation();
        if (canExpand) void toggleNode(view, row, kids, caret, depth);
      },
    }, caret),
    itemIcon(row),
    h('span', { class: 'ext-view-label' }, label),
    description ? h('span', { class: 'ext-view-desc' }, description) : null,
  );

  line.addEventListener('click', () => {
    if (row.command) {
      void api.exttreeCommand(view, row.key).catch((e) => toast(errText(e), 'err'));
    } else if (canExpand) {
      void toggleNode(view, row, kids, caret, depth);
    }
  });

  line.addEventListener('contextmenu', (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const target: MenuTarget = {
      path: row.path,
      view,
      viewItem: row.context,
    };
    const items = extMenuRows('view/item/context', target);
    if (!items.length) return;
    popupMenu(line, items, (value) => {
      if (!value.startsWith('ext:')) return;
      void api.exttreeCommand(view, row.key, value.slice(4)).catch((err) => toast(errText(err), 'err'));
    }, { at: { x: e.clientX, y: e.clientY } });
  });

  const wrap = h('div', { class: 'ext-view-node' }, line, kids);
  // VS Code restores `Expanded` items on reveal.
  if (canExpand && row.state === 2) queueMicrotask(() => void toggleNode(view, row, kids, caret, depth));
  return wrap;
}
