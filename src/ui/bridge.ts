// Web2API panel — reverse-proxy browser-based AI apps behind a local
// OpenAI/Anthropic-compatible endpoint.
//
// The panel edits a draft copy of `app.config.bridge`; nothing reaches the
// running server until Save. That matters here more than in other panels:
// a half-typed session cookie must not be picked up by the next request.

import { app, emit } from '../core/state';
import { api, errText } from '../core/api';
import { h, clear, copyText } from '../core/dom';
import { icon } from './icons';
import { t } from '../core/i18n';
import { toast } from './toast';
import type { BridgeConfig, BridgeStatus, BridgeUpstream, WebSessionInfo, WebSource } from '../types';

let root: HTMLElement | null = null;
let configEl: HTMLElement | null = null;
let dotEl: HTMLElement | null = null;
let stateEl: HTMLElement | null = null;
let toggleBtn: HTMLButtonElement | null = null;
let saveBtn: HTMLButtonElement | null = null;
let urlInput: HTMLInputElement | null = null;
let keyInput: HTMLInputElement | null = null;
let portInput: HTMLInputElement | null = null;
let autoInput: HTMLInputElement | null = null;
let modeSel: HTMLSelectElement | null = null;
let defaultSel: HTMLSelectElement | null = null;
let modelInput: HTMLInputElement | null = null;
let providerSel: HTMLSelectElement | null = null;
let providerRow: HTMLElement | null = null;
let routesEl: HTMLElement | null = null;
let sourcesEl: HTMLElement | null = null;
let routingCards: HTMLElement[] = [];
let logBody: HTMLElement | null = null;
let statsEl: HTMLElement | null = null;
let testBtn: HTMLButtonElement | null = null;
let testEl: HTMLElement | null = null;
let sessionEl: HTMLElement | null = null;
let startEl: HTMLElement | null = null;
let autoEnsureInput: HTMLInputElement | null = null;

let status: BridgeStatus | null = null;
let sessions: WebSessionInfo[] = [];
let sessionSig = '';
let poll: number | null = null;
let draft: BridgeConfig | null = null;
let gateEl: HTMLElement | null = null;

const clone = (c: BridgeConfig): BridgeConfig => JSON.parse(JSON.stringify(c)) as BridgeConfig;

/**
 * Mounts the panel into `host` and returns its disposer.
 *
 * It is a dock surface again — Settings → Advanced only owns the switch that
 * makes the rail button exist. Polling belongs to the mount: unmounting tears
 * the panel down, and nothing may keep a 3 s timer alive after that.
 */
