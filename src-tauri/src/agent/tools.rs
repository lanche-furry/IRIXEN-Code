//! The agent's built-in ("native") tool set.
//!
//! Deliberately small and sharp: seven tools that cover read → edit → verify.
//! Every tool confines writes to the workspace root and caps its own output so
//! a single call can never blow up the context window.

use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

fn spec(name: &str, description: &str, parameters: Value) -> ToolSpec {
    ToolSpec {
        name: name.to_string(),
        description: description.to_string(),
        parameters,
    }
}

pub fn native_tool_specs() -> Vec<ToolSpec> {
    vec![
        spec(
            "read_file",
            "Read a UTF-8 text file from the workspace, optionally a line range. \
             Output is line-numbered so you can target edits precisely. \
             Prefer narrow ranges over whole large files.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path relative to the workspace root" },
                    "start_line": { "type": "integer", "description": "1-based first line (optional)" },
                    "end_line": { "type": "integer", "description": "Inclusive last line (optional)" }
                },
                "required": ["path"]
            }),
        ),
        spec(
            "write_file",
            "Create or overwrite a file with the given content. Parent directories are created. \
             Reports whether the content actually changed.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
        ),
        spec(
            "replace_in_file",
            "Exact string replacement inside one file — the preferred edit tool. \
             `old` must match verbatim (including indentation) and be unique unless replace_all is true. \
             Fails loudly instead of guessing, so include enough surrounding context.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old": { "type": "string", "description": "Exact text to replace" },
                    "new": { "type": "string", "description": "Replacement text" },
                    "replace_all": { "type": "boolean", "description": "Replace every occurrence (default false)" }
                },
                "required": ["path", "old", "new"]
            }),
        ),
        spec(
            "list_dir",
            "List a directory tree (skips .git, node_modules, target, dist). \
             Use it to orient yourself before reading files.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory relative to workspace root (default: root)" },
                    "depth": { "type": "integer", "description": "Recursion depth, default 2, max 4" }
                }
            }),
        ),
        spec(
            "search_files",
            "Search file contents in the workspace (gitignore-aware) and return matching lines \
             with path:line. Use instead of reading many files just to locate a symbol.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Literal text to find" },
                    "glob": { "type": "string", "description": "Optional glob filter, e.g. src/**/*.ts" },
                    "case_sensitive": { "type": "boolean" },
                    "max_results": { "type": "integer", "description": "Default 80, cap 300" }
                },
                "required": ["query"]
            }),
        ),
        spec(
            "run_command",
            "Run a shell command in the workspace and return exit code, stdout and stderr. \
             Use for builds, tests, linters and scripts. Output is truncated; keep commands quiet.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Full command line" },
                    "timeout_secs": { "type": "integer", "description": "Default 120, max 900" },
                    "cwd": { "type": "string", "description": "Optional directory relative to workspace root" }
                },
                "required": ["command"]
            }),
        ),
        spec(
            "git",
            "Run a git operation in the workspace: status, diff, stage, unstage, commit, log, branch, checkout, init.",
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "diff", "stage", "unstage", "commit", "log", "branch", "checkout", "init"]
                    },
                    "paths": { "type": "array", "items": { "type": "string" } },
                    "message": { "type": "string", "description": "Commit message (action=commit)" },
                    "staged": { "type": "boolean", "description": "Show staged diff (action=diff)" },
                    "limit": { "type": "integer", "description": "Log entries, default 10" }
                },
                "required": ["action"]
            }),
        ),
    ]
}

/// Resolve a user-supplied path and refuse anything outside the workspace.
pub(crate) fn resolve(workspace: &Path, raw: &str) -> Result<PathBuf, String> {
    let ws = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let joined = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        ws.join(raw)
    };
    let mut out = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if !out.starts_with(&ws) {
        return Err(format!(
            "refused: '{raw}' resolves outside the workspace. Ask the user to open the right folder first."
        ));
    }
    Ok(out)
}

fn need_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("missing required argument '{key}'"))
}

