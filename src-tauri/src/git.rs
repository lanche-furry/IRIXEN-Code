//! Git integration — shells out to the `git` binary so IRIXEN Code stays
//! tiny (no libgit2) and always matches the user's own git configuration.

use serde::Serialize;
use std::path::Path;
use std::process::Command;

pub fn git(workspace: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(workspace)
        // Without this, git quotes any non-ASCII path as C-style octal
        // (`"\344\270\255..."`) and the UI shows mojibake for Chinese names.
        .arg("-c")
        .arg("core.quotePath=false")
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git (is it installed?): {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.success() {
        Ok(stdout)
    } else {
        let msg = format!("{stdout}{stderr}").trim().to_string();
        Err(if msg.is_empty() {
            format!("git {:?} exited with {}", args, out.status)
        } else {
            msg
        })
    }
}

pub fn is_repo(workspace: &Path) -> bool {
    workspace.join(".git").exists()
}

#[derive(Clone, Debug, Serialize)]
pub struct GitFileStatus {
    pub path: String,
    /// Two-letter porcelain code, e.g. "M ", " M", "??", "R ".
    pub code: String,
    pub staged: bool,
}

pub fn status(workspace: &Path) -> Result<Vec<GitFileStatus>, String> {
    let out = git(workspace, &["status", "--porcelain=v1", "-uall"])?;
    let mut files = Vec::new();
    for line in out.lines() {
        if line.len() < 4 {
            continue;
        }
        let code = line[..2].to_string();
        let mut path = line[3..].to_string();
        // Renames: "R  old -> new"
        if let Some(pos) = path.find(" -> ") {
            path = path[pos + 4..].to_string();
        }
        let path = unquote_git_path(&path);
        let staged = !code.starts_with(' ') && code != "??";
        files.push(GitFileStatus { path, code, staged });
    }
    Ok(files)
}

/// `core.quotePath=false` covers normal cases, but a repo-level config can
/// still win and git also escapes quotes/backslashes. Decode the C-style
/// octal sequences back into their UTF-8 bytes so a path like 中文/文件.txt
/// reaches the UI intact.
fn unquote_git_path(raw: &str) -> String {
    let s = raw.trim();
    let inner = if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    };
    let b = inner.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len()
            && (b'0'..=b'7').contains(&b[i + 1])
            && (b'0'..=b'7').contains(&b[i + 2])
            && (b'0'..=b'7').contains(&b[i + 3])
        {
            let v = ((b[i + 1] - b'0') as u16) * 64
                + ((b[i + 2] - b'0') as u16) * 8
                + (b[i + 3] - b'0') as u16;
            out.push(v as u8);
            i += 4;
        } else if b[i] == b'\\' && i + 1 < b.len() {
            let c = match b[i + 1] {
                b'n' => Some(b'\n'),
                b't' => Some(b'\t'),
                b'r' => Some(b'\r'),
                b'"' => Some(b'"'),
                b'\\' => Some(b'\\'),
                _ => None,
            };
            if let Some(c) = c {
                out.push(c);
                i += 2;
            } else {
                out.push(b[i]);
                i += 1;
            }
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

pub fn current_branch(workspace: &Path) -> Result<String, String> {
    git(workspace, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|s| s.trim().to_string())
}

pub fn branches(workspace: &Path) -> Result<Vec<String>, String> {
    let out = git(workspace, &["branch", "--format=%(refname:short)"])?;
    Ok(out.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
}

pub fn diff(workspace: &Path, staged: bool, path: Option<&str>) -> Result<String, String> {
    let mut args = vec!["diff", "--no-color"];
    if staged {
        args.push("--cached");
    }
    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    git(workspace, &args)
}

pub fn stage(workspace: &Path, paths: &[String]) -> Result<String, String> {
    let mut args = vec!["add", "--"];
    args.extend(paths.iter().map(|s| s.as_str()));
    git(workspace, &args)
}

pub fn unstage(workspace: &Path, paths: &[String]) -> Result<String, String> {
    let mut args = vec!["restore", "--staged", "--"];
    args.extend(paths.iter().map(|s| s.as_str()));
    git(workspace, &args)
}

pub fn commit(workspace: &Path, message: &str) -> Result<String, String> {
    git(workspace, &["commit", "-m", message])
}

pub fn log(workspace: &Path, limit: usize) -> Result<String, String> {
    let n = limit.to_string();
    git(
        workspace,
        &[
            "log",
            &format!("-{n}"),
            "--date=short",
            "--pretty=format:%h\t%ad\t%an\t%s",
        ],
    )
}

pub fn init(workspace: &Path) -> Result<String, String> {
    git(workspace, &["init"])
}

pub fn push(workspace: &Path) -> Result<String, String> {
    git(workspace, &["push"])
}

pub fn pull(workspace: &Path) -> Result<String, String> {
    git(workspace, &["pull", "--ff-only"])
}

pub fn checkout(workspace: &Path, branch: &str) -> Result<String, String> {
    git(workspace, &["checkout", branch])
}