export function mountBridge(host: HTMLElement): () => void {
  root = h('div', { class: 'bridge', dataset: { panel: 'bridge' } });

  dotEl = h('span', { class: 'sb-dot' });
  stateEl = h('span', { class: 'bridge-state' }, t('bridge.stopped'));
  toggleBtn = h('button', {
    class: 'btn primary',
    onClick: () => void toggleRun(),
  }, icon('plug', 12), t('bridge.start')) as HTMLButtonElement;
  saveBtn = h('button', {
    class: 'btn',
    title: t('bridge.save'),
    onClick: () => void apply(),
  }, icon('check', 12), t('bridge.save')) as HTMLButtonElement;

  const head = h(
    'div',
    { class: 'chat-head' },
    h('span', { class: 'chat-title' }, 'WEB2API'),
    h('span', { class: 'bridge-dot' }, dotEl),
    stateEl,
    h('div', { class: 'spacer', style: { flex: '1' } }),
    // Group the actions so they keep a readable gap even when the shared
    // `.chat-head` gap is tightened elsewhere.
    h('div', { class: 'bridge-actions' },
      toggleBtn,
      saveBtn,
      h('button', {
        class: 'icon-btn',
        title: t('common.refresh'),
        onClick: () => void refreshBridge(),
      }, icon('refresh', 13)),
    ),
  );

  configEl = h('div', { class: 'bridge-config' });
  // The second gate sits above the settings it guards: the panel is visible as
  // soon as the switch is on, but the endpoint does not answer until this is
  // confirmed — and that is the one thing a switch must not grant silently.
  gateEl = h('div', { class: 'bridge-gate' });
  statsEl = h('div', { class: 'bridge-note' });
  logBody = h('div', { class: 'bridge-log' });
  const logCard = h(
    'div',
    { class: 'bridge-card bridge-log-card' },
    h('div', { class: 'row' },
      h('div', { class: 'bridge-label' }, t('bridge.log')),
      h('div', { class: 'spacer', style: { flex: '1' } }),
      statsEl,
    ),
    logBody,
  );

  // The header is a fixed 32px row; keeping the test action out of it avoids
  // the button-over-border problem after the global font bump.
  testBtn = h(
    'button',
    { class: 'btn', onClick: () => void runTest() },
    icon('zap', 13),
    t('bridge.test'),
  ) as HTMLButtonElement;
  testEl = h('div', { class: 'probe', hidden: true });
  const testCard = h(
    'div',
    { class: 'bridge-card' },
    h('div', { class: 'row' },
      h('div', { class: 'bridge-label' }, t('bridge.test')),
      h('div', { class: 'spacer', style: { flex: '1' } }),
      testBtn,
    ),
    testEl,
  );

  root.append(head, h('div', { class: 'bridge-body' }, gateEl, configEl, testCard, logCard));
  host.append(root);

  renderConfig();
  renderGate();
  void refreshBridge();
  void refreshSessions(false);
  startPolling();

  return () => {
    stopPolling();
    root?.remove();
    root = null;
    gateEl = null;
    // A pending paint would otherwise write into a detached tree; nulling the
    // draft also makes a late `apply()` a no-op instead of a surprise save.
    draft = null;
  };
}

/* ------------------------------------------------------------------ form */

function textInput(value: string, onInput: (v: string) => void, props: Record<string, unknown> = {}) {
  const el = h('input', { class: 'mono-input', value, ...props }) as HTMLInputElement;
  el.addEventListener('input', () => onInput(el.value));
  return el;
}

function textArea(value: string, onInput: (v: string) => void, rows = 3) {
  const el = h('textarea', { class: 'bridge-textarea', rows, spellcheck: false }, value) as HTMLTextAreaElement;
  el.addEventListener('input', () => onInput(el.value));
  return el;
}

function field(label: string, control: HTMLElement) {
  return h('div', { class: 'bridge-field' }, h('div', { class: 'bridge-label' }, label), control);
}

