//! OpenAI-compatible chat provider.
//!
//! Serves three configured provider kinds:
//! * `llamacpp` — llama.cpp `llama-server` in OpenAI mode;
//! * `openai_compatible` — any `/v1/chat/completions` endpoint;
//! * `custom_http` — same protocol with custom headers / completion path.

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

#[derive(Debug, Clone)]
pub struct OpenAiCompatProvider {
    kind: &'static str,
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
    extra_headers: BTreeMap<String, String>,
    config: ModelConfig,
    local: bool,
    accelerator: Option<String>,
}

/// Host GPU report, detected once and cached for the process.
pub(crate) fn host_gpu() -> &'static nexus_core::gpu::GpuReport {
    use std::sync::OnceLock;
    static REPORT: OnceLock<nexus_core::gpu::GpuReport> = OnceLock::new();
    REPORT.get_or_init(nexus_core::gpu::detect)
}

/// The accelerator a *local* model can use: the host GPU's backend when one is
/// present, `CPU` when the model runs locally without a GPU, and `None` for a
/// remote endpoint (whose hardware the harness cannot observe).
pub(crate) fn local_accelerator(local: bool) -> Option<String> {
    if !local {
        return None;
    }
    Some(
        host_gpu()
            .primary_backend()
            .map(|b| b.to_string())
            .unwrap_or_else(|| "CPU".to_string()),
    )
}

impl OpenAiCompatProvider {
    pub fn new(kind: &'static str, config: &ModelConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            // No client-wide response deadline: reqwest applies it to the
            // whole exchange, which killed healthy long streams — a llama.cpp
            // answer still arriving token by token was cut off the moment it
            // outlived `timeout_secs`. Deadlines are applied per phase below:
            // an allowance for the first token, then a stall timeout between
            // chunks.
            .connect_timeout(Duration::from_secs(10))
            .danger_accept_invalid_certs(!config.tls_verify)
            .build()
            .map_err(|e| NexusError::Provider {
                provider: kind.to_string(),
                message: format!("http client: {e}"),
            })?;
        let uses_codex = config.auth.as_deref() == Some("codex");
        let mut extra_headers = BTreeMap::new();

        // Resolve the bearer token. Precedence: a Codex ("Sign in with ChatGPT")
        // session when `auth = "codex"`, otherwise the API key named by
        // `api_key_env`. Keys never come from the config file itself.
        let api_key = if uses_codex {
            match crate::codex_auth::load_with_consent(config.allow_existing_codex)? {
                Some(cred) => {
                    if let Some(acct) = cred.account_id {
                        extra_headers.insert("chatgpt-account-id".to_string(), acct);
                    }
                    Some(cred.bearer)
                }
                None => {
                    return Err(NexusError::Config(
                        "auth = \"codex\" but no Codex session was found. Run `snx auth login` \
                         (which delegates to the `codex` CLI) or `codex login`, then retry."
                            .into(),
                    ));
                }
            }
        } else {
            // Precedence: environment variable, then a key resolved at
            // bootstrap from the credential store (`api_key_ref`).
            config
                .api_key_env
                .as_ref()
                .and_then(|var| std::env::var(var).ok())
                .filter(|v| !v.is_empty())
                .or_else(|| {
                    config
                        .resolved_api_key
                        .as_ref()
                        .map(|s| s.expose().to_string())
                        .filter(|v| !v.is_empty())
                })
        };

        // The dedicated `openai` kind (and Codex auth) target api.openai.com by
        // default. Treat an unset base_url — including the llama.cpp struct
        // default that a config inherits when the field is omitted — as "use the
        // OpenAI default", so an `openai` model never silently posts to :8080.
        let trimmed = config.base_url.trim_end_matches('/');
        let is_unset = trimmed.is_empty() || trimmed == "http://127.0.0.1:8080/v1";
        let base_url = if (kind == "openai" || uses_codex) && is_unset {
            "https://api.openai.com/v1".to_string()
        } else {
            trimmed.to_string()
        };
        if kind == "openai" && !uses_codex && api_key.is_none() {
            return Err(NexusError::Config(format!(
                "provider `openai` requires credentials: set `api_key_env` to the name \
                 of an environment variable holding your key (e.g. api_key_env = \"OPENAI_API_KEY\"), \
                 or set `auth = \"codex\"` to reuse a `codex login` session{}",
                match &config.api_key_env {
                    Some(v) => format!(" — `{v}` is unset or empty"),
                    None => String::new(),
                }
            )));
        }
        let local = is_local_url(&base_url);
        Ok(Self {
            kind,
            client,
            base_url,
            model: config.model.clone(),
            api_key,
            extra_headers,
            config: config.clone(),
            local,
            accelerator: local_accelerator(local),
        })
    }

