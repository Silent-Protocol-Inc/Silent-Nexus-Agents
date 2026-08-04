//! Codex ("Sign in with ChatGPT") inference provider.
//!
//! Speaks the ChatGPT-backend Responses API (`POST {base}/responses`,
//! streaming SSE) — the same wire protocol the official `codex` CLI uses —
//! authenticated with the Codex OAuth session (isolated Silent Nexus profile
//! first, then the user's own CLI session read-only). This is what makes the
//! models on the operator's ChatGPT plan usable for inference; a plan OAuth
//! token cannot call `api.openai.com/v1/chat/completions`.
//!
//! Wire facts (verified empirically against the live backend):
//! * requests must set `stream: true`; the response is always SSE;
//! * `max_output_tokens` and `temperature` are rejected ("Unsupported
//!   parameter") — the backend governs output length itself;
//! * tool calls arrive as complete `function_call` output items, usage on the
//!   final `response.completed` event.

use crate::provider::ModelProvider;
use crate::sse::{SseItem, SseParser};
use crate::types::*;
use futures::stream::BoxStream;
use futures::StreamExt;
use nexus_core::config::ModelConfig;
use nexus_core::{NexusError, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The ChatGPT backend the codex CLI targets.
pub const CODEX_BACKEND_DEFAULT: &str = "https://chatgpt.com/backend-api/codex";
pub const OPENAI_RESPONSES_DEFAULT: &str = "https://api.openai.com/v1";

#[derive(Debug, Clone)]
pub struct CodexResponsesProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    bearer: String,
    account_id: Option<String>,
    auth_mode: &'static str,
    config: ModelConfig,
}

impl CodexResponsesProvider {
    pub fn new(config: &ModelConfig) -> Result<Self> {
        let cred = crate::codex_auth::load_with_consent(config.allow_existing_codex)?.ok_or_else(
            || {
                NexusError::Config(
                    "provider `codex` needs a Codex session. Log in via /login (device login \
                 or import), explicitly consent to the existing CLI login, or run \
                 `snx auth login`, then retry."
                        .into(),
                )
            },
        )?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .connect_timeout(Duration::from_secs(10))
            .danger_accept_invalid_certs(!config.tls_verify)
            .build()
            .map_err(|e| NexusError::Provider {
                provider: "codex".into(),
                message: format!("http client: {e}"),
            })?;
        let base_url = default_base_url(config, cred.mode);
        Ok(Self {
            client,
            base_url,
            model: config.model.clone(),
            bearer: cred.bearer,
            account_id: cred.account_id,
            auth_mode: cred.mode,
            config: config.clone(),
        })
    }