function renderConfig() {
  if (!configEl || !app.config) return;
  draft = clone(app.config.bridge);
  clear(configEl);
  routingCards = [];

  /* endpoint ------------------------------------------------------------- */
  urlInput = h('input', { readonly: true, class: 'mono-input' }) as HTMLInputElement;
  urlInput.value = `http://127.0.0.1:${draft.port}/v1`;
  keyInput = textInput(draft.api_key, (v) => { if (draft) draft.api_key = v; });
  portInput = textInput(String(draft.port), (v) => {
    if (!draft) return;
    const n = Number.parseInt(v, 10);
    if (Number.isFinite(n)) draft.port = n;
  }, { class: 'mono-input bridge-port' });
  autoInput = h('input', { type: 'checkbox' }) as HTMLInputElement;
  autoInput.checked = draft.auto_start;

  const endpointCard = h(
    'div',
    { class: 'bridge-card' },
    h('div', { class: 'bridge-label' }, t('bridge.endpoint')),
    h('div', { class: 'row' },
      urlInput,
      h('button', {
        class: 'icon-btn',
        title: t('bridge.copy'),
        onClick: async () => {
          if (urlInput && (await copyText(urlInput.value))) toast(t('bridge.copied'), 'ok', 1200);
        },
      }, icon('copy', 13)),
    ),
    h('div', { class: 'bridge-label', style: { marginTop: '8px' } }, t('bridge.apiKey')),
    h('div', { class: 'row' },
      keyInput,
      h('button', {
        class: 'icon-btn',
        title: t('bridge.generate'),
        onClick: () => {
          if (!keyInput || !draft) return;
          keyInput.value = makeKey();
          draft.api_key = keyInput.value;
          toast(t('bridge.generated'), 'ok', 1400);
        },
      }, icon('refresh', 13)),
    ),
    h('div', { class: 'row bridge-port-row' },
      h('div', { class: 'bridge-label' }, t('bridge.port')),
      portInput,
      h('label', { class: 'bridge-check' }, autoInput, h('span', null, t('bridge.autoStart'))),
    ),
  );

  /* mode ----------------------------------------------------------------- */
  modeSel = h('select', {},
    h('option', { value: 'web' }, t('bridge.modeWeb')),
    h('option', { value: 'provider' }, t('bridge.modeProviderDebug')),
  ) as HTMLSelectElement;
  modeSel.value = draft.upstream === 'provider' ? 'provider' : 'web';
  modeSel.addEventListener('change', () => {
    if (!draft) return;
    draft.upstream = (modeSel!.value || 'web') as BridgeUpstream;
    applyModeVisibility();
  });

  defaultSel = h('select', {},
    ...draft.sources.map((s) => h('option', { value: s.id }, s.label)),
  ) as HTMLSelectElement;
  defaultSel.value = draft.default_source;

  modelInput = textInput(draft.model, (v) => { if (draft) draft.model = v; }, { placeholder: 'claude-sonnet-4-5' });

  providerSel = h('select', {}) as HTMLSelectElement;
  {
    const providers = (app.config.providers || []).filter((p) => p.enabled && p.base_url.trim());
    providerSel.append(h('option', { value: '' }, t('bridge.providerAuto')));
    for (const p of providers) {
      providerSel.append(h('option', { value: p.id }, `${p.name || p.id}${p.api_key.trim() ? '' : ` · ${t('settings.noKey')}`}`));
    }
    providerSel.value = draft.provider_id || '';
  }
  providerRow = h('div', { class: 'bridge-sub' },
    h('div', { class: 'bridge-label' }, t('bridge.provider')),
    providerSel,
  );

  const defaultRow = h('div', { class: 'bridge-sub' },
    h('div', { class: 'bridge-label' }, t('bridge.defaultProxy')),
    defaultSel,
    h('div', { class: 'bridge-label', style: { marginTop: '8px' } }, t('bridge.model')),
    modelInput,
  );

  const modeCard = h(
    'div',
    { class: 'bridge-card' },
    h('div', { class: 'bridge-label' }, t('bridge.mode')),
    modeSel,
    providerRow,
  );

  /* startup options ------------------------------------------------------ */
  startEl = h('div', { class: 'bridge-start-list' });
  autoEnsureInput = h('input', { type: 'checkbox' }) as HTMLInputElement;
  autoEnsureInput.checked = draft.auto_ensure !== false;
  const startCard = h(
    'div',
    { class: 'bridge-card' },
    h('div', { class: 'bridge-label' }, t('bridge.startOptions')),
    startEl,
    defaultRow,
    h('label', { class: 'bridge-check bridge-check-block' },
      autoEnsureInput, h('span', null, t('bridge.autoEnsure'))),
  );
  routingCards.push(startCard);

  /* routes --------------------------------------------------------------- */
  routesEl = h('div', { class: 'bridge-routes' });
  const routesCard = h(
    'div',
    { class: 'bridge-card' },
    h('div', { class: 'row' },
      h('div', { class: 'bridge-label' }, t('bridge.routes')),
      h('div', { class: 'spacer', style: { flex: '1' } }),
      h('button', {
        class: 'btn sm',
        onClick: () => {
          if (!draft) return;
          draft.routes.push({ prefix: '', source_id: draft.default_source });
          renderRoutes();
        },
      }, icon('plus', 12), t('common.add')),
    ),
    h('div', { class: 'bridge-route-head' },
      h('span', { class: 'bridge-route-head-prefix' }, t('bridge.routePrefix')),
      h('span', { class: 'bridge-route-head-target' }, t('bridge.routeTarget')),
    ),
    routesEl,
  );
  routingCards.push(routesCard);

  /* sources -------------------------------------------------------------- */
  sourcesEl = h('div', { class: 'bridge-sources' });
  const sourcesCard = h(
    'div',
    { class: 'bridge-card' },
    h('div', { class: 'bridge-label' }, t('bridge.sources')),
    sourcesEl,
  );
  routingCards.push(sourcesCard);

  /* login state ---------------------------------------------------------- */
  sessionEl = h('div', { class: 'bridge-sessions' });
  const sessionCard = h(
    'div',
    { class: 'bridge-card' },
    h('div', { class: 'row' },
      h('div', { class: 'bridge-label' }, t('bridge.sessions')),
      h('div', { class: 'spacer', style: { flex: '1' } }),
      h('button', {
        class: 'btn sm',
        onClick: () => void refreshSessions(true),
      }, icon('refresh', 12), t('bridge.getLogin')),
    ),
    sessionEl,
  );

  configEl.append(endpointCard, modeCard, startCard, routesCard, sourcesCard, sessionCard);
  renderRoutes();
  renderSources();
  renderStartOptions();
  applyModeVisibility();
  void refreshSessions(false);
}

