// Settings modal: providers, agent, appearance, MCP, about.

import { h, clear, fmtBytes } from '../core/dom';
import { icon } from './icons';
import { t, setLang, LANGS } from '../core/i18n';
import { api, errText, onUpdateProgress } from '../core/api';
import { app, emit, on } from '../core/state';
import { toast, confirmDialog, promptDialog, errorBox } from './toast';
import { applyTheme, customVarDefaults, parseCustomColors } from './theme';
import { debounce } from '../core/dom';
import { isLoopback } from '../core/providers';
import { ALL_EFFORTS, effortLabel } from '../core/effort';
import { openOnboarding } from './onboarding';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import type { AppConfig, AppUpdate, Effort, McpServerConfig, ProviderConfig, Protocol } from '../types';

type Tab = 'providers' | 'agent' | 'appearance' | 'mcp' | 'advanced' | 'about';

let overlay: HTMLElement | null = null;
let draft: AppConfig | null = null;
let tab: Tab = 'providers';
let bodyEl: HTMLElement | null = null;
let tabsEl: HTMLElement | null = null;
let titleEl: HTMLElement | null = null;
let closeBtn: HTMLButtonElement | null = null;
let unsubLang: (() => void) | null = null;

const persistSoon = debounce(() => void persist(), 500);

/** Vendor forms are collapsed by default: a dozen fully expanded forms made
 *  the providers tab impossible to scan. This survives list redraws. */
const expandedProviders = new Set<string>();

async function persist() {
  if (!draft) return;
  try {
    const saved = await api.saveConfig(draft);
    app.config = saved;
    // `saveConfig` re-derives the built-in Web2API provider. Copy it back in
    // place (not by replacing `draft`) so the inputs still bound to their
    // original objects don't write a stale port/key on the next keystroke.
    const builtin = saved.providers.find((p) => p.id === 'web2api');
    const local = draft?.providers.find((p) => p.id === 'web2api');
    if (builtin && local) Object.assign(local, builtin);
    applyTheme(app.config);
    setLang(app.config.language);
    emit('config:changed');
  } catch (e) {
    toast(errText(e), 'err');
  }
}

const clone = <T,>(v: T): T => JSON.parse(JSON.stringify(v));

export function openSettings(initial: Tab = 'providers') {
  if (!app.config) return;
  tab = initial;
  draft = clone(app.config);
  if (overlay) overlay.remove();
  // Only one modal may own a language listener at a time. Without this,
  // re-opening settings leaves the previous (detached) head subscribed.
  unsubLang?.();
  unsubLang = null;

  overlay = h('div', { class: 'modal', onClick: (e: MouseEvent) => {
    if (e.target === overlay) closeSettings();
  } });
  const card = h('div', { class: 'modal-card' });
  tabsEl = h('div', { class: 'modal-tabs' });
  bodyEl = h('div', { class: 'modal-body' });

  titleEl = h('span', { class: 'modal-title' }, t('settings.title'));
  closeBtn = h(
    'button',
    { class: 'icon-btn', style: { marginLeft: 'auto' }, title: t('common.close'), onClick: () => closeSettings() },
    icon('close', 14),
  ) as HTMLButtonElement;

  const head = h('div', { class: 'modal-head' }, titleEl, tabsEl, closeBtn);
  card.append(head, bodyEl);
  overlay.append(card);
  document.body.append(overlay);

  // The modal lives outside the shell DOM, so shell's `lang:changed` relabel
  // never reaches its head. Repaint head + tabs + body together on every
  // language change; a half-relabelled modal ("设置" with 繁體 tabs) is worse
  // than none.
  unsubLang = on('lang:changed', () => {
    if (!overlay) return;
    relabelHead();
    renderTabs();
    renderBody();
  });

  renderTabs();
  renderBody();
  document.addEventListener('keydown', onKey);
}

function relabelHead() {
  if (titleEl) titleEl.textContent = t('settings.title');
  if (closeBtn) closeBtn.title = t('common.close');
}

function onKey(e: KeyboardEvent) {
  if (e.key === 'Escape') closeSettings();
}

export function closeSettings() {
  if (overlay) overlay.remove();
  overlay = null;
  titleEl = null;
  closeBtn = null;
  unsubLang?.();
  unsubLang = null;
  document.removeEventListener('keydown', onKey);
  void persist();
}

function renderTabs() {
  if (!tabsEl) return;
  clear(tabsEl);
  const defs: { id: Tab; label: string; icon: any }[] = [
    { id: 'providers', label: t('settings.providers'), icon: 'cpu' },
    { id: 'agent', label: t('settings.agent'), icon: 'spark' },
    { id: 'appearance', label: t('settings.appearance'), icon: 'sun' },
    { id: 'mcp', label: t('settings.mcp'), icon: 'terminal' },
    { id: 'advanced', label: t('settings.advanced'), icon: 'settings' },
    { id: 'about', label: t('settings.about'), icon: 'info' },
  ];
  for (const d of defs) {
    tabsEl.append(
      h(
        'div',
        {
          class: `modal-tab${tab === d.id ? ' active' : ''}`,
          onClick: () => {
            tab = d.id;
            renderTabs();
            renderBody();
          },
        },
        d.label,
      ),
    );
  }
}

function renderBody() {
  if (!bodyEl || !draft) return;
  clear(bodyEl);
  if (tab === 'providers') bodyEl.append(providersTab());
  else if (tab === 'agent') bodyEl.append(agentTab());
  else if (tab === 'appearance') bodyEl.append(appearanceTab());
  else if (tab === 'mcp') bodyEl.append(mcpTab());
  else if (tab === 'advanced') bodyEl.append(advancedTab());
  else bodyEl.append(aboutTab());
}

/* ------------------------------------------------------------- primitives */

function field(label: string, control: HTMLElement): HTMLElement {
  return h('div', { class: 'field' }, h('label', { class: 'field-label' }, label), control);
}