    /// Build the request body plus the wire→real tool-name map. The backend
    /// only accepts tool names matching `^[a-zA-Z0-9_-]+$`, while harness
    /// tools use dotted names (`fs.read`); names are rewritten on the way out
    /// and translated back when the model calls them.
    fn build_body(&self, request: &CompletionRequest) -> Result<(Value, BTreeMap<String, String>)> {
        let (real_to_wire, name_map) = wire_tool_maps(request);
        // Responses API separates the system prompt ("instructions") from the
        // input item list.
        let instructions: String = request
            .messages
            .iter()
            .filter(|m| m.role == Role::System)
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut input = Vec::new();
        for m in &request.messages {
            match m.role {
                Role::System => {}
                Role::User => input.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": m.content}],
                })),
                Role::Assistant => {
                    if !m.content.is_empty() {
                        input.push(json!({
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": m.content}],
                        }));
                    }
                    for c in &m.tool_calls {
                        // Historical calls may refer to a tool that is not
                        // exposed on this turn (for example, a simple
                        // follow-up after an earlier `repo.structure` call).
                        // In that case `real_to_wire` has no entry, but replay
                        // still has to obey the Responses API function-name
                        // contract. Never send the raw harness name.
                        let wire_name = real_to_wire
                            .get(&c.name)
                            .cloned()
                            .unwrap_or_else(|| wire_tool_name(&c.name));
                        input.push(json!({
                            "type": "function_call",
                            "name": wire_name,
                            "arguments": c.arguments,
                            "call_id": c.id,
                        }));
                    }
                }
                Role::Tool => input.push(json!({
                    "type": "function_call_output",
                    "call_id": m.tool_call_id.as_deref().unwrap_or(""),
                    "output": m.content,
                })),
            }
        }
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": real_to_wire
                        .get(&t.name)
                        .expect("all exposed tools are included in the wire-name map"),
                    "description": t.description,
                    "strict": false,
                    "parameters": t.parameters,
                })
            })
            .collect();
        let effort = self.config.reasoning_effort.as_deref().unwrap_or("medium");
        // The backend rejects `max_output_tokens` and `temperature`; a
        // CompletionRequest asking for them is honored as best-effort only.
        let mut body = json!({
            "model": self.model,
            "instructions": instructions,
            "input": input,
            "tools": tools,
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "reasoning": {"effort": effort},
            "store": false,
            "stream": true,
            "include": [],
        });
        // Prefix caching here is automatic — the backend already reports
        // `cached_tokens` without being asked. The key only pins the routing so
        // successive calls in one turn keep finding the same warm copy. It is
        // deliberately independent of `store`, which stays false.
        if self.config.prompt_cache_enabled() {
            body["prompt_cache_key"] = json!(request.prompt_cache_key());
        }
        validate_serialized_request(&body, &name_map)?;
        Ok((body, name_map))
    }

    fn request(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut req = self
            .client
            .post(format!("{}/responses", self.base_url))
            .json(body)
            .bearer_auth(&self.bearer)
            .header("Accept", "text/event-stream");
        if self.auth_mode == "oauth" {
            req = req
                .header("OpenAI-Beta", "responses=experimental")
                .header("originator", "codex_cli_rs");
        }
        if let Some(acct) = &self.account_id {
            req = req.header("chatgpt-account-id", acct);
        }
        req
    }

    fn provider_err(message: impl Into<String>) -> NexusError {
        NexusError::Provider {
            provider: "codex".into(),
            message: message.into(),
        }
    }
}

fn default_base_url(config: &ModelConfig, auth_mode: &str) -> String {
    let trimmed = config.base_url.trim_end_matches('/');
    if !trimmed.is_empty() && trimmed != "http://127.0.0.1:8080/v1" {
        return trimmed.to_string();
    }
    if auth_mode == "api_key" {
        OPENAI_RESPONSES_DEFAULT.to_string()
    } else {
        CODEX_BACKEND_DEFAULT.to_string()
    }
}

/// Rewrite a harness tool name into the `^[a-zA-Z0-9_-]+$` charset the
/// backend requires (`fs.read` → `fs_read`).
fn wire_tool_name(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        s.push_str("tool");
    }
    if s.len() > 64 {
        let suffix = format!("_{:08x}", stable_name_hash(name));
        s.truncate(64 - suffix.len());
        s.push_str(&suffix);
    }
    s
}

/// Build one deterministic mapping over every function name in the request,
/// including historical assistant calls that are not exposed on this turn.
/// Sorting makes collision resolution independent of tool-registration order.
fn wire_tool_maps(
    request: &CompletionRequest,
) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let mut real_names: Vec<String> = request.tools.iter().map(|tool| tool.name.clone()).collect();
    for message in &request.messages {
        real_names.extend(message.tool_calls.iter().map(|call| call.name.clone()));
    }
    real_names.sort();
    real_names.dedup();

    let mut base_counts: BTreeMap<String, usize> = BTreeMap::new();
    for real in &real_names {
        *base_counts.entry(wire_tool_name(real)).or_default() += 1;
    }

    let mut real_to_wire = BTreeMap::new();
    let mut wire_to_real = BTreeMap::new();
    for real in real_names {
        let base = wire_tool_name(&real);
        let mut wire = if base_counts.get(&base).copied().unwrap_or_default() > 1 {
            collision_name(&base, &real, 0)
        } else {
            base.clone()
        };
        let mut discriminator = 1u32;
        while wire_to_real.contains_key(&wire) {
            wire = collision_name(&base, &real, discriminator);
            discriminator += 1;
        }
        wire_to_real.insert(wire.clone(), real.clone());
        real_to_wire.insert(real, wire);
    }
    (real_to_wire, wire_to_real)
}