pub async fn execute_native(name: &str, args: &Value, workspace: &Path) -> Result<String, String> {
    match name {
        "read_file" => {
            let path = need_str(args, "path")?;
            let p = resolve(workspace, &path)?;
            let content = std::fs::read_to_string(&p)
                .map_err(|e| format!("cannot read {}: {e}", p.display()))?;
            let lines: Vec<&str> = content.lines().collect();
            let start = args
                .get("start_line")
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .max(1) as usize;
            let default_end = (start + 399).min(lines.len());
            let end = args
                .get("end_line")
                .and_then(|v| v.as_u64())
                .map(|v| (v as usize).min(lines.len()))
                .unwrap_or(default_end)
                .max(start);
            let mut out = format!(
                "{} ({} lines total, showing {}-{})\n",
                path,
                lines.len(),
                start,
                end.min(lines.len())
            );
            for i in start..=end {
                if i > lines.len() {
                    break;
                }
                out.push_str(&format!("{:>5}| {}\n", i, lines[i - 1]));
            }
            Ok(crate::util::cap_chars(&out, 14_000))
        }
        "write_file" => {
            let path = need_str(args, "path")?;
            let content = need_str(args, "content")?;
            let p = resolve(workspace, &path)?;
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let before = std::fs::read_to_string(&p).ok();
            if before.as_deref() == Some(content.as_str()) {
                return Ok(format!("unchanged: {path}"));
            }
            std::fs::write(&p, content.as_bytes()).map_err(|e| e.to_string())?;
            let lines = content.lines().count();
            Ok(format!(
                "wrote {} bytes / {lines} lines → {path}{}",
                content.len(),
                if before.is_some() { " (overwrite)" } else { " (new file)" }
            ))
        }
        "replace_in_file" => {
            let path = need_str(args, "path")?;
            let old = need_str(args, "old")?;
            let new = need_str(args, "new")?;
            let all = args
                .get("replace_all")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let p = resolve(workspace, &path)?;
            let content = std::fs::read_to_string(&p)
                .map_err(|e| format!("cannot read {}: {e}", p.display()))?;
            let count = content.matches(&old).count();
            if count == 0 {
                return Err(format!(
                    "'old' text not found in {path}. Re-read the file and copy the exact text."
                ));
            }
            if count > 1 && !all {
                return Err(format!(
                    "'old' is ambiguous: {count} matches in {path}. Add more context or set replace_all."
                ));
            }
            let updated = if all {
                content.replace(&old, &new)
            } else {
                content.replacen(&old, &new, 1)
            };
            std::fs::write(&p, updated.as_bytes()).map_err(|e| e.to_string())?;
            Ok(format!(
                "replaced {count} occurrence(s) in {path} ({} → {} bytes)",
                content.len(),
                updated.len()
            ))
        }
        "list_dir" => {
            let rel = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let depth = args
                .get("depth")
                .and_then(|v| v.as_u64())
                .unwrap_or(2)
                .min(4) as usize;
            let root = if rel.is_empty() {
                workspace
                    .canonicalize()
                    .unwrap_or_else(|_| workspace.to_path_buf())
            } else {
                resolve(workspace, &rel)?
            };
            let mut out = String::new();
            let mut count = 0usize;
            walk(&root, 0, depth, &mut out, &mut count, rel.is_empty());
            if count == 0 {
                return Ok(format!(
                    "{} is empty",
                    root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
                ));
            }
            if count >= 400 {
                out.push_str("… [listing truncated at 400 entries]\n");
            }
            Ok(out)
        }
        "search_files" => {
            let query = need_str(args, "query")?;
            let case_sensitive = args
                .get("case_sensitive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let max = args
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(80)
                .min(300) as usize;
            let mut builder = ignore::WalkBuilder::new(workspace);
            builder
                .hidden(true)
                .git_ignore(true)
                .git_global(false)
                .git_exclude(true)
                .max_depth(Some(24));
            if let Some(glob) = args.get("glob").and_then(|v| v.as_str()) {
                let mut ov = ignore::overrides::OverrideBuilder::new(workspace);
                if ov.add(glob).is_ok() {
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
            'outer: for entry in builder.build().flatten() {
                let path = entry.path();
                if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                let meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if meta.len() > 2_000_000 {
                    continue;
                }
                let content = match std::fs::read_to_string(path) {
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
                        let rel = path
                            .strip_prefix(workspace)
                            .unwrap_or(path)
                            .to_string_lossy()
                            .replace('\\', "/");
                        hits.push(format!(
                            "{}:{}: {}",
                            rel,
                            i + 1,
                            crate::util::one_line(line, 200)
                        ));
                        if hits.len() >= max {
                            break 'outer;
                        }
                    }
                }
            }
            if hits.is_empty() {
                return Ok(format!("no matches for '{query}'"));
            }
            let mut out = format!("{} match(es):\n", hits.len());
            out.push_str(&hits.join("\n"));
            Ok(crate::util::cap_chars(&out, 12_000))
        }
        "run_command" => {
            let command = need_str(args, "command")?;
            let timeout_secs = args
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(120)
                .min(900);
            let cwd = match args.get("cwd").and_then(|v| v.as_str()) {
                Some(c) if !c.is_empty() => resolve(workspace, c)?,
                _ => workspace.to_path_buf(),
            };
            run_shell(&command, &cwd, timeout_secs).await
        }
        "git" => {
            let action = need_str(args, "action")?;
            let paths: Vec<String> = args
                .get("paths")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let ws = workspace.to_path_buf();
            match action.as_str() {
                "status" => {
                    let files = crate::git::status(&ws)?;
                    if files.is_empty() {
                        Ok("working tree clean".into())
                    } else {
                        let mut out = String::new();
                        for f in files {
                            out.push_str(&format!("{} {}\n", f.code, f.path));
                        }
                        Ok(out)
                    }
                }
                "diff" => {
                    let staged = args.get("staged").and_then(|v| v.as_bool()).unwrap_or(false);
                    let path = paths.first().map(|s| s.as_str());
                    let d = crate::git::diff(&ws, staged, path)?;
                    if d.trim().is_empty() {
                        Ok("no changes".into())
                    } else {
                        Ok(crate::util::cap_chars(&d, 12_000))
                    }
                }
                "stage" => crate::git::stage(&ws, &paths),
                "unstage" => crate::git::unstage(&ws, &paths),
                "commit" => {
                    let msg = need_str(args, "message")?;
                    crate::git::commit(&ws, &msg)
                }
                "log" => {
                    let limit = args
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(10) as usize;
                    crate::git::log(&ws, limit)
                }
                "branch" => crate::git::current_branch(&ws),
                "checkout" => {
                    let b = paths
                        .first()
                        .cloned()
                        .or_else(|| args.get("message").and_then(|v| v.as_str()).map(|s| s.to_string()))
                        .ok_or("checkout needs paths[0] = branch name")?;
                    crate::git::checkout(&ws, &b)
                }
                "init" => crate::git::init(&ws),
                other => Err(format!("unknown git action '{other}'")),
            }
        }
        other => Err(format!("unknown tool '{other}'")),
    }
}

fn walk(dir: &Path, level: usize, max: usize, out: &mut String, count: &mut usize, root: bool) {
    if *count >= 400 {
        return;
    }
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().collect(),
        Err(_) => return,
    };
    entries.sort_by_key(|e| {
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        (if is_dir { 0 } else { 1 }, e.file_name())
    });
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(
            name.as_str(),
            ".git" | "node_modules" | "target" | "dist" | ".tools" | "__pycache__" | ".venv"
        ) || name == ".DS_Store"
        {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let indent = "  ".repeat(level);
        let size = if is_dir {
            String::new()
        } else {
            entry
                .metadata()
                .map(|m| format!("  ({})", human_bytes(m.len())))
                .unwrap_or_default()
        };
        out.push_str(&format!(
            "{indent}{}{}{}\n",
            name,
            if is_dir { "/" } else { "" },
            size
        ));
        *count += 1;
        if *count >= 400 {
            return;
        }
        if is_dir && level + 1 < max {
            walk(entry.path().as_path(), level + 1, max, out, count, false);
        }
    }
    let _ = root;
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} B", n)
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// The shell a `run_shell` string is handed to.
///
/// Split by platform instead of a runtime `cfg!` plus an inner `#[cfg]` block:
/// the inner block left `mut` and the argv unused on Linux, which is what
/// produced the `unused_mut` warnings there.
#[cfg(windows)]
fn shell_command(command: &str) -> tokio::process::Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = tokio::process::Command::new("cmd");
    // `chcp 65001` makes cmd's builtins (dir, type, …) and child programs
    // emit UTF-8 instead of the OEM code page, which is what turned every
    // Chinese path in the terminal into `?`. `raw_arg` passes the command
    // exactly as written; `arg()` re-quotes it and cmd does not understand
    // backslash escapes, so `"C:\中文\x.py"` used to arrive with quotes.
    cmd.as_std_mut()
        .raw_arg(format!("/S /C \"chcp 65001>nul & {command}\""));
    cmd
}

#[cfg(not(windows))]
fn shell_command(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-lc").arg(command);
    cmd
}

/// Run a shell command with a hard timeout and capped output.
pub async fn run_shell(command: &str, cwd: &Path, timeout_secs: u64) -> Result<String, String> {
    let mut cmd = shell_command(command);
    crate::util::utf8_env(&mut cmd);
    cmd.current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let fut = cmd.output();
    let result = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut).await;
    match result {
        Err(_) => Ok(format!("[timed out after {timeout_secs}s — command may still be running]")),
        Ok(Err(e)) => Err(format!("failed to spawn command: {e}")),
        Ok(Ok(out)) => {
            let stdout = crate::util::decode_output(&out.stdout);
            let stderr = crate::util::decode_output(&out.stderr);
            let code = out.status.code().unwrap_or(-1);
            let mut text = format!("exit code: {code}\n");
            if !stdout.trim().is_empty() {
                text.push_str("--- stdout ---\n");
                text.push_str(&stdout);
                if !text.ends_with('\n') {
                    text.push('\n');
                }
            }
            if !stderr.trim().is_empty() {
                text.push_str("--- stderr ---\n");
                text.push_str(&stderr);
            }
            Ok(crate::util::cap_chars(&text, 18_000))
        }
    }
}
