//! Run the active file.
//!
//! There is no terminal emulator here and no shell pool: one child process per
//! run, stdout/stderr read line by line, and each line emitted to the renderer
//! as `run://event`. The renderer owns the output panel; the core owns the
//! process, the working directory and a stop button that actually kills it.
//!
//! Everything is driven by a *template* (`python -u {file}`) so a language the
//! built-in table does not know can still be run by pointing it at any command.

use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::config::AppConfig;

// ---------------------------------------------------------------------------
// language defaults
// ---------------------------------------------------------------------------

/// `(command, args template)` for a language, before user overrides.
///
/// The tables are deliberately small and honest: the built-in command is one
/// that ships with the runtime people install for that language. Anything else
/// is a user edit in settings, not a hidden guess.
pub fn default_runner(lang: &str) -> Option<(String, String)> {
    let win = cfg!(windows);
    let (cmd, args) = match lang {
        // `-u` keeps Python's stdout unbuffered; otherwise a script that prints
        // and then loops shows nothing until it exits — which looks like a hang.
        "python" => ("python", "-u {file}"),
        "javascript" | "javascriptreact" => ("node", "{file}"),
        "typescript" | "typescriptreact" => ("node", "--experimental-strip-types {file}"),
        "go" => ("go", "run {file}"),
        "rust" => ("cargo", "run --quiet"),
        "java" => ("java", "{file}"),
        "php" => ("php", "{file}"),
        "ruby" => ("ruby", "{file}"),
        "perl" => ("perl", "{file}"),
        "lua" => ("lua", "{file}"),
        "r" => ("Rscript", "{file}"),
        "dart" => ("dart", "run {file}"),
        "deno" => ("deno", "run -A {file}"),
        "shellscript" => {
            if win {
                ("bash", "{file}")
            } else {
                ("sh", "{file}")
            }
        }
        "powershell" | "bat" => {
            if win {
                ("powershell", "-NoProfile -ExecutionPolicy Bypass -File {file}")
            } else {
                ("pwsh", "-NoProfile -File {file}")
            }
        }
        "c" => {
            if win {
                ("gcc", "{file} -o {dir}/.irixen-{stem}.exe && {dir}/.irixen-{stem}.exe")
            } else {
                ("cc", "{file} -o {dir}/.irixen-{stem} && {dir}/.irixen-{stem}")
            }
        }
        "cpp" => {
            if win {
                ("g++", "{file} -o {dir}/.irixen-{stem}.exe && {dir}/.irixen-{stem}.exe")
            } else {
                ("c++", "{file} -o {dir}/.irixen-{stem} && {dir}/.irixen-{stem}")
            }
        }
        _ => return None,
    };
    Some((cmd.to_string(), args.to_string()))
}

/// Candidate interpreters, best first. Used by the "运行器" picker.
fn runner_candidates(lang: &str) -> Vec<(String, String, String)> {
    let win = cfg!(windows);
    let mut out: Vec<(String, String, String)> = Vec::new();
    match lang {
        "python" => {
            if win {
                out.push(("py".into(), "py -3".into(), "-3 -u {file}".into()));
            }
            out.push(("python".into(), "python".into(), "-u {file}".into()));
            out.push(("python3".into(), "python3".into(), "-u {file}".into()));
        }
        "javascript" | "javascriptreact" => {
            out.push(("node".into(), "Node.js".into(), "{file}".into()));
        }
        "typescript" | "typescriptreact" => {
            out.push(("tsx".into(), "tsx".into(), "{file}".into()));
            out.push(("node".into(), "node --experimental-strip-types".into(), "--experimental-strip-types {file}".into()));
        }
        "rust" => {
            out.push(("cargo".into(), "cargo run".into(), "run --quiet".into()));
        }
        _ => {
            if let Some((c, a)) = default_runner(lang) {
                out.push((c.clone(), c, a));
            }
        }
    }
    out
}

#[derive(Serialize, Clone)]
pub struct RunnerOption {
    pub id: String,
    pub label: String,
    pub command: String,
    pub args: String,
    pub available: bool,
}

pub fn runner_options(lang: &str) -> Vec<RunnerOption> {
    let mut rows: Vec<RunnerOption> = runner_candidates(lang)
        .into_iter()
        .map(|(id, label, args)| RunnerOption {
            available: which(&id).is_some(),
            command: id.clone(),
            id,
            label,
            args,
        })
        .collect();
    if rows.is_empty() {
        if let Some((c, a)) = default_runner(lang) {
            rows.push(RunnerOption {
                available: which(&c).is_some(),
                id: c.clone(),
                label: c.clone(),
                command: c,
                args: a,
            });
        }
    }
    rows
}

