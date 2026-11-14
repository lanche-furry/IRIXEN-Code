// Standalone harness for the extension host: emulates the Rust core just enough
// to activate a real extension and drive its language providers.
//
//   node scripts/dev/ext-provider-test.cjs
//   EXT=xabikos.JavaScriptSnippets-1.8.0 node scripts/dev/ext-provider-test.cjs
//
// Every formatting result is *applied* to the source text and re-checked for
// idempotency, so a wrong range or offset shows up as a diff instead of a
// plausible-looking string.
const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const base = path.join(process.env.APPDATA || path.join(os.homedir(), 'AppData', 'Roaming'), 'IRIXEN Code');
const workspace = process.env.WS || fs.mkdtempSync(path.join(os.tmpdir(), 'irixen-ext-'));

const extDir = path.join(base, 'extensions', process.env.EXT || 'esbenp.prettier-vscode-12.4.0');
const pkg = JSON.parse(fs.readFileSync(path.join(extDir, 'package.json'), 'utf8'));
const entry = path.join(extDir, pkg.main || 'dist/extension.js');
// The runtime copy lives outside the repo, so Node does not pick up the repo's
// `"type": "module"` and stays in CommonJS mode (same as the real app).
const bootstrap = path.join(base, 'runtime', 'exthost', 'bootstrap.js');

const cfg = {
  id: pkg.publisher + '.' + pkg.name,
  name: pkg.displayName || pkg.name,
  version: pkg.version,
  dir: extDir,
  entry,
  esm: pkg.type === 'module',
  workspace,
  workspaceName: '测试工作区',
  storageDir: extDir,
  language: 'zh-CN',
  package: pkg,
};

const child = spawn('node', [bootstrap], {
  cwd: workspace,
  env: { ...process.env, IRIXEN_HOST_CFG: JSON.stringify(cfg) },
  stdio: ['pipe', 'pipe', 'pipe'],
});

let buf = '';
const providerId = {}; // kind -> host provider id
const send = (o) => child.stdin.write(JSON.stringify(o) + '\n');
let seq = 900;
const waits = new Map(); // rreq id -> resolve
let finished = false;

function call(method, params) {
  const id = seq++;
  return new Promise((resolve) => {
    waits.set(id, resolve);
    send({ t: 'rreq', id, method, params });
  });
}

/* ------------------------------------------------------------------ cases */

const CASES = [
  { name: 'json/short', path: workspace + '/demo.json', languageId: 'json', text: '{"a":1,"b":[1,2,3]}\n' },
  {
    name: 'json/long (must wrap)',
    path: workspace + '/demo.json',
    languageId: 'json',
    text:
      JSON.stringify({
        name: 'irixen',
        version: '1.0.0',
        keywords: ['editor', 'tauri', 'extension-host', 'prettier', '中文'],
        nested: { deep: { deeper: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12] } },
      }) + '\n',
  },
  {
    name: 'javascript/cjs',
    path: workspace + '/demo.js',
    languageId: 'javascript',
    text: 'const a=1;function f(x,y){return x+y}\nmodule.exports={a,f}\n',
  },
  { name: 'markdown', path: workspace + '/demo.md', languageId: 'markdown', text: '#  标题\n\n\n\n-   一\n-   二\n' },
  {
    // 中文内容：确认 UTF-16 offset 与 UTF-8 传输互不干扰
    name: 'json/中文内容',
    path: workspace + '/中文.json',
    languageId: 'json',
    text: '{"标题":"你好，世界","列表":[1,2,3]}\n',
  },
];

function applyEdits(text, edits) {
  // Edits are non-overlapping and expressed in UTF-16 offsets of the original.
  const toOffset = (pos) => {
    const lines = text.split('\n');
    let off = 0;
    for (let i = 0; i < pos.line; i++) off += lines[i].length + 1;
    return off + pos.character;
  };
  const sorted = edits
    .map((e) => ({ start: toOffset(e.range.start), end: toOffset(e.range.end), newText: e.newText }))
    .sort((a, b) => b.start - a.start);
  let out = text;
  for (const e of sorted) out = out.slice(0, e.start) + e.newText + out.slice(e.end);
  return out;
}

