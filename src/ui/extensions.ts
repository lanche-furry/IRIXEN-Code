// Extensions panel — installed extensions (with the Node host switches) and a
// built-in store backed by Open VSX.
//
// Two pages in one panel rather than two panels: installing is a shelf action,
// and the thing you install lands on the page next door, which is where you go
// to run it. Keeping them in the same container means the transition is instant
// and the store keeps its scroll position.

import { app, emit, on } from '../core/state';
import { api, errText } from '../core/api';
import { h, clear, fmtNum, fmtBytes } from '../core/dom';
import { icon } from './icons';
import { t } from '../core/i18n';
import { toast, confirmDialog, promptDialog, errorBox } from './toast';
import { applyExtensionTheme, applyTheme as applyAppTheme } from './theme';
import { renderMarkdown } from './markdown';
import { fileUrl } from '../core/extmenus';
import type { ExtDetail, ExtInspect, ExtSearchRow, ExtUpdate, ExtensionInfo, HostStatus } from '../types';

type Page = 'installed' | 'store';
type Sort = 'downloads' | 'rating' | 'recent';

let root: HTMLElement | null = null;
let listEl: HTMLElement | null = null;
let storeEl: HTMLElement | null = null;
let countEl: HTMLElement | null = null;
let bannerEl: HTMLElement | null = null;
let tabBtns: HTMLElement[] = [];
let sourceInput: HTMLInputElement | null = null;
let busy = false;
let updates: ExtUpdate[] = [];
/** Which extension owns the palette currently previewed on <html>. */
let previewOwner: string | null = null;
let updateChecking = false;

let page: Page = 'installed';

const store = {
  query: '',
  category: 'all',
  sort: 'downloads' as Sort,
  rows: [] as ExtSearchRow[],
  selected: '',
  detail: null as ExtDetail | null,
  loading: false,
  error: '',
};

export function mountExtensions(dock: HTMLElement) {
  root = h('div', { class: 'ext', dataset: { panel: 'extensions' } });

  countEl = h('span', { class: 'ext-count' }, '');
  const tabs = h('div', { class: 'ext-tabs' },
    ...(['installed', 'store'] as Page[]).map((p) =>
      h('button', {
        class: `ext-tab${page === p ? ' on' : ''}`,
        dataset: { page: p },
        onClick: () => setPage(p),
      }, icon(p === 'store' ? 'store' : 'puzzle', 12), t(p === 'store' ? 'ext.store' : 'ext.installed')),
    ),
  );
  tabBtns = [...tabs.querySelectorAll('.ext-tab')] as HTMLElement[];

  // `ext-head` keeps the page switcher pinned above the panes. The store used
  // to be absolutely positioned over the whole panel, so its own toolbar was
  // painted on top of these two buttons and swallowed the clicks.
  const head = h('div', { class: 'chat-head ext-head' },
    h('span', { class: 'chat-title' }, t('ext.title')),
    countEl,
    tabs,
    h('div', { class: 'spacer', style: { flex: '1' } }),
    h('button', {
      class: 'icon-btn',
      title: t('adv.addonsOpen'),
      onClick: () => void api.openAddonsFolder().catch(() => {}),
    }, icon('folderOpen', 13)),
    h('button', {
      class: 'icon-btn',
      title: t('ext.checkUpdates'),
      onClick: () => void refreshExtensions(true),
    }, icon('refresh', 13)),
  );

  bannerEl = h('div', { class: 'ext-banner-wrap' });
  listEl = h('div', { class: 'ext-body' });
  storeEl = h('div', { class: 'store', hidden: true });

  root.append(head, bannerEl, listEl, storeEl);
  dock.append(root);

  void refreshExtensions();
  void refreshNode();
  void refreshHosts();
  // Advisory check on every cold start; the Rust side caches the answer for ten
  // minutes, so reopening the panel does not hammer the registry.
  void checkUpdates();
}

function setPage(next: Page) {
  page = next;
  for (const b of tabBtns) b.classList.toggle('on', b.dataset.page === next);
  if (listEl) listEl.hidden = next !== 'installed';
  if (storeEl) storeEl.hidden = next !== 'store';
  if (next === 'store' && !store.rows.length && !store.loading) void loadStore(true);
}

/* ------------------------------------------------------------------ node */

async function refreshNode() {
  try {
    app.node = await api.exthostNode();
  } catch {
    app.node = null;
  }
  paintBanner();
}

function paintBanner() {
  if (!bannerEl) return;
  clear(bannerEl);
  const node = app.node;
  if (!node || node.found) return;

  const installing = h('button', { class: 'btn primary' }, t('ext.host.installNode')) as HTMLButtonElement;

  bannerEl.append(
    h('div', { class: 'ext-banner' },
      icon('alert', 14),
      h('div', { class: 'ext-banner-text' },
        h('b', null, t('ext.host.nodeMissing')),
      ),
      h('div', { class: 'row', style: { gap: '6px' } },
        h('button', {
          class: 'btn small',
          onClick: () => void pickNode(),
        }, t('ext.host.specifyNode')),
        installing,
      ),
    ),
  );

  installing.onclick = async () => {
    installing.disabled = true;
    installing.textContent = t('ext.host.installingNode');
    try {
      const info = await api.exthostInstallNode();
      app.node = info;
      toast(t('ext.host.nodeReady', { v: info.version }), 'ok', 2600);
      paintBanner();
    } catch (e) {
      toast(errText(e), 'err', 5000);
      installing.disabled = false;
      installing.textContent = t('ext.host.installNode');
    }
  };
}

