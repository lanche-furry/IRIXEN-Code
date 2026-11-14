// Theme application: light (default) / dark / user-defined custom themes.

import type { AppConfig, CustomTheme } from '../types';

export type ThemeId = 'light' | 'dark' | `custom:${string}`;

/** Light is the product default; anything unknown falls back to it. */
export const DEFAULT_THEME = 'light' as const;

/** Mirrors the light palette in styles/base.css — starting point for a new
 *  custom theme, so editing it is a change rather than a rewrite. */
const CUSTOM_VARS = [
  ['--bg-0', '#eaedf2'],
  ['--bg-1', '#f8f9fb'],
  ['--bg-2', '#e7eaef'],
  ['--bg-3', '#dbe0e8'],
  ['--border', '#d3d9e2'],
  ['--text-0', '#0b1018'],
  ['--text-1', '#3c4859'],
  ['--text-2', '#74808f'],
  ['--accent', '#0369a1'],
] as const;

/** Cache of the resolved base for the next cold start: index.html paints
 *  before the config arrives and reads this key. */
const STORE_KEY = 'irixen.theme';

/** Signature of the palette currently on screen. A `config:changed` event
 *  fires on every keystroke in Settings, so we must only animate when the
 *  resolved colours really changed — otherwise saving a form would flash. */
let appliedPalette = '';

function paletteKey(cfg: AppConfig): string {
  const id = cfg.theme || DEFAULT_THEME;
  if (!id.startsWith('custom:')) return `app:${id}`;
  const key = id.slice('custom:'.length);
  const t = cfg.custom_themes.find((x) => x.id === key);
  return `app:${id}:${t ? JSON.stringify(t.colors) : ''}`;
}

/** A short opacity fade instead of a hard palette snap. Switching between a
 *  near-white and a near-black UI in one frame reads as a flashbang. */
function fadeIfChanged(next: string) {
  const root = document.documentElement;
  const changed = appliedPalette !== '' && appliedPalette !== next;
  appliedPalette = next;
  if (!changed) return;
  root.classList.remove('theme-fade');
  // Force a reflow so two quick theme toggles restart the animation.
  void root.offsetWidth;
  root.classList.add('theme-fade');
  window.setTimeout(() => root.classList.remove('theme-fade'), 320);
}

export function applyTheme(cfg: AppConfig | null) {
  const root = document.documentElement;
  if (!cfg) return;
  fadeIfChanged(paletteKey(cfg));
  root.removeAttribute('style');
  const id = cfg.theme || DEFAULT_THEME;

  let base: 'light' | 'dark' = DEFAULT_THEME;

  if (id.startsWith('custom:')) {
    const key = id.slice('custom:'.length);
    const theme = cfg.custom_themes.find((t) => t.id === key);
    // A custom palette still needs a base for every variable it does not set;
    // pick it from its own background so the inverted artwork follows along.
    base = theme ? baseOf(theme) : DEFAULT_THEME;
    root.setAttribute('data-theme', base);
    if (theme) {
      for (const [name, value] of theme.colors) {
        if (name.startsWith('--') && value) root.style.setProperty(name, value);
      }
      // derive soft/accent-contrast helpers from the accent color
      const accent = theme.colors.find(([n]) => n === '--accent')?.[1];
      if (accent) {
        root.style.setProperty('--accent-soft', hexToRgba(accent, 0.14));
        root.style.setProperty('--accent-contrast', isLight(accent) ? '#06121b' : '#ffffff');
      }
    }
  } else {
    base = id === 'dark' ? 'dark' : 'light';
    root.setAttribute('data-theme', base);
  }

  rememberTheme(base);
}

export function customVarDefaults() {
  return CUSTOM_VARS.map(([k, v]) => `${k}: ${v}`).join('\n');
}

export function parseCustomColors(text: string): [string, string][] {
  const out: [string, string][] = [];
  for (const line of text.split('\n')) {
    const idx = line.indexOf(':');
    if (idx < 0) continue;
    const name = line.slice(0, idx).trim();
    const value = line.slice(idx + 1).trim();
    if (name.startsWith('--') && value) out.push([name, value]);
  }
  return out;
}

/** Remember the resolved base so the first paint of the next launch can pick
 *  the matching background before any IPC round-trip completes. */
export function rememberTheme(base: 'light' | 'dark') {
  try {
    localStorage.setItem(STORE_KEY, base);
  } catch {
    /* private mode / file origin — the default (light) still applies */
  }
}

/**
 * Preview a palette contributed by an extension.
 *
 * VS Code theme JSON has 700+ colour keys; the Rust side maps the dozen that
 * drive our layout onto our variables and everything else keeps its base
 * value. This is deliberately *not* persisted — it is a preview, and switching
 * the app theme (or restarting) puts the configured theme back.
 */
export function applyExtensionTheme(vars: Record<string, string>) {
  const root = document.documentElement;
  fadeIfChanged(`ext:${JSON.stringify(vars)}`);
  root.removeAttribute('style');
  const bg = vars['--bg-0'] || '';
  root.setAttribute('data-theme', isLight(bg || '#ffffff') ? 'light' : 'dark');
  for (const [name, value] of Object.entries(vars)) {
    if (name.startsWith('--') && value) root.style.setProperty(name, value);
  }
  const accent = vars['--accent'];
  if (accent) {
    root.style.setProperty('--accent-soft', hexToRgba(accent, 0.14));
    root.style.setProperty('--accent-contrast', isLight(accent) ? '#06121b' : '#ffffff');
  }
}

/** The base a custom palette implies: dark ink needs a dark canvas. */
function baseOf(theme: CustomTheme): 'light' | 'dark' {
  const bg =
    theme.colors.find(([n]) => n === '--bg-0')?.[1] ||
    theme.colors.find(([n]) => n === '--bg-1')?.[1] ||
    '';
  return isLight(bg) ? 'light' : 'dark';
}

function hexToRgba(hex: string, alpha: number): string {
  const m = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex.trim());
  if (!m) return hex;
  const r = parseInt(m[1], 16);
  const g = parseInt(m[2], 16);
  const b = parseInt(m[3], 16);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

function isLight(hex: string): boolean {
  const m = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex.trim());
  if (!m) return true;
  const [r, g, b] = [parseInt(m[1], 16), parseInt(m[2], 16), parseInt(m[3], 16)];
  return (r * 299 + g * 587 + b * 114) / 1000 > 150;
}
