//! Native Anthropic Messages API provider.

use crate::provider::ModelProvider;
use crate::sse::{SseItem, SseParser};
use crate::types::*;
use futures::stream::BoxStream;
use futures::StreamExt;
use nexus_core::config::ModelConfig;
use nexus_core::{NexusError, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub const ANTHROPIC_DEFAULT: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const ANTHROPIC_EXTENDED_CACHE_BETA: &str = "extended-cache-ttl-2025-04-11";

/// Read-only model discovery used by `/connect`.
pub async fn list_models(
    base_url: &str,
    api_key: &str,
    timeout: Duration,
) -> std::result::Result<crate::discovery::ProbeOutcome, crate::discovery::ProbeError> {
    use crate::discovery::{DiscoveredModel, ProbeError, ProbeFailure, ProbeOutcome};
    let base = crate::discovery::validate_base_url(base_url)?;
    let started = Instant::now();
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(5)))
        .build()
        .map_err(|error| ProbeError {
            failure: ProbeFailure::Other,
            detail: format!("http client: {error}"),
        })?;
    let response = client
        .get(format!("{}/models", base.as_str().trim_end_matches('/')))
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .send()
        .await
        .map_err(|error| ProbeError {
            failure: if error.is_timeout() {
                ProbeFailure::Timeout
            } else if error.is_connect() {
                ProbeFailure::ConnectionRefused
            } else {
                ProbeFailure::Other
            },
            detail: format!("GET /models: {error}"),
        })?;
    let status = response.status();
    if !status.is_success() {
        let failure = match status.as_u16() {
            401 => ProbeFailure::InvalidCredentials,
            403 => ProbeFailure::PermissionDenied,
            404 | 405 => ProbeFailure::UnsupportedEndpoint,
            429 => ProbeFailure::RateLimited,
            _ => ProbeFailure::Other,
        };
        return Err(ProbeError {
            failure,
            detail: format!("GET /models returned {status}"),
        });
    }
    let value: Value = response.json().await.map_err(|_| ProbeError {
        failure: ProbeFailure::MalformedResponse,
        detail: "response was not JSON".into(),
    })?;
    let entries = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| ProbeError {
            failure: ProbeFailure::MalformedResponse,
            detail: "no `data` array in Anthropic model listing".into(),
        })?;
    let models = entries
        .iter()
        .filter_map(|model| {
            Some(DiscoveredModel {
                id: model.get("id")?.as_str()?.to_string(),
                size_bytes: None,
                family: None,
                parameter_size: None,
                quantization: None,
                display_name: model
                    .get("display_name")
                    .and_then(Value::as_str)
                    .map(String::from),
                description: None,
                context_window: None,
                max_output_tokens: None,
                context_limit_source: None,
                output_limit_source: None,
                reasoning: None,
            })
        })
        .collect::<Vec<_>>();
    if models.is_empty() {
        return Err(ProbeError {
            failure: ProbeFailure::NoModels,
            detail: "Anthropic returned an empty model list".into(),
        });
    }
    Ok(ProbeOutcome {
        models,
        latency_ms: started.elapsed().as_millis() as u64,
    })
}

#[derive(Debug, Clone)]
pub struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
    config: ModelConfig,
}