async function pickNode() {
  const path = await promptDialog(t('ext.host.specifyNode'), 'C:\\Program Files\\nodejs\\node.exe', app.config?.extensions?.node_path || '');
  if (!path || !app.config) return;
  app.config.extensions = { ...(app.config.extensions || {}), node_path: path } as any;
  try {
    app.config = await api.saveConfig(app.config);
    await refreshNode();
    toast(t('ext.host.nodeSaved'), 'ok', 2000);
  } catch (e) {
    toast(errText(e), 'err');
  }
}

/* ------------------------------------------------------------- installed */

export async function refreshExtensions(withUpdateCheck = false) {
  if (!listEl) return;
  try {
    const items = await api.extensionsList();
    app.extensions = items;
    renderList(items);
    emit('extensions:count', items.length);
  } catch (e) {
    clear(listEl);
    listEl.append(errorBox(t('ext.loadFailed'), errText(e), () => void refreshExtensions()));
  }
  void refreshHosts();
  if (withUpdateCheck) void checkUpdates(true);
}

/** Version check is advisory: a missing/slow registry must not become an error. */
async function checkUpdates(force = false) {
  if (updateChecking) return;
  updateChecking = true;
  try {
    updates = await api.extensionsCheckUpdates(force);
    if (page === 'installed' && app.extensions.length) renderList(app.extensions);
    paintCount();
  } catch {
    /* offline registry — keep the panel usable */
  } finally {
    updateChecking = false;
  }
}

function paintCount() {
  if (!countEl) return;
  const total = app.extensions.length;
  const n = updates.length;
  countEl.textContent = total ? `${total}${n ? ` · ${n} ↑` : ''}` : '';
}

async function updateExt(info: ExtensionInfo, latest: string) {
  if (busy) return;
  busy = true;
  try {
    // Replacing a running extension's files under the host is a bad idea.
    await api.exthostStop(info.id).catch(() => undefined);
    await api.extensionsInstall(`${info.id}@${latest}`);
    updates = updates.filter((u) => u.id !== info.id);
    toast(`${info.display_name || info.id} → v${latest}`, 'ok', 2000);
    await refreshExtensions();
  } catch (e) {
    toast(errText(e), 'err', 4200);
  } finally {
    busy = false;
  }
}

/**
 * Re-download a package whose files are incomplete.
 *
 * `broken` means `package.json` names a `main` that never made it onto disk —
 * usually an install that was interrupted, or a package unpacked by an older
 * build with a smaller byte budget. Reinstalling at the same version is the
 * only thing that can fix it, and it has to happen before activation is
 * attempted again.
 */
async function repair(info: ExtensionInfo) {
  if (busy) return;
  busy = true;
  toast(t('ext.repairing'), 'ok', 1800);
  try {
    await api.exthostStop(info.id).catch(() => undefined);
    const spec = info.version ? `${info.id}@${info.version}` : info.id;
    const fresh = await api.extensionsInstall(spec);
    toast(`${t('ext.repaired')} ${fresh.display_name || fresh.id}`, 'ok', 2400);
    await refreshExtensions();
    // A repaired extension that stays stopped is the same dead end the user
    // just paid to escape, so bring it straight back up.
    await api.exthostStart(fresh.id).catch(() => undefined);
    await refreshHosts();
  } catch (e) {
    toast(errText(e), 'err', 5000);
  } finally {
    busy = false;
  }
}

async function refreshHosts() {
  try {
    app.hosts = await api.exthostStatus();
    app.extCommands = await api.exthostCommands();
  } catch {
    app.hosts = [];
    app.extCommands = [];
  }
  if (page === 'installed') renderList(app.extensions);
}

function hostOf(id: string): HostStatus | undefined {
  return app.hosts.find((s) => s.id === id);
}

function renderList(items: ExtensionInfo[]) {
  if (!listEl) return;
  clear(listEl);
  paintCount();
  if (items.length === 0) {
    listEl.append(h('div', { class: 'hint' }, t('ext.none')));
    return;
  }

  sourceInput = h('input', {
    class: 'mono-input',
    placeholder: t('ext.installHint'),
    onKeydown: (e: KeyboardEvent) => {
      if (e.key === 'Enter') void install();
    },
  }) as HTMLInputElement;

  listEl.append(
    h('div', { class: 'ext-install' },
      sourceInput,
      h('button', { class: 'btn primary', onClick: () => void install() },
        icon('download', 12), t('ext.install')),
    ),
    h('div', { class: 'ext-section-title' }, t('ext.installed')),
  );

  for (const info of items) listEl.append(card(info));
}

/**
 * Extension icon. Marketplace-style monogram when the package ships none —
 * VS Code shows the publisher's icon here too, so a missing one should not
 * leave a blank square.
 */
function extIcon(info: ExtensionInfo): HTMLElement {
  const letter = (info.display_name || info.name || info.id).trim().charAt(0).toUpperCase() || '?';
  const monogram = () => h('span', { class: 'ext-ico ext-ico-letter' }, letter);
  const url = fileUrl(info.icon);
  if (!url) return monogram();
  const img = h('img', { class: 'ext-ico', src: url, alt: '', loading: 'lazy', draggable: 'false' }) as HTMLImageElement;
  // A package can declare an icon it does not ship; fall back instead of
  // leaving a broken-image glyph in the list.
  img.addEventListener('error', () => img.replaceWith(monogram()), { once: true });
  return img;
}

