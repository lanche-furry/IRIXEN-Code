// Minimal DOM helpers — no virtual DOM, no framework, just typed builders.

type Props = Record<string, any>;

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props?: Props | null,
  ...children: (Node | string | number | null | undefined | false)[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  if (props) {
    for (const [key, value] of Object.entries(props)) {
      if (value === null || value === undefined || value === false) continue;
      if (key === 'class' || key === 'className') {
        el.className = String(value);
      } else if (key === 'style' && typeof value === 'object') {
        Object.assign(el.style, value);
      } else if (key === 'dataset' && typeof value === 'object') {
        Object.assign(el.dataset, value);
      } else if (key === 'html') {
        el.innerHTML = String(value);
      } else if (key.startsWith('on') && typeof value === 'function') {
        el.addEventListener(key.slice(2).toLowerCase(), value as EventListener);
      } else if (key in el && key !== 'list' && key !== 'type') {
        (el as any)[key] = value;
      } else {
        el.setAttribute(key, String(value));
      }
    }
  }
  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  nameIconOnlyControl(el);
  return el;
}

/**
 * `title` is a tooltip, not an accessible name.
 *
 * Almost every icon button in the shell is an SVG plus a `title`. Screen
 * readers and the browser's accessibility audit treat those as controls with
 * no name ("控件没有名称"), because the only child is a decorative graphic.
 * Mirroring `title` into `aria-label` fixes the whole app in one place and
 * never overrides a name the caller set on purpose.
 */
function nameIconOnlyControl(el: HTMLElement) {
  const role = el.getAttribute('role');
  const isControl = el instanceof HTMLButtonElement || role === 'button' || role === 'menuitem';
  if (!isControl) return;
  if (el.hasAttribute('aria-label') || el.hasAttribute('aria-labelledby')) return;
  if ((el.textContent || '').trim()) return;
  const title = el.getAttribute('title');
  if (title && title.trim()) el.setAttribute('aria-label', title);
}

export function svg(path: string, size = 16): SVGSVGElement {
  const template = document.createElement('template');
  // Decorative by default — the owning control carries the accessible name.
  template.innerHTML =
    `<svg viewBox="0 0 24 24" width="${size}" height="${size}" fill="none" ` +
    `stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" ` +
    `aria-hidden="true" focusable="false">${path}</svg>`;
  return template.content.firstElementChild as SVGSVGElement;
}

export function clear(el: Element) {
  while (el.firstChild) el.removeChild(el.firstChild);
}

export function fmtBytes(n: number): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return i === 0 ? `${n} B` : `${v.toFixed(1)} ${units[i]}`;
}

export function fmtNum(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

export function basename(p: string): string {
  const parts = p.replace(/\\/g, '/').split('/');
  return parts[parts.length - 1] || p;
}

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export function debounce<T extends (...args: any[]) => void>(fn: T, ms: number): T {
  let t: number | undefined;
  return ((...args: any[]) => {
    window.clearTimeout(t);
    t = window.setTimeout(() => fn(...args), ms);
  }) as T;
}
