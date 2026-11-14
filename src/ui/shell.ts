// Application shell: titlebar, sidebar router, stage (dock + file view), rail, statusbar.
//
// Layout contract
//   .body = [ sidebar ][ resizer ][ stage ][ rail ]
//   .stage = dock (chat / review / bridge) with the editor stacked on top of it.
// Opening a file *covers* the dock instead of stealing a third column; the dock
// (and any stream running inside it) stays alive underneath, so switching back
// to chat is instant and nothing is lost mid-answer.

import { h, clear, fmtNum, svg } from '../core/dom';
import { icon, mark, type IconName } from './icons';
import { t } from '../core/i18n';
import { api, errText } from '../core/api';
import { app, emit, on, type Surface, type ViewId } from '../core/state';
import { toast } from './toast';
import { openSettings } from './settings';
import { applyTheme } from './theme';
import { mountEditor, saveActive } from './editor';
import { mountExplorer, render as renderExplorer, pickFolder, explorerActions } from './explorer';
import { mountSearch } from './search';
import { mountGit, refreshGit } from './git';
import { mountSkills, loadSkills } from './skills';
import { mountStats, refreshStats } from './stats';
import { mountChat, refreshComposer, renderHead, reconcileProviders } from './chat';
import { mountReview, refreshReview, reviewCount } from './review';
import { mountBridge, refreshBridge } from './bridge';
import { mountExtensions, refreshExtensions } from './extensions';
import { mountExtGlue } from './extglue';
import { mountWebviews } from './webview';
import {
  extContainers, extContainerIcon, extContainerTitle,
  mountExtViews, refreshExtViews, unmountExtViews,
} from './extviews';
import { mountTerminal, clearTerminal, terminalFocus, newTerminal } from './terminal';
import { openPalette } from './palette';

let root: HTMLElement | null = null;
let bodyEl: HTMLElement | null = null;
let sideBody: HTMLElement | null = null;
let sideHead: HTMLElement | null = null;
let sideTitle: HTMLElement | null = null;
let activityEl: HTMLElement | null = null;
let sideTools: HTMLElement | null = null;
let stageEl: HTMLElement | null = null;
let surfaceEl: HTMLElement | null = null;
let editorArea: HTMLElement | null = null;
let panelEl: HTMLElement | null = null;
let railEl: HTMLElement | null = null;
let statusTextEl: HTMLElement | null = null;
let statusDot: HTMLElement | null = null;
let statusBranch: HTMLElement | null = null;
let statusModel: HTMLElement | null = null;
let statusTokens: HTMLElement | null = null;
let termBody: HTMLElement | null = null;
let extStatusEl: HTMLElement | null = null;
let mounted: ViewId | string = '';
/** Non-null while an extension-contributed container owns the sidebar. */
let activeExtView: string | null = null;

const VIEW_META: Record<ViewId, { icon: IconName; label: string; short: string }> = {
  explorer: { icon: 'files', label: 'nav.explorer', short: 'act.explorer' },
  search: { icon: 'search', label: 'nav.search', short: 'act.search' },
  git: { icon: 'git', label: 'nav.git', short: 'act.git' },
  skills: { icon: 'skills', label: 'nav.skills', short: 'act.skills' },
  stats: { icon: 'stats', label: 'nav.stats', short: 'act.stats' },
};

export function mountShell(host: HTMLElement) {
  root = h('div', { class: 'app' });
  root.style.setProperty('--sidebar-w', `${app.sidebarWidth}px`);

  const titlebar = buildTitlebar();
  bodyEl = h('div', { class: 'body' });
  const sidebar = buildSidebar();
  const resizerA = h('div', { class: 'resizer-v' });

  surfaceEl = h('div', { class: 'surface' });
  const dock = h('div', { class: 'dock', dataset: { role: 'dock' } });
  editorArea = h('div', { class: 'editor-area', hidden: true });
  surfaceEl.append(dock, editorArea);

  stageEl = h('div', { class: 'stage' }, surfaceEl);

  bodyEl.append(buildActivity(), sidebar, resizerA, stageEl, buildRail());
  const statusbar = buildStatusbar();
  root.append(titlebar, bodyEl, statusbar);
  host.append(root);

  mountChat(dock);
  mountReview(dock);
  mountBridge(dock);
  mountExtensions(dock);
  mountEditor(editorArea);
  mountExtGlue();
  mountWebviews();
  bindResizer(resizerA);
  // Manifests are scanned after the first paint, and providers register even
  // later. Both change what the activity column has to show.
  on('extview:changed', () => paintActivity());
  on('extensions:count', () => paintActivity());

  selectView(app.view);
  setSurface(app.surface);
  renderStatus();
  renderExtStatus();
  bindGlobal();

  return root;
}

