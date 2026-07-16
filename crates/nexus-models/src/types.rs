//! Provider-neutral chat and completion types.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// One requested tool invocation inside an assistant message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// Provider-assigned or harness-assigned call id.
    pub id: String,
    /// Tool name as exposed to the model.
    pub name: String,
    /// Raw JSON arguments string (validated downstream against the schema).
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// Tool calls requested by an assistant message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRequest>,
    /// For `Role::Tool` messages: the call this responds to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// For `Role::Tool` messages: the tool name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
        }
    }
    pub fn tool_result(call_id: &str, name: &str, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: Some(call_id.to_string()),
            name: Some(name.to_string()),
        }
    }
}

/// Tool schema surfaced to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments object.
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub messages: Vec<ChatMessage>,
    /// Tools the model may call this turn (already filtered to a minimal set).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    /// Ask the provider for a JSON object response when supported.
    #[serde(default)]
    pub json_mode: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Completion {
    pub content: String,
    pub tool_calls: Vec<ToolCallRequest>,
    pub usage: Usage,
    /// Provider's stated finish reason (`stop`, `length`, `tool_calls`, …).
    pub finish_reason: String,
}

/// Incremental streaming event.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A chunk of assistant text.
    TextDelta(String),
    /// A chunk of a tool call being assembled. `index` groups deltas of the
    /// same call; name/arguments accumulate.
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: String,
    },
    /// Terminal event carrying final usage and finish reason.
    Done { usage: Usage, finish_reason: String },
}

/// Static capabilities a provider declares for a configured model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub model_id: String,
    pub provider_kind: String,
    pub streaming: bool,
    pub native_tool_calls: bool,
    pub structured_output: bool,
    pub image_input: bool,
    pub embeddings: bool,
    pub context_window: usize,
    pub max_output_tokens: usize,
    /// Reasoning-effort style controls, when the endpoint honors them.
    pub reasoning_controls: bool,
    pub local: bool,
    /// Compute accelerator available for this model. For local providers this
    /// reflects host GPU detection: `Some("CUDA")`, `Some("Metal")`, … when a
    /// GPU is present, `Some("CPU")` when the model runs locally without one,
    /// and `None` for remote endpoints (where the harness cannot know).
    #[serde(default)]
    pub accelerator: Option<String>,
}

/// Result of a provider health probe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealth {
    pub reachable: bool,
    pub detail: String,
    pub latency_ms: Option<u64>,
}

/// Task class used for routing (deterministic fallback classification lives
/// in nexus-agent; this is the shared vocabulary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskClass {
    Simple,
    Coding,
    Planning,
    Research,
    Verification,
}

impl TaskClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskClass::Simple => "simple",
            TaskClass::Coding => "coding",
            TaskClass::Planning => "planning",
            TaskClass::Research => "research",
            TaskClass::Verification => "verification",
        }
    }
}
