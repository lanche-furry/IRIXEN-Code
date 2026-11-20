//! How much the agent may do without asking.
//!
//! Three modes, one ladder, no hidden fourth state. The question this module
//! answers is not "is this tool safe" — no tool is — but "would the user, who
//! is sitting right there, want to be interrupted for this one".
//!
//! ```text
//! restricted  read: workspace only   write: ask        command: ask      outside: refused
//! auto        read: workspace only   write: allow      command: allow    outside: ask
//! full        read: anywhere         write: allow      command: allow    outside: allow
//! ```
//!
//! `outside` means "names a path outside the workspace root". A shell command
//! is never parsed for paths: a permission system that reads shell strings and
//! thinks it knows what they touch is worse than one that admits it cannot, so
//! a command is judged as a command.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    /// Nothing outside the workspace is reachable, every write asks, every
    /// command asks. The default for someone who has not decided yet.
    Restricted,
    /// Inside the workspace the agent works uninterrupted; the boundary is
    /// where it asks. This is the default: the common case is a turn that
    /// edits files in the folder the user just opened, and prompting for each
    /// of those trains people to click "allow" without reading.
    #[default]
    Auto,
    /// No prompts, no boundary. `Full` is an explicit decision, never a
    /// side effect of approving something once.
    Full,
}

impl AccessMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "restricted" => Some(AccessMode::Restricted),
            "auto" => Some(AccessMode::Auto),
            "full" => Some(AccessMode::Full),
            _ => None,
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            AccessMode::Restricted => "restricted",
            AccessMode::Auto => "auto",
            AccessMode::Full => "full",
        }
    }

    /// One line for the system prompt. The model needs to know when a tool call
    /// will stop and wait, or it will spend the turn apologising for a prompt
    /// the user never saw.
    pub fn prompt_note(self) -> &'static str {
        match self {
            AccessMode::Restricted => {
                "当前访问模式：受限访问。每次写文件、每条命令都会先请用户确认；工作区以外的路径一律不可访问。\
                 把一轮需要的写入合并成尽量少的调用，别为了试探边界浪费回合。"
            }
            AccessMode::Auto => {
                "当前访问模式：自动审批。工作区内的写入和命令直接执行；工作区以外的路径会先询问用户。\
                 需要工作区外的文件时，一次说清要哪个路径、为什么。"
            }
            AccessMode::Full => {
                "当前访问模式：完全访问。写入和命令不再询问，工作区以外的路径也可以读写。\
                 越界操作仍然要说明理由：没有提示不代表没有后果。"
            }
        }
    }
}

/// What the user is being asked to approve. `kind` and `target` are structured
/// so the renderer can phrase this in the user's own language instead of
/// echoing a Chinese sentence onto a Japanese screen.
#[derive(Clone, Debug, Serialize)]
pub struct Request {
    pub tool: String,
    /// `write` | `command` | `git` | `mcp` | `read`
    pub kind: &'static str,
    pub target: String,
    /// Optional extra context: byte count, replacement preview, working dir.
    pub detail: String,
}

#[derive(Clone, Debug)]
pub enum Decision {
    Allow,
    Ask(Request),
    Deny(String),
}

