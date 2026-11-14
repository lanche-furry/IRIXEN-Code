'use strict';
/*
 * IRIXEN Code — Node extension host bootstrap.
 *
 * Runs inside a real `node` process, loads one VS Code extension and gives it a
 * `vscode` module whose methods are proxied back to the Rust core over NDJSON on
 * stdin/stdout. The core owns the UI, the filesystem and the workspace, so the
 * shim only has to describe what the extension asked for — never how to do it.
 *
 * Ground rules:
 *   * stdout belongs to the protocol. Anything the extension logs is pushed to
 *     stderr, which the core records in the output panel.
 *   * Every call that needs the host is async under the hood, but the API is
 *     shaped like the real one (sync getters on cached state, promises for I/O).
 *   * Nothing here is a stub that silently lies: unsupported surface throws a
 *     clear "not supported by this host" error instead of returning undefined.
 */

const path = require('path');
const fs = require('fs');
const Module = require('module');
const { pathToFileURL } = require('url');

/* ------------------------------------------------------------------ transport */

const rawWrite = process.stdout.write.bind(process.stdout);
function sendLine(obj) {
  try {
    rawWrite(JSON.stringify(obj) + '\n');
  } catch {
    /* the pipe is gone; the process is about to die anyway */
  }
}

// Extensions (and their dependencies) write to stdout all the time. That would
// shred the framing, so everything that is not our own `sendLine` goes to
// stderr, where the core turns it into output-channel lines.
process.stdout.write = function (chunk, enc, cb) {
  return process.stderr.write(chunk, enc, cb);
};
for (const level of ['log', 'info', 'warn', 'error', 'debug', 'trace']) {
  const toStderr = (...args) =>
    process.stderr.write(
      args
        .map((a) => {
          if (typeof a === 'string') return a;
          if (a instanceof Error) return a.stack || a.message;
          try {
            return JSON.stringify(a);
          } catch {
            return String(a);
          }
        })
        .join(' ') + '\n',
    );
  console[level] = toStderr;
}

let seq = 1;
const pending = new Map(); // id -> { resolve, reject }

function request(method, params) {
  const id = seq++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    sendLine({ t: 'req', id, method, params });
    // A hung UI prompt must not wedge the extension forever.
    setTimeout(() => {
      if (pending.delete(id)) reject(new Error(`host call timed out: ${method}`));
    }, 10 * 60 * 1000).unref();
  });
}

/** Fire-and-forget: id 0 means "no reply expected". */
function notify(method, params) {
  sendLine({ t: 'req', id: 0, method, params });
}

function log(level, text) {
  notify('log', { level, text: String(text).slice(0, 8000) });
}

const CFG = (() => {
  try {
    return JSON.parse(process.env.IRIXEN_HOST_CFG || '{}');
  } catch {
    return {};
  }
})();

/* --------------------------------------------------------------- primitives */

class Disposable {
  constructor(fn) {
    this._fn = fn;
  }
  dispose() {
    if (this._fn) {
      const fn = this._fn;
      this._fn = null;
      try {
        fn();
      } catch (e) {
        log('err', e && e.stack ? e.stack : e);
      }
    }
  }
  static from(...items) {
    return new Disposable(() => items.forEach((i) => i && i.dispose && i.dispose()));
  }
}

class EventEmitter {
  constructor() {
    this._listeners = new Set();
    this.event = (listener, thisArg, disposables) => {
      const bound = thisArg ? listener.bind(thisArg) : listener;
      this._listeners.add(bound);
      const d = new Disposable(() => this._listeners.delete(bound));
      if (Array.isArray(disposables)) disposables.push(d);
      return d;
    };
  }
  fire(value) {
    for (const l of [...this._listeners]) {
      try {
        l(value);
      } catch (e) {
        log('err', e && e.stack ? e.stack : e);
      }
    }
  }
  dispose() {
    this._listeners.clear();
  }
}

