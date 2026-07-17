//! Model registry, task routing, and provider fallback.

use crate::mock::MockProvider;
use crate::ollama::OllamaProvider;
use crate::openai_compat::OpenAiCompatProvider;
use crate::provider::ModelProvider;
use crate::types::{
    FallbackEligibility, ModelCapabilities, ModelLocality, ModelPrivacy, ModelRequest,
    ModelResponse, ProviderHealth, Role, StreamEvent, TaskClass,
};
use futures::stream::BoxStream;
use nexus_core::config::{Config, ModelConfig, RoutingConfig};
use nexus_core::{NexusError, Result};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Safe, non-sensitive reason a pre-stream fallback was attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreStreamFallbackReason {
    ProviderUnavailable,
    Timeout,
}

impl PreStreamFallbackReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ProviderUnavailable => "provider unavailable",
            Self::Timeout => "provider timeout",
        }
    }
}

/// Metadata describing a fallback that occurred before any stream existed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PreStreamFallback {
    pub from_model: String,
    pub reason: PreStreamFallbackReason,
}

/// Selected model and its live stream. Once returned, NEXUS never changes
/// providers for this request, including when the first stream item is an
/// error.
pub struct RoutedModelStream {
    pub model_name: String,
    pub fallback: Option<PreStreamFallback>,
    pub stream: BoxStream<'static, Result<StreamEvent>>,
}

impl std::fmt::Debug for RoutedModelStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RoutedModelStream")
            .field("model_name", &self.model_name)
            .field("fallback", &self.fallback)
            .finish_non_exhaustive()
    }
}

pub struct ModelManager {
    providers: BTreeMap<String, Arc<dyn ModelProvider>>,
    unavailable: BTreeMap<String, String>,
    routing: RoutingConfig,
    default_model: Option<String>,
}

impl ModelManager {
    /// Build providers from configuration. A provider with missing/repairable
    /// credentials is retained as unavailable so startup and `/connect` still
    /// work; routing reports the model-specific construction error.
    pub fn from_config(config: &Config) -> Result<Self> {
        let mut providers: BTreeMap<String, Arc<dyn ModelProvider>> = BTreeMap::new();
        let mut unavailable = BTreeMap::new();
        for (name, model_cfg) in &config.models {
            match Self::build_provider(model_cfg) {
                Ok(provider) => {
                    providers.insert(name.clone(), provider);
                }
                Err(e) => {
                    unavailable.insert(name.clone(), format!("models.{name}: {e}"));
                }
            }
        }
        let default_model = config
            .routing
            .fallback
            .clone()
            .or_else(|| config.models.keys().next().cloned());
        Ok(Self {
            providers,
            unavailable,
            routing: config.routing.clone(),
            default_model,
        })
    }

    fn build_provider(cfg: &ModelConfig) -> Result<Arc<dyn ModelProvider>> {
        if cfg.auth.as_deref() == Some("codex") {
            // A Codex OAuth token belongs to the ChatGPT-backend Responses
            // protocol, not api.openai.com chat/completions. Route it through
            // the dedicated provider regardless of legacy provider spelling.
            return Ok(Arc::new(
                crate::codex_responses::CodexResponsesProvider::new(cfg)?,
            ));
        }
        Ok(match cfg.provider.as_str() {
            "llamacpp" => Arc::new(OpenAiCompatProvider::new("llamacpp", cfg)?),
            "openai" => Arc::new(OpenAiCompatProvider::new("openai", cfg)?),
            "openai_compatible" => Arc::new(OpenAiCompatProvider::new("openai_compatible", cfg)?),
            "custom_http" => Arc::new(OpenAiCompatProvider::new("custom_http", cfg)?),
            "ollama" => Arc::new(OllamaProvider::new(cfg)?),
            "codex" => Arc::new(crate::codex_responses::CodexResponsesProvider::new(cfg)?),
            "claude-plan" => Arc::new(crate::claude_plan::ClaudePlanProvider::new(cfg)?),
            "anthropic" => Arc::new(crate::anthropic::AnthropicProvider::new(cfg)?),
            "mock" => Arc::new(MockProvider::new(vec![])),
            other => {
                return Err(NexusError::Config(format!(
                    "unknown provider kind `{other}`"
                )))
            }
        })
    }

    /// Register a provider directly (tests, dynamic setup).
    pub fn insert(&mut self, name: &str, provider: Arc<dyn ModelProvider>) {
        self.unavailable.remove(name);
        self.providers.insert(name.to_string(), provider);
        if self.default_model.is_none() {
            self.default_model = Some(name.to_string());
        }
    }