/* --------------------------------------------------------------- titlebar */

function buildTitlebar(): HTMLElement {
  const drag = { 'data-tauri-drag-region': 'true' } as any;

  const brand = h(
    'div',
    { class: 'tb-brand', ...drag },
    h('span', { class: 'tb-logo' }, mark(18)),
    h('span', { class: 'tb-title' }, 'IRIXEN Code'),
  );

  const ws = h('div', { class: 'tb-ws', ...drag, title: app.config?.workspace || '' },
    app.config?.workspace || t('explorer.empty'));
  ws.addEventListener('click', () => void pickFolder());

  const openBtn = h('button', { class: 'tb-btn', title: t('explorer.openFolder'), onClick: () => void pickFolder() },
    icon('folderOpen', 14), t('explorer.openFolder'));

  const themeBtn = h('button', { class: 'tb-btn', title: t('settings.theme'), onClick: () => toggleTheme() },
    icon(app.config?.theme === 'light' ? 'moon' : 'sun', 14));

  const panelBtn = h('button', { class: 'tb-btn', title: t('panel.terminal'), onClick: () => togglePanel() },
    icon('panel', 14));

  const chatBtn = h('button', { class: 'tb-btn', title: t('chat.title'), onClick: () => toggleChat() },
    icon('chat', 14));

  const setBtn = h('button', { class: 'tb-btn', title: t('nav.settings'), onClick: () => openSettings('providers') },
    icon('settings', 14));

  const actions = h('div', { class: 'tb-actions' }, openBtn, themeBtn, panelBtn, chatBtn, setBtn);

  const controls = h('div', { class: 'win-controls' },
    winBtn('minimize', () => void api.windowControl('minimize')),
    winBtn('maximize', () => void api.windowControl('maximize')),
    winBtn('close', () => void api.windowControl('close')),
  );

  const bar = h('div', { class: 'titlebar', ...drag }, brand, ws, actions, controls);
  for (const el of [openBtn, themeBtn, panelBtn, chatBtn, setBtn, controls]) {
    el.addEventListener('pointerdown', (e) => e.stopPropagation());
  }
  themeBtn.dataset.role = 'theme';
  chatBtn.dataset.role = 'chat';
  return bar;
}

function winBtn(kind: 'minimize' | 'maximize' | 'close', onClick: () => void): HTMLElement {
  const paths = {
    minimize: '<path d="M5 12h14"/>',
    maximize: '<rect x="6" y="6" width="12" height="12" rx="1.5"/>',
    close: '<path d="M6 6l12 12M18 6L6 18"/>',
  };
  // Window controls are icon-only; `h()` mirrors this title into aria-label so
  // the button is announced instead of showing up as a nameless control.
  return h('button', { class: `win-btn ${kind}`, title: t(`window.${kind}`), onClick }, svg(paths[kind], 13));
}

function toggleTheme() {
  if (!app.config) return;
  const next = app.config.theme === 'light' ? 'dark' : 'light';
  app.config.theme = next;
  applyTheme(app.config);
  void api.saveConfig(app.config).then((c) => {
    app.config = c;
    applyTheme(c);
  });
  const btn = root?.querySelector('[data-role="theme"]') as HTMLElement | null;
  if (btn) {
    clear(btn);
    btn.append(icon(next === 'light' ? 'moon' : 'sun', 14));
  }
}

/* ------------------------------------------------------------- activity bar */

/**
 * Left view switcher. Fixed position, fixed order, icon + short label — this
 * column is not a "VS Code cosplay", it is where these five views have always
 * been and where the eye goes looking for them.
 */
function buildActivity(): HTMLElement {
  activityEl = h('div', { class: 'activity' });
  paintActivity();
  return activityEl;
}