function card(info: ExtensionInfo) {
  const chips: HTMLElement[] = [];
  const c = info.contributes;
  const chip = (label: string, n: number) => {
    if (n > 0) chips.push(h('span', { class: 'ext-chip' }, `${label} ${n}`));
  };
  chip(t('ext.themes'), c.themes.length);
  chip(t('ext.snippets'), c.snippets.length);
  chip(t('ext.languages'), c.languages.length);
  chip(t('ext.commands'), c.commands.length);
  chip(t('ext.skills'), c.skills.length);

  // The card itself stays compact; the full manifest, contributions and
  // README live in a modal so the list never turns into a wall of metadata.
  const actions = h('div', { class: 'ext-actions' },
    h('button', {
      class: 'btn',
      title: t('ext.detail.title'),
      onClick: () => void openInstalledDetail(info.id),
    }, icon('info', 12), t('ext.detail')),
  );

  if (c.themes.length > 0) {
    const sel = h('select', { class: 'ext-theme-sel' },
      ...c.themes.map((th) => h('option', { value: th.label || th.id }, th.label || th.id)),
    ) as HTMLSelectElement;
    actions.append(sel, h('button', { class: 'btn', onClick: () => void applyTheme(info, sel.value) }, t('ext.applyTheme')));
  }

  const up = updates.find((u) => u.id.toLowerCase() === info.id.toLowerCase());
  if (up) {
    actions.append(
      h('span', { class: 'ext-update-tag' }, `v${up.current} → v${up.latest}`),
      h('button', { class: 'btn primary', onClick: () => void updateExt(info, up.latest) },
        icon('download', 12), t('ext.update')),
    );
  }

  actions.append(
    h('button', { class: 'btn', onClick: () => void toggle(info) }, info.enabled ? t('ext.disable') : t('ext.enable')),
    h('button', { class: 'btn danger', onClick: () => void remove(info) }, t('ext.uninstall')),
  );

  // A package whose `main` never landed on disk cannot do anything, and every
  // symptom it produces ("Cannot find module", a host that starts and dies)
  // reads like our bug. Say what is wrong and offer the one useful verb.
  const repairBtn = info.broken
    ? h('button', { class: 'btn primary', onClick: () => void repair(info) }, icon('refresh', 12), t('ext.repair'))
    : null;
  if (repairBtn) actions.prepend(repairBtn);

  const body = h('div', { class: 'ext-detail', hidden: openCardId !== info.id });
  const hasHost = !!info.entry;

  // The list is rebuilt on every host event, so an open output pane has to be
  // redrawn from the freshly created nodes — holding on to the old element
  // would silently send live log lines into a detached tree.
  if (openCardId === info.id) setTimeout(() => drawDetailBody(info, body), 0);

  return h('div', { class: `ext-card${info.enabled ? '' : ' off'}`, dataset: { ext: info.id } },
    h('div', { class: 'ext-card-head' },
      extIcon(info),
      h('span', { class: 'ext-name' }, info.display_name || info.id),
      h('span', { class: 'ext-ver' }, `v${info.version || '—'}`),
      h('span', { class: `ext-pill ${info.kind}` }, info.kind === 'vscode' ? t('ext.kindVscode') : t('ext.kindDsh')),
      info.source === 'git' ? h('span', { class: 'ext-pill src' }, 'git') : null,
      info.broken ? h('span', { class: 'ext-pill broken', title: t('ext.brokenHint') }, t('ext.broken')) : null,
    ),
    h('div', { class: 'ext-id' }, `${info.id} · ${info.publisher}`),
    info.description ? h('div', { class: 'ext-desc' }, info.description) : null,
    chips.length ? h('div', { class: 'ext-chips' }, ...chips) : null,
    hasHost ? hostBar(info) : null,
    actions,
    body,
  );

  function hostBar(info: ExtensionInfo): HTMLElement {
    const st = hostOf(info.id);
    const state = st?.state || 'stopped';
    const dot = h('span', { class: `host-dot ${state}` });
    const label = st?.error
      ? h('span', { class: 'host-label err', title: st.error }, st.error.slice(0, 90))
      : h('span', { class: 'host-label' }, t(`ext.host.${state}`, { pid: st?.pid ? String(st.pid) : '—' }));

    const toggleBtn = h('button', {
      class: `btn small${state === 'running' || state === 'starting' ? '' : ' primary'}`,
      onClick: async (e: Event) => {
        const btn = e.currentTarget as HTMLButtonElement;
        btn.disabled = true;
        try {
          if (state === 'running' || state === 'starting') {
            await api.exthostStop(info.id);
            toast(t('ext.host.stopped'), 'ok', 1600);
          } else {
            btn.textContent = t('ext.host.starting');
            const s = await api.exthostStart(info.id);
            toast(s.error || t('ext.host.running', { pid: String(s.pid || '') }), s.error ? 'err' : 'ok', 2600);
          }
        } catch (err) {
          toast(errText(err), 'err', 5000);
        } finally {
          btn.disabled = false;
          await refreshHosts();
        }
      },
    },
      icon(state === 'running' || state === 'starting' ? 'stop' : 'play', 11),
      state === 'running' || state === 'starting' ? t('ext.host.stop') : t('ext.host.start'),
    );

    const infoBtn = h('button', {
      class: 'btn small',
      onClick: () => {
        const open = body.hidden;
        body.hidden = !open;
        if (open) {
          openCardId = info.id;
          drawDetailBody(info, body);
        } else if (openCardId === info.id) {
          openCardId = '';
        }
      },
    }, t('ext.host.output'));

    return h('div', { class: 'ext-host' },
      dot, label,
      h('div', { class: 'spacer', style: { flex: '1' } }),
      infoBtn, toggleBtn,
    );
  }
}

/**
 * Commands + output tail for one card. Module scope (not a closure inside
 * `card`) because an open pane has to be repainted when the host registers a
 * command after the card was created.
 */
