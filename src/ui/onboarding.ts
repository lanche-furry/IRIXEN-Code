// First-run setup wizard.
//
// VS Code greets a fresh profile with a short, dismissible setup flow instead
// of silently falling back to defaults. IRIXEN Code does the same: four steps
// (welcome → language/theme → model provider → ready) drawn as a modal outside
// the shell DOM, exactly like settings.ts.
//
// Completion is persisted in config.json (`onboarding_completed`), NOT in
// localStorage: the Rust setup hook writes a default config on the very first
// launch, so "does config.json exist" can never be the signal. Skipping counts
// as completion; the wizard can be re-run from the command palette.

import { h, clear } from '../core/dom';
import { t, setLang, LANGS } from '../core/i18n';
import { api } from '../core/api';
import { app, emit, on } from '../core/state';
import { applyTheme } from './theme';
import { icon, type IconName } from './icons';
import { openSettings } from './settings';
import { pickFolder } from './explorer';
import { setSurface } from './shell';
import type { AppConfig } from '../types';

type Step = 'welcome' | 'appearance' | 'provider' | 'ready';

const STEPS: Step[] = ['welcome', 'appearance', 'provider', 'ready'];
const STEP_LABEL: Record<Step, string> = {
  welcome: 'onboard.stepWelcome',
  appearance: 'onboard.stepAppearance',
  provider: 'onboard.stepProvider',
  ready: 'onboard.stepReady',
};

let overlay: HTMLElement | null = null;
let step: Step = 'welcome';
let draft: AppConfig | null = null;
let unsubLang: (() => void) | null = null;
let closing = false;

const clone = <T,>(v: T): T => JSON.parse(JSON.stringify(v));

/** Show the wizard only on an untouched profile. `force` re-opens it on demand. */
export function maybeShowOnboarding(force = false) {
  if (!app.config) return;
  if (!force && app.config.onboarding_completed) return;
  show();
}

export function openOnboarding() {
  maybeShowOnboarding(true);
}

function show() {
  if (overlay) overlay.remove();
  unsubLang?.();
  unsubLang = null;
  closing = false;
  step = 'welcome';
  draft = clone(app.config!);

  overlay = h('div', { class: 'modal ob-modal' });
  document.body.append(overlay);
  // The modal lives outside the shell, so shell's lang:changed relabel never
  // reaches it. Repaint on every language change (same as settings.ts).
  unsubLang = on('lang:changed', () => render());
  render();
  document.addEventListener('keydown', onKey);
}

/** Persist language/theme as the user clicks, so a crash mid-wizard keeps them. */
async function sync() {
  if (!draft) return;
  try {
    const saved = await api.saveConfig(draft);
    app.config = saved;
  } catch {
    /* plain browser / transient IPC failure — the wizard still works locally */
  }
}

/** Finish (or skip): record completion, then get out of the user's way. */
async function finish() {
  if (closing) return;
  closing = true;
  const cfg: AppConfig = {
    ...(app.config || draft!),
    ...(draft || {}),
    onboarding_completed: true,
  } as AppConfig;
  try {
    const saved = await api.saveConfig(cfg);
    app.config = saved;
  } catch {
    // Even if the write failed locally, never trap the user behind the wizard.
    if (app.config) app.config.onboarding_completed = true;
  }
  if (draft) {
    setLang(draft.language);
    applyTheme(draft);
  }
  emit('config:changed');
  close();
}

function close() {
  document.removeEventListener('keydown', onKey);
  unsubLang?.();
  unsubLang = null;
  overlay?.remove();
  overlay = null;
  draft = null;
  closing = false;
}

function onKey(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    e.preventDefault();
    void finish();
  }
}

function go(next: Step) {
  step = next;
  render();
}

function render() {
  if (!overlay || !draft) return;
  clear(overlay);
  const idx = STEPS.indexOf(step);
  const card = h('div', { class: 'ob-card' });

  /* ---------------------------------------------------------- left rail */
  const side = h('div', { class: 'ob-side' });
  side.append(
    h('div', { class: 'ob-brand' },
      // The real app mark (public/irixen-icon.png, generated from
      // assets/icon.png), not a letter placeholder.
      h('img', { class: 'ob-mark', src: '/irixen-icon.png', alt: '' }),
      h('span', {}, 'IRIXEN Code'),
    ),
    h('div', { class: 'ob-badge' }, t('onboard.badge')),
  );
  const list = h('div', { class: 'ob-steps' });
  STEPS.forEach((s, i) => {
    const done = i < idx;
    const active = i === idx;
    list.append(
      h('div', { class: `ob-step${active ? ' active' : ''}${done ? ' done' : ''}` },
        h('span', { class: 'ob-step-n' }, done ? icon('check', 12) : String(i + 1)),
        h('span', {}, t(STEP_LABEL[s])),
      ),
    );
  });
  side.append(list);
  const version = app.info?.version || '';
  side.append(h('div', { class: 'ob-side-foot' }, version || 'IRIXEN'));

  /* -------------------------------------------------------------- main */
  const main = h('div', { class: 'ob-main' });
  const head = h('div', { class: 'ob-head' },
    h('span', { class: 'ob-counter' }, `${idx + 1} / ${STEPS.length}`),
    h('button', {
      class: 'icon-btn', title: t('onboard.skip'), 'aria-label': t('onboard.skip'),
      onClick: () => void finish(),
    }, icon('close', 14)),
  );
  const body = h('div', { class: 'ob-body' });
  if (step === 'welcome') body.append(welcomeStep());
  else if (step === 'appearance') body.append(appearanceStep());
  else if (step === 'provider') body.append(providerStep());
  else body.append(readyStep());

  const foot = h('div', { class: 'ob-foot' });
  foot.append(h('button', { class: 'ob-link', onClick: () => void finish() }, t('onboard.skip')));
  const nav = h('div', { class: 'row' });
  if (idx > 0) {
    nav.append(h('button', {
      class: 'btn', onClick: () => go(STEPS[idx - 1]),
    }, t('onboard.back')));
  }
  if (step === 'welcome') {
    nav.append(h('button', {
      class: 'btn primary', onClick: () => go('appearance'),
    }, t('onboard.start'), icon('chevron', 13)));
  } else if (step === 'ready') {
    nav.append(h('button', {
      class: 'btn primary', onClick: () => void finish(),
    }, icon('check', 13), t('onboard.finish')));
  } else {
    nav.append(h('button', {
      class: 'btn primary', onClick: () => go(STEPS[idx + 1]),
    }, t('onboard.next'), icon('chevron', 13)));
  }
  foot.append(nav);
  main.append(head, body, foot);

  card.append(side, main);
  overlay.append(card);
}

