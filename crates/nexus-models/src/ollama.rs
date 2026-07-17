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
            .timeout(Duration::from_secs(config.timeout_secs))
            .connect_timeout(Duration::from_secs(10))
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
            "options": {
                "num_ctx": self.config.context_window,
                "num_predict": request.max_tokens.unwrap_or(self.config.max_output_tokens),
            }
        });
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

    fn parse_usage(value: &Value) -> Usage {
        Usage {
            prompt_tokens: value
                .get("prompt_eval_count")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
            completion_tokens: value.get("eval_count").and_then(Value::as_u64).unwrap_or(0)
                as usize,
        }
    }

    fn parse_stream_line(line: &str) -> Result<Vec<StreamEvent>> {
        let value = serde_json::from_str::<Value>(line).map_err(|error| NexusError::Provider {
            provider: "ollama".into(),
            message: format!("invalid stream line: {error}"),
        })?;
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
                finish_reason: "stop".into(),
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
            reasoning_controls: false,
            system_prompt: true,
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
        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&body)
            .send()
            .await
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
        let (content, tool_calls) = Self::parse_message(&value);
        Ok(Completion {
            content,
            tool_calls,
            usage: Self::parse_usage(&value),
            finish_reason: if value.get("done").and_then(Value::as_bool).unwrap_or(true) {
                "stop".into()
            } else {
                "length".into()
            },
        })
    }

    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        let body = self.build_body(&request, true);
        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&body)
            .send()
            .await
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
        // NDJSON: one JSON object per line.
        let stream = futures::stream::unfold(
            (byte_stream, String::new(), Vec::<StreamEvent>::new()),
            move |(mut bytes, mut buf, mut pending)| async move {
                loop {
                    if let Some(ev) = pending.pop() {
                        return Some((Ok(ev), (bytes, buf, pending)));
                    }
                    match bytes.next().await {
                        Some(Ok(chunk)) => {
                            buf.push_str(&String::from_utf8_lossy(&chunk));
                            let mut events = Vec::new();
                            while let Some(pos) = buf.find('\n') {
                                let line: String = buf.drain(..=pos).collect();
                                let line = line.trim();
                                if line.is_empty() {
                                    continue;
                                }
                                match OllamaProvider::parse_stream_line(line) {
                                    Ok(mut parsed) => events.append(&mut parsed),
                                    Err(error) => {
                                        return Some((Err(error), (bytes, buf, pending)));
                                    }
                                }
                            }
                            events.reverse();
                            pending = events;
                        }
                        Some(Err(e)) => {
                            return Some((
                                Err(NexusError::Provider {
                                    provider: "ollama".into(),
                                    message: format!("stream interrupted: {e}"),
                                }),
                                (bytes, buf, pending),
                            ));
                        }
                        None => {
                            // `bytes_stream` is not required to end with a
                            // newline. Ollama and reverse proxies may close
                            // immediately after the terminal JSON object, so
                            // parse the remaining buffer exactly once at EOF.
                            let line = std::mem::take(&mut buf);
                            let line = line.trim();
                            if line.is_empty() {
                                return None;
                            }
                            match OllamaProvider::parse_stream_line(line) {
                                Ok(mut events) => {
                                    events.reverse();
                                    pending = events;
                                }
                                Err(error) => {
                                    return Some((Err(error), (bytes, String::new(), pending)));
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
