//! nexus-mcp: Model Context Protocol client and server.
//!
//! Client: launches an MCP server as a subprocess, speaks JSON-RPC 2.0 over
//! stdio, discovers capabilities, and imports tools/resources/prompts. Every
//! imported tool is `untrusted` by default — invoking it requires approval —
//! until the user marks the server trusted. Adding or installing a server is
//! always an explicit user action; a model may only *propose* one.
//!
//! Server: exposes a curated, approved subset of Silent Nexus capabilities to
//! other MCP hosts. The server never exposes destructive or privileged tools.

pub mod client;
pub mod registry;
pub mod server;

pub use client::{McpClient, McpTool};
pub use registry::{McpRegistry, McpServerRecord, TrustState};

use serde::{Deserialize, Serialize};

/// JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: &str, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        }
    }
}

/// JSON-RPC 2.0 response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jsonrpc: Option<String>,
    pub id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}