impl AnthropicProvider {
    pub fn new(config: &ModelConfig) -> Result<Self> {
        let api_key = config
            .api_key_env
            .as_ref()
            .and_then(|name| std::env::var(name).ok())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                config
                    .resolved_api_key
                    .as_ref()
                    .map(|secret| secret.expose().to_string())
                    .filter(|value| !value.is_empty())
            })
            .ok_or_else(|| {
                NexusError::Config(
                    "provider `anthropic` requires ANTHROPIC_API_KEY or a stored \
                     credential selected through `/login`"
                        .into(),
                )
            })?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .connect_timeout(Duration::from_secs(10))
            .danger_accept_invalid_certs(!config.tls_verify)
            .build()
            .map_err(|error| provider_error(format!("http client: {error}")))?;
        let configured = config.base_url.trim_end_matches('/');
        let base_url = if configured.is_empty() || configured == "http://127.0.0.1:8080/v1" {
            ANTHROPIC_DEFAULT.into()
        } else {
            configured.into()
        };
        Ok(Self {
            client,
            base_url,
            model: config.model.clone(),
            api_key,
            config: config.clone(),
        })
    }

    fn messages_url(&self) -> String {
        format!("{}/messages", self.base_url)
    }

    fn build_body(&self, request: &CompletionRequest, stream: bool) -> Value {
        let system = request
            .messages
            .iter()
            .filter(|message| message.role == Role::System)
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut messages = anthropic_messages(&request.messages);
        // Two breakpoints, inside the four the API allows. Anthropic renders
        // `tools` then `system` then `messages`, so a marker on the last system
        // block covers the tool schemas too; a marker on the last message block
        // means this turn's write becomes the next turn's read.
        let cache_control =
            self.config
                .prompt_cache_enabled()
                .then(|| match self.config.prompt_cache_ttl() {
                    "1h" => json!({"type": "ephemeral", "ttl": "1h"}),
                    _ => json!({"type": "ephemeral"}),
                });
        if let Some(marker) = cache_control.as_ref() {
            mark_last_block(&mut messages, marker);
        }
        let mut body = json!({
            "model": self.model,
            "max_tokens": request.max_tokens.unwrap_or(self.config.max_output_tokens),
            "messages": messages,
            "stream": stream,
        });
        if !system.is_empty() {
            body["system"] = match cache_control.as_ref() {
                Some(marker) => {
                    json!([{"type": "text", "text": system, "cache_control": marker}])
                }
                None => json!(system),
            };
        }
        if let Some(temperature) = request.temperature.or(self.config.temperature) {
            body["temperature"] = json!(temperature);
        }
        if !request.stop.is_empty() {
            body["stop_sequences"] = json!(request.stop);
        }
        if let Some(effort) = self.config.reasoning_effort.as_deref() {
            body["output_config"] = json!({"effort": effort});
        }
        if !request.tools.is_empty() && self.capabilities().native_tool_calls {
            body["tools"] = json!(request
                .tools
                .iter()
                .map(|tool| json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.parameters,
                }))
                .collect::<Vec<_>>());
        }
        body
    }

    fn request(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut builder = self
            .client
            .post(self.messages_url())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION);
        // The hour-long TTL was gated behind a beta flag before it went
        // general; sending it costs nothing on accounts that no longer need it.
        if self.config.prompt_cache_enabled() && self.config.prompt_cache_ttl() == "1h" {
            builder = builder.header("anthropic-beta", ANTHROPIC_EXTENDED_CACHE_BETA);
        }
        builder.json(body)
    }
}

