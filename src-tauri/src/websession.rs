//! Persistent, logged-in browser sessions for Web2API.
//!
//! A raw HTTP request to a vendor's private endpoint is not enough any more:
//! `claude.ai/api/organizations` answers 403 and `chatgpt.com/backend-api/...`
//! answers 401 / "Unusual activity has been detected" even with a pasted
//! session cookie. Two things make this work instead:
//!
//! 1. **The vendor's own page, kept alive.** One WebView window per source is
//!    created once; closing it hides it rather than destroying it, so the
//!    cookie jar, localStorage, UA, origin, CSRF and PoW tokens all survive
//!    for the whole app lifetime (and WebView2's shared profile keeps them on
//!    disk across restarts).
//! 2. **Driving the page's own composer.** Instead of calling the guarded
//!    private JSON API, the default `ui` driver types the prompt into the
//!    page's input and reads the rendered answer. The page's own JavaScript
//!    supplies whatever sentinel / proof-of-work tokens the vendor wants.
//!    The legacy `api` driver still POSTs to the private endpoint for sources
//!    that allow it; it is one click away in the panel.
//!
//! Results come back through `eval_with_callback` reading a well-known global
//! — no local callback server, no CORS, no mixed-content, and the `fetch`
//! (when used) stays same-origin so the vendor's CSP allows it.

use crate::config::{BridgeConfig, WebSource};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::oneshot;

static APP: OnceLock<AppHandle> = OnceLock::new();
static SEQ: AtomicU64 = AtomicU64::new(1);
/// Cached login state, keyed by source id. Written by `probe_login`, read by
/// the bridge's `/v1/models` so a source that is not logged in never leaks a
/// model into the agent's picker.
static STATES: OnceLock<Mutex<HashMap<String, SessionState>>> = OnceLock::new();
/// Login probe endpoints, synced from config so the background watcher does
/// not need to borrow the app state.
static ENDPOINTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
/// The login watcher starts once: at boot only when `bridge.auto_start` is on,
/// otherwise on the first manual bridge start. Idempotent so both paths can
/// call `start_watcher()` without stacking 7-second loops.
static WATCHER_STARTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
struct SessionState {
    logged_in: bool,
    account: String,
    detail: String,
    checked_at: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub label: String,
    pub open: bool,
    pub logged_in: bool,
    pub account: String,
    pub detail: String,
    pub checked_at: u64,
}

fn states() -> &'static Mutex<HashMap<String, SessionState>> {
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}
fn endpoints() -> &'static Mutex<HashMap<String, String>> {
    ENDPOINTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn init(app: AppHandle) {
    let _ = APP.set(app);
}

pub fn label(source_id: &str) -> String {
    format!("web2api-{source_id}")
}

pub fn is_open(source_id: &str) -> bool {
    APP.get()
        .and_then(|a| a.get_webview_window(&label(source_id)))
        .is_some()
}

/// Refresh the login-probe endpoints from config. Called on load, on every
/// save and before auto-opening sessions.
pub fn sync_sources(cfg: &BridgeConfig) {
    let mut map = lock(endpoints());
    map.clear();
    for s in &cfg.sources {
        map.insert(s.id.clone(), s.login_endpoint.clone());
    }
}