function paintActivity() {
  if (!activityEl) return;
  clear(activityEl);

  for (const v of Object.keys(VIEW_META) as ViewId[]) {
    const meta = VIEW_META[v];
    activityEl.append(
      h('button', {
        class: `act-btn${!activeExtView && app.view === v ? ' on' : ''}`,
        title: t(meta.label),
        'aria-label': t(meta.label),
        dataset: { view: v },
        onClick: () => selectView(v),
      }, icon(meta.icon, 17), h('span', null, t(meta.short))),
    );
  }

  // `contributes.viewsContainers.activitybar` adds a second group. The
  // container must be visible even before its host registers a provider —
  // the section then says "waiting" instead of the whole button being absent.
  const containers = extContainers();
  if (containers.length) {
    activityEl.append(h('div', { class: 'act-sep' }));
    for (const c of containers) {
      const url = extContainerIcon(c.id);
      const glyph = url
        ? h('img', { class: 'act-ext-icon', src: url, alt: '' })
        : icon('puzzle', 17);
      activityEl.append(
        h('button', {
          class: `act-btn act-ext${activeExtView === c.id ? ' on' : ''}`,
          title: c.title,
          'aria-label': c.title,
          dataset: { view: `ext:${c.id}` },
          onClick: () => selectView(`ext:${c.id}`),
        }, glyph, h('span', null, c.title)),
      );
    }
  }

  activityEl.append(
    h('div', { class: 'act-sep' }),
    h('button', {
      class: `act-btn${app.surface === 'review' ? ' on' : ''}`,
      title: t('rail.review'),
      'aria-label': t('rail.review'),
      dataset: { surface: 'review' },
      onClick: () => setSurface('review'),
    },
      icon('review', 17),
      h('span', null, t('rail.review')),
      reviewCount() ? h('span', { class: 'act-badge' }, reviewCount() > 99 ? '99+' : `${reviewCount()}`) : null,
    ),
  );
}

/* ---------------------------------------------------------------- sidebar */

function buildSidebar(): HTMLElement {
  sideHead = h('div', { class: 'side-head' });
  sideBody = h('div', { class: 'side-body' });
  sideTitle = h('div', { class: 'side-title' });
  sideTools = h('div', { class: 'side-tools row', style: { marginLeft: 'auto' } });
  sideHead.append(sideTitle, sideTools);
  return h('div', { class: 'sidebar' }, sideHead, sideBody);
}

/**
 * Sidebar header only: the view name plus its toolbar. Split out of `selectView`
 * so a language change can relabel the chrome without tearing down the mounted
 * view body (and whatever the user was typing into it).
 */
function paintSideHead(view: ViewId | string) {
  if (!sideTitle || !sideTools) return;
  if (typeof view === 'string' && view.startsWith('ext:')) {
    const id = view.slice(4);
    sideTitle.textContent = extContainerTitle(id);
    clear(sideTools);
    sideTools.append(
      h('button', {
        class: 'icon-btn',
        title: t('common.refresh'),
        onClick: () => void refreshExtViews(id),
      }, icon('refresh', 13)),
    );
    return;
  }
  const builtin = view as ViewId;
  if (!VIEW_META[builtin]) return;
  sideTitle.textContent = t(VIEW_META[builtin].label);
  clear(sideTools);

  // Explorer gets VS Code-style header shortcuts (new file / new folder /
  // collapse) next to the refresh button; other views only need refresh.
  if (view === 'explorer') {
    for (const btn of explorerActions()) sideTools.append(btn);
  }

  sideTools.append(
    h('button', {
      class: 'icon-btn',
      title: t('common.refresh'),
      onClick: () => {
        if (view === 'explorer') void renderExplorer();
        else if (view === 'git') void refreshGit();
        else if (view === 'skills') void loadSkills();
        else if (view === 'stats') void refreshStats();
        else selectView(view);
      },
    }, icon('refresh', 13)),
  );
}

export function selectView(view: ViewId | string) {
  if (typeof view === 'string' && view.startsWith('ext:')) {
    activeExtView = view.slice(4);
    paintActivity();
    paintSideHead(view);
    if (!sideBody) return;
    clear(sideBody);
    unmountExtViews();
    mounted = view;
    mountExtViews(sideBody, activeExtView);
    return;
  }
  activeExtView = null;
  unmountExtViews();
  app.view = view as ViewId;
  paintActivity();
  paintSideHead(view);
  if (!sideBody) return;
  clear(sideBody);
  mounted = view;
  if (view === 'explorer') mountExplorer(sideBody);
  else if (view === 'search') mountSearch(sideBody);
  else if (view === 'git') { mountGit(sideBody); void refreshGit(); }
  else if (view === 'skills') mountSkills(sideBody);
  else mountStats(sideBody);
}

/* ------------------------------------------------------------------- rail */