/** Which reverse proxies start with the endpoint, and their login state. */
function renderStartOptions() {
  if (!startEl || !draft) return;
  const el = startEl;
  clear(el);
  draft.sources.forEach((src, i) => {
    const box = h('input', { type: 'checkbox' }) as HTMLInputElement;
    box.checked = src.auto_open !== false;
    box.addEventListener('change', () => {
      if (draft) draft.sources[i].auto_open = box.checked;
    });
    const info = sessions.find((s) => s.id === src.id);
    const badge = h('span', {
      class: `bridge-login-badge${info?.logged_in ? ' on' : ''}`,
    }, info?.logged_in ? t('bridge.loggedIn') : t('bridge.notLoggedIn'));
    el.append(h('label', { class: 'bridge-start-row' },
      box,
      h('span', { class: 'bridge-start-name' }, src.label),
      h('span', { class: 'bridge-start-id' }, src.id),
      badge,
      h('span', { class: 'bridge-start-hint' }, t('bridge.startWith')),
    ));
  });
}

async function refreshSessions(probe: boolean) {
  try {
    sessions = probe ? await api.web2apiProbe() : await api.web2apiSessionStatus();
  } catch {
    return;
  }
  const sig = sessions.map((s) => `${s.id}:${s.logged_in ? 1 : 0}`).join('|');
  const changed = sig !== sessionSig;
  sessionSig = sig;
  paintSessions();
  // Only rebuild the checkbox rows when a session actually changed — a
  // full re-render every 3s would steal focus from a checkbox mid-click.
  if (changed) renderStartOptions();
}

function paintSessions() {
  if (!sessionEl) return;
  clear(sessionEl);
  if (!sessions.length) {
    sessionEl.append(h('div', { class: 'side-empty', style: { padding: '8px 2px', textAlign: 'left' } }, t('common.notConfigured')));
    return;
  }
  for (const s of sessions) {
    sessionEl.append(h('div', { class: 'bridge-session-row' },
      h('span', { class: `bridge-login-badge${s.logged_in ? ' on' : ''}` },
        s.logged_in ? t('bridge.loggedIn') : t('bridge.notLoggedIn')),
      h('span', { class: 'bridge-session-name' }, s.label || s.id),
      h('span', { class: 'bridge-session-detail' },
        s.account || s.detail || (s.open ? t('bridge.sessionOpen') : '')),
    ));
  }
}

function applyModeVisibility() {
  const web = draft?.upstream === 'web';
  for (const card of routingCards) card.hidden = !web;
  if (providerRow) providerRow.hidden = web;
}

