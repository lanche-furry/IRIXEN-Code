//! Tauri command surface — the "native interface" half of IRIXEN Code.
//!
//! Everything the UI can do that touches the OS goes through these commands:
//! files, shell, git, workspace stats, providers, models, sessions, skills, MCP.

use crate::agent::models::ModelInfo;
use crate::agent::types::{Attachment, Effort, Message};
use crate::agent::{AgentEngine, SessionMeta};
use crate::config::{AppConfig, McpServerConfig, ProviderConfig, WebSource};
use crate::git::GitFileStatus;
use crate::mcp::McpToolInfo;
use crate::skills::{Skill, SkillRegistry};
use crate::state::AppState;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{Manager, State};

// ---------------------------------------------------------------------------
// config / app
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> AppConfig {
    crate::util::lock(&state.config).clone()
}

#[tauri::command]
pub fn save_config(state: State<'_, AppState>, mut config: AppConfig) -> Result<AppConfig, String> {
    config.bridge.migrate();
    // The Web2API provider is derived, not user-owned: re-derive it here so a
    // settings draft captured before a bridge restart cannot push a stale
    // port/key back into the live config.
    config.sync_web2api_provider();
    config.sanitize_active();
    // Write first, swap the live config second. If the disk write fails, memory
    // must not hold a config that will not survive a restart — that divergence
    // reads as "the key I typed works until I relaunch".
    config.save().map_err(|e| e.to_string())?;
    {
        let mut cfg = crate::util::lock(&state.config);
        *cfg = config.clone();
    }
    for s in state.skills.list() {
        let want = config.skills.enabled.contains(&s.id);
        if s.enabled != want {
            state.skills.set_enabled(&s.id, want);
        }
    }
    Ok(config)
}

/// Reset the persisted profile back to factory defaults and arm the first-run
/// wizard again.
///
/// This is the in-app twin of deleting `config.json` by hand — the only way out
/// of a profile that was written by an earlier build or by a test run, where
/// `onboarding_completed` is already `true` and the setup flow therefore never
/// appears. The old file is kept next to it as `config.json.bak-<epoch>` so an
/// API key typed by accident is still recoverable.
#[tauri::command]
pub fn config_reset(state: State<'_, AppState>) -> Result<AppConfig, String> {
    let path = crate::config::config_path();
    if path.is_file() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let backup = path.with_file_name(format!("config.json.bak-{stamp}"));
        std::fs::copy(&path, &backup).map_err(|e| format!("备份旧配置失败：{e}"))?;
    }
    // Model names discovered from the old providers would otherwise survive in
    // the picker even though every provider is back to "no key".
    let _ = std::fs::remove_file(crate::config::model_cache_path());

    let fresh = AppConfig::default();
    fresh.save().map_err(|e| e.to_string())?;
    {
        let mut cfg = crate::util::lock(&state.config);
        *cfg = fresh.clone();
    }
    for s in state.skills.list() {
        if s.enabled {
            state.skills.set_enabled(&s.id, false);
        }
    }
    Ok(fresh)
}

#[derive(Serialize)]
pub struct AppInfo {
    /// Human-facing product label shown in every window, e.g. "Beta 0.1".
    pub version: String,
    /// Machine-readable package version for diagnostics, e.g. "0.1.0-beta.1".
    pub semver: String,
    pub config_dir: String,
    pub platform: String,
    pub workspace: Option<String>,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: crate::update::DISPLAY_VERSION.to_string(),
        semver: env!("CARGO_PKG_VERSION").to_string(),
        config_dir: crate::config::base_dir().to_string_lossy().to_string(),
        platform: std::env::consts::OS.to_string(),
        workspace: crate::util::lock(&state.config).workspace.clone(),
    }
}

#[tauri::command]
pub fn set_workspace(state: State<'_, AppState>, path: String) -> Result<String, String> {
    let p = PathBuf::from(&path);
    if !p.is_dir() {
        return Err(format!("{} 不是一个文件夹", p.display()));
    }
    let canonical = p.canonicalize().unwrap_or(p);
    // Windows `canonicalize()` yields a `\\?\`-prefixed verbatim path. Git and
    // a few Win32 APIs choke on that form, the explorer builds `dir/name` with
    // forward slashes (which verbatim paths treat as literal characters), and
    // it reads as garbage in the titlebar — especially for non-ASCII folders.
    let text = crate::util::plain_path(&canonical.to_string_lossy());
    {
        let mut cfg = crate::util::lock(&state.config);
        cfg.workspace = Some(text.clone());
    }
    state.save()?;
    Ok(text)
}

// ---------------------------------------------------------------------------
// providers & models
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn provider_upsert(
    state: State<'_, AppState>,
    provider: ProviderConfig,
) -> Result<Vec<ProviderConfig>, String> {
    {
        let mut cfg = crate::util::lock(&state.config);
        match cfg.providers.iter_mut().find(|p| p.id == provider.id) {
            Some(existing) => *existing = provider,
            None => cfg.providers.push(provider),
        }
    }
    state.save()?;
    Ok(crate::util::lock(&state.config).providers.clone())
}