function input(
  value: string,
  onInput: (v: string) => void,
  opts: { placeholder?: string; type?: string; mono?: boolean } = {},
): HTMLInputElement {
  const el = h('input', {
    value,
    placeholder: opts.placeholder || '',
    type: opts.type || 'text',
    style: opts.mono ? { fontFamily: 'var(--font-mono)', fontSize: '14px' } : undefined,
    onInput: (e: Event) => {
      onInput((e.target as HTMLInputElement).value);
      persistSoon();
    },
  }) as HTMLInputElement;
  return el;
}

function numInput(value: number, onChange: (v: number) => void, step = 1): HTMLInputElement {
  return input(String(value), (v) => {
    const n = Number(v);
    if (!Number.isNaN(n)) onChange(n);
  }, { type: 'number' });
}

function textarea(
  value: string,
  onInput: (v: string) => void,
  rows = 3,
  mono = true,
): HTMLTextAreaElement {
  const el = h('textarea', {
    value,
    rows,
    style: mono ? { fontFamily: 'var(--font-mono)', fontSize: '14px' } : undefined,
    onInput: (e: Event) => {
      onInput((e.target as HTMLTextAreaElement).value);
      persistSoon();
    },
  }) as HTMLTextAreaElement;
  return el;
}

function toggle(on: boolean, onChange: (v: boolean) => void): HTMLElement {
  const el = h('div', { class: `switch${on ? ' on' : ''}` });
  el.addEventListener('click', () => {
    const next = !el.classList.contains('on');
    el.classList.toggle('on', next);
    onChange(next);
    void persist();
  });
  return el;
}

function select(
  options: { value: string; label: string }[],
  value: string,
  onChange: (v: string) => void,
): HTMLSelectElement {
  const el = h('select', {
    onChange: (e: Event) => {
      onChange((e.target as HTMLSelectElement).value);
      void persist();
    },
  }) as HTMLSelectElement;
  for (const o of options) {
    const opt = h('option', { value: o.value }, o.label) as HTMLOptionElement;
    if (o.value === value) opt.selected = true;
    el.append(opt);
  }
  return el;
}

/* --------------------------------------------------------------- providers */

function providersTab(): HTMLElement {
  const wrap = h('div', { class: 'col', style: { display: 'flex', flexDirection: 'column', gap: '10px' } });
  const list = h('div', { class: 'col', style: { display: 'flex', flexDirection: 'column', gap: '10px' } });

  const addBtn = h(
    'button',
    {
      class: 'btn',
      onClick: () => void addProvider(),
    },
    icon('plus', 13),
    t('settings.addProvider'),
  );

  const draw = () => {
    clear(list);
    if (!draft) return;
    for (const p of draft.providers) list.append(providerCard(p, draw));
  };

  wrap.append(
    netSection(),
    addBtn,
    h('div', { style: { height: '12px' } }),
    list,
  );
  draw();
  return wrap;
}

