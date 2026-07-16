//! MCP stdio client.

use crate::{JsonRpcRequest, JsonRpcResponse};
use nexus_core::redact::Redactor;
use nexus_core::{NexusError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

/// A tool discovered from an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// A live connection to an MCP server over stdio.
pub struct McpClient {
    server_name: String,
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<BufReader<ChildStdout>>,
    next_id: AtomicU64,
    timeout: Duration,
    redactor: Redactor,
}

impl McpClient {
    /// Launch an MCP server subprocess with a scrubbed environment and
    /// perform the `initialize` handshake.
    pub async fn connect_stdio(
        server_name: &str,
        command: &str,
        args: &[String],
        env_allowlist: &[String],
        timeout_secs: u64,
    ) -> Result<Self> {
        if command.is_empty() {
            return Err(NexusError::Other("MCP command is empty".into()));
        }
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env_clear()
            .kill_on_drop(true);
        // Forward only allowlisted, non-sensitive env vars.
        for key in env_allowlist {
            if Redactor::is_sensitive_env_key(key) {
                continue;
            }
            if let Ok(v) = std::env::var(key) {
                cmd.env(key, v);
            }
        }
        let mut child = cmd.spawn().map_err(|e| {
            NexusError::Other(format!(
                "failed to launch MCP server `{server_name}` ({command}): {e}"
            ))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| NexusError::Other("no stdin for MCP server".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| NexusError::Other("no stdout for MCP server".into()))?;

        let client = Self {
            server_name: server_name.to_string(),
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout)),
            next_id: AtomicU64::new(1),
            timeout: Duration::from_secs(timeout_secs.max(1)),
            redactor: Redactor::new(),
        };

        // MCP initialize handshake.
        let init = client
            .request(
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "silent-nexus", "version": nexus_core::brand::VERSION}
                })),
            )
            .await?;
        if init.error.is_some() {
            return Err(NexusError::Other(format!(
                "MCP initialize failed: {}",
                init.error.map(|e| e.message).unwrap_or_default()
            )));
        }
        // Notify initialized (no response expected).
        client.notify("notifications/initialized", None).await?;
        Ok(client)
    }

    async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = JsonRpcRequest::new(id, method, params);
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|e| NexusError::Other(format!("MCP write: {e}")))?;
            stdin.flush().await.ok();
        }
        // Read responses until we see the matching id (skip notifications).
        let read = async {
            let mut stdout = self.stdout.lock().await;
            loop {
                let mut buf = String::new();
                let n = stdout
                    .read_line(&mut buf)
                    .await
                    .map_err(|e| NexusError::Other(format!("MCP read: {e}")))?;
                if n == 0 {
                    return Err(NexusError::Other(format!(
                        "MCP server `{}` closed the connection",
                        self.server_name
                    )));
                }
                let trimmed = buf.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<JsonRpcResponse>(trimmed) {
                    Ok(resp) if resp.id == Some(id) => return Ok(resp),
                    Ok(_) => continue, // notification or other id
                    Err(_) => continue,
                }
            }
        };
        tokio::time::timeout(self.timeout, read)
            .await
            .map_err(|_| NexusError::Other(format!("MCP request `{method}` timed out")))?
    }

    async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> Result<()> {
        // Notifications have no id.
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or(serde_json::json!({}))
        });
        let mut line = serde_json::to_string(&payload)?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| NexusError::Other(format!("MCP notify: {e}")))?;
        stdin.flush().await.ok();
        Ok(())
    }

    /// Discover the server's tools.
    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        let resp = self.request("tools/list", None).await?;
        if let Some(err) = resp.error {
            return Err(NexusError::Other(format!(
                "tools/list error: {}",
                err.message
            )));
        }
        let tools = resp
            .result
            .and_then(|r| r.get("tools").cloned())
            .and_then(|t| t.as_array().cloned())
            .unwrap_or_default();
        Ok(tools
            .into_iter()
            .filter_map(|t| {
                Some(McpTool {
                    name: t.get("name")?.as_str()?.to_string(),
                    description: t
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string(),
                    input_schema: t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or(serde_json::json!({"type": "object"})),
                })
            })
            .collect())
    }

    /// List resource URIs the server exposes.
    pub async fn list_resources(&self) -> Result<Vec<String>> {
        let resp = self.request("resources/list", None).await?;
        Ok(resp
            .result
            .and_then(|r| r.get("resources").cloned())
            .and_then(|r| r.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| r.get("uri").and_then(|u| u.as_str()).map(String::from))
            .collect())
    }

    /// Call a tool. Output is redacted before returning to the caller.
    pub async fn call_tool(&self, name: &str, arguments: serde_json::Value) -> Result<String> {
        let resp = self
            .request(
                "tools/call",
                Some(serde_json::json!({"name": name, "arguments": arguments})),
            )
            .await?;
        if let Some(err) = resp.error {
            return Err(NexusError::ToolFailed {
                tool: format!("mcp:{name}"),
                message: err.message,
            });
        }
        let content = resp
            .result
            .and_then(|r| r.get("content").cloned())
            .and_then(|c| c.as_array().cloned())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| i.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        Ok(self.redactor.redact(&content))
    }

    /// Health probe: re-list tools and report count.
    pub async fn health(&self) -> Result<String> {
        let tools = self.list_tools().await?;
        Ok(format!("ok: {} tools available", tools.len()))
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Shut the server down.
    pub async fn shutdown(&self) {
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
    }
}

/// Environment the client forwards (documented for the registry/UI).
pub fn describe_forwarded_env(env_allowlist: &[String]) -> BTreeMap<String, bool> {
    env_allowlist
        .iter()
        .map(|k| (k.clone(), !Redactor::is_sensitive_env_key(k)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_empty_command() {
        let result = McpClient::connect_stdio("x", "", &[], &[], 5).await;
        match result {
            Ok(_) => panic!("connecting with an empty command must fail"),
            Err(e) => assert!(e.to_string().contains("empty")),
        }
    }

    #[test]
    fn forwarded_env_marks_sensitive() {
        let map = describe_forwarded_env(&["PATH".into(), "OPENAI_API_KEY".into()]);
        assert_eq!(map.get("PATH"), Some(&true));
        assert_eq!(map.get("OPENAI_API_KEY"), Some(&false));
    }
}
