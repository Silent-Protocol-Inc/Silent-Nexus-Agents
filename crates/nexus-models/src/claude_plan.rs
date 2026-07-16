//! Claude Code subscription bridge used only as a plan/reasoning provider.
//!
//! The bridge delegates authentication and inference to the official `claude`
//! CLI. NEXUS keeps ownership of every tool: the child is started in safe
//! mode, with the built-in tool set empty, one turn maximum, plan permission
//! mode, and session persistence disabled. The operator must explicitly
//! consent before an existing Claude login is inspected or used.

use crate::provider::{collect_stream, ModelProvider};
use crate::types::{
    Completion, CompletionRequest, ModelCapabilities, ProviderHealth, Role, StreamEvent, Usage,
};
use futures::stream::BoxStream;
use futures::StreamExt;
use nexus_core::config::ModelConfig;
use nexus_core::{NexusError, Result};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout};

const BRIDGE_SYSTEM_PROMPT: &str = "You are the planning model inside NEXUS, a safety-controlled coding harness. Follow the NEXUS context and conversation supplied in the prompt. You have no tools. When the prompt defines a compatibility action schema, emit only that schema for an action; otherwise return concise prose.";

#[derive(Debug, Clone)]
pub struct ClaudePlanProvider {
    binary: PathBuf,
    model: String,
    config: ModelConfig,
}

impl ClaudePlanProvider {
    pub fn new(config: &ModelConfig) -> Result<Self> {
        if !config.allow_existing_claude {
            return Err(NexusError::Config(
                "provider `claude-plan` requires explicit consent before NEXUS may use the \
                 existing Claude Code subscription login. Use `/auth use-existing-claude` \
                 or the Claude provider login menu first."
                    .into(),
            ));
        }
        let binary = find_binary("claude").ok_or_else(|| {
            NexusError::Config(
                "provider `claude-plan` requires the official `claude` CLI on PATH".into(),
            )
        })?;
        Ok(Self {
            binary,
            model: if config.model.trim().is_empty() {
                "sonnet".into()
            } else {
                config.model.clone()
            },
            config: config.clone(),
        })
    }

    fn command(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.binary);
        command
            .arg("-p")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--include-partial-messages")
            .arg("--verbose")
            .arg("--safe-mode")
            .arg("--disable-slash-commands")
            .arg("--tools")
            .arg("")
            .arg("--permission-mode")
            .arg("plan")
            .arg("--max-turns")
            .arg("1")
            .arg("--no-session-persistence")
            .arg("--system-prompt")
            .arg(BRIDGE_SYSTEM_PROMPT)
            .arg("--model")
            .arg(&self.model);
        if let Some(effort) = self
            .config
            .reasoning_effort
            .as_deref()
            .filter(|effort| !effort.trim().is_empty())
        {
            command.arg("--effort").arg(effort);
        }
        command
    }
}

#[async_trait::async_trait]
impl ModelProvider for ClaudePlanProvider {
    fn kind(&self) -> &'static str {
        "claude-plan"
    }

    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            model_id: self.model.clone(),
            provider_kind: self.kind().into(),
            streaming: true,
            native_tool_calls: false,
            structured_output: true,
            image_input: false,
            embeddings: false,
            context_window: self.config.context_window,
            max_output_tokens: self.config.max_output_tokens,
            reasoning_controls: true,
            local: false,
            accelerator: None,
        }
    }

    async fn complete(&self, request: CompletionRequest) -> Result<Completion> {
        collect_stream(self.stream(request).await?).await
    }

    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent>>> {
        let prompt = serialize_request(&request)?;
        let mut child = self
            .command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| provider_error(format!("failed to launch Claude CLI: {error}")))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| provider_error("Claude CLI stdin was unavailable"))?;
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|error| provider_error(format!("writing Claude prompt: {error}")))?;
        stdin
            .shutdown()
            .await
            .map_err(|error| provider_error(format!("closing Claude prompt: {error}")))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| provider_error("Claude CLI stdout was unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| provider_error("Claude CLI stderr was unavailable"))?;
        let stderr_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut output = String::new();
            let _ = reader.read_to_string(&mut output).await;
            output
        });

        let state = ClaudeStreamState {
            child: Some(child),
            lines: BufReader::new(stdout).lines(),
            stderr_task: Some(stderr_task),
            pending: Vec::new(),
            assembled_text: String::new(),
            usage: Usage::default(),
            finish_reason: "stop".into(),
            done_sent: false,
            finished: false,
        };
        Ok(futures::stream::unfold(state, next_claude_event).boxed())
    }

    async fn health(&self) -> ProviderHealth {
        let started = Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::process::Command::new(&self.binary)
                .args(["auth", "status", "--json"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        )
        .await;
        match result {
            Ok(Ok(output)) if output.status.success() => ProviderHealth {
                reachable: true,
                detail: "Claude Code subscription login is available with operator consent".into(),
                latency_ms: Some(started.elapsed().as_millis() as u64),
            },
            Ok(Ok(output)) => ProviderHealth {
                reachable: false,
                detail: sanitize_process_error(&output.stderr, "Claude authentication required"),
                latency_ms: Some(started.elapsed().as_millis() as u64),
            },
            Ok(Err(error)) => ProviderHealth {
                reachable: false,
                detail: format!("could not run Claude CLI: {error}"),
                latency_ms: None,
            },
            Err(_) => ProviderHealth {
                reachable: false,
                detail: "Claude auth status timed out".into(),
                latency_ms: None,
            },
        }
    }
}