function renderRoutes() {
  if (!routesEl || !draft) return;
  const el = routesEl;
  clear(el);
  const sources = draft.sources;
  draft.routes.forEach((route, i) => {
    const prefix = textInput(route.prefix, (v) => { if (draft) draft.routes[i].prefix = v; }, { placeholder: 'claude' });
    prefix.classList.add('bridge-route-prefix');
    const target = h('select', { class: 'bridge-route-target' },
      ...sources.map((s) => h('option', { value: s.id }, s.label)),
    ) as HTMLSelectElement;
    target.value = route.source_id;
    target.addEventListener('change', () => { if (draft) draft.routes[i].source_id = target.value; });
    el.append(
      h('div', { class: 'bridge-route' },
        prefix,
        h('span', { class: 'bridge-arrow' }, '→'),
        target,
        h('button', {
          class: 'icon-btn',
          title: t('common.delete'),
          onClick: () => {
            if (!draft) return;
            draft.routes.splice(i, 1);
            renderRoutes();
          },
        }, icon('trash', 13)),
      ),
    );
  });
}

function renderSources() {
  if (!sourcesEl || !draft) return;
  const el = sourcesEl;
  clear(el);
  draft.sources.forEach((src, i) => {
    const set = <K extends keyof WebSource>(key: K, value: WebSource[K]) => {
      if (draft) draft.sources[i][key] = value;
    };
    const browser = (src.mode || 'browser') !== 'http';
    const ready = browser || src.session_key.trim().length > 0;
    const badge = h('span', { class: `bridge-src-badge${ready ? '' : ' missing'}` },
      browser ? t('bridge.browserBadge') : ready ? t('bridge.ready') : t('bridge.missing'));
    const summary = h('summary', {},
      h('b', null, src.label),
      h('span', { class: 'bridge-src-id' }, src.id),
      badge,
    );

    const labelInput = textInput(src.label, (v) => {
      set('label', v);
      const b = summary.querySelector('b');
      if (b) b.textContent = v;
    });
    const sessionInput = textInput(src.session_key, (v) => {
      set('session_key', v);
      if (badge && (src.mode || 'browser') === 'http') {
        badge.textContent = v.trim() ? t('bridge.ready') : t('bridge.missing');
        badge.classList.toggle('missing', !v.trim());
      }
    }, { type: 'password', placeholder: 'sessionKey / access token / cookie' });
    const sessionRow = field(t('bridge.srcSession'), sessionInput);
    sessionRow.hidden = browser;

    const srcMode = h('select', { class: 'mono-input' },
      h('option', { value: 'browser' }, t('bridge.modeBrowser')),
      h('option', { value: 'http' }, t('bridge.modeHttp')),
    ) as HTMLSelectElement;
    srcMode.value = browser ? 'browser' : 'http';
    srcMode.addEventListener('change', () => {
      const isBrowser = srcMode.value === 'browser';
      set('mode', srcMode.value);
      sessionRow.hidden = isBrowser;
      driverRow.hidden = !isBrowser;
      syncPromptRow();
      badge.textContent = isBrowser
        ? t('bridge.browserBadge')
        : src.session_key.trim() ? t('bridge.ready') : t('bridge.missing');
      badge.classList.toggle('missing', !isBrowser && !src.session_key.trim());
    });

    const homeInput = textInput(src.home_url, (v) => set('home_url', v),
      { placeholder: 'https://chatgpt.com/' });
    const siteRow = h('div', { class: 'row bridge-site-row' },
      field(t('bridge.siteUrl'), homeInput),
      h('button', {
        class: 'btn',
        onClick: async () => {
          try {
            await api.web2apiOpenSession(src.id, homeInput.value.trim() || src.home_url);
            toast(t('bridge.sessionOpen'), 'ok', 1600);
            await refreshSessions(false);
          } catch (e) {
            toast(errText(e), 'err', 4200);
          }
        },
      }, icon('external', 13), t('bridge.openSite')),
    );

    const driverSel = h('select', { class: 'mono-input' },
      h('option', { value: 'ui' }, t('bridge.driverUi')),
      h('option', { value: 'api' }, t('bridge.driverApi')),
    ) as HTMLSelectElement;
    driverSel.value = src.driver === 'api' ? 'api' : 'ui';
    driverSel.addEventListener('change', () => {
      set('driver', driverSel.value);
      syncPromptRow();
    });
    const driverRow = field(t('bridge.driver'), driverSel);
    driverRow.hidden = !browser;

    // A logged-in vendor page already keeps its own persona + conversation.
    // Default to typing only the newest user turn instead of re-injecting the
    // whole system prompt and transcript on every request.
    const promptSel = h('select', { class: 'mono-input' },
      h('option', { value: 'incremental' }, t('bridge.promptIncremental')),
      h('option', { value: 'full' }, t('bridge.promptFull')),
    ) as HTMLSelectElement;
    promptSel.value = src.prompt_mode === 'full' ? 'full' : 'incremental';
    promptSel.addEventListener('change', () => set('prompt_mode', promptSel.value));
    const promptRow = field(t('bridge.promptMode'), promptSel);
    const syncPromptRow = () => {
      promptRow.hidden = srcMode.value !== 'browser' || driverSel.value === 'api';
    };
    syncPromptRow();

    const cumulative = h('input', { type: 'checkbox' }) as HTMLInputElement;
    cumulative.checked = src.cumulative;
    cumulative.addEventListener('change', () => set('cumulative', cumulative.checked));

    const advanced = h('details', { class: 'bridge-adv' },
      h('summary', {}, t('bridge.advanced')),
      field(t('bridge.tplAuth'), textArea(src.auth_header, (v) => set('auth_header', v), 2)),
      field('Content-Type', textInput(src.content_type, (v) => set('content_type', v))),
      field(t('bridge.tplBody'), textArea(src.body_template, (v) => set('body_template', v), 5)),
      field(t('bridge.tplText'), textInput(src.text_path, (v) => set('text_path', v))),
      field(t('bridge.tplGuard'), textInput(src.text_guard, (v) => set('text_guard', v))),
      h('label', { class: 'bridge-check' }, cumulative, h('span', null, t('bridge.tplCumulative'))),
      field(t('bridge.tplOrg'), textInput(src.org_endpoint, (v) => set('org_endpoint', v))),
      field(t('bridge.tplOrg'), textInput(src.org_path, (v) => set('org_path', v))),
      field('Login probe URL', textInput(src.login_endpoint, (v) => set('login_endpoint', v),
        { placeholder: 'https://chatgpt.com/api/auth/session' })),
      field('UI driver script (empty = built-in)',
        textArea(src.ui_script, (v) => set('ui_script', v), 4)),
    );

    el.append(
      h('details', { class: 'bridge-src' },
        summary,
        field(t('bridge.srcLabel'), labelInput),
        field(t('bridge.srcMode'), srcMode),
        driverRow,
        promptRow,
        siteRow,
        sessionRow,
        field(t('bridge.org'), textInput(src.org_id, (v) => set('org_id', v))),
        field(t('bridge.srcModel'), textInput(src.model, (v) => set('model', v))),
        field(t('bridge.srcEndpoint'), textInput(src.endpoint, (v) => set('endpoint', v))),
        advanced,
      ),
    );
  });
}