fn collision_name(base: &str, real: &str, discriminator: u32) -> String {
    let hash_input = if discriminator == 0 {
        real.to_string()
    } else {
        format!("{real}#{discriminator}")
    };
    let suffix = format!("_{:08x}", stable_name_hash(&hash_input));
    let mut prefix = base.to_string();
    prefix.truncate(64usize.saturating_sub(suffix.len()));
    format!("{prefix}{suffix}")
}

/// Stable FNV-1a hash used only as a compact collision discriminator.
fn stable_name_hash(name: &str) -> u32 {
    let mut hash = 0x811c9dc5u32;
    for byte in name.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

fn valid_wire_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Validate the complete provider payload before transmission. A local
/// serialization bug is deterministic and must never be retried over HTTP.
fn validate_serialized_request(
    body: &Value,
    wire_to_real: &BTreeMap<String, String>,
) -> Result<()> {
    serde_json::to_vec(body).map_err(|error| NexusError::Provider {
        provider: "codex".into(),
        message: format!("local request serialization failed: {error}"),
    })?;

    let validate_name = |name: &str, location: &str| -> Result<()> {
        if !valid_wire_tool_name(name) {
            return Err(NexusError::Provider {
                provider: "codex".into(),
                message: format!(
                    "local request validation failed: {location} tool name `{name}` violates ^[a-zA-Z0-9_-]+$ or exceeds 64 bytes"
                ),
            });
        }
        if !wire_to_real.contains_key(name) {
            return Err(NexusError::Provider {
                provider: "codex".into(),
                message: format!(
                    "local request validation failed: {location} tool name `{name}` has no reverse mapping"
                ),
            });
        }
        Ok(())
    };

    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        let mut seen = BTreeMap::<String, ()>::new();
        for tool in tools {
            let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
            validate_name(name, "exposed")?;
            if seen.insert(name.to_string(), ()).is_some() {
                return Err(NexusError::Provider {
                    provider: "codex".into(),
                    message: format!(
                        "local request validation failed: duplicate exposed tool name `{name}`"
                    ),
                });
            }
        }
    }
    if let Some(input) = body.get("input").and_then(Value::as_array) {
        for item in input {
            if item.get("type").and_then(Value::as_str) == Some("function_call") {
                validate_name(
                    item.get("name").and_then(Value::as_str).unwrap_or(""),
                    "historical",
                )?;
            }
        }
    }
    Ok(())
}

/// Convert one Responses-API SSE payload into stream events. `calls` counts
/// function calls seen so far (their `index` for delta grouping); `names`
/// maps wire tool names back to the harness names.
fn event_to_stream(
    v: &Value,
    calls: &mut usize,
    names: &BTreeMap<String, String>,
) -> Vec<StreamEvent> {
    let mut out = Vec::new();
    match v.get("type").and_then(Value::as_str).unwrap_or("") {
        "response.output_text.delta" => {
            if let Some(text) = v.get("delta").and_then(Value::as_str) {
                if !text.is_empty() {
                    out.push(StreamEvent::TextDelta(text.to_string()));
                }
            }
        }
        "response.output_item.done" => {
            let item = &v["item"];
            if item.get("type").and_then(Value::as_str) == Some("function_call") {
                out.push(StreamEvent::ToolCallDelta {
                    index: *calls,
                    id: item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .map(String::from),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .map(|n| names.get(n).cloned().unwrap_or_else(|| n.to_string())),
                    arguments_delta: item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}")
                        .to_string(),
                });
                *calls += 1;
            }
        }
        "response.completed" => {
            // `cached_tokens` and `cache_write_tokens` are reported inside
            // `input_tokens_details` as parts of `input_tokens`, not in
            // addition to it — confirmed live: total_tokens stayed equal to
            // input + output while cached_tokens was 1792. A missing detail
            // object is simply an uncached call.
            let count = |path: &str| v.pointer(path).and_then(Value::as_u64).unwrap_or(0) as usize;
            let usage = Usage::from_inclusive_input(
                count("/response/usage/input_tokens"),
                count("/response/usage/input_tokens_details/cached_tokens"),
                count("/response/usage/input_tokens_details/cache_write_tokens"),
                count("/response/usage/output_tokens"),
            );
            let finish = if *calls > 0 { "tool_calls" } else { "stop" };
            out.push(StreamEvent::Done {
                usage,
                finish_reason: finish.to_string(),
            });
        }
        _ => {}
    }
    out
}