/// Classify one tool call. Pure: same inputs, same answer, no I/O beyond a
/// canonicalize, which is why this is testable without a running app.
pub fn decide(mode: AccessMode, tool: &str, args: &Value, workspace: Option<&Path>) -> Decision {
    let outside = names_outside(tool, args, workspace);
    let inside_note = format!(
        "被访问模式拒绝：'{}' 在工作区之外。受限访问只能读写工作区内的路径。\
         告诉用户你要访问什么、为什么，由用户在输入框里切到「自动审批」或「完全访问」。",
        target_of(tool, args)
    );

    match tool {
        // Reads are free until they leave the workspace: looking at a file
        // cannot destroy anything, and prompting for `list_dir` is how a mode
        // becomes something people switch off on day one.
        "read_file" | "list_dir" | "search_files" | "security_scan" => {
            if !outside {
                return Decision::Allow;
            }
            match mode {
                AccessMode::Restricted => Decision::Deny(inside_note),
                AccessMode::Auto => Decision::Ask(request_for(tool, args, "read")),
                AccessMode::Full => Decision::Allow,
            }
        }
        "write_file" | "replace_in_file" => match mode {
            AccessMode::Restricted if outside => Decision::Deny(inside_note),
            AccessMode::Restricted => Decision::Ask(request_for(tool, args, "write")),
            AccessMode::Auto if outside => Decision::Ask(request_for(tool, args, "write")),
            _ => Decision::Allow,
        },
        // A shell command is judged as a command. Only its `cwd` is a path we
        // can actually check; parsing the string for `rm`, `..` or absolute
        // paths would be theatre, and worse: it would make the mode look like
        // a sandbox it is not.
        "run_command" => match mode {
            AccessMode::Restricted => Decision::Ask(request_for(tool, args, "command")),
            AccessMode::Auto if outside => Decision::Ask(request_for(tool, args, "command")),
            _ => Decision::Allow,
        },
        "git" => {
            let action = args.get("action").and_then(Value::as_str).unwrap_or("");
            let mutating = matches!(action, "stage" | "unstage" | "commit" | "checkout" | "init");
            if mutating && mode == AccessMode::Restricted {
                return Decision::Ask(request_for(tool, args, "git"));
            }
            Decision::Allow
        }
        // An MCP server is someone else's code with someone else's side
        // effects; no local rule can describe it. In the one mode that asks
        // for everything, it asks for this too.
        t if t.starts_with("mcp__") => match mode {
            AccessMode::Restricted => Decision::Ask(request_for(tool, args, "mcp")),
            _ => Decision::Allow,
        },
        _ => match mode {
            AccessMode::Restricted => Decision::Ask(request_for(tool, args, "mcp")),
            _ => Decision::Allow,
        },
    }
}

/// Returned to the model when the user says no. Naming the consequence keeps
/// the next turn from re-trying the same call with different wording.
pub fn denied_note(tool: &str) -> String {
    format!(
        "用户拒绝了这次 {tool} 调用。不要重试同一个调用，也不要换个路径绕开：\
         先说明你打算做什么、为什么要这么做，等用户重新授权。"
    )
}

fn target_of(tool: &str, args: &Value) -> String {
    let raw = match tool {
        "run_command" => args.get("command").and_then(Value::as_str),
        "git" => args.get("action").and_then(Value::as_str),
        _ => args.get("path").and_then(Value::as_str),
    };
    raw.unwrap_or("")
        .chars()
        .take(160)
        .collect::<String>()
}

fn request_for(tool: &str, args: &Value, kind: &'static str) -> Request {
    let target = target_of(tool, args);
    let detail = match (kind, tool) {
        ("write", "write_file") => {
            let bytes = args.get("content").and_then(Value::as_str).map(str::len).unwrap_or(0);
            format!("{bytes} bytes")
        }
        ("write", _) => {
            let old = args.get("old").and_then(Value::as_str).unwrap_or("");
            let new = args.get("new").and_then(Value::as_str).unwrap_or("");
            format!(
                "{} → {}",
                crate::util::one_line(old, 60),
                crate::util::one_line(new, 60)
            )
        }
        ("command", _) => args
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        ("git", _) => args
            .get("paths")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .take(6)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        _ => crate::util::one_line(&args.to_string(), 200),
    };
    Request { tool: tool.to_string(), kind, target, detail }
}

/// True when the tool names a path and that path leaves the workspace.
fn names_outside(tool: &str, args: &Value, workspace: Option<&Path>) -> bool {
    // Someone else's tool, someone else's argument names: guessing that an MCP
    // server's `path` means a local path would be a rule with no evidence.
    if tool.starts_with("mcp__") {
        return false;
    }
    // No workspace open: there is no boundary to cross, and the native tools
    // that need one refuse on their own.
    let Some(workspace) = workspace else {
        return false;
    };
    match tool {
        "run_command" => outside_names_cwd(args, workspace),
        "git" => args
            .get("paths")
            .and_then(Value::as_array)
            .map(|paths| {
                paths
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|p| !super::tools::inside(workspace, p))
            })
            .unwrap_or(false),
        _ => args
            .get("path")
            .and_then(Value::as_str)
            .map(|p| !p.is_empty() && !super::tools::inside(workspace, p))
            .unwrap_or(false),
    }
}

