// Static compatibility audit: which `vscode` API members does an extension
// bundle actually touch, and which of those does our host not provide?
//
//   node scripts/dev/api-audit.cjs                 # audit every installed extension
//   node scripts/dev/api-audit.cjs <ext-dir> ...   # audit specific ones
//
// The API surface is produced by running the host's own `bootstrap.js` in dump
// mode, so the report cannot drift from what the app really ships. Minified
// (webpack) bundles cannot be read this way; for those, use the runtime probe:
// IRIXEN_API_TRACE=1 plus scripts/dev/ext-webview-test.cjs.
const { spawnSync } = require('child_process');
const fs = require('fs');
const path = require('path');
const os = require('os');

const repo = path.resolve(__dirname, '..', '..');
const bootstrapSrc = path.join(repo, 'src-tauri', 'src', 'exthost', 'bootstrap.js');
const base = path.join(process.env.APPDATA || path.join(os.homedir(), 'AppData', 'Roaming'), 'IRIXEN Code');

const DUMP = `
try {
  const surface = {};
  for (const k of Object.keys(vscode)) {
    const v = vscode[k];
    surface[k] = v && typeof v === 'object' ? Object.keys(v).sort() : typeof v;
  }
  fs.writeFileSync(process.env.DUMP_TO, JSON.stringify(surface));
  process.exit(0);
} catch (e) {
  console.error(e);
  process.exit(1);
}
`;

function apiSurface() {
  const stamp = `${process.pid}`;
  const tmp = path.join(os.tmpdir(), `irixen-surface-${stamp}.js`);
  const out = path.join(os.tmpdir(), `irixen-surface-${stamp}.json`);
  // The copy must not sit inside the repo: `"type": "module"` in package.json
  // would turn this CommonJS script into an ES module.
  fs.writeFileSync(tmp, fs.readFileSync(bootstrapSrc, 'utf8') + DUMP);
  const r = spawnSync(process.execPath, [tmp], {
    env: { ...process.env, DUMP_TO: out, IRIXEN_HOST_CFG: '{}' },
    stdio: ['ignore', 'ignore', 'pipe'],
    cwd: os.tmpdir(),
  });
  if (!fs.existsSync(out)) {
    console.error('failed to dump API surface:\n' + String(r.stderr || ''));
    process.exit(2);
  }
  const surface = JSON.parse(fs.readFileSync(out, 'utf8'));
  fs.rmSync(tmp, { force: true });
  fs.rmSync(out, { force: true });
  return surface;
}

const surface = apiSurface();
const namespaces = new Set(Object.keys(surface).filter((k) => Array.isArray(surface[k])));

function expandAliases(src) {
  const aliases = new Map(); // alias -> namespace
  const moduleVars = new Set(); // vars holding the whole module object

  for (const m of src.matchAll(/import\s*\{([^}]*)\}\s*from\s*["']vscode["']/g)) {
    for (const part of m[1].split(',')) {
      const t = part.trim();
      if (!t) continue;
      const [name, alias] = t.split(/\s+as\s+/).map((x) => x && x.trim());
      aliases.set(alias || name, name);
    }
  }
  for (const m of src.matchAll(/(?:const|let|var)\s*\{([^}]*)\}\s*=\s*require\(\s*["']vscode["']\s*\)/g)) {
    for (const part of m[1].split(',')) {
      const t = part.trim();
      if (!t) continue;
      const [name, alias] = t.split(':').map((x) => x && x.trim());
      aliases.set(alias || name, name);
    }
  }
  for (const m of src.matchAll(/(?:const|let|var)\s+([\w$]+)\s*=\s*require\(\s*["']vscode["']\s*\)/g)) {
    moduleVars.add(m[1]);
  }
  return { aliases, moduleVars };
}

function audit(dir) {
  const pkgPath = path.join(dir, 'package.json');
  if (!fs.existsSync(pkgPath)) return 0;
  const pkg = JSON.parse(fs.readFileSync(pkgPath, 'utf8'));
  const entry = path.join(dir, pkg.main || 'dist/extension.js');
  console.log(`\n== ${pkg.publisher || '?'}.${pkg.name}@${pkg.version}`);
  if (!fs.existsSync(entry)) {
    console.log('   (no bundle entry point)');
    return 0;
  }
  const src = fs.readFileSync(entry, 'utf8');
  const { aliases, moduleVars } = expandAliases(src);

  const used = new Map();
  // Source-map comments and path strings look like accesses (`commands.ts:12`,
  // `workspace/foo`), so drop obvious file extensions and anything followed by a
  // quote or slash. Cheap filter, and the false positives disappear.
  const NOT_A_MEMBER = new Set(['ts', 'tsx', 'js', 'jsx', 'mjs', 'cjs', 'json', 'jsonc', 'md', 'css', 'html', 'map', 'txt']);
  const add = (ns, member) => {
    if (!namespaces.has(ns)) return;
    if (NOT_A_MEMBER.has(member)) return;
    if (!used.has(ns)) used.set(ns, new Set());
    used.get(ns).add(member);
  };
  // A member followed by a quote or a slash is part of a string (`"commands.ts"`,
  // `"workspace/foo"`), not a property access. Cheap filter, wrong answers gone.
  for (const [alias, ns] of aliases) {
    if (!namespaces.has(ns)) continue;
    const re = new RegExp(`\\b${alias.replace(/\$/g, '\\$')}\\.([\\w$]+)(?!['"\`/])`, 'g');
    for (const m of src.matchAll(re)) add(ns, m[1]);
  }
  for (const v of moduleVars) {
    const re = new RegExp(`\\b${v}\\.([\\w$]+)\\.([\\w$]+)(?!['"\`/])`, 'g');
    for (const m of src.matchAll(re)) add(m[1], m[2]);
  }

  if (!used.size) {
    console.log('   (minified bundle — use the runtime probe instead:');
    console.log('    IRIXEN_API_TRACE=1 node scripts/dev/ext-webview-test.cjs)');
    return 0;
  }

  let missing = 0;
  for (const [ns, members] of [...used].sort()) {
    const have = new Set(surface[ns]);
    const lack = [...members].filter((x) => !have.has(x)).sort();
    if (!lack.length) continue;
    missing += lack.length;
    console.log(`   MISSING ${ns}: ${lack.join(', ')}`);
  }
  if (!missing) console.log('   no missing members');
  return missing;
}

let dirs = process.argv.slice(2);
if (!dirs.length) {
  const root = path.join(base, 'extensions');
  dirs = fs.existsSync(root)
    ? fs
        .readdirSync(root)
        .map((d) => path.join(root, d))
        .filter((d) => fs.statSync(d).isDirectory())
    : [];
}
if (!dirs.length) {
  console.error('no extensions found');
  process.exit(2);
}
let total = 0;
for (const d of dirs) total += audit(d);
console.log(`\n${total} missing member(s)`);
process.exit(total ? 1 : 0);