/// Resolve an executable the way `cmd` would: PATH + PATHEXT on Windows,
/// executable bit on Unix. A command with a path separator is checked directly
/// so `C:\Python312\python.exe` works without being on PATH.
pub fn which(cmd: &str) -> Option<PathBuf> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return None;
    }
    let raw = PathBuf::from(cmd);
    if cmd.contains('/') || cmd.contains('\\') {
        return raw.is_file().then_some(raw);
    }
    let exts: Vec<String> = if cfg!(windows) {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        std::iter::once(String::new())
            .chain(pathext.split(';').map(|s| s.to_ascii_lowercase()))
            .collect()
    } else {
        vec![String::new()]
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let cand = dir.join(format!("{cmd}{ext}"));
            if cand.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(m) = cand.metadata() {
                        if m.permissions().mode() & 0o111 == 0 {
                            continue;
                        }
                    }
                }
                return Some(cand);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// command building
// ---------------------------------------------------------------------------

/// Replace `{file}` / `{stem}` / `{name}` / `{dir}`.
///
/// A placeholder gets quotes unless the template already quoted it, so
/// `python {file}` and `python "{file}"` both produce a single argument for a
/// path with spaces — and a Chinese folder is just a path, not a special case.
fn expand(tpl: &str, vars: &[(&str, String)]) -> String {
    let mut out = tpl.to_string();
    for (key, value) in vars {
        let token = format!("{{{key}}}");
        let mut from = 0usize;
        loop {
            let Some(rel) = out[from..].find(&token) else { break };
            let at = from + rel;
            let quoted = at > 0
                && out[..at].ends_with('"')
                && out[at + token.len()..].starts_with('"');
            let insert = if quoted { value.clone() } else { format!("\"{value}\"") };
            out.replace_range(at..at + token.len(), &insert);
            from = at + insert.len();
        }
    }
    out
}

