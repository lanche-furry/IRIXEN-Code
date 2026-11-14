//! A real pseudo-terminal (PTY).
//!
//! What was here before was not a terminal: type a line, spawn one process,
//! print its output. That has no prompt, no `cd` that sticks, no tab
//! completion, no arrow-key history, no `ctrl-c`, no colour and no REPL — and
//! it is why Chinese text came out as mojibake, because a pipe has no console
//! code page to negotiate.
//!
//! A PTY fixes all of it at once. `portable-pty` gives us ConPTY on Windows and
//! `openpty` on Unix, so the shell believes it owns a real console: it prints
//! its own prompt, reads keys as they are typed, and gets `SIGWINCH`-equivalent
//! resize events. The core owns the process; the renderer (xterm.js) only
//! forwards keystrokes and paints the byte stream.
//!
//! Encoding: the child is started on a UTF-8 console (`chcp 65001`) with the
//! usual `PYTHONUTF8` family, and ConPTY itself serialises its output as UTF-8.
//! Bytes travel to the renderer base64-encoded so a multi-byte character split
//! across two reads is never corrupted — xterm.js reassembles it.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::runner::which;

static SEQ: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------
// shells
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct ShellOption {
    pub id: String,
    /// Shown in the picker, e.g. "PowerShell 7".
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    pub available: bool,
    /// Opened when nothing else is configured.
    pub recommended: bool,
}

/// Every shell we know how to start, best first.
///
/// The list is discovered rather than hard-coded per install: `pwsh` may or may
/// not be there, Git Bash lives in one of two places, and WSL is optional. Only
/// entries whose executable actually resolves are marked available.
pub fn shells() -> Vec<ShellOption> {
    let mut out: Vec<ShellOption> = Vec::new();
    if cfg!(windows) {
        out.push(ps_option("pwsh", "PowerShell 7", true));
        out.push(ps_option("powershell", "Windows PowerShell", true));
        out.push(ShellOption {
            id: "cmd".into(),
            label: "Command Prompt".into(),
            program: "cmd.exe".into(),
            args: vec!["/K".into(), "chcp 65001>nul".into()],
            available: which("cmd").is_some(),
            recommended: false,
        });
        for (prog, label) in [
            (r"C:\Program Files\Git\bin\bash.exe", "Git Bash"),
            (r"C:\Program Files (x86)\Git\bin\bash.exe", "Git Bash"),
        ] {
            if std::path::Path::new(prog).is_file() {
                out.push(ShellOption {
                    id: "git-bash".into(),
                    label: label.into(),
                    program: prog.into(),
                    args: vec!["--login".into(), "-i".into()],
                    available: true,
                    recommended: false,
                });
                break;
            }
        }
        if which("wsl").is_some() {
            out.push(ShellOption {
                id: "wsl".into(),
                label: "WSL".into(),
                program: "wsl.exe".into(),
                args: Vec::new(),
                available: true,
                recommended: false,
            });
        }
    } else {
        let login = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let name = PathBuf::from(&login)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "shell".into());
        out.push(ShellOption {
            id: name.clone(),
            label: login.clone(),
            program: login.clone(),
            args: Vec::new(),
            available: std::path::Path::new(&login).is_file(),
            recommended: true,
        });
        for cand in ["/bin/zsh", "/bin/bash", "/bin/sh"] {
            if cand == login {
                continue;
            }
            let id = PathBuf::from(cand)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            out.push(ShellOption {
                id,
                label: cand.into(),
                program: cand.into(),
                args: Vec::new(),
                available: std::path::Path::new(cand).is_file(),
                recommended: false,
            });
        }
    }
    // Exactly one recommendation, even if the preferred shell is missing:
    // the first available entry takes the flag.
    if !out.iter().any(|s| s.recommended) {
        if let Some(first) = out.iter_mut().find(|s| s.available) {
            first.recommended = true;
        }
    }
    out
}

fn ps_option(program: &str, label: &str, recommended: bool) -> ShellOption {
    ShellOption {
        id: program.into(),
        label: label.into(),
        program: format!("{program}.exe"),
        // `-NoExit` keeps the session interactive after the one-shot init, and
        // `-Command` does not echo itself — so the console page is already
        // UTF-8 before the first prompt and the user sees no injection.
        //
        // The code page is changed with `$null = chcp …` rather than
        // `chcp … > $null`: a `>` inside an argument list is not reliably
        // preserved through CreateProcess quoting, and PowerShell ended up
        // handing `65001>` to chcp as a parameter ("Parameter format not
        // correct - 65001>").
        args: vec![
            "-NoLogo".into(),
            "-NoExit".into(),
            "-Command".into(),
            "[Console]::OutputEncoding=[Text.Encoding]::UTF8;$OutputEncoding=[Text.Encoding]::UTF8;$null=chcp 65001".into(),
        ],
        available: which(program).is_some() || which(&format!("{program}.exe")).is_some(),
        recommended,
    }
}

