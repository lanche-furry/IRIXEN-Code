//! Skill system: installable prompt packs that make the agent cheaper and
//! sharper. Skills are plain `SKILL.md` files with YAML-ish frontmatter.
//!
//! Built-ins are embedded at compile time (zero disk cost, always available).
//! User skills live in `<config>/skills/<id>/SKILL.md` and can be installed
//! from a local folder, a single `.md` URL, or a git repository — all from the
//! Skills panel inside the app.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Keywords that auto-activate the skill when they appear in the prompt.
    pub when: Vec<String>,
    pub body: String,
    pub builtin: bool,
    pub enabled: bool,
    pub path: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Frontmatter {
    name: String,
    description: String,
    when: Vec<String>,
}

const BUILTINS: &[(&str, &str)] = &[
    ("token-saver", include_str!("skills/builtin/token-saver.md")),
    ("commit-craft", include_str!("skills/builtin/commit-craft.md")),
    ("repo-scout", include_str!("skills/builtin/repo-scout.md")),
    ("diff-surgeon", include_str!("skills/builtin/diff-surgeon.md")),
    ("code-reviewer", include_str!("skills/builtin/code-reviewer.md")),
];

fn parse_frontmatter(text: &str) -> (Frontmatter, String) {
    let mut fm = Frontmatter::default();
    let trimmed = text.trim_start();
    if !trimmed.starts_with("---") {
        return (fm, text.to_string());
    }
    let rest = &trimmed[3..];
    let end = match rest.find("\n---") {
        Some(i) => i,
        None => return (fm, text.to_string()),
    };
    let header = &rest[..end];
    let body = rest[end + 4..].trim_start_matches('\n').to_string();
    for line in header.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let value = v.trim().trim_matches('"').to_string();
            match key.as_str() {
                "name" => fm.name = value,
                "description" => fm.description = value,
                "when" | "keywords" | "triggers" => {
                    fm.when = value
                        .split([',', '，'])
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                _ => {}
            }
        }
    }
    (fm, body)
}

fn skill_from_text(id: &str, text: &str, builtin: bool, path: Option<PathBuf>) -> Skill {
    let (fm, body) = parse_frontmatter(text);
    Skill {
        id: id.to_string(),
        name: if fm.name.is_empty() {
            id.to_string()
        } else {
            fm.name
        },
        description: fm.description,
        when: fm.when,
        body,
        builtin,
        enabled: true,
        path: path.map(|p| p.to_string_lossy().to_string()),
    }
}

pub struct SkillRegistry {
    skills: std::sync::Mutex<Vec<Skill>>,
}

