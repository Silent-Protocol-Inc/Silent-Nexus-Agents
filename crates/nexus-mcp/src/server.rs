//! MCP server mode: expose approved Silent Nexus capabilities to other hosts.
//!
//! `snx mcp serve` runs this over stdio. It exposes only an explicitly
//! curated, safe subset of capabilities — never destructive, privileged, or
//! terminal tools — and every exposed tool is described here rather than
//! auto-derived from the full registry, so nothing dangerous leaks by default.

use crate::{JsonRpcError, JsonRpcResponse};
use serde::Serialize;

/// A capability this server offers to MCP clients.
#[derive(Debug, Clone, Serialize)]
pub struct ExposedTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// The safe, curated capability set. Read-only by construction.
pub fn exposed_tools() -> Vec<ExposedTool> {
    vec![
        ExposedTool {
            name: "nexus.search_code".into(),
            description: "Search the indexed workspace for a symbol by name.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"]
            }),
        },
        ExposedTool {
            name: "nexus.read_file".into(),
            description: "Read a workspace file (path is workspace-confined).".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        },
        ExposedTool {
            name: "nexus.project_structure".into(),
            description: "Return the project structure overview.".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
        },
    ]
}

/// Handle one JSON-RPC request in server mode. Returns a response value.
/// `dispatch` is supplied by the host to actually run an exposed capability.
pub async fn handle_request<F, Fut>(
    method: &str,
    id: Option<u64>,
    params: Option<serde_json::Value>,
    dispatch: F,
) -> JsonRpcResponse
where
    F: FnOnce(String, serde_json::Value) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    match method {
        "initialize" => ok(
            id,
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "silent-nexus", "version": nexus_core::brand::VERSION}
            }),
        ),
        "notifications/initialized" => ok(id, serde_json::json!({})),
        "tools/list" => ok(id, serde_json::json!({"tools": exposed_tools()})),
        "tools/call" => {
            let name = params
                .as_ref()
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            if !exposed_tools().iter().any(|t| t.name == name) {
                return err(id, -32601, &format!("tool `{name}` is not exposed"));
            }
            let args = params
                .and_then(|p| p.get("arguments").cloned())
                .unwrap_or(serde_json::json!({}));
            match dispatch(name, args).await {
                Ok(text) => ok(
                    id,
                    serde_json::json!({"content": [{"type": "text", "text": text}]}),
                ),
                Err(e) => err(id, -32000, &e),
            }
        }
        other => err(id, -32601, &format!("method `{other}` not found")),
    }
}

fn ok(id: Option<u64>, result: serde_json::Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: Some("2.0".into()),
        id,
        result: Some(result),
        error: None,
    }
}

fn err(id: Option<u64>, code: i64, message: &str) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: Some("2.0".into()),
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exposes_only_safe_tools() {
        let tools = exposed_tools();
        // No destructive/terminal capabilities exposed.
        assert!(tools.iter().all(|t| !t.name.contains("delete")
            && !t.name.contains("terminal")
            && !t.name.contains("run")));
    }

    #[tokio::test]
    async fn rejects_unexposed_tool_call() {
        let resp = handle_request(
            "tools/call",
            Some(1),
            Some(serde_json::json!({"name": "nexus.delete_everything"})),
            |_n, _a| async { Ok(String::new()) },
        )
        .await;
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn dispatches_exposed_tool() {
        let resp = handle_request(
            "tools/call",
            Some(2),
            Some(serde_json::json!({"name": "nexus.read_file", "arguments": {"path": "a"}})),
            |name, _args| async move { Ok(format!("dispatched {name}")) },
        )
        .await;
        assert!(resp.error.is_none());
        let text = resp.result.expect("result")["content"][0]["text"]
            .as_str()
            .expect("text")
            .to_string();
        assert!(text.contains("dispatched nexus.read_file"));
    }

    #[tokio::test]
    async fn initialize_reports_server_info() {
        let resp = handle_request("initialize", Some(0), None, |_n, _a| async {
            Ok(String::new())
        })
        .await;
        assert!(resp.result.is_some());
        assert_eq!(
            resp.result.expect("result")["serverInfo"]["name"].as_str(),
            Some("silent-nexus")
        );
    }
}