/* ------------------------------------------------------------------ steps */

function welcomeStep(): HTMLElement {
  const wrap = h('div', { class: 'ob-welcome' });
  wrap.append(
    h('img', { class: 'ob-hero-mark', src: '/irixen-icon.png', alt: '' }),
    h('h1', {}, t('onboard.title')),
    h('p', { class: 'ob-lead' }, t('onboard.subtitle')),
  );
  const preview = h('div', { class: 'ob-preview' });
  const items: { key: string; ico: IconName }[] = [
    { key: 'onboard.stepAppearance', ico: 'sun' },
    { key: 'onboard.stepProvider', ico: 'cpu' },
    { key: 'onboard.stepReady', ico: 'spark' },
  ];
  items.forEach((it, i) => {
    preview.append(
      h('div', { class: 'ob-preview-row' },
        h('span', { class: 'ob-preview-n' }, String(i + 1)),
        icon(it.ico, 15),
        h('span', {}, t(it.key)),
      ),
    );
  });
  wrap.append(preview);
  return wrap;
}

function appearanceStep(): HTMLElement {
  const wrap = h('div');
  wrap.append(
    h('h2', {}, t('onboard.appearanceTitle')),
    h('p', { class: 'ob-lead' }, t('onboard.appearanceHint')),
    h('h3', {}, t('settings.language')),
  );
  const langs = h('div', { class: 'row wrap' });
  for (const l of LANGS) {
    langs.append(h('button', {
      class: `chip${draft!.language === l.id ? ' on' : ''}`,
      onClick: () => {
        if (!draft) return;
        draft.language = l.id;
        // setLang emits lang:changed, which repaints this modal and the shell.
        setLang(l.id);
        void sync();
      },
    }, l.label));
  }
  wrap.append(langs, h('h3', {}, t('settings.theme')));

  const grid = h('div', { class: 'ob-themes' });
  const themes: { id: string; label: string; tones: string[]; ico: IconName }[] = [
    { id: 'light', label: t('settings.themeLight'), tones: ['#f8f9fb', '#d3d9e2', '#0369a1'], ico: 'sun' },
    { id: 'dark', label: t('settings.themeDark'), tones: ['#0e121a', '#333f4f', '#38bdf8'], ico: 'moon' },
  ];
  for (const th of themes) {
    grid.append(h('button', {
      class: `ob-theme${draft!.theme === th.id ? ' active' : ''}`,
      onClick: () => {
        if (!draft) return;
        draft.theme = th.id;
        applyTheme(draft);
        void sync();
        render();
      },
    },
      h('span', { class: 'ob-swatch' }, ...th.tones.map((c) => h('i', { style: { background: c } }))),
      h('span', { class: 'ob-theme-label' }, icon(th.ico, 14), th.label),
    ));
  }
  wrap.append(grid);
  return wrap;
}

function providerStep(): HTMLElement {
  const wrap = h('div');
  wrap.append(
    h('h2', {}, t('onboard.providerTitle')),
    h('p', { class: 'ob-lead' }, t('onboard.providerHint')),
  );
  const count = (draft!.providers || []).filter((p) => (p.api_key || '').trim()).length;
  wrap.append(h('div', { class: `ob-status${count ? ' ok' : ''}` },
    icon(count ? 'check' : 'info', 15),
    h('span', {}, count ? t('onboard.providerReady', { n: count }) : t('onboard.providerNone')),
  ));
  wrap.append(h('div', { class: 'row wrap', style: { marginTop: '18px' } },
    h('button', {
      class: 'btn primary',
      onClick: async () => {
        await finish();
        openSettings('providers');
      },
    }, icon('cpu', 14), t('onboard.providerNow')),
    h('button', {
      class: 'btn', onClick: () => go('ready'),
    }, t('onboard.providerLater')),
  ));
  return wrap;
}

function readyStep(): HTMLElement {
  const wrap = h('div');
  wrap.append(
    h('div', { class: 'ob-done-mark' }, icon('check', 26)),
    h('h2', {}, t('onboard.doneTitle')),
    h('p', { class: 'ob-lead' }, t('onboard.doneHint')),
  );
  const actions: { label: string; ico: IconName; run: () => void }[] = [
    { label: t('onboard.doneWorkspace'), ico: 'folderOpen', run: () => void pickFolder() },
    { label: t('onboard.doneSettings'), ico: 'settings', run: () => openSettings('appearance') },
    { label: t('onboard.doneChat'), ico: 'chat', run: () => setSurface('chat') },
  ];
  const grid = h('div', { class: 'ob-actions' });
  for (const a of actions) {
    grid.append(h('button', {
      class: 'ob-action',
      onClick: () => {
        void finish();
        a.run();
      },
    }, icon(a.ico, 18), h('span', {}, a.label), icon('chevron', 13)));
  }
  wrap.append(grid);
  return wrap;
}
