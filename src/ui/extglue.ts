// Frontend half of the Node extension host: event fan-in, the modal prompts an
// extension can block on, and the status-bar slots it owns.
//
// Kept out of extensions.ts on purpose — these events arrive whether or not the
// extensions panel is open, and a closed panel must not be the reason an
// extension's `showInputBox` promise never settles.

import { onExtHostEvent, api, errText } from '../core/api';
import { listen } from '@tauri-apps/api/event';
import { app, emit } from '../core/state';
import { h, clear } from '../core/dom';
import { t } from '../core/i18n';
import { toast, promptDialog } from './toast';
import { icon } from './icons';
import { openFile, applyExternalEdits, saveActive } from './editor';
import { attachTerminal, forgetTerminal } from './terminal';
import type { ExtUiRequest, HostLog, HostStatus, RuntimeCommand, StatusItem } from '../types';

// Extension hosts are auto-activated during app startup, before the dock and
// its panels mount. Subscribing only inside `mountExtGlue()` dropped every
// early `status` frame, so `app.hosts` stayed empty: the cards said "宿主未运行"
// and the pid never appeared even though the process was running. These three
// subscriptions only touch the global store, so they can go live at module load.
void onExtHostEvent<{ hosts: HostStatus[] }>('status', (p) => {
  app.hosts = p.hosts || [];
  emit('exthost:status');
});
void onExtHostEvent<{ commands: RuntimeCommand[] }>('commands', (p) => {
  app.extCommands = p.commands || [];
  emit('exthost:commands');
});
void onExtHostEvent<{ items: StatusItem[] }>('statusbar', (p) => {
  app.extStatus = p.items || [];
  emit('exthost:statusbar');
});

export function mountExtGlue() {
  // Belt and braces: even with the module-level subscription, an event emitted
  // while the webview was still being created is not replayed. Pull a snapshot
  // once the UI is up, then again briefly for hosts that finish activating late.
  void api
    .exthostStatus()
    .then((hosts) => {
      app.hosts = hosts || [];
      emit('exthost:status');
    })
    .catch(() => undefined);

  void onExtHostEvent<HostLog>('log', (line) => {
    app.extLogs.push(line);
    if (app.extLogs.length > 800) app.extLogs.shift();
    emit('exthost:log', line);
  });

  void onExtHostEvent<ExtUiRequest>('ui', (req) => {
    void answerPrompt(req);
  });

  void onExtHostEvent<{ path: string }>('open', (p) => {
    if (p && p.path) void openFile(p.path);
  });

  void onExtHostEvent<{ path: string; edits: EditSpec[] }>('edit', (p) => {
    if (!p || !p.path) return;
    void applyExternalEdits(p.path, p.edits || []).catch((e) => toast(errText(e), 'err'));
  });

  void onExtHostEvent<{ path: string }>('save', () => {
    void saveActive();
  });

  // File-run stream (`file_run`): one child process, output line by line.
  void listen<Record<string, unknown>>('run://event', (e) => emit('run:event', e.payload));

  // An extension opened a terminal or asked for one to be shown. The core has
  // been buffering the PTY the whole time, so attaching replays its output —
  // `npm run dev` is already printing before the panel exists.
  void onExtHostEvent<{ action: 'show' | 'close'; id: string }>('terminal', (p) => {
    if (!p || !p.id) return;
    if (p.action === 'close') {
      forgetTerminal(p.id);
      return;
    }
    app.panelTab = 'terminal';
    emit('panel:open');
    void attachTerminal(p.id);
  });
}

interface EditSpec {
  start: { line: number; character: number };
  end: { line: number; character: number };
  text: string;
}

/* ------------------------------------------------------------------ prompts */

/**
 * `window.showInformationMessage` and friends. The extension is awaiting this
 * promise for as long as the dialog is up, so every path (button, Escape,
 * backdrop click) must send exactly one reply.
 */
async function answerPrompt(req: ExtUiRequest) {
  if (req.kind === 'inputBox') {
    const value = await promptDialog(req.prompt || req.placeHolder || t('ext.prompt.title'), req.placeHolder || '', req.value || '');
    await api.exthostReply(req.id, value);
    return;
  }
  if (req.kind === 'quickPick') {
    const picked = await quickPick(req);
    await api.exthostReply(req.id, picked);
    return;
  }
  const picked = await messageBox(req);
  await api.exthostReply(req.id, picked);
}

function overlay(): { back: HTMLElement; card: HTMLElement; close: (v: unknown, resolve: (v: any) => void) => void } {
  const back = h('div', { class: 'modal ext-prompt', style: { zIndex: '85' } });
  const card = h('div', { class: 'ext-prompt-card' });
  const close = (v: unknown, resolve: (v: any) => void) => {
    back.remove();
    resolve(v);
  };
  return { back, card, close };
}

