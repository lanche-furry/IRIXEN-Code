// Activate every installed extension under the API probe and report what it
// asked for that the host does not provide.
//
//   node scripts/dev/ext-compat-audit.cjs
//   node scripts/dev/ext-compat-audit.cjs --json > report.json
//
// The probe (`IRIXEN_API_TRACE=1`) wraps the `vscode` surface in a Proxy and
// logs `[api-missing] ns.member` for every property an extension reads that is
// not there. Static scanning cannot do this for webpack bundles — the member
// names are minified away — so activation is the only honest measurement.
//
// Exit code is 0 when every extension activated, 1 when at least one threw.
const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const base = path.join(process.env.APPDATA || path.join(os.homedir(), 'AppData', 'Roaming'), 'IRIXEN Code');
const bootstrap = path.join(base, 'runtime', 'exthost', 'bootstrap.js');
const extRoot = path.join(base, 'extensions');
const timeoutMs = Number(process.env.TIMEOUT || 9000);

if (!fs.existsSync(bootstrap)) {
  console.error('找不到运行时宿主，请先启动一次应用：', bootstrap);
  process.exit(2);
}

const asJson = process.argv.includes('--json');

function listExtensions() {
  const out = [];
  for (const dir of fs.readdirSync(extRoot)) {
    const pkgPath = path.join(extRoot, dir, 'package.json');
    if (!fs.existsSync(pkgPath)) continue;
    let pkg;
    try {
      pkg = JSON.parse(fs.readFileSync(pkgPath, 'utf8'));
    } catch {
      continue;
    }
    // Only real VS Code extensions have an entry point and an engine range.
    if (!pkg.main) continue;
    if (!pkg.engines || !pkg.engines.vscode) continue;
    out.push({ dir, pkg });
  }
  return out;
}

function auditOne({ dir, pkg }) {
  return new Promise((resolve) => {
    const extDir = path.join(extRoot, dir);
    const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'irixen-audit-'));
    const cfg = {
      id: `${pkg.publisher || 'unknown'}.${pkg.name}`,
      name: pkg.displayName || pkg.name,
      version: pkg.version,
      dir: extDir,
      entry: path.join(extDir, pkg.main),
      esm: pkg.type === 'module',
      workspace,
      workspaceName: path.basename(workspace),
      storageDir: extDir,
      language: 'zh-CN',
      package: pkg,
    };

    const child = spawn('node', [bootstrap], {
      cwd: workspace,
      env: { ...process.env, IRIXEN_HOST_CFG: JSON.stringify(cfg), IRIXEN_API_TRACE: '1' },
      stdio: ['pipe', 'pipe', 'pipe'],
    });

    const result = {
      id: cfg.id,
      dir,
      missing: new Set(),
      activated: false,
      fatal: '',
      error: '',
    };

    let buf = '';
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      try {
        child.kill();
      } catch {
        /* already gone */
      }
      resolve(result);
    };

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
        if (msg.t !== 'req') continue;
        const p = msg.params || {};
        if (msg.method === 'boot') {
          child.stdin.write(JSON.stringify({ t: 'res', id: msg.id, result: { state: {}, config: {} } }) + '\n');
        } else if (msg.method === 'activated') {
          result.activated = true;
          // Leave a beat for post-activation API access, then stop.
          setTimeout(finish, 1200);
        } else if (msg.method === 'fatal') {
          result.fatal = String(p.message || '').split('\n')[0];
          finish();
        } else if (msg.id) {
          child.stdin.write(JSON.stringify({ t: 'res', id: msg.id, result: null }) + '\n');
        }
      }
    });

    child.stderr.on('data', (d) => {
      const text = d.toString('utf8');
      for (const line of text.split('\n')) {
        const m = /\[api-missing\]\s+(\S+)/.exec(line);
        if (m) result.missing.add(m[1]);
      }
    });

    child.on('exit', (code) => {
      if (!result.activated && !result.fatal) {
        result.error = `exit ${code}`;
      }
      finish();
    });

    setTimeout(finish, timeoutMs);
  });
}

(async () => {
  const exts = listExtensions();
  if (!exts.length) {
    console.error('没有找到已安装的 VS Code 扩展：', extRoot);
    process.exit(2);
  }

  const results = [];
  for (const ext of exts) {
    const r = await auditOne(ext);
    results.push(r);
    if (!asJson) {
      const state = r.activated ? 'ok  ' : r.fatal ? 'FAIL' : 'hang';
      console.log(`${state} ${r.id.padEnd(38)} missing=${String(r.missing.size).padStart(2)}`);
      if (r.fatal) console.log(`       ${r.fatal.slice(0, 140)}`);
      if (r.error) console.log(`       ${r.error}`);
      if (r.missing.size) console.log(`       ${[...r.missing].slice(0, 12).join(', ')}`);
    }
  }

  const tally = new Map();
  for (const r of results) for (const m of r.missing) tally.set(m, (tally.get(m) || 0) + 1);

  if (asJson) {
    console.log(
      JSON.stringify(
        {
          extensions: results.map((r) => ({
            id: r.id,
            activated: r.activated,
            fatal: r.fatal,
            error: r.error,
            missing: [...r.missing],
          })),
          gaps: [...tally.entries()].sort((a, b) => b[1] - a[1]),
        },
        null,
        2,
      ),
    );
    return;
  }

  const ok = results.filter((r) => r.activated).length;
  console.log('');
  console.log(`activated ${ok}/${results.length}`);
  if (tally.size) {
    console.log('');
    console.log('缺失的成员（按扩展数排序）：');
    for (const [name, count] of [...tally.entries()].sort((a, b) => b[1] - a[1])) {
      console.log(`  ${String(count).padStart(2)}x  ${name}`);
    }
  } else {
    console.log('没有缺失的 API。');
  }

  process.exitCode = ok === results.length ? 0 : 1;
})();
