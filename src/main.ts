// IRIXEN Code — renderer entry point. Boots config, mounts the shell, reveals
// the window, and drives the startup checklist that index.html paints.

import './styles/base.css';
import './styles/app.css';

import { api, errText, onExtHostEvent } from './core/api';
import { app, emit } from './core/state';
import { setLang, t } from './core/i18n';
import { applyTheme } from './ui/theme';
import { mountShell, renderStatus } from './ui/shell';
import {
  loadSession, newSession, refreshComposer, ensureSelection,
  autoEnableWeb2Api, watchWeb2ApiSessions,
} from './ui/chat';
import { pickDefaultProvider } from './core/providers';
import { refreshGit } from './ui/git';
import { toast, reportProblem } from './ui/toast';
import type { HostLog, HostStatus } from './types';
import { previewConfig } from './core/preview';
import { bootFinish, bootInit, bootRelabel, bootStep, within } from './core/boot';
import { maybeShowOnboarding } from './ui/onboarding';

/** How long the checklist will wait on a single optional stage before telling
 *  the user it is continuing in the background. Starting the proxy and
 *  restoring a session are both network-bound; a slow upstream must never make
 *  the window look hung. */
const STAGE_BUDGET_MS = 2800;

async function boot() {
  // The card is already on screen (index.html). Hand it the localized labels
  // immediately so the first frame and the finished card agree.
  bootInit();

  const host = document.createElement('div');
  host.id = 'root';
  document.body.append(host);

  let version = '';
  try {
    const [config, info] = await Promise.all([api.getConfig(), api.appInfo()]);
    devThemeOverride(config);
    app.config = config;
    app.info = info;
    app.providers = config.providers;
    setLang(config.language);
    applyTheme(config);
    version = info.version || '';

    // Same rule as the agent (config.rs::default_provider_id): a provider that
    // is only "enabled" but has no key cannot be the default, or the first
    // message goes out unauthenticated and comes back as a 401.
    app.providerId =
      config.active?.provider_id || pickDefaultProvider(config.providers)?.id || '';
    app.model = config.active?.model || '';
    app.effort = config.active?.effort || config.effort || 'auto';
  } catch (e) {
    if (import.meta.env.DEV) {
      // Plain browser (vite dev): mount with a representative config so the
      // shell has something to lay out.
      const cfg = previewConfig();
      devThemeOverride(cfg);
      app.config = cfg;
      app.providers = cfg.providers;
      setLang(cfg.language);
      applyTheme(cfg);
      app.providerId = cfg.active?.provider_id || pickDefaultProvider(cfg.providers)?.id || '';
      app.model = cfg.active?.model || '';
      app.effort = cfg.active?.effort || cfg.effort || 'auto';
    } else {
      app.config = null;
      console.error('[irixen] config load failed', e);
    }
  }

  // The config file carries the language, which may differ from the OS locale
  // the splash guessed. Redraw the labels once, before the first step settles.
  bootRelabel();
  bootStep('core', 'done', version);

  bootStep('ui', 'active');
  mountShell(host);
  renderStatus();
  bootStep('ui', 'done');

  try {
    await api.mainLoaded();
  } catch {
    /* running in a plain browser (vite dev) — ignore */
  }

  // `finally` so a throw anywhere in the sequence still hands the window over;
  // the card's own 7s watchdog is the last resort, not the plan.
  void afterPaint().finally(() => {
    bootFinish();
    // First-run setup waits for the boot card's 170ms fade. The wizard sits at
    // z-index 60, below the problem dialog (95), so a fatal error reported
    // during startup is never hidden behind it.
    setTimeout(() => maybeShowOnboarding(), 240);
  });
}

/** Everything that can fail or be slow, after the first frame is visible.
 *  Each stage reports to the checklist; the card leaves once they settle or
 *  their budget runs out. */