function providerCard(p: ProviderConfig, redraw: () => void): HTMLElement {
  const builtin = p.id === 'web2api';
  const open = expandedProviders.has(p.id);
  const card = h('div', { class: `provider-card${open ? '' : ' collapsed'}` });
  const flip = () => {
    if (open) expandedProviders.delete(p.id);
    else expandedProviders.add(p.id);
    redraw();
  };

  // A provider that is switched on but has no key cannot send anything; say so
  // on the card instead of letting the first message fail with a 401.
  const keyBadge = h('span', { class: 'provider-badge' });
  const syncBadge = () => {
    const missing = !p.api_key.trim() && !isLoopback(p.base_url);
    keyBadge.textContent = missing ? t('settings.noKey') : '';
    keyBadge.style.display = missing ? '' : 'none';
  };

  const foldBtn = h(
    'button',
    {
      class: `provider-fold${open ? ' is-open' : ''}`,
      title: open ? t('settings.collapse') : t('settings.expand'),
      onClick: (e: MouseEvent) => {
        e.stopPropagation();
        flip();
      },
    },
    icon('chevron', 12),
  );

  // Testing must stay reachable while the card is collapsed — that was the
  // whole point of folding the vendor list in the first place.
  const headTestBtn = h(
    'button',
    {
      class: 'icon-btn',
      title: t('settings.test'),
      onClick: (e: MouseEvent) => {
        e.stopPropagation();
        void runTest(headTestBtn as HTMLButtonElement);
      },
    },
    icon('zap', 13),
  ) as HTMLButtonElement;

  const head = h(
    'div',
    { class: 'provider-head' },
    foldBtn,
    headTestBtn,
    h('span', { class: 'provider-name' }, p.name || p.id),
    h('span', { class: 'provider-id' }, p.id),
    h('span', { class: 'provider-id', style: { marginLeft: 'auto' } }, p.protocol),
    keyBadge,
    builtin ? h('span', { class: 'provider-badge auto' }, 'AUTO') : null,
    toggle(p.enabled, (v) => (p.enabled = v)),
  );
  // Clicking the row opens it; the switch itself must keep its own behaviour.
  head.style.cursor = 'pointer';
  head.addEventListener('click', (e) => {
    if ((e.target as HTMLElement).closest('.switch')) return;
    flip();
  });

  const name = input(p.name, (v) => (p.name = v), { placeholder: 'DeepSeek' });
  const baseUrl = input(p.base_url, (v) => {
    p.base_url = v;
    syncBadge();
  }, {
    placeholder: 'https://api.openai.com/v1',
    mono: true,
  });
  const apiKey = input(p.api_key, (v) => {
    p.api_key = v;
    syncBadge();
  }, {
    placeholder: 'sk-…',
    type: 'password',
    mono: true,
  });
  syncBadge();
  if (builtin) {
    // Derived from the Web2API panel: read-only here, otherwise the next
    // config save would silently overwrite whatever the user typed.
    for (const el of [name, baseUrl, apiKey]) el.readOnly = true;
  }
  const modelsPath = input(p.models_path, (v) => (p.models_path = v), {
    placeholder: '/models',
    mono: true,
  });
  const protocol = select(
    [
      { value: 'openai', label: 'OpenAI chat/completions' },
      { value: 'anthropic', label: 'Anthropic messages' },
      { value: 'gemini', label: 'Gemini generateContent' },
    ],
    p.protocol,
    (v) => {
      p.protocol = v as Protocol;
      if (v === 'openai' && !p.models_path) p.models_path = '/models';
    },
  );
  if (builtin) {
    protocol.disabled = true;
    modelsPath.readOnly = true;
  }

  const status = h('span', { class: 'hint' });
  const report = h('div', { class: 'probe' });

  async function runTest(btn: HTMLButtonElement) {
    btn.disabled = true;
    clear(status);
    clear(report);
    status.className = 'hint';
    status.append(t('settings.testing'));
    try {
      await persist();
      const r = await api.providerProbe(p.id);
      clear(status);
      if (builtin) {
        // The auto-configured Web2API card only needs to show which provider
        // was tested; a six-step URL/proxy report is noise on a loopback endpoint.
        report.append(h('div', {
          class: `probe-row probe-compact ${r.ok ? 'ok' : 'bad'}`,
          title: r.error || '',
        },
          h('span', { class: 'probe-mark' }, r.ok ? 'OK' : '×'),
          h('span', { class: 'probe-label probe-provider' }, p.name || p.id),
        ));
      } else {
        report.append(...probeRows(r));
      }
      if (!r.ok) {
        status.className = 'hint err-text';
        status.append(r.error || t('settings.testFail'));
      } else {
        status.className = 'hint ok-text';
        status.append(t('settings.testOk', { n: r.models }));
        try {
          const list = await api.modelsList(p.id, true);
          // Only the provider the composer is pointed at owns `app.models`;
          // warming any other one would put its models under a different
          // provider's name in the picker.
          if (p.id === app.providerId) {
            app.models = list;
            emit('models:changed');
          }
        } catch (err) {
          // The endpoint answered — failing to hydrate the picker is a
          // different problem and must not be reported as a failed test.
          status.className = 'hint warn-text';
          status.append(` · ${t('settings.modelsLoadFail')}: ${errText(err)}`);
        }
      }
    } catch (err) {
      clear(status);
      status.className = 'hint err-text';
      status.append(`${t('settings.testFail')}: ${errText(err)}`);
    } finally {
      btn.disabled = false;
    }
  }

  const testBtn = h(
    'button',
    { class: 'btn', onClick: (e: Event) => void runTest(e.currentTarget as HTMLButtonElement) },
    icon('zap', 13),
    t('settings.test'),
  );

  const delBtn = h(
    'button',
    {
      class: 'btn',
      onClick: async () => {
        if (!draft) return;
        if (!(await confirmDialog(t('settings.delete'), p.name || p.id))) return;
        draft.providers = draft.providers.filter((x) => x.id !== p.id);
        await persist();
        redraw();
      },
    },
    icon('trash', 13),
    t('settings.delete'),
  );

  card.append(head);
  if (open) {
    card.append(
      h('div', { class: 'field-row-3' },
        field(t('settings.name'), name),
        field(t('settings.protocol'), protocol),
        field(t('settings.modelsPath'), modelsPath),
      ),
      field(t('settings.baseUrl'), baseUrl),
      field(t('settings.apiKey'), apiKey),
      h('div', { class: 'row', style: { marginTop: '8px' } }, testBtn, builtin ? null : delBtn),
    );
  }
  // Outside the fold so the head's test button can report even when collapsed.
  card.append(h('div', { class: 'row', style: { marginTop: '8px' } }, status), report);
  return card;
}

async function addProvider() {
  if (!draft) return;
  const name = await promptDialog(t('settings.addProvider'), 'Provider name', '');
  if (!name) return;
  const id = name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '') || `p${Date.now()}`;
  draft.providers.push({
    id: draft.providers.some((p) => p.id === id) ? `${id}-${Date.now().toString(36).slice(-4)}` : id,
    name,
    protocol: 'openai',
    base_url: '',
    api_key: '',
    enabled: true,
    models_path: '/models',
    extra_headers: [],
    extra_body: {},
    note: '',
  });
  await persist();
  openSettings('providers');
}

/* ------------------------------------------------------------------- agent */

function agentTab(): HTMLElement {
  if (!draft) return h('div');
  const a = draft.agent;
  const c = a.compression;
  const wrap = h('div');

  wrap.append(
    h('h3', null, t('settings.agent')),
    h('div', { class: 'field-row-3' },
      field(t('settings.maxIterations'), numInput(a.max_iterations, (v) => (a.max_iterations = Math.max(1, v)))),
      field(t('settings.temperature'), input(a.temperature === null ? '' : String(a.temperature), (v) => {
        a.temperature = v.trim() === '' ? null : Number(v);
      }, { placeholder: '0.7' })),
      field(t('settings.language'), select(
        LANGS.map((l) => ({ value: l.id, label: l.label })),
        draft.language,
        (v) => (draft!.language = v),
      )),
    ),
    h('div', { class: 'field-row' },
      field(t('settings.effortDefault'), select(
        ALL_EFFORTS.map((e) => ({ value: e, label: effortLabel(e) })),
        draft.effort || 'auto',
        (v) => (draft!.effort = v as Effort),
      )),
    ),
    field(t('settings.systemPrompt'), textarea(a.system_prompt, (v) => (a.system_prompt = v), 7)),
    h('div', { class: 'row', style: { margin: '10px 0 14px' } },
      toggle(a.auto_skills, (v) => (a.auto_skills = v)),
      h('span', { class: 'hint' }, t('settings.autoSkills')),
    ),
    h('div', { class: 'row', style: { margin: '0 0 14px' } },
      toggle(Boolean(draft?.editor_format_on_save), (v) => {
        if (draft) draft.editor_format_on_save = v;
      }),
      h('span', { class: 'hint' }, t('settings.formatOnSave')),
    ),
    h('h3', null, t('settings.compression')),
    h('div', { class: 'row', style: { marginBottom: '10px' } },
      toggle(c.enabled, (v) => (c.enabled = v)),
      h('span', { class: 'hint' }, t('settings.compressionEnabled')),
    ),
    h('div', { class: 'field-row-3' },
      field(t('settings.maxContext'), numInput(c.max_context_tokens, (v) => (c.max_context_tokens = v), 1000)),
      field(t('settings.threshold'), input(String(c.threshold), (v) => {
        const n = Number(v);
        if (!Number.isNaN(n)) c.threshold = Math.min(0.98, Math.max(0.3, n));
      })),
      field(t('settings.keepRecent'), numInput(c.keep_recent, (v) => (c.keep_recent = Math.max(2, v)))),
    ),
    h('div', { class: 'field-row-3' },
      field(t('settings.toolAge'), numInput(c.tool_result_age, (v) => (c.tool_result_age = Math.max(1, v)))),
      field(t('settings.toolResultCap'), numInput(c.tool_result_cap, (v) => (c.tool_result_cap = Math.max(500, v)), 500)),
      field(t('settings.digestTokens'), numInput(c.digest_max_tokens, (v) => (c.digest_max_tokens = Math.max(200, v)), 100)),
    ),
  );
  return wrap;
}

