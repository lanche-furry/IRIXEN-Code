// Thinking-effort vocabulary shared by the composer picker and the settings
// page. Kept out of ui/chat.ts so settings can render the same ladder without
// pulling in the whole transcript renderer.

import { getLang, type Lang } from './i18n';
import type { Effort, ModelInfo } from '../types';

// Typed as Record<Lang, …> on purpose: adding a locale to i18n without adding
// its effort ladder here is a compile error, not a silent English fallback.
const LABELS: Record<Lang, Record<Effort, string>> = {
  'zh-CN': { auto: '自动', off: '关闭', minimal: '极低', low: '低', medium: '中', high: '高' },
  'zh-TW': { auto: '自動', off: '關閉', minimal: '極低', low: '低', medium: '中', high: '高' },
  'en-US': { auto: 'Auto', off: 'Off', minimal: 'Minimal', low: 'Low', medium: 'Medium', high: 'High' },
  'ja-JP': { auto: '自動', off: 'オフ', minimal: '最小', low: '低', medium: '中', high: '高' },
  'ko-KR': { auto: '자동', off: '끔', minimal: '최소', low: '낮음', medium: '중간', high: '높음' },
  'ru-RU': { auto: 'Авто', off: 'Выкл', minimal: 'Мин', low: 'Низк', medium: 'Сред', high: 'Выс' },
  'es-ES': { auto: 'Auto', off: 'Desactivado', minimal: 'Mínimo', low: 'Bajo', medium: 'Medio', high: 'Alto' },
  'fr-FR': { auto: 'Auto', off: 'Désactivé', minimal: 'Minimal', low: 'Faible', medium: 'Moyen', high: 'Élevé' },
};

export function effortLabel(e: Effort): string {
  const dict = LABELS[getLang()] ?? LABELS['en-US'];
  return dict[e] ?? e;
}

/** Filled bars in the composer's three-step meter. Auto draws none. */
export function effortLevel(e: Effort): number {
  switch (e) {
    case 'minimal':
    case 'low':
      return 1;
    case 'medium':
      return 2;
    case 'high':
      return 3;
    default:
      return 0;
  }
}

/** The generic ladder, used when a model doesn't publish its own levels. */
export const ALL_EFFORTS: Effort[] = ['auto', 'off', 'low', 'medium', 'high'];

/**
 * Which values the selector offers for a model.
 *
 * `unknown` and `unsupported` still get the full ladder on purpose: plenty of
 * OpenAI-compatible vendors accept `reasoning_effort` without advertising it,
 * and the request path degrades gracefully (it retries once without the field
 * if the server rejects it). Collapsing the menu to a lone "Auto" — what the
 * old version did for every DeepSeek model — reads as "this build has no
 * thinking knob at all".
 */
export function effortChoices(m?: ModelInfo | null): Effort[] {
  const th = m?.thinking;
  if (!th) return ALL_EFFORTS;
  if (th.type === 'always_on') return ['auto'];
  if (th.type === 'effort') {
    const seen = new Set<Effort>(['auto', 'off', ...th.levels]);
    return [...seen];
  }
  if (th.type === 'budget') return ALL_EFFORTS;
  return ALL_EFFORTS;
}
