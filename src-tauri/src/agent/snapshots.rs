//! Per-turn file snapshots.
//!
//! One agent turn can rewrite a dozen files before anyone sees the result, and
//! `git diff` answers "what changed in the workspace", not "what did *this*
//! turn change". Before each native write the original bytes of the file are
//! captured here, keyed by turn id, so the UI can offer a single "undo this
//! turn" that puts every touched file back — without staging, stashing or
//! otherwise involving the user's git state.
//!
//! Only text is captured. A turn that overwrites something undecodable (an
//! icon, a binary lockfile blob) is marked non-undoable instead of being
//! half-restored, and so is a turn whose originals exceed [`MAX_BYTES`]:
//! refusing to undo is recoverable, a silent partial restore is not.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Originals kept per turn. Past this a turn is "too big to undo".
const MAX_BYTES: usize = 8 * 1024 * 1024;

struct Saved {
    abs: PathBuf,
    rel: String,
    before: Option<String>,
}

#[derive(Default)]
pub struct Turn {
    saved: Vec<Saved>,
    seen: HashSet<PathBuf>,
    bytes: usize,
    blocked: Option<String>,
}

impl Turn {
    /// Capture a file right before a tool writes it. Only the first write to a
    /// path is captured: undo must return the file to its state before the
    /// turn, not to the state between the second and third write.
    pub fn capture(&mut self, abs: &Path, workspace: &Path) {
        if !self.seen.insert(abs.to_path_buf()) {
            return;
        }
        let rel = abs
            .strip_prefix(workspace)
            .unwrap_or(abs)
            .to_string_lossy()
            .replace('\\', "/");

        match std::fs::read(abs) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => {
                    self.bytes += text.len();
                    if self.bytes > MAX_BYTES {
                        self.blocked = Some(format!(
                            "本轮原始内容超过 {} MB，无法安全撤销",
                            MAX_BYTES / 1024 / 1024
                        ));
                        return;
                    }
                    self.saved.push(Saved { abs: abs.to_path_buf(), rel, before: Some(text) });
                }
                Err(_) => {
                    // Binary file. Restoring it as text would corrupt it.
                    self.blocked = Some(format!("{rel} 不是文本文件，无法撤销"));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.saved.push(Saved { abs: abs.to_path_buf(), rel, before: None });
            }
            Err(e) => {
                self.blocked = Some(format!("无法读取 {rel}: {e}"));
            }
        }
    }

    /// Workspace-relative paths touched so far, in the order they were first
    /// written — what the UI lists under the turn.
    pub fn changed(&self) -> Vec<String> {
        self.saved.iter().map(|s| s.rel.clone()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.saved.is_empty()
    }

    pub fn undoable(&self) -> bool {
        self.blocked.is_none() && !self.saved.is_empty()
    }

    pub fn blocked(&self) -> Option<&str> {
        self.blocked.as_deref()
    }

    /// Restore every captured file, newest first. Returns the paths that were
    /// put back so the caller can tell the user what moved.
    pub fn undo(&self) -> Result<Vec<String>, String> {
        if let Some(reason) = &self.blocked {
            return Err(reason.clone());
        }
        let mut done = Vec::with_capacity(self.saved.len());
        for s in self.saved.iter().rev() {
            match &s.before {
                Some(text) => {
                    if let Some(dir) = s.abs.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    std::fs::write(&s.abs, text.as_bytes())
                        .map_err(|e| format!("无法还原 {}: {e}", s.rel))?;
                }
                None => {
                    if s.abs.exists() {
                        std::fs::remove_file(&s.abs)
                            .map_err(|e| format!("无法删除本轮新建的 {}: {e}", s.rel))?;
                    }
                }
            }
            done.push(s.rel.clone());
        }
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("irixen-snap-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn undo_restores_edits_and_deletes_new_files() {
        let ws = scratch("undo");
        let edited = ws.join("a.txt");
        let created = ws.join("nested/b.txt");
        std::fs::write(&edited, "before").unwrap();

        let mut turn = Turn::default();
        turn.capture(&edited, &ws);
        turn.capture(&created, &ws);
        assert_eq!(turn.changed().len(), 2);
        assert!(turn.undoable());

        // The turn runs: one file rewritten, one created.
        std::fs::write(&edited, "after").unwrap();
        std::fs::create_dir_all(created.parent().unwrap()).unwrap();
        std::fs::write(&created, "new").unwrap();

        let done = turn.undo().unwrap();
        assert_eq!(done, vec!["nested/b.txt".to_string(), "a.txt".to_string()]);
        assert_eq!(std::fs::read_to_string(&edited).unwrap(), "before");
        assert!(!created.exists(), "a file the turn created must be gone again");

        let _ = std::fs::remove_dir_all(&ws);
    }

    /// Re-capturing on the second write would undo to the middle of the turn.
    #[test]
    fn only_the_first_write_to_a_path_is_kept() {
        let ws = scratch("first");
        let f = ws.join("a.txt");
        std::fs::write(&f, "one").unwrap();

        let mut turn = Turn::default();
        turn.capture(&f, &ws);
        std::fs::write(&f, "two").unwrap();
        turn.capture(&f, &ws);
        std::fs::write(&f, "three").unwrap();

        assert_eq!(turn.changed().len(), 1);
        turn.undo().unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "one");

        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn a_binary_overwrite_makes_the_whole_turn_non_undoable() {
        let ws = scratch("binary");
        let bin = ws.join("icon.png");
        std::fs::write(&bin, [0x89, 0x50, 0x4e, 0x47, 0xff, 0xfe]).unwrap();

        let mut turn = Turn::default();
        turn.capture(&bin, &ws);
        assert!(!turn.undoable());
        assert!(turn.blocked().is_some());
        assert!(turn.undo().is_err());

        let _ = std::fs::remove_dir_all(&ws);
    }
}