pub fn source_ids() -> Vec<String> {
    lock(endpoints()).keys().cloned().collect()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Open (or focus) the vendor's site in a persistent WebView window.
/// `visible = false` opens it in the background — used when the endpoint
/// starts or a Web2API model is picked, so no window steals focus.
///
/// Tauri's Windows backend deadlocks when a window is built on a synchronous
/// command thread, so the build always happens on its own thread.
pub async fn open(source_id: &str, home_url: &str, visible: bool) -> Result<(), String> {
    let app = APP.get().cloned().ok_or("应用尚未就绪")?;
    let lbl = label(source_id);
    let url = if home_url.trim().is_empty() {
        match source_id {
            "claude" => "https://claude.ai/",
            "chatgpt" => "https://chatgpt.com/",
            "gemini" => "https://gemini.google.com/",
            _ => "",
        }
        .to_string()
    } else {
        home_url.trim().to_string()
    };
    if url.is_empty() {
        return Err("请先填写「官网地址」".into());
    }
    let (tx, rx) = oneshot::channel::<Result<(), String>>();
    std::thread::spawn(move || {
        let res = (|| -> Result<(), String> {
            if let Some(win) = app.get_webview_window(&lbl) {
                if visible {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
                return Ok(());
            }
            let parsed: url::Url = url.parse().map_err(|e| format!("网址无效：{e}"))?;
            WebviewWindowBuilder::new(&app, lbl.clone(), WebviewUrl::External(parsed))
                .title("IRIXEN Code · Web2API")
                .inner_size(1180.0, 820.0)
                .visible(visible)
                .build()
                .map_err(|e| format!("打开官网窗口失败：{e}"))?;
            Ok(())
        })();
        let _ = tx.send(res);
    });
    let out = rx.await.map_err(|_| "窗口创建中断".to_string())?;
    if out.is_ok() {
        spawn_probe(source_id.to_string());
    }
    out
}

/// Hide a session window without destroying it. The login state survives so
/// the next request can go straight through instead of asking the user to
/// click "打开官网" again.
pub fn hide(source_id: &str) -> bool {
    APP.get()
        .and_then(|a| a.get_webview_window(&label(source_id)))
        .map(|w| w.hide().is_ok())
        .unwrap_or(false)
}

/// Show a session window when a request fails. A hidden window leaves the user
/// guessing at a login wall or bot-guard challenge; surfacing it makes the
/// next step (log in / solve the challenge) obvious.
pub fn reveal(source_id: &str) {
    if let Some(win) = APP
        .get()
        .and_then(|a| a.get_webview_window(&label(source_id)))
    {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

pub fn logged_in(source_id: &str) -> bool {
    lock(states())
        .get(source_id)
        .map(|s| s.logged_in)
        .unwrap_or(false)
}

pub fn session_infos(cfg: &BridgeConfig) -> Vec<SessionInfo> {
    let st = lock(states());
    cfg.sources
        .iter()
        .map(|s| {
            let e = st.get(&s.id);
            SessionInfo {
                id: s.id.clone(),
                label: s.label.clone(),
                open: is_open(&s.id),
                logged_in: e.map(|x| x.logged_in).unwrap_or(false),
                account: e.map(|x| x.account.clone()).unwrap_or_default(),
                detail: e.map(|x| x.detail.clone()).unwrap_or_default(),
                checked_at: e.map(|x| x.checked_at).unwrap_or(0),
            }
        })
        .collect()
}

/* ------------------------------------------------------------ script engine */

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrowserRequest {
    pub url: String,
    pub body: String,
    pub headers: HashMap<String, String>,
    pub content_type: String,
    pub org: String,
    pub org_endpoint: String,
    pub org_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrowserReply {
    pub done: bool,
    pub status: u16,
    pub raw: String,
    pub error: String,
}

/// Wrap a JS body with the shared global-result plumbing. `body` is inserted
/// verbatim (not through a format string), so vendor scripts can contain as
/// many braces as they like without `{{`/`}}` escaping.
fn wrap_script(key_json: &str, body: &str) -> String {
    let head = r#"(function(){
  const K=__KEY__;
  const box=window.__irixenWeb2Api||(window.__irixenWeb2Api={});
  box[K]=null;
  const put=(o)=>{ box[K]=Object.assign({done:true,status:0,raw:'',error:''},o); };
"#;
    let head = head.replace("__KEY__", key_json);
    format!("{head}\n{body}\n}})();")
}

/// Inject `body` into the source's page and wait until it calls `put(...)`.
/// The page writes into a well-known global; WebView2 always resolves a
/// *synchronous* eval, so we poll that global instead of relying on
/// Promise-aware script results.
async fn inject_and_wait(
    source_id: &str,
    body: &str,
    timeout: Duration,
) -> Result<BrowserReply, String> {
    let app = APP.get().ok_or("应用尚未就绪")?;
    let win = app
        .get_webview_window(&label(source_id))
        .ok_or_else(|| format!("网页源「{source_id}」尚未打开官网窗口，请先点「打开官网」并登录"))?;

    let key = format!(
        "r{}-{}",
        crate::util::now_ms(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let key_json = serde_json::to_string(&key).map_err(|e| e.to_string())?;
    let script = wrap_script(&key_json, body);
    win.eval(script)
        .map_err(|e| format!("注入浏览器请求失败：{e}"))?;

    let probe = format!(
        "JSON.stringify((window.__irixenWeb2Api||{{}})[{key_json}]||null)"
    );
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        let (tx, rx) = oneshot::channel::<String>();
        let slot = Arc::new(Mutex::new(Some(tx)));
        win.eval_with_callback(probe.clone(), move |s| {
            if let Some(sender) = lock(&slot).take() {
                let _ = sender.send(s);
            }
        })
        .map_err(|e| format!("读取浏览器响应失败：{e}"))?;
        if let Ok(Ok(raw)) = tokio::time::timeout(Duration::from_secs(2), rx).await {
            if let Some(reply) = decode(&raw) {
                if reply.done {
                    return Ok(reply);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    Err(format!(
        "网页源「{source_id}」的浏览器会话在 {} 秒内没有响应",
        timeout.as_secs()
    ))
}

/// WebView2 returns the script's value as JSON — our script already produced a
/// JSON string, so the value may be double encoded.
fn decode(s: &str) -> Option<BrowserReply> {
    let v: Value = serde_json::from_str(s.trim()).ok()?;
    match v {
        Value::String(inner) => serde_json::from_str::<BrowserReply>(&inner).ok(),
        other => serde_json::from_value::<BrowserReply>(other).ok(),
    }
}

/* --------------------------------------------------------- legacy api driver */

/// Run one POST inside the logged-in page. Kept for sources whose private
/// endpoint still works (and for the panel's `api` driver choice).
pub async fn fetch(source_id: &str, cfg: BrowserRequest) -> Result<BrowserReply, String> {
    let cfg_json = serde_json::to_string(&cfg).map_err(|e| e.to_string())?;
    let body = HTTP_SCRIPT.replace("__CFG__", &cfg_json);
    inject_and_wait(source_id, &body, Duration::from_secs(60)).await
}

const HTTP_SCRIPT: &str = r#"(async()=>{
  const C=__CFG__;
  try {
    let org=C.org||'';
    if(!org&&C.orgEndpoint){
      const g=await fetch(C.orgEndpoint,{credentials:'include',headers:C.headers});
      if(g.ok){
        const j=await g.json();
        org=C.orgPath.split('.').filter(Boolean).reduce((x,k)=>x&&(Array.isArray(x)?x[+k]:x[k]),j)||'';
      }
    }
    const url=C.url.split('{org}').join(org);
    const body=C.body.split('{org}').join(org);
    const r=await fetch(url,{
      method:'POST', credentials:'include',
      headers:Object.assign({'content-type':C.contentType},C.headers), body:body
    });
    const t=await r.text();
    put({status:r.status,raw:t.slice(0,512000)});
  } catch(e) { put({error:String(e&&e.message||e)}); }
})();"#;

/* ------------------------------------------------------------- ui driver */

/// Built-in page driver. Types the prompt into the vendor's own composer,
/// clicks its send button and scrapes the rendered answer. No private API,
/// no sentinel token, no proof-of-work challenge — the page does all of that.
/// A source can override it with `ui_script`; `__PROMPT__` is substituted with
/// the JSON-encoded prompt and `K` / `box` / `put` are provided by the wrapper.
pub const DEFAULT_UI_SCRIPT: &str = r#"(async()=>{
  const P=__PROMPT__;
  const q=(s)=>document.querySelector(s);
  const qa=(s)=>Array.from(document.querySelectorAll(s));
  // Some vendors (Gemini especially) render messages inside shadow roots, so
  // `body *` alone can miss the composer's transcript. Walk shadow roots too.
  const deep=()=>{
    const out=[], roots=[document];
    while(roots.length){
      const r=roots.pop();
      for(const el of r.querySelectorAll('*')){
        out.push(el);
        if(el.shadowRoot) roots.push(el.shadowRoot);
        if(out.length>20000) return out;
      }
    }
    return out;
  };
  const SKIP_TAG=/^(SCRIPT|STYLE|NOSCRIPT|TEMPLATE|SVG|PATH|BUTTON|MAT-ICON|INPUT|TEXTAREA|SELECT|OPTION|HEAD|META|LINK|TITLE)$/;
  const inChrome=(el)=>{ try { return !!el.closest('button,nav,header,footer,aside,bard-sidenav,side-navigation,mat-sidenav,[role="button"],[role="navigation"],[role="banner"],[role="complementary"],[role="dialog"],[role="menu"],[role="tooltip"],[aria-modal="true"],.cdk-overlay-container,[class*="sidenav"],[class*="side-nav"],[class*="sidebar"]'); } catch(e){ return false; } };
  const visible=(el)=>{ try { if(!el.getClientRects().length) return false; const s=getComputedStyle(el); return s.display!=='none'&&s.visibility!=='hidden'&&s.opacity!=='0'; } catch(e){ return false; } };
  const ANS_SEL='[data-message-author-role="assistant"],[data-testid="assistant-message"],model-response,.model-response,message-content,.markdown,[data-is-streaming],[class*="assistant-message"],[class*="response-container"],.font-claude-message';
  const sleep=(ms)=>new Promise(r=>setTimeout(r,ms));
  const norm=(s)=>String(s||'').replace(/\s+/g,' ').trim();
  const PF=norm(P);
  const flat=()=>(document.body&&document.body.innerText||'').replace(/\s+/g,' ').trim();
  // Every vendor prefixes a user turn with some "you said" label. Counting
  // them is a DOM-independent way to know a NEW turn really started — the
  // element scan can miss bubbles that live in a shadow root.
  const countTurns=()=>{
    const p=flat();
    return (p.match(/你说\s*[:：]?/g)||[]).length+(p.match(/You said/gi)||[]).length+(p.match(/あなた\s*[:：]/g)||[]).length;
  };
  const markOf=()=>{
    const page=flat();
    const marks=['你说：','你说:','你说 ','You said:','You said ','あなた：'];
    let idx=-1,len=0;
    for(const m of marks){ const i=page.lastIndexOf(m); if(i>idx){ idx=i; len=m.length; } }
    return {page,idx,len};
  };
  // True only when the LAST user turn on the page is this request. A failed
  // send leaves the previous turn last, so its answer can never be returned as
  // if it were the reply to this prompt.
  const ourTurn=()=>{
    const m=markOf();
    if(m.idx<0) return false;
    const a=m.page.slice(m.idx+m.len).trim().replace(/^[:：\s]+/,'');
    return !!PF && a.startsWith(PF);
  };
  const diag=()=>{
    let sendish='';
    try{
      sendish=qa('button,[role="button"],gem-icon-button').filter(b=>{
        const lab=[b.getAttribute('aria-label'),b.title,b.getAttribute('data-testid'),b.className].join(' ');
        return /send|发送|送出|傳送|訊息/i.test(lab);
      }).slice(-4).map(b=>((b.getAttribute('aria-label')||String(b.className||'')).slice(0,24))+(b.disabled?'(禁)':'')).join('/');
    }catch(e){}
    return ' [页面片段:'+flat().slice(-180)+' | 输入:'+(q('.ql-editor[contenteditable="true"]')?'有':'无')+' | 镜像:'+(q('rich-textarea textarea')?'有':'无')+' | 发送:'+(sendish||'未找到')+' | 按钮:'+qa('button').slice(-14).map(b=>((b.getAttribute('aria-label')||b.title||'').trim())).filter(Boolean).join('/').slice(0,200)+']';
  };
  // Vendor chrome. A static greeting, the model selector, the disclaimer and
  // the "已回复" toast must never be returned as the model's answer.
  // `CUT` trims an answer that was concatenated with chrome; `CHROME` rejects
  // candidates that are chrome-only (e.g. the new-chat landing screen).
  const CUT=/Gemini\s*自动|Gemini\s*不是真人|ChatGPT\s*可能会出错|ChatGPT can make mistakes|这段对话对你有帮助吗|你喜欢这种回复风格吗|显示更多|最新一条回复|你说\s*[:：]/i;
  const CHROME=/Gemini\s*不是真人|回答未必正确|请注意核查|ChatGPT\s*可能会出错|ChatGPT can make mistakes|这段对话对你有帮助吗|你喜欢这种回复风格吗|Gemini\s*自动|人机验证|unusual activity|verify you are human/i;
  const clean=(s)=>{
    let t=norm(s);
    const m=t.search(CUT); if(m>0) t=t.slice(0,m);
    // Screen-reader speaker labels ("Gemini 说", "ChatGPT 说：") are not content.
    t=t.replace(/^(?:(?:ChatGPT|Claude|Gemini|Bard|模型|Assistant)\s*(?:说|回答)?\s*[:：]?\s*)+/i,'');
    // A partial anchor used to leave the greeting's "！" glued to the front.
    t=t.replace(/^[\s!！?？,，.。:：;；、…·]+/,'');
    t=t.replace(/(?:\s*(?:Gemini|Claude|ChatGPT)\s*已回复|\s*已复制|\s*回答已完成|\s*重新生成|\s*升级)+\s*$/g,'');
    return t.trim();
  };
  const NOISE=/^(?:思考|回答已完成|已复制|重新生成|正在思考|生成中|Loading|自动|升级|设置|更多|复制|重做|分享|登录|停止|发送|新对话|Gemini|ChatGPT|Claude|[\s.·…]+)$/;
  const ok=(s)=>s && s.length>1 && s!==PF && !CHROME.test(s) && !NOISE.test(s);
  try {
    const composer=q('.ql-editor[contenteditable="true"]')||q('#prompt-textarea')||q('div[contenteditable="true"]')||q('textarea')||q('[role="textbox"]');
    if(!composer){ put({status:0,error:'未找到对话输入框；请在弹出的官网窗口登录并停留在对话页'+diag()}); return; }
    // React / ProseMirror / Quill ignore `textContent` followed by a synthetic
    // `Event('input')`, so the vendor's send button never enables. A real
    // editing command fires trusted input events the framework can observe.
    composer.focus();
    if(composer.tagName==='TEXTAREA'){
      const setter=Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype,'value').set;
      setter.call(composer,P);
      composer.dispatchEvent(new Event('input',{bubbles:true}));
    } else if(document.execCommand){
      // Replace the WHOLE composer. If a previous send failed, its prompt is
      // still sitting there; appending makes prompts pile up into one long
      // blob that the page never submits, so no new answer ever appears.
      const range=document.createRange();
      range.selectNodeContents(composer);
      const selection=window.getSelection();
      if(selection){ selection.removeAllRanges(); selection.addRange(range); }
      if(!document.execCommand('insertText',false,P)){
        composer.textContent=P;
        composer.dispatchEvent(new InputEvent('input',{bubbles:true,inputType:'insertText',data:P}));
      }
    } else {
      composer.textContent=P;
      composer.dispatchEvent(new InputEvent('input',{bubbles:true,inputType:'insertText',data:P}));
    }
    composer.dispatchEvent(new Event('change',{bubbles:true}));
    // Gemini's `rich-textarea` keeps a hidden <textarea> as the Angular form
    // control. Filling only the Quill editor leaves that control empty, so no
    // send button is ever rendered (the page keeps showing the landing
    // greeting). Mirror the text into the control and notify Angular.
    const mirror=q('rich-textarea textarea')||q('textarea');
    if(mirror&&mirror!==composer){
      try {
        const setter=Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype,'value').set;
        setter.call(mirror,P);
        mirror.dispatchEvent(new Event('input',{bubbles:true}));
        mirror.dispatchEvent(new Event('change',{bubbles:true}));
      } catch(e) {}
    }
    // Baseline BEFORE sending: element identity + text. Only nodes that appear
    // or change after this point can belong to this turn's answer.
    const base=new Map(), baseBubbles=new Set();
    for(const el of deep()){
      if(el===composer||composer.contains(el)) continue;
      const raw=el.textContent||'';
      if(raw) base.set(el,raw);
      const t=norm(raw);
      if(t && (t===PF||t.endsWith(PF)) && t.length<=PF.length+40) baseBubbles.add(el);
    }
    const baseTurns=countTurns();
    const sendWords=/send|submit|发送|送出|传送|傳送|提交|訊息|询问|提问/i;
    const findSend=()=>{
      const named=q('[data-testid="send-button"]')||q('button[aria-label*="Send"]')||q('button[aria-label*="发送"]')||q('button[aria-label*="送出"]')||q('button[aria-label*="傳送"]')||q('.send-button')||q('button[data-testid*="send"]');
      if(named&&!named.disabled) return named;
      // Gemini / Claude do not expose a stable send test id. Any visible
      // control whose label or icon says "send" and that sits next to the
      // composer is the send button — never the "upgrade" or "mic" one.
      const cr=composer.getBoundingClientRect();
      const dist=(b)=>{ const r=b.getBoundingClientRect(); return Math.abs(r.top-cr.top)+Math.abs(r.left-cr.left); };
      const list=qa('button,[role="button"],gem-icon-button').filter(b=>{
        if(b.disabled) return false;
        const r=b.getBoundingClientRect(); if(r.width<=0||r.height<=0) return false;
        const label=[b.getAttribute('aria-label'),b.getAttribute('title'),b.getAttribute('data-tooltip'),b.getAttribute('data-testid'),b.className,(b.textContent||'').slice(0,30)].join(' ');
        const ic=b.querySelector('mat-icon,gem-icon,svg');
        const icon=norm(ic&&((ic.textContent||'')+' '+(ic.getAttribute('data-icon')||'')));
        return sendWords.test(label+' '+icon)||/^(?:send|arrow_upward|send_rounded)$/i.test(icon);
      });
      list.sort((a,b)=>dist(a)-dist(b));
      return list[0]||null;
    };
    let send=null;
    for(let i=0;i<40;i++){
      send=findSend();
      if(send&&send.isConnected&&!send.disabled) break;
      await sleep(150);
    }
    let sent=false;
    if(send&&send.isConnected&&!send.disabled){ send.click(); sent=true; }
    else {
      // A synthetic Enter is not a trusted event; kept only as a last resort.
      for(const el of [composer,mirror]){
        if(el) el.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',code:'Enter',keyCode:13,which:13,bubbles:true,cancelable:true}));
      }
    }

    const busy=()=>!!(q('[data-testid="stop-button"]')||q('button[aria-label*="Stop"]')||q('button[aria-label*="停止"]')||q('.stop-button')||q('[data-is-streaming="true"]'));
    const isAfter=(el,a)=>{ try { return !!(a.compareDocumentPosition(el)&4); } catch(e){ return false; } };
    // The REAL user bubble. A short prompt such as "你好" also occurs inside
    // Gemini's landing greeting ("你好！今天过得怎么样？…"). The old
    // `body.innerText.lastIndexOf(prompt)` anchor matched that static text and
    // returned "！今天…升级" — the greeting tail — as if it were the answer.
    const bubbleOf=()=>{
      let best=null,bestScore=1e9;
      const USER_HINT='user-query,[data-message-author-role="user"],[data-testid="user-message"],.font-user-message,[class*="user-message"],[class*="query-text"],[class*="user-query"]';
      for(const el of deep()){
        if(el===composer||composer.contains(el)) continue;
        if(!el.getClientRects().length) continue;
        if(inChrome(el)) continue;
        // An answer that happens to equal the prompt (ask "你好", get "你好")
        // must not be mistaken for the user's bubble: it is what we look after.
        if(el.closest('model-response,[data-message-author-role="assistant"],[data-testid="assistant-message"],.font-claude-message')) continue;
        const raw=el.textContent||'', t=norm(raw);
        if(!t||t.length>PF.length+40||!(t===PF||t.endsWith(PF))||CHROME.test(t)) continue;
        if(baseBubbles.has(el)&&base.get(el)===raw) continue;
        // A real user bubble carries a user-query role/class; prefer it over a
        // same-text node elsewhere, and keep the LAST one on a tie.
        const userScore=(el.matches(USER_HINT)||el.closest(USER_HINT))?0:60;
        const score=userScore+el.children.length;
        if(score<=bestScore){ bestScore=score; best=el; }
      }
      return best;
    };
    const answerOf=(anchor)=>{
      const pick=(el)=>{
        let t=norm(el.innerText||el.textContent||'');
        if(PF.length>=6){ const i=t.lastIndexOf(PF); if(i>=0) t=t.slice(i+PF.length); }
        return clean(t);
      };
      // Primary: the vendor's own assistant message node. Scoping to real
      // message containers keeps menus, tooltips and settings panels out.
      let best='';
      for(const el of deep()){
        if(el===composer||composer.contains(el)||anchor.contains(el)) continue;
        if(!el.matches||!el.matches(ANS_SEL)||!isAfter(el,anchor)) continue;
        const raw=el.textContent||'';
        if(!raw||raw.length>60000||base.get(el)===raw||!visible(el)) continue;
        const t=pick(el);
        if(ok(t)) best=t;
      }
      if(best) return best;
      // Fallback: the largest changed VISIBLE block after the bubble. Hidden
      // dialogs are skipped; otherwise an open settings panel ("语音输入 …
      // 切换模型 …") wins simply for being the longest text on the page.
      const pool=[];
      for(const el of deep()){
        if(el===composer||composer.contains(el)||el===anchor) continue;
        if(anchor.contains(el)||!isAfter(el,anchor)) continue;
        if(SKIP_TAG.test(el.tagName)||inChrome(el)) continue;
        const raw=el.textContent||'';
        if(!raw||raw.length>60000||base.get(el)===raw) continue;
        pool.push([el,raw.length]);
      }
      pool.sort((a,b)=>b[1]-a[1]);
      for(const [el] of pool.slice(0,40)){
        if(!visible(el)) continue;
        const t=pick(el);
        if(t.length>=8&&ok(t)&&t.length>best.length) best=t;
      }
      return best;
    };
    // Shadow-DOM-proof fallback: slice the rendered text after the LAST
    // "你说 / You said" label. Chrome is trimmed, the prompt itself is removed
    // once (the label and the visible bubble duplicate it), and the leading
    // punctuation that produced "！今天…" can never survive.
    const textAnswer=()=>{
      const m=markOf();
      if(m.idx<0) return '';
      let a=m.page.slice(m.idx+m.len).trim().replace(/^[:：\s]+/,'');
      if(!PF||!a.startsWith(PF)) return '';
      a=a.slice(PF.length);
      for(let k=0;k<2;k++){
        a=a.replace(/^[\s:：]+/,'');
        if(PF&&a.startsWith(PF)) a=a.slice(PF.length);
      }
      a=clean(a);
      return ok(a)?a:'';
    };
    let last='',stable=0,everText=false,started=Date.now(),bubble=null;
    while(Date.now()-started<100000){
      await sleep(350);
      const page=flat();
      if(/unusual activity|unusual traffic|verify you are human|try again later|人机验证|异常活动/i.test(page)){
        put({status:0,error:'官网触发风控验证；请在弹出的官网窗口完成人机验证后重试'+diag()}); return;
      }
      if(bubble&&!bubble.isConnected) bubble=null;
      if(!bubble) bubble=bubbleOf();
      const fresh=countTurns()>baseTurns&&ourTurn();
      let cur=fresh?textAnswer():'';
      if(!cur&&fresh&&bubble) cur=answerOf(bubble);
      if(cur) everText=true;
      if(cur&&cur===last) stable++; else stable=0;
      last=cur;
      box[K]={done:false,status:200,raw:cur,error:''};
      if(cur&&!busy()&&stable>=5){ put({status:200,raw:cur}); return; }
      // Neither a user bubble nor a new "你说" turn means the message never
      // went out. The old code still returned landing-page text here.
      if(!bubble&&!fresh&&Date.now()-started>12000){
        const why=sent?'已点击发送按钮，但页面没有出现本轮用户消息':'发送按钮未激活，消息可能没有发出';
        put({status:0,error:'消息没有发出（'+why+'；可能停在登录/验证页）。请在弹出的官网窗口确认后重试'+diag()}); return;
      }
      if((bubble||fresh)&&!everText&&Date.now()-started>25000){
        put({status:0,error:'消息已发出，但 25 秒内没有抓到官网回答；请确认官网窗口已登录且未停在验证页'+diag()}); return;
      }
    }
    put({status:200,raw:last,error:last?'':'页面在 100 秒内未返回完整回答；已弹出官网窗口，请确认登录状态后重试'+diag()});
  } catch(e){ put({error:String(e&&e.message||e)}); }
})();"#;

/// Clean the scraped page text at the IPC boundary.
///
/// The page's `innerText` routinely carries vendor chrome around the real
/// answer (model selector, disclaimer, "已回复" toast). A short prompt could
/// also leave a single punctuation mark behind when an anchor matched static
/// page text. Every UI driver — including a custom `ui_script` — passes
/// through here, so the chat never renders "！今天有什么我可以帮你的吗…" as
/// if it were the model's answer.
fn sanitize_answer(raw: &str) -> String {
    let mut text = raw.trim().to_string();
    for marker in [
        "Gemini 自动",
        "Gemini 不是真人",
        "ChatGPT 可能会出错",
        "ChatGPT can make mistakes",
        "这段对话对你有帮助吗",
        "你喜欢这种回复风格吗",
        "显示更多",
        "Gemini 已回复",
    ] {
        if let Some(i) = text.find(marker) {
            if i > 0 {
                text.truncate(i);
            }
        }
    }
    // Screen-reader speaker labels are not part of the content.
    for label in ["ChatGPT", "Claude", "Gemini", "Bard", "模型", "Assistant"] {
        for sep in [" 说：", " 说:", " 说 ", "说：", "说:", "说 "] {
            let prefix = format!("{label}{sep}");
            while text.starts_with(&prefix) {
                text = text[prefix.len()..].trim_start().to_string();
            }
        }
    }
    for tail in [" 升级", " 已复制", " 回答已完成", " 重新生成", " Gemini 已回复"] {
        while text.ends_with(tail) {
            text.truncate(text.len() - tail.len());
        }
    }
    text.trim_start_matches(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '！' | '!' | '？' | '?' | '，' | ',' | '。' | '.' | '；' | ';' | '：' | ':' | '、' | '…' | '·'
            )
    })
    .trim()
    .to_string()
}

/// Drive the page's UI and return the rendered answer as plain text.
pub async fn run_ui(source_id: &str, prompt: &str, custom_script: &str) -> Result<String, String> {
    let prompt_json = serde_json::to_string(prompt).map_err(|e| e.to_string())?;
    let tpl = if custom_script.trim().is_empty() {
        DEFAULT_UI_SCRIPT
    } else {
        custom_script
    };
    let body = tpl
        .replace("__PROMPT__", &prompt_json)
        .replace("{prompt_json}", &prompt_json);
    // Must stay below the provider client's 150 s read timeout: if the page
    // never answers, we want a structured 502 explaining *why*, not a
    // client-side read timeout that surfaces as a vague "网络错误".
    let reply = match inject_and_wait(source_id, &body, Duration::from_secs(115)).await {
        Ok(r) => r,
        Err(e) => {
            reveal(source_id);
            return Err(e);
        }
    };
    if !reply.error.is_empty() {
        reveal(source_id);
        return Err(reply.error);
    }
    let text = sanitize_answer(&reply.raw);
    if text.is_empty() {
        reveal(source_id);
        return Err("页面没有返回回答（可能未登录，或官网触发了风控）；已弹出官网窗口，请确认登录状态后重试".into());
    }
    Ok(text)
}

/* ----------------------------------------------------------- login probe */

#[derive(Deserialize)]
struct LoginProbe {
    logged_in: bool,
    account: String,
    detail: String,
}

/// Ask the page whether its cookie jar is logged in and cache the answer.
/// Uses a same-origin identity endpoint when configured, plus a DOM/cookie
/// fallback for vendors without one. Never throws: an unready page simply
/// reports `logged_in = false` so the watcher retries.
pub async fn probe_login(source_id: &str) -> Result<SessionInfo, String> {
    let endpoint = lock(endpoints())
        .get(source_id)
        .cloned()
        .unwrap_or_default();
    probe_login_with(source_id, &endpoint).await
}

/// Same probe with an explicit endpoint — used by "测试连接", which operates
/// on the panel's unsaved draft rather than the live config.
pub async fn probe_login_with(source_id: &str, endpoint: &str) -> Result<SessionInfo, String> {
    let endpoint_json = serde_json::to_string(endpoint).map_err(|e| e.to_string())?;
    let body = LOGIN_SCRIPT.replace("__ENDPOINT__", &endpoint_json);
    let info = match inject_and_wait(source_id, &body, Duration::from_secs(14)).await {
        Ok(reply) => {
            let parsed: Option<LoginProbe> = serde_json::from_str(&reply.raw).ok();
            SessionState {
                logged_in: parsed.as_ref().map(|p| p.logged_in).unwrap_or(false),
                account: parsed.as_ref().map(|p| p.account.clone()).unwrap_or_default(),
                detail: parsed
                    .as_ref()
                    .map(|p| p.detail.clone())
                    .unwrap_or_else(|| reply.error.clone()),
                checked_at: crate::util::now_ms(),
            }
        }
        Err(e) => SessionState {
            logged_in: false,
            account: String::new(),
            detail: e,
            checked_at: crate::util::now_ms(),
        },
    };
    let mut st = lock(states());
    st.insert(source_id.to_string(), info.clone());
    Ok(SessionInfo {
        id: source_id.to_string(),
        label: String::new(),
        open: is_open(source_id),
        logged_in: info.logged_in,
        account: info.account,
        detail: info.detail,
        checked_at: info.checked_at,
    })
}

const LOGIN_SCRIPT: &str = r#"(async()=>{
  const endpoint=__ENDPOINT__;
  const finish=(logged_in,account,detail)=>put({status:200,raw:JSON.stringify({logged_in:logged_in,account:account,detail:detail})});
  try {
    let guarded=false;
    if(endpoint){
      try {
        const r=await fetch(endpoint,{credentials:'include',headers:{'accept':'application/json'}});
        const t=(await r.text()).slice(0,4000);
        let account='';
        try { const j=JSON.parse(t); account=String((j.user&&(j.user.email||j.user.name))||j.email||j.name||''); } catch(e) {}
        const empty=!t||t==='{}'||t==='[]'||t==='null'||/^<!doctype|^<html/i.test(t);
        if(r.ok&&!empty){ finish(true,account,'会话接口 '+r.status+(account?' · '+account:'')); return; }
        // 401/403 from a bot guard is NOT proof of being logged out. The
        // identity endpoint can be challenged while the page itself is fully
        // signed in, so fall through to the DOM check below instead.
        guarded=!r.ok;
      } catch(e){ guarded=true; }
    }
    const c=document.cookie||'';
    const pwd=!!document.querySelector('input[type="password"]');
    const pageText=(document.body&&document.body.innerText||'').slice(0,1500);
    const wall=/log in|sign in|sign up|登录|登入|ログイン/i.test(pageText);
    // A bot-guard / challenge page is NOT a usable session. It often still
    // contains a textarea, which used to make the DOM heuristic report
    // "logged in" and publish models that can never answer a chat request.
    const guard=/unusual activity|unusual traffic|verify you are human|attention required|cloudflare|captcha|try again later|人机验证|异常活动|access denied/i.test(pageText);
    if(guard){ finish(false,'','官网风控验证页；请在窗口内完成人机验证后重试'); return; }
    const composer=!!document.querySelector('#prompt-textarea, textarea, [contenteditable="true"], [role="textbox"]');
    const cookie=/(^|;\s*)(__Secure-next-auth\.session-token|sessionKey|session|__Secure-1PSID|SID|auth_token|_auth)/i.test(c);
    const logged=composer&&!pwd&&(cookie||!wall);
    finish(logged,'',logged?(guarded?'页面已就绪（会话接口被风控，改用页面判定）':'页面已就绪'):(wall?'页面停在登录页':'页面尚未就绪'));
  } catch(e){ finish(false,'',String(e&&e.message||e)); }
})();"#;

