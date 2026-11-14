//! Minimal but complete MCP client: JSON-RPC 2.0 over stdio.
//!
//! One process per server, spawned on demand, newline-delimited JSON framed.
//! Tools are exposed to the agent as `mcp__<server>__<tool>`.

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use crate::config::McpServerConfig;

#[derive(Clone, Debug, Serialize)]
pub struct McpToolInfo {
    pub server_id: String,
    pub server_name: String,
    pub name: String,
    pub qualified_name: String,
    pub description: String,
    pub input_schema: Value,
}

pub struct McpConn {
    pub server_id: String,
    pub server_name: String,
    out_tx: mpsc::UnboundedSender<String>,
    pending: Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    next_id: AtomicU64,
    tools: std::sync::Mutex<Vec<McpToolInfo>>,
    child: tokio::sync::Mutex<Option<tokio::process::Child>>,
    stderr_tail: Arc<std::sync::Mutex<Vec<String>>>,
}

impl McpConn {
    async fn send(&self, msg: &Value) -> Result<(), String> {
        let line = msg.to_string();
        self.out_tx
            .send(line)
            .map_err(|_| "mcp server process is gone".to_string())
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        crate::util::lock(&self.pending).insert(id, tx);
        self.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }))
        .await?;
        match tokio::time::timeout(std::time::Duration::from_secs(90), rx).await {
            Ok(Ok(v)) => {
                if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
                    Err(format!("mcp error: {err}"))
                } else {
                    Ok(v.get("result").cloned().unwrap_or(Value::Null))
                }
            }
            Ok(Err(_)) => Err("mcp connection closed".into()),
            Err(_) => Err(format!("mcp '{method}' timed out after 90s")),
        }
    }

    pub fn tools(&self) -> Vec<McpToolInfo> {
        crate::util::lock(&self.tools).clone()
    }

    pub fn stderr_tail(&self) -> Vec<String> {
        crate::util::lock(&self.stderr_tail).clone()
    }

    pub async fn call_tool(&self, tool: &str, args: &Value) -> Result<String, String> {
        let result = self
            .request("tools/call", json!({ "name": tool, "arguments": args }))
            .await?;
        let is_error = result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mut text = String::new();
        if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
            for part in content {
                match part.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t);
                        }
                    }
                    Some("resource") => {
                        text.push_str(&format!(
                            "[resource] {}",
                            part.get("resource").cloned().unwrap_or(Value::Null)
                        ));
                    }
                    _ => {
                        text.push_str(&part.to_string());
                    }
                }
            }
        } else {
            text = result.to_string();
        }
        if is_error {
            Err(text)
        } else {
            Ok(crate::util::cap_chars(&text, 16_000))
        }
    }

    pub async fn shutdown(&self) {
        let mut guard = self.child.lock().await;
        if let Some(child) = guard.as_mut() {
            let _ = child.kill().await;
        }
        *guard = None;
        self.out_tx.send("__IRIXEN_EOF__".into()).ok();
    }
}

pub struct McpHost {
    conns: std::sync::Mutex<HashMap<String, Arc<McpConn>>>,
}

impl Default for McpHost {
    fn default() -> Self {
        McpHost::new()
    }
}