    pub fn names(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    pub fn get(&self, name: &str) -> Result<Arc<dyn ModelProvider>> {
        if let Some(provider) = self.providers.get(name) {
            return Ok(provider.clone());
        }
        if let Some(reason) = self.unavailable.get(name) {
            return Err(NexusError::Config(format!(
                "{reason}. Repair this provider with /connect; other configured providers remain usable"
            )));
        }
        Err(NexusError::NotFound(format!("model `{name}`")))
    }

    /// Return normalized capabilities for one configured model. Callers such
    /// as `/model` use this instead of inspecting provider-specific config or
    /// payloads, keeping the control plane provider agnostic.
    pub fn capabilities(&self, name: &str) -> Result<ModelCapabilities> {
        Ok(self.get(name)?.capabilities())
    }

    pub fn default_model(&self) -> Option<&str> {
        self.default_model.as_deref()
    }

    /// Route a task class to a configured model name.
    pub fn route(&self, class: TaskClass) -> Result<(String, Arc<dyn ModelProvider>)> {
        let candidate = match class {
            TaskClass::Simple => self.routing.simple.as_ref(),
            TaskClass::Coding => self.routing.coding.as_ref(),
            TaskClass::Planning | TaskClass::Verification => self.routing.planning.as_ref(),
            TaskClass::Research => self.routing.coding.as_ref(),
        };
        let name = candidate
            .or(self.routing.fallback.as_ref())
            .map(|s| s.as_str())
            .or(self.default_model.as_deref())
            .ok_or_else(|| {
                NexusError::Config("no models configured; add a [models.*] entry".into())
            })?;
        Ok((name.to_string(), self.get(name)?))
    }

    /// Complete with automatic fallback: if the routed provider fails with a
    /// provider/timeout error and a distinct fallback model exists, retry once
    /// on the fallback.
    pub async fn complete_routed(
        &self,
        class: TaskClass,
        request: ModelRequest,
    ) -> Result<(String, ModelResponse)> {
        let (name, provider) = self.route(class)?;
        match provider.complete(request.clone()).await {
            Ok(c) => Ok((name, c)),
            Err(e) if is_fallback_worthy(&e) => {
                if let Some(fb) = self.fallback_other_than(&name) {
                    tracing::warn!(model = %name, fallback = %fb, error = %e, "provider failed; using fallback");
                    let provider = self.get(&fb)?;
                    let c = provider.complete(request).await?;
                    Ok((fb, c))
                } else {
                    Err(e)
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Stream from a specific model with no fallback (fallback mid-stream
    /// would duplicate partial output; the agent loop handles retries).
    pub async fn stream_model(
        &self,
        name: &str,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        self.get(name)?.stream(request).await
    }

    /// Route and construct a stream with one privacy- and capability-aware
    /// fallback opportunity.
    ///
    /// Fallback is considered only when the primary provider fails while
    /// constructing the stream. Once a stream is returned—even if it later
    /// yields an error—no model switch is attempted, avoiding duplicated
    /// partial output. A remote cross-provider fallback requires the explicit
    /// `allow_cross_provider` policy flag; a same-provider-kind or verified
    /// local-only fallback does not.
    pub async fn stream_routed_with_fallback(
        &self,
        class: TaskClass,
        request: ModelRequest,
        allow_cross_provider: bool,
    ) -> Result<RoutedModelStream> {
        let (primary_name, primary) = self.route(class)?;
        match primary.stream(request.clone()).await {
            Ok(stream) => Ok(RoutedModelStream {
                model_name: primary_name,
                fallback: None,
                stream,
            }),
            Err(error) if is_fallback_worthy(&error) => {
                let reason = pre_stream_reason(&error);
                let Some(fallback_name) = self.fallback_other_than(&primary_name) else {
                    return Err(error);
                };
                let fallback = self.get(&fallback_name)?;
                let primary_caps = primary.capabilities();
                let fallback_caps = fallback.capabilities();

                if fallback_caps.fallback_eligibility == FallbackEligibility::Ineligible {
                    return Err(fallback_rejected(
                        &primary_name,
                        &fallback_name,
                        reason,
                        "model is marked ineligible for fallback",
                    ));
                }
                if let Some(detail) = wire_incompatibility(&request, &primary_caps, &fallback_caps)
                {
                    return Err(fallback_rejected(
                        &primary_name,
                        &fallback_name,
                        reason,
                        detail,
                    ));
                }

                let same_provider_kind = primary_caps.provider_kind == fallback_caps.provider_kind;
                let local_only = (fallback_caps.local
                    || fallback_caps.locality == ModelLocality::Local)
                    && fallback_caps.privacy == ModelPrivacy::LocalOnly;
                if !same_provider_kind && !local_only && !allow_cross_provider {
                    return Err(fallback_rejected(
                        &primary_name,
                        &fallback_name,
                        reason,
                        "remote cross-provider fallback requires explicit policy approval",
                    ));
                }

                tracing::warn!(
                    model = %primary_name,
                    fallback = %fallback_name,
                    reason = reason.as_str(),
                    same_provider_kind,
                    local_only,
                    "provider stream setup failed; using approved pre-stream fallback"
                );
                let stream = fallback.stream(request).await?;
                Ok(RoutedModelStream {
                    model_name: fallback_name,
                    fallback: Some(PreStreamFallback {
                        from_model: primary_name,
                        reason,
                    }),
                    stream,
                })
            }
            Err(error) => Err(error),
        }
    }

    fn fallback_other_than(&self, exclude: &str) -> Option<String> {
        self.routing
            .fallback
            .as_ref()
            .filter(|f| f.as_str() != exclude)
            .cloned()
    }

    /// Probe all configured providers.
    pub async fn health_all(&self) -> Vec<(String, &'static str, ProviderHealth)> {
        let mut out = Vec::new();
        for (name, provider) in &self.providers {
            out.push((name.clone(), provider.kind(), provider.health().await));
        }
        for (name, reason) in &self.unavailable {
            out.push((
                name.clone(),
                "unavailable",
                ProviderHealth {
                    reachable: false,
                    detail: reason.clone(),
                    latency_ms: None,
                },
            ));
        }
        out
    }
}

fn is_fallback_worthy(e: &NexusError) -> bool {
    e.is_provider_retryable()
}

fn pre_stream_reason(error: &NexusError) -> PreStreamFallbackReason {
    if matches!(error, NexusError::ModelTimeout(_)) {
        PreStreamFallbackReason::Timeout
    } else {
        PreStreamFallbackReason::ProviderUnavailable
    }
}

fn wire_incompatibility(
    request: &ModelRequest,
    primary: &ModelCapabilities,
    fallback: &ModelCapabilities,
) -> Option<&'static str> {
    if !request.tools.is_empty() && primary.native_tool_calls != fallback.native_tool_calls {
        return Some("native tool-call capability differs from the routed model");
    }
    if request
        .messages
        .iter()
        .any(|message| message.role == Role::System)
        && primary.system_prompt != fallback.system_prompt
    {
        return Some("system-role capability differs from the routed model");
    }
    if request.json_mode && primary.structured_output != fallback.structured_output {
        return Some("structured-output capability differs from the routed model");
    }
    None
}

fn fallback_rejected(
    primary: &str,
    fallback: &str,
    reason: PreStreamFallbackReason,
    detail: &str,
) -> NexusError {
    NexusError::Provider {
        provider: primary.to_string(),
        message: format!(
            "stream setup failed ({}); fallback `{fallback}` not used: {detail}",
            reason.as_str()
        ),
    }
}

/// Detect local model servers on their default ports without any config.
/// Used by `snx doctor` — detection only; no model downloads.
pub async fn detect_local_servers() -> Vec<(String, String)> {
    let mut found = Vec::new();
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return found,
    };
    for (label, url) in [
        (
            "llama.cpp (llama-server)",
            "http://127.0.0.1:8080/v1/models",
        ),
        ("Ollama", "http://127.0.0.1:11434/api/tags"),
        (
            "OpenAI-compatible on :8000 (vLLM/others)",
            "http://127.0.0.1:8000/v1/models",
        ),
    ] {
        if let Ok(resp) = client.get(url).send().await {
            if resp.status().is_success() {
                found.push((label.to_string(), url.to_string()));
            }
        }
    }
    found
}

/// A local model runtime discovered on the host, with its installed models.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LocalRuntime {
    /// Provider kind to use in config: `ollama` or `openai_compatible`.
    pub provider: String,
    /// Human label, e.g. `Ollama` or `llama.cpp (llama-server)`.
    pub label: String,
    /// Base URL to put in config (`…/v1` for OpenAI-compatible, host for Ollama).
    pub base_url: String,
    /// Models the runtime reports as installed/loaded.
    pub models: Vec<String>,
    /// Whether the runtime's CLI is on PATH (installed, even if not running).
    pub binary_on_path: bool,
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

/// Detect local runtimes AND enumerate their installed models, validating the
/// response shape so an unrelated service on the same port is not mistaken for
/// a model server. Detection only — never downloads or loads anything.
pub async fn detect_local_models() -> Vec<LocalRuntime> {
    let mut out = Vec::new();
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return out,
    };

    // Ollama (native): GET /api/tags -> { "models": [ { "name": ... } ] }
    if let Ok(resp) = client.get("http://127.0.0.1:11434/api/tags").send().await {
        if resp.status().is_success() {
            if let Ok(v) = resp.json::<serde_json::Value>().await {
                if let Some(models) = v.get("models").and_then(|m| m.as_array()) {
                    let names = models
                        .iter()
                        .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                        .map(String::from)
                        .collect();
                    out.push(LocalRuntime {
                        provider: "ollama".into(),
                        label: "Ollama".into(),
                        base_url: "http://127.0.0.1:11434".into(),
                        models: names,
                        binary_on_path: on_path("ollama"),
                    });
                }
            }
        }
    }

    // OpenAI-compatible servers: GET {base}/v1/models -> { "data": [ { "id" } ] }
    for (label, base, bins) in [
        (
            "llama.cpp (llama-server)",
            "http://127.0.0.1:8080/v1",
            &["llama-server", "llama-cli"][..],
        ),
        (
            "OpenAI-compatible :8000 (vLLM/others)",
            "http://127.0.0.1:8000/v1",
            &[][..],
        ),
        ("LM Studio :1234", "http://127.0.0.1:1234/v1", &[][..]),
    ] {
        if let Ok(resp) = client.get(format!("{base}/models")).send().await {
            if resp.status().is_success() {
                if let Ok(v) = resp.json::<serde_json::Value>().await {
                    // Must look like an OpenAI models listing to count.
                    if let Some(data) = v.get("data").and_then(|d| d.as_array()) {
                        let ids = data
                            .iter()
                            .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
                            .map(String::from)
                            .collect();
                        out.push(LocalRuntime {
                            provider: "openai_compatible".into(),
                            label: label.into(),
                            base_url: base.into(),
                            models: ids,
                            binary_on_path: bins.iter().any(|b| on_path(b)),
                        });
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockScript;
    use crate::types::{ChatMessage, ToolSpec};
    use futures::StreamExt;

    fn manager_with_mock(script: Vec<MockScript>) -> ModelManager {
        let mut m = ModelManager {
            providers: BTreeMap::new(),
            unavailable: BTreeMap::new(),
            routing: RoutingConfig::default(),
            default_model: None,
        };
        m.insert("main", Arc::new(MockProvider::new(script)));
        m
    }

    fn manager_with_fallback(
        primary: Arc<MockProvider>,
        fallback: Arc<MockProvider>,
    ) -> ModelManager {
        let mut manager = ModelManager {
            providers: BTreeMap::new(),
            unavailable: BTreeMap::new(),
            routing: RoutingConfig::default(),
            default_model: None,
        };
        manager.insert("main", primary);
        manager.insert("backup", fallback);
        manager.routing.simple = Some("main".into());
        manager.routing.fallback = Some("backup".into());
        manager
    }

    #[tokio::test]
    async fn routes_to_default_when_unconfigured() {
        let m = manager_with_mock(vec![MockScript::Text("hi".into())]);
        let (name, c) = m
            .complete_routed(TaskClass::Coding, ModelRequest::default())
            .await
            .expect("complete");
        assert_eq!(name, "main");
        assert_eq!(c.content, "hi");
    }

    #[tokio::test]
    async fn falls_back_on_provider_error() {
        let mut m = manager_with_mock(vec![MockScript::Error("down".into())]);
        m.insert(
            "backup",
            Arc::new(MockProvider::new(vec![MockScript::Text("rescued".into())])),
        );
        m.routing.fallback = Some("backup".into());
        // Route resolves simple → fallback… make routing.simple point at main
        m.routing.simple = Some("main".into());
        let (name, c) = m
            .complete_routed(TaskClass::Simple, ModelRequest::default())
            .await
            .expect("fallback should rescue");
        assert_eq!(name, "backup");
        assert_eq!(c.content, "rescued");
    }

    #[tokio::test]
    async fn stream_setup_uses_same_provider_fallback_without_cross_provider_grant() {
        let primary = Arc::new(
            MockProvider::new(vec![MockScript::Timeout])
                .with_provider_kind("hosted-a")
                .with_remote_endpoint(),
        );
        let fallback = Arc::new(
            MockProvider::new(vec![MockScript::Text("rescued".into())])
                .with_provider_kind("hosted-a")
                .with_remote_endpoint(),
        );
        let manager = manager_with_fallback(primary, fallback.clone());

        let selected = manager
            .stream_routed_with_fallback(TaskClass::Simple, ModelRequest::default(), false)
            .await
            .expect("same-provider fallback");
        assert_eq!(selected.model_name, "backup");
        assert_eq!(
            selected.fallback,
            Some(PreStreamFallback {
                from_model: "main".into(),
                reason: PreStreamFallbackReason::Timeout,
            })
        );
        let response = crate::provider::collect_stream(selected.stream)
            .await
            .expect("fallback response");
        assert_eq!(response.content, "rescued");
        assert_eq!(fallback.recorded_requests().len(), 1);
    }

    #[tokio::test]
    async fn stream_setup_denies_unapproved_remote_cross_provider_fallback() {
        let primary = Arc::new(
            MockProvider::new(vec![MockScript::Error("connection reset".into())])
                .with_provider_kind("hosted-a")
                .with_remote_endpoint(),
        );
        let fallback = Arc::new(
            MockProvider::new(vec![MockScript::Text("must not run".into())])
                .with_provider_kind("hosted-b")
                .with_remote_endpoint(),
        );
        let manager = manager_with_fallback(primary, fallback.clone());

        let error = manager
            .stream_routed_with_fallback(TaskClass::Simple, ModelRequest::default(), false)
            .await
            .expect_err("cross-provider fallback requires approval");
        assert!(error
            .to_string()
            .contains("remote cross-provider fallback requires explicit policy approval"));
        assert!(fallback.recorded_requests().is_empty());
    }

    #[tokio::test]
    async fn stream_setup_allows_explicit_remote_cross_provider_fallback() {
        let primary = Arc::new(
            MockProvider::new(vec![MockScript::Error("connection reset".into())])
                .with_provider_kind("hosted-a")
                .with_remote_endpoint(),
        );
        let fallback = Arc::new(
            MockProvider::new(vec![MockScript::Text("approved".into())])
                .with_provider_kind("hosted-b")
                .with_remote_endpoint(),
        );
        let manager = manager_with_fallback(primary, fallback);

        let selected = manager
            .stream_routed_with_fallback(TaskClass::Simple, ModelRequest::default(), true)
            .await
            .expect("approved cross-provider fallback");
        assert_eq!(selected.model_name, "backup");
        assert_eq!(
            selected.fallback.as_ref().map(|fallback| fallback.reason),
            Some(PreStreamFallbackReason::ProviderUnavailable)
        );
    }

    #[tokio::test]
    async fn stream_setup_allows_local_only_cross_provider_fallback() {
        let primary = Arc::new(
            MockProvider::new(vec![MockScript::Error("connection reset".into())])
                .with_provider_kind("hosted-a")
                .with_remote_endpoint(),
        );
        let fallback = Arc::new(
            MockProvider::new(vec![MockScript::Text("local".into())])
                .with_provider_kind("local-runtime"),
        );
        let manager = manager_with_fallback(primary, fallback);

        let selected = manager
            .stream_routed_with_fallback(TaskClass::Simple, ModelRequest::default(), false)
            .await
            .expect("local-only fallback needs no cross-provider grant");
        assert_eq!(selected.model_name, "backup");
        assert!(selected.fallback.is_some());
    }

    #[tokio::test]
    async fn stream_setup_rejects_wire_incompatible_fallback() {
        let primary = Arc::new(
            MockProvider::new(vec![MockScript::Error("connection reset".into())])
                .with_provider_kind("same-provider"),
        );
        let fallback = Arc::new(
            MockProvider::new(vec![MockScript::Text("must not run".into())])
                .with_provider_kind("same-provider")
                .without_native_tools(),
        );
        let manager = manager_with_fallback(primary, fallback.clone());
        let request = ModelRequest {
            tools: vec![ToolSpec {
                name: "fs.read".into(),
                description: "read a file".into(),
                parameters: serde_json::json!({"type": "object"}),
            }],
            ..Default::default()
        };

        let error = manager
            .stream_routed_with_fallback(TaskClass::Simple, request, true)
            .await
            .expect_err("wire mismatch must stop fallback");
        assert!(error
            .to_string()
            .contains("native tool-call capability differs"));
        assert!(fallback.recorded_requests().is_empty());
    }

    #[tokio::test]
    async fn stream_setup_checks_system_and_structured_output_compatibility() {
        let system_primary = Arc::new(
            MockProvider::new(vec![MockScript::Error("connection reset".into())])
                .with_provider_kind("same-provider"),
        );
        let system_fallback = Arc::new(
            MockProvider::new(vec![MockScript::Text("must not run".into())])
                .with_provider_kind("same-provider")
                .without_system_prompt(),
        );
        let manager = manager_with_fallback(system_primary, system_fallback);
        let error = manager
            .stream_routed_with_fallback(
                TaskClass::Simple,
                ModelRequest {
                    messages: vec![ChatMessage::system("policy")],
                    ..Default::default()
                },
                true,
            )
            .await
            .expect_err("system role mismatch");
        assert!(error.to_string().contains("system-role capability differs"));

        let structured_primary = Arc::new(
            MockProvider::new(vec![MockScript::Error("connection reset".into())])
                .with_provider_kind("same-provider"),
        );
        let structured_fallback = Arc::new(
            MockProvider::new(vec![MockScript::Text("must not run".into())])
                .with_provider_kind("same-provider")
                .without_structured_output(),
        );
        let manager = manager_with_fallback(structured_primary, structured_fallback);
        let error = manager
            .stream_routed_with_fallback(
                TaskClass::Simple,
                ModelRequest {
                    json_mode: true,
                    ..Default::default()
                },
                true,
            )
            .await
            .expect_err("structured output mismatch");
        assert!(error
            .to_string()
            .contains("structured-output capability differs"));
    }

    #[tokio::test]
    async fn stream_error_after_construction_never_triggers_fallback() {
        let primary = Arc::new(
            MockProvider::new(vec![MockScript::PartialStreamFailure {
                partial: "partial".into(),
            }])
            .with_provider_kind("hosted-a")
            .with_remote_endpoint(),
        );
        let fallback = Arc::new(
            MockProvider::new(vec![MockScript::Text("must not run".into())])
                .with_provider_kind("hosted-a")
                .with_remote_endpoint(),
        );
        let manager = manager_with_fallback(primary, fallback.clone());

        let selected = manager
            .stream_routed_with_fallback(TaskClass::Simple, ModelRequest::default(), false)
            .await
            .expect("primary stream was constructed");
        assert_eq!(selected.model_name, "main");
        assert!(selected.fallback.is_none());
        let mut stream = selected.stream;
        assert!(matches!(
            stream.next().await,
            Some(Ok(StreamEvent::TextDelta(text))) if text == "partial"
        ));
        assert!(matches!(stream.next().await, Some(Err(_))));
        assert!(fallback.recorded_requests().is_empty());
    }

    #[tokio::test]
    async fn deterministic_http_client_error_does_not_fallback() {
        let mut m = manager_with_mock(vec![MockScript::Error(
            "HTTP 400 Bad Request: invalid function name".into(),
        )]);
        m.insert(
            "backup",
            Arc::new(MockProvider::new(vec![MockScript::Text(
                "should not run".into(),
            )])),
        );
        m.routing.fallback = Some("backup".into());
        m.routing.simple = Some("main".into());

        let error = m
            .complete_routed(TaskClass::Simple, ModelRequest::default())
            .await
            .expect_err("HTTP 400 must surface without fallback");
        assert!(error.to_string().contains("HTTP 400 Bad Request"));
    }

    #[tokio::test]
    async fn error_when_no_models() {
        let m = ModelManager {
            providers: BTreeMap::new(),
            unavailable: BTreeMap::new(),
            routing: RoutingConfig::default(),
            default_model: None,
        };
        assert!(m.route(TaskClass::Simple).is_err());
    }

    #[tokio::test]
    async fn missing_credentials_do_not_prevent_manager_startup() {
        let mut config = Config::default();
        config.models.insert(
            "hosted".into(),
            ModelConfig {
                provider: "openai".into(),
                model: "gpt-test".into(),
                api_key_env: Some("NEXUS_TEST_KEY_THAT_IS_NOT_SET".into()),
                ..Default::default()
            },
        );
        let manager = ModelManager::from_config(&config).expect("manager remains repairable");
        assert!(manager.route(TaskClass::Coding).is_err());
        let health = manager.health_all().await;
        assert!(health.iter().any(|(name, kind, report)| {
            name == "hosted" && *kind == "unavailable" && !report.reachable
        }));
    }
}
