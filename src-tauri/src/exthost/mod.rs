//! Node extension host — runs real VS Code extensions in a `node` child process.
//!
//! The host is deliberately boring: one process per extension, NDJSON over
//! stdin/stdout, and a JS bootstrap (`bootstrap.js`) that installs a `vscode`
//! module backed by RPC into this core. Everything the extension can observe —
//! messages, quick picks, the workspace filesystem, the open document, status
//! bar items, `globalState` — is answered here, which is why the extension sees
//! *this* IDE's state instead of a fake one.
//!
//! What is intentionally absent: the language-feature providers (completion,
//! hover, diagnostics) are registered by the shim so extensions do not crash,
//! but nothing drives them yet. `plan` is honest about that instead of silently
//! returning empty results from a half-built pipeline.

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::config::AppConfig;

/// Written into `<config>/runtime/exthost/bootstrap.js` on first start.
const BOOTSTRAP: &str = include_str!("bootstrap.js");
/// Cap for a single host→core frame (a whole-file document event is the big one).
const MAX_FRAME: usize = 8 * 1024 * 1024;
const LOG_CAP: usize = 2000;

// ---------------------------------------------------------------------------
// public model
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct HostStatus {
    pub id: String,
    /// `stopped` | `starting` | `running` | `crashed`
    pub state: String,
    pub pid: u32,
    pub node: String,
    pub error: String,
    pub started_at: u64,
    pub activated: bool,
    pub commands: Vec<String>,
    pub entry: String,
}

#[derive(Serialize, Clone)]
pub struct RuntimeCommand {
    pub id: String,
    pub title: String,
    pub category: String,
    pub ext: String,
    pub ext_name: String,
    /// True once the extension registered it at runtime; false means it is only
    /// declared in `contributes.commands` and will fail with a clear message.
    pub live: bool,
}

#[derive(Serialize, Clone)]
pub struct HostLog {
    pub id: String,
    pub ts: u64,
    pub level: String,
    pub text: String,
}

#[derive(Serialize, Clone)]
pub struct StatusItem {
    pub ext: String,
    pub id: String,
    pub text: String,
    pub tooltip: String,
    pub command: String,
    pub visible: bool,
    pub alignment: i32,
    pub priority: i32,
}

#[derive(Serialize, Clone, Default)]
pub struct NodeInfo {
    pub found: bool,
    pub path: String,
    pub version: String,
    pub bundled: bool,
}

/// A language feature provider registered by an extension.
///
/// Only the routing metadata lives here — the provider function itself stays in
/// its host process, which is what makes completions/formatting run in the
/// extension's own runtime instead of a re-implementation.
#[derive(Serialize, Clone)]
pub struct ProviderRow {
    pub ext: String,
    pub id: String,
    pub kind: String,
    pub selector: Value,
    pub trigger_characters: Vec<String>,
}

// ---------------------------------------------------------------------------
// node runtime
// ---------------------------------------------------------------------------

pub fn runtime_dir() -> PathBuf {
    crate::config::base_dir().join("runtime").join("node")
}

fn bundled_node() -> Option<PathBuf> {
    let p = if cfg!(windows) {
        runtime_dir().join("node.exe")
    } else {
        runtime_dir().join("bin").join("node")
    };
    p.is_file().then_some(p)
}

fn version_of(exe: &Path) -> Option<String> {
    let mut cmd = Command::new(exe);
    cmd.arg("-v");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    v.starts_with('v').then_some(v)
}

/// Resolve a node binary: explicit setting → bundled copy → PATH → vendor dirs.
pub fn detect_node(configured: &str) -> NodeInfo {
    let mut cands: Vec<(PathBuf, bool)> = Vec::new();
    if !configured.trim().is_empty() {
        cands.push((PathBuf::from(configured.trim()), false));
    }
    if let Some(p) = bundled_node() {
        cands.push((p, true));
    }
    cands.push((PathBuf::from("node"), false));
    #[cfg(windows)]
    {
        for env in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Ok(base) = std::env::var(env) {
                cands.push((PathBuf::from(base).join("nodejs").join("node.exe"), false));
            }
        }
    }
    #[cfg(not(windows))]
    {
        for p in ["/usr/local/bin/node", "/opt/homebrew/bin/node", "/usr/bin/node"] {
            cands.push((PathBuf::from(p), false));
        }
    }

    for (path, bundled) in cands {
        if let Some(version) = version_of(&path) {
            return NodeInfo {
                found: true,
                path: path.to_string_lossy().to_string(),
                version,
                bundled,
            };
        }
    }
    NodeInfo::default()
}

/// Download a portable Node build and keep only `node.exe`.
///
/// The zip is ~30 MB and the binary inside is ~80 MB, which is why this is a
/// deliberate, user-triggered action rather than something that happens on
/// first launch. We reuse the ZIP reader from the extension module, so no new
/// dependency and the same path-safety checks apply.
pub async fn install_node(proxy: &str) -> Result<NodeInfo, String> {
    if !cfg!(windows) {
        return Err("自动安装目前只支持 Windows；请用系统包管理器安装 Node.js".into());
    }
    let client = crate::net::build_client(proxy);
    let index_url = "https://nodejs.org/dist/index.json";
    let resp = client
        .get(index_url)
        .send()
        .await
        .map_err(|e| crate::net::explain_transport(&e, index_url, &crate::net::proxy_label(proxy)))?;
    if !resp.status().is_success() {
        return Err(format!("查询 Node 版本失败（HTTP {}）", resp.status().as_u16()));
    }
    let rows: Value = resp.json().await.map_err(|e| format!("解析版本列表失败：{e}"))?;
    let pick = rows
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .find(|r| r.get("lts").map(|v| !v.is_null() && v != &Value::Bool(false)).unwrap_or(false))
                .or_else(|| arr.first())
        })
        .ok_or_else(|| "nodejs.org 没有返回任何版本".to_string())?;
    let ver = pick.get("version").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if ver.is_empty() {
        return Err("版本列表格式异常".into());
    }
    let url = format!("https://nodejs.org/dist/{ver}/node-{ver}-win-x64.zip");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| crate::net::explain_transport(&e, &url, &crate::net::proxy_label(proxy)))?;
    if !resp.status().is_success() {
        return Err(format!("下载 Node 失败（HTTP {}）", resp.status().as_u16()));
    }
    let bytes = resp.bytes().await.map_err(|e| format!("下载中断：{e}"))?;

    let entries = crate::extensions::zip::list(&bytes)?;
    let inner = format!("node-{ver}-win-x64/node.exe");
    let entry = entries
        .iter()
        .find(|e| e.name == inner)
        .or_else(|| entries.iter().find(|e| e.name.ends_with("/node.exe")))
        .ok_or_else(|| "压缩包里没有 node.exe".to_string())?;
    let data = crate::extensions::zip::read(&bytes, entry)?;

    let dir = runtime_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建运行目录失败：{e}"))?;
    let target = dir.join("node.exe");
    std::fs::write(&target, &data).map_err(|e| format!("写入 node.exe 失败：{e}"))?;

    let mut info = detect_node("");
    if !info.found {
        return Err("node.exe 已写入但无法执行".into());
    }
    info.bundled = true;
    Ok(info)
}

// ---------------------------------------------------------------------------
// host
// ---------------------------------------------------------------------------

struct Host {
    tx: Sender<String>,
    child: Arc<Mutex<Child>>,
    status: Arc<Mutex<HostStatus>>,
}

/// A `window.*` prompt the renderer has to answer.
struct UiWaiter {
    /// Owning extension, so a crashed host's pending prompts can be dropped.
    ext: String,
    /// The host's own request id — the reply frame must carry this number.
    req: u64,
    tx: Sender<String>,
}

struct Inner {
    app: AppHandle,
    config: Arc<Mutex<AppConfig>>,
    hosts: Mutex<HashMap<String, Host>>,
    logs: Mutex<VecDeque<HostLog>>,
    commands: Mutex<HashMap<String, RuntimeCommand>>,
    statusbar: Mutex<Vec<StatusItem>>,
    ui_seq: AtomicU64,
    /// UI prompts waiting for the renderer, keyed by the id sent to the renderer.
    ///
    /// The key and the id in the reply frame are *not* the same number: the
    /// renderer needs an id that is unique across extensions (`ui_seq`), while
    /// the waiting host only accepts the reply for its own request (`req`).
    /// Using one number for both silently drops every prompt as soon as an
    /// extension issues two requests with different prompt counts.
    ui_waiters: Mutex<HashMap<u64, UiWaiter>>,
    /// Reverse RPC (core → host) replies.
    rev_seq: AtomicU64,
    rev_waiters: Mutex<HashMap<u64, Sender<Result<Value, String>>>>,
    docs: Mutex<HashMap<String, Value>>,
    /// Language providers, keyed by (ext, provider id). Registry order matters:
    /// the first registered provider wins, like VS Code's provider priority.
    providers: Mutex<Vec<ProviderRow>>,
    /// Webview panels created by extensions, so the UI can list/close them.
    webviews: Mutex<HashMap<String, Value>>,
    /// Tree data providers, keyed by the contributed view id. The pid is the
    /// handle the Node process needs to find the provider again.
    trees: Mutex<HashMap<String, TreeRow>>,
}

