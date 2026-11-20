// Inline SVG icon set (24px stroke icons, feather-style) + the IRIXEN mark.

const P = {
  files: '<path d="M4 4h6l2 2h8v12a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z"/>',
  search: '<circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/>',
  git: '<circle cx="6" cy="6" r="2.6"/><circle cx="6" cy="18" r="2.6"/><circle cx="18" cy="9" r="2.6"/><path d="M6 8.6v6.8"/><path d="M18 11.6c0 3.2-2.4 4.4-6 4.4H8.4"/>',
  skills: '<path d="M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9z"/><path d="M18 15l.9 2.1L21 18l-2.1.9L18 21l-.9-2.1L15 18l2.1-.9z"/>',
  stats: '<path d="M4 20V10M10 20V4M16 20v-7M22 20H2"/>',
  settings: '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.6 1.6 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-2.7 1.1V21a2 2 0 1 1-4 0v-.1A1.6 1.6 0 0 0 7.5 19l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1A1.6 1.6 0 0 0 3.6 13H3a2 2 0 1 1 0-4h.1a1.6 1.6 0 0 0 1.1-2.7l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1A1.6 1.6 0 0 0 10 3.6V3a2 2 0 1 1 4 0v.1a1.6 1.6 0 0 0 2.7 1.1l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1A1.6 1.6 0 0 0 20.4 10H21a2 2 0 1 1 0 4h-.1a1.6 1.6 0 0 0-1.5 1z"/>',
  chat: '<path d="M21 12a8 8 0 0 1-8 8H8l-5 3V12a8 8 0 0 1 8-8h2a8 8 0 0 1 8 8z"/>',
  send: '<path d="M12 19V5"/><path d="M5 12l7-7 7 7"/>',
  stop: '<rect x="6" y="6" width="12" height="12" rx="2"/>',
  chevron: '<path d="M9 6l6 6-6 6"/>',
  down: '<path d="M6 9l6 6 6-6"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  refresh: '<path d="M21 12a9 9 0 1 1-3-6.7"/><path d="M21 4v5h-5"/>',
  trash: '<path d="M4 7h16"/><path d="M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"/><path d="M6 7l1 13a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1l1-13"/>',
  close: '<path d="M6 6l12 12M18 6L6 18"/>',
  check: '<path d="M4 12.5l5 5L20 6.5"/>',
  terminal: '<path d="M4 5h16a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1z"/><path d="M7 10l2.5 2L7 14"/><path d="M12.5 15H17"/>',
  copy: '<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M6 15V6a2 2 0 0 1 2-2h8"/>',
  clip: '<path d="M20 11l-8.5 8.5a5 5 0 0 1-7-7L13 4a3.5 3.5 0 0 1 5 5l-8.5 8.5a1.8 1.8 0 0 1-2.5-2.5L15 6.5"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M2 12h2M20 12h2M5 5l1.5 1.5M17.5 17.5L19 19M19 5l-1.5 1.5M6.5 17.5L5 19"/>',
  moon: '<path d="M20 14.5A8.5 8.5 0 0 1 9.5 4 8.5 8.5 0 1 0 20 14.5z"/>',
  folderOpen: '<path d="M3 8V6a2 2 0 0 1 2-2h4l2 2h6a2 2 0 0 1 2 2v1"/><path d="M3 8h17l-2 10a2 2 0 0 1-2 1.6H5A2 2 0 0 1 3 18z"/>',
  file: '<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>',
  branch: '<circle cx="6" cy="6" r="2.5"/><circle cx="6" cy="18" r="2.5"/><circle cx="18" cy="8" r="2.5"/><path d="M6 8.5v7"/><path d="M18 10.5c0 3.5-3 4.5-6 4.5H8.5"/>',
  zap: '<path d="M13 2L4 14h6l-1 8 9-12h-6l1-8z"/>',
  cpu: '<rect x="6" y="6" width="12" height="12" rx="2"/><path d="M9 2v3M15 2v3M9 19v3M15 19v3M2 9h3M2 15h3M19 9h3M19 15h3"/>',
  book: '<path d="M4 5a2 2 0 0 1 2-2h13v16H6a2 2 0 0 0-2 2z"/><path d="M4 19a2 2 0 0 1 2-2h13"/>',
  alert: '<path d="M12 4l9 16H3z"/><path d="M12 10v5M12 18v.5"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 8v.5"/>',
  dot: '<circle cx="12" cy="12" r="3" fill="currentColor" stroke="none"/>',
  panel: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 14h18"/>',
  spark: '<path d="M12 3v6M12 15v6M3 12h6M15 12h6"/><circle cx="12" cy="12" r="2.5"/>',
  history: '<path d="M3 12a9 9 0 1 0 3-6.7"/><path d="M3 4v5h5"/><path d="M12 8v4l3 2"/>',
  layout: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16M3 10h18"/>',
  // review: a diff under a magnifier — "审当前改动"
  review: '<path d="M4 5h9M4 9h6M4 13h9M4 17h5"/><circle cx="16.5" cy="14.5" r="3.8"/><path d="M19.3 17.3L22 20"/>',
  // shield with a check — the security audit entry point
  shield: '<path d="M12 3l7 3v5.5c0 4.4-2.9 7.4-7 9-4.1-1.6-7-4.6-7-9V6z"/><path d="M9 11.8l2.2 2.2L15.5 9.6"/>',
  // closed padlock — every write and command asks first
  lock: '<rect x="4.5" y="10" width="15" height="10" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3"/>',
  // open padlock — the workspace boundary is gone, not just unprompted
  unlock: '<rect x="4.5" y="10" width="15" height="10" rx="2"/><path d="M8 10V7a4 4 0 0 1 7.6-1.7"/>',
  // plug: the bridge endpoint
  plug: '<path d="M9 3v5M15 3v5"/><path d="M7 8h10v3a5 5 0 0 1-10 0z"/><path d="M12 16v5"/>',
  // puzzle: third-party extensions
  puzzle: '<path d="M10 4a2 2 0 1 1 4 0v1.5h3.5A1.5 1.5 0 0 1 19 7v3.5h1a2 2 0 1 1 0 4h-1V18a1.5 1.5 0 0 1-1.5 1.5H14v-1a2 2 0 1 0-4 0v1H6.5A1.5 1.5 0 0 1 5 18v-3.5H4a2 2 0 1 1 0-4h1V7a1.5 1.5 0 0 1 1.5-1.5H10z"/>',
  download: '<path d="M12 4v11"/><path d="M6 11l6 6 6-6"/><path d="M4 20h16"/>',
  play: '<path d="M7 4.5l12 7.5-12 7.5z"/>',
  external: '<path d="M14 4h6v6"/><path d="M20 4l-7.5 7.5"/><path d="M18 14v4a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4"/>',
  store: '<path d="M4.5 8h15l-1.4 11a2 2 0 0 1-2 1.8H7.9a2 2 0 0 1-2-1.8z"/><path d="M8.8 8V6.4a3.2 3.2 0 0 1 6.4 0V8"/>',
  command: '<path d="M4 6h6M14 6h6M4 12h10M18 12h2M4 18h6M14 18h6"/>',
};

