// Skills panel: enable/disable, inspect, uninstall and interactive install.

import { app, emit } from '../core/state';
import { api } from '../core/api';
import { h, clear } from '../core/dom';
import { icon } from './icons';
import { t } from '../core/i18n';
import { toast, confirmDialog } from './toast';
import { codeBlock } from './markdown';

let bodyEl: HTMLElement | null = null;

export function mountSkills(container: HTMLElement) {
  bodyEl = container;
  void loadSkills();
}

export async function loadSkills() {
  try {
    app.skills = await api.skills();
  } catch (e) {
    toast(String(e), 'err');
  }
  render();
}

function render() {
  if (!bodyEl) return;
  clear(bodyEl);

  const input = h('input', { placeholder: t('skills.installPlaceholder'), style: { fontSize: '14.5px' } });
  const btn = h('button', { class: 'btn primary', style: { justifyContent: 'center' } }, icon('plus', 13), t('skills.install'));
  const installing = (busy: boolean) => {
    btn.disabled = busy;
    btn.textContent = '';
    btn.append(icon(busy ? 'refresh' : 'plus', 13), t('skills.install'));
    if (busy) btn.firstElementChild?.classList.add('spin');
  };

  const install = async () => {
    const src = input.value.trim();
    if (!src) return;
    installing(true);
    try {
      const skill = await api.skillInstall(src);
      input.value = '';
      toast(`+ ${skill.name}`, 'ok');
      await loadSkills();
      emit('skills:changed');
    } catch (e) {
      toast(String(e), 'err', 5000);
    } finally {
      installing(false);
    }
  };
  btn.addEventListener('click', () => void install());
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') void install();
  });

  bodyEl.append(h('div', { class: 'search-wrap' }, input, btn));

  const builtin = app.skills.filter((s) => s.builtin);
  const installed = app.skills.filter((s) => !s.builtin);

  if (builtin.length) {
    bodyEl.append(h('div', { class: 'git-section', style: { padding: '10px 18px 4px' } }, t('skills.builtin')));
    builtin.forEach((s) => bodyEl!.append(card(s)));
  }
  bodyEl.append(h('div', { class: 'git-section', style: { padding: '10px 18px 4px' } }, t('skills.installed')));
  if (installed.length === 0) {
    bodyEl.append(h('div', { class: 'side-empty', style: { padding: '10px 18px' } }, t('skills.empty')));
  } else {
    installed.forEach((s) => bodyEl!.append(card(s)));
  }
}

function card(skill: (typeof app.skills)[number]): HTMLElement {
  const toggleEl = h('div', {
    class: `switch ${skill.enabled ? 'on' : ''}`,
    title: skill.enabled ? t('skills.disable') : t('skills.enable'),
    onClick: async () => {
      try {
        await api.skillToggle(skill.id, !skill.enabled);
        skill.enabled = !skill.enabled;
        render();
      } catch (e) {
        toast(String(e), 'err');
      }
    },
  });

  const actions = h('div', { class: 'skill-actions' });
  actions.append(
    h(
      'button',
      { class: 'btn sm ghost', onClick: () => void viewSkill(skill.id, skill.name) },
      icon('book', 12),
      t('skills.read'),
    ),
  );
  if (!skill.builtin) {
    actions.append(
      h(
        'button',
        {
          class: 'btn sm ghost danger',
          onClick: async () => {
            const ok = await confirmDialog(t('skills.remove'), skill.name);
            if (!ok) return;
            try {
              await api.skillUninstall(skill.id);
              await loadSkills();
            } catch (e) {
              toast(String(e), 'err');
            }
          },
        },
        icon('trash', 12),
        t('skills.remove'),
      ),
    );
  }

  return h(
    'div',
    { class: 'skill-card' },
    h('div', { class: 'skill-head' }, h('span', { class: 'skill-name' }, skill.name), h('span', { class: 'spacer' }), toggleEl),
    skill.description ? h('div', { class: 'skill-desc' }, skill.description) : null,
    skill.when.length
      ? h('div', { class: 'skill-tags' }, ...skill.when.slice(0, 6).map((w) => h('span', { class: 'chip' }, w)))
      : null,
    actions,
  );
}

async function viewSkill(id: string, name: string) {
  let text = '';
  try {
    text = await api.skillRead(id);
  } catch (e) {
    toast(String(e), 'err');
    return;
  }
  const overlay = h('div', { class: 'modal' });
  const body = h('div', { style: { padding: '14px 16px', overflow: 'auto', maxHeight: '64vh' } });
  body.append(codeBlock(text, 'markdown'));
  const card = h(
    'div',
    { style: { width: 'min(760px, 92vw)', background: 'var(--bg-1)', border: '1px solid var(--border-strong)', borderRadius: '12px', overflow: 'hidden', display: 'grid', gridTemplateRows: '44px 1fr' } },
    h(
      'div',
      { class: 'row', style: { padding: '0 14px', borderBottom: '1px solid var(--border)' } },
      h('span', { style: { fontWeight: '600' } }, name),
      h('span', { class: 'spacer' }),
      h('button', { class: 'icon-btn', title: t('common.close'), onClick: () => overlay.remove() }, icon('close', 14)),
    ),
    body,
  );
  overlay.append(card);
  overlay.addEventListener('click', (e) => {
    if (e.target === overlay) overlay.remove();
  });
  document.body.append(overlay);
}