function messageBox(req: ExtUiRequest): Promise<string | undefined> {
  return new Promise((resolve) => {
    const { back, card, close } = overlay();
    const items = (req.items || []).map((i) => (typeof i === 'string' ? i : i.label)).filter(Boolean);
    const sev = req.severity || 'info';

    const buttons = h('div', { class: 'ext-prompt-actions' });
    for (const label of items) {
      buttons.append(h('button', { class: 'btn', onClick: () => close(label, resolve) }, label));
    }
    buttons.append(h('button', {
      class: `btn${items.length ? '' : ' primary'}`,
      onClick: () => close(undefined, resolve),
    }, items.length ? t('common.close') : t('common.ok')));

    card.append(
      // An icon, not a text glyph: "✕/!/i" were drawn in whatever colour the
      // platform font picked and the ✕ read as a close button rather than an
      // error. The SVG inherits `currentColor`, so it follows --err/--warn.
      h('div', { class: `ext-prompt-sev ${sev}` }, icon(sev === 'info' ? 'info' : 'alert', 13)),
      h('div', { class: 'ext-prompt-body' },
        h('div', { class: 'ext-prompt-title' }, req.message || ''),
        req.detail ? h('div', { class: 'ext-prompt-detail' }, req.detail) : null,
        h('div', { class: 'hint', style: { fontSize: '13px' } }, req.ext),
      ),
      buttons,
    );

    back.append(card);
    back.addEventListener('click', (e) => {
      if (e.target === back) close(undefined, resolve);
    });
    const esc = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        document.removeEventListener('keydown', esc);
        close(undefined, resolve);
      }
    };
    document.addEventListener('keydown', esc);
    document.body.append(back);
  });
}

/**
 * Rows come back as *indices* into `req.items`, not labels: the host maps them
 * back to the objects it was given, so `const item = await showQuickPick(items)`
 * still yields `{ label, description, value }` instead of a bare string.
 */
function quickPick(req: ExtUiRequest): Promise<number | number[] | undefined> {
  return new Promise((resolve) => {
    const { back, card, close } = overlay();
    const multi = !!req.canPickMany;
    const chosen = new Set<number>();
    const raw = (req.items || []).map((i) => (typeof i === 'string' ? { label: i, description: '', detail: '' } : i));

    const input = h('input', { class: 'mono-input', placeholder: req.placeholder || '' }) as HTMLInputElement;
    const list = h('div', { class: 'ext-prompt-list' });

    const draw = () => {
      clear(list);
      const q = input.value.trim().toLowerCase();
      const rows = raw.filter((r) => !q || r.label.toLowerCase().includes(q));
      if (rows.length === 0) {
        list.append(h('div', { class: 'hint' }, t('common.noMatches')));
        return;
      }
      for (const row of rows) {
        const at = raw.indexOf(row);
        const on = chosen.has(at);
        list.append(h('div', {
          class: `ext-prompt-row${on ? ' on' : ''}`,
          onClick: () => {
            if (multi) {
              if (on) chosen.delete(at);
              else chosen.add(at);
              draw();
            } else {
              close(at, resolve);
            }
          },
        },
          h('span', { class: 'ext-prompt-label' }, row.label),
          row.description ? h('span', { class: 'ext-prompt-desc' }, row.description) : null,
          multi && on ? h('span', { class: 'ext-prompt-check' }, '✓') : null,
        ));
      }
    };

    input.addEventListener('input', draw);
    input.addEventListener('keydown', (e: KeyboardEvent) => {
      if (e.key === 'Enter') {
        const first = list.querySelector('.ext-prompt-row') as HTMLElement | null;
        if (multi) close([...chosen], resolve);
        else first?.click();
      }
      if (e.key === 'Escape') close(undefined, resolve);
    });
    draw();

    card.append(
      h('div', { class: 'ext-prompt-body' },
        h('div', { class: 'ext-prompt-title' }, req.placeholder || t('ext.prompt.pick')),
        h('div', { class: 'hint', style: { fontSize: '13px' } }, req.ext),
      ),
      input,
      list,
      h('div', { class: 'ext-prompt-actions' },
        h('button', { class: 'btn', onClick: () => close(undefined, resolve) }, t('common.cancel')),
        multi ? h('button', { class: 'btn primary', onClick: () => close([...chosen], resolve) }, t('common.ok')) : null,
      ),
    );
    back.append(card);
    back.addEventListener('click', (e) => {
      if (e.target === back) close(undefined, resolve);
    });
    document.body.append(back);
    setTimeout(() => input.focus(), 30);
  });
}