/// Resolve the shell to start: explicit id → configured id → recommended.
fn pick_shell(wanted: Option<&str>) -> Result<ShellOption, String> {
    let all = shells();
    if let Some(w) = wanted.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(hit) = all
            .iter()
            .find(|s| s.id.eq_ignore_ascii_case(w) || s.program.eq_ignore_ascii_case(w))
        {
            if hit.available {
                return Ok(hit.clone());
            }
        }
        // A path typed by hand: use it verbatim rather than silently falling
        // back to something else the user did not ask for.
        if std::path::Path::new(w).is_file() {
            return Ok(ShellOption {
                id: "custom".into(),
                label: w.to_string(),
                program: w.to_string(),
                args: Vec::new(),
                available: true,
                recommended: false,
            });
        }
    }
    all.into_iter()
        .find(|s| s.recommended)
        .or_else(|| shells().into_iter().find(|s| s.available))
        .ok_or_else(|| "找不到可用的 shell".to_string())
}

// ---------------------------------------------------------------------------
// sessions
// ---------------------------------------------------------------------------

/// How much scrollback the core keeps for a session the renderer has not
/// attached to yet. An extension can `createTerminal()` and `sendText()` long
/// before it calls `show()`, and without this the user would open the panel to
/// a blank box — the command ran, the output went nowhere.
const REPLAY_LIMIT: usize = 256 * 1024;

struct Session {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    shell: String,
    cwd: String,
    pid: Option<u32>,
    /// Raw PTY bytes since the session opened (or since the last attach), so a
    /// late-joining renderer can be caught up.
    replay: Mutex<Vec<u8>>,
}

#[derive(Serialize, Clone)]
pub struct TermInfo {
    pub id: String,
    pub shell: String,
    pub cwd: String,
    pub pid: Option<u32>,
}

/// What a renderer needs to paint a session that already exists.
#[derive(Serialize, Clone)]
pub struct TermView {
    pub id: String,
    pub shell: String,
    pub cwd: String,
    /// Base64 of everything the PTY has emitted so far.
    pub replay: String,
}

type Sessions = Arc<Mutex<HashMap<String, Arc<Session>>>>;

#[derive(Default)]
pub struct TermService {
    sessions: Sessions,
}