async function run() {
  let pass = 0;
  let fail = 0;

  for (const c of CASES) {
    const res = await call('provider.call', {
      provider: providerId.format,
      kind: 'format',
      doc: { path: c.path, text: c.text, languageId: c.languageId, version: 1 },
      options: { tabSize: 2, insertSpaces: true },
    });
    if (res.error) {
      console.log(`FAIL ${c.name}\n  error: ${String(res.error).split('\n')[0]}`);
      fail++;
      continue;
    }
    const edits = res.result || [];
    const got = applyEdits(c.text, edits);
    // Prettier is deterministic and idempotent: formatting twice must be stable.
    const res2 = await call('provider.call', {
      provider: providerId.format,
      kind: 'format',
      doc: { path: c.path, text: got, languageId: c.languageId, version: 2 },
      options: { tabSize: 2, insertSpaces: true },
    });
    const stable = !res2.error && applyEdits(got, res2.result || []) === got;
    const changed = got !== c.text;
    if (edits.length > 0 && changed && stable) {
      pass++;
      console.log(`ok   ${c.name}  (${edits.length} edit${edits.length > 1 ? 's' : ''})`);
    } else {
      fail++;
      console.log(`FAIL ${c.name}  edits=${edits.length} changed=${changed} stable=${stable}`);
      console.log('  in :', JSON.stringify(c.text));
      console.log('  out:', JSON.stringify(got));
    }
  }

  // Range formatting must touch only the requested lines.
  const rangeDoc = CASES[2];
  const rr = await call('provider.call', {
    provider: providerId.rangeFormat,
    kind: 'rangeFormat',
    doc: { path: rangeDoc.path, text: rangeDoc.text, languageId: rangeDoc.languageId, version: 1 },
    // Start must be non-zero: Prettier guards range options with a truthiness
    // check, so offset 0 silently falls back to whole-document formatting.
    range: { start: { line: 1, character: 0 }, end: { line: 1, character: 18 } },
    options: { tabSize: 2, insertSpaces: true },
  });
  if (rr.error) {
    console.log('FAIL rangeFormat\n  error:', String(rr.error).split('\n')[0]);
    fail++;
  } else {
    const edits = rr.result || [];
    const out = applyEdits(rangeDoc.text, edits);
    const restIntact = out.split('\n')[0] === rangeDoc.text.split('\n')[0];
    const ok = edits.length > 0 && restIntact;
    ok ? pass++ : fail++;
    console.log(`${ok ? 'ok  ' : 'FAIL'} rangeFormat  (${edits.length} edit${edits.length > 1 ? 's' : ''}, untouched line kept=${restIntact})`);
    if (!ok) { console.log('  in :', JSON.stringify(rangeDoc.text)); console.log('  out:', JSON.stringify(out)); }
  }

  console.log(`\n${pass} passed, ${fail} failed`);
  finished = true;
  child.kill();
  process.exit(fail ? 1 : 0);
}

function runFormat() {
  if (!providerId.format) {
    console.log('no formatting provider registered');
    child.kill();
    process.exit(1);
  }
  run().catch((e) => {
    console.log('harness error:', e && e.stack);
    child.kill();
    process.exit(1);
  });
}

/* -------------------------------------------------------------- transport */

child.stdout.on('data', (chunk) => {
  buf += chunk.toString('utf8');
  let i;
  while ((i = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, i);
    buf = buf.slice(i + 1);
    if (!line.trim()) continue;
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      continue;
    }
    if (msg.t === 'req') {
      const p = msg.params || {};
      if (msg.method === 'boot') {
        send({ t: 'res', id: msg.id, result: { state: {}, config: {} } });
      } else if (msg.method === 'providers.put') {
        providerId[p.kind] = p.id;
        console.log('provider:', p.kind, p.id);
        send({ t: 'res', id: msg.id, result: null });
      } else if (msg.method === 'activated') {
        console.log('ACTIVATED', JSON.stringify(p.commands || []));
        setTimeout(runFormat, 300);
      } else if (msg.method === 'fatal') {
        console.log('FATAL', p.message);
      } else if (msg.method === 'webview.create') {
        console.log('WEBVIEW', p.id, JSON.stringify(p.title), 'html.length =', (p.html || '').length);
        send({ t: 'res', id: msg.id, result: null });
      } else {
        if (msg.id) send({ t: 'res', id: msg.id, result: null });
      }
    } else if (msg.t === 'rres') {
      const w = waits.get(msg.id);
      if (w) {
        waits.delete(msg.id);
        w(msg);
      }
    }
  }
});

child.stderr.on('data', (d) => {
  const text = d.toString('utf8');
  if (/error|Error|throw/.test(text)) console.log('STDERR:', text.slice(0, 800));
});

setTimeout(() => {
  if (!finished) {
    console.log('timeout');
    child.kill();
    process.exit(1);
  }
}, 45000);