/* -------------------------------------------------------------- appearance */

function appearanceTab(): HTMLElement {
  if (!draft) return h('div');
  const wrap = h('div', { class: 'appearance-tab' });

  // One section per concern. The old flat stack of h3 + spacer divs glued the
  // terminal and extension blocks together and left half the modal empty.
  const section = (title: string, ...content: (Node | string | null)[]): HTMLElement =>
    h('section', { class: 'appearance-section' }, h('h3', null, title), ...content);

  const themes: { id: string; name: string; swatch: string[] }[] = [
    { id: 'light', name: t('settings.themeLight'), swatch: ['#f8f9fb', '#d3d9e2', '#0369a1'] },
    { id: 'dark', name: t('settings.themeDark'), swatch: ['#0e121a', '#141a24', '#38bdf8'] },
  ];
  for (const ct of draft.custom_themes) {
    themes.push({
      id: `custom:${ct.id}`,
      name: ct.name,
      swatch: [
        ct.colors.find(([k]) => k === '--bg-1')?.[1] || '#111',
        ct.colors.find(([k]) => k === '--bg-2')?.[1] || '#222',
        ct.colors.find(([k]) => k === '--accent')?.[1] || '#38bdf8',
      ],
    });
  }

  const grid = h('div', { class: 'theme-grid' });
  for (const th of themes) {
    const swatch = h('div', { class: 'theme-swatch' }, ...th.swatch.map((c) => h('i', { style: { background: c } })));
    const card = h(
      'div',
      {
        class: `theme-card${draft.theme === th.id ? ' active' : ''}`,
        onClick: () => {
          draft!.theme = th.id;
          applyTheme(draft);
          void persist();
          renderBody();
        },
      },
      swatch,
      h('div', { class: 'theme-name' }, th.name),
    );
    grid.append(card);
  }

  const addCustom = h(
    'button',
    {
      class: 'btn appearance-add',
      onClick: async () => {
        const name = await promptDialog(t('settings.themeCustom'), t('settings.name'), 'My Theme');
        if (!name || !draft) return;
        const id = `t${Date.now().toString(36).slice(-5)}`;
        draft.custom_themes.push({
          id,
          name,
          colors: parseCustomColors(customVarDefaults()),
        });
        draft.theme = `custom:${id}`;
        await persist();
        applyTheme(draft);
        renderBody();
      },
    },
    icon('plus', 14),
    t('settings.themeCustom'),
  );

  wrap.append(section(t('settings.theme'), grid, addCustom));

  const current = draft.custom_themes.find((c) => `custom:${c.id}` === draft!.theme);
  if (current) {
    const colorsText = current.colors.map(([k, v]) => `${k}: ${v}`).join('\n');
    const accent = current.colors.find(([k]) => k === '--accent')?.[1] || '#38bdf8';
    wrap.append(
      section(
        current.name,
        h('div', { class: 'field-row' },
          field(t('settings.name'), input(current.name, (v) => (current.name = v))),
          field(t('settings.accent'), input(accent, (v) => {
            const idx = current.colors.findIndex(([k]) => k === '--accent');
            if (idx >= 0) current.colors[idx] = ['--accent', v];
            else current.colors.push(['--accent', v]);
            applyTheme(draft);
          }, { type: 'color' })),
        ),
        field(
          t('settings.customColors'),
          textarea(colorsText, (v) => {
            current.colors = parseCustomColors(v);
            applyTheme(draft);
          }, 12),
        ),
        h('div', { class: 'row' },
          h('button', {
            class: 'btn',
            onClick: () => {
              if (!draft) return;
              applyTheme(draft);
              toast(t('settings.saved'), 'ok', 1200);
            },
          }, icon('check', 14), t('settings.save')),
          h('button', {
            class: 'btn',
            onClick: async () => {
              if (!draft) return;
              if (!(await confirmDialog(t('settings.delete'), current.name))) return;
              draft.custom_themes = draft.custom_themes.filter((x) => x.id !== current.id);
              draft.theme = 'light';
              await persist();
              applyTheme(draft);
              renderBody();
            },
          }, icon('trash', 14), t('settings.delete')),
        ),
      ),
    );
  }

  wrap.append(
    section(
      t('settings.language'),
      h('div', { class: 'row wrap lang-row' },
        ...LANGS.map((l) =>
          h('button', {
            class: `chip lang-chip${draft!.language === l.id ? ' on' : ''}`,
            onClick: () => {
              draft!.language = l.id;
              // `lang:changed` relabels the shell and, through the listener
              // registered in openSettings, the modal head/tabs/body as well.
              setLang(l.id);
              void persist();
            },
          }, l.label),
        ),
      ),
    ),
  );

  // Terminal and extension startup are both short: two columns use the wide
  // modal properly instead of stacking them into a cramped top-left column.
  wrap.append(
    h('div', { class: 'appearance-split' }, terminalSection(), extensionsSection()),
  );
  return wrap;
}