function drawDetailBody(info: ExtensionInfo, body: HTMLElement) {
  clear(body);
  const cmds = app.extCommands.filter((c) => c.ext === info.id);
  const logBox = h('pre', { class: 'ext-log' }, t('common.loading'));

  body.append(
    h('div', { class: 'ext-section-title' }, `${t('ext.commands')} · ${cmds.length}`),
    cmds.length === 0
      ? h('div', { class: 'hint' }, t('ext.host.noCommands'))
      : h('div', { class: 'ext-cmds' }, ...cmds.map((cmd) =>
          h('button', {
            class: `ext-cmd${cmd.live ? '' : ' dead'}`,
            title: cmd.id,
            onClick: () => void runCommand(cmd.id, cmd.title),
          },
            icon('command', 11),
            h('span', { class: 'ext-cmd-title' }, cmd.title),
            h('span', { class: 'ext-cmd-id' }, cmd.id),
          ),
        )),
    h('div', { class: 'ext-section-title row' },
      h('span', null, t('ext.host.output')),
      h('div', { class: 'spacer', style: { flex: '1' } }),
      h('button', {
        class: 'icon-btn',
        title: t('ext.host.clear'),
        onClick: async () => {
          await api.exthostClearLogs(info.id);
          await drawLogs(info.id, logBox);
        },
      }, icon('trash', 11)),
    ),
    logBox,
  );

  void drawLogs(info.id, logBox);
}

/** Repaints the open output pane, if any, from the current card in the DOM. */
function redrawOpen() {
  if (!openCardId || !listEl) return;
  const node = listEl.querySelector<HTMLElement>(`.ext-card[data-ext="${openCardId}"] .ext-detail`);
  const info = app.extensions.find((e) => e.id === openCardId);
  if (node && info) drawDetailBody(info, node);
}

async function drawLogs(id: string, box: HTMLElement) {
  try {
    const rows = await api.exthostLogs(id);
    clear(box);
    if (rows.length === 0) {
      box.append(h('span', { class: 'hint' }, t('ext.host.noOutput')));
      return;
    }
    for (const r of rows.slice(-300)) {
      box.append(h('div', { class: `ext-log-line ${r.level}` }, r.text));
    }
    box.scrollTop = box.scrollHeight;
  } catch (e) {
    clear(box);
    box.append(errorBox(t('ext.host.logFailed'), errText(e), () => void drawLogs(id, box)));
  }
}

async function runCommand(id: string, title: string) {
  try {
    const res = await api.exthostRun(id);
    toast(`${title} ✓${res === null || res === undefined ? '' : ` · ${String(res).slice(0, 80)}`}`, 'ok', 1800);
  } catch (e) {
    toast(errText(e), 'err', 4200);
  }
}

async function install(source?: string) {
  const value = (source ?? sourceInput?.value ?? '').trim();
  if (!value || busy) return;
  busy = true;
  toast(t('ext.installing'), 'ok', 1400);
  try {
    const info = await api.extensionsInstall(value);
    if (sourceInput) sourceInput.value = '';
    toast(`${t('ext.installed')} ${info.display_name || info.id}`, 'ok', 1800);
    markInstalled(info.id, info.version);
    await refreshExtensions();
    emit('skills:changed', true);
    if (store.selected && store.selected === info.id) void openDetail(info.id, true);
  } catch (e) {
    toast(errText(e), 'err', 4200);
  } finally {
    busy = false;
  }
}

async function toggle(info: ExtensionInfo) {
  try {
    await api.extensionsToggle(info.id, !info.enabled);
    if (info.enabled) {
      await api.exthostStop(info.id).catch(() => undefined);
      restoreExtensionState(info);
    }
    await refreshExtensions();
  } catch (e) {
    toast(errText(e), 'err');
  }
}

async function remove(info: ExtensionInfo) {
  const ok = await confirmDialog(t('ext.uninstall'), info.display_name || info.id);
  if (!ok) return;
  try {
    await api.exthostStop(info.id).catch(() => undefined);
    await api.extensionsUninstall(info.id);
    restoreExtensionState(info);
    await refreshExtensions();
    if (store.selected === info.id) void openDetail(info.id, true);
  } catch (e) {
    toast(errText(e), 'err');
  }
}

/**
 * A contributed palette is applied as inline variables on <html>, not written
 * to config. Removing (or disabling) its extension therefore has to put the
 * configured theme back, otherwise the UI keeps wearing a dead extension's
 * colours until the next restart. Other contributions (skills, snippets)
 * refresh through the same events.
 */
function restoreExtensionState(info: ExtensionInfo) {
  if (previewOwner === info.id) {
    previewOwner = null;
    applyAppTheme(app.config);
  }
  emit('config:changed');
  emit('skills:changed', true);
}

async function applyTheme(info: ExtensionInfo, label: string) {
  try {
    const vars = await api.extensionsTheme(info.id, label);
    if (!vars || Object.keys(vars).length === 0) {
      toast(t('ext.themeEmpty'), 'err', 3000);
      return;
    }
    applyExtensionTheme(vars);
    previewOwner = info.id;
    toast(t('ext.themeApplied'), 'ok', 1800);
  } catch (e) {
    toast(errText(e), 'err');
  }
}

/* ----------------------------------------------------------------- store */

const CATEGORIES: { id: string; label: string; api: string }[] = [
  { id: 'all', label: 'store.cat.all', api: '' },
  { id: 'themes', label: 'store.cat.themes', api: 'Themes' },
  { id: 'languages', label: 'store.cat.languages', api: 'Programming Languages' },
  { id: 'snippets', label: 'store.cat.snippets', api: 'Snippets' },
  { id: 'formatters', label: 'store.cat.formatters', api: 'Formatters' },
  { id: 'linters', label: 'store.cat.linters', api: 'Linters' },
  { id: 'other', label: 'store.cat.other', api: 'Other' },
];