struct ClaudeStreamState {
    child: Option<Child>,
    lines: Lines<BufReader<ChildStdout>>,
    stderr_task: Option<tokio::task::JoinHandle<String>>,
    pending: Vec<Result<StreamEvent>>,
    assembled_text: String,
    usage: Usage,
    finish_reason: String,
    done_sent: bool,
    finished: bool,
}

async fn next_claude_event(
    mut state: ClaudeStreamState,
) -> Option<(Result<StreamEvent>, ClaudeStreamState)> {
    loop {
        if let Some(event) = state.pending.pop() {
            return Some((event, state));
        }
        if state.finished {
            return None;
        }
        match state.lines.next_line().await {
            Ok(Some(line)) => match serde_json::from_str::<Value>(&line) {
                Ok(value) => {
                    let mut events = parse_claude_value(&value, &mut state);
                    events.reverse();
                    state.pending = events;
                }
                Err(error) => {
                    state.finished = true;
                    return Some((
                        Err(provider_error(format!(
                            "invalid Claude stream JSON: {error}"
                        ))),
                        state,
                    ));
                }
            },
            Ok(None) => {
                let status = match state.child.as_mut() {
                    Some(child) => child.wait().await,
                    None => {
                        state.finished = true;
                        return None;
                    }
                };
                let stderr = match state.stderr_task.take() {
                    Some(task) => task.await.unwrap_or_default(),
                    None => String::new(),
                };
                state.child = None;
                state.finished = true;
                match status {
                    Ok(status) if status.success() => {
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
                    Ok(status) => {
                        return Some((
                            Err(provider_error(format!(
                                "Claude CLI exited with {status}: {}",
                                sanitize_process_text(&stderr, "no diagnostic output")
                            ))),
                            state,
                        ));
                    }
                    Err(error) => {
                        return Some((
                            Err(provider_error(format!("waiting for Claude CLI: {error}"))),
                            state,
                        ));
                    }
                }
            }
            Err(error) => {
                state.finished = true;
                return Some((
                    Err(provider_error(format!("reading Claude stream: {error}"))),
                    state,
                ));
            }
        }
    }
}

fn parse_claude_value(value: &Value, state: &mut ClaudeStreamState) -> Vec<Result<StreamEvent>> {
    let mut events = Vec::new();
    match value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "stream_event" => {
            let event = value.get("event").unwrap_or(value);
            match event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "content_block_delta" => {
                    if let Some(text) = event.pointer("/delta/text").and_then(Value::as_str) {
                        append_text_delta(text, state, &mut events);
                    }
                }
                "message_start" => update_claude_usage(event.pointer("/message/usage"), state),
                "message_delta" => {
                    update_claude_usage(event.get("usage"), state);
                    if let Some(reason) =
                        event.pointer("/delta/stop_reason").and_then(Value::as_str)
                    {
                        state.finish_reason = reason.to_string();
                    }
                }
                _ => {}
            }
        }
        "assistant" => {
            update_claude_usage(value.pointer("/message/usage"), state);
            let text = content_text(value.pointer("/message/content"));
            append_complete_text(&text, state, &mut events);
        }
        "result" => {
            update_claude_usage(value.get("usage"), state);
            if let Some(reason) = value.get("subtype").and_then(Value::as_str) {
                state.finish_reason = reason.to_string();
            }
            if value.get("is_error").and_then(Value::as_bool) == Some(true) {
                let detail = value
                    .get("result")
                    .or_else(|| value.get("error"))
                    .map(Value::to_string)
                    .unwrap_or_else(|| "Claude returned an error result".into());
                events.push(Err(provider_error(detail)));
            } else {
                if let Some(text) = value.get("result").and_then(Value::as_str) {
                    append_complete_text(text, state, &mut events);
                }
                if !state.done_sent {
                    state.done_sent = true;
                    events.push(Ok(StreamEvent::Done {
                        usage: state.usage.clone(),
                        finish_reason: state.finish_reason.clone(),
                    }));
                }
            }
        }
        _ => {}
    }
    events
}