/**
 * Extension startup policy.
 *
 * An extension that never activates contributes nothing but its manifest — no
 * providers, no commands, no listeners. VS Code can afford lazy activation
 * because everything else in it is native; here the Node host *is* the runtime,
 * so the default is to start them all and this switch is the way back.
 */
function extensionsSection(): HTMLElement {
  const box = h('div');
  box.append(h('h3', null, t('settings.extensions')));
  const cb = h('input', { type: 'checkbox' }) as HTMLInputElement;
  cb.checked = draft?.extensions.auto_activate_all !== false;
  cb.onchange = () => {
    if (!draft) return;
    draft.extensions.auto_activate_all = cb.checked;
    void persist();
  };
  box.append(
    h(
      'label',
      { class: 'row', style: { gap: '8px', alignItems: 'center', fontSize: '14px', cursor: 'pointer' } },
      cb,
      h('span', null, t('settings.extAutoActivate')),
    ),
    h('div', { class: 'dim', style: { fontSize: '13px', marginTop: '4px' } }, t('settings.extAutoActivateHint')),
  );
  return box;
}

/**
 * Which shell the integrated terminal starts.
 *
 * The list is discovered from the machine (PowerShell 7, Windows PowerShell,
 * cmd, Git Bash, WSL) rather than hard-coded, so a shell that is not installed
 * shows as unavailable instead of failing at launch.
 */
function terminalSection(): HTMLElement {
  const box = h('div');
  box.append(h('h3', null, t('settings.terminal')));
  const slot = h('div', { class: 'dim', style: { fontSize: '13.5px' } }, t('common.loading'));
  box.append(slot);

  void api.termShells().then((rows) => {
    slot.remove();
    if (!rows.length) {
      box.append(h('div', { class: 'dim', style: { fontSize: '13.5px' } }, t('terminal.noShell')));
      return;
    }
    const current =
      draft?.terminal_shell ||
      rows.find((r) => r.recommended && r.available)?.id ||
      rows[0].id;
    box.append(
      field(
        t('settings.terminalShell'),
        select(
          rows.map((r) => ({
            value: r.id,
            label: r.available ? r.label : `${r.label} — ${t('terminal.missing')}`,
          })),
          current,
          (v) => {
            if (draft) draft.terminal_shell = v;
          },
        ),
      ),
      h(
        'div',
        { class: 'dim', style: { fontSize: '13px', marginTop: '4px' } },
        t('settings.terminalHint'),
      ),
    );
  });
  return box;
}

/* --------------------------------------------------------------------- mcp */

function mcpTab(): HTMLElement {
  const wrap = h('div');
  const list = h('div');

  const draw = () => {
    clear(list);
    if (!app.mcp.length) {
      list.append(h('div', { class: 'side-empty' }, t('settings.mcpEmpty')));
      return;
    }
    for (const v of app.mcp) list.append(mcpCard(v));
  };

  wrap.append(
    h(
      'div',
      { class: 'row', style: { marginBottom: '12px' } },
      h('button', { class: 'btn', onClick: () => void addMcp(draw) }, icon('plus', 13), t('settings.mcpAdd')),
      h('button', {
        class: 'btn',
        onClick: async () => {
          try {
            app.mcp = await api.mcpServers();
            draw();
          } catch (e) {
            toast(errText(e), 'err');
          }
        },
      }, icon('refresh', 13), t('common.refresh')),
    ),
    list,
  );
  draw();
  return wrap;
}

function mcpCard(v: { config: McpServerConfig; running: boolean; tools: number; stderr: string[] }): HTMLElement {
  const c = v.config;
  const status = h('span', { class: 'chip' }, v.running ? t('settings.running') : t('settings.stopped'));
  const toolCount = h('span', { class: 'hint', style: { marginLeft: 'auto' } }, `${v.tools} ${t('settings.tools')}`);

  const toggleBtn = h(
    'button',
    {
      class: 'btn',
      onClick: async (e: Event) => {
        const btn = e.currentTarget as HTMLButtonElement;
        btn.disabled = true;
        try {
          if (v.running) await api.mcpStop(c.id);
          else await api.mcpStart(c.id);
          app.mcp = await api.mcpServers();
          renderBody();
        } catch (err) {
          toast(errText(err), 'err');
        } finally {
          btn.disabled = false;
        }
      },
    },
    icon(v.running ? 'stop' : 'zap', 13),
    v.running ? t('settings.stop') : t('settings.start'),
  );

  const delBtn = h(
    'button',
    {
      class: 'btn',
      onClick: async () => {
        if (!(await confirmDialog(t('settings.delete'), c.name))) return;
        try {
          await api.mcpDelete(c.id);
          if (draft) draft.mcp_servers = draft.mcp_servers.filter((s) => s.id !== c.id);
          app.mcp = await api.mcpServers();
          renderBody();
        } catch (e) {
          toast(errText(e), 'err');
        }
      },
    },
    icon('trash', 13),
  );

  const card = h(
    'div',
    { class: 'mcp-card' },
    h('div', { class: 'row', style: { marginBottom: '6px' } },
      h('strong', null, c.name),
      status,
      toolCount,
      toggleBtn,
      delBtn,
    ),
    h('div', { class: 'hint', style: { fontFamily: 'var(--font-mono)' } },
      `${c.command} ${c.args.join(' ')}`),
  );

  if (v.stderr.length) {
    card.append(
      h('div', {
        class: 'diff-view',
        style: { maxHeight: '90px', marginTop: '6px' },
        textContent: v.stderr.slice(-12).join('\n'),
      }),
    );
  }
  return card;
}