impl McpHost {
    pub fn new() -> Self {
        McpHost {
            conns: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn running(&self) -> Vec<String> {
        crate::util::lock(&self.conns).keys().cloned().collect()
    }

    pub fn tools_all(&self) -> Vec<McpToolInfo> {
        let conns = crate::util::lock(&self.conns);
        let mut out = Vec::new();
        for c in conns.values() {
            out.extend(c.tools());
        }
        out
    }

    pub fn stderr_tail(&self, server_id: &str) -> Vec<String> {
        crate::util::lock(&self.conns)
            .get(server_id)
            .map(|c| c.stderr_tail())
            .unwrap_or_default()
    }

    pub async fn start(&self, cfg: &McpServerConfig) -> Result<Vec<McpToolInfo>, String> {
        if let Some(existing) = crate::util::lock(&self.conns).get(&cfg.id) {
            return Ok(existing.tools());
        }

        let mut cmd = tokio::process::Command::new(&cfg.command);
        cmd.args(&cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(first_err) => {
                // Windows: npx/npm/uvx style shims need cmd /C.
                if cfg!(windows) {
                    let mut sh = tokio::process::Command::new("cmd");
                    sh.arg("/C")
                        .arg(&cfg.command)
                        .args(&cfg.args)
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .kill_on_drop(true);
                    for (k, v) in &cfg.env {
                        sh.env(k, v);
                    }
                    sh.spawn().map_err(|e| {
                        format!("cannot start '{}': {e} (first attempt: {first_err})", cfg.command)
                    })?
                } else {
                    return Err(format!("cannot start '{}': {first_err}", cfg.command));
                }
            }
        };

        let stdin = child.stdin.take().ok_or("mcp: no stdin")?;
        let stdout = child.stdout.take().ok_or("mcp: no stdout")?;
        let stderr = child.stderr.take();

        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
        // writer
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(line) = out_rx.recv().await {
                if line == "__IRIXEN_EOF__" {
                    break;
                }
                let mut payload = line;
                payload.push('\n');
                if stdin.write_all(payload.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        let pending: Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Value>>>> =
            Arc::new(std::sync::Mutex::new(HashMap::new()));
        let pending_reader = pending.clone();
        let out_tx_reader = out_tx.clone();
        // reader
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let v: Value = match serde_json::from_str(trimmed) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let id = v.get("id").and_then(|i| i.as_u64());
                match (id, v.get("method").and_then(|m| m.as_str())) {
                    (Some(id), Some(method)) => {
                        // server → client request; we advertise no extra capabilities
                        let resp = json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": { "code": -32601, "message": format!("IRIXEN Code does not support {method}") }
                        });
                        let _ = out_tx_reader.send(resp.to_string());
                    }
                    (Some(id), None) => {
                        if let Some(tx) = crate::util::lock(&pending_reader).remove(&id) {
                            let _ = tx.send(v);
                        }
                    }
                    _ => {}
                }
            }
        });

        let stderr_tail = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        if let Some(err) = stderr {
            let tail = stderr_tail.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(err).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut t = crate::util::lock(&tail);
                    t.push(crate::util::one_line(&line, 400));
                    if t.len() > 60 {
                        t.remove(0);
                    }
                }
            });
        }

        let conn = Arc::new(McpConn {
            server_id: cfg.id.clone(),
            server_name: cfg.name.clone(),
            out_tx,
            pending,
            next_id: AtomicU64::new(0),
            tools: std::sync::Mutex::new(Vec::new()),
            child: tokio::sync::Mutex::new(Some(child)),
            stderr_tail,
        });

        // Handshake
        let init = conn
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "roots": { "listChanged": false } },
                    "clientInfo": { "name": "IRIXEN Code", "version": env!("CARGO_PKG_VERSION") }
                }),
            )
            .await;
        if let Err(e) = init {
            let tail = conn.stderr_tail().join("\n");
            conn.shutdown().await;
            return Err(format!("{e}\n{tail}"));
        }
        let _ = conn
            .send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await;

        let tools = conn.request("tools/list", json!({})).await?;
        let mut infos = Vec::new();
        if let Some(arr) = tools.get("tools").and_then(|t| t.as_array()) {
            for t in arr {
                let name = t
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                infos.push(McpToolInfo {
                    server_id: cfg.id.clone(),
                    server_name: cfg.name.clone(),
                    qualified_name: format!("mcp__{}__{}", cfg.id, name),
                    name: name.clone(),
                    description: t
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string(),
                    input_schema: t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
                });
            }
        }
        *crate::util::lock(&conn.tools) = infos.clone();
        crate::util::lock(&self.conns).insert(cfg.id.clone(), conn);
        Ok(infos)
    }

    pub async fn stop(&self, server_id: &str) {
        let conn = crate::util::lock(&self.conns).remove(server_id);
        if let Some(c) = conn {
            c.shutdown().await;
        }
    }

    pub async fn stop_all(&self) {
        let conns: Vec<Arc<McpConn>> = crate::util::lock(&self.conns)
            .drain()
            .map(|(_, v)| v)
            .collect();
        for c in conns {
            c.shutdown().await;
        }
    }

    pub async fn call(
        &self,
        qualified_name: &str,
        args: &Value,
    ) -> Result<String, String> {
        // mcp__<server>__<tool>
        let rest = qualified_name
            .strip_prefix("mcp__")
            .ok_or_else(|| format!("not an mcp tool: {qualified_name}"))?;
        let (server_id, tool) = rest
            .split_once("__")
            .ok_or_else(|| format!("malformed mcp tool name: {qualified_name}"))?;
        let conn = crate::util::lock(&self.conns)
            .get(server_id)
            .cloned()
            .ok_or_else(|| format!("mcp server '{server_id}' is not running"))?;
        conn.call_tool(tool, args).await
    }
}
