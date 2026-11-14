// Boot checklist — the thin, typed bridge to the splash card in index.html.
//
// The card has to exist before the module bundle, so its DOM and its fallback
// strings live in index.html; everything that knows *what* the app is doing
// lives here. Every call is a no-op when the card is gone (plain-browser dev,
// hot reload, or a window that booted long ago), which is why nothing guards
// the call sites.

import { t } from './i18n';

export type BootState = 'pending' | 'active' | 'done' | 'skip' | 'error';

interface BootOverlay {
  init?: (items: { id: string; label?: string }[]) => void;
  relabel?: (items: { id: string; label?: string }[]) => void;
  step?: (id: string, state: BootState, detail?: string) => void;
  finish?: () => void;
  done?: () => void;
}

declare global {
  interface Window {
    __irixenBoot?: BootOverlay;
  }
}

/** Stage order is fixed: the checklist reads top-to-bottom like the boot does. */
export const BOOT_ORDER = ['core', 'ui', 'ext', 'bridge', 'workspace'] as const;

/** Cached so a language change can redraw without the caller re-listing them. */
let lastItems: { id: string; label?: string }[] | null = null;

function labels(): { id: string; label: string }[] {
  return [
    { id: 'core', label: t('boot.core') },
    { id: 'ui', label: t('boot.ui') },
    { id: 'ext', label: t('boot.ext') },
    { id: 'bridge', label: t('boot.bridge') },
    { id: 'workspace', label: t('boot.workspace') },
  ];
}

/** Draw the checklist. Safe to call more than once; labels are replaced. */
export function bootInit() {
  lastItems = labels();
  window.__irixenBoot?.init?.(lastItems);
}

/** Language finished loading (or changed): redraw text, keep the states. */
export function bootRelabel() {
  if (!lastItems) return;
  lastItems = labels();
  const api = window.__irixenBoot;
  (api?.relabel ?? api?.init)?.(lastItems);
}

/** Advance one row. `detail` is short, right-aligned context — a version, a
 *  count, a port — never a sentence. */
export function bootStep(id: string, state: BootState, detail?: string) {
  window.__irixenBoot?.step?.(id, state, detail);
}

/** Settle whatever is left and hand the window over to the app. Idempotent. */
export function bootFinish() {
  window.__irixenBoot?.finish?.();
}

/** Resolve to the promise's value, or `null` once `ms` elapses. The work keeps
 *  running — the checklist only stops waiting on it, and says so. */
export function within<T>(p: Promise<T>, ms: number): Promise<T | null> {
  return new Promise((resolve) => {
    let settled = false;
    const done = (v: T | null) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(v);
    };
    const timer = setTimeout(() => done(null), ms);
    p.then((v) => done(v)).catch(() => done(null));
  });
}