/// Retry the login probe while a freshly opened page finishes loading.
fn spawn_probe(source_id: String) {
    tauri::async_runtime::spawn(async move {
        for _ in 0..12 {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            if !is_open(&source_id) {
                return;
            }
            if let Ok(info) = probe_login(&source_id).await {
                if info.logged_in {
                    return;
                }
            }
        }
    });
}

/// Periodically refresh login state for every open session. This is what
/// keeps `/v1/models` honest: a source that logs out (or never logged in)
/// stops advertising its models without any manual refresh.
pub fn start_watcher() {
    if WATCHER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        loop {
            let mut any_open = false;
            for id in source_ids() {
                if is_open(&id) {
                    any_open = true;
                    let _ = probe_login(&id).await;
                } else {
                    let mut st = lock(states());
                    if let Some(s) = st.get_mut(&id) {
                        s.logged_in = false;
                    }
                }
            }
            // Nothing is open (the common case with auto-start off): there is
            // no login state to refresh, so back off hard instead of waking a
            // renderer thread every few seconds forever.
            tokio::time::sleep(Duration::from_secs(if any_open { 15 } else { 60 })).await;
        }
    });
}

/// Open the sessions the user allowed, in the background. `Some(model)` opens
/// just the source that model routes to; `None` opens every `auto_open`
/// source (used when the endpoint starts). Returns the ids that were opened.
pub async fn ensure_sessions(cfg: &BridgeConfig, model: Option<&str>) -> Vec<String> {
    sync_sources(cfg);
    let wanted: Vec<&WebSource> = if let Some(m) = model {
        let lower = m.to_lowercase();
        let id = cfg
            .routes
            .iter()
            .find(|r| {
                !r.prefix.trim().is_empty()
                    && lower.starts_with(&r.prefix.trim().to_lowercase())
            })
            .map(|r| r.source_id.clone())
            .unwrap_or_else(|| cfg.default_source.clone());
        cfg.sources.iter().filter(|s| s.id == id).collect()
    } else {
        cfg.sources.iter().filter(|s| s.auto_open).collect()
    };

    let mut opened = Vec::new();
    for s in wanted {
        if s.mode.eq_ignore_ascii_case("http") {
            continue;
        }
        if !s.auto_open && model.is_none() {
            continue;
        }
        if open(&s.id, &s.home_url, false).await.is_ok() {
            opened.push(s.id.clone());
        }
    }
    opened
}
