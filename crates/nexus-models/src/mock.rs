//! Deterministic mock provider for tests and offline demos.
//!
//! Scripted with a queue of [`MockScript`] entries; each `complete`/`stream`
//! call consumes the next entry. Used by the adversarial mock-model test
//! scenarios (invalid JSON, unknown tool, loops, timeouts, …).

use crate::provider::ModelProvider;
use crate::types::*;
use futures::stream::BoxStream;
use futures::StreamExt;
use nexus_core::{NexusError, Result};
use std::collections::VecDeque;
use std::sync::Mutex;

/// One scripted model turn.
#[derive(Debug, Clone)]
pub enum MockScript {
    /// Plain assistant text.
    Text(String),
    /// A structured tool call (name, JSON arguments).
    ToolCall { name: String, arguments: String },
    /// Text followed by a tool call in the same turn.
    TextThenToolCall {
        text: String,
        name: String,
        arguments: String,
    },
    /// Simulate a provider error.
    Error(String),
    /// Simulate a timeout.
    Timeout,
    /// Simulate a stream that fails partway through emitting `partial`.
    PartialStreamFailure { partial: String },
}

pub struct MockProvider {
    script: Mutex<VecDeque<MockScript>>,
    provider_kind: &'static str,
    /// Capability toggle so the tool-call compatibility layer can be tested.
    pub native_tool_calls: bool,
    pub system_prompt: bool,
    pub structured_output: bool,
    pub locality: ModelLocality,
    pub privacy: ModelPrivacy,
    pub context_window: usize,
    calls: Mutex<Vec<ModelRequest>>,
}

impl MockProvider {
    pub fn new(script: Vec<MockScript>) -> Self {
        Self {
            script: Mutex::new(script.into()),
            provider_kind: "mock",
            native_tool_calls: true,
            system_prompt: true,
            structured_output: true,
            locality: ModelLocality::Local,
            privacy: ModelPrivacy::LocalOnly,
            context_window: 8192,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn without_native_tools(mut self) -> Self {
        self.native_tool_calls = false;
        self
    }

    pub fn with_provider_kind(mut self, provider_kind: &'static str) -> Self {
        self.provider_kind = provider_kind;
        self
    }

    pub fn without_system_prompt(mut self) -> Self {
        self.system_prompt = false;
        self
    }

    pub fn without_structured_output(mut self) -> Self {
        self.structured_output = false;
        self
    }

    pub fn with_remote_endpoint(mut self) -> Self {
        self.locality = ModelLocality::Remote;
        self.privacy = ModelPrivacy::ProviderManaged;
        self
    }

    pub fn with_context_window(mut self, n: usize) -> Self {
        self.context_window = n;
        self
    }

    /// Requests received so far (for assertions).
    pub fn recorded_requests(&self) -> Vec<ModelRequest> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }

    fn next_script(&self, request: &ModelRequest) -> MockScript {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(request.clone());
        }
        self.script
            .lock()
            .ok()
            .and_then(|mut s| s.pop_front())
            .unwrap_or(MockScript::Text("mock script exhausted".into()))
    }

    fn to_completion(script: MockScript) -> Result<ModelResponse> {
        let usage = Usage {
            prompt_tokens: 10,
            completion_tokens: 10,
            ..Usage::default()
        };
        match script {
            MockScript::Text(t) => Ok(ModelResponse {
                content: t,
                tool_calls: vec![],
                usage,
                finish_reason: "stop".into(),
                provider_private: None,
            }),
            MockScript::ToolCall { name, arguments } => Ok(ModelResponse {
                content: String::new(),
                tool_calls: vec![ToolCallRequest {
                    id: format!("call_{}", uuid_ish()),
                    name,
                    arguments,
                }],
                usage,
                finish_reason: "tool_calls".into(),
                provider_private: None,
            }),
            MockScript::TextThenToolCall {
                text,
                name,
                arguments,
            } => Ok(ModelResponse {
                content: text,
                tool_calls: vec![ToolCallRequest {
                    id: format!("call_{}", uuid_ish()),
                    name,
                    arguments,
                }],
                usage,
                finish_reason: "tool_calls".into(),
                provider_private: None,
            }),
            MockScript::Error(message) => Err(NexusError::Provider {
                provider: "mock".into(),
                message,
            }),
            MockScript::Timeout => Err(NexusError::ModelTimeout(1)),
            MockScript::PartialStreamFailure { .. } => Err(NexusError::Provider {
                provider: "mock".into(),
                message: "stream interrupted".into(),
            }),
        }
    }
}

fn uuid_ish() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
}

#[async_trait::async_trait]
impl ModelProvider for MockProvider {
    fn kind(&self) -> &'static str {
        self.provider_kind
    }

    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            model_id: format!("mock-{}", self.provider_kind),
            provider_kind: self.provider_kind.into(),
            streaming: true,
            native_tool_calls: self.native_tool_calls,
            structured_output: self.structured_output,
            image_input: false,
            embeddings: false,
            context_window: self.context_window,
            max_output_tokens: 2048,
            context_limit_source: nexus_core::config::LimitSource::ConfiguredConservative,
            output_limit_source: nexus_core::config::LimitSource::ConfiguredConservative,
            reasoning_controls: false,
            reasoning: ReasoningProfile::default(),
            system_prompt: self.system_prompt,
            parallel_tool_calls: self.native_tool_calls,
            json_schema: true,
            local: self.locality == ModelLocality::Local,
            accelerator: None,
            locality: self.locality,
            privacy: self.privacy,
            latency_class: ModelLatencyClass::Low,
            cost_class: if self.locality == ModelLocality::Local {
                ModelCostClass::Free
            } else {
                ModelCostClass::Unknown
            },
            fallback_eligibility: if self.locality == ModelLocality::Local {
                FallbackEligibility::Eligible
            } else {
                FallbackEligibility::ApprovalRequired
            },
        }
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse> {
        Self::to_completion(self.next_script(&request))
    }

    async fn stream(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        let script = self.next_script(&request);
        // Partial stream failure: emit some text deltas, then an error.
        if let MockScript::PartialStreamFailure { partial } = script {
            let events: Vec<Result<StreamEvent>> = vec![
                Ok(StreamEvent::TextDelta(partial)),
                Err(NexusError::Provider {
                    provider: "mock".into(),
                    message: "stream interrupted mid-response".into(),
                }),
            ];
            return Ok(futures::stream::iter(events).boxed());
        }
        let completion = Self::to_completion(script)?;
        let mut events: Vec<Result<StreamEvent>> = Vec::new();
        // Split text into small deltas to exercise incremental rendering.
        for chunk in completion.content.as_bytes().chunks(8) {
            events.push(Ok(StreamEvent::TextDelta(
                String::from_utf8_lossy(chunk).to_string(),
            )));
        }
        for (i, call) in completion.tool_calls.iter().enumerate() {
            events.push(Ok(StreamEvent::ToolCallDelta {
                index: i,
                id: Some(call.id.clone()),
                name: Some(call.name.clone()),
                arguments_delta: call.arguments.clone(),
            }));
        }
        events.push(Ok(StreamEvent::Done {
            usage: completion.usage,
            finish_reason: completion.finish_reason,
        }));
        Ok(futures::stream::iter(events).boxed())
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth {
            reachable: true,
            detail: "mock provider".into(),
            latency_ms: Some(0),
        }
    }
}
