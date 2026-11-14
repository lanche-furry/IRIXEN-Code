// Check how a package.json looks *after* VS Code's manifest localisation.
//
//   node scripts/dev/manifest-l10n-check.cjs                     # all installed extensions
//   node scripts/dev/manifest-l10n-check.cjs ms-vscode.live-server-0.4.20
//
// Mirrors `nls_map` / `localize` in src-tauri/src/extensions.rs so a `%key%`
// that the Rust side fails to resolve shows up here instead of in the UI.

const fs = require('fs');
const path = require('path');
const os = require('os');

const base = path.join(os.homedir(), 'AppData', 'Roaming', 'IRIXEN Code', 'extensions');
const locale = process.env.LOCALE || 'zh-CN';

function nlsMap(dir) {
  const out = new Map();
  const read = (name) => {
    const p = path.join(dir, name);
    if (!fs.existsSync(p)) return;
    let map;
    try {
      map = JSON.parse(fs.readFileSync(p, 'utf8'));
    } catch {
      return;
    }
    for (const [k, v] of Object.entries(map)) if (typeof v === 'string') out.set(k, v);
  };
  read('package.nls.json');
  const low = locale.toLowerCase().replace('_', '-');
  const candidates = [low];
  const dash = low.indexOf('-');
  if (dash > 0) candidates.push(low.slice(0, dash));
  for (const c of candidates) {
    const name = `package.nls.${c}.json`;
    if (fs.existsSync(path.join(dir, name))) {
      read(name);
      break;
    }
  }
  return out;
}

function localizeStr(s, nls) {
  if (typeof s !== 'string' || s.length < 2 || !s.startsWith('%') || !s.endsWith('%')) return s;
  const key = s.slice(1, -1);
  return nls.get(key) || s;
}

function walk(v, nls) {
  if (Array.isArray(v)) return v.map((x) => walk(x, nls));
  if (v && typeof v === 'object') {
    if (typeof v.value === 'string' && 'original' in v) return v.value;
    const out = {};
    for (const [k, x] of Object.entries(v)) out[k] = walk(x, nls);
    return out;
  }
  return localizeStr(v, nls);
}

const only = process.argv[2];
const dirs = fs.existsSync(base) ? fs.readdirSync(base) : [];
let unresolved = 0;

for (const d of dirs) {
  if (only && d !== only) continue;
  const dir = path.join(base, d);
  const pkgPath = path.join(dir, 'package.json');
  if (!fs.existsSync(pkgPath)) continue;
  const pkg = JSON.parse(fs.readFileSync(pkgPath, 'utf8'));
  const nls = nlsMap(dir);
  const out = walk(pkg, nls);

  console.log(`== ${d}   (nls keys: ${nls.size}, locale: ${locale})`);
  console.log(`   displayName: ${JSON.stringify(pkg.displayName)} → ${JSON.stringify(out.displayName)}`);

  const cmds = (out.contributes && out.contributes.commands) || [];
  const rawCmds = (pkg.contributes && pkg.contributes.commands) || [];
  for (let i = 0; i < cmds.length; i++) {
    const before = rawCmds[i] && rawCmds[i].title;
    const after = cmds[i].title;
    const mark = typeof after === 'string' && after.startsWith('%') ? '  ✗ UNRESOLVED' : '';
    if (mark) unresolved++;
    if (typeof before === 'string' && before.startsWith('%')) {
      console.log(`   ${cmds[i].command}: ${JSON.stringify(before)} → ${JSON.stringify(after)}${mark}`);
    }
  }
}

console.log(unresolved ? `\n${unresolved} title(s) still raw — add the key to package.nls.json` : '\nall titles resolved');
