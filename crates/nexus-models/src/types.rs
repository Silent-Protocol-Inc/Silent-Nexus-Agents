//! Provider-neutral chat and completion types.

use nexus_core::config::LimitSource;
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
    /// Provider-private continuation state. It is intentionally never
    /// serialized, persisted, exported, or rendered.
    #[serde(skip)]
    pub provider_private: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            provider_private: None,
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            provider_private: None,
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            provider_private: None,
        }
    }
    pub fn tool_result(call_id: &str, name: &str, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: Some(call_id.to_string()),
            name: Some(name.to_string()),
            provider_private: None,
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

/// Provider-neutral request accepted by every model adapter.
///
/// The selected model is carried separately as a [`ModelReference`] by the
/// registry/manager. This keeps provider credentials and endpoint details out
/// of the request payload that can be logged or persisted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelRequest {
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

/// Provider-neutral response returned by every model adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCallRequest>,
    pub usage: Usage,
    /// Provider's stated finish reason (`stop`, `length`, `tool_calls`, …).
    pub finish_reason: String,
    /// Ephemeral provider-private continuation state for the current request.
    #[serde(skip)]
    pub provider_private: Option<String>,
}

/// Stable, serializable reference to a configured model.
///
/// This intentionally identifies a registry entry and model name only. It
/// never contains an endpoint credential or provider-specific request data.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelReference {
    /// Configured provider/connection name in the NEXUS model registry.
    pub provider: String,
    /// Provider model identifier.
    pub model: String,
}

impl ModelReference {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }
}

/// Backward-compatible 1.x name for [`ModelRequest`].
pub type CompletionRequest = ModelRequest;

/// Backward-compatible 1.x name for [`ModelResponse`].
pub type Completion = ModelResponse;

/// Incremental streaming event.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A chunk of assistant text.
    TextDelta(String),
    /// Ephemeral provider-private state; consumers must never display or
    /// persist it.
    ProviderPrivateDelta(String),
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

/// Whether model inference happens on this host or across a network boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelLocality {
    Local,
    Remote,
    Hybrid,
    #[default]
    Unknown,
}

/// Static data-handling posture advertised by a configured model endpoint.
///
/// Runtime policy remains authoritative: this metadata is descriptive and
/// must never be treated as permission to disclose scoped context.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPrivacy {
    /// Inference is performed locally and request data does not leave the host.
    LocalOnly,
    /// A user-managed endpoint whose privacy terms are configured separately.
    EndpointControlled,
    /// A hosted provider processes request data under its own policy.
    ProviderManaged,
    #[default]
    Unknown,
}

/// Coarse latency class for menu filtering and capability-aware routing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelLatencyClass {
    Low,
    Standard,
    High,
    #[default]
    Unknown,
}

/// Coarse cost class; exact prices belong in provider configuration, not in
/// the provider-neutral capability record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCostClass {
    Free,
    Low,
    Standard,
    High,
    #[default]
    Unknown,
}

/// Static fallback suitability. A runtime privacy/policy check is still
/// required even when a model is marked eligible.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackEligibility {
    Eligible,
    ApprovalRequired,
    Ineligible,
    #[default]
    Unknown,
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
    #[serde(default)]
    pub context_limit_source: LimitSource,
    #[serde(default)]
    pub output_limit_source: LimitSource,
    /// Reasoning-effort style controls, when the endpoint honors them.
    pub reasoning_controls: bool,
    /// Whether the adapter preserves dedicated system-role instructions.
    #[serde(default)]
    pub system_prompt: bool,
    /// Whether multiple tool calls may be requested in one model turn.
    #[serde(default)]
    pub parallel_tool_calls: bool,
    /// Whether the adapter can enforce a supplied JSON Schema response.
    #[serde(default)]
    pub json_schema: bool,
    pub local: bool,
    /// Compute accelerator available for this model. For local providers this
    /// reflects host GPU detection: `Some("CUDA")`, `Some("Metal")`, … when a
    /// GPU is present, `Some("CPU")` when the model runs locally without one,
    /// and `None` for remote endpoints (where the harness cannot know).
    #[serde(default)]
    pub accelerator: Option<String>,
    #[serde(default)]
    pub locality: ModelLocality,
    #[serde(default)]
    pub privacy: ModelPrivacy,
    #[serde(default)]
    pub latency_class: ModelLatencyClass,
    #[serde(default)]
    pub cost_class: ModelCostClass,
    #[serde(default)]
    pub fallback_eligibility: FallbackEligibility,
}