    /// Add a custom header (custom_http provider kind).
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.extra_headers
            .insert(name.to_string(), value.to_string());
        self
    }

    fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    fn build_body(&self, request: &CompletionRequest, stream: bool) -> Value {
        let messages: Vec<Value> = request.messages.iter().map(message_to_json).collect();
        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "stream": stream,
        });
        if stream {
            body["stream_options"] = json!({"include_usage": true});
        }
        if let Some(t) = request.temperature.or(self.config.temperature) {
            body["temperature"] = json!(t);
        }
        let max = request.max_tokens.unwrap_or(self.config.max_output_tokens);
        body["max_tokens"] = json!(max);
        if !request.stop.is_empty() {
            body["stop"] = json!(request.stop);
        }
        if request.json_mode {
            body["response_format"] = json!({"type": "json_object"});
        }
        // Only OpenAI itself documents this field, and this adapter also serves
        // llama.cpp, vLLM, LM Studio and arbitrary custom endpoints — several
        // of which reject unknown keys outright. Same restraint as
        // `reasoning_effort` below. Caching still happens on servers that do it
        // implicitly; the key just pins which warm copy a repeat request finds.
        if self.kind == "openai" && self.config.prompt_cache_enabled() {
            body["prompt_cache_key"] = json!(request.prompt_cache_key());
        }
        if let Some(effort) = self.config.reasoning_effort.as_deref() {
            if self.base_url.contains("openrouter.ai/") {
                // OpenRouter's normalized contract. Reasoning output is
                // excluded at the wire boundary and therefore cannot enter a
                // stream, transcript, export, log, or artifact.
                body["reasoning"] = json!({"effort": effort, "exclude": true});
            } else if self.kind == "openai" {
                body["reasoning_effort"] = json!(effort);
            }
        }
        if !request.tools.is_empty() && self.capabilities().native_tool_calls {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = json!(tools);
        }
        body
    }

    fn request(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut req = self.client.post(self.chat_url()).json(body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        for (name, value) in &self.extra_headers {
            req = req.header(name, value);
        }
        req
    }

    fn provider_err(&self, message: impl Into<String>) -> NexusError {
        NexusError::Provider {
            provider: self.kind.to_string(),
            message: message.into(),
        }
    }
}

pub fn is_local_url(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .map(|h| h == "localhost" || h == "127.0.0.1" || h == "::1" || h == "[::1]")
        .unwrap_or(false)
}

fn message_to_json(m: &ChatMessage) -> Value {
    let role = match m.role {
        // Deliberately not "developer": this adapter also serves llama.cpp,
        // LM Studio, and arbitrary OpenAI-compatible endpoints, many of
        // which reject an unknown role outright. Folding onto "system"
        // keeps today's behavior everywhere.
        Role::System | Role::Developer => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };
    let mut v = json!({"role": role, "content": m.content});
    if !m.tool_calls.is_empty() {
        v["tool_calls"] = json!(m
            .tool_calls
            .iter()
            .map(|c| json!({
                "id": c.id,
                "type": "function",
                "function": {"name": c.name, "arguments": c.arguments}
            }))
            .collect::<Vec<_>>());
    }
    if let Some(id) = &m.tool_call_id {
        v["tool_call_id"] = json!(id);
    }
    if m.role == Role::Tool {
        if let Some(name) = &m.name {
            v["name"] = json!(name);
        }
    }
    v
}

