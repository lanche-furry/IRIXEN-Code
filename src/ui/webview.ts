// Extension webview panels.
//
// A VS Code webview is an ordinary HTML document with its own origin plus a
// message channel. Here the document is published to the `irixen-wv` scheme
// (see `wvproto.rs`) and loaded in an iframe, with an `acquireVsCodeApi()` shim
// injected at the top of `<head>` and a `postMessage` bridge that maps 1:1 to
// `webview.postMessage` / `webview.onDidReceiveMessage`.
//
// The first version used `<iframe srcdoc>`. It rendered, but panels did not
// *work*: a srcdoc document has no origin of its own, `document.baseURI` points
// at the IDE, and any `file://` resource is refused. Live Preview's toolbar and
// preview iframe depend on exactly those things.

import { onExtHostEvent, api } from '../core/api';
import { h, clear } from '../core/dom';
import { icon } from './icons';
import { t } from '../core/i18n';
import { reportProblem, toast } from './toast';
import type { WebviewPanel } from '../types';

interface Panel {
  data: WebviewPanel;
  root: HTMLElement;
  frame: HTMLIFrameElement;
  titleEl: HTMLElement;
  /** `src` loads asynchronously; messages sent before load are lost. */
  ready: boolean;
  pending: unknown[];
  /** The published document really executed. A 404 fires `load` too, so the
   *  frame's load event alone cannot tell a healthy panel from a blank one. */
  loaded: boolean;
  /** Republish attempts made after the document failed to check in. */
  retries: number;
  /** A dead panel is reported once, not on every subsequent load event. */
  reported: boolean;
  /** Monotonic render token, so an older publish cannot overwrite a newer one. */
  seq: number;
  /** Last published document, kept for the manual reload button. */
  html: string;
  /** Fingerprint of that document with volatile tokens (CSP nonce) removed. */
  key: string;
}

/**
 * Live Preview rebuilds its whole HTML on every `reveal`, generating a fresh
 * CSP nonce each time. Byte-for-byte comparison therefore always differs and
 * reloading the iframe would reset the page being previewed. Strip the volatile
 * tokens so two functionally identical documents compare equal.
 */