#[derive(Clone)]
struct TreeRow {
    ext: String,
    pid: String,
}

pub struct ExtHost {
    inner: Arc<Inner>,
}

impl ExtHost {
    pub fn new(app: AppHandle, config: Arc<Mutex<AppConfig>>) -> Self {
        ExtHost {
            inner: Arc::new(Inner {
                app,
                config,
                hosts: Mutex::new(HashMap::new()),
                logs: Mutex::new(VecDeque::new()),
                commands: Mutex::new(HashMap::new()),
                statusbar: Mutex::new(Vec::new()),
                ui_seq: AtomicU64::new(1),
                ui_waiters: Mutex::new(HashMap::new()),
                rev_seq: AtomicU64::new(1),
                rev_waiters: Mutex::new(HashMap::new()),
                docs: Mutex::new(HashMap::new()),
                providers: Mutex::new(Vec::new()),
                webviews: Mutex::new(HashMap::new()),
                trees: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn config_snapshot(&self) -> Arc<Mutex<AppConfig>> {
        self.inner.config.clone()
    }

    pub fn node(&self) -> NodeInfo {
        let configured = crate::util::lock(&self.inner.config).extensions.node_path.clone();
        detect_node(&configured)
    }

    pub fn status(&self) -> Vec<HostStatus> {
        let hosts = self.inner.hosts.lock().unwrap_or_else(|e| e.into_inner());
        let mut rows: Vec<HostStatus> = hosts
            .values()
            .map(|h| h.status.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .collect();
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        rows
    }

    pub fn status_of(&self, id: &str) -> Option<HostStatus> {
        self.status().into_iter().find(|s| s.id == id)
    }

    pub fn running(&self, id: &str) -> bool {
        self.status_of(id).map(|s| s.state == "running" || s.state == "starting").unwrap_or(false)
    }

    pub fn commands(&self) -> Vec<RuntimeCommand> {
        let mut rows: Vec<RuntimeCommand> = self
            .inner
            .commands
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        rows.sort_by(|a, b| (a.ext_name.to_lowercase(), a.title.to_lowercase()).cmp(&(b.ext_name.to_lowercase(), b.title.to_lowercase())));
        rows
    }

    pub fn statusbar(&self) -> Vec<StatusItem> {
        self.inner
            .statusbar
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|i| i.visible && !i.text.trim().is_empty())
            .cloned()
            .collect()
    }

    pub fn logs(&self, id: Option<&str>) -> Vec<HostLog> {
        let logs = self.inner.logs.lock().unwrap_or_else(|e| e.into_inner());
        logs.iter()
            .filter(|l| id.map(|want| want == l.id).unwrap_or(true))
            .cloned()
            .collect()
    }

    pub fn clear_logs(&self, id: &str) {
        let mut logs = self.inner.logs.lock().unwrap_or_else(|e| e.into_inner());
        logs.retain(|l| l.id != id);
    }

    /// Spawn the host for one extension. Blocking: call it from `spawn_blocking`.
    pub fn start(&self, info: &crate::extensions::ExtensionInfo) -> Result<HostStatus, String> {
        if self.running(&info.id) {
            return Ok(self.status_of(&info.id).unwrap());
        }
        let node = self.node();
        if !node.found {
            return Err("没有找到 Node.js 运行时".into());
        }
        let entry = resolve_entry(info);
        let entry = entry.ok_or_else(|| "这个扩展没有 main 入口（纯声明式扩展不需要宿主）".to_string())?;
        let dir = PathBuf::from(&info.dir);
        if !dir.is_dir() {
            return Err("扩展目录不存在，请重装".into());
        }

        let boot_path = write_bootstrap(&entry)?;
        let workspace = crate::util::lock(&self.inner.config).workspace.clone().unwrap_or_default();
        let language = crate::util::lock(&self.inner.config).language.clone();
        let pkg: Value = std::fs::read(dir.join("package.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Value::Null);
        // Extensions read their own manifest (`extension.packageJSON`) for
        // command titles and view names. VS Code localises it before activation;
        // skipping that leaves `%key%` strings in whatever UI they build.
        let pkg = crate::extensions::localize_manifest(&pkg, &dir, &language);
        // `type: module`, or an `.mjs` entry: `require()` rejects the latter
        // with ERR_REQUIRE_ESM, which surfaced as "failed to load".
        let esm = pkg.get("type").and_then(|v| v.as_str()) == Some("module")
            || entry.extension().and_then(|e| e.to_str()) == Some("mjs");

        let cfg = json!({
            "id": info.id,
            "name": info.display_name,
            "version": info.version,
            "dir": info.dir,
            "entry": entry.to_string_lossy(),
            "esm": esm,
            "workspace": workspace,
            "workspaceName": Path::new(&workspace).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
            "storageDir": dir.to_string_lossy(),
            "language": language,
            "package": pkg,
        });

        let mut cmd = Command::new(&node.path);
        cmd.arg(&boot_path)
            .current_dir(if workspace.is_empty() { dir.clone() } else { PathBuf::from(&workspace) })
            .env("IRIXEN_HOST_CFG", serde_json::to_string(&cfg).unwrap_or_default())
            .env("ELECTRON_RUN_AS_NODE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }

        let mut child = cmd.spawn().map_err(|e| format!("启动 node 失败：{e}"))?;
        let pid = child.id();
        let stdout = child.stdout.take().ok_or("无法读取宿主输出")?;
        let stderr = child.stderr.take().ok_or("无法读取宿主日志")?;
        let stdin = child.stdin.take().ok_or("无法写入宿主输入")?;

        let (tx, rx) = channel::<String>();
        // Writer thread: the only place that touches the child's stdin, so the
        // reader and the UI never race on the pipe.
        {
            let mut stdin = stdin;
            std::thread::spawn(move || {
                while let Ok(line) = rx.recv() {
                    if stdin.write_all(line.as_bytes()).is_err() || stdin.write_all(b"\n").is_err() {
                        break;
                    }
                    let _ = stdin.flush();
                }
            });
        }

        let status = Arc::new(Mutex::new(HostStatus {
            id: info.id.clone(),
            state: "starting".into(),
            pid,
            node: format!("{} {}", node.version, node.path),
            error: String::new(),
            started_at: crate::util::now_ms(),
            activated: false,
            commands: Vec::new(),
            entry: entry.to_string_lossy().to_string(),
        }));

        let child = Arc::new(Mutex::new(child));

        // Reader thread: parse frames.
        {
            let inner = self.inner.clone();
            let id = info.id.clone();
            let tx = tx.clone();
            let status = status.clone();
            std::thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    match line {
                        Ok(l) => {
                            if l.len() > MAX_FRAME {
                                continue;
                            }
                            if let Ok(msg) = serde_json::from_str::<Value>(&l) {
                                handle_frame(&inner, &id, &tx, &status, msg);
                            }
                        }
                        Err(_) => break,
                    }
                }
                // Process is gone.
                {
                    let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
                    st.state = if st.state == "stopping" { "stopped" } else { "crashed" }.into();
                    st.pid = 0;
                }
                drop_waiters(&inner, &id);
                inner
                    .providers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .retain(|p| p.ext != id);
                let _ = inner.app.emit("exthost://status", json!({ "hosts": statuses(&inner) }));
            });
        }

        // stderr → log ring.
        {
            let inner = self.inner.clone();
            let id = info.id.clone();
            std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    push_log(&inner, &id, "err", line);
                }
            });
        }

        let host = Host { tx, child, status: status.clone() };
        self.inner
            .hosts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(info.id.clone(), host);

        // Declared commands are known before activation; mark them not-live so
        // the palette can show them and fail with a real message.
        {
            let mut cmds = self.inner.commands.lock().unwrap_or_else(|e| e.into_inner());
            for c in &info.contributes.commands {
                cmds.insert(
                    c.command.clone(),
                    RuntimeCommand {
                        id: c.command.clone(),
                        title: if c.title.is_empty() { c.command.clone() } else { c.title.clone() },
                        category: c.category.clone(),
                        ext: info.id.clone(),
                        ext_name: info.display_name.clone(),
                        live: false,
                    },
                );
            }
        }

        let st = status.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let _ = self.inner.app.emit("exthost://status", json!({ "hosts": self.status() }));
        let _ = self.inner.app.emit("exthost://commands", json!({ "commands": self.commands() }));
        Ok(st)
    }

    pub fn stop(&self, id: &str) -> Result<(), String> {
        let hosts = self.inner.hosts.lock().unwrap_or_else(|e| e.into_inner());
        let Some(host) = hosts.get(id) else {
            return Ok(());
        };
        {
            let mut st = host.status.lock().unwrap_or_else(|e| e.into_inner());
            st.state = "stopping".into();
            // The process is going away; keep no stale pid for the UI to show.
            st.pid = 0;
        }
        // Ask politely first so `deactivate()` runs, then make sure it is gone.
        let _ = host.tx.send(json!({ "t": "rreq", "id": 0, "method": "deactivate", "params": {} }).to_string());
        let child = host.child.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            let mut c = child.lock().unwrap_or_else(|e| e.into_inner());
            let _ = c.kill();
        });
        drop(hosts);
        self.inner
            .commands
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, c| c.ext != id);
        self.inner
            .statusbar
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|s| s.ext != id);
        // Providers belong to the process that registered them; leaving them
        // behind means every later completion waits 6 s for a dead pipe.
        self.inner
            .providers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|p| p.ext != id);
        // Tree providers live in that same process. Drop their rows so the
        // sidebar shows "waiting" instead of asking a dead pipe for children.
        {
            let mut trees = self.inner.trees.lock().unwrap_or_else(|e| e.into_inner());
            let gone: Vec<String> = trees
                .iter()
                .filter(|(_, r)| r.ext == id)
                .map(|(k, _)| k.clone())
                .collect();
            trees.retain(|_, r| r.ext != id);
            drop(trees);
            for view in gone {
                let _ = self
                    .inner
                    .app
                    .emit("exthost://tree", json!({ "action": "unregister", "viewId": view }));
            }
        }
        {
            let mut views = self.inner.webviews.lock().unwrap_or_else(|e| e.into_inner());
            let gone: Vec<String> = views
                .iter()
                .filter(|(_, v)| v.get("ext").and_then(|e| e.as_str()) == Some(id))
                .map(|(k, _)| k.clone())
                .collect();
            views.retain(|_, v| v.get("ext").and_then(|e| e.as_str()) != Some(id));
            drop(views);
            // The renderer still holds the DOM for those panels. Without a
            // dispose it keeps showing a dead frame until the next full reload.
            for wid in gone {
                let _ = self
                    .inner
                    .app
                    .emit("exthost://webview", json!({ "action": "dispose", "id": wid }));
            }
        }
        let _ = self.inner.app.emit("exthost://status", json!({ "hosts": self.status() }));
        let _ = self.inner.app.emit("exthost://statusbar", json!({ "items": self.statusbar() }));
        Ok(())
    }

    pub fn stop_all(&self) {
        let ids: Vec<String> = self
            .inner
            .hosts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        for id in ids {
            let _ = self.stop(&id);
        }
    }

    /// Execute a command on whichever host registered it.
    ///
    /// VS Code activates an extension lazily when one of its contributed
    /// commands runs. We do the same: a declared-but-not-running command starts
    /// its host, waits for `activate()`, then executes. Without this, every
    /// extension that does not ask for `onStartupFinished` (Live Preview, most
    /// language tooling) was installed and completely inert.
    pub fn run_command(&self, command: &str, args: Vec<Value>) -> Result<Value, String> {
        let ext = {
            let cmds = self.inner.commands.lock().unwrap_or_else(|e| e.into_inner());
            cmds.get(command).map(|c| c.ext.clone())
        };
        let ext = match ext {
            Some(ext) if self.running(&ext) => ext,
            other => {
                let owner = other.or_else(|| self.command_owner(command));
                let Some(owner) = owner else {
                    return Err(format!("没有扩展注册命令 {command}"));
                };
                if let Some(info) = crate::extensions::find(&owner) {
                    self.start(&info)?;
                } else {
                    return Err(format!("扩展 {owner} 已不存在"));
                }
                self.wait_activated(&owner, Duration::from_secs(20));
                owner
            }
        };
        self.reverse(&ext, "executeCommand", json!({ "command": command, "args": args }))
    }

    /// Which installed extension declares this command in `contributes.commands`.
    fn command_owner(&self, command: &str) -> Option<String> {
        for info in crate::extensions::list() {
            if info.contributes.commands.iter().any(|c| c.command == command) {
                return Some(info.id);
            }
        }
        None
    }

    fn wait_activated(&self, id: &str, budget: Duration) {
        let started = std::time::Instant::now();
        while started.elapsed() < budget {
            match self.status_of(id) {
                Some(s) if s.activated => return,
                Some(s) if s.state == "crashed" || s.state == "stopped" => return,
                _ => std::thread::sleep(Duration::from_millis(80)),
            }
        }
    }

    fn reverse(&self, id: &str, method: &str, params: Value) -> Result<Value, String> {
        self.reverse_timeout(id, method, params, Duration::from_secs(60))
    }

    fn reverse_timeout(
        &self,
        id: &str,
        method: &str,
        params: Value,
        budget: Duration,
    ) -> Result<Value, String> {
        let rid = self.inner.rev_seq.fetch_add(1, Ordering::SeqCst);
        let (rtx, rrx) = channel::<Result<Value, String>>();
        self.inner
            .rev_waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(rid, rtx);
        let line = json!({ "t": "rreq", "id": rid, "method": method, "params": params }).to_string();
        {
            let hosts = self.inner.hosts.lock().unwrap_or_else(|e| e.into_inner());
            let host = hosts.get(id).ok_or_else(|| format!("{id} 的宿主没有运行"))?;
            host.tx.send(line).map_err(|_| "宿主已退出".to_string())?;
        }
        match rrx.recv_timeout(budget) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e),
            Err(RecvTimeoutError::Timeout) => Err(format!("扩展响应超时（{}s）", budget.as_secs())),
            Err(RecvTimeoutError::Disconnected) => Err("宿主已退出".into()),
        }
    }

    pub fn providers(&self) -> Vec<ProviderRow> {
        self.inner
            .providers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Ask every host that registered a provider of `kind` for `language`.
    ///
    /// Returns one entry per provider that answered, in registration order. The
    /// renderer decides how to merge them (first non-empty, concatenated, …).
    pub fn language_call(
        &self,
        kind: &str,
        language: &str,
        doc: Value,
        extra: Value,
    ) -> Vec<Value> {
        let targets: Vec<(String, String)> = {
            let rows = self.inner.providers.lock().unwrap_or_else(|e| e.into_inner());
            rows.iter()
                .filter(|p| p.kind == kind && selector_matches(&p.selector, language))
                .map(|p| (p.ext.clone(), p.id.clone()))
                .collect()
        };
        let mut out = Vec::new();
        for (ext, provider) in targets {
            let mut params = json!({ "provider": provider, "kind": kind, "doc": doc });
            if let (Some(obj), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
                for (k, v) in extra {
                    obj.insert(k.clone(), v.clone());
                }
            }
            match self.reverse_timeout(&ext, "provider.call", params, Duration::from_secs(6)) {
                Ok(Value::Null) => {}
                Ok(v) => out.push(json!({ "ext": ext, "value": v })),
                Err(e) => push_log(&self.inner, &ext, "err", format!("{kind} provider: {e}")),
            }
        }
        out
    }

    /// Send a message from the panel page to the extension's webview.
    pub fn webview_post(&self, id: &str, message: Value) -> Result<(), String> {
        let ext = {
            let map = self.inner.webviews.lock().unwrap_or_else(|e| e.into_inner());
            map.get(id)
                .and_then(|w| w.get("ext"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };
        let Some(ext) = ext else { return Ok(()) };
        self.reverse_timeout(
            &ext,
            "webview.message",
            json!({ "id": id, "message": message }),
            Duration::from_secs(5),
        )
        .map(|_| ())
    }

    pub fn webview_dispose(&self, id: &str) {
        let ext = self
            .inner
            .webviews
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
            .and_then(|v| v.get("ext").and_then(|e| e.as_str()).map(|s| s.to_string()));
        let Some(ext) = ext else { return };
        // Closing the panel in the UI must reach the extension. Without this
        // `currentPanel` stays set, so every later "Start Preview" takes the
        // reveal path and no UI ever comes back. Fire-and-forget: never block
        // the click handler on an extension round-trip.
        let line = json!({ "t": "rreq", "id": 0, "method": "webview.dispose", "params": { "id": id } }).to_string();
        let hosts = self.inner.hosts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(host) = hosts.get(&ext) {
            let _ = host.tx.send(line);
        }
    }

    pub fn webviews(&self) -> Vec<Value> {
        self.inner
            .webviews
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// View ids whose extension has registered a live tree data provider.
    pub fn tree_views(&self) -> Vec<String> {
        self.inner
            .trees
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }

    fn tree_call(&self, view: &str, method: &str, params: Value) -> Result<Value, String> {
        let row = self
            .inner
            .trees
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(view)
            .cloned()
            .ok_or_else(|| format!("视图 {view} 尚未注册数据提供者"))?;
        let mut body = params.as_object().cloned().unwrap_or_default();
        body.insert("viewId".into(), json!(view));
        body.insert("pid".into(), json!(row.pid));
        // Gradle's first `getChildren` imports a project and can legitimately
        // take a while; 30s is enough without freezing the UI on a dead host.
        self.reverse_timeout(&row.ext, method, Value::Object(body), Duration::from_secs(30))
    }

    pub fn tree_children(&self, view: &str, key: Option<String>) -> Result<Value, String> {
        self.tree_call(view, "tree.children", json!({ "key": key.unwrap_or_default() }))
    }

    pub fn tree_command(&self, view: &str, key: &str, command: Option<String>) -> Result<Value, String> {
        self.tree_call(view, "tree.command", json!({ "key": key, "command": command }))
    }

    pub fn tree_refresh(&self, view: &str) -> Result<Value, String> {
        self.tree_call(view, "tree.refresh", json!({}))
    }

    /// The renderer answered a `window.*` prompt. `id` is the id we handed the
    /// renderer; the frame we send back carries the host's own request id.
    pub fn reply_ui(&self, id: u64, value: Value) {
        let waiter = self
            .inner
            .ui_waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        if let Some(w) = waiter {
            let line = json!({ "t": "res", "id": w.req, "result": value }).to_string();
            let _ = w.tx.send(line);
        }
    }

    /// Push an editor event to every running host.
    pub fn doc_event(&self, kind: &str, doc: Value) {
        {
            let mut docs = self.inner.docs.lock().unwrap_or_else(|e| e.into_inner());
            let path = doc.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
            match kind {
                "close" => {
                    docs.remove(&path);
                }
                _ => {
                    docs.insert(path, doc.clone());
                }
            }
        }
        let hosts = self.inner.hosts.lock().unwrap_or_else(|e| e.into_inner());
        for host in hosts.values() {
            let st = host.status.lock().unwrap_or_else(|e| e.into_inner()).state.clone();
            if st != "running" && st != "starting" {
                continue;
            }
            let mut payload = doc.clone();
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("kind".into(), Value::String(kind.into()));
            }
            let line = json!({ "t": "rreq", "id": 0, "method": "doc", "params": payload }).to_string();
            let _ = host.tx.send(line);
        }
    }

    /// Start every extension that can run.
    ///
    /// VS Code itself only activates what the user touches, and it can afford
    /// that because every other piece of the editor (language features,
    /// commands registered at activation, tree views) is native. Here the host
    /// *is* the extension runtime: an extension that never activates
    /// contributes nothing but its manifest — no providers, no `onDid*` hooks,
    /// no commands that work. So the default is to start them all, and the
    /// `auto_activate_all` flag exists only for someone who wants the old lazy
    /// behaviour back.
    pub fn autostart(&self) {
        let cfg = crate::util::lock(&self.inner.config).clone();
        if !cfg.extensions.node_host {
            return;
        }
        let activate_all = cfg.extensions.auto_activate_all;
        let mut wanted: Vec<crate::extensions::ExtensionInfo> = crate::extensions::list()
            .into_iter()
            .filter(|i| i.enabled && !i.broken)
            .filter(|i| {
                activate_all
                    || i.activation.iter().any(|e| e == "*" || e == "onStartupFinished")
            })
            .filter(|i| resolve_entry(i).is_some())
            .collect();
        if wanted.is_empty() {
            return;
        }
        // `onStartupFinished` first — those are the ones the user's daily work
        // leans on. The rest follow in a stable order so a restart is
        // reproducible when something goes wrong.
        wanted.sort_by_key(|i| {
            let eager = i.activation.iter().any(|e| e == "*" || e == "onStartupFinished");
            (!eager, i.id.clone())
        });

        let stagger = cfg.extensions.autostart_stagger_ms;
        let host = self.clone_handle();
        std::thread::spawn(move || {
            let total = wanted.len();
            for (n, info) in wanted.into_iter().enumerate() {
                // One node process per extension, each pulling in its own
                // bundle: starting eight at once starves the disk and the
                // window stops painting. A small gap keeps the UI responsive.
                if n > 0 && stagger > 0 {
                    std::thread::sleep(Duration::from_millis(stagger));
                }
                match host.start(&info) {
                    Ok(_) => push_log(
                        &host.inner,
                        &info.id,
                        "out",
                        format!("已启动（{}/{}）", n + 1, total),
                    ),
                    Err(e) => push_log(&host.inner, &info.id, "err", format!("启动失败：{e}")),
                }
            }
        });
    }

    fn clone_handle(&self) -> ExtHost {
        ExtHost { inner: self.inner.clone() }
    }
}

/// Which file the host should load, resolved against the extension directory.
///
/// Three shapes that all exist in the wild and all used to end with "这个扩展
/// 没有 main 入口":
///   * `main` is a directory (`"./out"`) — Node resolves it to its index
///   * only `browser` is declared — the entry for web extensions
///   * `main` is `.mjs` — a module whatever `type` says
fn resolve_entry(info: &crate::extensions::ExtensionInfo) -> Option<PathBuf> {
    let dir = PathBuf::from(&info.dir);
    let pkg: Value = std::fs::read(dir.join("package.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    let field = |k: &str| {
        pkg.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };

    let mut candidates: Vec<String> = Vec::new();
    if !info.entry.is_empty() {
        candidates.push(info.entry.clone());
    } else {
        for key in ["main", "browser"] {
            let v = field(key);
            if !v.is_empty() {
                candidates.push(v);
            }
        }
    }

    for rel in candidates {
        let rel = rel.trim_start_matches("./").replace('\\', "/");
        let path = dir.join(&rel);
        if path.is_file() {
            return Some(path);
        }
        if path.is_dir() {
            for name in ["index.js", "extension.js", "main.js", "index.mjs", "extension.mjs"] {
                let p = path.join(name);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Names an ESM extension may `import { X } from 'vscode'`. The shim writes a
/// static assignment for each one; unimplemented names get a permissive stub so
/// linking succeeds and the failure, if any, is a clear runtime error instead
/// of an opaque `SyntaxError: Named export 'X' not found`.
const VSCODE_NAMES: &[&str] = &[
    // namespaces
    "commands", "window", "workspace", "languages", "env", "extensions", "debug",
    "tasks", "scm", "notebook", "l10n", "authentication", "comments", "test",
    // primitives / types
    "Uri", "Position", "Range", "Selection", "Location", "LocationLink",
    "Disposable", "EventEmitter", "CancellationTokenSource", "CancellationToken",
    "ThemeColor", "ThemeIcon", "MarkdownString", "SnippetString",
    "RelativePattern", "FileSystemError", "DataTransfer", "DataTransferItem",
    // edits / documents
    "TextEdit", "WorkspaceEdit", "SnippetTextEdit", "TextDocumentSaveReason",
    "TextDocumentChangeReason", "EndOfLine", "IndentAction",
    // diagnostics / code actions
    "Diagnostic", "DiagnosticSeverity", "DiagnosticTag",
    "DiagnosticRelatedInformation", "CodeAction", "CodeActionKind",
    "CodeActionTriggerKind", "CodeLens", "DocumentLink", "DocumentHighlight",
    "DocumentHighlightKind", "InlayHint", "InlayHintKind",
    // completion / symbols / hover
    "CompletionItem", "CompletionItemKind", "CompletionList",
    "CompletionTriggerKind", "Hover", "SignatureHelp", "SignatureInformation",
    "ParameterInformation", "DocumentSymbol", "SymbolInformation", "SymbolKind",
    "SymbolTag", "SemanticTokens", "SemanticTokensLegend", "SemanticTokensBuilder",
    "FoldingRange", "FoldingRangeKind", "SelectionRange", "Color",
    "ColorInformation", "ColorPresentation", "LinkedEditingRanges",
    "CallHierarchyItem", "CallHierarchyIncomingCall", "CallHierarchyOutgoingCall",
    "TypeHierarchyItem",
    // UI / views
    "StatusBarAlignment", "ViewColumn", "ProgressLocation", "ConfigurationTarget",
    "ExtensionMode", "ExtensionKind", "UIKind", "QuickPickItemKind",
    "InputBoxValidationSeverity", "TextEditorRevealType",
    "TextEditorSelectionChangeKind", "TextEditorDecorationType",
    "DecorationRangeBehavior", "OverviewRulerLane", "QuickInputButtons",
    "TreeItem", "TreeItemCollapsibleState", "ThemeIconPath",
    "LanguageStatusSeverity", "FileType", "FileChangeType",
    // notebooks / tasks / comments
    "NotebookRange", "NotebookCellKind", "NotebookCellData", "NotebookData",
    "NotebookEdit", "NotebookCellStatusBarAlignment", "NotebookEditorRevealType",
    "Task", "TaskGroup", "TaskScope", "TaskRevealKind", "TaskPanelKind",
    "ShellExecution", "ShellQuoting", "ProcessExecution", "CustomExecution",
    "CommentThreadCollapsibleState", "CommentMode", "CommentAuthorInformation",
    // inline / chat
    "InlineValueText", "InlineValueVariableLookup", "InlineValueEvaluatableExpression",
    "InlineCompletionItem", "InlineCompletionList", "InlineCompletionTriggerKind",
];

/// JS identifier, so a name scraped out of a bundle cannot inject code.
fn is_ident(s: &str) -> bool {
    let mut it = s.chars();
    match it.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    it.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// Pull `import { A, B as C } from "vscode"` names out of a bundled ESM entry.
/// Only the *exported* names matter, aliases are dropped.
fn scan_vscode_imports(entry: &Path) -> Vec<String> {
    let mut text = std::fs::read_to_string(entry).unwrap_or_default();
    if text.len() > 4_000_000 {
        text.truncate(4_000_000);
    }
    let mut out = Vec::new();
    for marker in ["from \"vscode\"", "from 'vscode'"] {
        let mut pos = 0usize;
        while let Some(rel) = text.get(pos..).and_then(|t| t.find(marker)) {
            let at = pos + rel;
            if let Some(open) = text[..at].rfind('{') {
                let between = &text[open + 1..at];
                if let Some(close) = between.rfind('}') {
                    let body = &between[..close];
                    if body.len() < 4000 {
                        for part in body.split(',') {
                            let words: Vec<&str> = part.split_whitespace().collect();
                            let name = if words.first() == Some(&"type") {
                                words.get(1).copied().unwrap_or("")
                            } else {
                                words.first().copied().unwrap_or("")
                            };
                            if is_ident(name) && !out.iter().any(|x| x == name) {
                                out.push(name.to_string());
                            }
                        }
                    }
                }
            }
            pos = at + marker.len();
        }
    }
    out
}

/// Header for the generated `node_modules/vscode/index.js`. `stub` is a
/// permissive placeholder: property access, calls and `new` all succeed, so a
/// provider class an extension merely references at module scope can still be
/// defined. `then` is deliberately undefined to keep it non-thenable.
const SHIM_HEADER: &str = r#"const api = global.__IRIXEN_VSCODE__ || {};
function stub(name) {
  const p = new Proxy(function () { return p; }, {
    get(t, k) {
      if (k === 'name') return name;
      if (k === 'then' || k === Symbol.iterator) return undefined;
      if (k === Symbol.toPrimitive || k === 'toString' || k === 'valueOf') return () => name;
      return p;
    },
    apply() { return p; },
    construct() { return {}; },
  });
  return p;
}
module.exports = api;
"#;

fn write_bootstrap(entry: &Path) -> Result<PathBuf, String> {
    let dir = crate::config::base_dir().join("runtime").join("exthost");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建宿主目录失败：{e}"))?;
    let path = dir.join("bootstrap.js");
    let stale = std::fs::read_to_string(&path).map(|s| s != BOOTSTRAP).unwrap_or(true);
    if stale {
        std::fs::write(&path, BOOTSTRAP).map_err(|e| format!("写入宿主脚本失败：{e}"))?;
    }

    // `<config>/node_modules/vscode` makes `import 'vscode'` (ESM) resolve the
    // same singleton the CommonJS hook returns: the loader walks up from
    // `<config>/extensions/<id>/` and finds it. The named re-exports below are
    // what lets `import { commands } from 'vscode'` work under cjs-module-lexer.
    let shim_dir = crate::config::base_dir().join("node_modules").join("vscode");
    let _ = std::fs::create_dir_all(&shim_dir);
    let pkg = r#"{"name":"vscode","version":"1.101.0","main":"index.js"}"#;

    // cjs-module-lexer (which Node uses to build the named-export list of a
    // CommonJS module for `import { X } from 'vscode'`) only sees *static*
    // `module.exports.X = ...` lines. A Proxy on the exported object cannot
    // create named exports. So the shim is generated: a broad built-in list,
    // plus every name this extension actually imports, plus everything an
    // earlier host already wrote. That makes the file grow monotonically and
    // removes the race between two hosts starting at the same time.
    let mut names: std::collections::BTreeSet<String> =
        VSCODE_NAMES.iter().map(|n| (*n).to_string()).collect();
    let shim_index = shim_dir.join("index.js");
    let previous = std::fs::read_to_string(&shim_index).unwrap_or_default();
    for line in previous.lines() {
        if let Some(rest) = line.strip_prefix("module.exports.") {
            let n: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
                .collect();
            if is_ident(&n) {
                names.insert(n);
            }
        }
    }
    for n in scan_vscode_imports(entry) {
        names.insert(n);
    }

    let mut index = String::from(SHIM_HEADER);
    for n in &names {
        // The two-line form keeps `module.exports.X = api.X` verbatim, which is
        // exactly the pattern cjs-module-lexer recognises.
        index.push_str(&format!(
            "if (api.{n} === undefined) api.{n} = stub('{n}');\nmodule.exports.{n} = api.{n};\n"
        ));
    }

    let _ = std::fs::write(shim_dir.join("package.json"), pkg);
    let _ = std::fs::write(&shim_index, index);
    Ok(path)
}

// ---------------------------------------------------------------------------
// frame handling
// ---------------------------------------------------------------------------

/// Does a provider's `DocumentSelector` cover this language?
///
/// A selector may be a string, a filter, or an array of filters. A filter that
/// only constrains `scheme`/`pattern` is treated as a match: the renderer only
/// asks about files it has open, and refusing to call a provider is worse than
/// calling one that returns nothing.
fn selector_matches(sel: &Value, language: &str) -> bool {
    let list: Vec<Value> = match sel {
        Value::Array(a) => a.clone(),
        Value::Null => return true,
        other => vec![other.clone()],
    };
    if list.is_empty() {
        return true;
    }
    let mut constrained = false;
    for entry in list {
        let lang = entry
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| entry.get("language").and_then(|v| v.as_str()).map(|s| s.to_string()));
        if let Some(lang) = lang {
            constrained = true;
            if lang == "*" || lang.eq_ignore_ascii_case(language) {
                return true;
            }
        }
    }
    !constrained
}

fn statuses(inner: &Arc<Inner>) -> Vec<HostStatus> {    let hosts = inner.hosts.lock().unwrap_or_else(|e| e.into_inner());
    let mut rows: Vec<HostStatus> = hosts
        .values()
        .map(|h| h.status.lock().unwrap_or_else(|e| e.into_inner()).clone())
        .collect();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows
}

fn push_log(inner: &Arc<Inner>, id: &str, level: &str, text: String) {
    let line = HostLog {
        id: id.to_string(),
        ts: crate::util::now_ms(),
        level: level.to_string(),
        text: text.trim_end().to_string(),
    };
    {
        let mut logs = inner.logs.lock().unwrap_or_else(|e| e.into_inner());
        logs.push_back(line.clone());
        while logs.len() > LOG_CAP {
            logs.pop_front();
        }
    }
    let _ = inner.app.emit("exthost://log", &line);
}

fn respond(tx: &Sender<String>, id: u64, result: Value, error: Option<String>) {
    if id == 0 {
        return;
    }
    let line = match error {
        Some(e) => json!({ "t": "res", "id": id, "error": e }),
        None => json!({ "t": "res", "id": id, "result": result }),
    };
    let _ = tx.send(line.to_string());
}

/// The `argv` array of a `task.run` request, when the host sent one.
///
/// An argv vector runs verbatim; only a plain command line is re-tokenised, so
/// arguments containing spaces or quotes survive.
fn argv_of(params: &Value) -> Option<Vec<String>> {
    let out: Vec<String> = params
        .get("argv")
        .and_then(|v| v.as_array())?
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Forgets the prompts an extension was blocked on. Called when its process
/// exits, so a crashed host cannot leave answered-nowhere dialogs in the queue.
fn drop_waiters(inner: &Arc<Inner>, ext: &str) {
    inner
        .ui_waiters
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|_, w| w.ext != ext);
}

/// Rebuild a webview row from a host notification. Every `create`/`update`/
/// `reveal` frame now carries the full document, so a row the core has lost
/// (renderer reload, closed panel, host restart) can be re-created instead of
/// dropping the message on the floor.
fn webview_row(ext: &str, params: &Value) -> Option<(String, Value)> {
    let wid = params.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if wid.is_empty() {
        return None;
    }
    let panel = json!({
        "id": wid,
        "ext": ext,
        "title": params.get("title").cloned().unwrap_or(json!("")),
        "html": params.get("html").cloned().unwrap_or(json!("")),
        "viewType": params.get("viewType").cloned().unwrap_or(json!("")),
        "options": params.get("options").cloned().unwrap_or(json!({})),
    });
    Some((wid, panel))
}

fn handle_frame(inner: &Arc<Inner>, ext: &str, tx: &Sender<String>, status: &Arc<Mutex<HostStatus>>, msg: Value) {    let t = msg.get("t").and_then(|v| v.as_str()).unwrap_or("");
    match t {
        "rres" => {
            let id = msg.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            let waiter = inner
                .rev_waiters
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            if let Some(w) = waiter {
                let err = msg.get("error").and_then(|v| v.as_str()).map(|s| s.to_string());
                match err {
                    Some(e) if !e.is_empty() => {
                        let _ = w.send(Err(e));
                    }
                    _ => {
                        let _ = w.send(Ok(msg.get("result").cloned().unwrap_or(Value::Null)));
                    }
                }
            }
        }
        "req" => {
            let id = msg.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            match method.as_str() {
                "ready" => {
                    {
                        let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
                        st.state = "running".into();
                        st.error.clear();
                    }
                    push_log(inner, ext, "info", format!("宿主已启动 · {}", params.get("node").and_then(|v| v.as_str()).unwrap_or("")));
                    let _ = inner.app.emit("exthost://status", json!({ "hosts": statuses(inner) }));
                }
                "activated" => {
                    {
                        let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
                        st.activated = true;
                    }
                    let _ = inner.app.emit("exthost://status", json!({ "hosts": statuses(inner) }));
                }
                "fatal" => {
                    let message = params.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    {
                        let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
                        st.state = "crashed".into();
                        st.error = message.clone();
                    }
                    push_log(inner, ext, "err", message);
                    let _ = inner.app.emit("exthost://status", json!({ "hosts": statuses(inner) }));
                }
                "log" => {
                    let level = params.get("level").and_then(|v| v.as_str()).unwrap_or("info");
                    let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    push_log(inner, ext, level, text);
                }
                "output" => {
                    let channel = params.get("channel").and_then(|v| v.as_str()).unwrap_or("output");
                    let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    for line in text.split('\n') {
                        if !line.is_empty() {
                            push_log(inner, ext, "out", format!("[{channel}] {line}"));
                        }
                    }
                }
                "commands.put" => {
                    let cmd = params.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if !cmd.is_empty() {
                        let mut cmds = inner.commands.lock().unwrap_or_else(|e| e.into_inner());
                        let entry = cmds.entry(cmd.clone()).or_insert_with(|| RuntimeCommand {
                            id: cmd.clone(),
                            title: cmd.clone(),
                            category: String::new(),
                            ext: ext.to_string(),
                            ext_name: ext.to_string(),
                            live: false,
                        });
                        entry.live = true;
                        entry.ext = ext.to_string();
                        let list: Vec<RuntimeCommand> = cmds.values().cloned().collect();
                        drop(cmds);
                        {
                            let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
                            st.commands = list.iter().filter(|c| c.ext == ext && c.live).map(|c| c.id.clone()).collect();
                        }
                        let _ = inner.app.emit("exthost://commands", json!({ "commands": list }));
                    }
                    respond(tx, id, Value::Null, None);
                }
                "commands.execute" => {
                    let cmd = params.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let target = inner
                        .commands
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get(&cmd)
                        .map(|c| c.ext.clone());
                    match target {
                        Some(other) if other != ext => {
                            // Cross-extension command: hop to the owning host.
                            let hosts = inner.hosts.lock().unwrap_or_else(|e| e.into_inner());
                            if let Some(h) = hosts.get(&other) {
                                let _ = h.tx.send(
                                    json!({ "t": "rreq", "id": 0, "method": "executeCommand",
                                            "params": { "command": cmd, "args": params.get("args").cloned().unwrap_or(json!([])) } })
                                    .to_string(),
                                );
                            }
                            respond(tx, id, Value::Null, None);
                        }
                        _ => {
                            respond(tx, id, Value::Null, Some(format!("command not registered: {cmd}")));
                        }
                    }
                }
                "window.message" => {
                    let req = inner.ui_seq.fetch_add(1, Ordering::SeqCst);
                    // id 0 means the host asked without awaiting an answer; there
                    // is nothing to reply to, so registering a waiter would only
                    // leak.
                    if id != 0 {
                        inner
                            .ui_waiters
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(req, UiWaiter { ext: ext.to_string(), req: id, tx: tx.clone() });
                    }
                    let _ = inner.app.emit(
                        "exthost://ui",
                        json!({
                            "id": req, "kind": "message", "ext": ext,
                            "severity": params.get("severity").cloned().unwrap_or(json!("info")),
                            "message": params.get("message").cloned().unwrap_or(json!("")),
                            "detail": params.get("detail").cloned().unwrap_or(json!("")),
                            "items": params.get("items").cloned().unwrap_or(json!([])),
                            "modal": params.get("modal").cloned().unwrap_or(json!(false)),
                        }),
                    );
                }
                "window.quickPick" => {
                    let req = inner.ui_seq.fetch_add(1, Ordering::SeqCst);
                    // id 0 means the host asked without awaiting an answer; there
                    // is nothing to reply to, so registering a waiter would only
                    // leak.
                    if id != 0 {
                        inner
                            .ui_waiters
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(req, UiWaiter { ext: ext.to_string(), req: id, tx: tx.clone() });
                    }
                    let _ = inner.app.emit(
                        "exthost://ui",
                        json!({
                            "id": req, "kind": "quickPick", "ext": ext,
                            "items": params.get("items").cloned().unwrap_or(json!([])),
                            "placeholder": params.get("placeholder").cloned().unwrap_or(json!("")),
                            "canPickMany": params.get("canPickMany").cloned().unwrap_or(json!(false)),
                        }),
                    );
                }
                "window.inputBox" => {
                    let req = inner.ui_seq.fetch_add(1, Ordering::SeqCst);
                    // id 0 means the host asked without awaiting an answer; there
                    // is nothing to reply to, so registering a waiter would only
                    // leak.
                    if id != 0 {
                        inner
                            .ui_waiters
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(req, UiWaiter { ext: ext.to_string(), req: id, tx: tx.clone() });
                    }
                    let _ = inner.app.emit(
                        "exthost://ui",
                        json!({
                            "id": req, "kind": "inputBox", "ext": ext,
                            "prompt": params.get("prompt").cloned().unwrap_or(json!("")),
                            "value": params.get("value").cloned().unwrap_or(json!("")),
                            "placeHolder": params.get("placeHolder").cloned().unwrap_or(json!("")),
                            "password": params.get("password").cloned().unwrap_or(json!(false)),
                        }),
                    );
                }
                "window.showTextDocument" => {
                    let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let _ = inner.app.emit("exthost://open", json!({ "path": path }));
                    respond(tx, id, Value::Null, None);
                }
                "statusbar" => {
                    let item_id = params.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let mut items = inner.statusbar.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(slot) = items.iter_mut().find(|i| i.ext == ext && i.id == item_id) {
                        slot.text = params.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        slot.tooltip = params.get("tooltip").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        slot.command = params.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        slot.visible = params.get("visible").and_then(|v| v.as_bool()).unwrap_or(false);
                    } else {
                        items.push(StatusItem {
                            ext: ext.to_string(),
                            id: item_id,
                            text: params.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            tooltip: params.get("tooltip").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            command: params.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            visible: params.get("visible").and_then(|v| v.as_bool()).unwrap_or(false),
                            alignment: params.get("alignment").and_then(|v| v.as_i64()).unwrap_or(1) as i32,
                            priority: params.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                        });
                    }
                    let list = items.clone();
                    drop(items);
                    let _ = inner.app.emit("exthost://statusbar", json!({ "items": list }));
                    respond(tx, id, Value::Null, None);
                }
                "statusbar.remove" => {
                    let item_id = params.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let mut items = inner.statusbar.lock().unwrap_or_else(|e| e.into_inner());
                    items.retain(|i| !(i.ext == ext && i.id == item_id));
                    let list = items.clone();
                    drop(items);
                    let _ = inner.app.emit("exthost://statusbar", json!({ "items": list }));
                    respond(tx, id, Value::Null, None);
                }
                "boot" => {
                    let dir = crate::extensions::find(ext)
                        .map(|i| PathBuf::from(i.dir))
                        .unwrap_or_default();
                    let state = read_state(&dir);
                    let cfg: AppConfig = crate::util::lock(&inner.config).clone();
                    let values = json!({
                        "editor.fontSize": 13,
                        "editor.tabSize": 2,
                        "files.autoSave": "off",
                        "files.eol": "\n",
                        "workbench.colorTheme": cfg.theme,
                        "irixen.language": cfg.language,
                        "irixen.workspace": cfg.workspace.clone().unwrap_or_default(),
                    });
                    respond(tx, id, json!({ "state": state, "config": values }), None);
                }
                "state.set" => {
                    let dir = crate::extensions::find(ext)
                        .map(|i| PathBuf::from(i.dir))
                        .unwrap_or_default();
                    let scope = params.get("scope").and_then(|v| v.as_str()).unwrap_or("global");
                    let key = params.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let value = params.get("value").cloned().unwrap_or(Value::Null);
                    let mut state = read_state(&dir);
                    let bucket = if scope == "workspace" {
                        state.get_mut("workspace")
                    } else {
                        state.get_mut("global")
                    };
                    if let Some(Value::Object(map)) = bucket {
                        if value.is_null() {
                            map.remove(&key);
                        } else {
                            map.insert(key, value);
                        }
                    }
                    let _ = write_state(&dir, &state);
                    respond(tx, id, Value::Null, None);
                }
                "config.get" => {
                    let section = params.get("section").and_then(|v| v.as_str()).unwrap_or("");
                    let cfg: AppConfig = crate::util::lock(&inner.config).clone();
                    let value = match section {
                        "workbench.colorTheme" => Value::String(cfg.theme),
                        "irixen.language" => Value::String(cfg.language),
                        "irixen.workspace" => Value::String(cfg.workspace.unwrap_or_default()),
                        _ => Value::Null,
                    };
                    respond(tx, id, value, None);
                }
                "fs.read" => {
                    let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    match std::fs::read(path) {
                        Ok(bytes) => respond(tx, id, Value::String(String::from_utf8_lossy(&bytes).to_string()), None),
                        Err(e) => respond(tx, id, Value::Null, Some(format!("{path}: {e}"))),
                    }
                }
                "fs.write" => {
                    let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    match std::fs::write(path, text) {
                        Ok(_) => respond(tx, id, Value::Null, None),
                        Err(e) => respond(tx, id, Value::Null, Some(format!("{path}: {e}"))),
                    }
                }
                "fs.stat" => {
                    let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    match std::fs::metadata(path) {
                        Ok(m) => respond(
                            tx,
                            id,
                            json!({
                                "type": if m.is_dir() { "dir" } else { "file" },
                                "size": m.len(),
                                "mtime": m.modified().ok()
                                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                    .map(|d| d.as_millis() as u64)
                                    .unwrap_or(0),
                            }),
                            None,
                        ),
                        Err(e) => respond(tx, id, Value::Null, Some(format!("{path}: {e}"))),
                    }
                }
                "fs.readdir" => {
                    let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    match std::fs::read_dir(path) {
                        Ok(rd) => {
                            let rows: Vec<Value> = rd
                                .flatten()
                                .map(|e| {
                                    json!({
                                        "name": e.file_name().to_string_lossy(),
                                        "dir": e.file_type().map(|t| t.is_dir()).unwrap_or(false),
                                    })
                                })
                                .collect();
                            respond(tx, id, Value::Array(rows), None);
                        }
                        Err(e) => respond(tx, id, Value::Null, Some(format!("{path}: {e}"))),
                    }
                }
                "fs.mkdir" => {
                    let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    match std::fs::create_dir_all(path) {
                        Ok(_) => respond(tx, id, Value::Null, None),
                        Err(e) => respond(tx, id, Value::Null, Some(format!("{path}: {e}"))),
                    }
                }
                "fs.delete" => {
                    let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    let recursive = params.get("recursive").and_then(|v| v.as_bool()).unwrap_or(false);
                    let result = if recursive {
                        std::fs::remove_dir_all(path).or_else(|_| std::fs::remove_file(path))
                    } else {
                        std::fs::remove_file(path)
                    };
                    match result {
                        Ok(_) => respond(tx, id, Value::Null, None),
                        Err(e) => respond(tx, id, Value::Null, Some(format!("{path}: {e}"))),
                    }
                }
                "fs.rename" => {
                    let from = params.get("from").and_then(|v| v.as_str()).unwrap_or("");
                    let to = params.get("to").and_then(|v| v.as_str()).unwrap_or("");
                    match std::fs::rename(from, to) {
                        Ok(_) => respond(tx, id, Value::Null, None),
                        Err(e) => respond(tx, id, Value::Null, Some(format!("{from} → {to}: {e}"))),
                    }
                }
                "editor.edit" => {
                    let _ = inner.app.emit("exthost://edit", params.clone());
                    respond(tx, id, Value::Bool(true), None);
                }
                "doc.save" => {
                    let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let _ = inner.app.emit("exthost://save", json!({ "path": path }));
                    respond(tx, id, Value::Null, None);
                }
                "env.openExternal" => {
                    let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    respond(tx, id, Value::Null, crate::util::open_external(&url).err());
                }
                "clipboard.read" | "clipboard.write" => respond(tx, id, Value::Null, None),
                "providers.put" => {
                    let pid = params.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let kind = params.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if !pid.is_empty() && !kind.is_empty() {
                        let selector = params.get("selector").cloned().unwrap_or(json!([]));
                        let trigger_characters = params
                            .get("triggerCharacters")
                            .and_then(|v| v.as_array())
                            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                            .unwrap_or_default();
                        let mut rows = inner.providers.lock().unwrap_or_else(|e| e.into_inner());
                        rows.retain(|p| !(p.ext == ext && p.id == pid));
                        rows.push(ProviderRow {
                            ext: ext.to_string(),
                            id: pid,
                            kind,
                            selector,
                            trigger_characters,
                        });
                        drop(rows);
                        push_log(inner, ext, "info", "已注册语言功能（补全/格式化等）".into());
                    }
                    respond(tx, id, Value::Null, None);
                }
                "providers.remove" => {
                    let pid = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    inner
                        .providers
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .retain(|p| !(p.ext == ext && p.id == pid));
                    respond(tx, id, Value::Null, None);
                }
                // A contributed sidebar view becomes live only when its
                // extension calls `createTreeView` / `registerTreeDataProvider`.
                // The renderer needs that moment both to switch the section out
                // of its "waiting" state and to re-read data after `fire()`.
                "tree.register" => {
                    let view = params.get("viewId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let pid = params.get("pid").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if !view.is_empty() && !pid.is_empty() {
                        inner
                            .trees
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(view.clone(), TreeRow { ext: ext.to_string(), pid });
                        push_log(inner, ext, "info", format!("已注册侧栏视图 · {view}"));
                        let _ = inner.app.emit(
                            "exthost://tree",
                            json!({ "action": "register", "viewId": view }),
                        );
                    }
                    respond(tx, id, Value::Null, None);
                }
                "tree.changed" => {
                    let view = params.get("viewId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if !view.is_empty() {
                        let _ = inner.app.emit(
                            "exthost://tree",
                            json!({ "action": "changed", "viewId": view }),
                        );
                    }
                    respond(tx, id, Value::Null, None);
                }
                "tree.unregister" => {
                    let view = params.get("viewId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let pid = params.get("pid").and_then(|v| v.as_str()).unwrap_or("");
                    let removed = {
                        let mut rows = inner.trees.lock().unwrap_or_else(|e| e.into_inner());
                        let same = rows.get(&view).map(|r| r.pid.as_str()) == Some(pid);
                        if same { rows.remove(&view).is_some() } else { false }
                    };
                    if removed {
                        let _ = inner.app.emit(
                            "exthost://tree",
                            json!({ "action": "unregister", "viewId": view }),
                        );
                    }
                    respond(tx, id, Value::Null, None);
                }
                "webview.create" => {
                    if let Some((wid, panel)) = webview_row(ext, &params) {
                        inner
                            .webviews
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(wid, panel.clone());
                        let _ = inner.app.emit("exthost://webview", json!({ "action": "create", "panel": panel }));
                    }
                    respond(tx, id, Value::Null, None);
                }
                "webview.update" => {
                    if let Some((wid, row)) = webview_row(ext, &params) {
                        let mut views = inner.webviews.lock().unwrap_or_else(|e| e.into_inner());
                        let (snapshot, action) = match views.get_mut(&wid) {
                            Some(existing) => {
                                if let Some(obj) = existing.as_object_mut() {
                                    for key in ["html", "title", "viewType", "options"] {
                                        if let Some(v) = params.get(key) {
                                            // Extensions often clear `webview.html`
                                            // for a tick before rebuilding it. An
                                            // empty update must not erase the last
                                            // real document: that row is what the
                                            // renderer uses to recover a blank
                                            // panel after a host restart.
                                            if key == "html" && v.as_str() == Some("") {
                                                continue;
                                            }
                                            obj.insert(key.to_string(), v.clone());
                                        }
                                    }
                                }
                                (existing.clone(), "update")
                            }
                            // The row is gone but the document is not: a late
                            // update must rebuild the panel, not be discarded.
                            None => {
                                views.insert(wid, row.clone());
                                (row, "create")
                            }
                        };
                        drop(views);
                        let _ = inner
                            .app
                            .emit("exthost://webview", json!({ "action": action, "panel": snapshot }));
                    }
                    respond(tx, id, Value::Null, None);
                }
                // Live Preview (and most panel extensions) keep `currentPanel`
                // for the session: after the first open they only reveal and
                // post, never create again. If the renderer lost the panel this
                // is the *only* frame we get — rebuild it from the carried HTML.
                "webview.reveal" => {
                    if let Some((wid, row)) = webview_row(ext, &params) {
                        let incoming = params.get("html").and_then(|v| v.as_str()).unwrap_or("");
                        let mut views = inner.webviews.lock().unwrap_or_else(|e| e.into_inner());
                        let missing = !views.contains_key(&wid);
                        // A row rebuilt by an early `post` has no document yet.
                        let empty_doc = views
                            .get(&wid)
                            .and_then(|v| v.get("html"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .is_empty();
                        if missing {
                            views.insert(wid.clone(), row.clone());
                        } else if let Some(obj) = views.get_mut(&wid).and_then(|v| v.as_object_mut()) {
                            for key in ["html", "title"] {
                                if let Some(v) = params.get(key) {
                                    if key == "html" && v.as_str() == Some("") {
                                        continue;
                                    }
                                    obj.insert(key.to_string(), v.clone());
                                }
                            }
                        }
                        let snapshot = views.get(&wid).cloned().unwrap_or_else(|| row.clone());
                        drop(views);
                        // A reveal means "bring it forward", not "reload it".
                        // The document still travels with the event: if the
                        // renderer's frame went blank (404, lost publish, cold
                        // scheme) it can republish without an extension restart.
                        // An unchanged document is skipped by `docKey`, so the
                        // previewed page is not reset on a normal second Start.
                        let _ = if missing {
                            inner.app.emit("exthost://webview", json!({ "action": "create", "panel": row }))
                        } else if empty_doc && !incoming.is_empty() {
                            inner.app.emit("exthost://webview", json!({ "action": "update", "panel": snapshot.clone() }))
                        } else {
                            inner.app.emit("exthost://webview", json!({ "action": "reveal", "id": wid, "panel": snapshot }))
                        };
                    }
                    respond(tx, id, Value::Null, None);
                }
                "webview.post" => {
                    let wid = params.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let message = params.get("message").cloned().unwrap_or(Value::Null);
                    // A post can arrive before the panel exists (the extension
                    // reveals first). Rebuild from the last known row so the
                    // message is not lost; the renderer buffers until `load`.
                    let row = inner
                        .webviews
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get(&wid)
                        .cloned();
                    if row.is_none() {
                        if let Some((id2, made)) = webview_row(ext, &params) {
                            inner
                                .webviews
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .insert(id2, made.clone());
                            let _ = inner.app.emit("exthost://webview", json!({ "action": "create", "panel": made }));
                        }
                    }
                    let _ = inner.app.emit(
                        "exthost://webview",
                        json!({ "action": "post", "id": wid, "message": message }),
                    );
                    respond(tx, id, Value::Null, None);
                }
                "webview.dispose" => {
                    let wid = params.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    // `webview_close` already removed the row. Suppress the
                    // second dispose emitted by `panel.dispose()` on the host
                    // side so the UI does not get a duplicate close event.
                    let removed = inner
                        .webviews
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&wid)
                        .is_some();
                    if removed {
                        let _ = inner
                            .app
                            .emit("exthost://webview", json!({ "action": "dispose", "id": wid }));
                    }
                    respond(tx, id, Value::Null, None);
                }
                // An extension terminal is a real shell, not a command runner:
                // `createTerminal` opens a PTY, `sendText` types into it, and
                // `show` brings the panel forward. This is what makes "npm run
                // dev" from Live Preview (or a Gradle task) land in a terminal
                // the user can actually interrupt.
                "terminal.create" => {
                    let cwd = params
                        .get("cwd")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(|s| crate::util::plain_path(s));
                    let shell = params
                        .get("shellPath")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                    let svc = inner
                        .app
                        .state::<Arc<crate::terminal::TermService>>()
                        .inner()
                        .clone();
                    match svc.open(inner.app.clone(), cwd, shell, 100, 24) {
                        Ok(info) => respond(
                            tx,
                            id,
                            json!({ "id": info.id, "shell": info.shell, "cwd": info.cwd }),
                            None,
                        ),
                        Err(e) => respond(tx, id, Value::Null, Some(e)),
                    }
                }
                "terminal.write" => {
                    let tid = params.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let data = params.get("data").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let svc = inner
                        .app
                        .state::<Arc<crate::terminal::TermService>>()
                        .inner()
                        .clone();
                    match svc.write(&tid, &data) {
                        Ok(()) => respond(tx, id, Value::Null, None),
                        Err(e) => respond(tx, id, Value::Null, Some(e)),
                    }
                }
                "terminal.show" => {
                    let tid = params.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let _ = inner
                        .app
                        .emit("exthost://terminal", json!({ "action": "show", "id": tid }));
                    respond(tx, id, Value::Null, None);
                }
                "terminal.close" => {
                    let tid = params.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let svc = inner
                        .app
                        .state::<Arc<crate::terminal::TermService>>()
                        .inner()
                        .clone();
                    let _ = svc.close(&tid);
                    let _ = inner
                        .app
                        .emit("exthost://terminal", json!({ "action": "close", "id": tid }));
                    respond(tx, id, Value::Null, None);
                }
                // A build tool (Gradle, Maven, npm, cargo) is not run by the
                // extension itself: it hands VS Code a `Task` or opens a
                // terminal and calls `sendText`. Both arrive here and go
                // through the same runner as F5, so output lands in the panel
                // the user already knows instead of a void.
                "task.run" | "terminal.run" => {
                    let command = params
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if command.is_empty() {
                        respond(tx, id, Value::Null, Some("命令为空".into()));
                    } else {
                        let cwd = params
                            .get("cwd")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let dir = if cwd.is_empty() {
                            let ws = inner
                                .config
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .workspace
                                .clone()
                                .unwrap_or_default();
                            if ws.trim().is_empty() {
                                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                            } else {
                                PathBuf::from(crate::util::plain_path(&ws))
                            }
                        } else {
                            PathBuf::from(crate::util::plain_path(&cwd))
                        };
                        let svc = inner
                            .app
                            .state::<Arc<crate::runner::RunnerService>>()
                            .inner()
                            .clone();
                        let started = match argv_of(&params) {
                            Some(argv) => svc.run_argv(&inner.app, argv, &dir, Vec::new()),
                            None => svc.run_line(&inner.app, &command, &dir, Vec::new()),
                        };
                        match started {
                            Ok(info) => {
                                let tag = params
                                    .get("taskId")
                                    .or_else(|| params.get("id"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let run_id = info.id.clone();
                                let reply = json!({
                                    "id": info.id,
                                    "command": info.command,
                                    "cwd": info.cwd,
                                });
                                // Report completion back to the host so
                                // `onDidEndTask` / `onDidEndTaskProcess` fire
                                // with a real exit code.
                                let host_tx = inner
                                    .hosts
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .get(ext)
                                    .map(|h| h.tx.clone());
                                if let Some(htx) = host_tx {
                                    // Two copies: one only feeds the lookup key,
                                    // the other moves into the callback.
                                    let probe = run_id.clone();
                                    let owned = run_id.clone();
                                    svc.on_exit(&probe, move |code| {
                                        let _ = htx.send(
                                            json!({
                                                "t": "rreq", "id": 0, "method": "task.exit",
                                                "params": { "taskId": tag, "runId": owned, "code": code },
                                            })
                                            .to_string(),
                                        );
                                    });
                                }
                                respond(tx, id, reply, None);
                            }
                            Err(e) => respond(tx, id, Value::Null, Some(e)),
                        }
                    }
                }
                "task.stop" => {
                    let rid = params.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let svc = inner
                        .app
                        .state::<Arc<crate::runner::RunnerService>>()
                        .inner()
                        .clone();
                    respond(tx, id, Value::Null, svc.stop(&rid).err());
                }
                other => respond(tx, id, Value::Null, Some(format!("host 不支持的方法：{other}"))),
            }
        }
        _ => {}
    }
}

fn state_path(dir: &Path) -> PathBuf {
    dir.join(".state").join("ext-state.json")
}

fn read_state(dir: &Path) -> Value {
    std::fs::read(state_path(dir))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({ "global": {}, "workspace": {} }))
}

fn write_state(dir: &Path, value: &Value) -> Result<(), String> {
    let path = state_path(dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value).unwrap_or_default()).map_err(|e| e.to_string())
}

/// Blocking helper for the command layer, which runs it inside `spawn_blocking`.
pub fn start_blocking(host: &ExtHost, info: &crate::extensions::ExtensionInfo) -> Result<HostStatus, String> {
    host.start(info)
}