/// Split a command line into argv, honouring double quotes.
///
/// Only used for the Windows direct-exec path: a Windows path uses `\` as a
/// separator, so there is no backslash escaping to undo — quotes are the only
/// thing that groups an argument.
fn split_command(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in line.chars() {
        match ch {
            '"' => quoted = !quoted,
            ' ' | '\t' if !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Language id from a file name, used when the renderer sends an empty one.
pub fn language_of(path: &str) -> String {
    let ext = Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "py" | "pyw" => "python",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "go" => "go",
        "rs" => "rust",
        "java" => "java",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "php" => "php",
        "rb" => "ruby",
        "pl" => "perl",
        "lua" => "lua",
        "r" => "r",
        "dart" => "dart",
        "sh" | "bash" | "zsh" => "shellscript",
        "ps1" => "powershell",
        "bat" | "cmd" => "bat",
        _ => "plaintext",
    }
    .to_string()
}

// ---------------------------------------------------------------------------
// service
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct RunInfo {
    pub id: String,
    pub command: String,
    pub cwd: String,
    pub language: String,
}

type ProcMap = Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>>>>;
type ExitMap = Arc<Mutex<HashMap<String, Vec<Box<dyn FnOnce(i32) + Send>>>>>;
type DoneSet = Arc<Mutex<std::collections::HashSet<String>>>;

pub struct RunnerService {
    procs: ProcMap,
    exits: ExitMap,
    finished: DoneSet,
}

impl Default for RunnerService {
    fn default() -> Self {
        RunnerService {
            procs: Arc::new(Mutex::new(HashMap::new())),
            exits: Arc::new(Mutex::new(HashMap::new())),
            finished: Arc::new(Mutex::new(std::collections::HashSet::new())),
        }
    }
}

impl RunnerService {
    pub fn new() -> Self {
        RunnerService::default()
    }

    pub fn stop(&self, id: &str) -> Result<(), String> {
        let proc = self
            .procs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        match proc {
            Some(p) => {
                tauri::async_runtime::spawn(async move {
                    let mut guard = p.lock().await;
                    if let Some(child) = guard.as_mut() {
                        let _ = child.start_kill();
                    }
                });
                Ok(())
            }
            None => Err("这个运行已经结束".into()),
        }
    }

    pub fn running(&self) -> Vec<String> {
        self.procs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }

    /// Spawn a run. Returns as soon as the process exists; output arrives as
    /// `run://event` frames.
    pub fn start(
        &self,
        app: AppHandle,
        cfg: &AppConfig,
        path: &str,
        language: &str,
        command_override: Option<String>,
        args_override: Option<String>,
    ) -> Result<RunInfo, String> {
        let file = PathBuf::from(path);
        if !file.is_file() {
            return Err(format!("文件不存在：{path}"));
        }
        let dir = file.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
        let stem = file
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "main".into());
        let name = file
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "main".into());

        let lang = if language.trim().is_empty() {
            language_of(path)
        } else {
            language.to_string()
        };

        let saved = cfg.runners.get(&lang);
        let mut command = command_override
            .filter(|c| !c.trim().is_empty())
            .or_else(|| saved.map(|r| r.command.clone()).filter(|c| !c.trim().is_empty()))
            .unwrap_or_default();
        let mut args = args_override
            .filter(|a| !a.trim().is_empty())
            .or_else(|| saved.map(|r| r.args.clone()).filter(|a| !a.trim().is_empty()))
            .unwrap_or_else(|| "{file}".into());
        if command.is_empty() {
            let (c, a) = default_runner(&lang)
                .ok_or_else(|| format!("没有为 {lang} 配置运行器；请在设置里填写运行命令"))?;
            command = c;
            if args == "{file}" {
                args = a;
            }
        }

        let file_s = file.to_string_lossy().to_string();
        let dir_s = dir.to_string_lossy().to_string();
        let vars = [
            ("file", file_s.clone()),
            ("stem", stem.clone()),
            ("name", name.clone()),
            ("dir", dir_s.clone()),
        ];
        let expanded_args = expand(&args, &vars);
        let command_line = if expanded_args.trim().is_empty() {
            format!("\"{file_s}\"")
        } else {
            format!("{} {}", expand(&command, &vars), expanded_args)
        };

        // `cmd` is avoided entirely unless the template needs a pipe/redirect.
        //
        // `cmd /C "python -u \"C:\中文\x.py\""` is a trap: cmd does not
        // understand backslash-escaped quotes, so the program received the
        // quotes as part of the file name — which looked exactly like an
        // encoding bug. Direct execution with an argument vector, and `&&`
        // handled as a sequence of stages, keeps every path intact.
        let stages: Vec<Vec<String>> = split_stages(&command_line)
            .into_iter()
            .map(|stage| split_command(&stage))
            .collect();
        let id = format!("run-{}", crate::util::now_ms());
        let holder: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        self.procs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), holder.clone());

        let info = RunInfo {
            id: id.clone(),
            command: command_line.clone(),
            cwd: dir_s.clone(),
            language: lang.clone(),
        };
        let _ = app.emit(
            "run://event",
            json!({
                "run": id, "kind": "start",
                "command": command_line, "cwd": dir_s, "language": lang,
            }),
        );

        let env_pairs: Vec<(String, String)> = saved.map(|r| r.env.clone()).unwrap_or_default();
        spawn_stages(
            app.clone(),
            id.clone(),
            stages,
            dir.clone(),
            env_pairs,
            holder,
            self.procs.clone(),
            self.exits.clone(),
            self.finished.clone(),
        );

        Ok(info)
    }

    /// Run a raw command line — a `vscode.Task` or a terminal `sendText`.
    ///
    /// Unlike `start()` there is no file and no placeholder expansion: the
    /// caller already produced the exact line. Everything else (stage
    /// splitting, direct argv on Windows, streamed output, stop button) is the
    /// same pipeline, so a Gradle build reports into the same output panel as
    /// `F5` does.
    pub fn run_line(
        &self,
        app: &AppHandle,
        command_line: &str,
        dir: &Path,
        env_pairs: Vec<(String, String)>,
    ) -> Result<RunInfo, String> {
        let line = command_line.trim();
        if line.is_empty() {
            return Err("命令为空".into());
        }
        let stages: Vec<Vec<String>> = split_stages(line)
            .into_iter()
            .map(|stage| split_command(&stage))
            .collect();
        self.launch(app, stages, line, dir, env_pairs)
    }

    /// Run an argv vector exactly as given.
    ///
    /// `new ProcessExecution(process, args)` already carries an argument list.
    /// Joining it into a string and splitting it again would destroy any
    /// argument containing spaces or quotes — `node -e "console.log(1)"` came
    /// back as `console.log(1)` and failed to parse — so the vector goes
    /// straight to the OS.
    pub fn run_argv(
        &self,
        app: &AppHandle,
        argv: Vec<String>,
        dir: &Path,
        env_pairs: Vec<(String, String)>,
    ) -> Result<RunInfo, String> {
        if argv.is_empty() || argv[0].trim().is_empty() {
            return Err("命令为空".into());
        }
        let label = argv.join(" ");
        self.launch(app, vec![argv], &label, dir, env_pairs)
    }

    fn launch(
        &self,
        app: &AppHandle,
        stages: Vec<Vec<String>>,
        label: &str,
        dir: &Path,
        env_pairs: Vec<(String, String)>,
    ) -> Result<RunInfo, String> {
        let id = format!("task-{}", crate::util::now_ms());
        let holder: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        self.procs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), holder.clone());

        let info = RunInfo {
            id: id.clone(),
            command: label.to_string(),
            cwd: dir.to_string_lossy().to_string(),
            language: String::new(),
        };
        let _ = app.emit(
            "run://event",
            json!({
                "run": id, "kind": "start",
                "command": label, "cwd": dir.to_string_lossy(), "language": "",
            }),
        );

        spawn_stages(
            app.clone(),
            id,
            stages,
            dir.to_path_buf(),
            env_pairs,
            holder,
            self.procs.clone(),
            self.exits.clone(),
            self.finished.clone(),
        );
        Ok(info)
    }

    /// Register a one-shot callback for when run `id` exits.
    ///
    /// Used by the extension host to turn a finished process into
    /// `onDidEndTask` / `onDidEndTerminalShellExecution`.
    pub fn on_exit<F: FnOnce(i32) + Send + 'static>(&self, id: &str, f: F) {
        let already = self
            .finished
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(id);
        if already {
            f(0);
            return;
        }
        self.exits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id.to_string())
            .or_default()
            .push(Box::new(f));
    }
}

