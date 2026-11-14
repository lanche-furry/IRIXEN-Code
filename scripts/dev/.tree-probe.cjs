// One-off probe: run the real extension-host bootstrap against a tiny fake
// extension that registers a TreeDataProvider, then exercise the tree.* reverse
// RPC exactly as the Rust core does. Removed after verification.
const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');

const root = path.resolve(__dirname, '../..');
const tmp = path.join(root, '.tree-probe');
fs.mkdirSync(path.join(tmp, 'ext'), { recursive: true });
fs.copyFileSync(
  path.join(root, 'src-tauri/src/exthost/bootstrap.js'),
  path.join(tmp, 'bootstrap.cjs'),
);
fs.writeFileSync(path.join(tmp, 'ext/index.cjs'), `
const vscode = require('vscode');
const emitter = new vscode.EventEmitter();
class Node {
  constructor(label, kids) { this.label = label; this.kids = kids || []; this.parent = null; }
}
const rootA = new Node('Root A', [new Node('Child A1'), new Node('Child A2')]);
const rootB = new Node('Root B');
rootA.kids.forEach(k => k.parent = rootA); // circular: must NOT reach the wire
let clicked = null;
const provider = {
  onDidChangeTreeData: emitter.event,
  getTreeItem(el) {
    const item = new vscode.TreeItem(el.label, el.kids.length ? vscode.TreeItemCollapsibleState.Collapsed : vscode.TreeItemCollapsibleState.None);
    item.description = 'desc';
    item.contextValue = el.kids.length ? 'group' : 'task';
    item.iconPath = new vscode.ThemeIcon(el.kids.length ? 'folder' : 'play');
    item.command = { command: 'fake.run', title: 'Run', arguments: [el] };
    item.resourceUri = vscode.Uri.file('C:/proj/' + el.label + '.gradle');
    return item;
  },
  getChildren(el) { return el ? el.kids : [rootA, rootB]; },
};
function activate() {
  vscode.window.createTreeView('fakeView', { treeDataProvider: provider, showCollapseAll: true });
  vscode.commands.registerCommand('fake.run', (el) => { clicked = el && el.label; });
  global.__clicked = () => clicked;
}
exports.activate = activate;
`, 'utf8');

const cfg = {
  id: 'probe.fake',
  version: '1.0.0',
  dir: path.join(tmp, 'ext'),
  entry: path.join(tmp, 'ext/index.cjs'),
  storageDir: path.join(tmp, 'storage'),
  package: { name: 'fake', publisher: 'probe' },
};

const child = spawn(process.execPath, [path.join(tmp, 'bootstrap.cjs')], {
  env: { ...process.env, IRIXEN_HOST_CFG: JSON.stringify(cfg) },
  stdio: ['pipe', 'pipe', 'pipe'],
});
let buf = '';
let stage = 'boot';
let firstKey = '';
const notices = [];
const write = (o) => child.stdin.write(JSON.stringify(o) + '\n');

child.stdout.on('data', (chunk) => {
  buf += chunk.toString('utf8');
  let i;
  while ((i = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, i); buf = buf.slice(i + 1);
    let m; try { m = JSON.parse(line); } catch { continue; }
    if (m.t === 'req') {
      console.log('REQ:', m.method, JSON.stringify(m.params || {}).slice(0, 220));
      if (m.method === 'boot') write({ t: 'res', id: m.id, result: { state: {}, config: {} } });
      else if (m.method === 'tree.register') { notices.push(m.params); stage = 'registered'; }
      else if (m.method === 'commands.put' || m.method === 'log' || m.method === 'ready' || m.method === 'activated') {
        if (m.method === 'activated' && stage === 'registered') {
          write({ t: 'rreq', id: 101, method: 'tree.children', params: { viewId: 'fakeView', pid: notices[0].pid, key: '' } });
          stage = 'children';
        }
      }
    } else if (m.t === 'rres') {
      if (m.id === 101) {
        console.log('CHILDREN:', JSON.stringify(m.result, null, 1).slice(0, 1600));
        if (m.error) console.log('ERROR:', m.error);
        const parent = (m.result || []).find((r) => r.state === 1);
        firstKey = parent && parent.key;
        write({ t: 'rreq', id: 102, method: 'tree.children', params: { viewId: 'fakeView', pid: notices[0].pid, key: firstKey } });
      } else if (m.id === 102) {
        console.log('GRANDCHILDREN:', JSON.stringify(m.result));
        const child2 = (m.result || [])[0];
        write({ t: 'rreq', id: 103, method: 'tree.command', params: { viewId: 'fakeView', pid: notices[0].pid, key: child2 && child2.key } });
      } else if (m.id === 103) {
        console.log('COMMAND RESULT:', JSON.stringify({ result: m.result, error: m.error }));
        console.log('REGISTER:', JSON.stringify(notices));
        child.kill();
      }
    }
  }
});
child.stderr.on('data', (d) => {
  const s = d.toString('utf8').trim();
  if (s) console.log('STDERR:', s.slice(0, 400));
});
child.on('exit', (code) => { console.log('EXIT:', code); process.exit(0); });
setTimeout(() => { console.log('TIMEOUT stage=', stage); child.kill(); process.exit(1); }, 12000);