function buildStore() {
  if (!storeEl) return;

  const search = h('input', {
    class: 'mono-input store-search',
    placeholder: t('store.searchHint'),
    value: store.query,
    onKeydown: (e: KeyboardEvent) => {
      if (e.key === 'Enter') {
        store.query = search.value;
        void loadStore(true);
      }
    },
  }) as HTMLInputElement;
  search.addEventListener('input', () => {
    store.query = search.value;
  });

  const sortSel = h('select', { class: 'store-sort' },
    h('option', { value: 'downloads' }, t('store.sort.downloads')),
    h('option', { value: 'rating' }, t('store.sort.rating')),
    h('option', { value: 'recent' }, t('store.sort.recent')),
  ) as HTMLSelectElement;
  sortSel.value = store.sort;
  sortSel.addEventListener('change', () => {
    store.sort = sortSel.value as Sort;
    void loadStore(true);
  });

  const cats = h('div', { class: 'store-cats' },
    ...CATEGORIES.map((c) =>
      h('button', {
        class: `store-cat${store.category === c.id ? ' on' : ''}`,
        dataset: { cat: c.id },
        onClick: (e: Event) => {
          store.category = c.id;
          storeEl!.querySelectorAll('.store-cat').forEach((n) =>
            n.classList.toggle('on', (n as HTMLElement).dataset.cat === c.id));
          void loadStore(true);
          void e;
        },
      }, t(c.label)),
    ),
  );

  const list = h('div', { class: 'store-list', dataset: { role: 'store-list' } });
  const detail = h('div', { class: 'store-detail', dataset: { role: 'store-detail' } });
  detail.append(h('div', { class: 'store-empty' }, t('store.pick')));

  storeEl.append(
    h('div', { class: 'store-bar' },
      search,
      h('button', { class: 'btn', onClick: () => void loadStore(true) }, t('ext.search')),
      sortSel,
    ),
    cats,
    h('div', { class: 'store-main' }, list, detail),
  );
  void loadStore(true);
}

/**
 * Only the newest search may write to the list. Opening the tab fires a default
 * query, and a user who types immediately fires a second one; the slower
 * default request used to land last and replace the search results.
 */
let storeSeq = 0;
/** Same idea for the detail pane: rapid row clicks must not cross-render. */
let detailSeq = 0;

async function loadStore(reset = false) {
  if (!storeEl) return;
  if (!storeEl.childElementCount) {
    buildStore();
    return;
  }
  const list = storeEl.querySelector('[data-role="store-list"]') as HTMLElement | null;
  if (!list) return;
  const seq = ++storeSeq;
  if (reset) store.rows = [];
  const query = store.query;
  const sort = store.sort;
  const offset = store.rows.length;
  const cat = CATEGORIES.find((c) => c.id === store.category)?.api || '';
  store.loading = true;
  store.error = '';
  clear(list);
  list.append(h('div', { class: 'store-loading' }, t('common.loading')));

  try {
    const rows = await api.extensionsSearch(query, cat, sort, offset);
    if (seq !== storeSeq) return;
    store.rows = reset ? rows : [...store.rows, ...rows];
    renderStoreList(list);
  } catch (e) {
    if (seq !== storeSeq) return;
    clear(list);
    store.error = errText(e);
    list.append(errorBox(t('store.searchFailed'), store.error, () => void loadStore(true)));
  } finally {
    if (seq === storeSeq) store.loading = false;
  }
}

function renderStoreList(list: HTMLElement) {
  clear(list);
  if (store.rows.length === 0) {
    list.append(h('div', { class: 'store-loading' }, t('store.empty')));
    return;
  }
  for (const row of store.rows) list.append(storeRow(row));
  list.append(
    h('div', { class: 'store-more' },
      h('button', {
        class: 'btn small',
        onClick: () => void loadStore(false),
      }, t('store.more')),
    ),
  );
}

/** Keep the search result in sync after an install, whichever button did it. */
function markInstalled(id: string, version: string) {
  const row = store.rows.find((r) => r.id === id);
  if (!row) return;
  row.installed_version = version;
  const list = storeEl?.querySelector('[data-role="store-list"]') as HTMLElement | null;
  if (list) renderStoreList(list);
}

function storeRow(row: ExtSearchRow) {
  const installed = !!row.installed_version;
  return h('div', {
    class: `store-row${store.selected === row.id ? ' on' : ''}`,
    dataset: { id: row.id },
    onClick: () => void openDetail(row.id),
  },
    tile(row.display_name || row.id, row.icon),
    h('div', { class: 'store-row-main' },
      h('div', { class: 'store-row-head' },
        h('span', { class: 'store-row-name' }, row.display_name || row.id),
        row.version ? h('span', { class: 'store-row-ver' }, `v${row.version}`) : null,
      ),
      h('div', { class: 'store-row-id' }, row.id),
      row.description ? h('div', { class: 'store-row-desc' }, row.description) : null,
      h('div', { class: 'store-row-meta' },
        h('span', null, `${fmtNum(row.downloads)} ↓`),
        row.rating > 0 ? h('span', null, `★ ${row.rating.toFixed(1)}`) : null,
        installed ? h('span', { class: 'store-installed' }, t('store.installedTag', { v: row.installed_version })) : null,
      ),
    ),
    h('button', {
      class: `btn small${installed ? '' : ' primary'}`,
      onClick: (e: Event) => {
        e.stopPropagation();
        if (installed) {
          void openDetail(row.id);
        } else {
          void installRow(row, e.currentTarget as HTMLButtonElement);
        }
      },
    }, installed ? t('store.openDetail') : t('ext.install')),
  );
}