// Paths are kept raw internally (`fsPath`, document keys, `path.join` all need
// the real filesystem path); percent-encoding happens only at the wire boundary.
// Encoding is per segment so `/` survives, and well-formed `%XX` escapes are
// left alone so `Uri.parse(uri.toString())` is stable.
function encodePath(p) {
  const out = [];
  for (const seg of String(p).split('/')) {
    const chars = Array.from(seg);
    let s = '';
    for (let i = 0; i < chars.length; i++) {
      const ch = chars[i];
      if (ch === '%' && /^[0-9A-Fa-f]{2}$/.test((chars[i + 1] || '') + (chars[i + 2] || ''))) {
        s += ch + chars[i + 1] + chars[i + 2];
        i += 2;
        continue;
      }
      if (/^[A-Za-z0-9\-._~!$&'()*+,;=:@]$/.test(ch)) { s += ch; continue; }
      try { s += encodeURIComponent(ch).toUpperCase(); }
      catch { s += ch; }
    }
    out.push(s);
  }
  return out.join('/');
}
function decodePath(p) {
  try { return decodeURIComponent(p); } catch { return p; }
}

class Uri {
  constructor(scheme, authority, pathname, query, fragment) {
    this.scheme = scheme;
    this.authority = authority || '';
    // Raw, not percent-encoded, at every internal boundary: `fsPath`, document
    // keys and `path.join` all expect the real filesystem path. Encoding lives
    // only at the wire format (`toString` / `parse`).
    this.path = pathname || '';
    this.query = query || '';
    this.fragment = fragment || '';
  }
  static file(p) {
    let s = String(p).replace(/\\/g, '/');
    // UNC: \\server\share\dir → file://server/share/dir
    const unc = /^\/\/([^/]+)(\/.*)?$/.exec(s);
    if (unc) return new Uri('file', unc[1], unc[2] || '/', '', '');
    if (!s.startsWith('/')) s = '/' + s;
    return new Uri('file', '', s, '', '');
  }
  static parse(value) {
    const m = /^([a-zA-Z][a-zA-Z0-9+.-]*):(?:\/\/([^/?#]*))?([^?#]*)(?:\?([^#]*))?(?:#(.*))?$/.exec(String(value));
    if (!m) return Uri.file(value);
    return new Uri(m[1].toLowerCase(), m[2] || '', decodePath(m[3] || ''), m[4] || '', m[5] || '');
  }
  static joinPath(base, ...parts) {
    let p = base.path;
    for (const part of parts) p = p.replace(/\/$/, '') + '/' + String(part).replace(/^\//, '');
    return base.with({ path: p });
  }
  get fsPath() {
    // UNC: authority + path stays a usable Node path on Windows.
    if (this.scheme === 'file' && this.authority) return '//' + this.authority + this.path;
    // Windows: /c:/dir → c:/dir
    if (/^\/[a-zA-Z]:/.test(this.path)) return this.path.slice(1);
    return this.path;
  }
  with(change) {
    return new Uri(
      change.scheme ?? this.scheme,
      change.authority ?? this.authority,
      change.path ?? this.path,
      change.query ?? this.query,
      change.fragment ?? this.fragment,
    );
  }
  toString() {
    const auth = this.authority ? `//${this.authority}` : this.scheme === 'file' ? '//' : '';
    // Percent-encode per segment and keep the slashes. Extensions hand this
    // string to other runtimes — the Gradle importer does `java.net.URI.create`
    // on it — and a bare space throws "Illegal character in path at index 17".
    return `${this.scheme}:${auth}${encodePath(this.path)}${this.query ? '?' + this.query : ''}${this.fragment ? '#' + this.fragment : ''}`;
  }
  toJSON() {
    return { $mid: 1, scheme: this.scheme, authority: this.authority, path: this.path, query: this.query, fragment: this.fragment };
  }
  static revive(data) {
    if (data instanceof Uri) return data;
    if (typeof data === 'string') return Uri.parse(data);
    if (data && typeof data === 'object' && data.scheme) {
      return new Uri(data.scheme, data.authority, data.path, data.query, data.fragment);
    }
    return Uri.file(String(data));
  }
}

class Position {
  constructor(line, character) {
    this.line = Math.max(0, line | 0);
    this.character = Math.max(0, character | 0);
  }
  isBefore(o) {
    return this.line < o.line || (this.line === o.line && this.character < o.character);
  }
  isBeforeOrEqual(o) {
    return this.isBefore(o) || this.isEqual(o);
  }
  isAfter(o) {
    return !this.isBeforeOrEqual(o);
  }
  isAfterOrEqual(o) {
    return !this.isBefore(o);
  }
  isEqual(o) {
    return this.line === o.line && this.character === o.character;
  }
  compareTo(o) {
    return this.isBefore(o) ? -1 : this.isEqual(o) ? 0 : 1;
  }
  translate(dl = 0, dc = 0) {
    return new Position(this.line + dl, this.character + dc);
  }
  with(line, character) {
    return new Position(line === undefined ? this.line : line, character === undefined ? this.character : character);
  }
}

class Range {
  constructor(a, b, c, d) {
    if (typeof a === 'number') {
      this.start = new Position(a, b);
      this.end = new Position(c, d);
    } else {
      this.start = a;
      this.end = b;
    }
  }
  get isEmpty() {
    return this.start.isEqual(this.end);
  }
  get isSingleLine() {
    return this.start.line === this.end.line;
  }
  contains(pos) {
    return !pos.isBefore(this.start) && !pos.isAfter(this.end);
  }
  with(s, e) {
    return new Range(s ?? this.start, e ?? this.end);
  }
}

class Selection extends Range {
  constructor(a, b, c, d) {
    super(a, b, c, d);
    this.anchor = this.start;
    this.active = this.end;
  }
  get isReversed() {
    return this.anchor.isAfter(this.active);
  }
}

class Location {
  constructor(uri, rangeOrPos) {
    this.uri = Uri.revive(uri);
    this.range = rangeOrPos instanceof Range ? rangeOrPos : new Range(rangeOrPos, rangeOrPos);
  }
}

class ThemeColor {
  constructor(id) {
    this.id = id;
  }
}
class ThemeIcon {
  constructor(id, color) {
    this.id = id;
    this.color = color;
  }
}
class MarkdownString {
  constructor(value = '', supportThemeIcons = false) {
    this.value = value;
    this.isTrusted = undefined;
    this.supportThemeIcons = supportThemeIcons;
  }
  appendText(v) {
    this.value += v;
    return this;
  }
  appendMarkdown(v) {
    this.value += v;
    return this;
  }
  appendCodeblock(v, lang = '') {
    this.value += `\n\`\`\`${lang}\n${v}\n\`\`\`\n`;
    return this;
  }
}

/* --------------------------------------------------------------- documents */

class TextLine {
  constructor(lineNumber, text) {
    this.lineNumber = lineNumber;
    this.text = text;
    this.range = new Range(lineNumber, 0, lineNumber, text.length);
    this.rangeIncludingLineBreak = this.range;
    this.firstNonWhitespaceCharacterIndex = text.length - text.replace(/^\s+/, '').length;
    this.isEmptyOrWhitespace = text.trim().length === 0;
  }
}

class TextDocument {
  constructor(uri, languageId, version, text, dirty, selection) {
    this.uri = uri;
    this.languageId = languageId || 'plaintext';
    this.version = version || 1;
    this._text = text || '';
    this._lines = this._text.split('\n');
    this.isDirty = !!dirty;
    this.isUntitled = false;
    this.fileName = uri.fsPath;
    this.selection = selection
      ? new Selection(
          new Position(selection.start.line, selection.start.character),
          new Position(selection.end.line, selection.end.character),
        )
      : new Selection(0, 0, 0, 0);
  }
  get lineCount() {
    return this._lines.length;
  }
  get eol() {
    return 1;
  }
  getText(range) {
    if (!range) return this._text;
    const start = this.offsetAt(range.start);
    const end = this.offsetAt(range.end);
    return this._text.slice(start, end);
  }
  lineAt(lineOrPos) {
    const n = typeof lineOrPos === 'number' ? lineOrPos : lineOrPos.line;
    const idx = Math.min(Math.max(0, n), this._lines.length - 1);
    return new TextLine(idx, this._lines[idx]);
  }
  offsetAt(pos) {
    const line = Math.min(Math.max(0, pos.line), this._lines.length - 1);
    let off = 0;
    for (let i = 0; i < line; i++) off += this._lines[i].length + 1;
    return off + Math.min(Math.max(0, pos.character), this._lines[line].length);
  }
  positionAt(offset) {
    let rest = Math.max(0, Math.min(offset, this._text.length));
    for (let i = 0; i < this._lines.length; i++) {
      if (rest <= this._lines[i].length) return new Position(i, rest);
      rest -= this._lines[i].length + 1;
    }
    const last = this._lines.length - 1;
    return new Position(last, this._lines[last].length);
  }
  getWordRangeAtPosition(pos) {
    const line = this._lines[pos.line] || '';
    const re = /[A-Za-z0-9_$]+/g;
    let m;
    while ((m = re.exec(line))) {
      if (m.index <= pos.character && pos.character <= m.index + m[0].length) {
        return new Range(pos.line, m.index, pos.line, m.index + m[0].length);
      }
    }
    return undefined;
  }
  validateRange(r) {
    return r;
  }
  validatePosition(p) {
    return p;
  }
  save() {
    notify('doc.save', { path: this.uri.fsPath });
    return Promise.resolve(true);
  }
}

const openDocs = new Map(); // fsPath -> TextDocument

/* ------------------------------------------------------------- editor glue */

class TextEditor {
  constructor(doc, selection) {
    this.document = doc;
    this.selections = [selection || new Selection(0, 0, 0, 0)];
    this.options = { tabSize: 2, insertSpaces: true };
    this.viewColumn = 1;
    this.visibleRanges = [new Range(0, 0, doc.lineCount - 1, 0)];
  }
  get selection() {
    return this.selections[0];
  }
  set selection(v) {
    this.selections = [v];
  }
  edit(callback) {
    const edits = [];
    const builder = {
      replace(range, text) {
        edits.push({ range, text });
      },
      insert(pos, text) {
        edits.push({ range: new Range(pos, pos), text });
      },
      delete(range) {
        edits.push({ range, text: '' });
      },
      setEndOfLine() {},
    };
    try {
      callback(builder);
    } catch (e) {
      log('err', e && e.stack ? e.stack : e);
      return Promise.resolve(false);
    }
    if (edits.length === 0) return Promise.resolve(true);
    return request('editor.edit', {
      path: this.document.uri.fsPath,
      edits: edits.map((e) => ({
        start: { line: e.range.start.line, character: e.range.start.character },
        end: { line: e.range.end.line, character: e.range.end.character },
        text: e.text,
      })),
    })
      .then((ok) => ok !== false)
      .catch(() => false);
  }
  revealRange() {}
  setDecorations() {}
  insertSnippet() {
    return Promise.resolve(true);
  }
  show() {}
  hide() {}
}

/* --------------------------------------------------------------- the shim */

const commandsMap = new Map();

function messageFn(severity) {
  return (message, ...rest) => {
    const options = rest.length && typeof rest[rest.length - 1] === 'object' && !Array.isArray(rest[rest.length - 1]) ? rest.pop() : {};
    const items = rest.flat().filter((x) => x !== undefined && x !== null);
    return request('window.message', {
      severity,
      message: typeof message === 'string' ? message : String(message),
      items,
      detail: options.detail || '',
      modal: !!options.modal,
    });
  };
}

const statusBarItems = new Map();

function createStatusBarItem(alignment, priority) {
  const id = `sb${Math.random().toString(36).slice(2)}`;
  const state = { id, text: '', tooltip: '', command: '', visible: false, alignment: alignment || 1, priority: priority || 0 };
  statusBarItems.set(id, state);
  const push = () => notify('statusbar', state);
  const item = {
    get text() {
      return state.text;
    },
    set text(v) {
      state.text = v == null ? '' : String(v);
      push();
    },
    get tooltip() {
      return state.tooltip;
    },
    set tooltip(v) {
      state.tooltip = v == null ? '' : String(v);
      push();
    },
    get command() {
      return state.command;
    },
    set command(v) {
      state.command = v || '';
      push();
    },
    get color() {
      return undefined;
    },
    set color(_v) {},
    get backgroundColor() {
      return undefined;
    },
    set backgroundColor(_v) {},
    alignment: state.alignment,
    priority: state.priority,
    show() {
      state.visible = true;
      push();
    },
    hide() {
      state.visible = false;
      push();
    },
    dispose() {
      statusBarItems.delete(id);
      notify('statusbar.remove', { id });
    },
  };
  return item;
}

const outputChannels = new Map();
function createOutputChannel(name, _options) {
  const channel = String(name || 'extension');
  outputChannels.set(channel, true);
  return {
    name: channel,
    append(v) {
      notify('output', { channel, text: String(v) });
    },
    appendLine(v) {
      notify('output', { channel, text: String(v) + '\n' });
    },
    replace(v) {
      notify('output.replace', { channel, text: String(v) });
    },
    clear() {
      notify('output.clear', { channel });
    },
    show() {},
    hide() {},
    dispose() {},
  };
}

/** `contributes.configuration.properties[key].default` from our own manifest.
 *  Extensions rely on these: prettier, for instance, disables itself entirely
 *  when `prettier.enable` comes back undefined. VS Code merges these defaults
 *  before an extension ever reads a setting. */
function configDefault(full, typeFallback = false) {
  const raw = (CFG.package && CFG.package.contributes && CFG.package.contributes.configuration) || null;
  const groups = Array.isArray(raw) ? raw : raw ? [raw] : [];
  for (const g of groups) {
    const props = g && g.properties;
    if (props && Object.prototype.hasOwnProperty.call(props, full)) {
      const p = props[full];
      if (p && Object.prototype.hasOwnProperty.call(p, 'default')) return p.default;
      // Manifests often declare `type: array` / `type: object` without an
      // explicit `default` (Prettier's `prettier.documentSelectors` is one).
      // VS Code still hands back [] / {} there; returning undefined makes
      // `const { documentSelectors } = config; documentSelectors.map(...)`
      // throw on every editor change. Only these two container types get the
      // schema default — scalars keep `undefined` so caller defaults work.
      if (typeFallback && p) {
        if (p.type === 'array') return [];
        if (p.type === 'object') return {};
      }
      return undefined;
    }
  }
  return undefined;
}

const configCache = {};

function getConfiguration(section) {
  const fullOf = (key) => (section ? `${section}.${key}` : key);
  const load = (full) => {
    request('config.get', { section: full })
      .then((v) => {
        // Only cache real values. Caching `null` would make a later `.get()`
        // shadow the manifest default with an undefined.
        if (v !== undefined && v !== null) configCache[full] = v;
      })
      .catch(() => {});
  };
  const target = {
    get: (key, def) => {
      const full = fullOf(key);
      if (Object.prototype.hasOwnProperty.call(configCache, full)) return configCache[full];
      load(full);
      const fromManifest = configDefault(full);
      if (fromManifest !== undefined) return fromManifest;
      if (def !== undefined) return def;
      return configDefault(full, true);
    },
    has: (key) => {
      const full = fullOf(key);
      return Object.prototype.hasOwnProperty.call(configCache, full) || configDefault(full) !== undefined;
    },
    inspect: (key) => {
      const full = fullOf(key);
      return {
        key: full,
        defaultValue: configDefault(full),
        globalValue: undefined,
        workspaceValue: undefined,
        workspaceFolderValue: undefined,
      };
    },
    update: () => Promise.resolve(),
  };
  // VS Code's WorkspaceConfiguration exposes settings as plain properties too:
  // `const { enable } = workspace.getConfiguration('prettier')` is common in
  // real extensions. A Proxy maps any unknown key to its configured value.
  return new Proxy(target, {
    get(t, k) {
      if (typeof k !== 'string' || Object.prototype.hasOwnProperty.call(t, k)) {
        return Reflect.get(t, k);
      }
      const full = fullOf(k);
      if (Object.prototype.hasOwnProperty.call(configCache, full)) return configCache[full];
      load(full);
      const fromManifest = configDefault(full);
      return fromManifest !== undefined ? fromManifest : configDefault(full, true);
    },
    has: () => true,
  });
}

function makeStateAreas(raw) {
  const store = { global: raw.global || {}, workspace: raw.workspace || {} };
  const area = (key) => ({
    get(k, def) {
      return Object.prototype.hasOwnProperty.call(store[key], k) ? store[key][k] : def;
    },
    has(k) {
      return Object.prototype.hasOwnProperty.call(store[key], k);
    },
    keys() {
      return Object.keys(store[key]);
    },
    update(k, value) {
      if (value === undefined) delete store[key][k];
      else store[key][k] = value;
      return request('state.set', { scope: key, key: k, value: value === undefined ? null : value });
    },
    setKeysForSync() {},
  });
  return { globalState: area('global'), workspaceState: area('workspace') };
}

const diagnosticCollections = new Set();

/* ------------------------------------------------------- language providers */
//
// The provider *function* stays in this process: the core sends a reverse call
// with the document text and the cursor position, and gets plain JSON back.
// That is what makes a real language extension (Prettier, ESLint, …) do its job
// instead of only registering something nobody ever invokes.

const providers = new Map(); // providerId -> { kind, provider }
let providerSeq = 1;

function normalizeSelector(sel) {
  const arr = Array.isArray(sel) ? sel : [sel];
  return arr
    .map((s) => {
      if (typeof s === 'string') return { language: s };
      if (!s || typeof s !== 'object') return null;
      const out = {};
      if (s.language) out.language = s.language;
      if (s.scheme) out.scheme = s.scheme;
      const pat = typeof s.pattern === 'string' ? s.pattern : s.pattern && s.pattern.pattern;
      if (pat) out.pattern = pat;
      return out;
    })
    .filter(Boolean);
}

function registerProvider(kind, selector, provider, triggers) {
  const id = `p${providerSeq++}`;
  providers.set(id, { kind, provider });
  notify('providers.put', {
    id,
    kind,
    selector: normalizeSelector(selector),
    triggerCharacters: triggers || [],
  });
  return new Disposable(() => {
    providers.delete(id);
    notify('providers.remove', { id });
  });
}

/** Plain JSON: providers routinely return class instances and `undefined`. */
function plain(value) {
  if (value === undefined || value === null) return null;
  try {
    return JSON.parse(JSON.stringify(value));
  } catch {
    return null;
  }
}

function docFromWire(d) {
  const path = d.path || '';
  const text = d.text || '';
  const existing = openDocs.get(path);
  if (existing && existing._text === text) return existing;
  const doc = new TextDocument(Uri.file(path), d.languageId || 'plaintext', d.version || 1, text, false);
  if (path) openDocs.set(path, doc);
  return doc;
}

/* ------------------------------------------------------------------- trees */
//
// `window.createTreeView(id, { treeDataProvider })` is how extensions fill the
// sidebar views they declare in `contributes.viewsContainers` / `views`.
// Providers return *live objects* — class instances carrying parents, clients
// and caches — which cannot cross the NDJSON boundary. The host keeps those
// objects and gives the renderer opaque keys; expanding a node sends its key
// back and we resolve it to the real element before calling `getChildren`.

const treeProviders = new Map(); // pid -> row
const treeByView = new Map();    // viewId -> pid
let treeSeq = 1;
let treeElementSeq = 1;

function treeText(v) {
  if (v == null) return '';
  if (typeof v === 'string') return v;
  if (typeof v === 'object') {
    if (typeof v.label === 'string') return v.label;
    if (typeof v.value === 'string') return v.value;
  }
  return String(v);
}

function treeIconValue(icon) {
  if (!icon) return null;
  // ThemeIcon is a class here; some bundles ship a structurally identical
  // object from a second copy of the shim, so duck-type it as well.
  if (icon instanceof ThemeIcon || (typeof icon.id === 'string' && !icon.fsPath && !icon.path)) {
    return { themeIcon: String(icon.id) };
  }
  const pick = (v) => {
    if (!v) return '';
    if (typeof v === 'string') return v;
    if (v instanceof Uri) return v.fsPath;
    if (typeof v.fsPath === 'string') return v.fsPath;
    if (typeof v.path === 'string' && v.scheme === 'file') {
      return v.fsPath || v.path.replace(/^\/([a-zA-Z]:)/, '$1');
    }
    return '';
  };
  // `{ light: Uri, dark: Uri }` is the common form; fall back to a bare Uri.
  const found = pick(icon.light) || pick(icon.dark) || pick(icon);
  return found ? { iconPath: found } : null;
}

/** Only presentation fields are copied — never the provider's live element. */
function treeItemWire(item, key) {
  const out = {
    key,
    label: '',
    description: '',
    tooltip: '',
    state: Number(item && item.collapsibleState) || 0,
    context: '',
    path: '',
  };
  if (!item || typeof item !== 'object') return out;
  let res = item.resourceUri;
  if (res && !(res instanceof Uri) && typeof res === 'object' && res.scheme) {
    try { res = Uri.revive(res); } catch { /* keep the raw value */ }
  }
  const path = res && typeof res.fsPath === 'string' ? res.fsPath : '';
  let label = treeText(item.label) || (path ? path.split(/[\\/]/).pop() : '') || treeText(item.id);
  let description = typeof item.description === 'string' ? item.description : '';
  // Some extensions (Gradle's "No tasks found" placeholder) keep the label
  // empty and put the whole text in `description`. Promote it so the renderer
  // never has to fall back to showing an opaque element handle.
  if (!label && description) {
    label = description;
    description = '';
  }
  out.label = label;
  out.description = description;
  out.tooltip = treeText(item.tooltip).slice(0, 800);
  out.context = typeof item.contextValue === 'string' ? item.contextValue : '';
  out.path = path;
  const icon = treeIconValue(item.iconPath || item.icon);
  if (icon) Object.assign(out, icon);
  if (item.command && item.command.command) {
    out.command = { command: String(item.command.command), title: treeText(item.command.title) };
  }
  return out;
}

function makeTreeRow(viewId, provider) {
  const pid = `tree${treeSeq++}`;
  const row = { viewId, pid, provider, elements: new Map(), commands: new Map() };
  treeProviders.set(pid, row);
  treeByView.set(viewId, pid);
  if (provider && typeof provider.onDidChangeTreeData === 'function') {
    try {
      // VS Code returns a Disposable; the host process lives as long as the
      // provider does, so dropping it here is harmless.
      provider.onDidChangeTreeData(() => notify('tree.changed', { viewId, pid }));
    } catch (e) {
      log('err', `tree.onDidChangeTreeData: ${e && e.message ? e.message : e}`);
    }
  }
  notify('tree.register', { viewId, pid });
  return row;
}

function makeTreeView(viewId, provider) {
  const row = makeTreeRow(viewId, provider);
  const events = {
    onDidCollapseElement: new EventEmitter(),
    onDidExpandElement: new EventEmitter(),
    onDidChangeSelection: new EventEmitter(),
    onDidChangeVisibility: new EventEmitter(),
  };
  return {
    get visible() { return true; },
    get selection() { return []; },
    get activeItem() { return undefined; },
    onDidCollapseElement: events.onDidCollapseElement.event,
    onDidExpandElement: events.onDidExpandElement.event,
    onDidChangeSelection: events.onDidChangeSelection.event,
    onDidChangeVisibility: events.onDidChangeVisibility.event,
    reveal: () => Promise.resolve(),
    title: undefined,
    description: undefined,
    message: undefined,
    badge: undefined,
    dispose() {
      if (!treeProviders.has(row.pid)) return;
      if (treeByView.get(viewId) === row.pid) treeByView.delete(viewId);
      treeProviders.delete(row.pid);
      notify('tree.unregister', { viewId, pid: row.pid });
    },
  };
}

function createTreeView(viewId, options) {
  return makeTreeView(String(viewId), options && options.treeDataProvider);
}

function registerTreeDataProvider(viewId, provider) {
  const view = makeTreeView(String(viewId), provider);
  return new Disposable(() => view.dispose());
}

function treeRowFor(params) {
  const row = treeProviders.get(params && params.pid);
  if (row) return row;
  const pid = treeByView.get(params && params.viewId);
  return pid ? treeProviders.get(pid) : undefined;
}

async function treeChildren(row, key) {
  const provider = row && row.provider;
  if (!provider || typeof provider.getChildren !== 'function') return [];
  const element = key ? row.elements.get(key) : undefined;
  const children = await provider.getChildren(element === undefined ? undefined : element);
  let list = [];
  if (Array.isArray(children)) list = children;
  else if (children && typeof children[Symbol.iterator] === 'function') list = [...children];
  const out = [];
  for (const child of list.slice(0, 2000)) {
    if (child == null) continue;
    const ckey = `e${treeElementSeq++}`;
    row.elements.set(ckey, child);
    let item = child;
    if (typeof provider.getTreeItem === 'function') {
      try { item = (await provider.getTreeItem(child)) || child; }
      catch (e) { log('err', `tree.getTreeItem: ${e && e.message ? e.message : e}`); item = child; }
    }
    if (item && typeof item === 'object' && item.command && item.command.command) {
      row.commands.set(ckey, item.command);
    }
    out.push(treeItemWire(item, ckey));
  }
  // The handle table is a cache, not storage: bound it so a long session with
  // thousands of expansions cannot grow without limit.
  if (row.elements.size > 8000) {
    let drop = row.elements.size - 4000;
    for (const k of row.elements.keys()) {
      if (drop-- <= 0) break;
      row.elements.delete(k);
      row.commands.delete(k);
    }
  }
  return out;
}

async function treeRunCommand(row, key, commandId) {
  if (!row) return false;
  const element = row.elements.get(key);
  // A right-click menu command receives the clicked tree item; a TreeItem's
  // own command carries its arguments (often including the live element).
  const cmd = commandId
    ? { command: commandId, arguments: element === undefined ? [] : [element] }
    : row.commands.get(key);
  if (!cmd || !cmd.command) return false;
  const args = Array.isArray(cmd.arguments) ? cmd.arguments : [];
  const fn = commandsMap.get(cmd.command);
  if (fn) await fn(...args);
  else await api.commands.executeCommand(cmd.command, ...args);
  return true;
}

/* --------------------------------------------------------------- webviews */
//
// A webview is an iframe in the renderer. The page cannot see `vscode` — it
// gets an `acquireVsCodeApi()` shim that the renderer injects — so everything
// an extension puts in `webview.html` behaves like it does in VS Code, minus
// the parts that need a real editor (resource roots, custom editors).

const panels = new Map(); // panelId -> { webview, emitter, panel }
let panelSeq = 1;

function createWebviewPanel(viewType, title, _showOptions, options) {
  const id = `wv${panelSeq++}`;
  const emitter = new EventEmitter();
  const stateEmitter = new EventEmitter();
  const disposeEmitter = new EventEmitter();
  let html = '';
  let disposed = false;
  let timer = null;
  let announced = false;

  // `viewType` / `options` travel with every update so the core can rebuild a
  // panel row it has lost (renderer reload, closed panel, host restart) instead
  // of silently dropping the document.
  const describe = () => ({ id, html, title: panel.title, viewType, options: webview.options });
  const flush = () => {
    timer = null;
    if (!disposed) notify('webview.update', describe());
  };
  const webview = {
    get html() {
      return html;
    },
    set html(v) {
      html = String(v == null ? '' : v);
      if (timer) clearTimeout(timer);
      // `create` goes out with an empty body. Delaying the first real document
      // by 50ms raced it against the empty publish — whichever resolved last
      // became the iframe's src, so the panel was blank about half the time.
      if (!announced) {
        announced = true;
        flush();
      } else {
        timer = setTimeout(flush, 50);
      }
    },
    options: options || {},
    cspSource: 'https://file+.vscode-resource.vscode-cdn.net',
    // Resource roots are not served by this host; returning the URI keeps
    // `webview.asWebviewUri(u).toString()` usable for extensions that only
    // stringify it.
    asWebviewUri: (uri) => uri,
    postMessage: (message) => {
      notify('webview.post', { id, message: plain(message) });
      return Promise.resolve(true);
    },
    onDidReceiveMessage: emitter.event,
    get state() {
      return undefined;
    },
    set state(_v) {},
  };
  const panel = {
    viewType,
    title: String(title || viewType),
    webview,
    viewColumn: 1,
    visible: true,
    active: true,
    onDidDispose: disposeEmitter.event,
    onDidChangeViewState: stateEmitter.event,
    reveal() {
      // Extensions keep `currentPanel` for the whole session. Once it exists,
      // they never call createWebviewPanel again — Live Preview only reveals and
      // posts. If the renderer lost the panel (reload, user closed it, host
      // restart), that used to be a silent no-op: the server ran, nothing showed.
      // Ship the full document with the reveal so the core can re-create the row.
      notify('webview.reveal', describe());
      return panel;
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      if (timer) clearTimeout(timer);
      panels.delete(id);
      notify('webview.dispose', { id });
      disposeEmitter.fire();
    },
  };
  panels.set(id, { webview, emitter, panel });
  notify('webview.create', describe());
  return panel;
}

/** `registerWebviewViewProvider`: too little UI to host a real view container,
 *  but resolve the provider so its commands and state are at least alive. */
function registerWebviewViewProvider(viewId, provider) {
  const emitter = new EventEmitter();
  const webview = {
    html: '',
    options: {},
    cspSource: '',
    asWebviewUri: (uri) => uri,
    postMessage: () => Promise.resolve(true),
    onDidReceiveMessage: emitter.event,
  };
  const view = {
    viewType: viewId,
    webview,
    visible: true,
    onDidChangeVisibility: new EventEmitter().event,
    onDidDispose: new EventEmitter().event,
    show() {},
    dispose() {},
  };
  if (provider && typeof provider.resolveWebviewView === 'function') {
    Promise.resolve()
      .then(() => provider.resolveWebviewView(view, { state: undefined }, { isCancellationRequested: false, onCancellationRequested: () => new Disposable() }))
      .catch((e) => log('err', e && e.stack ? e.stack : e));
  }
  return new Disposable();
}


/* --------------------------------------------------------- tasks & terminals */
//
// Build tools do not run themselves. An extension either hands VS Code a `Task`
// or opens a `Terminal` and calls `sendText` — Gradle, Maven, npm and cargo all
// go through one of the two. Both used to be no-ops, so pressing "Run Gradle
// task" started nothing and reported nothing back, which looks exactly like a
// broken extension.
//
// They now run through the core's process runner: same stages, same output
// panel as F5, and a real exit code that turns into `onDidEndTask` /
// `onDidEndTaskProcess`.

const TaskScope = { Global: 1, Workspace: 2 };

class ShellExecution {
  constructor(commandOrLine, argsOrOptions, options) {
    if (typeof commandOrLine === 'string') {
      // (commandLine, options?)
      this.commandLine = commandOrLine;
      this.options = argsOrOptions || {};
    } else {
      // ({ value, quoting }, args?, options?)
      this.command = (commandOrLine && commandOrLine.value) || '';
      this.args = (argsOrOptions || []).map((a) => (typeof a === 'string' ? a : (a && a.value) || ''));
      this.options = options || {};
    }
  }
}

class ProcessExecution {
  constructor(processOrCommand, argsOrOptions, options) {
    if (typeof processOrCommand === 'string') {
      this.process = processOrCommand;
      if (Array.isArray(argsOrOptions)) {
        this.args = argsOrOptions;
        this.options = options || {};
      } else {
        this.args = [];
        this.options = argsOrOptions || {};
      }
    } else {
      this.process = (processOrCommand && processOrCommand.value) || '';
      this.args = (argsOrOptions || []).map((a) => (typeof a === 'string' ? a : (a && a.value) || ''));
      this.options = options || {};
    }
  }
}

class CustomExecution {
  constructor(callback) {
    this.callback = callback;
  }
}

class TaskGroup {
  constructor(id, isDefault) {
    this.id = id;
    this.isDefault = !!isDefault;
  }
}

class Task {
  constructor(definition, scopeOrName, nameOrSource, sourceOrExecution, execution) {
    // Two overloads in the real API:
    //   (definition, name, source, execution)
    //   (definition, scope, name, source, execution)
    if (typeof scopeOrName === 'string') {
      this.definition = definition;
      this.scope = TaskScope.Workspace;
      this.name = scopeOrName;
      this.source = nameOrSource || '';
      this.execution = sourceOrExecution;
    } else {
      this.definition = definition;
      this.scope = scopeOrName || TaskScope.Workspace;
      this.name = nameOrSource || '';
      this.source = sourceOrExecution || '';
      this.execution = execution;
    }
    this.isBackground = false;
    this.presentationOptions = {};
    this.detail = undefined;
    this.group = undefined;
    this.runOptions = definition && definition.runOptions;
  }
}

Task.Group = { Build: new TaskGroup('build'), Test: new TaskGroup('test', true), Rebuild: new TaskGroup('rebuild'), Clean: new TaskGroup('clean') };

/** What the core needs to start a process, from either execution shape. */
function executionToCommand(exec) {
  if (!exec) return null;
  const cwd = cwdOf(exec.options && exec.options.cwd);
  if (exec.commandLine) return { command: exec.commandLine, argv: [], cwd };
  const headOf = (v) => (typeof v === 'string' ? v : (v && v.value) || '');
  const argsOf = (list) => (list || []).map(headOf).filter((s) => s !== '');
  if (exec.command !== undefined) {
    const argv = [headOf(exec.command), ...argsOf(exec.args)].filter(Boolean);
    // `argv` travels as a vector: the core must not re-split it, or an argument
    // with spaces (`-e "console.log(1)"`) would be torn apart.
    return { command: argv.map(quoteArg).join(' '), argv, cwd };
  }
  if (exec.process !== undefined) {
    const argv = [headOf(exec.process), ...argsOf(exec.args)].filter(Boolean);
    return { command: argv.map(quoteArg).join(' '), argv, cwd };
  }
  return null;
}

/** Quote one argv item for our splitter: only spaces and quotes need it. */
function quoteArg(value) {
  const s = String(value);
  return /[\s"]/.test(s) ? `"${s.replace(/"/g, '')}"` : s;
}

function cwdOf(value) {
  if (!value) return '';
  if (typeof value === 'string') return value;
  if (value.fsPath) return value.fsPath;
  if (value.path) return value.path;
  return '';
}

/** Set by `buildVscode` so the reverse channel can report task completion. */
let activeTasks = null;
let taskSeq = 1;

function buildTasksModule() {
  const providers = new Map(); // type -> provider
  const executions = new Map(); // taskId -> TaskExecution
  const startEmitter = new EventEmitter();
  const endEmitter = new EventEmitter();
  const processStartEmitter = new EventEmitter();
  const processEndEmitter = new EventEmitter();

  class TaskExecution {
    constructor(task, runId, command) {
      this.task = task;
      this._runId = runId;
      this._command = command;
      this._ended = false;
    }
    terminate() {
      if (this._runId) request('task.stop', { id: this._runId }).catch(() => {});
    }
    /** Called from the core when the process exits. */
    _finish(code) {
      if (this._ended) return;
      this._ended = true;
      const exitCode = typeof code === 'number' ? code : -1;
      Task_API._executions.delete(this.task._id);
      processEndEmitter.fire({ execution: this, exitCode });
      endEmitter.fire({ execution: this });
    }
  }

  const Task_API = {
    TaskScope,
    Task,
    TaskGroup,
    ShellExecution,
    ProcessExecution,
    CustomExecution,

    registerTaskProvider(type, provider) {
      providers.set(type, provider);
      return new Disposable(() => providers.delete(type));
    },

    /** Ask every provider for its tasks — the same call the real host makes. */
    fetchTasks(filter) {
      const token = { isCancellationRequested: false, onCancellationRequested: () => new Disposable() };
      const wanted = (filter && filter.type) || null;
      const jobs = [];
      for (const [type, provider] of providers) {
        if (wanted && type !== wanted) continue;
        if (!provider || typeof provider.provideTasks !== 'function') continue;
        jobs.push(
          Promise.resolve()
            .then(() => provider.provideTasks(token, filter))
            .catch((e) => {
              log('err', `provideTasks(${type}) 失败：${e && e.message ? e.message : e}`);
              return [];
            }),
        );
      }
      return Promise.all(jobs).then((lists) => lists.flat().filter(Boolean));
    },

    executeTask(task) {
      if (!task || typeof task !== 'object' || !task.execution) {
        return Promise.reject(new Error('executeTask: 需要一个带 execution 的 Task'));
      }
      if (task.execution instanceof CustomExecution) {
        // Custom execution is a pseudoterminal the extension writes into. We
        // have no terminal UI to attach it to, so say so instead of hanging.
        return Promise.reject(new Error('暂不支持自定义执行（CustomExecution）任务'));
      }
      const spec = executionToCommand(task.execution);
      if (!spec || !spec.command) {
        return Promise.reject(new Error('executeTask: 任务没有可执行的命令'));
      }
      const taskId = `t${taskSeq++}`;
      task._id = taskId;
      const cwd = cwdOf(spec.cwd) || cwdOf(task.definition && task.definition.cwd) || CFG.workspace || '';
      const execution = new TaskExecution(task, null, spec.command);
      executions.set(taskId, execution);
      Task_API._executions.set(taskId, execution);

      request('task.run', {
        taskId,
        command: spec.command,
        argv: spec.argv || [],
        cwd,
        name: task.name || '',
        source: task.source || '',
      })
        .then((res) => {
          execution._runId = res && res.id;
          if (!execution._ended) {
            processStartEmitter.fire({ execution });
            startEmitter.fire({ execution });
          }
        })
        .catch((e) => {
          log('err', `任务启动失败：${e && e.message ? e.message : e}`);
          execution._finish(-1);
        });
      return Promise.resolve(execution);
    },

    get taskExecutions() {
      return [...executions.values()].filter((e) => !e._ended);
    },
    onDidStartTask: startEmitter.event,
    onDidEndTask: endEmitter.event,
    onDidStartTaskProcess: processStartEmitter.event,
    onDidEndTaskProcess: processEndEmitter.event,

    // Internal maps the terminal side also needs.
    _executions: executions,
    _providers: providers,

    /** Called by the reverse channel when a task process exits. */
    _finishTask(taskId, code) {
      const exec = executions.get(taskId);
      // A terminal run reports an empty taskId; it has no TaskExecution and
      // must not produce an `onDidEndTask` with an undefined task.
      if (exec) exec._finish(code);
    },
  };

  return Task_API;
}

/* --------------------------------------------------- value types & enums */
//
// Extensions import far more than functions. `CompletionItemKind`, `Diagnostic`,
// `CodeAction`, `SnippetString` and ~70 others are *values*, and a missing value
// is worse than a missing function: `class X extends vscode.SomeClass` throws
// "Class extends value undefined is not a constructor" while the bundle is still
// loading, so `activate()` never runs and the extension looks fundamentally
// broken. The Gradle extension failed exactly this way.
//
// Everything below is a data container or an enum: the extension builds it, the
// core reads its fields. None of them need a backing implementation.

const CompletionItemKind = {
  Text: 0, Method: 1, Function: 2, Constructor: 3, Field: 4, Variable: 5, Class: 6,
  Interface: 7, Module: 8, Property: 9, Unit: 10, Value: 11, Enum: 12, Keyword: 13,
  Snippet: 14, Color: 15, File: 16, Reference: 17, Folder: 18, EnumMember: 19,
  Constant: 20, Struct: 21, Event: 22, Operator: 23, TypeParameter: 24,
};

const SymbolKind = {
  File: 0, Module: 1, Namespace: 2, Package: 3, Class: 4, Method: 5, Property: 6,
  Field: 7, Constructor: 8, Enum: 9, Interface: 10, Function: 11, Variable: 12,
  Constant: 13, String: 14, Number: 15, Boolean: 16, Array: 17, Object: 18, Key: 19,
  Null: 20, EnumMember: 21, Struct: 22, Event: 23, Operator: 24, TypeParameter: 25,
};

const SymbolTag = { Deprecated: 1 };
const DiagnosticTag = { Unnecessary: 1, Deprecated: 2 };
const DocumentHighlightKind = { Text: 0, Read: 1, Write: 2 };
const FoldingRangeKind = { Comment: 1, Imports: 2, Region: 3 };

class FoldingRange {
  constructor(start, end, kind) {
    this.start = start;
    this.end = end;
    this.kind = kind;
  }
}

/** Returned by `createTextEditorDecorationType`; the decoration itself is
 *  applied through the renderer, this is the handle an extension keeps. */
class TextEditorDecorationType {
  constructor(key) {
    this.key = key || '';
  }
  dispose() {}
}
const FileChangeType = { Changed: 1, Created: 2, Deleted: 3 };
const CodeActionTriggerKind = { Invoke: 1, Automatic: 2 };
const InlayHintKind = { Type: 1, Parameter: 2 };
const InlineCompletionTriggerKind = { Invoke: 0, Automatic: 1 };
const NotebookCellKind = { Markup: 1, Code: 2 };
const NotebookCellStatusBarAlignment = { Left: 1, Right: 2 };
const NotebookEditorRevealType = { Default: 0, InCenter: 1, InCenterIfOutsideViewport: 2, AtTop: 3 };
const CommentMode = { Editing: 0, Preview: 1 };
const CommentThreadCollapsibleState = { Collapsed: 0, Expanded: 1 };
const QuickPickItemKind = { Separator: -1, Default: 0 };
const IndentAction = { None: 0, Indent: 1, IndentOutdent: 2, Outdent: 3 };
const TaskPanelKind = { Shared: 1, Dedicated: 2, New: 3 };
const TaskRevealKind = { Always: 1, Silent: 2, Never: 3 };
const ShellQuoting = { Escape: 1, Strong: 2, Weak: 3 };
const TextDocumentChangeReason = { Undo: 1, Redo: 2 };
const TextDocumentSaveReason = { Manual: 1, AfterDelay: 2, FocusOut: 3 };
const TextEditorSelectionChangeKind = { Keyboard: 1, Mouse: 2, Command: 3 };
const InputBoxValidationSeverity = { Info: 1, Warning: 2, Error: 3 };
const LanguageStatusSeverity = { Information: 0, Warning: 1, Error: 2 };
const FileType = { Unknown: 0, File: 1, Directory: 2, SymbolicLink: 64 };
const EndOfLine = { LF: 1, CRLF: 2 };
const QuickInputButtons = { Back: Object.freeze({ id: '__back__', title: 'Back' }) };
const ThemeIconPath = { File: 'file', Folder: 'folder' };

/* --------------------------------------------------------------- classes */

class CompletionItem {
  constructor(label, kind) {
    this.label = label;
    this.kind = kind;
    this.tags = [];
    this.detail = undefined;
    this.documentation = undefined;
    this.sortText = undefined;
    this.filterText = undefined;
    this.preselect = undefined;
    this.insertText = undefined;
    this.range = undefined;
    this.textEdit = undefined;
    this.additionalTextEdits = undefined;
    this.commitCharacters = undefined;
    this.command = undefined;
    this.keepWhitespace = undefined;
  }
}

class CompletionList {
  constructor(items, isIncomplete) {
    this.items = items || [];
    this.isIncomplete = !!isIncomplete;
  }
}

class CodeAction {
  constructor(title, kind) {
    this.title = title;
    this.kind = kind;
    this.diagnostics = undefined;
    this.edit = undefined;
    this.command = undefined;
    this.isPreferred = undefined;
    this.disabled = undefined;
  }
}

class CodeLens {
  constructor(range, command) {
    this.range = range;
    this.command = command;
    this.isResolved = !!command;
  }
}

class Color {
  constructor(red, green, blue, alpha) {
    this.red = red;
    this.green = green;
    this.blue = blue;
    this.alpha = alpha === undefined ? 1 : alpha;
  }
}

class ColorInformation {
  constructor(range, color) {
    this.range = range;
    this.color = color;
  }
}

class ColorPresentation {
  constructor(label) {
    this.label = label;
    this.textEdit = undefined;
    this.additionalTextEdits = undefined;
  }
}

class Diagnostic {
  constructor(range, message, severity) {
    this.range = range;
    this.message = message;
    this.severity = severity === undefined ? 0 : severity;
    this.code = undefined;
    this.source = undefined;
    this.relatedInformation = undefined;
    this.tags = undefined;
  }
}

class DiagnosticRelatedInformation {
  constructor(location, message) {
    this.location = location;
    this.message = message;
  }
}

class DocumentHighlight {
  constructor(range, kind) {
    this.range = range;
    this.kind = kind;
  }
}

class DocumentLink {
  constructor(range, target) {
    this.range = range;
    this.target = target;
    this.tooltip = undefined;
  }
}

class DocumentSymbol {
  constructor(name, detail, kind, range, selectionRange) {
    this.name = name;
    this.detail = detail;
    this.kind = kind;
    this.range = range;
    this.selectionRange = selectionRange;
    this.children = [];
    this.tags = [];
  }
}

class Hover {
  constructor(contents, range) {
    this.contents = Array.isArray(contents) ? contents : [contents];
    this.range = range;
  }
}

class LocationLink {
  constructor(targetUri, targetRange, targetSelectionRange, originSelectionRange) {
    this.targetUri = targetUri;
    this.targetRange = targetRange;
    this.targetSelectionRange = targetSelectionRange;
    this.originSelectionRange = originSelectionRange;
  }
}

class ParameterInformation {
  constructor(label, documentation) {
    this.label = label;
    this.documentation = documentation;
  }
}

class SignatureInformation {
  constructor(label, documentation) {
    this.label = label;
    this.documentation = documentation;
    this.parameters = [];
    this.activeParameter = undefined;
  }
}

class SignatureHelp {
  constructor() {
    this.signatures = [];
    this.activeSignature = 0;
    this.activeParameter = 0;
  }
}

class SelectionRange {
  constructor(range, parent) {
    this.range = range;
    this.parent = parent;
  }
}

class SemanticTokens {
  constructor(data, resultId) {
    this.data = data || new Uint32Array(0);
    this.resultId = resultId;
  }
}

class SemanticTokensLegend {
  constructor(tokenTypes, tokenModifiers) {
    this.tokenTypes = tokenTypes || [];
    this.tokenModifiers = tokenModifiers || [];
  }
}

class SemanticTokensBuilder {
  constructor(legend) {
    this._legend = legend;
    this._data = [];
    this._prevLine = 0;
    this._prevChar = 0;
  }
  push(line, char, length, tokenType, tokenModifiers) {
    this._data.push(line, char, length, tokenType, tokenModifiers || 0);
  }
  build() {
    return new SemanticTokens(new Uint32Array(this._data));
  }
}

class SnippetString {
  constructor(value) {
    this.value = value || '';
  }
  appendText(text) {
    this.value += String(text).replace(/[$}\\]/g, '\\$&');
    return this;
  }
  appendTabstop(n = 0) {
    this.value += `$${n}`;
    return this;
  }
  appendPlaceholder(value, n = 0) {
    this.value += `\${${n}:${value}}`;
    return this;
  }
  appendChoice(values, n = 0) {
    this.value += `\${${n}|${(values || []).join(',')}|}`;
    return this;
  }
  appendVariable(name, defaultValue) {
    this.value += defaultValue === undefined ? `\${${name}}` : `\${${name}:${defaultValue}}`;
    return this;
  }
}

class SnippetTextEdit {
  constructor(range, snippet) {
    this.range = range;
    this.snippet = snippet;
  }
  static insert(position, snippet) {
    return new SnippetTextEdit(new Range(position, position), snippet);
  }
  static replace(range, snippet) {
    return new SnippetTextEdit(range, snippet);
  }
}

class SymbolInformation {
  constructor(name, kind, containerName, location) {
    this.name = name;
    this.kind = kind;
    this.containerName = containerName;
    this.location = location;
    this.tags = [];
  }
}

class CallHierarchyItem {
  constructor(kind, name, detail, uri, range, selectionRange) {
    this.kind = kind;
    this.name = name;
    this.detail = detail;
    this.uri = uri;
    this.range = range;
    this.selectionRange = selectionRange;
    this.tags = [];
  }
}

class CallHierarchyIncomingCall {
  constructor(item, fromRanges) {
    this.from = item;
    this.fromRanges = fromRanges || [];
  }
}

class CallHierarchyOutgoingCall {
  constructor(item, fromRanges) {
    this.to = item;
    this.fromRanges = fromRanges || [];
  }
}

class TypeHierarchyItem {
  constructor(kind, name, detail, uri, range, selectionRange) {
    this.kind = kind;
    this.name = name;
    this.detail = detail;
    this.uri = uri;
    this.range = range;
    this.selectionRange = selectionRange;
  }
}

class LinkedEditingRanges {
  constructor(ranges, wordPattern) {
    this.ranges = ranges || [];
    this.wordPattern = wordPattern;
  }
}

class InlayHint {
  constructor(position, label, kind) {
    this.position = position;
    this.label = label;
    this.kind = kind;
    this.tooltip = undefined;
    this.paddingLeft = undefined;
    this.paddingRight = undefined;
    this.command = undefined;
  }
}

class InlineCompletionItem {
  constructor(insertText, range, command) {
    this.insertText = insertText;
    this.range = range;
    this.command = command;
  }
}

class InlineCompletionList {
  constructor(items) {
    this.items = items || [];
  }
}

class InlineValueText {
  constructor(range, text) {
    this.range = range;
    this.text = text;
  }
}

class InlineValueVariableLookup {
  constructor(range, variableName, caseSensitiveLookup) {
    this.range = range;
    this.variableName = variableName;
    this.caseSensitiveLookup = !!caseSensitiveLookup;
  }
}

class InlineValueEvaluatableExpression {
  constructor(range, expression) {
    this.range = range;
    this.expression = expression;
  }
}

class NotebookRange {
  constructor(start, end) {
    this.start = start;
    this.end = end;
    this.isEmpty = start === end;
  }
  with(change) {
    return new NotebookRange(change.start !== undefined ? change.start : this.start, change.end !== undefined ? change.end : this.end);
  }
}

class NotebookCellData {
  constructor(kind, value, languageId) {
    this.kind = kind;
    this.value = value;
    this.languageId = languageId;
    this.outputs = [];
    this.metadata = {};
    this.executionSummary = undefined;
  }
}

class NotebookData {
  constructor(cells) {
    this.cells = cells || [];
    this.metadata = {};
  }
}

class NotebookEdit {
  constructor(range, newCells) {
    this.range = range;
    this.newCells = newCells || [];
  }
  static replaceCells(range, newCells) {
    return new NotebookEdit(range, newCells);
  }
  static insertCells(index, newCells) {
    return new NotebookEdit(new NotebookRange(index, index), newCells);
  }
  static deleteCells(range) {
    return new NotebookEdit(range, []);
  }
}

class DataTransferItem {
  constructor(value) {
    this.value = value;
    this.mimeType = typeof value === 'string' ? 'text/plain' : 'application/octet-stream';
  }
  asString() {
    return Promise.resolve(typeof this.value === 'string' ? this.value : JSON.stringify(this.value));
  }
  asFile() {
    return Promise.resolve(undefined);
  }
}

class DataTransfer {
  constructor() {
    this._items = [];
  }
  set(item) {
    this._items.push(item);
  }
  get(mimeType) {
    return this._items.find((i) => i.mimeType === mimeType);
  }
  forEach(cb) {
    this._items.forEach(cb);
  }
  get size() {
    return this._items.length;
  }
  [Symbol.iterator]() {
    return this._items[Symbol.iterator]();
  }
}

/** `FileSystemError` is thrown by `workspace.fs`; extensions check `code`. */
class FileSystemError extends Error {
  constructor(messageOrUri, code) {
    super(typeof messageOrUri === 'string' ? messageOrUri : String(messageOrUri && messageOrUri.fsPath));
    this.code = code || 'Unknown';
    this.name = 'FileSystemError';
  }
  static FileNotFound(uri) {
    return new FileSystemError(uri, 'FileNotFound');
  }
  static FileExists(uri) {
    return new FileSystemError(uri, 'FileExists');
  }
  static FileNotADirectory(uri) {
    return new FileSystemError(uri, 'FileNotADirectory');
  }
  static FileIsADirectory(uri) {
    return new FileSystemError(uri, 'FileIsADirectory');
  }
  static NoPermissions(uri) {
    return new FileSystemError(uri, 'NoPermissions');
  }
  static Unavailable(uri) {
    return new FileSystemError(uri, 'Unavailable');
  }
  static FileTooLarge(uri) {
    return new FileSystemError(uri, 'FileTooLarge');
  }
}

/* ----------------------------------------------------------- namespaces */

const CancellationTokenSource = class {
  constructor() {
    this._emitter = new EventEmitter();
    this.token = {
      isCancellationRequested: false,
      onCancellationRequested: this._emitter.event,
    };
  }
  cancel() {
    this.token.isCancellationRequested = true;
    this._emitter.fire();
  }
  dispose() {
    this._emitter.dispose && this._emitter.dispose();
  }
};

const CancellationToken = {
  None: Object.freeze({
    isCancellationRequested: false,
    onCancellationRequested: () => new Disposable(),
  }),
  Cancelled: Object.freeze({
    isCancellationRequested: true,
    onCancellationRequested: (listener) => {
      listener();
      return new Disposable();
    },
  }),
};

/** `authentication`, `comments`, `notebook`, `scm`, `test`: register-and-forget.
 *  Extensions call these unconditionally; a missing namespace throws before
 *  activate() finishes, a no-op Disposable does not. */
const authentication = {
  getSession: () => Promise.resolve(undefined),
  getAccounts: () => Promise.resolve([]),
  onDidChangeSessions: new EventEmitter().event,
  registerAuthenticationProvider: () => new Disposable(),
};

const comments = {
  createCommentController: (id, label) => ({
    id,
    label,
    options: {},
    commentingRangeProvider: undefined,
    commentingRangeProviders: [],
    reactionHandler: undefined,
    createCommentThread(uri, range, commentsList) {
      return {
        uri,
        range,
        comments: commentsList || [],
        collapsibleState: 1,
        canReply: true,
        contextValue: undefined,
        label: undefined,
        threadId: undefined,
        dispose() {},
      };
    },
    dispose() {},
  }),
};

const notebook = {
  registerNotebookSerializer: () => new Disposable(),
  createNotebookController: () => ({
    id: 'irixen',
    label: 'irixen',
    supportsExecutionOrder: false,
    supportedLanguages: [],
    executeHandler: undefined,
    createNotebookCellExecution: () => ({
      token: CancellationToken.None,
      executionOrder: undefined,
      start() {},
      end() {},
      clearOutput() {},
      replaceOutput() {},
      appendOutput() {},
      replaceOutputItems() {},
      appendOutputItems() {},
    }),
    updateNotebookCells: () => {},
    dispose() {},
  }),
  onDidChangeNotebookDocument: new EventEmitter().event,
  onDidSaveNotebookDocument: new EventEmitter().event,
  onDidOpenNotebookDocument: new EventEmitter().event,
  onDidCloseNotebookDocument: new EventEmitter().event,
};

const scm = {
  createSourceControl: (id, label) => {
    const rootUri = CFG.workspace ? Uri.file(CFG.workspace) : Uri.file('');
    const inputBox = {
      value: '',
      placeholder: '',
      enabled: true,
      visible: true,
      onDidChange: new EventEmitter().event,
      show() {},
      hide() {},
      dispose() {},
    };
    return {
      id,
      label,
      rootUri,
      inputBox,
      statusBarCommands: [],
      count: 0,
      quickDiffProvider: undefined,
      commitTemplate: undefined,
      acceptInputCommand: undefined,
      createResourceGroup: (group) => ({
        id: group,
        label: group,
        hideWhenEmpty: false,
        resourceStates: [],
        dispose() {},
      }),
      dispose() {},
    };
  },
};

const test = {
  createTestController: (id, label) => ({
    id,
    label,
    items: new Map(),
    refreshHandler: undefined,
    resolveHandler: undefined,
    createTestItem: (itemId, itemLabel) => ({
      id: itemId,
      label: itemLabel,
      uri: undefined,
      children: { add() {}, delete() {}, replace() {}, forEach() {}, get() {}, get size() { return 0; } },
      canResolveChildren: false,
      busy: false,
      tags: [],
      error: undefined,
      parent: undefined,
    }),
    createRunProfile: () => ({ dispose() {} }),
    createTestRun: () => ({
      token: CancellationToken.None,
      name: undefined,
      started: () => Promise.resolve(),
      passed: () => Promise.resolve(),
      failed: () => Promise.resolve(),
      skipped: () => Promise.resolve(),
      errored: () => Promise.resolve(),
      appendOutput: () => Promise.resolve(),
      end: () => Promise.resolve(),
    }),
    dispose() {},
  }),
  registerTestProvider: () => new Disposable(),
};

/** Everything above, as one spreadable bag for the `vscode` module. */
const apiValues = {
  CompletionItem,
  CompletionList,
  CompletionItemKind,
  CodeAction,
  CodeActionTriggerKind,
  CodeLens,
  Color,
  ColorInformation,
  ColorPresentation,
  Diagnostic,
  DiagnosticRelatedInformation,
  DiagnosticTag,
  DocumentHighlight,
  DocumentHighlightKind,
  DocumentLink,
  DocumentSymbol,
  FoldingRange,
  FoldingRangeKind,
  TextEditorDecorationType,
  FileChangeType,
  FileSystemError,
  FileType,
  EndOfLine,
  Hover,
  SymbolInformation,
  SymbolKind,
  SymbolTag,
  LocationLink,
  ParameterInformation,
  SignatureHelp,
  SignatureInformation,
  SelectionRange,
  SemanticTokens,
  SemanticTokensBuilder,
  SemanticTokensLegend,
  SnippetString,
  SnippetTextEdit,
  CallHierarchyItem,
  CallHierarchyIncomingCall,
  CallHierarchyOutgoingCall,
  TypeHierarchyItem,
  LinkedEditingRanges,
  InlayHint,
  InlayHintKind,
  InlineCompletionItem,
  InlineCompletionList,
  InlineCompletionTriggerKind,
  InlineValueText,
  InlineValueVariableLookup,
  InlineValueEvaluatableExpression,
  NotebookRange,
  NotebookCellData,
  NotebookCellKind,
  NotebookData,
  NotebookEdit,
  NotebookEditorRevealType,
  NotebookCellStatusBarAlignment,
  DataTransfer,
  DataTransferItem,
  CommentAuthorInformation: { name: '', iconPath: undefined },
  CommentMode,
  CommentThreadCollapsibleState,
  QuickInputButtons,
  QuickPickItemKind,
  IndentAction,
  TaskPanelKind,
  TaskRevealKind,
  ShellQuoting,
  TextDocumentChangeReason,
  TextDocumentSaveReason,
  TextEditorSelectionChangeKind,
  InputBoxValidationSeverity,
  LanguageStatusSeverity,
  ThemeIconPath,
  CancellationToken,
  CancellationTokenSource,
  authentication,
  comments,
  notebook,
  scm,
  test,
};

function buildVscode(state) {
  const taskApi = buildTasksModule();
  activeTasks = taskApi;

  // ---------------------------------------------------------------- terminals
  //
  // These are real shells. `createTerminal()` opens a PTY in the core, so the
  // panel shows a prompt the user can type into, `ctrl-c` works, and a dev
  // server started by `sendText('npm run dev')` keeps running with its output
  // visible. Nothing here spawns one process per line any more.
  const terminals = [];
  const openTermEmitter = new EventEmitter();
  const closeTermEmitter = new EventEmitter();
  let termSeq = 1;

  function makeTerminal(nameOrOptions, shellPath, shellArgs) {
    const options =
      nameOrOptions && typeof nameOrOptions === 'object'
        ? nameOrOptions
        : { name: nameOrOptions, shellPath, shellArgs };
    const id = `term${termSeq++}`;
    const closeEmitter = new EventEmitter();
    let coreId = null;
    let queue = [];
    let disposed = false;

    const created = request('terminal.create', {
      cwd: cwdOf(options.cwd) || CFG.workspace || '',
      shellPath: options.shellPath || '',
    })
      .then((res) => {
        coreId = (res && res.id) || null;
        // Text typed before the shell existed must not be lost.
        for (const chunk of queue) {
          request('terminal.write', { id: coreId, data: chunk }).catch(() => {});
        }
        queue = [];
        return coreId;
      })
      .catch((e) => {
        log('err', `无法创建终端：${e && e.message ? e.message : e}`);
        return null;
      });

    const term = {
      name: options.name || `Terminal ${termSeq - 1}`,
      creationOptions: options,
      exitStatus: undefined,
      state: { isInteractedWith: true },
      shellIntegration: undefined,
      get processId() {
        return created.then((cid) => {
          if (!cid) return undefined;
          const digits = String(cid).replace(/\D+/g, '');
          return digits ? Number(digits) : undefined;
        });
      },
      get selection() {
        return undefined;
      },
      sendText(text, addNewLine) {
        const line = String(text == null ? '' : text);
        if (!line || disposed) return;
        const data = addNewLine === false ? line : `${line}\r`;
        if (coreId) {
          request('terminal.write', { id: coreId, data }).catch((e) =>
            log('err', `终端写入失败：${e && e.message ? e.message : e}`),
          );
        } else {
          queue.push(data);
        }
      },
      show() {
        void created.then((cid) => {
          if (cid) request('terminal.show', { id: cid }).catch(() => {});
        });
      },
      hide() {},
      dispose() {
        disposed = true;
        if (coreId) request('terminal.close', { id: coreId }).catch(() => {});
        const i = terminals.indexOf(term);
        if (i >= 0) terminals.splice(i, 1);
        closeEmitter.fire(term);
        closeTermEmitter.fire(term);
      },
      onDidClose: closeEmitter.event,
    };
    terminals.push(term);
    openTermEmitter.fire(term);
    return term;
  }

  const api = {
    // Value types first, so an explicit function below always wins.
    ...apiValues,
    version: '1.101.0',
    Uri,
    Position,
    Range,
    Selection,
    Location,
    Disposable,
    EventEmitter,
    ThemeColor,
    ThemeIcon,
    MarkdownString,
    // Build tooling needs these on the module, exactly like the real API.
    Task: taskApi.Task,
    TaskScope,
    TaskGroup,
    ShellExecution,
    ProcessExecution,
    CustomExecution,

    commands: {
      registerCommand(id, callback, thisArg) {
        const fn = thisArg ? callback.bind(thisArg) : callback;
        commandsMap.set(id, fn);
        notify('commands.put', { id });
        return new Disposable(() => commandsMap.delete(id));
      },
      registerTextEditorCommand(id, callback) {
        return api.commands.registerCommand(id, (...args) => callback(state.activeEditor, {}, ...args));
      },
      executeCommand(id, ...args) {
        if (commandsMap.has(id)) {
          return Promise.resolve()
            .then(() => commandsMap.get(id)(...args))
            .catch((e) => {
              log('err', e && e.stack ? e.stack : e);
              return undefined;
            });
        }
        // Built-in commands extensions call directly. Opening a document is the
        // one with a real effect here; context keys have no workbench to write
        // to, so acknowledging them is the honest no-op.
        if (id === 'vscode.open' || id === 'vscode.openWith') {
          const target = args[0];
          const p = target instanceof Uri ? target.fsPath : target && target.fsPath ? target.fsPath : String(target || '');
          if (p) notify('window.showTextDocument', { path: p });
          return Promise.resolve();
        }
        if (id === 'setContext' || id.startsWith('workbench.action.')) return Promise.resolve();
        return request('commands.execute', { id, args }).catch((e) => {
          log('err', e && e.message ? e.message : e);
          return undefined;
        });
      },
      getCommands() {
        return Promise.resolve([...commandsMap.keys()]);
      },
    },

    window: {
      get activeTextEditor() {
        return state.activeEditor;
      },
      get visibleTextEditors() {
        return state.activeEditor ? [state.activeEditor] : [];
      },
      onDidChangeActiveTextEditor: state.onActiveEditor.event,
      onDidChangeVisibleTextEditors: state.onVisibleEditors.event,
      onDidChangeTextEditorSelection: state.onSelection.event,
      onDidChangeWindowState: new EventEmitter().event,
      showInformationMessage: messageFn('info'),
      showWarningMessage: messageFn('warn'),
      showErrorMessage: messageFn('error'),
      showQuickPick(items, options = {}) {
        const list = Array.isArray(items) ? items : [];
        return request('window.quickPick', {
          items: list.map((it) => (typeof it === 'string' ? { label: it } : { label: it.label, description: it.description || '', detail: it.detail || '' })),
          placeholder: options.placeHolder || '',
          canPickMany: !!options.canPickMany,
        }).then((sel) => {
          // The renderer answers with indices; extensions expect their own items
          // back, including any `value`/`payload` they attached.
          if (sel === null || sel === undefined) return undefined;
          if (Array.isArray(sel)) return sel.map((i) => list[i]).filter((x) => x !== undefined);
          return list[sel];
        });
      },
      showInputBox(options = {}) {
        return request('window.inputBox', {
          prompt: options.prompt || '',
          value: options.value || '',
          placeHolder: options.placeHolder || '',
          password: !!options.password,
        });
      },
      createStatusBarItem: createStatusBarItem,
      createOutputChannel,
      setStatusBarMessage(text, timeoutOrThenable) {
        const item = createStatusBarItem(1, 1000);
        item.text = String(text);
        item.show();
        const hide = () => item.dispose();
        if (typeof timeoutOrThenable === 'number') setTimeout(hide, timeoutOrThenable);
        else if (timeoutOrThenable && timeoutOrThenable.then) timeoutOrThenable.then(hide, hide);
        return new Disposable(hide);
      },
      showTextDocument: (docOrUri, _options) =>
        Promise.resolve().then(() => {
          const p = docOrUri instanceof Uri ? docOrUri.fsPath : docOrUri && docOrUri.uri ? docOrUri.uri.fsPath : String(docOrUri);
          return request('window.showTextDocument', { path: p }).then(() => state.activeEditor || undefined);
        }),
      withProgress: (_options, task) => Promise.resolve(task({ report() {} }, { isCancellationRequested: false, onCancellationRequested: () => new Disposable() })),
      createTerminal: makeTerminal,
      createWebviewPanel,
      registerWebviewViewProvider,
      // A tab group with no tabs: extensions that enumerate editors get an
      // empty, well-typed list instead of `undefined`.
      tabGroups: {
        get all() {
          return [];
        },
        get activeTabGroup() {
          return undefined;
        },
        onDidChangeTabs: new EventEmitter().event,
        onDidChangeTabGroups: new EventEmitter().event,
        close: () => Promise.resolve(true),
      },
      registerTreeDataProvider,
      createTreeView,
      createTextEditorDecorationType: () => ({ dispose() {} }),
      registerUriHandler: () => new Disposable(),
      showOpenDialog: () => Promise.resolve(undefined),
      showSaveDialog: () => Promise.resolve(undefined),
      showInformationMessageWithOptions: messageFn('info'),

      // Register-and-forget providers. We do not have the backing feature (no
      // terminal, no notebook, no custom editors), but extensions call these
      // unconditionally in activate() and throw when the function is absent —
      // which is the difference between "extension loads" and "extension is
      // broken". A Disposable is the honest answer: it just never fires.
      registerTerminalLinkProvider: () => new Disposable(),
      registerTerminalProfileProvider: () => new Disposable(),
      registerTerminalQuickFixProvider: () => new Disposable(),
      registerCustomEditorProvider: () => new Disposable(),
      registerWebviewPanelSerializer: () => new Disposable(),
      registerFileDecorationProvider: () => new Disposable(),
      registerExternalUriOpener: () => new Disposable(),
      registerNotebookCellStatusBarItemProvider: () => new Disposable(),
      onDidOpenTerminal: openTermEmitter.event,
      onDidCloseTerminal: closeTermEmitter.event,
      onDidChangeActiveTerminal: new EventEmitter().event,
      onDidChangeTerminalState: new EventEmitter().event,
      onDidStartTerminalShellExecution: new EventEmitter().event,
      onDidEndTerminalShellExecution: new EventEmitter().event,
      get terminals() {
        return terminals;
      },
      get activeTerminal() {
        return terminals[terminals.length - 1];
      },
      onDidChangeActiveColorTheme: new EventEmitter().event,
      get activeColorTheme() {
        return { kind: 1 };
      },
    },

    workspace: {
      // A local IDE is a trusted workspace. prettier checks this and, when it
      // is falsy, strips its own config object down to a plain spread — which
      // loses every setting exposed through the configuration proxy below.
      isTrusted: true,
      createFileSystemWatcher: () => {
        const ev = () => new EventEmitter().event;
        return { onDidCreate: ev(), onDidChange: ev(), onDidDelete: ev(), dispose() {} };
      },
      get workspaceFolders() {
        return state.workspaceFolders;
      },
      get rootPath() {
        return state.workspaceFolders && state.workspaceFolders[0] ? state.workspaceFolders[0].uri.fsPath : undefined;
      },
      get name() {
        return state.workspaceName;
      },
      get workspaceFile() {
        return undefined;
      },
      get textDocuments() {
        return [...openDocs.values()];
      },
      getConfiguration,
      fs: {
        readFile: (uri) => request('fs.read', { path: Uri.revive(uri).fsPath }).then((text) => Buffer.from(String(text), 'utf8')),
        writeFile: (uri, content) =>
          request('fs.write', { path: Uri.revive(uri).fsPath, text: Buffer.from(content).toString('utf8') }),
        stat: (uri) =>
          request('fs.stat', { path: Uri.revive(uri).fsPath }).then((s) => ({
            type: s.type === 'dir' ? 2 : 1,
            ctime: s.mtime || 0,
            mtime: s.mtime || 0,
            size: s.size || 0,
          })),
        readDirectory: (uri) => request('fs.readdir', { path: Uri.revive(uri).fsPath }).then((rows) => rows.map((r) => [r.name, r.dir ? 2 : 1])),
        createDirectory: (uri) => request('fs.mkdir', { path: Uri.revive(uri).fsPath }),
        delete: (uri, options) => request('fs.delete', { path: Uri.revive(uri).fsPath, recursive: !!(options && options.recursive) }),
        rename: (from, to) => request('fs.rename', { from: Uri.revive(from).fsPath, to: Uri.revive(to).fsPath }),
        // `copy` is used by packagers and preview servers that stage files.
        copy: (from, to) =>
          request('fs.read', { path: Uri.revive(from).fsPath }).then((text) =>
            request('fs.write', { path: Uri.revive(to).fsPath, text }),
          ),
        isWritableFileSystem: () => true,
      },
      asRelativePath: (p) => {
        const root = api.workspace.rootPath;
        const target = typeof p === 'string' ? p : p.fsPath;
        if (!root) return target;
        const rel = path.relative(root, target);
        return rel.split(path.sep).join('/');
      },
      getWorkspaceFolder: (uri) => (state.workspaceFolders.length ? state.workspaceFolders[0] : undefined),
      openTextDocument: (arg) => {
        const p = arg instanceof Uri ? arg.fsPath : String(arg);
        return request('fs.read', { path: p }).then(
          (text) => new TextDocument(Uri.file(p), languageFor(p), 1, text, false),
        );
      },
      findFiles: () => Promise.resolve([]),
      onDidChangeTextDocument: state.onDidChangeDoc.event,
      onDidSaveTextDocument: state.onDidSaveDoc.event,
      onDidOpenTextDocument: state.onDidOpenDoc.event,
      onDidCloseTextDocument: state.onDidCloseDoc.event,
      onDidChangeConfiguration: new EventEmitter().event,
      onDidChangeWorkspaceFolders: new EventEmitter().event,
      onWillSaveTextDocument: new EventEmitter().event,
      onDidCreateFiles: new EventEmitter().event,
      onDidDeleteFiles: new EventEmitter().event,
      // The "will" variants see the change before it happens (used for
      // refactors that rewrite imports); we emit them from the watcher later,
      // but a registered listener must at least exist.
      onWillCreateFiles: new EventEmitter().event,
      onWillDeleteFiles: new EventEmitter().event,
      onWillRenameFiles: new EventEmitter().event,
      onDidRenameFiles: new EventEmitter().event,
      applyEdit: () => Promise.resolve(true),
      registerFileSystemProvider: () => new Disposable(),
    },

    languages: {
      createDiagnosticCollection: (name) => {
        const col = {
          name,
          set() {},
          delete() {},
          clear() {},
          dispose() {},
          forEach() {},
          get() {
            return undefined;
          },
        };
        diagnosticCollections.add(col);
        return col;
      },
      registerCompletionItemProvider: (selector, provider, ...triggers) =>
        registerProvider('completion', selector, provider, triggers),
      registerHoverProvider: (selector, provider) => registerProvider('hover', selector, provider),
      registerDocumentFormattingEditProvider: (selector, provider) =>
        registerProvider('format', selector, provider),
      registerDocumentRangeFormattingEditProvider: (selector, provider) =>
        registerProvider('rangeFormat', selector, provider),
      registerDefinitionProvider: () => new Disposable(),
      registerCodeActionsProvider: () => new Disposable(),
      registerDocumentSymbolProvider: () => new Disposable(),
      registerCodeLensProvider: () => new Disposable(),
      registerWorkspaceSymbolProvider: () => new Disposable(),
      setLanguageConfiguration: () => new Disposable(),
      // Prettier puts its per-file status there. The object must be mutable and
      // disposable; nothing reads it back from us.
      createLanguageStatusItem: (id, selector) => ({
        id,
        selector,
        name: id,
        text: '',
        detail: '',
        busy: false,
        severity: 0,
        command: undefined,
        accessibilityInformation: undefined,
        show() {},
        hide() {},
        dispose() {},
      }),
      registerSelectionRangeProvider: () => new Disposable(),
      registerDocumentHighlightProvider: () => new Disposable(),
      registerDocumentLinkProvider: () => new Disposable(),
      registerDocumentSemanticTokensProvider: () => new Disposable(),
      registerFoldingRangeProvider: () => new Disposable(),
      registerInlayHintsProvider: () => new Disposable(),
      registerCallHierarchyProvider: () => new Disposable(),
      registerTypeHierarchyProvider: () => new Disposable(),
      registerColorProvider: () => new Disposable(),
      registerRenameProvider: () => new Disposable(),
      registerReferenceProvider: () => new Disposable(),
      registerSignatureHelpProvider: () => new Disposable(),
      registerOnTypeFormattingEditProvider: () => new Disposable(),
      registerEvaluatableExpressionProvider: () => new Disposable(),
      registerLinkedEditingRangeProvider: () => new Disposable(),
      getLanguages: () => Promise.resolve([]),
      match: () => 0,
      CompletionItem: function (label) {
        return { label, kind: 0 };
      },
      CompletionItemKind: { Text: 0, Method: 1, Function: 2, Constructor: 3, Field: 4, Variable: 5, Class: 6, Interface: 7, Module: 8, Property: 9, Keyword: 13, Snippet: 14, File: 16, Folder: 18 },
      DiagnosticSeverity: { Error: 0, Warning: 1, Information: 2, Hint: 3 },
      Diagnostic: function (range, message, severity) {
        return { range, message, severity: severity || 0 };
      },
    },

    env: {
      appName: 'IRIXEN Code',
      appHost: 'desktop',
      appRoot: CFG.dir || '',
      language: CFG.language || 'zh-cn',
      machineId: CFG.machine || 'irixen',
      sessionId: CFG.session || 'irixen-session',
      uriScheme: 'irixen',
      // Desktop: several extensions branch on this to decide whether to start
      // a local server (they do) or use a browser API (they do not).
      uiKind: 1,
      remoteName: undefined,
      isTelemetryEnabled: false,
      onDidChangeTelemetryEnabled: new EventEmitter().event,
      // Telemetry is off, but extensions still build a logger inside activate()
      // (Live Preview does) and throw if the factory is missing, so hand back a
      // silent one that reports both channels as disabled.
      createTelemetryLogger: (sender, _options) => {
        const emitter = new EventEmitter();
        const noop = () => {};
        return {
          sender,
          isUsageEnabled: false,
          isErrorsEnabled: false,
          logUsage: noop,
          logError: noop,
          dispose: noop,
          onDidChangeEnableStates: emitter.event,
        };
      },
      clipboard: {
        readText: () => request('clipboard.read', {}),
        writeText: (t) => request('clipboard.write', { text: String(t) }).then(() => undefined),
      },
      openExternal: (uri) => request('env.openExternal', { url: uri instanceof Uri ? uri.toString() : String(uri) }),
      asExternalUri: (uri) => Promise.resolve(uri),
    },

    // `vscode.l10n.t` is used at module scope by some extensions; returning a
    // Disposable there would put an object in a menu label.
    l10n: {
      t(message, ...args) {
        if (message && typeof message === 'object' && message.message) {
          const values = message.args || args;
          return String(message.message).replace(/\{(\d+)\}/g, (m, i) =>
            values[i] !== undefined ? String(values[i]) : m,
          );
        }
        const text = String(message == null ? '' : message);
        return text.replace(/\{(\d+)\}/g, (m, i) => (args[i] !== undefined ? String(args[i]) : m));
      },
      bundle: {},
      uri: undefined,
    },
    UIKind: { Desktop: 1, Web: 2 },

    extensions: {
      get all() {
        return [
          {
            id: CFG.id,
            extensionPath: CFG.dir,
            packageJSON: CFG.package || {},
            isActive: true,
            exports: undefined,
          },
        ];
      },
      // `getExtension(ownId)` is used to read the extension's own manifest at
      // runtime; returning `undefined` made those extensions bail out early.
      getExtension: (id) => {
        if (String(id).toLowerCase() !== String(CFG.id).toLowerCase()) return undefined;
        return {
          id: CFG.id,
          extensionPath: CFG.dir,
          extensionUri: Uri.file(CFG.dir),
          packageJSON: CFG.package || {},
          isActive: true,
          exports: undefined,
          activate: () => Promise.resolve(undefined),
        };
      },
      onDidChange: new EventEmitter().event,
    },

    // Task and debug support. `tasks` is a real implementation now — Gradle,
    // Maven and npm extensions run their builds through `executeTask`; debug is
    // still registration-only, which is honest rather than pretending.
    tasks: taskApi,

    debug: {
      registerDebugAdapterDescriptorFactory: () => new Disposable(),
      registerDebugConfigurationProvider: () => new Disposable(),
      registerDebugAdapterTrackerFactory: () => new Disposable(),
      startDebugging: () => Promise.resolve(false),
      stopDebugging: () => Promise.resolve(),
      addBreakpoints: () => {},
      removeBreakpoints: () => {},
      asDebugSourceUri: (session) => (session && session.uri) || Uri.file(''),
      get activeDebugSession() {
        return undefined;
      },
      get activeDebugConsole() {
        return { append() {}, appendLine() {} };
      },
      get breakpoints() {
        return [];
      },
      onDidStartDebugSession: new EventEmitter().event,
      onDidTerminateDebugSession: new EventEmitter().event,
      onDidChangeActiveDebugSession: new EventEmitter().event,
      onDidReceiveDebugSessionCustomEvent: new EventEmitter().event,
      onDidChangeBreakpoints: new EventEmitter().event,
    },

    ProgressLocation: { SourceControl: 1, Window: 10, Notification: 15 },

    StatusBarAlignment: { Left: 1, Right: 2 },
    ViewColumn: { Active: -1, Beside: -2, One: 1, Two: 2, Three: 3 },
    ConfigurationTarget: { Global: 1, Workspace: 2, WorkspaceFolder: 3 },
    ExtensionMode: { Production: 1, Development: 2, Test: 3 },
    ExtensionKind: { UI: 1, Workspace: 2 },
    FileType: { Unknown: 0, File: 1, Directory: 2, SymbolicLink: 64 },
    EndOfLine: { LF: 1, CRLF: 2 },
    TextEditorRevealType: { Default: 0, InCenter: 1, InCenterIfOutsideViewport: 2, AtTop: 3 },
    DecorationRangeBehavior: { ClosedClosed: 0, OpenOpen: 1, OpenClosed: 2, ClosedOpen: 3 },
    OverviewRulerLane: { Left: 1, Center: 2, Right: 4, Full: 7 },
    DiagnosticSeverity: { Error: 0, Warning: 1, Information: 2, Hint: 3 },
    CompletionTriggerKind: { Invoke: 0, TriggerCharacter: 1, TriggerForIncompleteCompletions: 2 },
    TextEdit: {
      replace: (range, newText) => ({ range, newText }),
      insert: (pos, newText) => ({ range: new Range(pos, pos), newText }),
      delete: (range) => ({ range, newText: '' }),
    },
    WorkspaceEdit: function () {
      const ops = [];
      return {
        replace: (uri, range, text) => ops.push({ uri, range, text }),
        insert: (uri, pos, text) => ops.push({ uri, range: new Range(pos, pos), text }),
        delete: (uri, range) => ops.push({ uri, range, text: '' }),
        _ops: ops,
      };
    },
    RelativePattern: function (base, pattern) {
      return { base, pattern };
    },
    CancellationTokenSource: function () {
      const e = new EventEmitter();
      return { token: { isCancellationRequested: false, onCancellationRequested: e.event }, cancel() {}, dispose() {} };
    },
    // VS Code's CodeActionKind is a class with static kinds and append/contains.
    // Extensions evaluate `CodeActionKind.SourceFixAll.append('prettier')` at
    // module scope, so a two-key placeholder crashes them before activate().
    CodeActionKind: (() => {
      class K {
        constructor(value) { this.value = String(value || ''); }
        append(part) {
          return new K(this.value ? `${this.value}.${part}` : String(part));
        }
        contains(other) {
          const v = other && other.value !== undefined ? other.value : String(other || '');
          return this.value === '' || v === this.value || v.startsWith(this.value + '.');
        }
        intersects(other) {
          return this.contains(other) ||
            !!(other && typeof other.contains === 'function' && other.contains(this));
        }
        toString() { return this.value; }
      }
      K.Empty = new K('');
      K.QuickFix = new K('quickfix');
      K.Refactor = new K('refactor');
      K.RefactorExtract = new K('refactor.extract');
      K.RefactorInline = new K('refactor.inline');
      K.RefactorRewrite = new K('refactor.rewrite');
      K.Source = new K('source');
      K.SourceOrganizeImports = new K('source.organizeImports');
      K.SourceFixAll = new K('source.fixAll');
      K.SourceSortImports = new K('source.sortImports');
      K.Notebook = new K('notebook');
      return K;
    })(),
    TreeItem: function (label, collapsibleState) {
      return { label, collapsibleState };
    },
    TreeItemCollapsibleState: { None: 0, Collapsed: 1, Expanded: 2 },
  };
  api.workspace.applyEdit = (edit) => {
    const ops = edit && edit._ops ? edit._ops : [];
    if (!ops.length) return Promise.resolve(true);
    const byFile = new Map();
    for (const op of ops) {
      const p = Uri.revive(op.uri).fsPath;
      if (!byFile.has(p)) byFile.set(p, []);
      byFile.get(p).push({
        start: { line: op.range.start.line, character: op.range.start.character },
        end: { line: op.range.end.line, character: op.range.end.character },
        text: op.text,
      });
    }
    const jobs = [...byFile.entries()].map(([p, edits]) => request('editor.edit', { path: p, edits }));
    return Promise.all(jobs)
      .then(() => true)
      .catch(() => false);
  };

  // Language-service providers are intentionally out of scope (see §13.10),
  // but an extension that merely *registers* one at activation must not die on
  // `languages.someNewApi is not a function`. Any unknown method returns an
  // extensible Disposable — enough for `registerX()` results and for items
  // whose text/severity/command the extension assigns afterwards.
  api.languages = new Proxy(api.languages, {
    get(t, k) {
      if (typeof k === 'string' && !(k in t)) return () => new Disposable();
      return Reflect.get(t, k);
    },
  });
  return api;
}

/** Only used to label documents that were opened through `workspace.fs`. */
function languageFor(p) {
  const ext = (p.split('.').pop() || '').toLowerCase();
  const map = {
    js: 'javascript', mjs: 'javascript', cjs: 'javascript', jsx: 'javascriptreact',
    ts: 'typescript', tsx: 'typescriptreact', py: 'python', md: 'markdown',
    json: 'json', html: 'html', css: 'css', scss: 'scss', rs: 'rust', go: 'go',
    java: 'java', sh: 'shellscript', yml: 'yaml', yaml: 'yaml', toml: 'toml',
  };
  return map[ext] || 'plaintext';
}

/* --------------------------------------------------------------- lifecycle */

const state = {
  activeEditor: undefined,
  workspaceFolders: [],
  workspaceName: '',
  onActiveEditor: new EventEmitter(),
  onVisibleEditors: new EventEmitter(),
  onSelection: new EventEmitter(),
  onDidChangeDoc: new EventEmitter(),
  onDidSaveDoc: new EventEmitter(),
  onDidOpenDoc: new EventEmitter(),
  onDidCloseDoc: new EventEmitter(),
};

// Diagnostics only. `IRIXEN_API_TRACE=1` wraps the surface so every access to a
// member we do not implement is reported by name. Bundled extensions are
// minified (`n(1398).env.foo`), so static analysis cannot find these gaps — but
// the proxy sees the real accesses. Namespaces are proxied (cheap, read-only);
// constructors are handed back untouched so `instanceof` keeps working.
function traceSurface(api) {
  const seen = new Set();
  const kids = new Map();
  const report = (ns, key) => {
    const id = ns ? `${ns}.${key}` : String(key);
    if (seen.has(id)) return;
    seen.add(id);
    process.stderr.write(`[api-missing] ${id}\n`);
  };
  const skip = (k) => typeof k === 'symbol' || k === 'then' || k === '__esModule' || k === 'inspect';
  const isNamespace = (v) => v && typeof v === 'object' && !Array.isArray(v) && !(v instanceof Date);
  const wrapNs = (obj, ns) =>
    new Proxy(obj, {
      get(t, k, r) {
        if (skip(k)) return Reflect.get(t, k, r);
        if (!(k in t)) {
          report(ns, k);
          return undefined;
        }
        return Reflect.get(t, k, r);
      },
    });
  return new Proxy(api, {
    get(t, k, r) {
      if (skip(k)) return Reflect.get(t, k, r);
      const v = Reflect.get(t, k, r);
      if (v === undefined) {
        report('', k);
        return undefined;
      }
      if (typeof k === 'string' && isNamespace(v)) {
        if (!kids.has(k)) kids.set(k, wrapNs(v, k));
        return kids.get(k);
      }
      return v;
    },
  });
}

const rawVscode = buildVscode(state);
const vscode = process.env.IRIXEN_API_TRACE ? traceSurface(rawVscode) : rawVscode;
global.__IRIXEN_VSCODE__ = vscode;

// `require('vscode')` inside any nested dependency, whatever the resolution
// path, lands on the same singleton.
const originalLoad = Module._load;
Module._load = function (request, parent, isMain) {
  if (request === 'vscode') return vscode;
  return originalLoad.apply(this, arguments);
};

function applyDocEvent(method, p) {
  if (!p || !p.path) return;
  if (method === 'open' || method === 'change') {
    const doc = new TextDocument(Uri.file(p.path), p.languageId, p.version || 1, p.text || '', !!p.dirty, p.selection);
    openDocs.set(p.path, doc);
    if (method === 'open') {
      state.onDidOpenDoc.fire(doc);
      state.activeEditor = new TextEditor(doc, doc.selection);
      state.onActiveEditor.fire(state.activeEditor);
    } else {
      state.activeEditor = new TextEditor(doc, doc.selection);
      state.onDidChangeDoc.fire({ document: doc, contentChanges: [], reason: undefined });
    }
    return;
  }
  if (method === 'active') {
    const doc = openDocs.get(p.path);
    if (doc) {
      doc.selection = new Selection(
        new Position(p.selection.start.line, p.selection.start.character),
        new Position(p.selection.end.line, p.selection.end.character),
      );
      state.activeEditor = new TextEditor(doc, doc.selection);
      state.onActiveEditor.fire(state.activeEditor);
      state.onSelection.fire({ textEditor: state.activeEditor, selections: state.activeEditor.selections, kind: 1 });
    }
    return;
  }
  if (method === 'save') {
    const doc = openDocs.get(p.path);
    if (doc) {
      doc.isDirty = false;
      state.onDidSaveDoc.fire(doc);
    }
    return;
  }
  if (method === 'close') {
    const doc = openDocs.get(p.path);
    openDocs.delete(p.path);
    if (doc) state.onDidCloseDoc.fire(doc);
  }
}

async function handleReverse(msg) {
  let result = null;
  let error = null;
  try {
    switch (msg.method) {
      case 'ping':
        result = 'pong';
        break;
      case 'doc': {
        applyDocEvent(msg.params.kind, msg.params);
        break;
      }
      case 'deactivate': {
        if (typeof deactivateFn === 'function') await deactivateFn();
        result = true;
        break;
      }
      case 'executeCommand': {
        const id = msg.params.command;
        const fn = commandsMap.get(id);
        if (!fn) {
          error = `command not registered: ${id}`;
        } else {
          result = await fn(...(msg.params.args || []));
        }
        break;
      }
      case 'config': {
        Object.assign(configCache, msg.params.values || {});
        break;
      }
      case 'provider.call': {
        const p = providers.get(msg.params.provider);
        if (!p) {
          error = `provider not found: ${msg.params.provider}`;
          break;
        }
        const doc = docFromWire(msg.params.doc || {});
        const at = msg.params.position || { line: 0, character: 0 };
        const pos = new Position(at.line, at.character);
        const token = {
          isCancellationRequested: false,
          onCancellationRequested: () => new Disposable(),
        };
        const kind = msg.params.kind;
        if (kind === 'completion') {
          result = plain(
            await p.provider.provideCompletionItems(doc, pos, token, msg.params.context || {
              triggerKind: 0,
              triggerCharacter: undefined,
            }),
          );
        } else if (kind === 'hover') {
          result = plain(await p.provider.provideHover(doc, pos, token));
        } else if (kind === 'format') {
          result = plain(
            await p.provider.provideDocumentFormattingEdits(
              doc,
              msg.params.options || { tabSize: 2, insertSpaces: true },
              token,
            ),
          );
        } else if (kind === 'rangeFormat') {
          const r = msg.params.range || { start: at, end: at };
          result = plain(
            await p.provider.provideDocumentRangeFormattingEdits(
              doc,
              new Range(
                new Position(r.start.line, r.start.character),
                new Position(r.end.line, r.end.character),
              ),
              msg.params.options || { tabSize: 2, insertSpaces: true },
              token,
            ),
          );
        } else {
          error = `provider kind not supported: ${kind}`;
        }
        break;
      }
      case 'tree.children': {
        const p = msg.params || {};
        const row = treeRowFor(p);
        if (!row) {
          error = 'tree provider not registered';
          break;
        }
        result = await treeChildren(row, p.key || '');
        break;
      }
      case 'tree.command': {
        const p = msg.params || {};
        result = await treeRunCommand(treeRowFor(p), p.key, p.command);
        break;
      }
      case 'tree.refresh': {
        const row = treeRowFor(msg.params || {});
        if (row) { row.elements.clear(); row.commands.clear(); }
        result = true;
        break;
      }
      case 'webview.message': {
        const entry = panels.get(msg.params.id);
        if (entry) entry.emitter.fire(msg.params.message);
        result = true;
        break;
      }
      // The user closed the panel in the UI. Tell the extension its panel is
      // gone: otherwise `currentPanel` stays set and every later "Start" takes
      // the reveal path, which has no UI to reveal.
      case 'webview.dispose': {
        const entry = panels.get(msg.params.id);
        if (entry) entry.panel.dispose();
        result = true;
        break;
      }
      // A task or terminal command finished in the core. Turn it into the
      // events build tooling waits on, so a "Build" button can re-enable itself
      // and a failure can be reported instead of hanging forever.
      case 'task.exit': {
        const p = msg.params || {};
        if (activeTasks) activeTasks._finishTask(p.taskId || p.id || '', p.code);
        result = true;
        break;
      }
      case 'config.changed': {
        const values = msg.params.values || {};
        Object.assign(configCache, values);
        break;
      }
      default:
        error = `unknown host method: ${msg.method}`;
    }
  } catch (e) {
    error = e && e.stack ? e.stack : String(e);
  }
  sendLine({ t: 'rres', id: msg.id, result, error });
}

let deactivateFn = null;

function onLine(line) {
  if (!line) return;
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    log('err', `bad frame from host: ${line.slice(0, 200)}`);
    return;
  }
  if (msg.t === 'res') {
    const waiter = pending.get(msg.id);
    if (!waiter) return;
    pending.delete(msg.id);
    if (msg.error) waiter.reject(new Error(msg.error));
    else waiter.resolve(msg.result);
    return;
  }
  if (msg.t === 'rreq') {
    void handleReverse(msg);
  }
}

function installReadline(onLineFn) {
  let buf = '';
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', (chunk) => {
    buf += chunk;
    let idx;
    while ((idx = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, idx);
      buf = buf.slice(idx + 1);
      onLineFn(line);
    }
  });
  process.stdin.on('end', () => process.exit(0));
}

async function main() {
  installReadline(onLine);

  const boot = await request('boot', {}).catch(() => ({ state: {}, config: {} }));
  Object.assign(configCache, boot.config || {});
  const areas = makeStateAreas((boot.state || {}));

  state.workspaceName = CFG.workspaceName || (CFG.workspace ? path.basename(CFG.workspace) : '');
  state.workspaceFolders = CFG.workspace
    ? [{ uri: Uri.file(CFG.workspace), name: state.workspaceName, index: 0 }]
    : [];

  const storage = CFG.storageDir || CFG.dir;
  const context = {
    subscriptions: [],
    extensionPath: CFG.dir,
    extensionUri: Uri.file(CFG.dir),
    extensionMode: 1,
    globalState: areas.globalState,
    workspaceState: areas.workspaceState,
    secrets: {
      get: (k) => Promise.resolve(areas.globalState.get('secret.' + k, undefined)),
      store: (k, v) => areas.globalState.update('secret.' + k, v),
      delete: (k) => areas.globalState.update('secret.' + k, undefined),
      onDidChange: new EventEmitter().event,
    },
    globalStorageUri: Uri.file(path.join(storage, 'global')),
    globalStoragePath: path.join(storage, 'global'),
    logUri: Uri.file(path.join(storage, 'log')),
    logPath: path.join(storage, 'log'),
    storageUri: Uri.file(path.join(storage, 'workspace')),
    storagePath: path.join(storage, 'workspace'),
    environmentVariableCollection: { replace() {}, append() {}, prepend() {}, clear() {}, get() {}, delete() {}, persistent: false },
    asAbsolutePath: (rel) => path.join(CFG.dir, rel),
    extension: { id: CFG.id, extensionPath: CFG.dir, packageJSON: CFG.package || {}, isActive: true, exports: undefined },
  };
  for (const d of ['global', 'workspace', 'log']) {
    try {
      fs.mkdirSync(path.join(storage, d), { recursive: true });
    } catch {}
  }

  if (CFG.workspace) {
    try {
      process.chdir(CFG.workspace);
    } catch {}
  }

  notify('ready', { id: CFG.id, version: CFG.version || '0.0.0', node: process.version });

  const entry = CFG.entry;
  let mod;
  try {
    if (CFG.esm) {
      mod = await import(pathToFileURL(entry).href);
    } else {
      mod = require(entry);
    }
  } catch (e) {
    const msg = e && e.stack ? e.stack : String(e);
    notify('fatal', { message: `failed to load ${entry}: ${msg}` });
    log('err', msg);
    return;
  }

  const activate = (mod && mod.activate) || (mod && typeof mod.default === 'function' ? mod.default : null);
  deactivateFn = (mod && mod.deactivate) || null;

  if (typeof activate !== 'function') {
    notify('fatal', { message: `${CFG.id}: entry module has no activate() export` });
    return;
  }

  try {
    const exports = await activate(context);
    context.extension.exports = exports;
    notify('activated', { id: CFG.id, commands: [...commandsMap.keys()] });
  } catch (e) {
    const msg = e && e.stack ? e.stack : String(e);
    notify('fatal', { message: `activate() threw: ${msg}` });
    log('err', msg);
  }
}

process.on('uncaughtException', (e) => {
  log('err', `uncaught: ${e && e.stack ? e.stack : e}`);
});
process.on('unhandledRejection', (e) => {
  log('err', `unhandled rejection: ${e && e.stack ? e.stack : e}`);
});

void main();
