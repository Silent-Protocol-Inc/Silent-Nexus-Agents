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
            .timeout(Duration::from_secs(config.timeout_secs))
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
        Role::System => "system",
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
    Usage {
        prompt_tokens: value
            .pointer("/usage/prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        completion_tokens: value
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
    }
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
            reasoning_controls: false,
            system_prompt: true,
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
        let response = self
            .request(&body)
            .send()
            .await
            .map_err(|e| map_reqwest_error(self.kind, &e, self.config.timeout_secs))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| self.provider_err(format!("reading response: {e}")))?;
        if !status.is_success() {
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
        })
    }

    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        let body = self.build_body(&request, true);
        let kind = self.kind;
        let timeout = self.config.timeout_secs;
        let response = self
            .request(&body)
            .send()
            .await
            .map_err(|e| map_reqwest_error(kind, &e, timeout))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(NexusError::Provider {
                provider: kind.to_string(),
                message: format!("HTTP {status}: {}", truncate_err(&text)),
            });
        }
        let byte_stream = response.bytes_stream();
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
                    match bytes.next().await {
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