async function addMcp(redraw: () => void) {
  if (!draft) return;
  const name = await promptDialog(t('settings.mcpAdd'), t('settings.mcpName'), '');
  if (!name) return;
  const command = await promptDialog(t('settings.mcpCommand'), 'npx', '');
  if (!command) return;
  const argsRaw = await promptDialog(t('settings.mcpArgs'), '-y @modelcontextprotocol/server-filesystem ./', '');
  const server: McpServerConfig = {
    id: `mcp-${Date.now().toString(36)}`,
    name,
    command,
    args: (argsRaw || '').split(/\s+/).filter(Boolean),
    env: [],
    enabled: true,
    auto_start: true,
  };
  try {
    await api.mcpUpsert(server);
    draft.mcp_servers.push(server);
    await persist();
    app.mcp = await api.mcpServers();
    redraw();
    renderBody();
  } catch (e) {
    toast(errText(e), 'err');
  }
}

/* ------------------------------------------------------------------- about */

/* ---------------------------------------------------------------- advanced */

/**
 * Everything that is not a setting so much as a decision.
 *
 * The web bridge (Web2API) is switched on here because it reverse-proxies a
 * vendor's own web UI: running it can put an account at risk with that vendor,
 * and nothing about that belongs in a default install. This page owns the
 * switch only. Once it is on the panel appears in the right rail, and the step
 * that actually lets the endpoint answer — full access — is confirmed there, a
 * second yes that the switch does not grant.
 */
function advancedTab(): HTMLElement {
  if (!draft) return h('div');
  const b = draft.bridge;
  const wrap = h('div', { class: 'col', style: { display: 'flex', flexDirection: 'column', gap: '12px' } });

  const sw = h('div', { class: `switch${b.enabled ? ' on' : ''}` });
  sw.addEventListener('click', () => {
    if (!draft) return;
    const next = !sw.classList.contains('on');
    sw.classList.toggle('on', next);
    draft.bridge.enabled = next;
    void applyMasterSwitch(next);
  });

  const head = h('div', { class: 'adv-row' },
    h('div', { class: 'adv-text' },
      h('div', { class: 'adv-title' }, t('adv.bridge')),
      h('div', { class: 'hint' }, t('adv.bridgeHint')),
    ),
    sw,
  );

  wrap.append(
    h('h3', null, t('settings.advanced')),
    h('div', { class: 'adv-card' }, head,
      h('div', { class: 'adv-warn' }, t('adv.bridgeWarn')),
      b.enabled
        ? h('div', { class: 'adv-on' },
            h('div', { class: 'hint' }, t('adv.bridgeOn')),
            h('button', {
              class: 'btn',
              onClick: () => {
                // The panel is a dock surface now, so getting there means
                // leaving the modal first — the rail button it mirrors is
                // behind it.
                closeSettings();
                emit('surface:open', 'bridge');
              },
            }, icon('plug', 13), t('adv.bridgeOpen')),
          )
        : h('div', { class: 'hint', style: { marginTop: '8px' } }, t('adv.bridgeOff')),
    ),
  );

  wrap.append(addonsCard(), windowCard());
  return wrap;
}

/** Where user skills and extensions live.
 *
 *  Default is the folder the app was installed into; an empty field means
 *  "default" (the placeholder shows what that resolves to). The write happens
 *  on blur or Enter, never per keystroke: a debounced save of a half-typed
 *  `E:\IRI` would create that folder and move the plugin list into it.
 */
function addonsCard(): HTMLElement {
  if (!draft) return h('div');
  const def = app.info?.addons_default || '';

  const el = h('input', {
    class: 'adv-path',
    value: draft.addons_root || '',
    placeholder: def,
    spellcheck: 'false',
  }) as HTMLInputElement;

  const commit = async () => {
    if (!draft) return;
    const next = el.value.trim();
    if (next === (draft.addons_root || '')) return;
    draft.addons_root = next;
    await persist();
    // `persist` leaves `app.config` alone when the backend rejects the path
    // (no write permission, for instance), so reading it back rolls the field
    // to whatever is actually in effect.
    draft.addons_root = app.config?.addons_root ?? '';
    el.value = draft.addons_root;
    renderBody();
  };

  el.addEventListener('blur', () => void commit());
  el.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') el.blur();
    if (e.key === 'Escape') {
      el.value = draft?.addons_root || '';
      el.blur();
    }
  });

  const browse = h('button', {
    class: 'btn',
    title: t('adv.addonsPick'),
    onClick: async () => {
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: t('adv.addonsPick'),
        defaultPath: draft?.addons_root || def || undefined,
      });
      if (typeof picked === 'string' && picked) {
        el.value = picked;
        await commit();
      }
    },
  }, icon('folderOpen', 13), t('adv.addonsBrowse'));

  const reset = h('button', {
    class: 'btn',
    title: t('adv.addonsResetHint'),
    onClick: async () => {
      el.value = '';
      await commit();
    },
  }, t('adv.addonsReset'));

  const open = h('button', {
    class: 'btn',
    title: t('adv.addonsOpen'),
    onClick: async () => {
      try {
        await api.openAddonsFolder();
      } catch (e) {
        toast(errText(e), 'err', 4200);
      }
    },
  }, icon('folderOpen', 13), t('adv.addonsOpen'));

  const effective = draft.addons_root || def || '—';

  return h('div', { class: 'adv-card' },
    h('div', { class: 'adv-row' },
      h('div', { class: 'adv-text' },
        h('div', { class: 'adv-title' }, t('adv.addons')),
        h('div', { class: 'hint' }, t('adv.addonsHint')),
      ),
      open,
    ),
    h('div', { class: 'adv-path-row' }, el, browse, reset),
    h('div', { class: 'hint', style: { marginTop: '6px' } },
      `${t('adv.addonsCurrent')} ${effective}`),
    h('div', { class: 'hint' },
      `${t('adv.addonsDefault')} ${def || '—'}`),
    h('div', { class: 'hint' }, t('adv.addonsRestart')),
  );
}