impl SkillRegistry {
    pub fn load(enabled_ids: &[String], auto_match: bool) -> Self {
        let mut skills: Vec<Skill> = Vec::new();
        for (id, text) in BUILTINS {
            let mut s = skill_from_text(id, text, true, None);
            s.enabled = enabled_ids.is_empty() || enabled_ids.iter().any(|e| e == id);
            skills.push(s);
        }
        // user skills
        let dir = crate::config::skills_dir();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let folder = entry.path();
                let md = if folder.is_dir() {
                    folder.join("SKILL.md")
                } else {
                    folder.clone()
                };
                if !md.exists() {
                    continue;
                }
                let id = folder
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if let Ok(text) = std::fs::read_to_string(&md) {
                    let mut s = skill_from_text(&id, &text, false, Some(md.clone()));
                    s.enabled = enabled_ids.iter().any(|e| e == &id);
                    skills.push(s);
                }
            }
        }
        let _ = auto_match;
        SkillRegistry {
            skills: std::sync::Mutex::new(skills),
        }
    }

    pub fn list(&self) -> Vec<Skill> {
        let mut v = crate::util::lock(&self.skills).clone();
        v.sort_by(|a, b| b.builtin.cmp(&a.builtin).then(a.id.cmp(&b.id)));
        v
    }

    pub fn get(&self, id: &str) -> Option<Skill> {
        crate::util::lock(&self.skills)
            .iter()
            .find(|s| s.id == id)
            .cloned()
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> bool {
        let mut list = crate::util::lock(&self.skills);
        if let Some(s) = list.iter_mut().find(|s| s.id == id) {
            s.enabled = enabled;
            return true;
        }
        false
    }

    /// Skills whose keywords appear in the prompt (or that are explicitly
    /// requested with `/skill-id`).
    pub fn match_for(&self, text: &str, auto_match: bool) -> Vec<Skill> {
        let lower = text.to_lowercase();
        let list = crate::util::lock(&self.skills);
        let mut out = Vec::new();
        for s in list.iter().filter(|s| s.enabled) {
            if lower.contains(&format!("/{}", s.id)) || lower.contains(&format!("@{}", s.id)) {
                out.push(s.clone());
                continue;
            }
            if !auto_match {
                continue;
            }
            if s.when.iter().any(|kw| {
                let kw = kw.to_lowercase();
                !kw.is_empty() && lower.contains(&kw)
            }) {
                out.push(s.clone());
            }
        }
        out
    }

    pub fn install_from_text(&self, hint_id: &str, text: &str) -> Result<Skill, String> {
        let id = sanitize_id(hint_id);
        if id.is_empty() {
            return Err("cannot derive a skill id from the source".into());
        }
        let dir = crate::config::skills_dir().join(&id);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let md = dir.join("SKILL.md");
        std::fs::write(&md, text).map_err(|e| e.to_string())?;
        let skill = skill_from_text(&id, text, false, Some(md));
        self.upsert(skill.clone());
        Ok(skill)
    }

    fn upsert(&self, skill: Skill) {
        let mut list = crate::util::lock(&self.skills);
        if let Some(existing) = list.iter_mut().find(|s| s.id == skill.id) {
            *existing = skill;
        } else {
            list.push(skill);
        }
    }

    pub fn uninstall(&self, id: &str) -> Result<(), String> {
        let skill = self.get(id).ok_or_else(|| format!("skill '{id}' not found"))?;
        if skill.builtin {
            return Err("built-in skills cannot be removed — disable them instead".into());
        }
        let dir = crate::config::skills_dir().join(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        }
        crate::util::lock(&self.skills).retain(|s| s.id != id);
        Ok(())
    }

    /// Install a skill from a local path, an http(s) `.md` URL, or a git repo.
    pub async fn install(&self, source: &str) -> Result<Skill, String> {
        let src = source.trim();
        if src.is_empty() {
            return Err("empty source".into());
        }
        // 1. local path
        let p = Path::new(src);
        if p.exists() {
            if p.is_dir() {
                let md = p.join("SKILL.md");
                let file = if md.exists() { md } else { first_md(p)? };
                let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
                let id = p
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "skill".into());
                return self.install_from_text(&id, &text);
            }
            let text = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
            let id = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "skill".into());
            return self.install_from_text(&id, &text);
        }
        // 2. single markdown file over http(s)
        if src.starts_with("http://") || src.starts_with("https://") {
            let client = reqwest::Client::new();
            let resp = client
                .get(src)
                .send()
                .await
                .map_err(|e| format!("download failed: {e}"))?;
            if !resp.status().is_success() {
                return Err(format!("download failed: HTTP {}", resp.status()));
            }
            let text = resp.text().await.map_err(|e| e.to_string())?;
            let id = src
                .rsplit('/')
                .next()
                .unwrap_or("skill")
                .trim_end_matches(".md")
                .to_string();
            return self.install_from_text(&id, &text);
        }
        // 3. git repository
        let tmp = std::env::temp_dir().join(format!("irixen-skill-{}", crate::util::short_id()));
        let status = tokio::process::Command::new("git")
            .args(["clone", "--depth", "1", src])
            .arg(&tmp)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .status()
            .await
            .map_err(|e| format!("cannot run git: {e}"))?;
        if !status.success() {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err("git clone failed — check the URL".into());
        }
        let md = tmp.join("SKILL.md");
        let candidate = if md.exists() {
            md
        } else {
            match first_md(&tmp) {
                Ok(p) => p,
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&tmp);
                    return Err(e);
                }
            }
        };
        let text = std::fs::read_to_string(&candidate).map_err(|e| e.to_string())?;
        let id = src
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("skill")
            .trim_end_matches(".git")
            .to_string();
        let _ = std::fs::remove_dir_all(&tmp);
        self.install_from_text(&id, &text)
    }
}

fn first_md(dir: &Path) -> Result<PathBuf, String> {
    let mut found = None;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "md").unwrap_or(false) {
                found = Some(p);
                break;
            }
        }
    }
    found.ok_or_else(|| "no SKILL.md or .md file found in source".to_string())
}

fn sanitize_id(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}
