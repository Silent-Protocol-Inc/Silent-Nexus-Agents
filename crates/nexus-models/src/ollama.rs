//! Ollama provider using the native `/api/chat` NDJSON protocol.

use crate::provider::ModelProvider;
use crate::types::*;
use futures::stream::BoxStream;
use futures::StreamExt;
use nexus_core::config::ModelConfig;
use nexus_core::{NexusError, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    config: ModelConfig,
}

impl OllamaProvider {
    pub fn new(config: &ModelConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            // Do not impose a total response deadline: generation may remain
            // healthy for much longer than the header timeout. Streaming uses
            // an idle-between-chunks timeout below.
            .connect_timeout(Duration::from_secs(config.timeout_secs.clamp(1, 10)))
            .danger_accept_invalid_certs(!config.tls_verify)
            .build()
            .map_err(|e| NexusError::Provider {
                provider: "ollama".into(),
                message: format!("http client: {e}"),
            })?;
        Ok(Self {
            client,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            model: config.model.clone(),
            config: config.clone(),
        })
    }

    /// Query `/api/ps` for the configured model and report how it is loaded:
    /// fully on GPU, split, or CPU-only. Returns `None` if the model is not
    /// currently loaded or the endpoint is unavailable. This is the honest,
    /// per-model answer to "is it actually running on the GPU?".
    async fn gpu_offload(&self) -> Option<String> {
        let resp = self
            .client
            .get(format!("{}/api/ps", self.base_url))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let body: Value = resp.json().await.ok()?;
        let models = body.get("models").and_then(Value::as_array)?;
        let m = models.iter().find(|m| {
            m.get("name")
                .and_then(Value::as_str)
                .map(|n| n == self.model || n.starts_with(&format!("{}:", self.model)))
                .unwrap_or(false)
        })?;
        let total = m.get("size").and_then(Value::as_u64).unwrap_or(0);
        let vram = m.get("size_vram").and_then(Value::as_u64).unwrap_or(0);
        let mib = |b: u64| b / (1024 * 1024);
        Some(if vram == 0 {
            "loaded on CPU (0 MiB VRAM)".to_string()
        } else if total > 0 && vram >= total {
            format!("loaded on GPU ({} MiB VRAM)", mib(vram))
        } else {
            format!(
                "split GPU/CPU ({} of {} MiB in VRAM)",
                mib(vram),
                mib(total)
            )
        })
    }