#[async_trait::async_trait]
impl ModelProvider for AnthropicProvider {
    fn kind(&self) -> &'static str {
        "anthropic"
    }

    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            model_id: self.model.clone(),
            provider_kind: self.kind().into(),
            streaming: true,
            native_tool_calls: self.config.native_tool_calls.unwrap_or(true),
            structured_output: true,
            image_input: true,
            embeddings: false,
            context_window: self.config.context_window,
            max_output_tokens: self.config.max_output_tokens,
            context_limit_source: self.config.context_limit_source,
            output_limit_source: self.config.output_limit_source,
            reasoning_controls: self.config.reasoning_effort.is_some(),
            reasoning: self
                .config
                .reasoning_effort
                .as_ref()
                .map(|effort| ReasoningProfile {
                    supported_efforts: vec![effort.clone()],
                    default_effort: Some(effort.clone()),
                    control: ReasoningControl::Optional,
                    mandatory: false,
                    provider_managed: false,
                    provenance: ReasoningProvenance::ConfiguredDefault,
                })
                .unwrap_or_default(),
            system_prompt: true,
            parallel_tool_calls: self.config.native_tool_calls.unwrap_or(true),
            json_schema: false,
            local: false,
            accelerator: None,
            locality: ModelLocality::Remote,
            privacy: ModelPrivacy::ProviderManaged,
            latency_class: ModelLatencyClass::Unknown,
            cost_class: ModelCostClass::Unknown,
            fallback_eligibility: FallbackEligibility::ApprovalRequired,
        }
    }

    async fn complete(&self, request: CompletionRequest) -> Result<Completion> {
        let response = self
            .request(&self.build_body(&request, false))
            .send()
            .await
            .map_err(map_reqwest_error)?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| provider_error(format!("reading response: {error}")))?;
        if !status.is_success() {
            return Err(provider_error(format!(
                "HTTP {status}: {}",
                truncate_error(&text)
            )));
        }
        let value: Value = serde_json::from_str(&text)
            .map_err(|error| provider_error(format!("invalid JSON response: {error}")))?;
        Ok(parse_completion(&value))
    }

    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        let response = self
            .request(&self.build_body(&request, true))
            .send()
            .await
            .map_err(map_reqwest_error)?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(provider_error(format!(
                "HTTP {status}: {}",
                truncate_error(&text)
            )));
        }
        let state = AnthropicStreamState {
            bytes: response.bytes_stream().boxed(),
            parser: SseParser::new(),
            pending: Vec::new(),
            usage: Usage::default(),
            finish_reason: "stop".into(),
            done_sent: false,
            finished: false,
        };
        Ok(futures::stream::unfold(state, next_anthropic_event).boxed())
    }

    async fn health(&self) -> ProviderHealth {
        let started = Instant::now();
        let response = self
            .client
            .get(format!("{}/models", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .timeout(Duration::from_secs(5))
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => ProviderHealth {
                reachable: true,
                detail: "Anthropic API authenticated".into(),
                latency_ms: Some(started.elapsed().as_millis() as u64),
            },
            Ok(response) => ProviderHealth {
                reachable: false,
                detail: format!("endpoint returned {}", response.status()),
                latency_ms: Some(started.elapsed().as_millis() as u64),
            },
            Err(error) => ProviderHealth {
                reachable: false,
                detail: format!("unreachable: {error}"),
                latency_ms: None,
            },
        }
    }
}

type ByteStream =
    futures::stream::BoxStream<'static, std::result::Result<bytes::Bytes, reqwest::Error>>;

struct AnthropicStreamState {
    bytes: ByteStream,
    parser: SseParser,
    pending: Vec<Result<StreamEvent>>,
    usage: Usage,
    finish_reason: String,
    done_sent: bool,
    finished: bool,
}

async fn next_anthropic_event(
    mut state: AnthropicStreamState,
) -> Option<(Result<StreamEvent>, AnthropicStreamState)> {
    loop {
        if let Some(event) = state.pending.pop() {
            return Some((event, state));
        }
        if state.finished {
            return None;
        }
        match state.bytes.next().await {
            Some(Ok(chunk)) => {
                let mut events = Vec::new();
                for item in state.parser.feed(&chunk) {
                    match item {
                        SseItem::Done => state.finished = true,
                        SseItem::Data(data) => match serde_json::from_str::<Value>(&data) {
                            Ok(value) => {
                                events.extend(parse_stream_value(&value, &mut state));
                            }
                            Err(error) => {
                                events.push(Err(provider_error(format!(
                                    "invalid stream JSON: {error}"
                                ))));
                                state.finished = true;
                            }
                        },
                    }
                }
                events.reverse();
                state.pending = events;
            }
            Some(Err(error)) => {
                state.finished = true;
                return Some((
                    Err(provider_error(format!("stream interrupted: {error}"))),
                    state,
                ));
            }
            None => {
                state.finished = true;
                if !state.done_sent {
                    state.done_sent = true;
                    return Some((
                        Ok(StreamEvent::Done {
                            usage: state.usage.clone(),
                            finish_reason: state.finish_reason.clone(),
                        }),
                        state,
                    ));
                }
                return None;
            }
        }
    }
}