/** Probe the real upstreams, not just the local listener. */
async function runTest() {
  if (!draft || !testBtn || !testEl) return;
  const sources = clone(draft).sources;
  testBtn.disabled = true;
  clear(testEl);
  testEl.hidden = false;
  testEl.append(h('div', { class: 'probe-row' }, t('bridge.testing')));
  try {
    const rows = await api.web2apiTest(sources);
    clear(testEl);
    for (const r of rows) {
      // Only the provider is shown. HTTP status / upstream diagnostics stay
      // one hover away, so a vendor's raw error blob never fills the panel.
      testEl.append(
        h('div', {
          class: `probe-row probe-compact ${r.ok ? 'ok' : 'bad'}`,
          title: `${r.status || '—'} ${r.detail}`,
        },
          h('span', { class: 'probe-mark' }, r.ok ? 'OK' : '×'),
          h('span', { class: 'probe-label probe-provider' }, r.label),
        ),
      );
    }
  } catch (e) {
    clear(testEl);
    testEl.append(h('div', { class: 'probe-row bad' }, errText(e)));
  } finally {
    testBtn.disabled = false;
  }
}

/* ---------------------------------------------------------------- status */

function startPolling() {
  if (poll !== null) return;
  poll = window.setInterval(() => {
    if (document.hidden) return;
    void refreshBridge();
    void refreshSessions(false);
  }, 3000);
}