function docKey(html: string): string {
  return html
    .replace(/\bnonce-([^'"\s<>]+)/g, 'nonce-_')
    .replace(/(\bnonce\s*=\s*["'])[^"']*(["'])/gi, '$1_$2');
}

type WebviewEvent = { action: string; panel?: WebviewPanel; id?: string; message?: unknown };

/**
 * Frames emitted before the shell mounts.
 *
 * Extensions are auto-activated during app startup, frequently before the dock
 * exists. Their `webview.create` used to land while `layer` was still null, so
 * `create()` returned early and the first click appeared to do nothing until a
 * late sweep happened to notice the row. Subscribing at module load and
 * buffering frames makes the subscription independent of the render tree.
 */
const earlyEvents: WebviewEvent[] = [];
void onExtHostEvent<WebviewEvent>('webview', (ev) => {
  if (!layer) {
    if (earlyEvents.length < 64) earlyEvents.push(ev);
    return;
  }
  handle(ev);
});

const panels = new Map<string, Panel>();
let layer: HTMLElement | null = null;
/** Origin the panel documents are served from. Windows maps the scheme to
 *  `http://<scheme>.localhost`; every other platform keeps `<scheme>://`. */
let wvBase = 'http://irixen-wv.localhost';
/** `create` can arrive before `webviewBase()` resolves; every render waits. */
let baseReady: Promise<void> = Promise.resolve();

export function mountWebviews() {
  layer = h('div', { class: 'wv-layer' });
  document.body.append(layer);

  window.addEventListener('message', (e: MessageEvent) => {
    const data = e.data as {
      __irixen_wv?: string; type?: string; data?: unknown; src?: string;
      dev?: boolean; force?: boolean;
    } | null;
    if (!data || !data.__irixen_wv) return;
    const id = data.__irixen_wv;
    if (data.type === 'preview-src' && data.src) {
      void probePreview(id, data.src, !!data.dev, 0, !!data.force);
      return;
    }
    if (data.type === 'preview-switched') {
      toast(t('preview.switched'), 'ok');
      return;
    }
    if (data.type === 'loaded') {
      // Beacon from the injected bridge. This is the only reliable proof that
      // the scheme served *our* document rather than a 404 / error page.
      const p = panels.get(id);
      if (p) {
        p.loaded = true;
        p.retries = 0;
        markReady(p);
      }
      return;
    }
    if (data.type === 'open' && typeof data.data === 'string') {
      void api.openExternal(data.data);
      return;
    }
    void api.webviewPost(id, data.data ?? null).catch(() => undefined);
  });

  // Panels that a host is still holding after a reload come back with the app.
  baseReady = api
    .webviewBase()
    .then((base) => {
      if (base) wvBase = base;
    })
    .catch(() => undefined);
  // Replay everything the extensions announced before the dock existed. The
  // subscription is already live, so later frames go straight to `handle`.
  for (const ev of earlyEvents.splice(0)) handle(ev);

  // Panels whose `create` was emitted before the process had a window at all
  // still live in the core; the list is the authoritative snapshot.
  void baseReady
    .then(() => api.webviewList())
    .then((rows) => rows.filter((row) => !panels.has(row.id)).forEach((row) => create(row)))
    .catch(() => undefined);
  // A host may be restarted while the renderer is loading, which drops its
  // rows and re-announces them later. A few short sweeps pick those up too.
  // `close` removes the row from the core, so a panel the user deliberately
  // closed is not resurrected.
  for (const delay of [800, 2000, 3800, 6000]) {
    void baseReady
      .then(() => new Promise<void>((r) => setTimeout(r, delay)))
      .then(() => api.webviewList())
      .then((rows) => rows.filter((row) => !panels.has(row.id)).forEach((row) => create(row)))
      .catch(() => undefined);
  }
}

function handle(ev: { action: string; panel?: WebviewPanel; id?: string; message?: unknown }) {
  if (ev.action === 'create' && ev.panel) {
    create(ev.panel);
    return;
  }
  if (ev.action === 'update' && ev.panel) {
    // A `create` can be emitted before the renderer subscribes (cold start,
    // extension activating while the UI is still mounting). An update carries
    // the full document too, so treat it as a create when the panel is absent.
    if (!panels.has(ev.panel.id)) create(ev.panel);
    const p = panels.get(ev.panel.id);
    if (p) {
      p.data = ev.panel;
      p.titleEl.textContent = ev.panel.title || ev.panel.viewType || '';
      void render(p, ev.panel.html || '');
    }
    return;
  }
  if (ev.action === 'reveal' && ev.id) {
    const p = panels.get(ev.id);
    if (p) {
      // Re-append moves it above any panel opened later.
      layer?.append(p.root);
      if (ev.panel) {
        p.data = ev.panel;
        p.titleEl.textContent = ev.panel.title || ev.panel.viewType || '';
      }
      const html = ev.panel?.html || '';
      // A reveal is the user asking for the panel again. If its document never
      // checked in (404, cold scheme, stale `forget`), republishing is the only
      // way back short of restarting the extension. Once it has loaded,
      // `docKey` suppresses an identical document, so the previewed page and
      // its scroll position are preserved.
      if (!p.loaded) void recover(p, html || p.html || p.data.html || '');
      else {
        if (html) void render(p, html);
        // Re-arm the preview watchdog. It used to expire 15s after the panel
        // loaded, so a second click on an already-open preview could not retry
        // a dead embedded iframe — only recreating the panel (restarting the
        // extension) ever helped.
        panelCtl(p, { action: 'reveal' });
      }
      return;
    }
    // The renderer restarted but the core still has the row.
    void api
      .webviewList()
      .then((rows) => {
        const row = rows.find((r) => r.id === ev.id);
        if (row) create(row);
      })
      .catch(() => undefined);
    return;
  }
  if (ev.action === 'post' && ev.id) {
    const p = panels.get(ev.id);
    if (p) {
      if (p.ready) deliver(p, ev.message);
      else p.pending.push(ev.message);
      return;
    }
    // Messages can be the only frame we get after a lost create. Rebuild the
    // panel from the core's row, then deliver (or buffer until the frame loads).
    void api
      .webviewList()
      .then((rows) => {
        const row = rows.find((r) => r.id === ev.id);
        if (!row || panels.has(ev.id!)) return;
        create(row);
        const made = panels.get(ev.id!);
        if (made) {
          if (made.ready) deliver(made, ev.message);
          else made.pending.push(ev.message);
        }
      })
      .catch(() => undefined);
    return;
  }
  if (ev.action === 'dispose' && ev.id) close(ev.id, false);
}

function deliver(p: Panel, message: unknown) {
  p.frame.contentWindow?.postMessage({ __irixen_wv_in: p.data.id, data: message }, '*');
}

/** IDE -> shim control channel, deliberately separate from extension messages. */
function panelCtl(p: Panel, payload: Record<string, unknown>) {
  try {
    p.frame.contentWindow?.postMessage({ __irixen_wv_ctl: p.data.id, ...payload }, '*');
  } catch {
    /* the frame may be mid-navigation */
  }
}

const probed = new Map<string, { src: string; at: number }>();

/**
 * Ask Rust what the preview URL actually served.
 *
 * The previewed page lives on `127.0.0.1:<live-server-port>`, a different
 * origin from both the IDE and the panel document, so neither can read its DOM.
 * A blank body (a Vite entry point served as `video/mp2t`) looks exactly like a
 * healthy page from the outside. Probing the URL server-side is the only way to
 * replace that white rectangle with a real explanation.
 */
async function probePreview(id: string, src: string, dev: boolean, attempt = 0, force = false) {
  // `dev` marks a URL we already switched to; probing it would just re-report
  // the same bundler signature and open the hint again.
  // Live Preview writes `src="http://127.0.0.1:3000"` with no trailing slash,
  // so the path separator has to be optional.
  if (!src || dev || !/^https?:\/\/(127\.0\.0\.1|localhost)(:\d+)?(\/|$)/i.test(src)) return;
  const last = probed.get(id);
  const now = Date.now();
  // Scheduled retries and an explicit "retry" click must bypass the debounce.
  if (attempt === 0 && !force && last && last.src === src && now - last.at < 4000) return;
  probed.set(id, { src, at: now });
  const p = panels.get(id);
  if (!p) return;
  let result;
  try {
    result = await api.previewProbe(src);
  } catch {
    return; // probe is best-effort; never break a working preview
  }
  if (panels.get(id) !== p) return;
  if (result.reason === 'ok') {
    panelCtl(p, { action: 'clear' });
    return;
  }
  // The static server often starts a beat after the panel. Give the watchdog
  // time to reconnect before calling the preview dead.
  if (result.reason === 'unreachable' && attempt < 4) {
    setTimeout(() => void probePreview(id, src, false, attempt + 1), 2500);
    return;
  }
  const body =
    result.reason === 'bundler' ? t('preview.reasonBundler')
      : result.reason === 'unreachable' ? t('preview.reasonUnreachable')
        : result.reason === 'http' ? t('preview.reasonHttp', { status: result.status })
          : t('preview.reasonNotHtml');
  panelCtl(p, {
    action: 'diagnosis',
    title: t('preview.blankTitle'),
    body,
    devUrl: result.dev_url,
    devLabel: t('preview.devServer'),
    retryLabel: t('preview.retry'),
  });
}

/** Flush whatever the extension sent while the frame was still loading. */
function markReady(p: Panel) {
  p.ready = true;
  const queued = p.pending.splice(0);
  for (const message of queued) deliver(p, message);
}

/** Publish the document and point the frame at it. */
async function render(p: Panel, html: string, force = false) {
  // `webview.create` ships an empty document. Publishing it used to race the
  // first real update: whichever invoke resolved last became the iframe src,
  // so the panel was blank about half the time. Only render real documents,
  // and let the newest render win.
  //
  // Live Preview re-assigns `webview.html` on every reveal *and* the setter
  // flushes again 50ms later — two identical updates reloaded the iframe and
  // reset the page being previewed. Skip documents we have already published.
  const key = docKey(html);
  if (!html || (!force && key === p.key)) return;
  p.html = html;
  p.key = key;
  const seq = (p.seq += 1);
  p.ready = false;
  p.loaded = false;
  await baseReady;
  const doc = page(html, p.data.id);
  try {
    const url = await api.webviewPublish(p.data.id, doc);
    // Guard against a slow publish landing after a newer one or a dispose.
    if (seq !== p.seq || panels.get(p.data.id) !== p) return;
    p.frame.src = url;
  } catch {
    if (seq !== p.seq) return;
    // The scheme is unavailable (older binary): fall back to srcdoc so the
    // panel at least shows something instead of staying blank. Clear the
    // fingerprint so a later update can retry the scheme; otherwise an
    // identical document is skipped forever and the panel stays on srcdoc.
    p.key = '';
    p.frame.srcdoc = doc;
  }
}

/**
 * A panel document can disappear between `publish` and the iframe request:
 * a stale `forget`, a host restart, or a cold custom scheme. WebView2 fires
 * `load` for the resulting error page too, so the load handler alone used to
 * declare the panel healthy while the user saw a blank rectangle. Only the
 * injected bridge's `loaded` beacon proves the document ran.
 */
async function recover(p: Panel, html: string) {
  if (!html) {
    // A panel rebuilt from an early `post` frame has no document, and no
    // `load` event ever fires for an iframe without `src`. The core keeps the
    // authoritative row, so pull it instead of sitting on an empty frame.
    try {
      const rows = await api.webviewList();
      html = rows.find((r) => r.id === p.data.id)?.html || '';
    } catch {
      return;
    }
  }
  if (!html || p.loaded || panels.get(p.data.id) !== p) return;
  await render(p, html, true);
}

function ensureLoaded(p: Panel) {
  if (panels.get(p.data.id) !== p || p.loaded) return;
  if (p.retries >= 3) {
    if (!p.reported) {
      p.reported = true;
      // The user is looking at a white rectangle. Offer the same recovery the
      // old advice gave ("restart the extension"), with the run log attached.
      reportProblem(
        `webview panel ${p.data.id} stayed blank after 3 republish attempts`,
        'webview',
        'plugin',
        p.data.ext || '',
      );
    }
    return;
  }
  p.retries += 1;
  void recover(p, p.html || p.data.html || '');
}

function create(data: WebviewPanel) {
  if (panels.has(data.id)) {
    const existing = panels.get(data.id)!;
    existing.data = data;
    existing.titleEl.textContent = data.title || data.viewType;
    void render(existing, data.html || '');
    return;
  }
  if (!layer) return;

  const titleEl = h('span', { class: 'wv-title' }, data.title || data.viewType || 'Webview');
  // No `sandbox`: the document has its own origin and the extension behind it
  // already runs arbitrary code in the host process. A sandbox here would only
  // break `allow-same-origin` semantics for the preview iframe inside it.
  const frame = h('iframe', { class: 'wv-frame' }) as HTMLIFrameElement;

  const reload = h(
    'button',
    {
      class: 'icon-btn',
      title: t('webview.reload'),
      onClick: () => {
        const current = panels.get(data.id);
        if (!current) return;
        void render(current, current.html || current.data.html || '', true);
      },
    },
    icon('refresh', 13),
  );
  const closeBtn = h(
    'button',
    { class: 'icon-btn', title: t('common.close'), onClick: () => close(data.id, true) },
    icon('close', 13),
  );

  const root = h(
    'div',
    { class: 'wv-panel' },
    h('div', { class: 'wv-head' }, h('span', { class: 'wv-dot' }), titleEl, h('span', { class: 'wv-fill' }), reload, closeBtn),
    frame,
  );
  layer.append(root);
  const panel: Panel = {
    data, root, frame, titleEl, ready: false, pending: [],
    loaded: false, retries: 0, reported: false, seq: 0, html: '', key: '',
  };
  frame.addEventListener('load', () => {
    // An error page fires `load` as well. Wait for the bridge beacon; only
    // flush queued extension messages into a document we know is alive.
    setTimeout(() => {
      if (!panel.loaded) ensureLoaded(panel);
      else markReady(panel);
    }, 800);
  });
  panels.set(data.id, panel);
  void render(panel, data.html || '');
  // An empty document means the frame has no `src`, so no `load` event will
  // ever run the health check. Schedule one explicitly: the core may already
  // hold the real document (early `post`, restarted host).
  if (!data.html) setTimeout(() => ensureLoaded(panel), 1200);
}

function close(id: string, notifyHost: boolean) {
  const p = panels.get(id);
  if (!p) return;
  p.root.remove();
  panels.delete(id);
  void api.webviewForget(id).catch(() => undefined);
  if (notifyHost) void api.webviewClose(id).catch(() => undefined);
}

/**
 * Inject the `vscode` bridge before any page script runs.
 *
 * CAUTION: the shim below is a TEMPLATE LITERAL, so it is string-escaped before
 * it ever reaches the panel. A regex literal containing escaped characters
 * loses them in transit and turns the whole inline script into a SyntaxError —
 * which kills `acquireVsCodeApi` and every listener with no visible symptom
 * beyond an inert toolbar. Keep backslashes out of the shim body; build regexes
 * with `new RegExp("...")` and character classes instead.
 */
function page(html: string, id: string): string {
  // Extensions ship a CSP that only trusts VS Code's own resource origin
  // (`https://file+.vscode-resource.vscode-cdn.net`) and a single script nonce.
  // That origin does not exist here, so the policy would block both the bridge
  // below and the panel's own stylesheets. The panel is extension code, which
  // already runs with full trust in the host process, so the policy buys
  // nothing — but the nonce is reused for pages that check for it.
  const nonce = /\bnonce-([^'"\s]+)/.exec(html)?.[1];
  let out = html.replace(/<meta[^>]*http-equiv=["']?content-security-policy["']?[^>]*>/gi, '');
  out = remapResources(out);

  const shim = `<script${nonce ? ` nonce="${nonce}"` : ''}>(function(){
var id=${JSON.stringify(id)};
var state;
var api=null;
window.acquireVsCodeApi=function(){
  // VS Code returns the same object every time; a second call throws.
  if(api) return api;
  api={
    postMessage:function(m){parent.postMessage({__irixen_wv:id,type:"msg",data:m},"*");},
    setState:function(s){state=s;parent.postMessage({__irixen_wv:id,type:"state",data:s},"*");},
    getState:function(){return state;}
  };
  return api;
};
window.addEventListener("message",function(e){
  if(!e.data||e.data.__irixen_wv_in!==id)return;
  try{window.dispatchEvent(new MessageEvent("message",{data:e.data.data}));}catch(_){}
});
document.addEventListener("click",function(e){
  var a=e.target&&e.target.closest?e.target.closest("a[href]"):null;
  if(!a)return;
  var href=a.getAttribute("href")||"";
  if(/^https?:/i.test(href)){e.preventDefault();parent.postMessage({__irixen_wv:id,type:"open",data:href},"*");}
},true);
// Proof that the scheme served our document: WebView2 fires load for a 404
// too, so the IDE cannot tell a healthy panel from a blank one without this.
try{parent.postMessage({__irixen_wv:id,type:"loaded"},"*");}catch(_){}
// Live Preview injects a script into the previewed page which posts
// update-path back here. If the embedded iframe loads before the extension's
// HTTP server is listening (or against a stale port), it shows an error page
// forever: neither the extension nor VS Code retries. Re-navigate only that
// iframe until the handshake arrives — reloading the whole panel would throw
// away the toolbar, history and scroll position for nothing.
var pvAlive=false,pvTries=0,pvSrc="",pvStart=Date.now(),pvArmed=true,pvHint=null;
function pvFrame(){return document.getElementById("hostedContent");}
// Built from a string, not a regex literal: this whole shim lives inside a TS
// template literal, where regex backslash escapes are consumed as string
// escapes. The injected script once arrived with a broken regex — a SyntaxError
// that silently killed the entire bridge. No backslashes in here, on purpose.
var pvLocalRe=new RegExp("^https?://(127[.]0[.]0[.]1|localhost)(:[0-9]+)?(/|$)","i");
function pvLocal(s){return pvLocalRe.test(s);}
function pvPost(type,extra){var m={__irixen_wv:id,type:type};if(extra)for(var k in extra)m[k]=extra[k];try{parent.postMessage(m,"*");}catch(_){}}
function pvHide(){if(pvHint&&pvHint.parentNode)pvHint.parentNode.removeChild(pvHint);pvHint=null;}
function pvArm(){pvAlive=false;pvTries=0;pvStart=Date.now();pvArmed=true;}
function pvHintUI(d){
  pvHide();
  var f=pvFrame();if(!f||!f.parentNode)return;
  var box=f.parentNode;
  try{if(getComputedStyle(box).position==="static")box.style.position="relative";}catch(_){}
  var dark=false;try{dark=window.matchMedia("(prefers-color-scheme: dark)").matches;}catch(_){}
  var el=document.createElement("div");el.id="irixen-preview-hint";
  el.setAttribute("style","position:absolute;inset:0;z-index:50;display:flex;align-items:center;justify-content:center;background:rgba(128,128,128,.12);font:13px/1.6 system-ui,sans-serif;");
  var card=document.createElement("div");
  card.setAttribute("style","max-width:min(440px,88%);padding:18px 20px;border-radius:10px;border:1px solid rgba(128,128,128,.4);background:"+(dark?"#1f242c":"#fff")+";color:"+(dark?"#e6e9ee":"#24292f")+";box-shadow:0 8px 30px rgba(0,0,0,.18);");
  var h=document.createElement("div");h.textContent=d.title||"";h.setAttribute("style","font-weight:650;font-size:14px;margin-bottom:8px;");card.appendChild(h);
  var para=document.createElement("div");para.textContent=d.body||"";para.setAttribute("style","margin-bottom:14px;white-space:pre-wrap;");card.appendChild(para);
  var row=document.createElement("div");row.setAttribute("style","display:flex;gap:8px;justify-content:flex-end;flex-wrap:wrap;");
  function mk(label,primary,fn){
    var b=document.createElement("button");b.textContent=label;
    b.setAttribute("style","padding:6px 12px;border-radius:6px;border:1px solid "+(dark?"#4a525e":"#c8cdd4")+";background:"+(primary?"#0b62d6":dark?"#2b323c":"#fff")+";color:"+(primary?"#fff":dark?"#e6e9ee":"#24292f")+";cursor:pointer;font:12px system-ui,sans-serif;");
    b.onclick=fn;row.appendChild(b);
  }
  mk(d.retryLabel||"Retry",false,function(){pvHide();pvArm();try{f.src=f.src;pvPost("preview-src",{src:f.src,dev:false,force:true});}catch(_){}});
  if(d.devUrl)mk(d.devLabel||"Dev server",true,function(){
    try{f.setAttribute("data-irixen-dev","1");f.src=d.devUrl;pvPost("preview-switched",{});pvHide();pvArm();}catch(_){}
  });
  card.appendChild(row);el.appendChild(card);box.appendChild(el);pvHint=el;
}
window.addEventListener("message",function(e){
  var d=e.data;
  if(d&&d.__irixen_wv_ctl===id){
    if(d.action==="reveal"){
      pvArm();
      try{var f=pvFrame();if(f&&f.contentWindow)f.contentWindow.postMessage({command:"setup-parent-listener"},"*");}catch(_){}
    }else if(d.action==="diagnosis")pvHintUI(d);
    else if(d.action==="clear")pvHide();
    return;
  }
  if(d&&(d.command==="update-path"||d.command==="console"||d.command==="show-find-icon"||d.command==="did-keydown"))pvAlive=true;
});
function pvTick(){
  var f=pvFrame(),wait=document.hidden?30000:10000;
  if(f){
    // Use the resolved property, not the attribute: assigning iframe.src back
    // to itself (the retry gesture) rewrites the attribute to an absolute URL
    // with a trailing slash, which used to look like a navigation and dismiss
    // the diagnosis card immediately.
    var src=f.src||"";
    if(src!==pvSrc){
      pvSrc=src;pvHide();pvArm();
      // The IDE cannot read this cross-origin frame, so it asks Rust what the
      // URL served. That is how a Vite SPA silently showing an empty body gets
      // a real explanation instead of a white rectangle.
      pvPost("preview-src",{src:src,dev:f.getAttribute("data-irixen-dev")==="1"});
    }
    // Only the extension's own local server. An external page will never send
    // the handshake, and retrying it would reload whatever the user opened.
    // A successful handshake ends retries, but reveal can re-arm them.
    var retrying=pvArmed&&!pvAlive&&pvLocal(src)&&Date.now()-pvStart<20000&&pvTries<12;
    if(retrying&&!document.hidden){pvTries++;try{f.src=src;}catch(_){}}
    if(pvAlive||Date.now()-pvStart>20000)pvArmed=false;
    // A settled preview does not need 1.5 s wake-ups forever. Fast cadence is
    // only for the short retry window; a visible idle panel checks every 5 s,
    // no panel at all every 10 s, a hidden window every 30 s.
    wait=document.hidden?30000:(retrying?1500:5000);
  }
  setTimeout(pvTick,wait);
}
setTimeout(pvTick,800);
})();</script>`;
  if (/<head[^>]*>/i.test(out)) return out.replace(/<head[^>]*>/i, (m) => m + shim);
  if (/<html[^>]*>/i.test(out)) return out.replace(/<html[^>]*>/i, (m) => `${m}<head>${shim}</head>`);
  return shim + out;
}

/**
 * Point `file://` and VS Code's `vscode-cdn` URLs at the panel scheme.
 *
 * An extension calls `webview.asWebviewUri(uri)`, which in real VS Code returns
 * an `https://file+.vscode-resource…` URL. The host hands out plain `file://`
 * here, and a panel document cannot read `file://` (different origin), so its
 * stylesheet, script and font would silently 404 — the panel renders as an
 * unstyled, inert shell. Proxying through the same scheme keeps it same-origin.
 */
export function remapResources(html: string): string {
  const localUrl = (raw: string): string => {
    let p = raw.trim();
    const cdn = /^https?:\/\/file\+\.vscode-resource\.vscode-cdn\.net\/(.*)$/i.exec(p);
    if (cdn) p = cdn[1];
    else p = p.replace(/^file:\/\/\/?/i, '');
    // A query string is a cache-buster, not part of the filesystem path. Keep
    // it on the proxy URL so `?v=…` still busts the browser cache.
    const q = p.indexOf('?');
    const query = q >= 0 ? p.slice(q) : '';
    if (q >= 0) p = p.slice(0, q);
    try {
      p = decodeURIComponent(p);
    } catch {
      /* keep the raw value if it is not valid percent-encoding */
    }
    if (/^\/[a-zA-Z]:/.test(p)) p = p.slice(1); // /C:/dir → C:/dir
    // Path form, and each segment encoded separately so the `/` separators stay
    // literal. That matters twice over: a stylesheet's own `url(./codicon.ttf)`
    // is resolved against this URL, and if the whole path were one encoded
    // segment (`C%3A%2F…%2Fmedia%2Fcodicon.css`) the relative reference would
    // replace that single segment and lose the directory — which is exactly why
    // the icon font failed to load and every button showed an empty box.
    const encoded = p.split('/').map(encodeURIComponent).join('/');
    return `${wvBase}/__asset/${encoded}${query}`;
  };

  // Cover the whole document, not just `src=`/`href=`. Extensions put
  // asWebviewUri results in `srcset`, `poster`, inline <style> url(), JS
  // template strings and DOM built at runtime. A URL-shaped attribute regex
  // silently 404'd all of those, leaving an unstyled, inert panel.
  return html.replace(
    /(?:https?:\/\/file\+\.vscode-resource\.vscode-cdn\.net\/|file:\/\/\/?)[^\s"'<>\);,}]+/gi,
    (whole) => localUrl(whole),
  );
}

/** Kept for tests/tools that want to clear the layer. */
export function disposeAll() {
  if (!layer) return;
  clear(layer);
  panels.clear();
}