fn anthropic_messages(messages: &[ChatMessage]) -> Vec<Value> {
    let mut output: Vec<Value> = Vec::new();
    for message in messages
        .iter()
        .filter(|message| message.role != Role::System)
    {
        let (role, blocks) = match message.role {
            Role::User => (
                "user",
                vec![json!({"type": "text", "text": message.content})],
            ),
            Role::Assistant => {
                let mut blocks = Vec::new();
                if !message.content.is_empty() {
                    blocks.push(json!({"type": "text", "text": message.content}));
                }
                blocks.extend(message.tool_calls.iter().map(|call| {
                    json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": serde_json::from_str::<Value>(&call.arguments)
                            .unwrap_or_else(|_| json!({})),
                    })
                }));
                ("assistant", blocks)
            }
            Role::Tool => (
                "user",
                vec![json!({
                    "type": "tool_result",
                    "tool_use_id": message.tool_call_id,
                    "content": message.content,
                })],
            ),
            Role::System => unreachable!("filtered above"),
        };
        if let Some(previous) = output
            .last_mut()
            .filter(|previous| previous.get("role").and_then(Value::as_str) == Some(role))
        {
            if let (Some(existing), Some(more)) = (
                previous.get_mut("content").and_then(Value::as_array_mut),
                Some(blocks),
            ) {
                existing.extend(more);
            }
        } else {
            output.push(json!({"role": role, "content": blocks}));
        }
    }
    output
}

/// Attach a cache breakpoint to the final content block of the conversation.
///
/// A no-op on an empty conversation, and on the block shapes that carry no
/// content of their own — the API rejects `cache_control` on anything it does
/// not bill as input.
fn mark_last_block(messages: &mut [Value], cache_control: &Value) {
    let Some(blocks) = messages
        .last_mut()
        .and_then(|message| message.get_mut("content"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    if let Some(block) = blocks.last_mut().and_then(Value::as_object_mut) {
        block.insert("cache_control".into(), cache_control.clone());
    }
}

fn parse_completion(value: &Value) -> Completion {
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    if let Some(blocks) = value.get("content").and_then(Value::as_array) {
        for (index, block) in blocks.iter().enumerate() {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    content.push_str(block.get("text").and_then(Value::as_str).unwrap_or(""));
                }
                Some("tool_use") => tool_calls.push(ToolCallRequest {
                    id: block
                        .get("id")
                        .and_then(Value::as_str)
                        .map(String::from)
                        .unwrap_or_else(|| format!("tool_{index}")),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    arguments: block
                        .get("input")
                        .map(Value::to_string)
                        .unwrap_or_else(|| "{}".into()),
                }),
                _ => {}
            }
        }
    }
    Completion {
        content,
        tool_calls,
        usage: parse_usage(value.get("usage")),
        finish_reason: value
            .get("stop_reason")
            .and_then(Value::as_str)
            .unwrap_or("stop")
            .to_string(),
        provider_private: None,
    }
}

fn parse_stream_value(value: &Value, state: &mut AnthropicStreamState) -> Vec<Result<StreamEvent>> {
    let mut events = Vec::new();
    match value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "message_start" => {
            let usage = parse_usage(value.pointer("/message/usage"));
            merge_usage(&mut state.usage, &usage);
        }
        "content_block_start" => {
            let block = value.get("content_block").unwrap_or(value);
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                events.push(Ok(StreamEvent::ToolCallDelta {
                    index: value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize,
                    id: block.get("id").and_then(Value::as_str).map(String::from),
                    name: block.get("name").and_then(Value::as_str).map(String::from),
                    arguments_delta: String::new(),
                }));
            } else if let Some(text) = block.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    events.push(Ok(StreamEvent::TextDelta(text.to_string())));
                }
            }
        }
        "content_block_delta" => {
            let delta = value.get("delta").unwrap_or(value);
            if let Some(text) = delta.get("text").and_then(Value::as_str) {
                events.push(Ok(StreamEvent::TextDelta(text.to_string())));
            }
            if let Some(arguments) = delta.get("partial_json").and_then(Value::as_str) {
                events.push(Ok(StreamEvent::ToolCallDelta {
                    index: value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize,
                    id: None,
                    name: None,
                    arguments_delta: arguments.to_string(),
                }));
            }
        }
        "message_delta" => {
            if let Some(reason) = value.pointer("/delta/stop_reason").and_then(Value::as_str) {
                state.finish_reason = reason.to_string();
            }
            let usage = parse_usage(value.get("usage"));
            merge_usage(&mut state.usage, &usage);
        }
        "message_stop" => {
            if !state.done_sent {
                state.done_sent = true;
                events.push(Ok(StreamEvent::Done {
                    usage: state.usage.clone(),
                    finish_reason: state.finish_reason.clone(),
                }));
            }
        }
        "error" => {
            let detail = value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("Anthropic stream error");
            events.push(Err(provider_error(detail)));
            state.finished = true;
        }
        _ => {}
    }
    events
}