/// A terminal failure payload, if this event is one.
fn event_failure(v: &Value) -> Option<String> {
    match v.get("type").and_then(Value::as_str).unwrap_or("") {
        "response.failed" => Some(
            v.pointer("/response/error/message")
                .and_then(Value::as_str)
                .unwrap_or("response.failed with no error detail")
                .to_string(),
        ),
        "error" => Some(
            v.get("message")
                .and_then(Value::as_str)
                .unwrap_or("provider stream error")
                .to_string(),
        ),
        _ => None,
    }
}

#[async_trait::async_trait]
impl ModelProvider for CodexResponsesProvider {
    fn kind(&self) -> &'static str {
        "codex"
    }

    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            model_id: self.model.clone(),
            provider_kind: "codex".into(),
            streaming: true,
            native_tool_calls: self.config.native_tool_calls.unwrap_or(true),
            structured_output: false,
            image_input: false,
            embeddings: false,
            context_window: self.config.context_window,
            max_output_tokens: self.config.max_output_tokens,
            context_limit_source: self.config.context_limit_source,
            output_limit_source: self.config.output_limit_source,
            reasoning_controls: true,
            reasoning: ReasoningProfile {
                supported_efforts: self.config.reasoning_effort.clone().into_iter().collect(),
                default_effort: self.config.reasoning_effort.clone(),
                control: if self.config.reasoning_effort.is_some() {
                    crate::ReasoningControl::Mandatory
                } else {
                    crate::ReasoningControl::ProviderManaged
                },
                mandatory: true,
                provider_managed: self.config.reasoning_effort.is_none(),
                provenance: ReasoningProvenance::ConfiguredDefault,
            },
            system_prompt: true,
            // The Responses API carries them in `instructions`, separate from
            // the input item list.
            instruction_channel: nexus_core::persona::InstructionChannel::InstructionsField,
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

    /// The backend is stream-only; aggregate the stream for callers that want
    /// a single completion.
    async fn complete(&self, request: CompletionRequest) -> Result<Completion> {
        let mut stream = self.stream(request).await?;
        let mut content = String::new();
        let mut calls: Vec<ToolCallRequest> = Vec::new();
        let mut usage = Usage::default();
        let mut finish = "stop".to_string();
        while let Some(event) = stream.next().await {
            match event? {
                StreamEvent::TextDelta(t) => content.push_str(&t),
                StreamEvent::ProviderPrivateDelta(_) => {}
                StreamEvent::ToolCallDelta {
                    id,
                    name,
                    arguments_delta,
                    ..
                } => calls.push(ToolCallRequest {
                    id: id.unwrap_or_else(|| format!("call_{}", calls.len())),
                    name: name.unwrap_or_default(),
                    arguments: arguments_delta,
                }),
                StreamEvent::Done {
                    usage: u,
                    finish_reason,
                } => {
                    usage = u;
                    finish = finish_reason;
                }
            }
        }
        Ok(Completion {
            content,
            tool_calls: calls,
            usage,
            finish_reason: finish,
            provider_private: None,
        })
    }

    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        let (body, names) = self.build_body(&request)?;
        let timeout = self.config.timeout_secs;
        let response = self.request(&body).send().await.map_err(|e| {
            if e.is_timeout() {
                NexusError::ModelTimeout(timeout)
            } else {
                Self::provider_err(format!("request failed: {e}"))
            }
        })?;
        let status = response.status();
        if !status.is_success() {
            let headers = response.headers().clone();
            let text = response.text().await.unwrap_or_default();
            // A plan window being spent is not a broken session. Distinguished
            // here so the loop can pause and say when it resets, instead of
            // reporting the same generic provider failure as a bad request.
            if crate::rate_limit::is_rate_limit(status.as_u16()) {
                return Err(crate::rate_limit::error(
                    "codex",
                    &headers,
                    status.as_u16(),
                    &text,
                ));
            }
            let hint = if status.as_u16() == 401 {
                " — the Codex session may have expired; run /login again"
            } else {
                ""
            };
            return Err(Self::provider_err(format!(
                "HTTP {status}: {}{hint}",
                text.trim().chars().take(400).collect::<String>()
            )));
        }
        let byte_stream = response.bytes_stream();
        let stream = futures::stream::unfold(
            (
                byte_stream,
                SseParser::new(),
                Vec::<StreamEvent>::new(),
                false,
                0usize,
                names,
            ),
            move |(mut bytes, mut parser, mut pending, mut finished, mut calls, names)| async move {
                loop {
                    if let Some(event) = pending.pop() {
                        let done = matches!(event, StreamEvent::Done { .. });
                        return Some((
                            Ok(event),
                            (bytes, parser, pending, finished || done, calls, names),
                        ));
                    }
                    if finished {
                        return None;
                    }
                    match bytes.next().await {
                        Some(Ok(chunk)) => {
                            let mut new_events = Vec::new();
                            for item in parser.feed(&chunk) {
                                match item {
                                    SseItem::Done => finished = true,
                                    SseItem::Data(data) => {
                                        let v: Value = match serde_json::from_str(&data) {
                                            Ok(v) => v,
                                            Err(e) => {
                                                return Some((
                                                    Err(Self::provider_err(format!(
                                                        "invalid stream chunk: {e}"
                                                    ))),
                                                    (bytes, parser, pending, true, calls, names),
                                                ));
                                            }
                                        };
                                        if let Some(msg) = event_failure(&v) {
                                            return Some((
                                                Err(Self::provider_err(msg)),
                                                (bytes, parser, pending, true, calls, names),
                                            ));
                                        }
                                        new_events.extend(event_to_stream(&v, &mut calls, &names));
                                    }
                                }
                            }
                            // pop() takes from the back; keep order.
                            new_events.reverse();
                            pending = new_events;
                        }
                        Some(Err(e)) => {
                            return Some((
                                Err(Self::provider_err(format!("stream interrupted: {e}"))),
                                (bytes, parser, pending, true, calls, names),
                            ));
                        }
                        None => return None,
                    }
                }
            },
        );
        Ok(stream.boxed())
    }

    async fn health(&self) -> ProviderHealth {
        // No cheap read endpoint exists on the ChatGPT backend; reachability
        // is any HTTP answer from the host (the API itself is POST-only).
        let start = Instant::now();
        match self
            .client
            .get(&self.base_url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(r) => ProviderHealth {
                reachable: true,
                detail: format!(
                    "backend reachable ({}); session: {}",
                    r.status(),
                    if self.account_id.is_some() {
                        "oauth"
                    } else {
                        "api key"
                    }
                ),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(e) => ProviderHealth {
                reachable: false,
                detail: format!("unreachable: {e}"),
                latency_ms: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider_for_test() -> CodexResponsesProvider {
        CodexResponsesProvider {
            client: reqwest::Client::new(),
            base_url: CODEX_BACKEND_DEFAULT.into(),
            model: "gpt-5.5".into(),
            bearer: "test".into(),
            account_id: Some("acct".into()),
            auth_mode: "oauth",
            config: ModelConfig {
                provider: "codex".into(),
                model: "gpt-5.5".into(),
                ..Default::default()
            },
        }
    }

    /// Usage off a `response.completed` event, as the stream reader sees it.
    fn done_usage(event: &Value) -> Usage {
        let mut calls = 0usize;
        match event_to_stream(event, &mut calls, &BTreeMap::new()).pop() {
            Some(StreamEvent::Done { usage, .. }) => usage,
            other => panic!("expected a Done event, got {other:?}"),
        }
    }

    #[test]
    fn cached_tokens_are_a_subset_of_the_reported_input() {
        // Shape captured from a live `response.completed`, cache warm.
        let usage = done_usage(&json!({
            "type": "response.completed",
            "response": {"usage": {
                "input_tokens": 2296,
                "input_tokens_details": {"cached_tokens": 1792, "cache_write_tokens": 0},
                "output_tokens": 80,
                "total_tokens": 2376,
            }}
        }));
        assert_eq!(usage.cache_read_tokens, 1792);
        assert_eq!(usage.prompt_tokens, 504);
        assert_eq!(usage.total_input(), 2296);
    }

    #[test]
    fn a_cold_call_reports_no_cache_and_the_full_prompt() {
        let usage = done_usage(&json!({
            "type": "response.completed",
            "response": {"usage": {"input_tokens": 504, "output_tokens": 80}}
        }));
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.total_input(), 504);
    }

    #[test]
    fn the_cache_key_pins_routing_without_enabling_storage() {
        let provider = provider_for_test();
        let request = CompletionRequest {
            messages: vec![
                ChatMessage::system("stable rules"),
                ChatMessage::user("first ask"),
            ],
            ..Default::default()
        };
        let (body, _) = provider.build_body(&request).expect("body");
        assert!(body["prompt_cache_key"].is_string());
        // `store: false` is a privacy decision the cache key must not disturb.
        assert_eq!(body["store"], json!(false));
    }

    #[test]
    fn endpoint_defaults_follow_the_credential_platform() {
        let cfg = ModelConfig {
            base_url: String::new(),
            ..Default::default()
        };
        assert_eq!(default_base_url(&cfg, "oauth"), CODEX_BACKEND_DEFAULT);
        assert_eq!(default_base_url(&cfg, "api_key"), OPENAI_RESPONSES_DEFAULT);
        let custom = ModelConfig {
            base_url: "https://gateway.example/v1/".into(),
            ..Default::default()
        };
        assert_eq!(
            default_base_url(&custom, "api_key"),
            "https://gateway.example/v1"
        );
    }

    #[test]
    fn body_separates_instructions_and_maps_history() {
        let p = provider_for_test();
        let mut req = CompletionRequest {
            messages: vec![
                ChatMessage::system("sys rules"),
                ChatMessage::user("hi"),
                ChatMessage::assistant("using a tool"),
                ChatMessage::tool_result("call_1", "get", "result"),
            ],
            ..Default::default()
        };
        req.messages[2].tool_calls.push(ToolCallRequest {
            id: "call_1".into(),
            name: "get".into(),
            arguments: "{}".into(),
        });
        let (body, _names) = p.build_body(&req).expect("valid body");
        assert_eq!(body["instructions"], "sys rules");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert!(body.get("max_output_tokens").is_none(), "rejected upstream");
        assert!(body.get("temperature").is_none(), "rejected upstream");
        let input = body["input"].as_array().expect("input");
        assert_eq!(input.len(), 4); // user msg, assistant msg, function_call, output
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[3]["type"], "function_call_output");
        assert_eq!(input[3]["call_id"], "call_1");
    }

    #[test]
    fn dotted_tool_names_are_rewritten_and_mapped_back() {
        let p = provider_for_test();
        let req = CompletionRequest {
            messages: vec![ChatMessage::user("hi")],
            tools: vec![ToolSpec {
                name: "fs.read".into(),
                description: "read a file".into(),
                parameters: json!({"type": "object"}),
            }],
            ..Default::default()
        };
        let (body, names) = p.build_body(&req).expect("valid body");
        // The backend requires ^[a-zA-Z0-9_-]+$; dots must be gone.
        assert_eq!(body["tools"][0]["name"], "fs_read");
        assert_eq!(names.get("fs_read").map(String::as_str), Some("fs.read"));

        // A call to the wire name comes back as the harness name.
        let mut calls = 0usize;
        let call: Value = serde_json::from_str(
            r#"{"type":"response.output_item.done","item":{"type":"function_call","name":"fs_read","arguments":"{}","call_id":"c1"}}"#,
        )
        .expect("json");
        match &event_to_stream(&call, &mut calls, &names)[..] {
            [StreamEvent::ToolCallDelta { name, .. }] => {
                assert_eq!(name.as_deref(), Some("fs.read"));
            }
            other => panic!("unexpected events: {other:?}"),
        }
    }

    #[test]
    fn historical_dotted_tool_names_are_sanitized_when_tool_is_not_exposed() {
        let p = provider_for_test();
        let mut historical_call = ChatMessage::assistant("");
        historical_call.tool_calls.push(ToolCallRequest {
            id: "call_history".into(),
            name: "repo.structure".into(),
            arguments: "{}".into(),
        });
        let req = CompletionRequest {
            messages: vec![
                ChatMessage::user("inspect the repository"),
                historical_call,
                ChatMessage::tool_result("call_history", "repo.structure", "top-level: src"),
                ChatMessage::user("yes"),
            ],
            // A simple follow-up can expose no tools at all. The historical
            // function call must still remain valid input for the API.
            tools: vec![],
            ..Default::default()
        };

        let (body, names) = p.build_body(&req).expect("valid body");
        let input = body["input"].as_array().expect("input");
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["name"], "repo_structure");
        assert!(input[1]["name"]
            .as_str()
            .expect("function name")
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
        assert_eq!(
            names.get("repo_structure").map(String::as_str),
            Some("repo.structure"),
            "historical calls remain reversible even when no tools are exposed"
        );
    }

    #[test]
    fn exposed_and_historical_collisions_are_stable_and_reversible() {
        let p = provider_for_test();
        let mut historical = ChatMessage::assistant("");
        historical.tool_calls.push(ToolCallRequest {
            id: "call_history".into(),
            name: "fs.read".into(),
            arguments: "{}".into(),
        });
        let req = CompletionRequest {
            messages: vec![historical],
            tools: vec![ToolSpec {
                name: "fs_read".into(),
                description: "collides after normalization".into(),
                parameters: json!({"type": "object"}),
            }],
            ..Default::default()
        };

        let (body, names) = p.build_body(&req).expect("valid collision body");
        let exposed = body["tools"][0]["name"].as_str().expect("wire tool");
        let historical = body["input"][0]["name"].as_str().expect("history tool");
        assert_ne!(exposed, historical);
        assert_eq!(names.get(exposed).map(String::as_str), Some("fs_read"));
        assert_eq!(names.get(historical).map(String::as_str), Some("fs.read"));
        assert!(valid_wire_tool_name(exposed));
        assert!(valid_wire_tool_name(historical));

        let (body_again, names_again) = p.build_body(&req).expect("stable body");
        assert_eq!(body, body_again);
        assert_eq!(names, names_again);
    }

    #[test]
    fn every_wire_name_is_locally_validated() {
        let p = provider_for_test();
        let req = CompletionRequest {
            tools: vec![ToolSpec {
                name: "namespace.tool/with spaces and unicode-λ".repeat(4),
                description: "long invalid harness name".into(),
                parameters: json!({"type": "object"}),
            }],
            ..Default::default()
        };
        let (body, names) = p.build_body(&req).expect("normalized body");
        let wire = body["tools"][0]["name"].as_str().expect("wire");
        assert!(valid_wire_tool_name(wire));
        assert!(wire.len() <= 64);
        validate_serialized_request(&body, &names).expect("local validation");
    }

    #[test]
    fn sse_events_map_to_stream_events() {
        let mut calls = 0usize;
        let text: Value =
            serde_json::from_str(r#"{"type":"response.output_text.delta","delta":"hey"}"#)
                .expect("json");
        let names = BTreeMap::new();
        let ev = event_to_stream(&text, &mut calls, &names);
        assert!(matches!(&ev[..], [StreamEvent::TextDelta(t)] if t == "hey"));

        let call: Value = serde_json::from_str(
            r#"{"type":"response.output_item.done","item":{"type":"function_call","name":"get_weather","arguments":"{\"city\":\"Paris\"}","call_id":"call_9"}}"#,
        )
        .expect("json");
        let ev = event_to_stream(&call, &mut calls, &names);
        match &ev[..] {
            [StreamEvent::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            }] => {
                assert_eq!(*index, 0);
                assert_eq!(id.as_deref(), Some("call_9"));
                assert_eq!(name.as_deref(), Some("get_weather"));
                assert!(arguments_delta.contains("Paris"));
            }
            other => panic!("unexpected events: {other:?}"),
        }

        let done: Value = serde_json::from_str(
            r#"{"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":3}}}"#,
        )
        .expect("json");
        let ev = event_to_stream(&done, &mut calls, &names);
        match &ev[..] {
            [StreamEvent::Done {
                usage,
                finish_reason,
            }] => {
                assert_eq!(usage.prompt_tokens, 10);
                assert_eq!(usage.completion_tokens, 3);
                assert_eq!(finish_reason, "tool_calls"); // a call was seen
            }
            other => panic!("unexpected events: {other:?}"),
        }
    }

    #[test]
    fn failure_events_are_detected() {
        let v: Value = serde_json::from_str(
            r#"{"type":"response.failed","response":{"error":{"message":"quota exceeded"}}}"#,
        )
        .expect("json");
        assert_eq!(event_failure(&v).as_deref(), Some("quota exceeded"));
        let ok: Value = serde_json::from_str(r#"{"type":"response.created"}"#).expect("json");
        assert!(event_failure(&ok).is_none());
    }
}