/// Drive every stage of a command line, emitting `run://event` as it goes.
#[allow(clippy::too_many_arguments)]
fn spawn_stages(
    app: AppHandle,
    id: String,
    stages: Vec<Vec<String>>,
    dir: PathBuf,
    env_pairs: Vec<(String, String)>,
    holder: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>,
    procs: ProcMap,
    exits: ExitMap,
    finished: DoneSet,
) {
    tauri::async_runtime::spawn(async move {
        let id_task = id;
        let mut code = 0i32;
        for (index, stage) in stages.iter().enumerate() {
            if index > 0 {
                let _ = app.emit(
                    "run://event",
                    json!({ "run": id_task, "kind": "chunk", "stream": "stdout", "text": format!("$ {}", stage.join(" ")) }),
                );
            }
            let mut cmd = build_stage(stage, &dir);
            cmd.current_dir(&dir)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .env("PYTHONUTF8", "1")
                .env("PYTHONIOENCODING", "utf-8")
                .env("PYTHONLEGACYWINDOWSSTDIO", "0");
            for (k, v) in &env_pairs {
                if !k.trim().is_empty() {
                    cmd.env(k.trim(), v);
                }
            }
            #[cfg(windows)]
            {
                cmd.creation_flags(0x0800_0000);
            }

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    let _ = app.emit(
                        "run://event",
                        json!({ "run": id_task, "kind": "chunk", "stream": "stderr", "text": format!("启动失败：{e}") }),
                    );
                    code = -1;
                    break;
                }
            };
            let out = child.stdout.take();
            let err = child.stderr.take();
            *holder.lock().await = Some(child);
            let a = pump(app.clone(), id_task.clone(), out, "stdout");
            let b = pump(app.clone(), id_task.clone(), err, "stderr");
            let _ = tokio::join!(a, b);
            let mut guard = holder.lock().await;
            let status = match guard.as_mut() {
                Some(c) => c.wait().await.ok(),
                None => None,
            };
            drop(guard);
            code = status.and_then(|s| s.code()).unwrap_or(-1);
            if code != 0 {
                break;
            }
        }
        procs.lock().unwrap_or_else(|e| e.into_inner()).remove(&id_task);
        let _ = app.emit(
            "run://event",
            json!({ "run": id_task, "kind": "exit", "code": code }),
        );
        finished
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id_task.clone());
        let waiters = exits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id_task);
        if let Some(list) = waiters {
            for cb in list {
                cb(code);
            }
        }
    });
}

/// Split a command template into `&&` stages (quote-aware).
fn split_stages(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                quoted = !quoted;
                cur.push(ch);
            }
            '&' if !quoted && chars.peek() == Some(&'&') => {
                chars.next();
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    if out.is_empty() {
        out.push(line.to_string());
    }
    out
}