function buildRail(): HTMLElement {
  railEl = h('div', { class: 'rail' });
  paintRail();
  return railEl;
}

function paintRail() {
  if (!railEl) return;
  clear(railEl);

  const dockBtn = (surface: Surface, iconName: IconName, label: string, badge?: number) =>
    h('button', {
      class: `rail-btn${app.surface === surface ? ' on' : ''}`,
      title: label,
      'aria-label': label,
      dataset: { surface },
      onClick: () => setSurface(surface),
    },
    icon(iconName, 17),
    badge ? h('span', { class: 'rail-badge' }, badge > 99 ? '99+' : `${badge}`) : null,
  );

  const hasFiles = app.files.length > 0;

  railEl.append(
    dockBtn('chat', 'chat', t('rail.chat')),
    dockBtn('bridge', 'plug', t('rail.bridge')),
    dockBtn('extensions', 'puzzle', t('rail.extensions'), app.hosts.filter((h) => h.state === 'running').length),
  );

  if (hasFiles) {
    railEl.append(
      h('div', { class: 'rail-sep' }),
      dockBtn('editor', 'file', t('rail.file')),
    );
  }

  railEl.append(
    h('div', { class: 'rail-spacer' }),
    h('button', {
      class: 'rail-btn',
      title: t('panel.terminal'),
      'aria-label': t('panel.terminal'),
      onClick: () => togglePanel(),
    }, icon('terminal', 17)),
    h('button', {
      class: 'rail-btn',
      title: t('nav.settings'),
      'aria-label': t('nav.settings'),
      onClick: () => openSettings('providers'),
    }, icon('settings', 17)),
  );
}

/** Show a dock panel, or the file view that covers it. */
export function setSurface(surface: Surface) {
  if (surface === 'editor' && app.files.length === 0) surface = 'chat';
  app.surface = surface;

  const dock = surfaceEl?.querySelector('.dock') as HTMLElement | null;
  if (dock) {
    dock.hidden = surface === 'editor';
    if (surface !== 'editor') dock.dataset.active = surface;
  }
  if (editorArea) editorArea.hidden = surface !== 'editor';

  railEl?.querySelectorAll('.rail-btn').forEach((n) => {
    n.classList.toggle('on', (n as HTMLElement).dataset.surface === surface);
  });
  paintActivity();

  if (surface === 'chat') refreshComposer();
  else if (surface === 'review') void refreshReview();
  else if (surface === 'extensions') void refreshExtensions();
  else if (surface === 'bridge') void refreshBridge();
  emit('surface', surface);
}

export function currentSurface(): Surface {
  return app.surface;
}

/* ------------------------------------------------------------------ panel */

export function togglePanel(force?: boolean) {
  app.panelOpen = force === undefined ? !app.panelOpen : force;
  renderPanel();
}

function renderPanel() {
  if (!stageEl) return;
  stageEl.classList.toggle('panel-open', app.panelOpen);
  if (!app.panelOpen) {
    panelEl?.remove();
    panelEl = null;
    termBody = null;
    return;
  }
  if (panelEl) {
    drawPanelBody();
    return;
  }

  const head = h('div', { class: 'panel-head' });
  const tabs: { id: 'terminal' | 'output' | 'usage'; label: string }[] = [
    { id: 'terminal', label: t('panel.terminal') },
    { id: 'output', label: t('panel.output') },
    { id: 'usage', label: t('panel.usage') },
  ];
  for (const tb of tabs) {
    head.append(h('div', {
      class: `panel-tab${app.panelTab === tb.id ? ' active' : ''}`,
      onClick: () => {
        app.panelTab = tb.id;
        drawPanelHead(head);
        drawPanelBody();
      },
    }, tb.label));
  }
  drawPanelHead(head);

  // The terminal owns its own chrome (tabs, new/restart buttons) and takes the
  // whole body: xterm needs a sized element, not a padded text box.
  termBody = h('div', { class: 'panel-body', style: { flex: '1', minHeight: '0' } });

  panelEl = h('div', { class: 'panel' }, head, termBody);
  stageEl.append(panelEl);
  drawPanelBody();
}

