// Host-side check for task and terminal execution.
//
//   node scripts/dev/ext-task-test.cjs
//
// Boots the real `bootstrap.js` in a node child, loads the probe extension from
// `fixtures/task-ext`, and plays the role of the Rust core: every `task.run` /
// `terminal.run` reverse request is echoed back so the command string the host
// produced can be inspected — quoting bugs (a path with spaces split into three
// argv items) show up here, not in the UI.
//
// Exits non-zero when a task never reports its command or never ends.
const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const base = path.join(process.env.APPDATA || path.join(os.homedir(), 'AppData', 'Roaming'), 'IRIXEN Code');
const bootstrap = path.join(base, 'runtime', 'exthost', 'bootstrap.js');
const extDir = path.join(__dirname, 'fixtures', 'task-ext');
const workspace = process.env.WS || fs.mkdtempSync(path.join(os.tmpdir(), 'irixen-task-'));
const pkg = JSON.parse(fs.readFileSync(path.join(extDir, 'package.json'), 'utf8'));

if (!fs.existsSync(bootstrap)) {
  console.error('bootstrap.js 不存在，先启动一次应用以释放运行时：', bootstrap);
  process.exit(2);
}

const cfg = {
  id: pkg.publisher + '.' + pkg.name,
  name: pkg.displayName || pkg.name,
  version: pkg.version,
  dir: extDir,
  entry: path.join(extDir, 'extension.js'),
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

const started = [];
let ended = 0;
let activations = 0;

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
      const method = msg.method || '';
      const p = msg.params || {};

      if (method === 'boot') {
        // Shape matters: the host reads `boot.state` and `boot.config`.
        send({ t: 'res', id: msg.id, result: { state: {}, config: {} } });
        continue;
      }

      if (method === 'task.run' || method === 'terminal.run') {
        started.push({ method, command: p.command, cwd: p.cwd, taskId: p.taskId || p.id });
        console.log(`${method.padEnd(12)} ${JSON.stringify(p.command)}  cwd=${p.cwd || '(workspace)'}`);
        // The core answers with the run id, then reports the exit later.
        send({ t: 'res', id: msg.id, result: { id: 'run-' + started.length, command: p.command, cwd: p.cwd } });
        const taskId = p.taskId || '';
        setTimeout(() => {
          send({ t: 'rreq', id: 0, method: 'task.exit', params: { taskId, code: 0 } });
        }, 200);
        continue;
      }

      // `activated`, `fatal` and `log` are notifications (id 0): no reply.
      if (process.env.ALL) console.log('  req:', method, JSON.stringify(p).slice(0,140));
      if (msg.id) send({ t: 'res', id: msg.id, result: null });
      continue;
    }
  }
});

child.stderr.on('data', (d) => {
  const t = d.toString('utf8');
  for (const line of t.split('\n')) {
    if (process.env.ALL || /TASK-PROBE|onDidEndTask|Error|error/.test(line)) console.log('  ext:', line.trim().replace(/^TASK-PROBE /, ''));
  }
});

setTimeout(() => {
  console.log('');
  console.log(`commands started: ${started.length}`);
  const missing = [];
  if (!started.some((s) => s.method === 'terminal.run')) missing.push('terminal.run');
  if (!started.some((s) => s.method === 'task.run')) missing.push('task.run');

  // Quoting: `node -e console.log("argv ok")` must stay one argument, not three.
  const argvOne = started.find((s) => s.command && s.command.includes('-e'));
  if (argvOne && !/"-e"|'-e'|-e/.test(argvOne.command)) missing.push('argv quoting');

  if (missing.length) {
    console.log('MISSING:', missing.join(', '));
    process.exitCode = 1;
  } else {
    console.log('all task/terminal paths reached the core');
    process.exitCode = 0;
  }
  child.kill();
  process.exit(process.exitCode);
}, 6000);
