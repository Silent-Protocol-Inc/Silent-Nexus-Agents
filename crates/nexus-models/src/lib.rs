//! nexus-models: provider-neutral model abstraction.
//!
//! Providers: llama.cpp (OpenAI mode), Ollama (native), Codex, Claude Plan,
//! Anthropic, generic OpenAI-compatible/custom HTTP, and a deterministic mock.
//! [`manager::ModelManager`] owns routing (task class → model) and fallback.

pub mod anthropic;
pub mod claude_plan;
pub mod codex_auth;
pub mod codex_responses;
pub mod discovery;
pub mod manager;
pub mod mock;
pub mod ollama;
pub mod openai_compat;
pub mod provider;
pub mod sse;
pub mod types;

pub use anthropic::{AnthropicProvider, ANTHROPIC_DEFAULT};
pub use claude_plan::ClaudePlanProvider;
pub use codex_auth::{load as load_codex_credentials, CodexCredentials, CodexSource};
pub use codex_responses::{CodexResponsesProvider, CODEX_BACKEND_DEFAULT};
pub use discovery::{
    human_size, list_ollama_models, list_ollama_models_with_tls, list_openai_models,
    list_openai_models_with_tls, validate_base_url, DiscoveredModel, ProbeError, ProbeFailure,
    ProbeOutcome,
};
pub use manager::{detect_local_models, detect_local_servers, LocalRuntime, ModelManager};
pub use provider::{collect_stream, ModelProvider};
pub use types::*;

#[cfg(test)]
mod integration_tests {
    use super::*;
    use nexus_core::config::ModelConfig;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn model_config(base_url: &str) -> ModelConfig {
        ModelConfig {
            provider: "openai_compatible".into(),
            base_url: base_url.to_string(),
            model: "test-model".into(),
            context_window: 4096,
            max_output_tokens: 256,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn openai_compat_parses_completion_and_tool_calls() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "I will read the file.",
                        "tool_calls": [{
                            "id": "call_abc",
                            "type": "function",
                            "function": {"name": "fs.read_file", "arguments": "{\"path\":\"src/main.rs\"}"}
                        }]
                    },
                    "finish_reason": "tool_calls"
                }],
                "usage": {"prompt_tokens": 42, "completion_tokens": 17}
            })))
            .mount(&server)
            .await;
        let cfg = model_config(&format!("{}/v1", server.uri()));
        let provider =
            openai_compat::OpenAiCompatProvider::new("openai_compatible", &cfg).expect("provider");
        let completion = provider
            .complete(CompletionRequest {
                messages: vec![ChatMessage::user("read main.rs")],
                ..Default::default()
            })
            .await
            .expect("completion");
        assert_eq!(completion.content, "I will read the file.");
        assert_eq!(completion.tool_calls.len(), 1);
        assert_eq!(completion.tool_calls[0].name, "fs.read_file");
        assert_eq!(completion.usage.prompt_tokens, 42);
        assert_eq!(completion.finish_reason, "tool_calls");
    }

    #[tokio::test]
    async fn openai_compat_streams_sse() {
        let server = MockServer::start().await;
        let sse_body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
            .mount(&server)
            .await;
        let cfg = model_config(&format!("{}/v1", server.uri()));
        let provider =
            openai_compat::OpenAiCompatProvider::new("openai_compatible", &cfg).expect("provider");
        let stream = provider
            .stream(CompletionRequest {
                messages: vec![ChatMessage::user("hi")],
                ..Default::default()
            })
            .await
            .expect("stream");
        let completion = collect_stream(stream).await.expect("collect");
        assert_eq!(completion.content, "Hello");
        assert_eq!(completion.finish_reason, "stop");
    }

    #[tokio::test]
    async fn openai_compat_surfaces_http_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("model exploded"))
            .mount(&server)
            .await;
        let cfg = model_config(&format!("{}/v1", server.uri()));
        let provider =
            openai_compat::OpenAiCompatProvider::new("openai_compatible", &cfg).expect("provider");
        let err = provider
            .complete(CompletionRequest::default())
            .await
            .expect_err("must fail");
        let msg = err.to_string();
        assert!(msg.contains("500"), "got: {msg}");
    }

    #[tokio::test]
    async fn ollama_parses_native_chat() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": {"role": "assistant", "content": "pong"},
                "done": true,
                "prompt_eval_count": 5,
                "eval_count": 2
            })))
            .mount(&server)
            .await;
        let mut cfg = model_config(&server.uri());
        cfg.provider = "ollama".into();
        let provider = ollama::OllamaProvider::new(&cfg).expect("provider");
        let completion = provider
            .complete(CompletionRequest {
                messages: vec![ChatMessage::user("ping")],
                ..Default::default()
            })
            .await
            .expect("completion");
        assert_eq!(completion.content, "pong");
        assert_eq!(completion.usage.prompt_tokens, 5);
    }
}