/** What the titlebar's X does.
 *
 *  The tray icon exists either way, so this is not "enable the tray" — it is
 *  whether closing the window also ends the process. Hiding is the default
 *  because an agent turn, a build in the terminal and a Node extension host all
 *  live in this process; a close button that silently kills them is a data loss
 *  waiting to happen. The tray menu always offers a real quit, so choosing
 *  "keep running" can never strand the app.
 */
function windowCard(): HTMLElement {
  if (!draft) return h('div');
  const d = draft;
  const card = h('div', { class: 'adv-card' });

  const row = (toTray: boolean, title: string, hint: string) => {
    const on = Boolean(d.close_to_tray) === toTray;
    return h('button', {
      class: `adv-opt${on ? ' on' : ''}`,
      'aria-pressed': on ? 'true' : 'false',
      onClick: async () => {
        if (Boolean(d.close_to_tray) === toTray) return;
        d.close_to_tray = toTray;
        // Answering here is the same answer as the first-close card, so the
        // window must not ask again just because the choice was made on this
        // page instead of the titlebar.
        d.close_mode_asked = true;
        // The close handler reads the live config, so this takes effect on the
        // very next click on the X — no restart.
        await persist();
        draw();
      },
    },
    h('div', { class: 'adv-text' },
      h('div', { class: 'adv-title' }, title),
      h('div', { class: 'hint' }, hint),
    ),
    on ? icon('check', 14) : null);
  };

  const draw = () => {
    clear(card);
    card.append(
      h('div', { class: 'adv-text' },
        h('div', { class: 'adv-title' }, t('adv.window')),
        h('div', { class: 'hint' }, t('adv.windowHint')),
      ),
      h('div', { class: 'adv-opts' },
        row(true, t('adv.windowTray'), t('adv.windowTrayHint')),
        row(false, t('adv.windowQuit'), t('adv.windowQuitHint')),
      ),
    );
  };
  draw();
  return card;
}

/** Persist the master switch, then take the listener down if it just closed. */
async function applyMasterSwitch(on: boolean) {
  // Switching off revokes the panel's confirmation on the backend (the two
  // gates cannot be inherited in that order), so the draft has to forget it
  // too — otherwise the fields would show a grant the config no longer holds.
  if (!on && draft) draft.bridge.full_access = false;
  await persist();
  if (!on) {
    // `web2api_stop` is a no-op when it was never started; the call is here so
    // closing the switch actually closes the port instead of leaving a socket
    // that no panel can reach any more.
    try {
      await api.web2apiStop();
    } catch {
      /* not running — nothing to stop */
    }
  }
  renderBody();
}

/** Survives tab switches and language changes so reopening About does not
 *  fire another GitHub request. Only an explicit button press re-checks. */
const updateState: { loading: boolean; data: AppUpdate | null; error: string } = {
  loading: false,
  data: null,
  error: '',
};

function aboutTab(): HTMLElement {
  const info = app.info;
  const kv = h('dl', { class: 'kv' });
  const rows: [string, string][] = [
    [t('settings.version'), info?.version || '—'],
    [t('settings.platform'), info?.platform || '—'],
    [t('settings.configDir'), info?.config_dir || '—'],
    [t('app.workspace'), app.config?.workspace || '—'],
    [t('settings.license'), 'Apache License 2.0'],
    [t('settings.repository'), 'github.com/lanche-furry/IRIXEN-Code'],
    [t('chat.context'), `${fmtBytes(0)} / ${app.contextTokens} tokens`],
  ];
  for (const [k, vRaw] of rows) {
    kv.append(h('dt', null, k), h('dd', null, vRaw));
  }

  // Versions are managed on GitHub Releases: the app only reads the newest
  // release and hands the user off to the download page. It never replaces
  // its own exe while running.
  const updateBox = h('div', { class: 'about-update' });
  renderUpdateBox(updateBox);

  return h('div', null, h('h3', null, t('app.name')), kv, updateBox, maintenanceBox());
}

/** Recovery hatch for a profile that was never really set up: re-run the wizard
 *  without touching anything the user typed, or wipe the config back to factory
 *  defaults (the previous file is kept as `config.json.bak-<epoch>`). */
function maintenanceBox(): HTMLElement {
  const rerun = h('button', {
    class: 'btn',
    title: t('onboard.reopen'),
    onClick: () => openOnboarding(),
  }, icon('spark', 12), t('onboard.reopen'));

  const reset = h('button', {
    class: 'btn danger',
    title: t('about.resetProfile'),
    onClick: async () => {
      if (!(await confirmDialog(t('about.resetProfile'), t('about.resetConfirm')))) return;
      try {
        await api.configReset();
      } catch (e) {
        toast(errText(e), 'err', 4200);
        return;
      }
      // A full reload is the only honest way back to a first-run window: every
      // panel, session and provider picker was drawn from the old config.
      location.reload();
    },
  }, icon('trash', 12), t('about.resetProfile'));

  return h('div', { class: 'about-maint' },
    h('h3', null, t('about.maintenance')),
    h('div', { class: 'row wrap' }, rerun, reset),
    h('p', { class: 'about-maint-hint' }, t('about.resetHint')),
  );
}

async function runUpdateCheck(box: HTMLElement) {
  if (updateState.loading) return;
  updateState.loading = true;
  updateState.error = '';
  renderUpdateBox(box);
  try {
    updateState.data = await api.appCheckUpdate(true);
  } catch (e) {
    updateState.data = null;
    updateState.error = errText(e);
  } finally {
    updateState.loading = false;
  }
  if (box.isConnected) renderUpdateBox(box);
}

function openLink(url: string) {
  if (!url) return;
  void api.openExternal(url).catch((e) => toast(errText(e), 'err'));
}

