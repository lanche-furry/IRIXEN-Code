//! Shared application state handed to every Tauri command.

use crate::agent::AgentEngine;
use crate::config::AppConfig;
use crate::mcp::McpHost;
use crate::skills::SkillRegistry;
use std::sync::{Arc, Mutex};

pub struct AppState {
    pub config: Arc<Mutex<AppConfig>>,
    pub engine: Arc<AgentEngine>,
    pub skills: Arc<SkillRegistry>,
    pub mcp: Arc<McpHost>,
}

impl AppState {
    pub fn workspace(&self) -> Result<std::path::PathBuf, String> {
        let ws = crate::util::lock(&self.config).workspace.clone();
        match ws {
            Some(w) if !w.trim().is_empty() => Ok(std::path::PathBuf::from(w)),
            _ => Err("尚未打开工作区".to_string()),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        crate::util::lock(&self.config).save().map_err(|e| e.to_string())
    }
}