function stopPolling() {
  if (poll === null) return;
  window.clearInterval(poll);
  poll = null;
}

/* ------------------------------------------------------------ full access */

/**
 * The panel's second yes.
 *
 * `enabled` (Settings → Advanced) only makes this panel reachable. The endpoint
 * answers with the user's logged-in browser session, so the step that actually
 * opens it is taken here, in front of the endpoint it opens — not on a settings
 * page where the consequence is abstract.
 */
function renderGate() {
  if (!gateEl) return;
  const armed = Boolean(app.config?.bridge.full_access);
  clear(gateEl);
  gateEl.hidden = armed;
  if (toggleBtn) {
    // A start button that throws a permission error is a worse answer than a
    // button that is visibly waiting for the confirmation above it.
    toggleBtn.disabled = !armed;
    toggleBtn.title = armed ? '' : t('bridge.gateNeed');
  }
  if (armed) return;
  gateEl.append(
    h('div', { class: 'bridge-gate-card' },
      h('div', { class: 'bridge-gate-title' },
        icon('lock', 13),
        h('span', null, t('bridge.gateTitle')),
      ),
      h('button', {
        class: 'btn primary',
        onClick: () => void confirmFullAccess(),
      }, icon('shield', 13), t('bridge.gateConfirm')),
    ),
  );
}

async function confirmFullAccess() {
  try {
    const cfg = await api.web2apiSetFullAccess(true);
    if (app.config) app.config.bridge = cfg;
    // The draft was cloned before this click; without this the next Save would
    // post `full_access: false` and quietly revoke what was just granted.
    if (draft) draft.full_access = true;
    emit('config:changed');
    renderGate();
    toast(t('bridge.gateDone'), 'ok', 2200);
  } catch (e) {
    toast(errText(e), 'err', 4200);
  }
}

function paint() {
  if (!status) return;
  if (dotEl) dotEl.classList.toggle('on', status.running);
  if (stateEl) {
    stateEl.textContent = status.running ? `${t('bridge.running')} · :${status.port}` : t('bridge.stopped');
    stateEl.classList.toggle('on', status.running);
  }
  if (toggleBtn) {
    clear(toggleBtn);
    toggleBtn.className = status.running ? 'btn' : 'btn primary';
    toggleBtn.append(icon(status.running ? 'stop' : 'plug', 12),
      h('span', null, status.running ? t('bridge.stop') : t('bridge.start')));
  }
  if (urlInput && !status.running) urlInput.value = `http://127.0.0.1:${draft?.port || status.port}/v1`;
  if (statsEl) {
    statsEl.textContent = status.running
      ? `${status.upstream} · ${status.default_source} · ${status.sources} src / ${status.routes} routes · ${status.requests} req / ${status.errors} err`
      : status.last_error || '';
    statsEl.classList.toggle('err-text', !status.running && Boolean(status.last_error));
  }
  paintLog();
}