#[tauri::command]
pub fn provider_delete(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<ProviderConfig>, String> {
    {
        let mut cfg = crate::util::lock(&state.config);
        cfg.providers.retain(|p| p.id != id);
        if cfg.active.as_ref().map(|a| a.provider_id == id).unwrap_or(false) {
            cfg.active = None;
        }
    }
    state.save()?;
    Ok(crate::util::lock(&state.config).providers.clone())
}

#[tauri::command]
pub async fn provider_test(state: State<'_, AppState>, id: String) -> Result<usize, String> {
    state.engine.test_provider(&id).await
}

/// Staged connectivity report — tells the user *where* the request died.
#[tauri::command]
pub async fn provider_probe(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::agent::ProbeReport, String> {
    state.engine.probe(&id).await
}

#[tauri::command]
pub async fn models_list(
    state: State<'_, AppState>,
    provider_id: String,
    refresh: bool,
) -> Result<Vec<ModelInfo>, String> {
    state.engine.models(&provider_id, refresh).await
}

// ---------------------------------------------------------------------------
// agent sessions
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn agent_sessions(engine: State<'_, Arc<AgentEngine>>) -> Vec<SessionMeta> {
    engine.sessions()
}

#[tauri::command]
pub fn agent_new_session(engine: State<'_, Arc<AgentEngine>>) -> SessionMeta {
    engine.new_session()
}

#[tauri::command]
pub fn agent_set_active(engine: State<'_, Arc<AgentEngine>>, id: String) -> bool {
    engine.set_active(&id)
}

#[tauri::command]
pub fn agent_active(engine: State<'_, Arc<AgentEngine>>) -> Option<String> {
    engine.active_id()
}

#[tauri::command]
pub fn agent_messages(engine: State<'_, Arc<AgentEngine>>, id: String) -> Vec<Message> {
    engine.messages(&id)
}

#[tauri::command]
pub fn agent_send(
    engine: State<'_, Arc<AgentEngine>>,
    id: String,
    text: String,
    attachments: Vec<Attachment>,
) -> Result<(), String> {
    engine.inner().send(&id, &text, attachments)
}

#[tauri::command]
pub fn agent_stop(engine: State<'_, Arc<AgentEngine>>, id: String) {
    engine.stop(&id)
}

#[tauri::command]
pub fn agent_clear(engine: State<'_, Arc<AgentEngine>>, id: String) {
    engine.clear(&id)
}

#[tauri::command]
pub fn agent_delete(engine: State<'_, Arc<AgentEngine>>, id: String) {
    engine.delete_session(&id)
}

#[tauri::command]
pub fn agent_set_model(
    engine: State<'_, Arc<AgentEngine>>,
    id: String,
    provider_id: Option<String>,
    model: Option<String>,
    effort: Option<Effort>,
) -> Result<(), String> {
    engine.set_model(&id, provider_id, model, effort)
}

/// Sticky default for the composer's thinking-effort knob.
///
/// Separate from `save_config` on purpose: the picker can be clicked several
/// times while the user compares levels, and rewriting the whole config (with
/// every provider key in it) for one enum is needless risk.
#[tauri::command]
pub fn set_default_effort(state: State<'_, AppState>, effort: Effort) -> Result<(), String> {
    {
        let mut cfg = crate::util::lock(&state.config);
        if cfg.effort == effort {
            return Ok(());
        }
        cfg.effort = effort;
    }
    state.save()
}

#[tauri::command]
pub fn agent_busy(engine: State<'_, Arc<AgentEngine>>, id: String) -> bool {
    engine.is_busy(&id)
}

#[tauri::command]
pub fn agent_usage(engine: State<'_, Arc<AgentEngine>>) -> crate::agent::meter::UsageReport {
    engine.usage()
}

#[tauri::command]
pub fn agent_reset_usage(engine: State<'_, Arc<AgentEngine>>) {
    engine.meter.reset()
}

// ---------------------------------------------------------------------------
// filesystem
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
}

#[tauri::command]
pub fn fs_list(state: State<'_, AppState>, path: String) -> Result<Vec<FsEntry>, String> {
    let base = if path.trim().is_empty() {
        state.workspace()?
    } else {
        PathBuf::from(&path)
    };
    let rd = std::fs::read_dir(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    let mut out: Vec<FsEntry> = Vec::new();
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git" {
            continue;
        }
        let meta = entry.metadata().ok();
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push(FsEntry {
            name,
            // Never hand a verbatim path to the renderer: it joins entries with
            // `/`, which a `\\?\` path would treat as part of the file name.
            path: crate::util::plain_path(&entry.path().to_string_lossy()),
            is_dir: entry.file_type().map(|t| t.is_dir()).unwrap_or(false),
            size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            modified,
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

#[tauri::command]
pub fn fs_read(path: String) -> Result<String, String> {
    let p = Path::new(&path);
    let meta = std::fs::metadata(p).map_err(|e| e.to_string())?;
    if meta.len() > 8 * 1024 * 1024 {
        return Err("文件超过 8MB，编辑器不加载超大文件".into());
    }
    let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
    if bytes.iter().take(4096).any(|b| *b == 0) {
        return Err("看起来是二进制文件".into());
    }
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

#[tauri::command]
pub fn fs_write(path: String, content: String) -> Result<u64, String> {
    let p = Path::new(&path);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(p, content.as_bytes()).map_err(|e| e.to_string())?;
    Ok(content.len() as u64)
}

#[tauri::command]
pub fn fs_create_file(state: State<'_, AppState>, path: String) -> Result<String, String> {
    let p = if Path::new(&path).is_absolute() {
        PathBuf::from(&path)
    } else {
        state.workspace()?.join(&path)
    };
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    if p.exists() {
        return Err("文件已存在".into());
    }
    std::fs::write(&p, b"").map_err(|e| e.to_string())?;
    Ok(p.to_string_lossy().to_string())
}

#[tauri::command]
pub fn fs_create_dir(state: State<'_, AppState>, path: String) -> Result<String, String> {
    let p = if Path::new(&path).is_absolute() {
        PathBuf::from(&path)
    } else {
        state.workspace()?.join(&path)
    };
    std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
    Ok(p.to_string_lossy().to_string())
}

#[tauri::command]
pub fn fs_delete(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if p.is_dir() {
        std::fs::remove_dir_all(p).map_err(|e| e.to_string())
    } else {
        std::fs::remove_file(p).map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub fn fs_rename(from: String, to: String) -> Result<(), String> {
    std::fs::rename(&from, &to).map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct SearchHit {
    pub path: String,
    pub line: u64,
    pub text: String,
}

/// Directories that are never worth walking. `ignore` only knows `.gitignore`,
/// which is empty for a folder that isn't a repo yet (and useless for
/// `node_modules` in a repo that ignores nothing). Skipping them is the
/// difference between "82 ms" and "the window stops responding".
fn is_heavy_dir(name: &str) -> bool {
    matches!(
        name,
        "node_modules"
            | "target"
            | "dist"
            | "build"
            | "out"
            | ".git"
            | ".hg"
            | ".svn"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".tools"
            | ".cache"
            | ".next"
            | ".nuxt"
            | ".turbo"
            | "coverage"
    )
}

/// Build a workspace walker that skips heavy directories.
fn walker(root: &Path) -> ignore::WalkBuilder {
    let mut builder = ignore::WalkBuilder::new(root);
    builder.hidden(true).git_ignore(true);
    builder.filter_entry(|entry| {
        entry
            .file_name()
            .to_str()
            .map(|n| !is_heavy_dir(n))
            .unwrap_or(true)
    });
    builder
}

#[tauri::command]
pub async fn fs_search(
    state: State<'_, AppState>,
    query: String,
    glob: Option<String>,
    case_sensitive: Option<bool>,
    max_results: Option<usize>,
) -> Result<Vec<SearchHit>, String> {
    let ws = state.workspace()?;
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    blocking(move || {
        let case_sensitive = case_sensitive.unwrap_or(false);
        let max = max_results.unwrap_or(200).min(1000);
        let mut builder = walker(&ws);
        builder.max_depth(Some(24));
        if let Some(g) = glob.filter(|g| !g.trim().is_empty()) {
            let mut ov = ignore::overrides::OverrideBuilder::new(&ws);
            if ov.add(&g).is_ok() {
                if let Ok(o) = ov.build() {
                    builder.overrides(o);
                }
            }
        }
        let needle = if case_sensitive {
            query.clone()
        } else {
            query.to_lowercase()
        };
        let mut hits = Vec::new();
        for entry in builder.build().flatten() {
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            if entry.metadata().map(|m| m.len() > 2_000_000).unwrap_or(true) {
                continue;
            }
            let content = match std::fs::read_to_string(entry.path()) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for (i, line) in content.lines().enumerate() {
                let matched = if case_sensitive {
                    line.contains(&needle)
                } else {
                    line.to_lowercase().contains(&needle)
                };
                if matched {
                    hits.push(SearchHit {
                        path: entry.path().to_string_lossy().to_string(),
                        line: (i + 1) as u64,
                        text: crate::util::one_line(line, 240),
                    });
                    if hits.len() >= max {
                        return Ok(hits);
                    }
                }
            }
        }
        Ok(hits)
    })
    .await
}

#[tauri::command]
pub async fn shell_run(
    state: State<'_, AppState>,
    command: String,
    cwd: Option<String>,
) -> Result<String, String> {
    let dir = match cwd.filter(|c| !c.trim().is_empty()) {
        Some(c) => PathBuf::from(c),
        None => state.workspace()?,
    };
    crate::agent::tools::run_shell(&command, &dir, 300).await
}

// ---------------------------------------------------------------------------
// language mode + running files
// ---------------------------------------------------------------------------

/// Pin a file to a language. An empty language drops the override, so the
/// extension-based guess applies again.
#[tauri::command]
pub fn language_set(state: State<'_, AppState>, path: String, language: String) -> Result<(), String> {
    let path = crate::util::plain_path(&path);
    {
        let mut cfg = crate::util::lock(&state.config);
        if language.trim().is_empty() {
            cfg.file_languages.remove(&path);
        } else {
            cfg.file_languages.insert(path, language.trim().to_string());
        }
    }
    state.save()
}

#[tauri::command]
pub fn runner_options(lang: String) -> Vec<crate::runner::RunnerOption> {
    crate::runner::runner_options(&lang)
}

/// Try the interpreters for a language and report which ones exist. The
/// settings panel uses this to show "未安装" instead of letting a run fail.
#[tauri::command]
pub fn runner_detect(lang: String) -> Vec<crate::runner::RunnerOption> {
    crate::runner::runner_options(&lang)
}

#[tauri::command]
pub fn runner_save(
    state: State<'_, AppState>,
    lang: String,
    command: String,
    args: String,
) -> Result<(), String> {
    {
        let mut cfg = crate::util::lock(&state.config);
        let entry = cfg.runners.entry(lang).or_default();
        entry.command = command;
        entry.args = args;
    }
    state.save()
}

#[tauri::command]
pub fn runner_reset(state: State<'_, AppState>, lang: String) -> Result<(), String> {
    {
        let mut cfg = crate::util::lock(&state.config);
        cfg.runners.remove(&lang);
    }
    state.save()
}

/// Start the active file. Output is streamed as `run://event`.
#[tauri::command]
pub fn file_run(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    runner: State<'_, std::sync::Arc<crate::runner::RunnerService>>,
    path: String,
    language: Option<String>,
    command: Option<String>,
    args: Option<String>,
) -> Result<crate::runner::RunInfo, String> {
    let cfg = crate::util::lock(&state.config).clone();
    runner.start(
        app,
        &cfg,
        &path,
        language.as_deref().unwrap_or(""),
        command,
        args,
    )
}

#[tauri::command]
pub fn file_run_stop(
    runner: State<'_, std::sync::Arc<crate::runner::RunnerService>>,
    id: String,
) -> Result<(), String> {
    runner.stop(&id)
}

/// Run a command typed into the output panel.
///
/// This is `run_shell` for humans, not for the agent: the process is streamed
/// as `run://event` instead of being captured whole, so a two-minute
/// `gradle build` shows its progress line by line, and the stop button works on
/// it. The command itself goes through the same runner as F5, which is what
/// teaches it about `gradlew.bat` and `.cmd` wrappers.
#[tauri::command]
pub fn shell_stream(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    runner: State<'_, std::sync::Arc<crate::runner::RunnerService>>,
    command: String,
    cwd: Option<String>,
) -> Result<crate::runner::RunInfo, String> {
    let cfg = crate::util::lock(&state.config).clone();
    let dir = cwd
        .filter(|c| !c.trim().is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            cfg.workspace
                .clone()
                .filter(|w| !w.trim().is_empty())
                .map(std::path::PathBuf::from)
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    runner.run_line(&app, &command, &dir, Vec::new())
}

// ---------------------------------------------------------------------------
// git
// ---------------------------------------------------------------------------

/// Run a blocking job on the thread pool.
///
/// Tauri executes plain `fn` commands **on the main thread** — the same one that
/// pumps IPC and window events. Anything that shells out to git or walks the
/// filesystem therefore freezes the whole window while it runs, which is what
/// made the app look "dead" on large workspaces. Every command below that can
/// outlive a frame is `async` and hops through here.
async fn blocking<T, F>(job: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|e| format!("后台任务失败：{e}"))?
}

fn require_repo(state: &AppState) -> Result<PathBuf, String> {
    let ws = state.workspace()?;
    if !crate::git::is_repo(&ws) {
        return Err("当前工作区不是 git 仓库".into());
    }
    Ok(ws)
}

#[tauri::command]
pub fn git_is_repo(state: State<'_, AppState>) -> bool {
    state
        .workspace()
        .map(|w| crate::git::is_repo(&w))
        .unwrap_or(false)
}

#[tauri::command]
pub async fn git_status(state: State<'_, AppState>) -> Result<Vec<GitFileStatus>, String> {
    let ws = require_repo(&state)?;
    blocking(move || crate::git::status(&ws)).await
}

#[tauri::command]
pub async fn git_diff(
    state: State<'_, AppState>,
    staged: bool,
    path: Option<String>,
) -> Result<String, String> {
    let ws = require_repo(&state)?;
    blocking(move || crate::git::diff(&ws, staged, path.as_deref())).await
}

#[tauri::command]
pub async fn git_stage(state: State<'_, AppState>, paths: Vec<String>) -> Result<String, String> {
    let ws = require_repo(&state)?;
    blocking(move || crate::git::stage(&ws, &paths)).await
}

#[tauri::command]
pub async fn git_unstage(state: State<'_, AppState>, paths: Vec<String>) -> Result<String, String> {
    let ws = require_repo(&state)?;
    blocking(move || crate::git::unstage(&ws, &paths)).await
}

#[tauri::command]
pub async fn git_commit(state: State<'_, AppState>, message: String) -> Result<String, String> {
    let ws = require_repo(&state)?;
    blocking(move || crate::git::commit(&ws, &message)).await
}

#[tauri::command]
pub async fn git_branch(state: State<'_, AppState>) -> Result<String, String> {
    let ws = require_repo(&state)?;
    blocking(move || crate::git::current_branch(&ws)).await
}

#[tauri::command]
pub async fn git_branches(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let ws = require_repo(&state)?;
    blocking(move || crate::git::branches(&ws)).await
}

#[tauri::command]
pub async fn git_checkout(state: State<'_, AppState>, branch: String) -> Result<String, String> {
    let ws = require_repo(&state)?;
    blocking(move || crate::git::checkout(&ws, &branch)).await
}

#[tauri::command]
pub async fn git_log(state: State<'_, AppState>, limit: Option<usize>) -> Result<String, String> {
    let ws = require_repo(&state)?;
    blocking(move || crate::git::log(&ws, limit.unwrap_or(20))).await
}

#[tauri::command]
pub async fn git_init(state: State<'_, AppState>) -> Result<String, String> {
    let ws = state.workspace()?;
    blocking(move || crate::git::init(&ws)).await
}

#[tauri::command]
pub async fn git_push(state: State<'_, AppState>) -> Result<String, String> {
    let ws = require_repo(&state)?;
    blocking(move || crate::git::push(&ws)).await
}

#[tauri::command]
pub async fn git_pull(state: State<'_, AppState>) -> Result<String, String> {
    let ws = require_repo(&state)?;
    blocking(move || crate::git::pull(&ws)).await
}

// ---------------------------------------------------------------------------
// workspace statistics
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct ExtStat {
    pub ext: String,
    pub files: u64,
    pub loc: u64,
    pub bytes: u64,
}

#[derive(Serialize)]
pub struct FileStat {
    pub path: String,
    pub bytes: u64,
    pub lines: u64,
}

#[derive(Serialize)]
pub struct WorkspaceStats {
    pub root: String,
    pub files: u64,
    pub dirs: u64,
    pub total_bytes: u64,
    pub total_loc: u64,
    pub by_ext: Vec<ExtStat>,
    pub largest: Vec<FileStat>,
    pub scanned_at: u64,
}

#[tauri::command]
pub async fn workspace_stats(state: State<'_, AppState>) -> Result<WorkspaceStats, String> {
    let root = state.workspace()?;
    blocking(move || scan_workspace(root)).await
}

/// The actual scan. Every file under 1.5 MB is read once to count lines, so
/// this has to stay off the main thread — see `blocking()`.
fn scan_workspace(root: PathBuf) -> Result<WorkspaceStats, String> {
    let mut builder = walker(&root);
    builder.max_depth(Some(32));
    let mut files = 0u64;
    let mut dirs = 0u64;
    let mut total_bytes = 0u64;
    let mut total_loc = 0u64;
    let mut per_ext: std::collections::HashMap<String, (u64, u64, u64)> = Default::default();
    let mut largest: Vec<FileStat> = Vec::new();
    for entry in builder.build().flatten() {
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            dirs += 1;
            continue;
        }
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        files += 1;
        total_bytes += meta.len();
        let ext = entry
            .path()
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let mut loc = 0u64;
        if meta.len() < 1_500_000 {
            if let Ok(bytes) = std::fs::read(entry.path()) {
                if !bytes.iter().take(2048).any(|b| *b == 0) {
                    loc = bytes.iter().filter(|b| **b == b'\n').count() as u64;
                }
            }
        }
        total_loc += loc;
        let slot = per_ext.entry(ext).or_insert((0, 0, 0));
        slot.0 += 1;
        slot.1 += loc;
        slot.2 += meta.len();
        largest.push(FileStat {
            path: entry.path().to_string_lossy().to_string(),
            bytes: meta.len(),
            lines: loc,
        });
    }
    largest.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    largest.truncate(12);
    let mut by_ext: Vec<ExtStat> = per_ext
        .into_iter()
        .map(|(ext, (f, l, b))| ExtStat {
            ext: if ext.is_empty() {
                "(无扩展名)".into()
            } else {
                ext
            },
            files: f,
            loc: l,
            bytes: b,
        })
        .collect();
    by_ext.sort_by(|a, b| b.loc.cmp(&a.loc));

    Ok(WorkspaceStats {
        root: root.to_string_lossy().to_string(),
        files,
        dirs,
        total_bytes,
        total_loc,
        by_ext,
        largest,
        scanned_at: crate::util::now_ms(),
    })
}

// ---------------------------------------------------------------------------
// MCP
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct McpServerView {
    pub config: McpServerConfig,
    pub running: bool,
    pub tools: usize,
    pub stderr: Vec<String>,
}

#[tauri::command]
pub fn mcp_servers(state: State<'_, AppState>) -> Vec<McpServerView> {
    let cfg = crate::util::lock(&state.config);
    let running = state.mcp.running();
    let tools = state.mcp.tools_all();
    cfg.mcp_servers
        .iter()
        .map(|s| McpServerView {
            config: s.clone(),
            running: running.contains(&s.id),
            tools: tools.iter().filter(|t| t.server_id == s.id).count(),
            stderr: state.mcp.stderr_tail(&s.id),
        })
        .collect()
}

#[tauri::command]
pub fn mcp_upsert(
    state: State<'_, AppState>,
    server: McpServerConfig,
) -> Result<Vec<McpServerConfig>, String> {
    {
        let mut cfg = crate::util::lock(&state.config);
        match cfg.mcp_servers.iter_mut().find(|s| s.id == server.id) {
            Some(existing) => *existing = server,
            None => cfg.mcp_servers.push(server),
        }
    }
    state.save()?;
    Ok(crate::util::lock(&state.config).mcp_servers.clone())
}

#[tauri::command]
pub async fn mcp_delete(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<McpServerConfig>, String> {
    state.mcp.stop(&id).await;
    {
        let mut cfg = crate::util::lock(&state.config);
        cfg.mcp_servers.retain(|s| s.id != id);
    }
    state.save()?;
    Ok(crate::util::lock(&state.config).mcp_servers.clone())
}

#[tauri::command]
pub async fn mcp_start(state: State<'_, AppState>, id: String) -> Result<Vec<McpToolInfo>, String> {
    let server = {
        let cfg = crate::util::lock(&state.config);
        cfg.mcp_servers
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .ok_or_else(|| format!("unknown MCP server '{id}'"))?
    };
    state.mcp.start(&server).await
}

#[tauri::command]
pub async fn mcp_stop(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.mcp.stop(&id).await;
    Ok(())
}

#[tauri::command]
pub fn mcp_tools(state: State<'_, AppState>) -> Vec<McpToolInfo> {
    state.mcp.tools_all()
}

// ---------------------------------------------------------------------------
// skills
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn skills_list(skills: State<'_, Arc<SkillRegistry>>) -> Vec<Skill> {
    skills.list()
}

#[tauri::command]
pub async fn skills_install(
    skills: State<'_, Arc<SkillRegistry>>,
    source: String,
) -> Result<Skill, String> {
    let skill = skills.install(&source).await?;
    skills.set_enabled(&skill.id, true);
    Ok(skill)
}

#[tauri::command]
pub fn skills_uninstall(skills: State<'_, Arc<SkillRegistry>>, id: String) -> Result<(), String> {
    skills.uninstall(&id)
}

#[tauri::command]
pub fn skills_toggle(
    state: State<'_, AppState>,
    skills: State<'_, Arc<SkillRegistry>>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    if !skills.set_enabled(&id, enabled) {
        return Err(format!("skill '{id}' not found"));
    }
    {
        let mut cfg = crate::util::lock(&state.config);
        cfg.skills.enabled.retain(|e| e != &id);
        if enabled {
            cfg.skills.enabled.push(id);
        }
    }
    state.save()
}

#[tauri::command]
pub fn skills_read(skills: State<'_, Arc<SkillRegistry>>, id: String) -> Result<String, String> {
    skills
        .get(&id)
        .map(|s| {
            format!(
                "---\nname: {}\ndescription: {}\nwhen: {}\n---\n\n{}",
                s.name,
                s.description,
                s.when.join(", "),
                s.body
            )
        })
        .ok_or_else(|| format!("skill '{id}' not found"))
}

// ---------------------------------------------------------------------------
// window
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn main_loaded(app: tauri::AppHandle) {
    crate::mark_main_loaded(&app);
}

#[tauri::command]
pub fn window_control(app: tauri::AppHandle, action: String) -> Result<(), String> {
    let win = app.get_webview_window("main").ok_or("main window missing")?;
    match action.as_str() {
        "minimize" => win.minimize().map_err(|e| e.to_string()),
        "maximize" => {
            let is_max = win.is_maximized().unwrap_or(false);
            if is_max {
                win.unmaximize().map_err(|e| e.to_string())
            } else {
                win.maximize().map_err(|e| e.to_string())
            }
        }
        "close" => win.close().map_err(|e| e.to_string()),
        other => Err(format!("unknown window action '{other}'")),
    }
}

// ---------------------------------------------------------------------------
// web2api bridge
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn web2api_status(bridge: State<'_, Arc<crate::bridge::Bridge>>) -> crate::bridge::BridgeStatus {
    bridge.status()
}

/// Open the vendor's own site in a persistent WebView so the user can log in
/// once; browser-mode sources then send requests from inside that page.
/// Closing that window hides it — the login state is kept.
#[tauri::command]
pub async fn web2api_open_session(
    state: State<'_, AppState>,
    source_id: String,
    home_url: String,
) -> Result<(), String> {
    {
        let cfg = crate::util::lock(&state.config);
        crate::websession::sync_sources(&cfg.bridge);
    }
    crate::websession::open(&source_id, &home_url, true).await
}

/// Probe every configured web source with a real minimal request (or, for the
/// page driver, with a login check). Rows carry only the provider label in the
/// panel; the diagnostic text stays in the tooltip.
#[tauri::command]
pub async fn web2api_test(
    state: State<'_, AppState>,
    sources: Vec<WebSource>,
) -> Result<Vec<crate::bridge::WebTestRow>, String> {
    let proxy = crate::util::lock(&state.config).proxy.clone();
    Ok(crate::bridge::test_sources(sources, &proxy).await)
}

/// Login state for every reverse proxy. The panel uses this to show which
/// sources are usable and the composer uses it to hide unlogged-in models.
#[tauri::command]
pub fn web2api_session_status(
    state: State<'_, AppState>,
) -> Vec<crate::websession::SessionInfo> {
    let cfg = crate::util::lock(&state.config);
    crate::websession::sync_sources(&cfg.bridge);
    crate::websession::session_infos(&cfg.bridge)
}

/// Explicit "获取登录信息": re-run the login probe on every open session now,
/// instead of waiting for the background watcher's next tick.
#[tauri::command]
pub async fn web2api_probe(
    state: State<'_, AppState>,
) -> Result<Vec<crate::websession::SessionInfo>, String> {
    let bridge = crate::util::lock(&state.config).bridge.clone();
    crate::websession::sync_sources(&bridge);
    for s in &bridge.sources {
        if crate::websession::is_open(&s.id) {
            let _ = crate::websession::probe_login_with(&s.id, &s.login_endpoint).await;
        }
    }
    Ok(crate::websession::session_infos(&bridge))
}

/// Hide (do not destroy) a session window. The cookie jar survives, so the
/// next request needs no "打开官网" click.
#[tauri::command]
pub fn web2api_hide_session(source_id: String) -> bool {
    crate::websession::hide(&source_id)
}

#[tauri::command]
pub async fn web2api_start(
    state: State<'_, AppState>,
    bridge: State<'_, Arc<crate::bridge::Bridge>>,
) -> Result<crate::bridge::BridgeStatus, String> {
    // Own the Arc before awaiting: the command may outlive the State borrow.
    let b = bridge.inner().clone();
    match b.start().await {
        Ok(_) => {
            // Starting the bridge also publishes it to the agent: the built-in
            // Web2API provider becomes selectable and its models show up in
            // the composer's model picker.
            {
                let mut cfg = crate::util::lock(&state.config);
                cfg.set_web2api_enabled(true);
            }
            state.save()?;
            // A click on "启动" is an explicit opt-in, so it is allowed even
            // when "start on launch" is off. Start the login watcher here
            // (idempotent) rather than unconditionally at boot.
            let bridge_cfg = {
                let cfg = crate::util::lock(&state.config);
                cfg.bridge.clone()
            };
            crate::websession::start_watcher();
            if bridge_cfg.auto_ensure {
                // Hidden windows for the sources the user selected; no manual
                // "打开官网" round-trip.
                crate::websession::ensure_sessions(&bridge_cfg, None).await;
            }
            Ok(b.status())
        }
        Err(e) => {
            b.fail(e.clone());
            Err(e)
        }
    }
}

/// Picking a Web2API model in the composer calls this: start the endpoint if
/// it is down, publish the provider, and open only the reverse proxy that
/// model routes to — all in the background, no manual steps for the user.
#[tauri::command]
pub async fn web2api_ensure(
    state: State<'_, AppState>,
    bridge: State<'_, Arc<crate::bridge::Bridge>>,
    model: Option<String>,
) -> Result<crate::bridge::BridgeStatus, String> {
    let b = bridge.inner().clone();
    if !b.status().running {
        b.start().await.map_err(|e| {
            b.fail(e.clone());
            e
        })?;
    }
    let cfg = {
        let mut cfg = crate::util::lock(&state.config);
        cfg.set_web2api_enabled(true);
        cfg.bridge.clone()
    };
    state.save()?;
    if cfg.auto_ensure {
        // Picking a Web2API model is a manual action too: it may be the first
        // thing that ever starts the bridge when auto-start is disabled.
        crate::websession::start_watcher();
        crate::websession::ensure_sessions(&cfg, model.as_deref()).await;
    }
    Ok(b.status())
}

#[tauri::command]
pub fn web2api_stop(
    state: State<'_, AppState>,
    bridge: State<'_, Arc<crate::bridge::Bridge>>,
) -> crate::bridge::BridgeStatus {
    let status = bridge.stop();
    {
        let mut cfg = crate::util::lock(&state.config);
        cfg.set_web2api_enabled(false);
    }
    let _ = state.save();
    status
}

#[tauri::command]
pub fn web2api_log(bridge: State<'_, Arc<crate::bridge::Bridge>>) -> Vec<crate::bridge::LogRow> {
    bridge.log()
}

// ---------------------------------------------------------------------------
// extensions (VS Code / Open VSX / DSH plugins)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn extensions_list() -> Vec<crate::extensions::ExtensionInfo> {
    crate::extensions::list()
}

/// Install from a `.vsix` path, a `.vsix` URL, `publisher.name[@version]`
/// (Open VSX) or `github:owner/repo` (the shape DSH plugins ship in).
#[tauri::command]
pub async fn extensions_install(
    state: State<'_, AppState>,
    skills: State<'_, Arc<SkillRegistry>>,
    source: String,
) -> Result<crate::extensions::ExtensionInfo, String> {
    let proxy = crate::util::lock(&state.config).proxy.clone();
    let info = crate::extensions::install(&source, &proxy).await?;
    if !info.contributes.skills.is_empty() {
        let registry: &SkillRegistry = &skills;
        crate::extensions::import_skills(&info, registry);
    }
    Ok(info)
}

#[tauri::command]
pub fn extensions_uninstall(id: String) -> Result<(), String> {
    crate::extensions::uninstall(&id)
}

#[tauri::command]
pub fn extensions_toggle(
    id: String,
    enabled: bool,
) -> Result<crate::extensions::ExtensionInfo, String> {
    crate::extensions::set_enabled(&id, enabled)
}

#[tauri::command]
pub async fn extensions_search(
    state: State<'_, AppState>,
    query: String,
    category: Option<String>,
    sort: Option<String>,
    offset: Option<usize>,
) -> Result<Vec<crate::extensions::ExtSearchRow>, String> {
    let proxy = crate::util::lock(&state.config).proxy.clone();
    crate::extensions::search(
        &query,
        category.as_deref().unwrap_or(""),
        sort.as_deref().unwrap_or("downloads"),
        offset.unwrap_or(0),
        &proxy,
    )
    .await
}

/// Advisory version check for installed extensions; never fails the UI.
#[tauri::command]
pub async fn extensions_check_updates(
    state: State<'_, AppState>,
    force: Option<bool>,
) -> Result<Vec<crate::extensions::ExtUpdate>, String> {
    // Tauri requires a `Result` from async commands holding borrowed inputs.
    let proxy = crate::util::lock(&state.config).proxy.clone();
    Ok(crate::extensions::check_updates(force.unwrap_or(false), &proxy).await)
}

#[tauri::command]
pub async fn extensions_detail(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::extensions::ExtDetail, String> {
    let proxy = crate::util::lock(&state.config).proxy.clone();
    crate::extensions::detail(&id, &proxy).await
}

/// Local, offline detail for an extension that is already installed. Distinct
/// from `extensions_detail`, which is an Open VSX store lookup.
#[tauri::command]
pub fn extensions_inspect(id: String) -> Result<crate::extensions::ExtInspect, String> {
    crate::extensions::inspect(&id)
}

/// Ask GitHub Releases whether a newer version exists. Read-only: the app
/// never downloads or installs an update by itself.
#[tauri::command]
pub async fn app_check_update(
    state: State<'_, AppState>,
    force: Option<bool>,
) -> Result<crate::update::AppUpdate, String> {
    let proxy = crate::util::lock(&state.config).proxy.clone();
    crate::update::check(&proxy, force.unwrap_or(false)).await
}

#[tauri::command]
pub fn extensions_theme(id: String, label: String) -> Result<serde_json::Value, String> {
    crate::extensions::theme_colors(&id, &label)
}

#[tauri::command]
pub fn extensions_snippets(language: String) -> Vec<crate::extensions::SnippetRow> {
    crate::extensions::snippets_for(&language)
}

// ---------------------------------------------------------------------------
// extension host (Node)
// ---------------------------------------------------------------------------

/// Which `node` binary the host would use, and whether one exists at all.
#[tauri::command]
pub fn exthost_node(host: State<'_, Arc<crate::exthost::ExtHost>>) -> crate::exthost::NodeInfo {
    host.node()
}

/// Download a portable Node build into `<config>/runtime/node`.
#[tauri::command]
pub async fn exthost_install_node(
    state: State<'_, AppState>,
) -> Result<crate::exthost::NodeInfo, String> {
    let proxy = crate::util::lock(&state.config).proxy.clone();
    crate::exthost::install_node(&proxy).await
}

#[tauri::command]
pub fn exthost_status(
    host: State<'_, Arc<crate::exthost::ExtHost>>,
) -> Vec<crate::exthost::HostStatus> {
    host.status()
}

#[tauri::command]
pub async fn exthost_start(
    host: State<'_, Arc<crate::exthost::ExtHost>>,
    id: String,
) -> Result<crate::exthost::HostStatus, String> {
    let host = host.inner().clone();
    let info = crate::extensions::find(&id).ok_or_else(|| format!("未安装扩展 {id}"))?;
    if !crate::util::lock(&host.config_snapshot()).extensions.node_host {
        return Err("设置里关闭了 Node 扩展宿主".into());
    }
    tauri::async_runtime::spawn_blocking(move || crate::exthost::start_blocking(&host, &info))
        .await
        .map_err(|e| format!("启动失败：{e}"))?
}

#[tauri::command]
pub async fn exthost_stop(
    host: State<'_, Arc<crate::exthost::ExtHost>>,
    id: String,
) -> Result<(), String> {
    let host = host.inner().clone();
    tauri::async_runtime::spawn_blocking(move || host.stop(&id))
        .await
        .map_err(|e| format!("停止失败：{e}"))?
}

#[tauri::command]
pub fn exthost_stop_all(host: State<'_, Arc<crate::exthost::ExtHost>>) {
    host.stop_all();
}

/// The "restart" offered by the problem dialog. Dropping every host also
/// disposes its webview panels, terminals and providers, then the same
/// autostart sequence used at boot brings the enabled extensions back. Doing
/// both on the Rust side avoids a renderer race between `stop` and a burst of
/// individual `start` invokes.
#[tauri::command]
pub fn exthost_restart_all(host: State<'_, Arc<crate::exthost::ExtHost>>) {
    host.stop_all();
    host.autostart();
}

/// The "restart this plugin" action in the plugin-fault dialog. `stop()` marks
/// the host `stopping` immediately but the child exits asynchronously; wait for
/// it before starting a replacement, otherwise the old reader thread can race
/// the new host's registration.
#[tauri::command]
pub async fn exthost_restart(
    host: State<'_, Arc<crate::exthost::ExtHost>>,
    id: String,
) -> Result<crate::exthost::HostStatus, String> {
    let handle = host.inner().clone();
    let info = crate::extensions::find(&id).ok_or_else(|| format!("未安装扩展 {id}"))?;
    tauri::async_runtime::spawn_blocking(move || {
        handle.stop(&id)?;
        for _ in 0..40 {
            let state = handle
                .status_of(&id)
                .map(|s| s.state.clone())
                .unwrap_or_default();
            if state == "stopped" || state.is_empty() || state == "crashed" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        handle.start(&info)
    })
    .await
    .map_err(|e| format!("重启失败：{e}"))?
}

#[tauri::command]
pub fn exthost_logs(
    host: State<'_, Arc<crate::exthost::ExtHost>>,
    id: Option<String>,
) -> Vec<crate::exthost::HostLog> {
    host.logs(id.as_deref())
}

#[tauri::command]
pub fn exthost_clear_logs(host: State<'_, Arc<crate::exthost::ExtHost>>, id: String) {
    host.clear_logs(&id);
}

#[tauri::command]
pub fn exthost_commands(
    host: State<'_, Arc<crate::exthost::ExtHost>>,
) -> Vec<crate::exthost::RuntimeCommand> {
    host.commands()
}

#[tauri::command]
pub fn exthost_statusbar(
    host: State<'_, Arc<crate::exthost::ExtHost>>,
) -> Vec<crate::exthost::StatusItem> {
    host.statusbar()
}

#[tauri::command]
pub async fn exthost_run(
    host: State<'_, Arc<crate::exthost::ExtHost>>,
    command: String,
) -> Result<serde_json::Value, String> {
    let host = host.inner().clone();
    tauri::async_runtime::spawn_blocking(move || host.run_command(&command, Vec::new()))
        .await
        .map_err(|e| format!("执行失败：{e}"))?
}

/// Ask extensions for a language feature (completion / hover / format).
///
/// Returns one entry per provider that answered: `[{ ext, value }]`. The
/// renderer merges them, because only it knows whether a completion list should
/// be concatenated and a format result applied.
#[tauri::command]
pub fn ext_language(
    host: State<'_, Arc<crate::exthost::ExtHost>>,
    kind: String,
    path: String,
    language: String,
    text: String,
    line: Option<u32>,
    character: Option<u32>,
    options: Option<serde_json::Value>,
) -> Vec<serde_json::Value> {
    let doc = serde_json::json!({
        "path": path,
        "text": text,
        "languageId": language,
        "version": 1,
    });
    let mut extra = serde_json::Map::new();
    if let (Some(l), Some(c)) = (line, character) {
        extra.insert("position".into(), serde_json::json!({ "line": l, "character": c }));
    }
    if let Some(o) = options {
        extra.insert("options".into(), o);
    }
    host.language_call(&kind, &language, doc, serde_json::Value::Object(extra))
}

#[tauri::command]
pub fn ext_providers(host: State<'_, Arc<crate::exthost::ExtHost>>) -> Vec<crate::exthost::ProviderRow> {
    host.providers()
}

#[tauri::command]
pub fn webview_post(
    host: State<'_, Arc<crate::exthost::ExtHost>>,
    id: String,
    message: serde_json::Value,
) -> Result<(), String> {
    host.webview_post(&id, message)
}

#[tauri::command]
pub fn webview_close(host: State<'_, Arc<crate::exthost::ExtHost>>, id: String) {
    host.webview_dispose(&id);
}

/// Publish a panel document to the `irixen-wv` scheme and return the URL the
/// iframe should load.
///
/// The renderer sends the final HTML (bridge injected, resources rewritten);
/// the scheme then serves it as a real document with its own origin, so the
/// panel behaves like a VS Code webview instead of a sandboxed `srcdoc`.
static WEBVIEW_PUBLISH_SEQ: AtomicU64 = AtomicU64::new(0);

#[tauri::command]
pub fn webview_publish(id: String, html: String, user_agent: Option<String>) -> String {
    crate::wvproto::put(&id, &html);
    let base = crate::wvproto::base_for(user_agent.as_deref().unwrap_or(""));
    // The version tag makes a republish actually reload: assigning the same
    // `src` to an iframe is a no-op, so a panel refresh would otherwise keep
    // showing the old document. Two publishes can land in the same millisecond
    // (create + first update), so the counter guarantees uniqueness.
    let seq = WEBVIEW_PUBLISH_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{base}/{id}?v={}-{}", crate::util::now_ms(), seq)
}

#[tauri::command]
pub fn webview_forget(id: String) {
    crate::wvproto::remove(&id);
}

/// The origin panels are served from, so the renderer can rewrite resources
/// without hard-coding a Windows-only URL shape.
#[tauri::command]
pub fn webview_base(user_agent: Option<String>) -> String {
    crate::wvproto::base_for(user_agent.as_deref().unwrap_or(""))
}

/// What Live Preview's static server actually returned for a preview URL.
///
/// The preview iframe is cross-origin from both the IDE and the panel document,
/// so neither can read its DOM. A blank rectangle is therefore indistinguishable
/// from "page rendered nothing" without asking the server ourselves. This is
/// also how we spot the common Vite case: `index.html` references
/// `/src/main.ts`, which a static server hands out as `video/mp2t`, so Chromium
/// refuses the module script and the body stays empty.
#[derive(Serialize)]
pub struct PreviewProbe {
    /// ok | bundler | unreachable | http | not_html
    pub reason: String,
    pub status: u16,
    /// A reachable Vite dev server for the same path, if one is running.
    pub dev_url: String,
}

#[tauri::command]
pub async fn preview_probe(url: String) -> PreviewProbe {
    let bad = |reason: &str, status: u16| PreviewProbe {
        reason: reason.to_string(),
        status,
        dev_url: String::new(),
    };
    // Loopback only. This command reads arbitrary URLs on behalf of the panel;
    // allowing off-host targets would turn a preview into an SSRF primitive.
    let parsed = match url::Url::parse(&url) {
        Ok(u) => u,
        Err(_) => return bad("unreachable", 0),
    };
    let host = parsed.host_str().unwrap_or("");
    if parsed.scheme() != "http" || !matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1") {
        return bad("unreachable", 0);
    }
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(_) => return bad("unreachable", 0),
    };
    let resp = match client.get(parsed.as_str()).send().await {
        Ok(r) => r,
        Err(_) => return bad("unreachable", 0),
    };
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        return bad("http", status);
    }
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !ctype.contains("html") && !ctype.is_empty() {
        return bad("not_html", status);
    }
    let body = resp.text().await.unwrap_or_default();
    let bundler = body.contains("type=\"module\"")
        && (body.contains("/@vite/client")
            || body.contains("/src/main.")
            || body.contains("/src/index.")
            || body.contains("/src/main\""));

    // A Vite dev server serves the same entry point with the right MIME types.
    // Only offer the switch when one is genuinely listening; the button is
    // explicit so we never silently navigate a preview the user chose.
    let mut dev_url = String::new();
    if bundler {
        for port in [5173_u16, 5174] {
            // A static file server answers 200 for any path; require the Vite
            // client module (served as JavaScript) before offering the switch.
            let marker = client
                .get(format!("http://127.0.0.1:{port}/@vite/client"))
                .send()
                .await;
            let is_vite = match marker {
                Ok(r) => {
                    let js = r
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .contains("javascript");
                    js && r.status().is_success()
                }
                Err(_) => false,
            };
            if !is_vite {
                continue;
            }
            dev_url = format!(
                "http://127.0.0.1:{}{}{}",
                port,
                parsed.path(),
                parsed.query().map(|q| format!("?{q}")).unwrap_or_default()
            );
            break;
        }
    }
    PreviewProbe {
        reason: if bundler { "bundler" } else { "ok" }.to_string(),
        status,
        dev_url,
    }
}

#[tauri::command]
pub fn webview_list(host: State<'_, Arc<crate::exthost::ExtHost>>) -> Vec<serde_json::Value> {
    host.webviews()
}

/// Contributed sidebar views: which of them have a live tree data provider.
#[tauri::command]
pub fn exttree_views(host: State<'_, Arc<crate::exthost::ExtHost>>) -> Vec<String> {
    host.tree_views()
}

/// Children of a root (`key: null`) or of a previously returned node.
/// `getChildren` may import a Gradle project, so this runs off the UI thread.
#[tauri::command]
pub async fn exttree_children(
    host: State<'_, Arc<crate::exthost::ExtHost>>,
    view: String,
    key: Option<String>,
) -> Result<serde_json::Value, String> {
    let host = host.inner().clone();
    tauri::async_runtime::spawn_blocking(move || host.tree_children(&view, key))
        .await
        .map_err(|e| format!("读取侧栏数据失败：{e}"))?
}

/// Run a `TreeItem.command`, or a `view/item/context` command for that node.
#[tauri::command]
pub async fn exttree_command(
    host: State<'_, Arc<crate::exthost::ExtHost>>,
    view: String,
    key: String,
    command: Option<String>,
) -> Result<serde_json::Value, String> {
    let host = host.inner().clone();
    tauri::async_runtime::spawn_blocking(move || host.tree_command(&view, &key, command))
        .await
        .map_err(|e| format!("执行侧栏命令失败：{e}"))?
}

/// Drop the provider's element cache before re-reading a view.
#[tauri::command]
pub async fn exttree_refresh(
    host: State<'_, Arc<crate::exthost::ExtHost>>,
    view: String,
) -> Result<serde_json::Value, String> {
    let host = host.inner().clone();
    tauri::async_runtime::spawn_blocking(move || host.tree_refresh(&view))
        .await
        .map_err(|e| format!("刷新侧栏失败：{e}"))?
}

/// The renderer answered a `window.show*` prompt from an extension.
#[tauri::command]
pub fn exthost_reply(
    host: State<'_, Arc<crate::exthost::ExtHost>>,
    id: u64,
    value: Option<serde_json::Value>,
) {
    // `null` is a legitimate answer (the user dismissed a prompt). Never let
    // an absent key strand the extension's pending `await`.
    host.reply_ui(id, value.unwrap_or(serde_json::Value::Null));
}

/// Editor document state, mirrored into every running host so
/// `window.activeTextEditor` is real.
#[tauri::command]
pub fn exthost_doc(
    host: State<'_, Arc<crate::exthost::ExtHost>>,
    kind: String,
    path: String,
    language: String,
    text: String,
    dirty: bool,
    version: u64,
    selection: Option<serde_json::Value>,
) {
    let doc = serde_json::json!({
        "path": path,
        "languageId": language,
        "text": text,
        "dirty": dirty,
        "version": version,
        "selection": selection.unwrap_or_else(|| serde_json::json!({
            "start": { "line": 0, "character": 0 },
            "end": { "line": 0, "character": 0 }
        })),
    });
    host.doc_event(&kind, doc);
}

/// Open a URL in the OS browser (store links, extension `env.openExternal`).
#[tauri::command]
pub fn open_external(url: String) -> Result<(), String> {
    crate::util::open_external(&url)
}

// ---------------------------------------------------------------------------
// terminal (real PTY)
// ---------------------------------------------------------------------------

/// Shells we can start, in preference order, with availability resolved.
#[tauri::command]
pub fn term_shells(state: State<'_, AppState>) -> Vec<crate::terminal::ShellOption> {
    let wanted = crate::util::lock(&state.config).terminal_shell.clone();
    let mut rows = crate::terminal::shells();
    // Reflect the user's choice: whatever is configured is the one marked as
    // the default, so the picker and the "new terminal" button agree.
    if !wanted.trim().is_empty() {
        let mut hit = false;
        for row in rows.iter_mut() {
            let is = row.id.eq_ignore_ascii_case(wanted.trim())
                || row.program.eq_ignore_ascii_case(wanted.trim())
                || row.program.eq_ignore_ascii_case(&format!("{}.exe", wanted.trim()));
            row.recommended = is;
            hit |= is;
        }
        if !hit {
            rows.insert(
                0,
                crate::terminal::ShellOption {
                    id: "custom".into(),
                    label: wanted.clone(),
                    program: wanted,
                    args: Vec::new(),
                    available: true,
                    recommended: true,
                },
            );
        }
    }
    rows
}

/// Create a shell on a fresh PTY. Output arrives as `term://data` frames.
#[tauri::command]
pub fn term_open(
    app: tauri::AppHandle,
    term: State<'_, Arc<crate::terminal::TermService>>,
    cwd: Option<String>,
    shell: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<crate::terminal::TermInfo, String> {
    term.open(app, cwd, shell, cols.unwrap_or(100), rows.unwrap_or(24))
}

/// Keystrokes, pasted text, control sequences — forwarded verbatim.
#[tauri::command]
pub fn term_write(
    term: State<'_, Arc<crate::terminal::TermService>>,
    id: String,
    data: String,
) -> Result<(), String> {
    term.write(&id, &data)
}

#[tauri::command]
pub fn term_resize(
    term: State<'_, Arc<crate::terminal::TermService>>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    term.resize(&id, cols, rows)
}

#[tauri::command]
pub fn term_close(
    term: State<'_, Arc<crate::terminal::TermService>>,
    id: String,
) -> Result<(), String> {
    term.close(&id)
}

/// Describe a live session (used when the UI attaches to a terminal that an
/// extension created before the panel was open).
#[tauri::command]
pub fn term_attach(
    term: State<'_, Arc<crate::terminal::TermService>>,
    id: String,
) -> Result<crate::terminal::TermView, String> {
    term.attach(&id)
}

#[tauri::command]
pub fn term_list(term: State<'_, Arc<crate::terminal::TermService>>) -> Vec<crate::terminal::TermInfo> {
    term.list()
}