/// Resolve a program for `CreateProcess`: `(path, needs_shell)`.
///
/// Windows needs help here, and every build tool hits it:
///   * `gradlew` is a *shell* script on a Mac and `gradlew.bat` on Windows —
///     the same task definition must find the `.bat`.
///   * `CreateProcess` cannot start a `.bat`/`.cmd` at all: it has to go
///     through `cmd`.
///   * A bare `gradle` has no extension and is not on PATH, while
///     `./gradlew` does — resolving relative to the task's cwd is what makes
///     "run the wrapper" work without a global install.
fn resolve_program(prog: &str, dir: &Path) -> (String, bool) {
    if !cfg!(windows) {
        return (prog.to_string(), false);
    }
    let lower = prog.to_ascii_lowercase();
    if lower.ends_with(".exe") || lower.ends_with(".com") {
        return (prog.to_string(), false);
    }
    if lower.ends_with(".bat") || lower.ends_with(".cmd") {
        return (prog.to_string(), true);
    }

    let base: Option<PathBuf> = if prog.contains('/') || prog.contains('\\') {
        let p = PathBuf::from(prog.replace('/', "\\"));
        Some(if p.is_absolute() { p } else { dir.join(p) })
    } else {
        which(prog)
    };

    let Some(base) = base else {
        return (prog.to_string(), false);
    };
    // An exact file already wins; otherwise add the PATHEXT the caller omitted.
    if base.is_file() {
        let is_batch = base
            .extension()
            .map(|e| e.eq_ignore_ascii_case("bat") || e.eq_ignore_ascii_case("cmd"))
            .unwrap_or(false);
        return (base.to_string_lossy().to_string(), is_batch);
    }
    for ext in [".exe", ".cmd", ".bat", ".com"] {
        let cand = PathBuf::from(format!("{}{ext}", base.to_string_lossy()));
        if cand.is_file() {
            return (cand.to_string_lossy().to_string(), ext == ".bat" || ext == ".cmd");
        }
    }
    (prog.to_string(), false)
}

/// One stage of a command line, already tokenised.
///
/// `&&` is split before this point, so a stage is either an argv vector from an
/// extension (`ProcessExecution`) or a shell-ish line that still contains a
/// pipe/redirect and therefore needs `cmd`/`sh` to be interpreted.
fn build_stage(argv: &[String], dir: &Path) -> tokio::process::Command {
    let shellish = argv
        .iter()
        .any(|a| a.contains('|') || a.contains('>') || a.contains('<'));
    if shellish {
        let stage = argv.join(" ");
        if cfg!(windows) {
            let mut c = tokio::process::Command::new("cmd");
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                c.as_std_mut()
                    .raw_arg(format!("/S /C \"chcp 65001>nul & {stage}\""));
            }
            return c;
        }
        let mut c = tokio::process::Command::new("sh");
        c.arg("-lc").arg(stage);
        return c;
    }

    let head = argv.first().cloned().unwrap_or_default();
    let (prog, needs_shell) = resolve_program(&head, dir);
    if needs_shell {
        // `.bat` / `.cmd` cannot be spawned directly. `cmd /C "<path>" args`
        // with the path quoted is the only form that survives a directory name
        // containing spaces.
        let rest = argv[1..].join(" ");
        let mut c = tokio::process::Command::new("cmd");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.as_std_mut()
                .raw_arg(format!("/S /C \"chcp 65001>nul & \"{}\" {rest}\"", prog));
        }
        #[cfg(not(windows))]
        {
            c.arg("/C").arg(&prog).args(&argv[1..]);
        }
        return c;
    }

    let mut c = tokio::process::Command::new(prog);
    if argv.len() > 1 {
        c.args(&argv[1..]);
    }
    c
}

/// Forward a child's output, line by line, to the renderer.
///
/// Reads raw bytes and decodes each *complete* line. `BufReader::lines()` used
/// to be the obvious choice, but it fails the whole loop with `InvalidData` on
/// the first byte sequence that is not valid UTF-8 — a GBK build tool would
/// produce one broken line and then go silent, which looked like the process
/// had hung.
async fn pump<R>(app: AppHandle, run: String, pipe: Option<R>, stream: &'static str)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(pipe) = pipe else { return };
    let mut reader = BufReader::new(pipe);
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => break,
            Ok(_) => {
                let text = crate::util::decode_output(&buf);
                let line = text.trim_end_matches(['\r', '\n']);
                let _ = app.emit(
                    "run://event",
                    json!({ "run": run, "kind": "chunk", "stream": stream, "text": line }),
                );
            }
            Err(_) => break,
        }
    }
}