function drawPanelHead(head: HTMLElement) {
  head.querySelectorAll('.panel-tab').forEach((n) => {
    const label = n.textContent || '';
    if (label === t('panel.terminal')) n.classList.toggle('active', app.panelTab === 'terminal');
    if (label === t('panel.output')) n.classList.toggle('active', app.panelTab === 'output');
    if (label === t('panel.usage')) n.classList.toggle('active', app.panelTab === 'usage');
  });
  if (!head.querySelector('[data-role="panel-close"]')) {
    head.append(
      h('div', { class: 'spacer', style: { flex: '1' } }),
      h('button', {
        class: 'icon-btn',
        title: t('panel.clear'),
        onClick: () => {
          // Clearing the terminal clears the scrollback of the live shell;
          // clearing output drops the run log. Two different things.
          if (app.panelTab === 'terminal') clearTerminal();
          else {
            app.terminal = [];
            drawPanelBody();
          }
        },
      }, icon('trash', 12)),
      h('button', {
        class: 'icon-btn',
        title: t('common.close'),
        dataset: { role: 'panel-close' },
        onClick: () => togglePanel(false),
      }, icon('close', 12)),
    );
  }
}

function drawPanelBody() {
  if (!termBody) return;
  if (app.panelTab === 'terminal') {
    // xterm paints into a sized box; it stays mounted across tab switches so
    // the shell (and its scrollback) survives.
    mountTerminal(termBody);
    return;
  }
  termBody.classList.remove('term-host');
  clear(termBody);
  if (app.panelTab === 'usage') {
    const u = app.usage;
    if (!u) {
      termBody.append(h('div', { class: 'term-line' }, t('common.loading')));
      void api.usage().then((r) => {
        app.usage = r;
        drawPanelBody();
      });
      return;
    }
    const rows: [string, string][] = [
      [t('stats.calls'), `${u.calls}`],
      [t('stats.input'), fmtNum(u.input_tokens)],
      [t('stats.output'), fmtNum(u.output_tokens)],
      [t('stats.reasoning'), fmtNum(u.reasoning_tokens)],
      [t('stats.cacheRead'), fmtNum(u.cache_read_tokens)],
      ['elapsed', `${u.elapsed_secs.toFixed(1)}s`],
      ['errors', `${u.errors}`],
    ];
    for (const [k, v] of rows) {
      termBody.append(h('div', { class: 'term-line' },
        h('span', { style: { color: 'var(--text-2)', display: 'inline-block', width: '110px' } }, k), v));
    }
    for (const m of u.by_model.slice(0, 8)) {
      termBody.append(h('div', { class: 'term-line', style: { color: 'var(--text-2)' } },
        `${m.model}  ·  in ${fmtNum(m.usage.input_tokens)} / out ${fmtNum(m.usage.output_tokens)}`));
    }
    return;
  }

  if (app.terminal.length === 0) {
    app.terminal.push(`${t('app.name')} · ${app.config?.workspace || t('explorer.empty')}`);
  }  for (const line of app.terminal) {
    const isCmd = line.startsWith('$ ');
    const isErr = line.startsWith('! ');
    termBody.append(h('div', { class: `term-line${isCmd ? ' cmd' : isErr ? ' err' : ''}` }, line));
  }
  termBody.scrollTop = termBody.scrollHeight;
}

/* --------------------------------------------------------------- statusbar */

function buildStatusbar(): HTMLElement {
  statusDot = h('span', { class: 'sb-dot' });
  statusTextEl = h('span', null, t('status.ready'));
  const left = h('div', { class: 'sb-item' }, statusDot, statusTextEl);

  statusBranch = h('div', {
    class: 'sb-item clickable',
    onClick: () => selectView('git'),
  }, icon('branch', 12), h('span', null, app.gitBranch || '—'));

  statusTokens = h('div', { class: 'sb-item' }, icon('zap', 12), h('span', null, '0'));
  extStatusEl = h('div', { class: 'sb-exts' });
  statusModel = h('div', { class: 'sb-item clickable', onClick: () => { setSurface('chat'); refreshComposer(); } },
    icon('cpu', 12), h('span', null, app.model || t('common.notConfigured')));

  const version = h('div', { class: 'sb-item' }, app.info?.version || 'Beta 0.1');

  return h('div', { class: 'statusbar' },
    left,
    statusBranch,
    extStatusEl!,
    h('div', { class: 'spacer', style: { flex: '1' } }),
    statusTokens,
    statusModel,
    version,
  );
}

