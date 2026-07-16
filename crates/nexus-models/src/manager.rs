//! Model registry, task routing, and provider fallback.

use crate::mock::MockProvider;
use crate::ollama::OllamaProvider;
use crate::openai_compat::OpenAiCompatProvider;
use crate::provider::ModelProvider;
use crate::types::{Completion, CompletionRequest, ProviderHealth, StreamEvent, TaskClass};
use futures::stream::BoxStream;
use nexus_core::config::{Config, ModelConfig, RoutingConfig};
use nexus_core::{NexusError, Result};
use std::collections::BTreeMap;
use std::sync::Arc;

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
        request: CompletionRequest,
    ) -> Result<(String, Completion)> {
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
        request: CompletionRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        self.get(name)?.stream(request).await
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

    #[tokio::test]
    async fn routes_to_default_when_unconfigured() {
        let m = manager_with_mock(vec![MockScript::Text("hi".into())]);
        let (name, c) = m
            .complete_routed(TaskClass::Coding, CompletionRequest::default())
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
            .complete_routed(TaskClass::Simple, CompletionRequest::default())
            .await
            .expect("fallback should rescue");
        assert_eq!(name, "backup");
        assert_eq!(c.content, "rescued");
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
            .complete_routed(TaskClass::Simple, CompletionRequest::default())
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
