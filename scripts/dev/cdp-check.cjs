// Inspect the running app through WebView2's DevTools protocol.
//
//   node scripts/dev/cdp-check.cjs                 # probe the panel scheme
//   node scripts/dev/cdp-check.cjs "<js expression>"  # evaluate anything
//
// The dev instance must be started with a debugging port:
//
//   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 npm run app:dev
//
// This exists because a webview panel can render and still be dead: a missing
// origin, a blocked script and a blank frame all look the same from the outside.
// Evaluating inside the page is the only way to tell them apart.
const target = process.env.CDP || 'http://127.0.0.1:9222';

async function listTargets() {
  const res = await fetch(`${target}/json/list`);
  if (!res.ok) throw new Error(`devtools list failed: ${res.status}`);
  return res.json();
}

function send(wsUrl, method, params, timeoutMs = 20000) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(wsUrl);
    const id = 1;
    const timer = setTimeout(() => {
      try {
        ws.close();
      } catch {
        /* ignore */
      }
      reject(new Error(`${method} timed out`));
    }, timeoutMs);
    ws.addEventListener('open', () => ws.send(JSON.stringify({ id, method, params })));
    ws.addEventListener('message', (ev) => {
      const msg = JSON.parse(typeof ev.data === 'string' ? ev.data : String(ev.data));
      if (msg.id !== id) return;
      clearTimeout(timer);
      ws.close();
      if (msg.error) return reject(new Error(JSON.stringify(msg.error)));
      resolve(msg.result);
    });
    ws.addEventListener('error', () => {
      clearTimeout(timer);
      reject(new Error('devtools websocket failed'));
    });
  });
}

function evaluate(wsUrl, expression, timeoutMs = 15000) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(wsUrl);
    let id = 1;
    const timer = setTimeout(() => {
      try {
        ws.close();
      } catch {
        /* ignore */
      }
      reject(new Error('devtools evaluate timed out'));
    }, timeoutMs);

    ws.addEventListener('open', () => {
      ws.send(
        JSON.stringify({
          id,
          method: 'Runtime.evaluate',
          params: {
            expression,
            awaitPromise: true,
            returnByValue: true,
            userGesture: true,
          },
        }),
      );
    });

    ws.addEventListener('message', (ev) => {
      const msg = JSON.parse(typeof ev.data === 'string' ? ev.data : String(ev.data));
      if (msg.id !== id) return;
      clearTimeout(timer);
      ws.close();
      if (msg.error) return reject(new Error(JSON.stringify(msg.error)));
      const r = msg.result || {};
      if (r.exceptionDetails) {
        return reject(new Error(r.exceptionDetails.exception?.description || 'evaluation threw'));
      }
      resolve(r.result ? r.result.value : undefined);
    });

    ws.addEventListener('error', () => {
      clearTimeout(timer);
      reject(new Error('devtools websocket failed'));
    });
  });
}

/** Default probe: is the panel scheme actually reachable from the renderer? */
const DEFAULT_PROBE = `(async () => {
  const base = navigator.userAgent.includes('Windows')
    ? 'http://irixen-wv.localhost'
    : 'irixen-wv://localhost';
  const out = { base };
  try {
    const res = await fetch(base + '/__asset?p=' + encodeURIComponent('C:/definitely/not/here.css'));
    out.assetStatus = res.status;          // 404 => scheme works, file missing
  } catch (e) {
    out.assetError = String(e && e.message || e);
  }
  out.panels = document.querySelectorAll('.wv-panel').length;
  out.frames = [...document.querySelectorAll('.wv-frame')].map((f) => f.src || '(srcdoc)');
  return out;
})()`;

(async () => {
  const targets = await listTargets();
  // The app is not the only webview in the process: hidden login windows and
  // bridge tabs are targets too, and the first one is not necessarily ours.
  const pages = targets.filter((t) => t.type === 'page' || t.type === 'iframe');
  const want = process.env.CDP_TARGET;
  const page = want
    ? pages.find((t) => (t.url || '').includes(want))
    : pages.find((t) => /localhost:5173|tauri\.localhost/.test(t.url || '')) ||
      pages.find((t) => /IRIXEN|5173/.test(t.title || '')) ||
      pages[0];
  if (!page) throw new Error('no page target; is the app running with a debugging port?');
  if (process.env.VERBOSE) {
    console.error('targets:', pages.map((t) => `${t.url} (${t.title})`).join(' | '));
  }
  const expression = process.argv[2] || DEFAULT_PROBE;
  const shotAt = process.argv.indexOf('--shot');
  if (shotAt > 0) {
    const out = process.argv[shotAt + 1] || 'shot.png';
    const res = await send(page.webSocketDebuggerUrl, 'Page.captureScreenshot', { format: 'png' });
    require('fs').writeFileSync(out, Buffer.from(res.data, 'base64'));
    console.log('screenshot ->', out);
    return;
  }

  // `--frame <index>` runs the expression *inside* the nth iframe. Panel
  // problems (missing font, unstyled buttons) live one level down and cannot be
  // observed from the host document.
  const frameAt = process.argv.indexOf('--frame');
  if (frameAt > 0) {
    const want = Number(process.argv[frameAt + 1] || 0);
    const tree = await send(page.webSocketDebuggerUrl, 'Page.getFrameTree', {});
    const flat = [];
    const walk = (node) => {
      if (node && node.frame) flat.push(node.frame);
      for (const child of (node && node.childFrames) || []) walk(child);
    };
    walk(tree.frameTree);
    const frame = flat[want];
    if (!frame) throw new Error(`no frame #${want} (found ${flat.length})`);
    const world = await send(page.webSocketDebuggerUrl, 'Page.createIsolatedWorld', {
      frameId: frame.id,
      worldName: 'irixen-probe',
      grantUniveralAccess: true,
    });
    const res = await send(page.webSocketDebuggerUrl, 'Runtime.evaluate', {
      expression: process.argv[2] || 'document.title',
      contextId: world.executionContextId,
      awaitPromise: true,
      returnByValue: true,
    });
    if (res.exceptionDetails) {
      throw new Error(res.exceptionDetails.exception?.description || 'threw');
    }
    console.log(JSON.stringify(res.result ? res.result.value : res, null, 2));
    return;
  }

  const result = await evaluate(page.webSocketDebuggerUrl, expression);
  console.log(typeof result === 'string' ? result : JSON.stringify(result, null, 2));
})().catch((e) => {
  console.error('CDP 失败：', e.message);
  process.exit(1);
});