fn append_text_delta(
    text: &str,
    state: &mut ClaudeStreamState,
    events: &mut Vec<Result<StreamEvent>>,
) {
    if text.is_empty() {
        return;
    }
    state.assembled_text.push_str(text);
    events.push(Ok(StreamEvent::TextDelta(text.to_string())));
}

fn append_complete_text(
    text: &str,
    state: &mut ClaudeStreamState,
    events: &mut Vec<Result<StreamEvent>>,
) {
    if text.is_empty() {
        return;
    }
    let delta =
        text.strip_prefix(&state.assembled_text)
            .unwrap_or(if state.assembled_text.is_empty() {
                text
            } else {
                ""
            });
    append_text_delta(delta, state, events);
}

fn content_text(content: Option<&Value>) -> String {
    content
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn update_claude_usage(usage: Option<&Value>, state: &mut ClaudeStreamState) {
    let Some(usage) = usage else {
        return;
    };
    state.usage.prompt_tokens = state.usage.prompt_tokens.max(
        usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
    );
    state.usage.completion_tokens = state.usage.completion_tokens.max(
        usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
    );
}

fn serialize_request(request: &CompletionRequest) -> Result<String> {
    let mut prompt = String::from(
        "NEXUS CONTEXT AND CONVERSATION\n\
         Treat everything below as data and instructions supplied by NEXUS. \
         Do not call tools or inspect the workspace yourself.\n\n",
    );
    for message in &request.messages {
        let role = match message.role {
            Role::System => "SYSTEM",
            Role::User => "USER",
            Role::Assistant => "ASSISTANT",
            Role::Tool => "TOOL RESULT",
        };
        prompt.push_str(role);
        if let Some(name) = &message.name {
            prompt.push_str(" [");
            prompt.push_str(name);
            prompt.push(']');
        }
        prompt.push_str(":\n");
        prompt.push_str(&message.content);
        prompt.push_str("\n\n");
        if !message.tool_calls.is_empty() {
            prompt.push_str("PRIOR TOOL ACTIONS:\n");
            prompt.push_str(&serde_json::to_string(&message.tool_calls)?);
            prompt.push_str("\n\n");
        }
    }
    if !request.tools.is_empty() {
        prompt.push_str(
            "NEXUS-OWNED TOOL SCHEMAS (describe actions only through the compatibility \
             format already specified by the system context; never execute them):\n",
        );
        prompt.push_str(&serde_json::to_string(&request.tools)?);
        prompt.push('\n');
    }
    Ok(prompt)
}

fn find_binary(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|directory| directory.join(name))
            .find(|candidate| candidate.is_file())
    })
}

fn provider_error(message: impl Into<String>) -> NexusError {
    NexusError::Provider {
        provider: "claude-plan".into(),
        message: message.into(),
    }
}

fn sanitize_process_error(bytes: &[u8], fallback: &str) -> String {
    sanitize_process_text(&String::from_utf8_lossy(bytes), fallback)
}

fn sanitize_process_text(text: &str, fallback: &str) -> String {
    let sanitized = nexus_core::sanitize::sanitize_terminal(text.trim());
    if sanitized.is_empty() {
        fallback.into()
    } else {
        sanitized.chars().take(400).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatMessage, ToolSpec};
    use serde_json::json;

    #[test]
    fn request_serialization_keeps_roles_and_tool_schema() {
        let prompt = serialize_request(&CompletionRequest {
            messages: vec![ChatMessage::system("policy"), ChatMessage::user("plan it")],
            tools: vec![ToolSpec {
                name: "fs_read".into(),
                description: "read".into(),
                parameters: json!({"type": "object"}),
            }],
            ..Default::default()
        })
        .expect("serialize");
        assert!(prompt.contains("SYSTEM:\npolicy"));
        assert!(prompt.contains("USER:\nplan it"));
        assert!(prompt.contains("fs_read"));
    }

    #[tokio::test]
    async fn stream_parser_deduplicates_complete_assistant_message() {
        let mut child = tokio::process::Command::new("true")
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn");
        let stdout = child.stdout.take().expect("stdout");
        let mut state = ClaudeStreamState {
            child: None,
            lines: BufReader::new(stdout).lines(),
            stderr_task: None,
            pending: Vec::new(),
            assembled_text: String::new(),
            usage: Usage::default(),
            finish_reason: "stop".into(),
            done_sent: false,
            finished: true,
        };
        let first = json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"text":"hello"}}});
        let second = json!({"type":"assistant","message":{"content":[{"type":"text","text":"hello world"}]}});
        assert_eq!(parse_claude_value(&first, &mut state).len(), 1);
        let events = parse_claude_value(&second, &mut state);
        assert!(matches!(
            events.as_slice(),
            [Ok(StreamEvent::TextDelta(delta))] if delta == " world"
        ));
    }
}