fn outside_names_cwd(args: &Value, workspace: &Path) -> bool {
    args.get("cwd")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .map(|c| !super::tools::inside(workspace, c))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ws() -> std::path::PathBuf {
        // Any absolute directory works: `inside` canonicalizes the root and
        // compares, it does not require the leaf to exist.
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn is_allow(d: &Decision) -> bool {
        matches!(d, Decision::Allow)
    }

    #[test]
    fn reads_inside_the_workspace_never_ask() {
        for mode in [AccessMode::Restricted, AccessMode::Auto, AccessMode::Full] {
            for tool in ["read_file", "list_dir", "search_files", "security_scan"] {
                let d = decide(mode, tool, &json!({ "path": "src/main.rs" }), Some(&ws()));
                assert!(is_allow(&d), "{tool} in {mode:?} should be allowed");
            }
        }
    }

    #[test]
    fn restricted_refuses_to_leave_the_workspace_instead_of_asking() {
        let outside = json!({ "path": "../elsewhere/secret.txt" });
        let d = decide(AccessMode::Restricted, "read_file", &outside, Some(&ws()));
        assert!(matches!(d, Decision::Deny(_)), "restricted must refuse, not prompt");
    }

    #[test]
    fn restricted_asks_before_every_write_but_still_allows_it() {
        let args = json!({ "path": "src/lib.rs", "content": "fn main() {}" });
        let d = decide(AccessMode::Restricted, "write_file", &args, Some(&ws()));
        match d {
            Decision::Ask(req) => {
                assert_eq!(req.kind, "write");
                assert_eq!(req.target, "src/lib.rs");
            }
            other => panic!("expected Ask, got {other:?}"),
        }
    }

    #[test]
    fn auto_only_asks_once_the_path_leaves_the_workspace() {
        let inside = json!({ "path": "src/lib.rs", "content": "x" });
        assert!(is_allow(&decide(AccessMode::Auto, "write_file", &inside, Some(&ws()))));

        let outside = json!({ "path": "/tmp/notes.md", "content": "x" });
        assert!(matches!(
            decide(AccessMode::Auto, "write_file", &outside, Some(&ws())),
            Decision::Ask(_)
        ));
    }

    #[test]
    fn full_never_asks() {
        let outside = json!({ "path": "/tmp/notes.md", "content": "x" });
        assert!(is_allow(&decide(AccessMode::Full, "write_file", &outside, Some(&ws()))));
        let cmd = json!({ "command": "rm -rf build" });
        assert!(is_allow(&decide(AccessMode::Full, "run_command", &cmd, Some(&ws()))));
    }

    #[test]
    fn restricted_asks_for_commands_and_git_writes() {
        let cmd = json!({ "command": "cargo test" });
        assert!(matches!(
            decide(AccessMode::Restricted, "run_command", &cmd, Some(&ws())),
            Decision::Ask(_)
        ));
        let read_only_git = json!({ "action": "status" });
        assert!(is_allow(&decide(AccessMode::Restricted, "git", &read_only_git, Some(&ws()))));
        let commit = json!({ "action": "commit", "message": "x" });
        assert!(matches!(
            decide(AccessMode::Restricted, "git", &commit, Some(&ws())),
            Decision::Ask(_)
        ));
    }

    #[test]
    fn a_command_cwd_outside_the_workspace_asks_even_in_auto() {
        let cmd = json!({ "command": "ls", "cwd": "../" });
        assert!(matches!(
            decide(AccessMode::Auto, "run_command", &cmd, Some(&ws())),
            Decision::Ask(_)
        ));
    }

    #[test]
    fn mcp_calls_ask_in_restricted_and_pass_elsewhere() {
        let args = json!({ "query": "x" });
        assert!(matches!(
            decide(AccessMode::Restricted, "mcp__github__search", &args, Some(&ws())),
            Decision::Ask(_)
        ));
        assert!(is_allow(&decide(AccessMode::Auto, "mcp__github__search", &args, Some(&ws()))));
    }

    #[test]
    fn modes_round_trip_through_their_wire_names() {
        for mode in [AccessMode::Restricted, AccessMode::Auto, AccessMode::Full] {
            assert_eq!(AccessMode::parse(mode.slug()), Some(mode));
        }
        assert_eq!(AccessMode::parse("full-access"), None);
    }
}