function paintLog() {
  if (!logBody || !status) return;
  clear(logBody);
  if (!status.log.length) {
    logBody.append(h('div', { class: 'side-empty', style: { padding: '10px 2px', textAlign: 'left' } }, t('bridge.noLog')));
    return;
  }
  for (const row of status.log) {
    const time = new Date(row.at).toLocaleTimeString('zh-CN', { hour12: false });
    logBody.append(
      h('div', { class: `bridge-log-row${row.status >= 400 ? ' bad' : ''}` },
        h('span', { class: 't' }, time),
        h('span', { class: 'm' }, row.method),
        h('span', { class: 'p' }, row.path),
        h('span', { class: 's' }, `${row.status}`),
        h('span', { class: 'ms' }, `${row.ms}ms`),
        h('span', { class: 'd' }, row.detail),
      ),
    );
  }
}

export async function refreshBridge() {
  try {
    status = await api.web2apiStatus();
  } catch (e) {
    if (stateEl) stateEl.textContent = errText(e);
    return;
  }
  paint();
  emit('bridge:status', status.running);
}

async function toggleRun() {
  if (!toggleBtn) return;
  // The button is disabled while the gate is shut; this is the guard for the
  // paths that do not go through a click (a stale panel, a keyboard activation
  // racing the config) and it names the missing step instead of the raw error.
  if (!app.config?.bridge.full_access) {
    toast(t('bridge.gateNeed'), 'err', 3600);
    return;
  }
  toggleBtn.disabled = true;
  try {
    status = status?.running ? await api.web2apiStop() : await api.web2apiStart();
    // The backend owns the persisted switch on the built-in Web2API provider;
    // mirror it here so the composer picks up / drops those models right away.
    if (app.config) {
      const wp = app.config.providers.find((p) => p.id === 'web2api');
      if (wp) wp.enabled = status.running;
      emit('config:changed');
    }
    paint();
    void refreshSessions(false);
    if (status.running) toast(`${t('bridge.running')} · ${status.base_url}`, 'ok', 2600);
  } catch (e) {
    toast(errText(e), 'err', 4200);
    await refreshBridge();
  } finally {
    toggleBtn.disabled = false;
  }
}

async function apply() {
  if (!app.config || !draft) return;
  const port = Number.parseInt(portInput?.value || '', 10);
  draft.port = Number.isFinite(port) ? Math.min(65535, Math.max(1024, port)) : 8791;
  draft.api_key = (keyInput?.value || '').trim();
  draft.auto_start = Boolean(autoInput?.checked);
  draft.upstream = (modeSel?.value || 'web') as BridgeUpstream;
  draft.default_source = defaultSel?.value || draft.default_source;
  draft.model = (modelInput?.value || '').trim();
  draft.provider_id = providerSel?.value || '';
  draft.auto_ensure = autoEnsureInput ? autoEnsureInput.checked : true;
  // Not an editable field, and the draft may predate the confirmation: taking
  // the live value keeps a Save from revoking the gate by accident.
  draft.full_access = app.config.bridge.full_access;
  draft.enabled = app.config.bridge.enabled;

  const wasRunning = Boolean(status?.running);
  try {
    app.config = await api.saveConfig({ ...app.config, bridge: draft });
    // Port/key changes re-derive the built-in provider on the backend; refresh
    // the composer so its models follow the new endpoint.
    emit('config:changed');
    toast(t('bridge.saved'), 'ok', 1400);
  } catch (e) {
    toast(errText(e), 'err');
    return;
  }
  renderConfig();
  if (wasRunning) {
    // Settings only reach the server on start, so a running bridge restarts.
    try {
      await api.web2apiStop();
      status = await api.web2apiStart();
      paint();
    } catch (e) {
      toast(errText(e), 'err', 4200);
      await refreshBridge();
    }
  } else {
    await refreshBridge();
  }
}

function makeKey() {
  const bytes = new Uint8Array(18);
  crypto.getRandomValues(bytes);
  const hex = [...bytes].map((b) => b.toString(16).padStart(2, '0')).join('');
  return `sk-irixen-${hex}`;
}