fn parse_tool_calls(value: &Value) -> Vec<ToolCallRequest> {
    value
        .as_array()
        .map(|arr| {
            arr.iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    let f = c.get("function")?;
                    Some(ToolCallRequest {
                        id: c
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or(&format!("call_{i}"))
                            .to_string(),
                        name: f.get("name")?.as_str()?.to_string(),
                        arguments: f
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_usage(value: &Value) -> Usage {
    // Chat Completions reports the cached portion as a detail *inside*
    // `prompt_tokens`, so it is subtracted rather than added. A self-hosted
    // server that reports no details object simply looks fully uncached.
    let count = |path: &str| value.pointer(path).and_then(Value::as_u64).unwrap_or(0) as usize;
    Usage::from_inclusive_input(
        count("/usage/prompt_tokens"),
        count("/usage/prompt_tokens_details/cached_tokens"),
        count("/usage/prompt_tokens_details/cache_write_tokens"),
        count("/usage/completion_tokens"),
    )
}

#[async_trait::async_trait]
impl ModelProvider for OpenAiCompatProvider {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn capabilities(&self) -> ModelCapabilities {
        let native_tool_calls = self
            .config
            .native_tool_calls
            .unwrap_or(self.kind != "llamacpp");
        ModelCapabilities {
            model_id: self.model.clone(),
            provider_kind: self.kind.to_string(),
            streaming: true,
            // llama.cpp's OpenAI layer supports tools for many templates but
            // not all; config can override in either direction.
            native_tool_calls,
            structured_output: true,
            image_input: false,
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
            instruction_channel: nexus_core::persona::InstructionChannel::SystemRole,
            // Only the dedicated OpenAI adapter has a contract strong enough
            // to advertise parallel calls. Generic compatible endpoints vary.
            parallel_tool_calls: native_tool_calls && self.kind == "openai",
            // This adapter currently supports JSON-object mode, not an
            // arbitrary caller-supplied response schema.
            json_schema: false,
            local: self.local,
            accelerator: self.accelerator.clone(),
            locality: if self.local {
                ModelLocality::Local
            } else {
                ModelLocality::Remote
            },
            privacy: if self.local {
                ModelPrivacy::LocalOnly
            } else if self.kind == "openai" {
                ModelPrivacy::ProviderManaged
            } else {
                ModelPrivacy::EndpointControlled
            },
            latency_class: ModelLatencyClass::Unknown,
            cost_class: if self.local {
                ModelCostClass::Free
            } else {
                ModelCostClass::Unknown
            },
            fallback_eligibility: if self.local {
                FallbackEligibility::Eligible
            } else {
                FallbackEligibility::ApprovalRequired
            },
        }
    }

    async fn complete(&self, request: CompletionRequest) -> Result<Completion> {
        let body = self.build_body(&request, false);
        // A non-streaming response only lands once generation is finished, so
        // the whole answer has to fit inside the first-token allowance.
        let first_token = self.config.first_token_timeout_secs();
        let response =
            tokio::time::timeout(Duration::from_secs(first_token), self.request(&body).send())
                .await
                .map_err(|_| NexusError::ModelFirstTokenTimeout(first_token))?
                .map_err(|e| map_reqwest_error(self.kind, &e, self.config.timeout_secs))?;
        let status = response.status();
        let headers = response.headers().clone();
        let text = response
            .text()
            .await
            .map_err(|e| self.provider_err(format!("reading response: {e}")))?;
        if !status.is_success() {
            // A quota is not a fault. Reported as its own kind so the loop can
            // wait rather than treat it as a broken request.
            if crate::rate_limit::is_rate_limit(status.as_u16()) {
                return Err(crate::rate_limit::error(
                    self.kind,
                    &headers,
                    status.as_u16(),
                    &text,
                ));
            }
            return Err(self.provider_err(format!("HTTP {status}: {}", truncate_err(&text))));
        }
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| self.provider_err(format!("invalid JSON response: {e}")))?;
        let choice = value
            .pointer("/choices/0")
            .ok_or_else(|| self.provider_err("response has no choices"))?;
        let content = choice
            .pointer("/message/content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let tool_calls = choice
            .pointer("/message/tool_calls")
            .map(parse_tool_calls)
            .unwrap_or_default();
        Ok(Completion {
            content,
            tool_calls,
            usage: parse_usage(&value),
            finish_reason: choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .unwrap_or("stop")
                .to_string(),
            provider_private: None,
        })
    }

    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        let body = self.build_body(&request, true);
        let kind = self.kind;
        let timeout = self.config.timeout_secs;
        // Headers arrive when the server has something to say, so this covers
        // model load and prefill on a self-hosted endpoint.
        let first_token = self.config.first_token_timeout_secs();
        let response =
            tokio::time::timeout(Duration::from_secs(first_token), self.request(&body).send())
                .await
                .map_err(|_| NexusError::ModelFirstTokenTimeout(first_token))?
                .map_err(|e| map_reqwest_error(kind, &e, timeout))?;
        let status = response.status();
        if !status.is_success() {
            let headers = response.headers().clone();
            let text = response.text().await.unwrap_or_default();
            if crate::rate_limit::is_rate_limit(status.as_u16()) {
                return Err(crate::rate_limit::error(
                    kind,
                    &headers,
                    status.as_u16(),
                    &text,
                ));
            }
            return Err(NexusError::Provider {
                provider: kind.to_string(),
                message: format!("HTTP {status}: {}", truncate_err(&text)),
            });
        }
        let byte_stream = response.bytes_stream();
        // Once tokens are flowing, `timeout_secs` measures what it is named
        // for: silence. A stream that keeps producing is never cut off.
        let idle_timeout = Duration::from_secs(timeout.max(1));
        let stream = futures::stream::unfold(
            (
                byte_stream,
                SseParser::new(),
                Vec::<StreamEvent>::new(),
                false,
            ),
            move |(mut bytes, mut parser, mut pending, mut finished)| async move {
                loop {
                    if let Some(event) = pending.pop() {
                        return Some((Ok(event), (bytes, parser, pending, finished)));
                    }
                    if finished {
                        return None;
                    }
                    let next = match tokio::time::timeout(idle_timeout, bytes.next()).await {
                        Ok(next) => next,
                        Err(_) => {
                            return Some((
                                Err(NexusError::ModelTimeout(idle_timeout.as_secs())),
                                (bytes, parser, pending, true),
                            ));
                        }
                    };
                    match next {
                        Some(Ok(chunk)) => {
                            let mut new_events = Vec::new();
                            for item in parser.feed(&chunk) {
                                match item {
                                    SseItem::Done => finished = true,
                                    SseItem::Data(data) => {
                                        match serde_json::from_str::<Value>(&data) {
                                            Ok(v) => new_events.extend(chunk_to_events(&v)),
                                            Err(e) => {
                                                return Some((
                                                    Err(NexusError::Provider {
                                                        provider: kind.to_string(),
                                                        message: format!(
                                                            "invalid stream chunk: {e}"
                                                        ),
                                                    }),
                                                    (bytes, parser, pending, true),
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                            // pop() takes from the back; keep order.
                            new_events.reverse();
                            pending = new_events;
                        }
                        Some(Err(e)) => {
                            return Some((
                                Err(NexusError::Provider {
                                    provider: kind.to_string(),
                                    message: format!("stream interrupted: {e}"),
                                }),
                                (bytes, parser, pending, true),
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
        let start = Instant::now();
        // Try /models first (works on llama.cpp and OpenAI-compatible APIs).
        let url = format!("{}/models", self.base_url);
        let mut req = self.client.get(&url).timeout(Duration::from_secs(5));
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        match req.send().await {
            Ok(r) if r.status().is_success() || r.status().as_u16() == 404 => ProviderHealth {
                reachable: true,
                detail: format!("endpoint responded ({})", r.status()),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            },
            Ok(r) => ProviderHealth {
                reachable: false,
                detail: format!("endpoint returned {}", r.status()),
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

/// Convert one streamed chunk object into events.
fn chunk_to_events(v: &Value) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    if let Some(delta) = v.pointer("/choices/0/delta") {
        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            if !text.is_empty() {
                events.push(StreamEvent::TextDelta(text.to_string()));
            }
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for c in calls {
                events.push(StreamEvent::ToolCallDelta {
                    index: c.get("index").and_then(Value::as_u64).unwrap_or(0) as usize,
                    id: c.get("id").and_then(Value::as_str).map(String::from),
                    name: c
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .map(String::from),
                    arguments_delta: c
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                });
            }
        }
    }
    let finish = v
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str);
    // Usage arrives on the final chunk (stream_options.include_usage) which
    // may have an empty choices array.
    if v.get("usage").map(|u| !u.is_null()).unwrap_or(false) {
        events.push(StreamEvent::Done {
            usage: parse_usage(v),
            finish_reason: finish.unwrap_or("stop").to_string(),
        });
    } else if let Some(reason) = finish {
        events.push(StreamEvent::Done {
            usage: Usage::default(),
            finish_reason: reason.to_string(),
        });
    }
    events
}

fn map_reqwest_error(kind: &str, e: &reqwest::Error, timeout_secs: u64) -> NexusError {
    if e.is_timeout() {
        NexusError::ModelTimeout(timeout_secs)
    } else {
        NexusError::Provider {
            provider: kind.to_string(),
            message: format!("request failed: {e}"),
        }
    }
}

fn truncate_err(text: &str) -> String {
    let t = text.trim();
    if t.len() > 400 {
        format!(
            "{}…",
            &t[..t
                .char_indices()
                .take(400)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0)]
        )
    } else {
        t.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openrouter_uses_normalized_reasoning_and_excludes_private_output() {
        let config = ModelConfig {
            provider: "openai_compatible".into(),
            base_url: "https://openrouter.ai/api/v1".into(),
            model: "exact/model".into(),
            reasoning_effort: Some("high".into()),
            ..Default::default()
        };
        let provider = OpenAiCompatProvider::new("openai_compatible", &config).expect("provider");
        let body = provider.build_body(&CompletionRequest::default(), true);
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["reasoning"]["exclude"], true);
        assert!(body.get("include_reasoning").is_none());
    }

    #[test]
    fn cached_tokens_are_subtracted_from_the_inclusive_prompt_count() {
        let usage = parse_usage(&json!({
            "usage": {
                "prompt_tokens": 1012,
                "completion_tokens": 5,
                "prompt_tokens_details": {"cached_tokens": 900},
            }
        }));
        // 1012 already contains the 900: reporting both would double-count the
        // prefix and inflate every turn's input by the size of the cache.
        assert_eq!(usage.prompt_tokens, 112);
        assert_eq!(usage.cache_read_tokens, 900);
        assert_eq!(usage.total_input(), 1012);
    }

    #[test]
    fn a_response_without_cache_details_is_unchanged() {
        let usage = parse_usage(&json!({
            "usage": {"prompt_tokens": 40, "completion_tokens": 2}
        }));
        assert_eq!(usage.prompt_tokens, 40);
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.total_input(), 40);
    }

    #[test]
    fn both_api_families_normalize_to_the_same_total_input() {
        // The same logical prompt — 1012 tokens, 900 of them cached — as each
        // family reports it. This pair is the regression guard for the one
        // difference between them: Anthropic excludes the cached portion from
        // its input count, the OpenAI family includes it.
        let openai = parse_usage(&json!({
            "usage": {
                "prompt_tokens": 1012,
                "completion_tokens": 5,
                "prompt_tokens_details": {"cached_tokens": 900},
            }
        }));
        let anthropic = crate::anthropic::parse_usage(Some(&json!({
            "input_tokens": 112,
            "output_tokens": 5,
            "cache_read_input_tokens": 900,
        })));
        assert_eq!(openai.total_input(), anthropic.total_input());
        assert_eq!(openai.cache_read_tokens, anthropic.cache_read_tokens);
        assert_eq!(openai.prompt_tokens, anthropic.prompt_tokens);
    }

    #[test]
    fn the_cache_key_is_stable_within_a_conversation_and_distinct_across_them() {
        let config = ModelConfig {
            provider: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: "exact-model".into(),
            resolved_api_key: Some(nexus_core::SecretString::new("test-only")),
            ..Default::default()
        };
        let provider = OpenAiCompatProvider::new("openai", &config).expect("provider");
        let mut request = CompletionRequest {
            messages: vec![
                ChatMessage::system("stable rules"),
                ChatMessage::user("first ask"),
            ],
            ..Default::default()
        };
        let first = provider.build_body(&request, false)["prompt_cache_key"].clone();
        // A turn grows by tool results and assistant replies; the key is taken
        // from the prefix, so it must not move as that happens.
        request.messages.push(ChatMessage::user("a later step"));
        assert_eq!(
            provider.build_body(&request, false)["prompt_cache_key"],
            first
        );

        let other = CompletionRequest {
            messages: vec![
                ChatMessage::system("stable rules"),
                ChatMessage::user("an unrelated ask"),
            ],
            ..Default::default()
        };
        assert_ne!(
            provider.build_body(&other, false)["prompt_cache_key"],
            first
        );
    }

    #[test]
    fn the_cache_key_is_withheld_from_endpoints_that_do_not_document_it() {
        // llama.cpp, vLLM and custom endpoints reject unknown body keys.
        let config = ModelConfig {
            provider: "openai_compatible".into(),
            base_url: "https://example.invalid/v1".into(),
            model: "exact/model".into(),
            ..Default::default()
        };
        let provider = OpenAiCompatProvider::new("openai_compatible", &config).expect("provider");
        let body = provider.build_body(&CompletionRequest::default(), false);
        assert!(body.get("prompt_cache_key").is_none());
    }

    #[test]
    fn native_openai_uses_chat_reasoning_effort_contract() {
        let config = ModelConfig {
            provider: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: "exact-model".into(),
            api_key_env: Some("NEXUS_TEST_MISSING_OPENAI_KEY".into()),
            resolved_api_key: Some(nexus_core::SecretString::new("test-only")),
            reasoning_effort: Some("low".into()),
            ..Default::default()
        };
        let provider = OpenAiCompatProvider::new("openai", &config).expect("provider");
        let body = provider.build_body(&CompletionRequest::default(), false);
        assert_eq!(body["reasoning_effort"], "low");
        assert!(body.get("reasoning").is_none());
    }

    /// A self-hosted server that takes a while to produce the first token is
    /// working, not stalled. Before the first-token allowance existed, the
    /// client-wide deadline cut this exchange off at `timeout_secs`.
    #[tokio::test]
    async fn a_slow_first_response_is_not_mistaken_for_a_stalled_one() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(1200))
                    .set_body_json(serde_json::json!({
                        "choices": [{
                            "message": {"content": "warm at last"},
                            "finish_reason": "stop",
                        }]
                    })),
            )
            .mount(&server)
            .await;

        let config = ModelConfig {
            provider: "llamacpp".into(),
            base_url: server.uri(),
            model: "local".into(),
            timeout_secs: 1,
            first_token_timeout_secs: Some(20),
            ..Default::default()
        };
        let provider = OpenAiCompatProvider::new("llamacpp", &config).expect("provider");
        let completion = provider
            .complete(CompletionRequest::default())
            .await
            .expect("a slow first response must still be delivered");
        assert_eq!(completion.content, "warm at last");
    }

    #[tokio::test]
    async fn the_first_token_allowance_is_reported_as_such_when_it_runs_out() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;

        let config = ModelConfig {
            provider: "llamacpp".into(),
            base_url: server.uri(),
            model: "local".into(),
            timeout_secs: 120,
            first_token_timeout_secs: Some(1),
            ..Default::default()
        };
        let provider = OpenAiCompatProvider::new("llamacpp", &config).expect("provider");
        let error = provider
            .complete(CompletionRequest::default())
            .await
            .expect_err("the allowance must expire");
        assert!(
            matches!(error, NexusError::ModelFirstTokenTimeout(1)),
            "expected a first-token timeout, got {error}",
        );
    }
}
