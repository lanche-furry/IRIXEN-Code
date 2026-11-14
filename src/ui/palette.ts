// Command palette (Ctrl+Shift+P).
//
// Two sources in one list: the app's own actions and every command an installed
// extension contributed (declared or registered at runtime). Running an
// extension command goes through the host, so failures surface as a toast
// instead of a silently dead menu entry.

import { h, clear } from '../core/dom';
import { icon } from './icons';
import { t } from '../core/i18n';
import { api, errText } from '../core/api';
import { app } from '../core/state';
import { toast } from './toast';
import { openSettings } from './settings';
import { setSurface, selectView, togglePanel } from './shell';
import { pickFolder } from './explorer';
import { formatDocument, runActiveFile, chooseLanguage } from './editor';
import { commandLabel, commandPaletteHidden } from '../core/extmenus';
import { applyTheme } from './theme';
import { showStore, showInstalled } from './extensions';
import { openOnboarding } from './onboarding';

interface Item {
  label: string;
  group: string;
  hint?: string;
  run: () => void | Promise<void>;
}

let open = false;

function builtins(): Item[] {
  const theme: Item = {
    label: t('settings.theme'),
    group: t('app.name'),
    hint: 'Ctrl+Shift+T',
    run: () => {
      if (!app.config) return;
      app.config.theme = app.config.theme === 'light' ? 'dark' : 'light';
      applyTheme(app.config);
      void api.saveConfig(app.config).then((c) => {
        app.config = c;
        applyTheme(c);
      });
    },
  };
  return [
    { label: t('ext.title'), group: t('app.name'), run: () => setSurface('extensions') },
    { label: t('ext.store'), group: t('app.name'), run: () => { setSurface('extensions'); showStore(); } },
    { label: t('ext.installed'), group: t('app.name'), run: () => { setSurface('extensions'); showInstalled(); } },
    { label: t('nav.settings'), group: t('app.name'), run: () => openSettings('providers') },
    { label: t('onboard.reopen'), group: t('app.name'), run: () => openOnboarding() },
    { label: t('explorer.openFolder'), group: t('app.name'), run: () => void pickFolder() },
    { label: t('nav.explorer'), group: t('app.name'), run: () => selectView('explorer') },
    { label: t('nav.search'), group: t('app.name'), run: () => selectView('search') },
    { label: t('nav.git'), group: t('app.name'), run: () => selectView('git') },
    { label: t('nav.skills'), group: t('app.name'), run: () => selectView('skills') },
    { label: t('nav.stats'), group: t('app.name'), run: () => selectView('stats') },
    { label: t('rail.review'), group: t('app.name'), run: () => setSurface('review') },
    { label: t('rail.bridge'), group: t('app.name'), run: () => setSurface('bridge') },
    { label: t('panel.terminal'), group: t('app.name'), hint: 'Ctrl+J', run: () => togglePanel() },
    { label: t('editor.runFile'), group: t('app.name'), hint: 'F5', run: () => runActiveFile() },
    { label: t('editor.formatDoc'), group: t('app.name'), hint: 'Ctrl+Shift+I', run: () => void formatDocument() },
    { label: t('editor.setLanguage'), group: t('app.name'), run: () => chooseLanguage() },
    { label: t('rail.chat'), group: t('app.name'), hint: 'Ctrl+Shift+C', run: () => setSurface('chat') },
    theme,
  ];
}

function extensionItems(): Item[] {
  return app.extCommands.filter((c) => !commandPaletteHidden(c.id)).map((c) => ({
    label: commandLabel(c.id),
    group: c.ext_name || c.ext,
    hint: c.live ? c.id : `${c.id} · ${t('ext.host.notLive')}`,
    run: async () => {
      try {
        await api.exthostRun(c.id);
      } catch (e) {
        toast(errText(e), 'err', 4200);
      }
    },
  }));
}

export function openPalette() {
  if (open) return;
  open = true;

  const input = h('input', {
    class: 'palette-input',
    placeholder: t('palette.placeholder'),
  }) as HTMLInputElement;
  const list = h('div', { class: 'palette-list' });
  const back = h('div', { class: 'palette-back' });
  const card = h('div', { class: 'palette' }, h('div', { class: 'palette-head' }, icon('command', 13), input), list);
  back.append(card);
  document.body.append(back);

  const all = [...builtins(), ...extensionItems()];
  let shown: Item[] = all;
  let cursor = 0;

  const close = () => {
    open = false;
    back.remove();
    document.removeEventListener('keydown', onKey, true);
  };

  const run = (item?: Item) => {
    if (!item) return;
    close();
    void Promise.resolve(item.run()).catch((e) => toast(errText(e), 'err'));
  };

  const draw = () => {
    clear(list);
    if (shown.length === 0) {
      list.append(h('div', { class: 'palette-empty' }, t('palette.empty')));
      return;
    }
    shown.forEach((item, i) => {
      list.append(h('div', {
        class: `palette-row${i === cursor ? ' on' : ''}`,
        onMouseenter: () => {
          cursor = i;
          draw();
        },
        onClick: () => run(item),
      },
        h('span', { class: 'palette-label' }, item.label),
        item.hint ? h('span', { class: 'palette-hint' }, item.hint) : null,
        h('span', { class: 'palette-group' }, item.group),
      ));
    });
    (list.children[cursor] as HTMLElement | undefined)?.scrollIntoView({ block: 'nearest' });
  };

  const filter = () => {
    const q = input.value.trim().toLowerCase();
    shown = !q
      ? all
      : all.filter((it) => it.label.toLowerCase().includes(q) || it.group.toLowerCase().includes(q) || (it.hint || '').toLowerCase().includes(q));
    cursor = 0;
    draw();
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      cursor = Math.min(cursor + 1, shown.length - 1);
      draw();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      cursor = Math.max(cursor - 1, 0);
      draw();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      run(shown[cursor]);
    }
  };

  input.addEventListener('input', filter);
  document.addEventListener('keydown', onKey, true);
  back.addEventListener('click', (e) => {
    if (e.target === back) close();
  });

  draw();
  setTimeout(() => input.focus(), 20);
}