export type IconName = keyof typeof P;

export function icon(name: IconName, size = 16): SVGSVGElement {
  const tpl = document.createElement('template');
  // Icons here are always decorative: the control that owns them carries the
  // label. Without `aria-hidden` the SVG joins the accessibility tree as an
  // unnamed graphic, and auditing tools report the *button* as nameless even
  // when it has text or a title.
  tpl.innerHTML =
    `<svg viewBox="0 0 24 24" width="${size}" height="${size}" fill="none" stroke="currentColor" ` +
    `stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round" ` +
    `aria-hidden="true" focusable="false">${P[name]}</svg>`;
  return tpl.content.firstElementChild as SVGSVGElement;
}

/** The IRIXEN app mark used in the titlebar / editor empty state.
 *
 *  `public/irixen-icon.png` is generated from the same master as the native
 *  icons (see scripts/gen-icons.mjs). The boot splash already loads it, so the
 *  renderer hits its cache rather than paying for a second inlined bitmap.
 *  Blue artwork on a transparent plate works on both themes; never invert it. */
export function mark(size = 20): HTMLImageElement {
  const img = document.createElement('img');
  img.src = '/irixen-icon.png';
  img.alt = 'IRIXEN Code';
  img.draggable = false;
  img.width = size;
  img.height = size;
  img.style.cssText = `display:block;width:${size}px;height:${size}px;object-fit:contain;`;
  return img;
}
