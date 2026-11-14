// End-to-end check for the hardest extension path: activate a webview-based
// extension (Live Preview), run its command, and watch what it asks the host for.
//
//   node scripts/dev/ext-webview-test.cjs
//   TRACE=1 node scripts/dev/ext-webview-test.cjs   # also report missing APIs
//
// Prints every reverse request the extension makes, so a missing API shows up as
// a named failure instead of a silent "panel never opened". Exits non-zero when
// no panel HTML was produced.
const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const base = path.join(process.env.APPDATA || path.join(os.homedir(), 'AppData', 'Roaming'), 'IRIXEN Code');
const workspace = process.env.WS || fs.mkdtempSync(path.join(os.tmpdir(), 'irixen-wv-'));
const extDir = path.join(base, 'extensions', process.env.EXT || 'ms-vscode.live-server-0.4.20');
const pkg = JSON.parse(fs.readFileSync(path.join(extDir, 'package.json'), 'utf8'));
const entry = path.join(extDir, pkg.main);
const bootstrap = path.join(base, 'runtime', 'exthost', 'bootstrap.js');

const cfg = {
  id: pkg.publisher + '.' + pkg.name,
  name: pkg.displayName || pkg.name,
  version: pkg.version,
  dir: extDir,
  entry,
  esm: false,
  workspace,
  workspaceName: path.basename(workspace),
  storageDir: extDir,
  language: 'zh-CN',
  package: pkg,
};

const child = spawn('node', [bootstrap], {
  cwd: workspace,
  env: { ...process.env, IRIXEN_HOST_CFG: JSON.stringify(cfg), ...(process.env.TRACE ? { IRIXEN_API_TRACE: '1' } : {}) },
  stdio: ['pipe', 'pipe', 'pipe'],
});

let buf = '';
const send = (o) => child.stdin.write(JSON.stringify(o) + '\n');
let seq = 900;
const waits = new Map();

function call(method, params) {
  const id = seq++;
  return new Promise((resolve) => {
    waits.set(id, resolve);
    send({ t: 'rreq', id, method, params });
  });
}

const webviews = [];
let activated = false;

async function afterActivate() {
  if (activated) return;
  activated = true;

  // The preview needs something to preview; hand it the active document the same
  // way the core does when the user runs the command with a file open.
  const htmlPath = path.join(workspace, 'index.html');
  if (!fs.existsSync(htmlPath)) {
    fs.writeFileSync(htmlPath, '<!doctype html><html><body><h1>你好，Live Preview</h1></body></html>\n');
  }
  await call('doc', {
    kind: 'open',
    path: htmlPath,
    languageId: 'html',
    version: 1,
    text: fs.readFileSync(htmlPath, 'utf8'),
    dirty: false,
  });

  const r = await call('executeCommand', { command: 'livePreview.start', args: [] });
  if (r.error) {
    console.log('executeCommand error:', String(r.error).split('\n').slice(0, 4).join(' | '));
  } else {
    console.log('executeCommand ok');
  }

  // Give the extension a moment to build and ship panel HTML.
  setTimeout(() => {
    if (!webviews.length) {
      console.log('NO WEBVIEW CREATED');
      process.exitCode = 1;
    } else {
      for (const w of webviews) {
        console.log(`WEBVIEW ${w.id} title=${JSON.stringify(w.title)} html=${w.html.length} bytes`);
        console.log('  head:', w.html.replace(/\s+/g, ' ').slice(0, 220));
        if (process.env.OUT) {
          fs.writeFileSync(process.env.OUT, w.html);
          console.log('  ->', process.env.OUT);
        }
        if (!w.html.length) process.exitCode = 1;
      }
      process.exitCode = process.exitCode || 0;
    }
    child.kill();
    process.exit(process.exitCode);
  }, 8000);
}

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
      console.log('  <-', msg.method, p.command ? `(${p.command})` : '');
      if (msg.method === 'boot') {
        send({ t: 'res', id: msg.id, result: { state: {}, config: {} } });
      } else if (msg.method === 'activated') {
        send({ t: 'res', id: msg.id, result: null });
        setTimeout(afterActivate, 200);
      } else if (msg.method === 'webview.update') {
        const w = webviews.find((x) => x.id === p.id);
        if (w && p.html) w.html = p.html;
        send({ t: 'res', id: msg.id, result: null });
      } else if (msg.method === 'webview.create') {
        webviews.push({ id: p.id, title: p.title, html: p.html || '' });
        send({ t: 'res', id: msg.id, result: null });
      } else if (msg.method === 'fatal') {
        console.log('FATAL', p.message);
        send({ t: 'res', id: msg.id, result: null });
      } else {
        // Everything else is answered with null/true so the extension keeps
        // moving; the printed method list is the interesting part.
        if (msg.id) send({ t: 'res', id: msg.id, result: msg.method === 'config.get' ? null : true });
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
  const t = d.toString('utf8');
  if (t.includes('[api-missing]')) console.log('MISS', t.trim());
  else if (/error|Error|Cannot|undefined/.test(t)) console.log('STDERR:', t.trim().slice(0, 1200));
});

setTimeout(() => {
  if (!activated) console.log('never activated');
  child.kill();
  process.exit(process.exitCode || 1);
}, 40000);
