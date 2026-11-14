// Extension-contributed context menus.
//
// VS Code lets an extension add rows to `editor/context`, `explorer/context` and
// friends through `contributes.menus`, gated by a `when` clause. We do not have
// a VS Code context-key engine, but the clauses these rows actually use are a
// handful of one-line comparisons against the resource being clicked — so this
// module evaluates that subset and hands the renderer plain menu rows.
//
// Unknown keys resolve to `false`: showing an item that does not apply (a
// "Deploy" row on a folder with no manifest) is worse than hiding one it does.

import { convertFileSrc } from '@tauri-apps/api/core';
import { app } from './state';

/** Any `contributes.menus` key; the three built-in anchors are the common case. */
export type MenuLocation = 'editor/context' | 'explorer/context' | 'view/item/context' | (string & {});

export interface MenuTarget {
  /** Language id of the file being right-clicked. */
  lang?: string;
  /** Absolute path of the resource. */
  path?: string;
  isDir?: boolean;
  /** True when the menu is anchored inside a text editor. */
  editor?: boolean;
  /** `view` / `viewItem` context keys, for `view/title` and `view/item/context`. */
  view?: string;
  viewItem?: string;
}

export interface ExtMenuRow {
  /** Display label, already prefixed with the extension's command category. */
  label: string;
  /** `ext:<command>` — pickers hand this straight back to us. */
  value: string;
  icon?: string;
  /** One group per extension, so rows from different packages stay together. */
  group: string;
  command: string;
}

/** Path → URL the renderer can actually load (asset protocol). */
export function fileUrl(path?: string): string | undefined {
  if (!path) return undefined;
  try {
    return convertFileSrc(path);
  } catch {
    return undefined;
  }
}

function baseName(path: string): string {
  return path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path;
}

function dirName(path: string): string {
  return path.replace(/[\\/]+[^\\/]+$/, '');
}

function contextValue(key: string, target: MenuTarget): string | undefined {
  const path = target.path || '';
  switch (key) {
    case 'resourceLangId':
    case 'editorLangId':
      return target.lang;
    case 'resourceExtname': {
      const name = baseName(path);
      const dot = name.lastIndexOf('.');
      return dot > 0 ? name.slice(dot) : '';
    }
    case 'resourceFilename':
    case 'resourceBasename':
      return path ? baseName(path) : undefined;
    case 'resourceDirname':
      return path ? dirName(path) : undefined;
    case 'resourcePath':
    case 'resourceFsPath':
      return path || undefined;
    case 'resourceScheme':
      return path ? 'file' : undefined;
    case 'resource':
      return path || undefined;
    case 'view':
      return target.view;
    case 'viewItem':
      return target.viewItem;
    default:
      return undefined;
  }
}