export function renderStatus() {
  if (statusTextEl) {
    statusTextEl.textContent = app.statusText || (app.busy ? t('status.busy') : t('status.ready'));
  }
  statusDot?.classList.toggle('busy', app.busy);
  if (statusBranch) {
    const label = statusBranch.querySelector('span');
    if (label) label.textContent = app.gitBranch || '—';
  }
  if (statusTokens) {
    const label = statusTokens.querySelector('span');
    if (label) label.textContent = `${fmtNum(app.contextTokens)} ${t('status.tokens')}`;
  }
  if (statusModel) {
    const label = statusModel.querySelector('span');
    if (label) label.textContent = app.model || t('common.notConfigured');
  }
}

/**
 * Status-bar slots owned by extensions (`createStatusBarItem`). Left alignment
 * sits next to the branch; right alignment joins the token counter.
 */
function renderExtStatus() {
  if (!extStatusEl) return;
  clear(extStatusEl);
  for (const item of [...app.extStatus].sort((a, b) => b.priority - a.priority).slice(0, 6)) {
    if (!item.visible || !item.text.trim()) continue;
    extStatusEl.append(
      h('div', {
        class: `sb-item${item.command ? ' clickable' : ''}`,
        title: item.tooltip || item.text,
        onClick: () => {
          if (!item.command) return;
          void api.exthostRun(item.command).catch((e) => toast(errText(e), 'err', 3000));
        },
      }, h('span', null, plainStatusText(item.text))),
    );
  }
}

/**
 * VS Code writes status items as `$(radio-tower) Port: 3000`.
 *
 * There is no codicon font here, so the icon token would render as the literal
 * text `$(radio-tower)`. Dropping it keeps the label; when the item is *only*
 * an icon, the name is used so the control is never blank.
 */