pub(crate) fn parse_usage(value: Option<&Value>) -> Usage {
    // Anthropic's `input_tokens` already excludes the cached portion, so the
    // cache counts are kept alongside it rather than subtracted — the opposite
    // of the OpenAI family. `Usage::total_input` re-joins them.
    let count = |key: &str| {
        value
            .and_then(|usage| usage.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize
    };
    Usage {
        prompt_tokens: count("input_tokens"),
        completion_tokens: count("output_tokens"),
        cache_read_tokens: count("cache_read_input_tokens"),
        cache_write_tokens: count("cache_creation_input_tokens"),
    }
}

fn merge_usage(target: &mut Usage, update: &Usage) {
    target.prompt_tokens = target.prompt_tokens.max(update.prompt_tokens);
    target.completion_tokens = target.completion_tokens.max(update.completion_tokens);
    // The cache counts arrive on `message_start` and are absent from the later
    // deltas; `max` keeps them instead of letting a zero overwrite them.
    target.cache_read_tokens = target.cache_read_tokens.max(update.cache_read_tokens);
    target.cache_write_tokens = target.cache_write_tokens.max(update.cache_write_tokens);
}

fn map_reqwest_error(error: reqwest::Error) -> NexusError {
    if error.is_timeout() {
        NexusError::ModelTimeout(0)
    } else {
        provider_error(format!("request failed: {error}"))
    }
}

fn provider_error(message: impl Into<String>) -> NexusError {
    NexusError::Provider {
        provider: "anthropic".into(),
        message: message.into(),
    }
}

fn truncate_error(text: &str) -> String {
    text.trim().chars().take(400).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_tool_calls_and_results_to_content_blocks() {
        let messages = vec![
            ChatMessage {
                role: Role::Assistant,
                content: "checking".into(),
                tool_calls: vec![ToolCallRequest {
                    id: "call-1".into(),
                    name: "fs_read".into(),
                    arguments: "{\"path\":\"a\"}".into(),
                }],
                tool_call_id: None,
                name: None,
                provider_private: None,
            },
            ChatMessage::tool_result("call-1", "fs_read", "contents"),
        ];
        let converted = anthropic_messages(&messages);
        assert_eq!(converted.len(), 2);
        assert_eq!(converted[0]["content"][1]["type"], "tool_use");
        assert_eq!(converted[1]["content"][0]["type"], "tool_result");
    }

    #[test]
    fn parses_native_tool_use_completion() {
        let completion = parse_completion(&json!({
            "content": [
                {"type":"text","text":"read it"},
                {"type":"tool_use","id":"call-1","name":"fs_read","input":{"path":"a"}}
            ],
            "usage":{"input_tokens":10,"output_tokens":4},
            "stop_reason":"tool_use"
        }));
        assert_eq!(completion.content, "read it");
        assert_eq!(completion.tool_calls[0].name, "fs_read");
        assert_eq!(completion.usage.prompt_tokens, 10);
    }

    #[test]
    fn native_effort_uses_output_config() {
        let config = ModelConfig {
            provider: "anthropic".into(),
            base_url: ANTHROPIC_DEFAULT.into(),
            model: "exact-model".into(),
            resolved_api_key: Some(nexus_core::SecretString::new("test-only")),
            reasoning_effort: Some("medium".into()),
            ..Default::default()
        };
        let provider = AnthropicProvider::new(&config).expect("provider");
        let body = provider.build_body(&CompletionRequest::default(), false);
        assert_eq!(body["output_config"]["effort"], "medium");
    }

    fn cache_config(prompt_cache: Option<bool>, ttl: Option<&str>) -> ModelConfig {
        ModelConfig {
            provider: "anthropic".into(),
            base_url: ANTHROPIC_DEFAULT.into(),
            model: "exact-model".into(),
            resolved_api_key: Some(nexus_core::SecretString::new("test-only")),
            prompt_cache,
            prompt_cache_ttl: ttl.map(str::to_string),
            ..Default::default()
        }
    }

    fn conversation() -> CompletionRequest {
        CompletionRequest {
            messages: vec![
                ChatMessage::system("stable rules"),
                ChatMessage::user("do the thing"),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn marks_the_last_system_block_and_the_last_message_block() {
        let provider = AnthropicProvider::new(&cache_config(None, None)).expect("provider");
        let body = provider.build_body(&conversation(), false);
        // The system array form is what carries a breakpoint; the string form
        // cannot, and Anthropic renders tools before system, so this one
        // marker covers the tool schemas too.
        assert_eq!(body["system"][0]["text"], "stable rules");
        assert_eq!(
            body["system"][0]["cache_control"],
            json!({"type":"ephemeral"})
        );
        let last = &body["messages"][0]["content"][0];
        assert_eq!(last["cache_control"], json!({"type":"ephemeral"}));
    }

    #[test]
    fn long_ttl_is_opt_in_per_model() {
        let provider = AnthropicProvider::new(&cache_config(None, Some("1h"))).expect("provider");
        let body = provider.build_body(&conversation(), false);
        assert_eq!(
            body["system"][0]["cache_control"],
            json!({"type":"ephemeral","ttl":"1h"})
        );
        // Anything unrecognized falls back to the cheap default rather than
        // being forwarded to the API verbatim.
        let provider = AnthropicProvider::new(&cache_config(None, Some("7d"))).expect("provider");
        let body = provider.build_body(&conversation(), false);
        assert_eq!(
            body["system"][0]["cache_control"],
            json!({"type":"ephemeral"})
        );
    }

    #[test]
    fn disabling_the_cache_sends_the_pre_cache_body_verbatim() {
        let off = AnthropicProvider::new(&cache_config(Some(false), None)).expect("provider");
        let body = off.build_body(&conversation(), false);
        assert_eq!(body["system"], json!("stable rules"));
        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_none());
    }

    #[test]
    fn cache_counts_sit_alongside_the_anthropic_input_count() {
        let usage = parse_usage(Some(&json!({
            "input_tokens": 12,
            "output_tokens": 5,
            "cache_read_input_tokens": 900,
            "cache_creation_input_tokens": 100,
        })));
        assert_eq!(usage.prompt_tokens, 12);
        assert_eq!(usage.cache_read_tokens, 900);
        assert_eq!(usage.cache_write_tokens, 100);
        // Anthropic excludes cached tokens from `input_tokens`, so the whole
        // prompt is the sum — the number the budget and manifest must see.
        assert_eq!(usage.total_input(), 1012);
    }

    #[test]
    fn usage_without_cache_fields_parses_as_zero() {
        let usage = parse_usage(Some(&json!({"input_tokens": 40, "output_tokens": 2})));
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.cache_write_tokens, 0);
        assert_eq!(usage.total_input(), 40);
    }

    #[test]
    fn streamed_cache_counts_survive_later_deltas() {
        let mut total = parse_usage(Some(&json!({
            "input_tokens": 12,
            "cache_read_input_tokens": 900,
        })));
        // Anthropic restates usage on `message_delta` without the cache fields.
        merge_usage(&mut total, &parse_usage(Some(&json!({"output_tokens": 7}))));
        assert_eq!(total.cache_read_tokens, 900);
        assert_eq!(total.completion_tokens, 7);
    }
}