    fn build_body(&self, request: &CompletionRequest, stream: bool) -> Value {
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    // Ollama has one instruction channel; the developer
                    // channel folds back onto it.
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
                            "function": {
                                "name": c.name,
                                "arguments": serde_json::from_str::<Value>(&c.arguments)
                                    .unwrap_or(json!({})),
                            }
                        }))
                        .collect::<Vec<_>>());
                }
                v
            })
            .collect();
        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "stream": stream,
            // Keep the model resident between turns. Without this Ollama
            // unloads on its own schedule and the next turn pays a full cold
            // load before it can emit a token.
            "keep_alive": self.config.keep_alive(),
            "options": {
                "num_ctx": self.config.context_window,
                "num_predict": request.max_tokens.unwrap_or(self.config.max_output_tokens),
            }
        });
        body["think"] = match self.config.reasoning_effort.as_deref() {
            Some("on") => json!(true),
            Some(effort @ ("low" | "medium" | "high")) => json!(effort),
            _ => json!(false),
        };
        if let Some(t) = request.temperature.or(self.config.temperature) {
            body["options"]["temperature"] = json!(t);
        }
        if !request.stop.is_empty() {
            body["options"]["stop"] = json!(request.stop);
        }
        if request.json_mode {
            body["format"] = json!("json");
        }
        if !request.tools.is_empty() && self.capabilities().native_tool_calls {
            body["tools"] = json!(request
                .tools
                .iter()
                .map(|t| json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                }))
                .collect::<Vec<_>>());
        }
        body
    }

    fn parse_message(value: &Value) -> (String, Vec<ToolCallRequest>) {
        let content = value
            .pointer("/message/content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let tool_calls = value
            .pointer("/message/tool_calls")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .enumerate()
                    .filter_map(|(i, c)| {
                        let f = c.get("function")?;
                        Some(ToolCallRequest {
                            id: format!("call_{i}"),
                            name: f.get("name")?.as_str()?.to_string(),
                            // Ollama sends arguments as a JSON object.
                            arguments: f
                                .get("arguments")
                                .map(|a| a.to_string())
                                .unwrap_or_else(|| "{}".into()),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        (content, tool_calls)
    }

    /// Ollama reports no cache accounting, so the cache fields stay zero.
    ///
    /// That is accurate rather than missing: the server reuses its KV cache
    /// across requests without telling us how much it reused, and there is no
    /// request-side knob to set. Keeping the model resident — which is what
    /// actually preserves that reuse — is already handled by `keep_alive`.
    fn parse_usage(value: &Value) -> Usage {
        Usage {
            prompt_tokens: value
                .get("prompt_eval_count")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
            completion_tokens: value.get("eval_count").and_then(Value::as_u64).unwrap_or(0)
                as usize,
            ..Usage::default()
        }
    }

    fn visible_content(content: &str) -> String {
        // Older Ollama/model combinations may ignore `think: false` and put
        // private deliberation in `message.content`, sometimes without an
        // opening tag. Buffering until the terminal record lets us remove it
        // before any text reaches callers.
        content
            .rfind("</think>")
            .map(|end| content[end + "</think>".len()..].to_string())
            .unwrap_or_else(|| content.to_string())
    }

    fn finalize_buffered_content(
        events: Vec<StreamEvent>,
        buffered: &mut String,
    ) -> Vec<StreamEvent> {
        let mut output = Vec::new();
        for event in events {
            match event {
                StreamEvent::TextDelta(delta) => buffered.push_str(&delta),
                done @ StreamEvent::Done { .. } => {
                    let visible = Self::visible_content(buffered);
                    buffered.clear();
                    if !visible.is_empty() {
                        output.push(StreamEvent::TextDelta(visible));
                    }
                    output.push(done);
                }
                other => output.push(other),
            }
        }
        output
    }

    fn parse_stream_line(line: &str, _retain_thinking: bool) -> Result<Vec<StreamEvent>> {
        let value = serde_json::from_str::<Value>(line).map_err(|error| NexusError::Provider {
            provider: "ollama".into(),
            message: format!("invalid stream line: {error}"),
        })?;
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Err(NexusError::Provider {
                provider: "ollama".into(),
                message: format!("stream error: {error}"),
            });
        }
        let (content, calls) = Self::parse_message(&value);
        let mut events = Vec::new();
        if !content.is_empty() {
            events.push(StreamEvent::TextDelta(content));
        }
        for (index, call) in calls.into_iter().enumerate() {
            events.push(StreamEvent::ToolCallDelta {
                index,
                id: Some(call.id),
                name: Some(call.name),
                arguments_delta: call.arguments,
            });
        }
        if value.get("done").and_then(Value::as_bool).unwrap_or(false) {
            events.push(StreamEvent::Done {
                usage: Self::parse_usage(&value),
                finish_reason: value
                    .get("done_reason")
                    .and_then(Value::as_str)
                    .unwrap_or("stop")
                    .to_string(),
            });
        }
        Ok(events)
    }
}

#[async_trait::async_trait]
impl ModelProvider for OllamaProvider {
    fn kind(&self) -> &'static str {
        "ollama"
    }

    fn capabilities(&self) -> ModelCapabilities {
        let local = crate::openai_compat::is_local_url(&self.config.base_url);
        ModelCapabilities {
            model_id: self.model.clone(),
            provider_kind: "ollama".into(),
            streaming: true,
            native_tool_calls: self.config.native_tool_calls.unwrap_or(true),
            structured_output: true,
            image_input: false,
            embeddings: true,
            context_window: self.config.context_window,
            max_output_tokens: self.config.max_output_tokens,
            context_limit_source: self.config.context_limit_source,
            output_limit_source: self.config.output_limit_source,
            reasoning_controls: true,
            reasoning: ReasoningProfile {
                supported_efforts: self.config.reasoning_effort.clone().into_iter().collect(),
                default_effort: self.config.reasoning_effort.clone(),
                control: crate::ReasoningControl::Optional,
                mandatory: false,
                provider_managed: false,
                provenance: ReasoningProvenance::ConfiguredDefault,
            },
            system_prompt: true,
            // A real system role in `messages`. Ollama drops the model's own
            // Modelfile SYSTEM when the request supplies one, so the selected
            // persona is the only application instruction the model sees.
            instruction_channel: nexus_core::persona::InstructionChannel::SystemRole,
            parallel_tool_calls: false,
            json_schema: false,
            local,
            accelerator: crate::openai_compat::local_accelerator(local),
            locality: if local {
                ModelLocality::Local
            } else {
                ModelLocality::Remote
            },
            privacy: if local {
                ModelPrivacy::LocalOnly
            } else {
                ModelPrivacy::EndpointControlled
            },
            latency_class: ModelLatencyClass::Unknown,
            cost_class: if local {
                ModelCostClass::Free
            } else {
                ModelCostClass::Unknown
            },
            fallback_eligibility: if local {
                FallbackEligibility::Eligible
            } else {
                FallbackEligibility::ApprovalRequired
            },
        }
    }

    async fn complete(&self, request: CompletionRequest) -> Result<Completion> {
        let body = self.build_body(&request, false);
        // A non-streaming Ollama response arrives only once generation is
        // finished, so the whole answer has to fit inside the first-token
        // allowance — the stall timeout has nothing to measure here.
        let first_token = self.config.first_token_timeout_secs();
        let response = tokio::time::timeout(
            Duration::from_secs(first_token),
            self.client
                .post(format!("{}/api/chat", self.base_url))
                .json(&body)
                .send(),
        )
        .await
        .map_err(|_| NexusError::ModelFirstTokenTimeout(first_token))?
        .map_err(|e| {
            if e.is_timeout() {
                NexusError::ModelTimeout(self.config.timeout_secs)
            } else {
                NexusError::Provider {
                    provider: "ollama".into(),
                    message: format!("request failed: {e}"),
                }
            }
        })?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(NexusError::Provider {
                provider: "ollama".into(),
                message: format!("HTTP {status}: {text}"),
            });
        }
        let value: Value = serde_json::from_str(&text).map_err(|e| NexusError::Provider {
            provider: "ollama".into(),
            message: format!("invalid JSON: {e}"),
        })?;
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Err(NexusError::Provider {
                provider: "ollama".into(),
                message: error.to_string(),
            });
        }
        let (content, tool_calls) = Self::parse_message(&value);
        let content = Self::visible_content(&content);
        Ok(Completion {
            content,
            tool_calls,
            usage: Self::parse_usage(&value),
            finish_reason: if value.get("done").and_then(Value::as_bool).unwrap_or(true) {
                value
                    .get("done_reason")
                    .and_then(Value::as_str)
                    .unwrap_or("stop")
                    .to_string()
            } else {
                "length".into()
            },
            provider_private: None,
        })
    }

    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        let body = self.build_body(&request, true);
        // Ollama does not write response headers until it has something to
        // say, so this deadline covers model load and prefill — work that is
        // categorically different from a stream going silent mid-answer.
        let first_token = self.config.first_token_timeout_secs();
        let response = tokio::time::timeout(
            Duration::from_secs(first_token),
            self.client
                .post(format!("{}/api/chat", self.base_url))
                .json(&body)
                .send(),
        )
        .await
        .map_err(|_| NexusError::ModelFirstTokenTimeout(first_token))?
        .map_err(|e| NexusError::Provider {
            provider: "ollama".into(),
            message: format!("request failed: {e}"),
        })?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(NexusError::Provider {
                provider: "ollama".into(),
                message: format!("HTTP {status}: {text}"),
            });
        }
        let byte_stream = response.bytes_stream();
        let idle_timeout = Duration::from_secs(self.config.timeout_secs.max(1));
        let retain_thinking = matches!(
            self.config.reasoning_effort.as_deref(),
            Some("on" | "low" | "medium" | "high")
        );
        // NDJSON: one JSON object per line.
        let stream = futures::stream::unfold(
            (
                byte_stream,
                String::new(),
                Vec::<StreamEvent>::new(),
                String::new(),
            ),
            move |(mut bytes, mut buf, mut pending, mut content)| async move {
                loop {
                    if let Some(ev) = pending.pop() {
                        return Some((Ok(ev), (bytes, buf, pending, content)));
                    }
                    match tokio::time::timeout(idle_timeout, bytes.next()).await {
                        Err(_) => {
                            return Some((
                                Err(NexusError::ModelTimeout(idle_timeout.as_secs())),
                                (bytes, buf, pending, content),
                            ));
                        }
                        Ok(Some(Ok(chunk))) => {
                            buf.push_str(&String::from_utf8_lossy(&chunk));
                            let mut events = Vec::new();
                            while let Some(pos) = buf.find('\n') {
                                let line: String = buf.drain(..=pos).collect();
                                let line = line.trim();
                                if line.is_empty() {
                                    continue;
                                }
                                match OllamaProvider::parse_stream_line(line, retain_thinking) {
                                    Ok(parsed) => {
                                        events.extend(OllamaProvider::finalize_buffered_content(
                                            parsed,
                                            &mut content,
                                        ))
                                    }
                                    Err(error) => {
                                        return Some((Err(error), (bytes, buf, pending, content)));
                                    }
                                }
                            }
                            events.reverse();
                            pending = events;
                        }
                        Ok(Some(Err(e))) => {
                            return Some((
                                Err(NexusError::Provider {
                                    provider: "ollama".into(),
                                    message: format!("stream interrupted: {e}"),
                                }),
                                (bytes, buf, pending, content),
                            ));
                        }
                        Ok(None) => {
                            // `bytes_stream` is not required to end with a
                            // newline. Ollama and reverse proxies may close
                            // immediately after the terminal JSON object, so
                            // parse the remaining buffer exactly once at EOF.
                            let line = std::mem::take(&mut buf);
                            let line = line.trim();
                            if line.is_empty() {
                                return None;
                            }
                            match OllamaProvider::parse_stream_line(line, retain_thinking) {
                                Ok(events) => {
                                    let mut events = OllamaProvider::finalize_buffered_content(
                                        events,
                                        &mut content,
                                    );
                                    events.reverse();
                                    pending = events;
                                }
                                Err(error) => {
                                    return Some((
                                        Err(error),
                                        (bytes, String::new(), pending, content),
                                    ));
                                }
                            }
                        }
                    }
                }
            },
        );
        Ok(stream.boxed())
    }

    async fn health(&self) -> ProviderHealth {
        let start = Instant::now();
        match self
            .client
            .get(format!("{}/api/tags", self.base_url))
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                // Report whether the configured model is pulled.
                let models: Vec<String> = r
                    .json::<Value>()
                    .await
                    .ok()
                    .and_then(|v| {
                        v.get("models").and_then(Value::as_array).map(|arr| {
                            arr.iter()
                                .filter_map(|m| m.get("name").and_then(Value::as_str))
                                .map(String::from)
                                .collect()
                        })
                    })
                    .unwrap_or_default();
                let has_model = models
                    .iter()
                    .any(|m| m == &self.model || m.starts_with(&format!("{}:", self.model)));
                // Ask Ollama whether the model is currently loaded and, if so,
                // how much of it sits in VRAM — a real GPU-offload signal.
                let offload = self.gpu_offload().await;
                let mut detail = if has_model {
                    format!("ollama up; model `{}` available", self.model)
                } else {
                    format!(
                        "ollama up; model `{}` NOT pulled (run `ollama pull {}` yourself — snx does not download models without approval)",
                        self.model, self.model
                    )
                };
                if let Some(o) = offload {
                    detail.push_str(&format!("; {o}"));
                }
                ProviderHealth {
                    reachable: true,
                    detail,
                    latency_ms: Some(start.elapsed().as_millis() as u64),
                }
            }
            Ok(r) => ProviderHealth {
                reachable: false,
                detail: format!("ollama returned {}", r.status()),
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

    fn provider(effort: Option<&str>) -> OllamaProvider {
        let config = ModelConfig {
            provider: "ollama".into(),
            base_url: "http://127.0.0.1:11434".into(),
            model: "qwen3:4b".into(),
            reasoning_effort: effort.map(str::to_string),
            ..Default::default()
        };
        OllamaProvider::new(&config).expect("provider")
    }

    #[test]
    fn requests_keep_the_model_resident_and_ask_only_for_the_configured_window() {
        let config = ModelConfig {
            provider: "ollama".into(),
            base_url: "http://127.0.0.1:11434".into(),
            model: "qwen3.5:9b".into(),
            context_window: 32_768,
            context_ceiling: Some(262_144),
            max_output_tokens: 4_096,
            ..Default::default()
        };
        let body = OllamaProvider::new(&config)
            .expect("provider")
            .build_body(&ModelRequest::default(), true);
        assert_eq!(body["keep_alive"], "30m");
        assert_eq!(
            body["options"]["num_ctx"], 32_768,
            "the architecture ceiling is a capability, not a KV cache to allocate",
        );
        assert_eq!(body["options"]["num_predict"], 4_096);
    }

    #[test]
    fn thinking_is_off_by_default_and_explicit_effort_is_mapped() {
        assert_eq!(
            provider(None).build_body(&ModelRequest::default(), true)["think"],
            false
        );
        assert_eq!(
            provider(Some("on")).build_body(&ModelRequest::default(), true)["think"],
            true
        );
        assert_eq!(
            provider(Some("low")).build_body(&ModelRequest::default(), true)["think"],
            "low"
        );
    }

    #[test]
    fn thinking_is_not_rendered_and_done_reason_is_preserved() {
        let events = OllamaProvider::parse_stream_line(
            r#"{"message":{"thinking":"private","content":"ready"},"done":true,"done_reason":"length","eval_count":8}"#,
            false,
        ).expect("line");
        assert!(matches!(&events[0], StreamEvent::TextDelta(text) if text == "ready"));
        assert!(
            matches!(&events[1], StreamEvent::Done { finish_reason, .. } if finish_reason == "length")
        );
        assert!(!format!("{events:?}").contains("private"));
    }

    #[test]
    fn explicit_thinking_is_dropped_at_the_adapter_boundary() {
        let events = OllamaProvider::parse_stream_line(
            r#"{"message":{"thinking":"private","content":"ready"},"done":true}"#,
            true,
        )
        .expect("line");
        assert!(matches!(&events[0], StreamEvent::TextDelta(text) if text == "ready"));
        assert!(!format!("{events:?}").contains("private"));
    }

    #[test]
    fn legacy_content_thinking_is_removed_before_stream_output() {
        let mut buffered = String::new();
        assert!(OllamaProvider::finalize_buffered_content(
            vec![StreamEvent::TextDelta("private reasoning".into())],
            &mut buffered,
        )
        .is_empty());
        let events = OllamaProvider::finalize_buffered_content(
            vec![
                StreamEvent::TextDelta("</think>\n\nready".into()),
                StreamEvent::Done {
                    usage: Usage::default(),
                    finish_reason: "stop".into(),
                },
            ],
            &mut buffered,
        );
        assert!(matches!(&events[0], StreamEvent::TextDelta(text) if text.trim() == "ready"));
        assert!(matches!(&events[1], StreamEvent::Done { .. }));
    }

    #[test]
    fn streamed_error_object_fails() {
        let error = OllamaProvider::parse_stream_line(r#"{"error":"model runner crashed"}"#, false)
            .expect_err("must fail");
        assert!(error.to_string().contains("model runner crashed"));
    }
}