async function installRow(row: ExtSearchRow, btn: HTMLButtonElement) {
  btn.disabled = true;
  btn.textContent = t('ext.installing');
  try {
    await api.extensionsInstall(row.id);
    toast(`${t('ext.installed')} ${row.display_name || row.id}`, 'ok', 1800);
    await refreshExtensions();
    row.installed_version = row.version;
    renderStoreList(storeEl!.querySelector('[data-role="store-list"]') as HTMLElement);
    if (store.selected === row.id) void openDetail(row.id, true);
  } catch (e) {
    toast(errText(e), 'err', 4200);
  } finally {
    btn.disabled = false;
    btn.textContent = t('ext.install');
  }
}

function tile(name: string, iconUrl: string): HTMLElement {
  const wrap = h('div', { class: 'store-tile' }, (name || '?').trim().charAt(0).toUpperCase());
  if (iconUrl) {
    const img = h('img', { src: iconUrl, alt: '', loading: 'lazy' }) as HTMLImageElement;
    img.addEventListener('error', () => img.remove());
    img.addEventListener('load', () => { wrap.textContent = ''; wrap.append(img); });
  }
  return wrap;
}

async function openDetail(id: string, force = false) {
  if (!storeEl) return;
  const detail = storeEl.querySelector('[data-role="store-detail"]') as HTMLElement | null;
  if (!detail) return;
  if (!force && store.selected === id && store.detail) return;
  store.selected = id;
  storeEl.querySelectorAll('.store-row').forEach((n) =>
    n.classList.toggle('on', (n as HTMLElement).dataset.id === id));
  clear(detail);
  detail.append(h('div', { class: 'store-loading' }, t('common.loading')));
  const seq = ++detailSeq;
  try {
    const d = await api.extensionsDetail(id);
    if (seq !== detailSeq) return;
    store.detail = d;
    renderDetail(detail, d);
  } catch (e) {
    if (seq !== detailSeq) return;
    clear(detail);
    detail.append(errorBox(t('store.detailFailed'), errText(e), () => void openDetail(id, true)));
  }
}

function renderDetail(box: HTMLElement, d: ExtDetail) {
  clear(box);
  const installed = !!d.installed_version;

  const verSel = h('select', { class: 'store-ver' },
    ...d.versions.slice(0, 40).map((v) => h('option', { value: v }, `v${v}`)),
  ) as HTMLSelectElement;
  if (d.versions.length && d.versions[0]) verSel.value = d.version || d.versions[0];

  const installBtn = h('button', { class: 'btn primary' },
    icon('download', 12), installed ? t('store.reinstall') : t('ext.install')) as HTMLButtonElement;
  installBtn.onclick = async () => {
    installBtn.disabled = true;
    installBtn.textContent = t('ext.installing');
    const spec = verSel.value && verSel.value !== d.version ? `${d.id}@${verSel.value}` : d.id;
    try {
      await api.extensionsInstall(spec);
      toast(`${t('ext.installed')} ${d.display_name || d.id}`, 'ok', 1800);
      // Installing from the detail pane must also flip the row's tag; otherwise
      // the left list keeps saying "安装" for something already installed.
      markInstalled(d.id, d.version);
      await refreshExtensions();
      void openDetail(d.id, true);
    } catch (e) {
      toast(errText(e), 'err', 4200);
    } finally {
      installBtn.disabled = false;
      installBtn.textContent = installed ? t('store.reinstall') : t('ext.install');
    }
  };

  const meta: [string, string][] = [
    [t('store.detail.version'), d.version || '—'],
    [t('store.detail.downloads'), fmtNum(d.downloads)],
    [t('store.detail.rating'), d.rating > 0 ? `★ ${d.rating.toFixed(1)} (${d.reviews})` : '—'],
    [t('store.detail.license'), d.license || '—'],
  ];

  box.append(
    h('div', { class: 'store-detail-head' },
      tile(d.display_name || d.id, ''),
      h('div', { class: 'store-detail-title' },
        h('div', { class: 'row', style: { gap: '6px', alignItems: 'baseline' } },
          h('b', null, d.display_name || d.id),
          h('span', { class: 'ext-ver' }, `v${d.version}`),
          installed ? h('span', { class: 'store-installed' }, t('store.installedTag', { v: d.installed_version })) : null,
          d.node ? h('span', { class: 'ext-pill vscode' }, 'JS') : null,
        ),
        h('div', { class: 'store-row-id' }, d.id),
        d.description ? h('div', { class: 'store-row-desc' }, d.description) : null,
      ),
    ),
    h('div', { class: 'store-detail-actions' },
      verSel,
      installBtn,
      d.repository || d.homepage
        ? h('button', {
            class: 'btn small',
            onClick: () => void api.openExternal(d.repository || d.homepage).catch((e) => toast(errText(e), 'err')),
          }, icon('external', 11), t('store.detail.browser'))
        : null,
    ),
    h('div', { class: 'store-meta' },
      ...meta.map(([k, v]) => h('div', { class: 'store-meta-cell' },
        h('span', { class: 'k' }, k), h('span', { class: 'v' }, v)))),
  );

  // `append` rejects null, so conditionals are filtered before they go in.
  const tail: (Node | null)[] = [
    d.categories.length
      ? h('div', { class: 'ext-chips' }, ...d.categories.map((c) => h('span', { class: 'ext-chip' }, c)))
      : null,
    h('div', { class: 'store-readme' }, renderReadme(d.readme)),
  ];
  box.append(...tail.filter((n): n is Node => n !== null));
}