impl ModelCapabilities {
    /// Whether the harness must treat this model as constrained: small
    /// context window or no native tool calls. Structured JSON output is an
    /// optimization, not a prerequisite when native tool calls are available.
    /// Constrained models get smaller task bundles, tighter context budgets,
    /// and shorter turn limits without losing essential tools.
    pub fn constrained(&self) -> bool {
        self.context_window < 32_000 || !self.native_tool_calls
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_capability_documents_receive_safe_metadata_defaults() {
        let capabilities: ModelCapabilities = serde_json::from_value(serde_json::json!({
            "model_id": "legacy-model",
            "provider_kind": "legacy-provider",
            "streaming": true,
            "native_tool_calls": false,
            "structured_output": false,
            "image_input": false,
            "embeddings": false,
            "context_window": 4096,
            "max_output_tokens": 512,
            "reasoning_controls": false,
            "local": false
        }))
        .expect("legacy capabilities deserialize");

        assert!(!capabilities.system_prompt);
        assert!(!capabilities.parallel_tool_calls);
        assert!(!capabilities.json_schema);
        assert_eq!(capabilities.locality, ModelLocality::Unknown);
        assert_eq!(capabilities.privacy, ModelPrivacy::Unknown);
        assert_eq!(capabilities.latency_class, ModelLatencyClass::Unknown);
        assert_eq!(capabilities.cost_class, ModelCostClass::Unknown);
        assert_eq!(
            capabilities.fallback_eligibility,
            FallbackEligibility::Unknown
        );
    }

    #[test]
    fn native_tools_do_not_require_structured_output_to_be_unconstrained() {
        let mut capabilities: ModelCapabilities = serde_json::from_value(serde_json::json!({
            "model_id": "codex", "provider_kind": "codex", "streaming": true,
            "native_tool_calls": true, "structured_output": false,
            "image_input": false, "embeddings": false, "context_window": 128000,
            "max_output_tokens": 4096, "reasoning_controls": true, "local": false
        }))
        .expect("capabilities");
        assert!(!capabilities.constrained());
        capabilities.native_tool_calls = false;
        assert!(capabilities.constrained());
    }

    #[test]
    fn completion_names_remain_source_compatible_aliases() {
        let request: CompletionRequest = ModelRequest::default();
        let _: ModelRequest = request;

        let response = ModelResponse {
            content: "ok".into(),
            tool_calls: Vec::new(),
            usage: Usage::default(),
            finish_reason: "stop".into(),
            provider_private: None,
        };
        let legacy: Completion = response;
        let _: ModelResponse = legacy;
    }

    #[test]
    fn model_reference_contains_no_endpoint_or_credentials() {
        let reference = ModelReference::new("primary", "model-a");
        let serialized = serde_json::to_value(&reference).expect("serialize reference");
        assert_eq!(serialized["provider"], "primary");
        assert_eq!(serialized["model"], "model-a");
        assert_eq!(serialized.as_object().map(|o| o.len()), Some(2));
    }

    #[test]
    fn provider_private_state_is_never_serialized() {
        let mut message = ChatMessage::assistant("visible");
        message.provider_private = Some("hidden".into());
        let serialized = serde_json::to_string(&message).expect("serialize message");
        assert!(!serialized.contains("hidden"));
        assert!(!serialized.contains("provider_private"));
    }
}