function plainStatusText(text: string): string {
  const stripped = text
    .replace(/\$\([a-zA-Z0-9~-]+\)/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
  if (stripped) return stripped;
  const only = /\$\(([a-zA-Z0-9~-]+)\)/.exec(text)?.[1];
  return only ? only.replace(/[-~]/g, ' ') : text;
}

/* ------------------------------------------------------------ chat surface */

/** Kept for the places that still say "open/close the chat" (attach-to-chat,
 *  the titlebar button, Ctrl+Shift+C). Opening the chat shows the dock; closing
 *  it falls back to the file view when there is one. */
export function applyChatOpen(open: boolean) {
  app.chatOpen = open;
  if (open) {
    setSurface('chat');
  } else if (app.files.length > 0) {
    setSurface('editor');
  }
}

export function toggleChat(force?: boolean) {
  const want = force === undefined ? app.surface !== 'chat' : force;
  applyChatOpen(want);
}

/* --------------------------------------------------------------- resizers */

function bindResizer(el: HTMLElement) {
  el.addEventListener('pointerdown', (e: PointerEvent) => {
    e.preventDefault();
    el.setPointerCapture(e.pointerId);
    const startX = e.clientX;
    const startW = app.sidebarWidth;
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';

    const move = (ev: PointerEvent) => {
      if (!root) return;
      app.sidebarWidth = Math.max(180, Math.min(520, startW + ev.clientX - startX));
      root.style.setProperty('--sidebar-w', `${app.sidebarWidth}px`);
    };
    const up = () => {
      el.removeEventListener('pointermove', move);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };
    el.addEventListener('pointermove', move);
    el.addEventListener('pointerup', up, { once: true });
    el.addEventListener('pointercancel', up, { once: true });
  });
}

/* ----------------------------------------------------------------- relabel */

/** A language change that landed mid-stream: rebuild the panels when the turn
 *  ends, rather than yanking the streaming message out from under the user. */
let pendingRelabel = false;

/**
 * Refresh every piece of chrome that carries a `t()` string.
 *
 * The sidebar body and the dock panels hold their labels from mount time, so
 * they are re-mounted from `app` state instead of patched field by field —
 * `renderLog()` already rebuilds the transcript that way, and this keeps the
 * translation surface in one place (the mount functions) instead of two.
 */
function relabel() {
  const bar = root?.querySelector('.titlebar');
  if (bar) bar.replaceWith(buildTitlebar());

  paintActivity();
  paintSideHead(activeExtView ? `ext:${activeExtView}` : app.view);
  paintRail();
  renderStatus();

  if (app.panelOpen) {
    panelEl?.remove();
    panelEl = null;
    termBody = null;
    renderPanel();
  }

  if (app.busy) {
    pendingRelabel = true;
    return;
  }
  remountPanels();
}

function remountPanels() {
  const dock = surfaceEl?.querySelector('.dock') as HTMLElement | null;
  if (!dock) return;
  clear(dock);
  mountChat(dock);
  mountReview(dock);
  mountBridge(dock);
  mountExtensions(dock);
  setSurface(app.surface);
  if (app.surface === 'review') void refreshReview();
  else if (app.surface === 'bridge') void refreshBridge();
  else if (app.surface === 'extensions') void refreshExtensions();
}

/* ------------------------------------------------------------ global wiring */

function bindGlobal() {
  window.addEventListener('keydown', (e: KeyboardEvent) => {
    const mod = e.ctrlKey || e.metaKey;
    if (!mod) return;
    const k = e.key.toLowerCase();
    if (k === 's') {
      e.preventDefault();
      void saveActive();
    } else if (k === 'b') {
      e.preventDefault();
      const collapsed = app.sidebarWidth === 0;
      app.sidebarWidth = collapsed ? 264 : 0;
      bodyEl?.classList.toggle('no-side', !collapsed);
    } else if (k === 'j') {
      e.preventDefault();
      // Toggle, but always land on the terminal tab: after F5 the output tab
      // is in front, and Ctrl+J asking for a shell should not show run logs.
      if (app.panelOpen && app.panelTab === 'terminal') {
        togglePanel(false);
        return;
      }
      app.panelTab = 'terminal';
      if (!app.panelOpen) togglePanel(true);
      else {
        drawPanelBody();
        terminalFocus();
      }
    } else if (k === '`') {
      e.preventDefault();
      if (app.panelOpen && app.panelTab === 'terminal' && !e.shiftKey) {
        togglePanel(false);
        return;
      }
      app.panelTab = 'terminal';
      if (!app.panelOpen) togglePanel(true);
      else drawPanelBody();
      if (e.shiftKey) newTerminal();
      else terminalFocus();
    } else if (k === ',') {
      e.preventDefault();
      openSettings('providers');
    } else if (k === 'c' && e.shiftKey) {
      e.preventDefault();
      toggleChat();
    } else if (k === 'p' && e.shiftKey) {
      e.preventDefault();
      openPalette();
    }
  });

  on('chat:toggle', (v: boolean) => applyChatOpen(v !== false));
  on('surface', () => paintRail());
  on('settings:open', (tab: any) => openSettings(tab || 'providers'));
  on('status', () => renderStatus());
  on('agent:busy', () => {
    renderStatus();
    if (app.panelOpen && app.panelTab === 'usage') drawPanelBody();
    if (!app.busy && pendingRelabel) {
      pendingRelabel = false;
      remountPanels();
    }
  });
  on('lang:changed', () => relabel());
  on('workspace', () => {
    const ws = root?.querySelector('.tb-ws');
    if (ws) {
      ws.textContent = app.config?.workspace || t('explorer.empty');
      ws.setAttribute('title', app.config?.workspace || '');
    }
    if (mounted === 'explorer') void renderExplorer();
    void refreshGit().catch(() => undefined);
    renderStatus();
  });
  on('git:changed', () => {
    void refreshGit().catch(() => undefined);
    paintRail();
    if (app.surface === 'review') void refreshReview();
  });
  on('skills:changed', () => { if (mounted === 'skills') void loadSkills(); });
  on('models:changed', () => { refreshComposer(); renderStatus(); });
  on('config:changed', () => {
    applyTheme(app.config);
    renderHead();
    refreshComposer();
    // A provider may have been switched on/off (or the bridge started). Rebuild
    // the model list instead of leaving the composer on a stale selection.
    void reconcileProviders();
  });
  on('session:changed', () => refreshComposer());
  on('editor:file', () => renderStatus());
  on('editor:cover', (show: boolean) => setSurface(show ? 'editor' : 'chat'));
  // The file runner streams into the terminal panel; opening it is part of
  // "run this file", not something the user should have to click too.
  on('panel:open', () => {
    togglePanel(true);
    if (app.panelTab === 'terminal') terminalFocus();
  });
  on('panel:refresh', () => {
    if (app.panelOpen && app.panelTab === 'output') drawPanelBody();
  });
  on('editor:tabs', () => {
    if (app.files.length === 0 && app.surface === 'editor') setSurface('chat');
    paintRail();
  });
  on('review:count', () => paintActivity());
  on('exthost:status', () => paintRail());
  on('exthost:statusbar', () => renderExtStatus());
  on('bridge:status', () => paintRail());
}