/** Strip constructs the tiny renderer cannot honour (images, badges, HTML). */
function renderReadme(md: string): DocumentFragment | HTMLElement {
  if (!md.trim()) return h('div', { class: 'hint' }, t('store.detail.noReadme'));
  const clean = md
    .replace(/<img[^>]*>/gi, '')
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/<[^>]+>/g, '');
  return renderMarkdown(clean);
}

/* ------------------------------------------------------------------ refresh */

/** The card whose output pane is open, so live log lines land in it. */
let openCardId = '';

on('exthost:status', () => {
  if (page === 'installed') renderList(app.extensions);
});

// A host announces its commands a few frames after it reports `running`; without
// this an already-open pane would keep showing "no commands yet".
on('exthost:commands', () => {
  if (page !== 'installed') return;
  if (openCardId && listEl?.querySelector(`.ext-card[data-ext="${openCardId}"]`)) {
    redrawOpen();
    return;
  }
  renderList(app.extensions);
});

on('exthost:log', () => {
  // Appending beats re-fetching: a chatty extension can emit hundreds of lines a
  // second, and re-reading the whole ring for each one would thrash the DOM.
  if (!openCardId || page !== 'installed' || !listEl) return;
  const box = listEl.querySelector<HTMLElement>(`.ext-card[data-ext="${openCardId}"] .ext-log`);
  if (!box) return;
  const last = app.extLogs[app.extLogs.length - 1];
  if (!last || last.id !== openCardId) return;
  if (box.querySelector('.hint')) clear(box);
  box.append(h('div', { class: `ext-log-line ${last.level}` }, last.text));
  const lines = box.querySelectorAll('.ext-log-line');
  if (lines.length > 400) lines[0].remove();
  box.scrollTop = box.scrollHeight;
});

/** Exposed so the settings page can jump straight into the store. */
export function showStore() {
  setPage('store');
  if (!store.rows.length) void loadStore(true);
}

export function showInstalled() {
  setPage('installed');
}

/** Used by the command palette. */
export function openExtension(id: string) {
  setPage('installed');
  void refreshExtensions();
  setTimeout(() => {
    const row = [...(listEl?.querySelectorAll('.ext-card') || [])].find((n) =>
      (n.textContent || '').includes(id));
    row?.scrollIntoView({ block: 'center' });
  }, 260);
}


/* ------------------------------------------------- installed detail modal */

let inspectOverlay: HTMLElement | null = null;
let inspectKey: ((e: KeyboardEvent) => void) | null = null;

function closeInstalledDetail() {
  if (inspectOverlay) inspectOverlay.remove();
  inspectOverlay = null;
  if (inspectKey) document.removeEventListener('keydown', inspectKey);
  inspectKey = null;
}

/**
 * Full detail for a *locally installed* extension. This is deliberately a
 * modal rather than an expanded card: the extension list is rebuilt on every
 * host event, and a long README inside a card would fight the scroll position.
 */
function openInstalledDetail(id: string) {
  closeInstalledDetail();
  const body = h('div', { class: 'extinfo-body' },
    h('div', { class: 'hint' }, t('common.loading')));
  const overlay = h('div', {
    class: 'modal',
    onClick: (e: MouseEvent) => { if (e.target === overlay) closeInstalledDetail(); },
  }) as HTMLElement;
  const card = h('div', { class: 'modal-card extinfo-card' },
    h('div', { class: 'modal-head' },
      h('span', { class: 'modal-title' }, t('ext.detail.title')),
      h('button', {
        class: 'icon-btn',
        style: { marginLeft: 'auto' },
        title: t('common.close'),
        onClick: () => closeInstalledDetail(),
      }, icon('close', 14)),
    ),
    body,
  );
  overlay.append(card);
  document.body.append(overlay);
  inspectOverlay = overlay;
  inspectKey = (e: KeyboardEvent) => { if (e.key === 'Escape') closeInstalledDetail(); };
  document.addEventListener('keydown', inspectKey);

  void api.extensionsInspect(id).then((d) => {
    if (inspectOverlay !== overlay) return;
    clear(body);
    body.append(renderInstalledDetail(d));
  }).catch((e) => {
    if (inspectOverlay !== overlay) return;
    clear(body);
    body.append(errorBox(t('common.error'), errText(e), () => void openInstalledDetail(id)));
  });
}

function sourceLabel(src: string): string {
  if (src === 'git') return 'GitHub';
  if (src === 'vsix') return 'VSIX';
  if (src === 'online') return 'Open VSX';
  return src || '—';
}

function kvCell(key: string, value: string): HTMLElement {
  return h('div', { class: 'extinfo-cell' },
    h('span', { class: 'k' }, key),
    h('span', { class: 'v' }, value || '—'));
}

function detailSection(title: string, child: HTMLElement | DocumentFragment): HTMLElement {
  return h('section', { class: 'extinfo-section' },
    h('h4', null, title), child);
}

