//! The [`ModelProvider`] trait implemented by every backend.

use crate::types::{Completion, CompletionRequest, ModelCapabilities, ProviderHealth, StreamEvent};
use futures::stream::BoxStream;
use nexus_core::Result;

#[async_trait::async_trait]
pub trait ModelProvider: Send + Sync {
    /// Human-readable provider name (`llamacpp`, `ollama`, …).
    fn kind(&self) -> &'static str;

    /// Declared capabilities for the configured model.
    fn capabilities(&self) -> ModelCapabilities;

    /// Non-streaming completion.
    async fn complete(&self, request: CompletionRequest) -> Result<Completion>;

    /// Streaming completion. Providers without streaming support return a
    /// stream that yields the whole completion as one delta then `Done`.
    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>>;

    /// Probe endpoint reachability. Must be cheap and never mutate state.
    async fn health(&self) -> ProviderHealth;
}

/// Assemble a [`Completion`] by draining a stream (used by providers that
/// implement only `stream`, and by tests).
pub async fn collect_stream(
    mut stream: BoxStream<'static, Result<StreamEvent>>,
) -> Result<Completion> {
    use crate::types::{ToolCallRequest, Usage};
    use futures::StreamExt;

    let mut content = String::new();
    let mut calls: Vec<(Option<String>, String, String)> = Vec::new();
    let mut usage = Usage::default();
    let mut finish = String::from("stop");
    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::TextDelta(t) => content.push_str(&t),
            StreamEvent::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            } => {
                if calls.len() <= index {
                    calls.resize(index + 1, (None, String::new(), String::new()));
                }
                let slot = &mut calls[index];
                if id.is_some() {
                    slot.0 = id;
                }
                if let Some(n) = name {
                    slot.1.push_str(&n);
                }
                slot.2.push_str(&arguments_delta);
            }
            StreamEvent::Done {
                usage: u,
                finish_reason,
            } => {
                usage = u;
                finish = finish_reason;
            }
        }
    }
    let tool_calls = calls
        .into_iter()
        .enumerate()
        .filter(|(_, (_, name, _))| !name.is_empty())
        .map(|(i, (id, name, arguments))| ToolCallRequest {
            id: id.unwrap_or_else(|| format!("call_{i}")),
            name,
            arguments,
        })
        .collect();
    Ok(Completion {
        content,
        tool_calls,
        usage,
        finish_reason: finish,
    })
}