async function afterPaint() {
  // Extensions: reading manifests is pure disk IO and has nothing to do with
  // the session, so it runs beside the restore instead of after it — the
  // checklist advances without adding wall-clock time to startup.
  const extScan = (async () => {
    bootStep('ext', 'active');
    try {
      const list = await api.extensionsList();
      app.extensions = list;
      // The activity bar reads `contributes.viewsContainers` straight from
      // this list; it was painted before the scan finished on a cold start.
      emit('extview:changed');
      const enabled = list.filter((e) => e.enabled).length;
      bootStep('ext', 'done', enabled ? t('boot.extCount', { n: enabled }) : '');
    } catch {
      bootStep('ext', 'skip', t('boot.deferred'));
    }
  })();

  bootStep('workspace', 'active');
  try {
    const active = await api.active();
    if (active) await loadSession(active);
    else await newSession();
  } catch (e) {
    console.warn('[irixen] session restore failed', e);
  }
  refreshComposer();
  void refreshGit().catch(() => undefined);
  bootStep('workspace', 'done', workspaceName());

  // Resolve provider + model and write them into the session; without this the
  // pickers can show one provider while the agent sends with another. Budgeted
  // like the proxy: listing models hits the network, and a dead endpoint used
  // to leave the window on the checklist with no explanation.
  try {
    await within(ensureSelection(), STAGE_BUDGET_MS);
    refreshComposer();
    renderStatus();
    emit('models:changed');
  } catch (e) {
    console.warn('[irixen] provider selection failed', e);
  }

  // Web2API: hidden sessions, the local endpoint, and only the models of
  // reverse proxies that are actually logged in. Every *automatic* start is
  // gated on the explicit "start on launch" switch. When it is off we spawn
  // no WebView2 windows and install no polling loop — by far the biggest
  // idle-memory saving in the app.
  if (app.config?.bridge.auto_start === true) {
    bootStep('bridge', 'active');
    watchWeb2ApiSessions();
    const bridge = await within(autoEnableWeb2Api(), STAGE_BUDGET_MS);
    if (bridge?.running) bootStep('bridge', 'done', `:${bridge.port}`);
    else if (bridge) bootStep('bridge', 'skip');
    else bootStep('bridge', 'skip', t('boot.deferred'));
  } else {
    bootStep('bridge', 'skip');
  }

  // The window can be used from here on; everything below is warm-up.
  await extScan;
  void warmUp();
}
/** Post-boot warm-up: cheap, silent, and allowed to fail one by one. */
async function warmUp() {
  try {
    app.skills = await api.skills();
  } catch {
    /* ignore */
  }
  try {
    app.mcp = await api.mcpServers();
  } catch {
    /* ignore */
  }
  try {
    app.usage = await api.usage();
  } catch {
    /* ignore */
  }

  const ws = app.config?.workspace;
  if (!ws) return;
  try {
    if (await api.gitIsRepo()) {
      app.gitRepo = true;
      app.gitBranch = await api.gitBranch();
      renderStatus();
    }
  } catch {
    /* not a repo */
  }
}

/** Last path segment of the workspace, for the checklist's right-hand detail. */
function workspaceName(): string {
  const ws = app.config?.workspace || '';
  if (!ws) return '';
  const parts = ws.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || ws;
}

/** Dev-only: `?theme=light|dark` renders the other palette in a plain browser
 *  without touching the saved config. Handy for reviewing both themes. */
function devThemeOverride<T extends { theme: string }>(cfg: T): T {
  if (!import.meta.env.DEV) return cfg;
  const q = new URLSearchParams(location.search).get('theme');
  if (q === 'light' || q === 'dark') cfg.theme = q;
  return cfg;
}

window.addEventListener('error', (e) => {
  const detail = e.error instanceof Error ? (e.error.stack || e.error.message) : e.message;
  console.error('[irixen] uncaught', e.error || e.message);
  reportProblem(String(detail || 'unknown error'), 'renderer');
});
window.addEventListener('unhandledrejection', (e) => {
  const msg = errText((e as PromiseRejectionEvent).reason);
  console.error('[irixen] unhandled rejection', msg);
  // A missing IPC bridge means we are running in a plain browser (`vite dev`);
  // that is a known state, not something to alarm the user about.
  const noBridge = /transformCallback|__TAURI_INTERNALS__|not a function/.test(msg);
  if (app.config && app.info && !noBridge) reportProblem(msg, 'promise');
});

// Extension hosts stream stderr as `err` level, but a lot of perfectly healthy
// extensions log there (Live Preview prints "Server is running" to stderr).
// Only lines that actually describe a failure should open the dialog. Real
// exceptions also arrive as `out` lines via console.log (Prettier prints its
// TypeError that way), so those get a stricter pattern that skips INFO chatter.
const FATAL_LOG = /(uncaught|unhandled|TypeError|ReferenceError|SyntaxError|Error:|Cannot read|is not a function|crash|激活失败|启动失败|宿主退出|exited with|EADDRINUSE|ENOENT)/i;
const FATAL_OUT = /(TypeError:|ReferenceError:|SyntaxError:|UnhandledPromiseRejection|激活失败|启动失败)/i;
void onExtHostEvent<HostLog>('log', (row) => {
  const text = row.text || '';
  const fatal = row.level === 'err' ? FATAL_LOG.test(text) : FATAL_OUT.test(text);
  if (!fatal) return;
  // Anything an extension printed belongs to that extension, not to the IDE.
  // It gets the "插件故障" dialog with a restart for just that host.
  reportProblem(text, row.id || 'extension host', 'plugin', row.id || '');
});
void onExtHostEvent<{ hosts: HostStatus[] }>('status', (ev) => {
  for (const host of ev.hosts || []) {
    if (host.state === 'crashed' || (host.state !== 'running' && host.error)) {
      reportProblem(
        `${host.id}: ${host.error || 'extension host stopped unexpectedly'}`,
        'extension host',
        'plugin',
        host.id,
      );
    }
  }
});

void boot();