function compare(left: string, op: string, raw: string): boolean {
  const l = left.toLowerCase();
  // VS Code writes regex clauses as `/pattern/flags`. Lowercasing the whole
  // right-hand side (as the old code did) turns `\S` into `\s` and breaks the
  // match, and feeding the surrounding slashes to RegExp never matches at all.
  const r = raw.trim().replace(/^['"]|['"]$/g, '');
  switch (op) {
    case '==':
    case '=':
      return l === r.toLowerCase();
    case '!=':
      return l !== r.toLowerCase();
    case '=~': {
      const lit = /^\/(.*)\/([a-zA-Z]*)$/.exec(r);
      const pattern = lit ? lit[1] : r;
      let flags = lit ? lit[2] || 'i' : 'i';
      if (!flags.includes('i')) flags += 'i';
      try {
        return new RegExp(pattern, flags).test(left);
      } catch {
        return false;
      }
    }
    default:
      return false;
  }
}

function evalTerm(term: string, target: MenuTarget): boolean {
  const s = term.trim();
  if (!s) return true;
  if (s.startsWith('!')) return !evalTerm(s.slice(1), target);
  if (s === 'true') return true;
  if (s === 'false') return false;

  // Bare predicates: only the ones we can answer honestly.
  if (/^[a-zA-Z_][\w.]*$/.test(s)) {
    switch (s) {
      case 'isFileSystemResource':
        return !!target.path;
      case 'resourceIsFile':
      case 'isFile':
        return !!target.path && !target.isDir;
      case 'resourceIsDirectory':
        return !!target.isDir;
      case 'editorFocus':
      case 'editorIsOpen':
        return !!target.editor;
      case 'workspaceFolderCount':
        return true;
      default:
        return false;
    }
  }

  const m = /^([\w.]+)\s*(==|!=|=~)\s*(.+)$/.exec(s);
  if (!m) return false;
  const value = contextValue(m[1], target);
  if (value === undefined) return false;
  return compare(value, m[2], m[3]);
}

/** `a && b`, `a || b`, parentheses-free — which is what extensions write. */
export function evalWhen(when: string, target: MenuTarget): boolean {
  const clause = (when || '').trim();
  if (!clause) return true;
  return clause
    .split('||')
    .some((or) => or.split('&&').every((and) => evalTerm(and, target)));
}

/**
 * Human-readable label for a command id.
 *
 * `registerCommand(id, fn)` carries no title — VS Code only ever gets one from
 * `contributes.commands`. Without this lookup the command palette lists raw ids
 * like `livePreview.start`, which reads as a control that lost its name.
 * `contributes` is already localised by the Rust side (`package.nls.*`).
 */
export function commandLabel(command: string): string {
  for (const info of app.extensions) {
    const decl = info.contributes?.commands?.find((c) => c.command === command);
    if (decl?.title) return decl.category ? `${decl.category}: ${decl.title}` : decl.title;
  }
  const runtime = app.extCommands.find((c) => c.id === command);
  if (runtime?.title && runtime.title !== command) {
    return runtime.category ? `${runtime.category}: ${runtime.title}` : runtime.title;
  }
  return command;
}

function titleFor(command: string): { label: string; icon?: string } {
  let icon: string | undefined;
  for (const info of app.extensions) {
    const decl = info.contributes?.commands?.find((c) => c.command === command);
    if (decl) {
      icon = fileUrl(decl.icon);
      break;
    }
  }
  return { label: commandLabel(command), icon };
}

/**
 * Rows an extension contributes to `location`, filtered for `target` and ready
 * for `popupMenu`. The first row of each extension carries its `group`, so the
 * menu can draw one divider between packages.
 */
export function extMenuRows(location: MenuLocation, target: MenuTarget): ExtMenuRow[] {
  const rows: ExtMenuRow[] = [];
  for (const info of app.extensions) {
    if (!info.enabled) continue;
    const menus = info.contributes?.menus || [];
    const submenus = info.contributes?.submenus || [];
    const emit = (command: string, prefix?: string) => {
      const { label, icon } = titleFor(command);
      rows.push({
        label: prefix ? `${prefix}: ${label}` : label,
        value: `ext:${command}`,
        icon,
        group: info.id,
        command,
      });
    };
    for (const row of menus) {
      if (row.menu !== location) continue;
      // A submenu row has no `command` at all. Old builds dropped it, leaving
      // an empty label in the menu; we now expand the referenced submenu's
      // child rows in place under the submenu's own label.
      if (!evalWhen(row.when, target)) continue;
      if (row.command) {
        emit(row.command);
        continue;
      }
      if (!row.submenu) continue;
      const label = submenus.find((s) => s.id === row.submenu)?.label || row.submenu;
      for (const child of menus) {
        if (child.menu !== row.submenu || !child.command) continue;
        if (!evalWhen(child.when, target)) continue;
        emit(child.command, label);
      }
    }
  }
  return rows;
}

/**
 * Whether `contributes.menus.commandPalette` explicitly hides a command.
 *
 * Gradle (and other hosts with internal RPC commands) declares dozens of
 * `{"command": "…", "when": "false"}` rows. Without this the palette fills up
 * with `_gradle.*` internals that cannot be run by hand. Only constant-false
 * clauses are honoured: a `when` referencing `view`/`viewItem`/… describes a
 * context we do not have, and must not silently remove a working command.
 */
export function commandPaletteHidden(command: string): boolean {
  for (const info of app.extensions) {
    if (!info.enabled) continue;
    for (const row of info.contributes?.menus || []) {
      if (row.menu !== 'commandPalette' || row.command !== command) continue;
      if (/^(false|0|!true)$/i.test((row.when || '').trim())) return true;
    }
  }
  return false;
}

/** True when `value` came from `extMenuRows`. */
export function isExtMenuValue(value: string): boolean {
  return value.startsWith('ext:');
}