function renderInstalledDetail(d: ExtInspect): HTMLElement {
  const info = d.info;
  const c = info.contributes;
  const title = info.display_name || info.name || info.id;

  const toggleBtn = h('button', {
    class: 'btn',
    onClick: async () => { await toggle(info); void openInstalledDetail(info.id); },
  }, info.enabled ? t('ext.disable') : t('ext.enable'));

  const up = updates.find((u) => u.id.toLowerCase() === info.id.toLowerCase());
  const updateBtn = up
    ? h('button', {
        class: 'btn primary',
        onClick: async () => { await updateExt(info, up.latest); void openInstalledDetail(info.id); },
      }, icon('download', 12), `${t('ext.update')} v${up.latest}`)
    : null;

  const link = d.repository || d.homepage;
  const linkBtn = link
    ? h('button', {
        class: 'btn',
        onClick: () => void api.openExternal(link).catch((e) => toast(errText(e), 'err')),
      }, icon('external', 12), t('store.detail.browser'))
    : null;

  const removeBtn = h('button', {
    class: 'btn danger',
    onClick: async () => {
      await remove(info);
      if (!app.extensions.some((e) => e.id === info.id)) closeInstalledDetail();
      else void openInstalledDetail(info.id);
    },
  }, t('ext.uninstall'));

  const installedAt = info.installed_at
    ? new Date(info.installed_at).toLocaleString()
    : '—';

  const meta = h('div', { class: 'extinfo-meta' },
    kvCell(t('ext.detail.identifier'), info.id),
    kvCell(t('ext.detail.publisher'), info.publisher),
    kvCell(t('settings.version'), info.version ? `v${info.version}` : ''),
    kvCell(t('ext.detail.source'), sourceLabel(info.source)),
    kvCell(t('ext.detail.installedAt'), installedAt),
    kvCell(t('ext.detail.size'), `${fmtBytes(d.size_bytes)} · ${d.file_count}`),
    kvCell(t('ext.detail.location'), info.dir),
    kvCell(t('ext.detail.entry'), info.entry),
    kvCell(t('ext.detail.engine'), d.engines),
    kvCell(t('settings.license'), d.license),
  );

  const groups: { label: string; rows: string[] }[] = [
    { label: t('ext.themes'), rows: c.themes.map((x) => x.label || x.id) },
    { label: t('ext.snippets'), rows: c.snippets.map((x) => x.language || x.path) },
    { label: t('ext.languages'), rows: c.languages.map((x) => x.id) },
    { label: t('ext.commands'), rows: c.commands.map((x) => x.title || x.command) },
    { label: t('ext.skills'), rows: c.skills },
  ];
  const contrib = groups.filter((g) => g.rows.length);
  const contribBody = contrib.length
    ? h('div', { class: 'extinfo-groups' },
        ...contrib.map((g) => h('div', { class: 'extinfo-group' },
          h('div', { class: 'extinfo-group-title' }, `${g.label} · ${g.rows.length}`),
          h('div', { class: 'extinfo-chips' },
            ...g.rows.slice(0, 60).map((r) => h('span', { class: 'ext-chip', title: r }, r))),
          g.rows.length > 60 ? h('span', { class: 'hint' }, `+${g.rows.length - 60}`) : null,
        )))
    : h('div', { class: 'hint' }, t('ext.detail.noContrib'));

  const readmeBody = d.readme.trim()
    ? h('div', { class: 'store-readme' }, renderReadme(d.readme))
    : h('div', { class: 'hint' }, t('ext.detail.noReadme'));

  const tail: (HTMLElement | null)[] = [
    info.broken
      ? h('div', { class: 'extinfo-warn' }, icon('alert', 13), t('ext.brokenHint'))
      : null,
    d.categories.length || d.keywords.length
      ? h('div', { class: 'extinfo-chips' },
          ...[...d.categories, ...d.keywords].map((x) => h('span', { class: 'ext-chip' }, x)))
      : null,
    detailSection(t('ext.detail.contributions'), contribBody),
    d.api_gaps?.length
      ? detailSection(t('ext.detail.apiUsed'),
          h('div', null,
            h('div', { class: 'extinfo-chips' },
              ...d.api_gaps.map((g) =>
                h('span', {
                  class: `ext-chip mono${g.supported ? '' : ' api-gap'}`,
                  title: g.supported ? undefined : t('ext.detail.apiGapHint'),
                }, g.name))),
            d.api_gaps.some((g) => !g.supported)
              ? h('div', { class: 'hint', style: { marginTop: '6px' } }, t('ext.detail.apiGapHint'))
              : null))
      : null,
    info.activation.length
      ? detailSection(t('ext.detail.activation'),
          h('div', { class: 'ext-chips' },
            ...info.activation.map((a) => h('span', { class: 'ext-chip mono' }, a))))
      : null,
    detailSection(t('ext.detail.readme'), readmeBody),
    d.changelog.trim()
      ? detailSection(t('ext.detail.changelog'), h('pre', { class: 'extinfo-pre' }, d.changelog))
      : null,
    d.license_text.trim()
      ? detailSection(t('settings.license'), h('pre', { class: 'extinfo-pre' }, d.license_text))
      : null,
  ];

  return h('div', { class: 'extinfo' },
    h('div', { class: 'extinfo-head' },
      extIcon(info),
      h('div', { class: 'extinfo-title' },
        h('div', { class: 'row', style: { gap: '8px', alignItems: 'baseline', flexWrap: 'wrap' } },
          h('b', null, title),
          h('span', { class: 'ext-ver' }, `v${info.version || '—'}`),
          h('span', { class: `ext-pill ${info.kind}` },
            info.kind === 'vscode' ? t('ext.kindVscode') : t('ext.kindDsh')),
          h('span', { class: `extinfo-state ${info.enabled ? 'on' : 'off'}` },
            info.enabled ? t('ext.detail.enabled') : t('ext.detail.disabled')),
        ),
        h('div', { class: 'ext-id' }, `${info.id} · ${info.publisher}`),
        info.description ? h('div', { class: 'ext-desc' }, info.description) : null,
      ),
    ),
    h('div', { class: 'extinfo-actions' },
      updateBtn, toggleBtn, linkBtn,
      h('div', { class: 'spacer', style: { flex: '1' } }),
      removeBtn,
    ),
    meta,
    ...tail.filter((n): n is HTMLElement => n !== null),
  );
}