impl TermService {
    pub fn new() -> Self {
        TermService::default()
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Session>>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Open a shell on a fresh PTY. Returns immediately; output streams as
    /// `term://data` frames and the first prompt paints itself.
    pub fn open(
        &self,
        app: AppHandle,
        cwd: Option<String>,
        shell: Option<String>,
        cols: u16,
        rows: u16,
    ) -> Result<TermInfo, String> {
        let opt = pick_shell(shell.as_deref())?;
        let cwd = resolve_cwd(cwd);

        // An extension that opens a terminal per task can pile up shells. Cap
        // the fleet rather than letting a stray loop spawn hundreds.
        const MAX_SESSIONS: usize = 16;
        loop {
            let oldest = {
                let map = self.map();
                if map.len() < MAX_SESSIONS {
                    break;
                }
                map.keys().next().cloned()
            };
            match oldest {
                Some(id) => {
                    let _ = self.close(&id);
                }
                None => break,
            }
        }

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: rows.max(2),
                cols: cols.max(20),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("无法创建伪终端：{e}"))?;

        let mut cmd = CommandBuilder::new(opt.program.as_str());
        for a in &opt.args {
            cmd.arg(a);
        }
        cmd.cwd(&cwd);
        let mut env: Vec<(String, String)> = vec![
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
            ("TERM_PROGRAM".into(), "vscode".into()),
            ("PYTHONUTF8".into(), "1".into()),
            ("PYTHONIOENCODING".into(), "utf-8".into()),
            ("PYTHONLEGACYWINDOWSSTDIO".into(), "0".into()),
            // Windows tools that honour it: node, less, git for windows.
            ("LANG".into(), "zh_CN.UTF-8".into()),
        ];
        if cfg!(windows) {
            // Keep the inherited environment clean of a stale code page
            // setting; the shell's own `chcp` wins.
            env.push(("LC_ALL".into(), "".into()));
        }
        for (k, v) in &env {
            if v.is_empty() {
                continue;
            }
            cmd.env(k.as_str(), v.as_str());
        }

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("无法连接终端输入：{e}"))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("无法连接终端输出：{e}"))?;
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("启动 {label} 失败：{e}", label = opt.label))?;
        // The slave handle must go, otherwise the PTY never reports EOF when
        // the shell exits and the reader thread hangs forever.
        drop(pair.slave);

        let pid = child.process_id();
        let id = format!("term-{}-{}", crate::util::now_ms(), SEQ.fetch_add(1, Ordering::Relaxed));
        let session = Arc::new(Session {
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            child: Mutex::new(child),
            shell: opt.label.clone(),
            cwd: cwd.to_string_lossy().to_string(),
            pid,
            replay: Mutex::new(Vec::new()),
        });
        self.map().insert(id.clone(), session.clone());

        let info = TermInfo {
            id: id.clone(),
            shell: opt.label.clone(),
            cwd: cwd.to_string_lossy().to_string(),
            pid,
        };

        spawn_reader(app, self.sessions.clone(), id, reader, session);
        Ok(info)
    }

    /// Describe a live session so a renderer that was not listening yet can
    /// paint it — including everything it printed in the meantime.
    pub fn attach(&self, id: &str) -> Result<TermView, String> {
        let session = self.get(id)?;
        let replay = session
            .replay
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        Ok(TermView {
            id: id.to_string(),
            shell: session.shell.clone(),
            cwd: session.cwd.clone(),
            replay: STANDARD.encode(&replay),
        })
    }

    pub fn exists(&self, id: &str) -> bool {
        self.map().contains_key(id)
    }
    pub fn write(&self, id: &str, data: &str) -> Result<(), String> {
        let session = self.get(id)?;
        let mut w = session.writer.lock().unwrap_or_else(|e| e.into_inner());
        w.write_all(data.as_bytes())
            .and_then(|_| w.flush())
            .map_err(|e| format!("写入终端失败：{e}"))
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let session = self.get(id)?;
        let master = session.master.lock().unwrap_or_else(|e| e.into_inner());
        master
            .resize(PtySize {
                rows: rows.max(2),
                cols: cols.max(20),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("调整终端尺寸失败：{e}"))
    }

    pub fn close(&self, id: &str) -> Result<(), String> {
        let session = self
            .map()
            .remove(id)
            .ok_or_else(|| "这个终端已经关闭".to_string())?;
        kill_session(&session);
        Ok(())
    }

    pub fn close_all(&self) {
        let all: Vec<Arc<Session>> = self.map().drain().map(|(_, s)| s).collect();
        for s in all {
            kill_session(&s);
        }
    }

    pub fn list(&self) -> Vec<TermInfo> {
        self.map()
            .iter()
            .map(|(id, s)| TermInfo {
                id: id.clone(),
                shell: s.shell.clone(),
                cwd: String::new(),
                pid: s.pid,
            })
            .collect()
    }

    fn get(&self, id: &str) -> Result<Arc<Session>, String> {
        self.map()
            .get(id)
            .cloned()
            .ok_or_else(|| "终端不存在或已退出".to_string())
    }
}

impl Drop for TermService {
    fn drop(&mut self) {
        self.close_all();
    }
}

/// Kill the shell *and* whatever it started.
///
/// A build tool or an editor launched from the prompt is a grandchild; killing
/// only the shell would leave it holding the console.
fn kill_session(session: &Session) {
    if let Some(pid) = session.pid {
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("taskkill")
                .args(["/T", "/F", "/PID", &pid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        #[cfg(not(windows))]
        {
            let _ = std::process::Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
        }
    }
    if let Ok(mut child) = session.child.lock() {
        let _ = child.kill();
    }
}

/// Pump PTY bytes to the renderer until the shell exits.
fn spawn_reader(
    app: AppHandle,
    sessions: Sessions,
    id: String,
    mut reader: Box<dyn Read + Send>,
    session: Arc<Session>,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 16384];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    {
                        // Keep a bounded window for a late-attaching renderer.
                        let mut replay = session
                            .replay
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        replay.extend_from_slice(&buf[..n]);
                        if replay.len() > REPLAY_LIMIT {
                            let cut = replay.len() - REPLAY_LIMIT;
                            replay.drain(..cut);
                        }
                    }
                    let frame = json!({
                        "id": id,
                        // Base64, not a String: a 3-byte UTF-8 character can
                        // be split across two reads, and lossy decoding here
                        // would turn it into a permanent replacement glyph.
                        "data": STANDARD.encode(&buf[..n]),
                    });
                    if app.emit("term://data", frame).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let code = session
            .child
            .lock()
            .ok()
            .and_then(|mut c| c.wait().ok())
            .map(|s| s.exit_code() as i32)
            .unwrap_or(-1);
        sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        let _ = app.emit("term://exit", json!({ "id": id, "code": code }));
    });
}

fn resolve_cwd(wanted: Option<String>) -> PathBuf {
    if let Some(dir) = wanted.filter(|d| !d.trim().is_empty()) {
        let p = PathBuf::from(crate::util::plain_path(&dir));
        if p.is_dir() {
            return p;
        }
    }
    if let Ok(ws) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        let p = PathBuf::from(ws);
        if p.is_dir() {
            return p;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}