function renderUpdateBox(box: HTMLElement) {
  clear(box);
  const d = updateState.data;
  const checkBtn = h('button', {
    class: 'btn primary',
    title: t('about.checkUpdate'),
    onClick: () => void runUpdateCheck(box),
  }, icon('refresh', 12), t('about.checkUpdate')) as HTMLButtonElement;
  if (updateState.loading) checkBtn.disabled = true;

  box.append(
    h('div', { class: 'about-update-bar' },
      h('span', { class: 'about-update-current' }, app.info?.version || 'Beta 0.1'),
      h('span', { class: 'about-update-semver' }, app.info?.semver || ''),
      h('div', { class: 'spacer', style: { flex: '1' } }),
      checkBtn,
      h('button', {
        class: 'btn',
        title: t('about.openReleases'),
        onClick: () => openLink(d?.url || d?.releases_url || 'https://github.com/lanche-furry/IRIXEN-Code/releases'),
      }, icon('external', 12), t('about.openReleases')),
    ),
  );

  if (updateState.loading) {
    box.append(h('div', { class: 'about-update-msg' }, icon('refresh', 12), t('about.checking')));
    return;
  }
  if (updateState.error) {
    box.append(errorBox(t('about.checkUpdate'), updateState.error, () => void runUpdateCheck(box)));
    return;
  }
  if (!d) return;

  const when = d.checked_at
    ? new Date(d.checked_at * 1000).toLocaleString()
    : '';

  if (d.no_release) {
    box.append(h('div', { class: 'about-update-card' },
      h('span', null, t('about.noRelease')),
      h('span', { class: 'about-update-time' }, when)));
    return;
  }

  if (!d.has_update) {
    box.append(h('div', { class: 'about-update-card ok' },
      icon('check', 13),
      h('span', null, t('about.upToDate')),
      h('span', { class: 'about-update-time' }, `${d.latest || d.latest_semver} · ${when}`)));
    return;
  }

  const assets = d.assets.length
    ? h('div', { class: 'about-update-assets' },
        ...d.assets.map((a) => h('button', {
          class: 'btn small',
          title: a.download_url,
          onClick: () => openLink(a.download_url),
        }, icon('download', 11), a.name, h('span', { class: 'about-asset-size' }, fmtBytes(a.size)))))
    : null;

  const notes = d.notes.trim()
    ? h('pre', { class: 'about-update-notes' }, d.notes)
    : null;

  // One click fetches the installer for this platform, checks its hash and
  // hands over. The asset buttons stay as the manual route — they open the
  // release page, which is what an offline or corporate-network user needs.
  const installBtn = h('button', {
    class: 'btn primary',
    title: t('about.install'),
    onClick: () => void startInstall(box, installBtn),
  }, icon('download', 12), t('about.install')) as HTMLButtonElement;

  box.append(h('div', { class: 'about-update-card new' },
    h('div', { class: 'about-update-head' },
      icon('download', 14),
      h('b', null, t('about.updateAvailable', { v: d.latest || d.latest_semver })),
      d.prerelease ? h('span', { class: 'ext-pill src' }, 'pre-release') : null,
      h('div', { class: 'spacer', style: { flex: '1' } }),
      installBtn,
    ),
    d.name && d.name !== d.latest ? h('div', { class: 'about-update-name' }, d.name) : null,
    h('div', { class: 'about-update-time' },
      `${t('about.publishedAt')} ${d.published_at ? new Date(d.published_at).toLocaleString() : '—'}`),
    d.notes.trim() ? h('div', { class: 'about-update-label' }, t('about.releaseNotes')) : null,
    notes,
    assets,
  ));
}

/** Download, verify, hand over. Each step says what it is doing — a 90 MB
 *  download with no feedback reads as a hang, and the app is about to quit, so
 *  the last thing on screen has to explain why. */
async function startInstall(box: HTMLElement, btn: HTMLButtonElement) {
  if (!window.confirm(t('about.installConfirm'))) return;
  btn.disabled = true;
  const line = h('div', { class: 'about-update-msg' });
  const say = (glyph: 'download' | 'check' | 'alert', text: string) => {
    clear(line);
    line.append(icon(glyph, 12), h('span', null, text));
  };
  say('download', t('about.downloading', { p: '0' }));
  box.append(line);

  let stop: (() => void) | null = null;
  try {
    stop = await onUpdateProgress((p) => {
      const pct = p.total ? Math.min(100, Math.round((p.received / p.total) * 100)) : 0;
      say('download', t('about.downloading', { p: String(pct) }));
    });
    const got = await api.downloadUpdate();
    stop();
    stop = null;
    say(got.verified ? 'check' : 'alert', t(got.verified ? 'about.verified' : 'about.unverified'));
    await new Promise((r) => setTimeout(r, 800));
    await api.applyUpdate(got.path);
  } catch (e) {
    stop?.();
    say('alert', errText(e));
    btn.disabled = false;
  }
}

/* ------------------------------------------------------------------- net */

/** Global outbound proxy. Left blank the app follows env vars and then the
 *  Windows system proxy; `none` forces a direct connection. */
function netSection(): HTMLElement {
  if (!draft) return h('div');
  const input_ = input(
    draft.proxy,
    (v) => {
      draft!.proxy = v.trim();
      persistSoon();
    },
    { placeholder: t('settings.proxyPlaceholder'), mono: true },
  );
  return h(
    'div',
    { class: 'provider-card' },
    h('div', { class: 'provider-head' },
      h('span', { class: 'provider-name' }, t('settings.network')),
      h('span', { class: 'provider-id', style: { marginLeft: 'auto' } }, 'egress'),
    ),
    field(t('settings.proxy'), input_),
  );
}

function probeRows(r: ProbeReport_): HTMLElement[] {
  const rows: HTMLElement[] = r.steps.map((s) =>
    h('div', { class: `probe-row ${s.ok ? 'ok' : 'bad'}` },
      h('span', { class: 'probe-mark' }, s.ok ? 'OK' : '×'),
      h('span', { class: 'probe-label' }, s.label),
      h('span', { class: 'probe-detail' }, s.detail),
    ),
  );
  if (r.error) {
    rows.push(h('pre', { class: 'probe-err' }, r.error));
  }
  return rows;
}

type ProbeReport_ = import('../types').ProbeReport;
