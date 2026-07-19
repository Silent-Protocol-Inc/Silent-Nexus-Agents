//! Structured action parsing and the tool-call compatibility layer.
//!
//! Native tool-callers hand back structured `tool_calls`. Models without
//! native tool calling instead emit a JSON action block; this module extracts
//! and validates it, so a 1–3B local model can still drive tools safely. The
//! parser is strict: it never executes commands merely because they appear in
//! prose — only a well-formed, schema-valid action object is actioned.

use nexus_models::types::{Completion, ToolCallRequest};
use serde::{Deserialize, Serialize};

/// A parsed model action.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentAction {
    /// Call a tool with JSON arguments.
    ToolCall(ToolCallRequest),
    /// Finish the turn with a final answer.
    Finish { message: String },
    /// The model produced only prose (no action); treated as a final answer
    /// unless the loop expected a tool call.
    Message(String),
}

/// The JSON schema models are told to emit when native tool calling is
/// unavailable. Kept tiny for small models.
pub const COMPAT_INSTRUCTIONS: &str = r#"You do not have native tool calling. To act, output a single JSON object on its own line, wrapped in a fenced ```json block, with this shape:
{"action": "tool", "tool": "<tool_name>", "arguments": { ... }}
To finish, output:
{"action": "finish", "message": "<your final answer>"}
Output exactly one such JSON object and nothing else after it."#;

#[derive(Debug, Deserialize, Serialize)]
struct CompatAction {
    action: String,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    arguments: Option<serde_json::Value>,
    #[serde(default)]
    message: Option<String>,
}

/// Extract actions from a completion. Native tool calls take precedence; when
/// absent, attempt to parse a compat JSON block from the text.
pub fn parse(completion: &Completion, native_expected: bool) -> Result<AgentAction, String> {
    if let Some(call) = completion.tool_calls.first() {
        return Ok(AgentAction::ToolCall(call.clone()));
    }
    if native_expected {
        // Native model chose to answer in prose → final answer.
        return Ok(AgentAction::Message(completion.content.clone()));
    }
    parse_compat(&completion.content)
}

/// Parse a compatibility-layer action from model text.
pub fn parse_compat(text: &str) -> Result<AgentAction, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("model returned an empty response".to_string());
    }
    let Some(candidate) = extract_json_block(trimmed) else {
        if looks_like_action_attempt(trimmed) {
            return Err("malformed JSON action payload".to_string());
        }
        return Ok(AgentAction::Message(trimmed.to_string()));
    };
    // JSON used as ordinary prose/data is not an attempted tool call. Only
    // objects that explicitly claim to be compatibility actions enter the
    // action parser.
    if !candidate.contains("\"action\"") && !candidate.contains("'action'") {
        return Ok(AgentAction::Message(trimmed.to_string()));
    }
    let parsed: CompatAction =
        serde_json::from_str(&candidate).map_err(|e| format!("action JSON is invalid: {e}"))?;
    match parsed.action.as_str() {
        "tool" | "tool_call" | "call" => {
            let tool = parsed
                .tool
                .filter(|tool| !tool.trim().is_empty())
                .ok_or_else(|| "action `tool` requires a `tool` field".to_string())?;
            let arguments = parsed.arguments.unwrap_or(serde_json::json!({}));
            if !arguments.is_object() {
                return Err("action `tool` requires `arguments` to be a JSON object".into());
            }
            Ok(AgentAction::ToolCall(ToolCallRequest {
                id: format!("call_{}", short_id()),
                name: tool,
                arguments: arguments.to_string(),
            }))
        }
        "finish" | "final" | "done" => {
            let message = parsed.message.unwrap_or_default();
            if message.trim().is_empty() {
                return Err("action `finish` requires a non-empty `message`".into());
            }
            Ok(AgentAction::Finish { message })
        }
        other => Err(format!("unknown action `{other}` (expected tool|finish)")),
    }
}

fn looks_like_action_attempt(text: &str) -> bool {
    text.contains("\"action\"")
        || text.contains("'action'")
        || text.trim_start().starts_with("```json")
        || text.trim_start().starts_with("{\"action")
}

/// Extract the first balanced JSON object from text, preferring a fenced
/// ```json block. Returns the raw JSON string.
fn extract_json_block(text: &str) -> Option<String> {
    // Prefer fenced blocks.
    if let Some(start) = text.find("```json") {
        let after = &text[start + 7..];
        if let Some(end) = after.find("```") {
            let block = after[..end].trim();
            if let Some(obj) = balanced_object(block) {
                return Some(obj);
            }
        }
    }
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        if let Some(end) = after.find("```") {
            let block = after[..end].trim();
            if let Some(obj) = balanced_object(block) {
                return Some(obj);
            }
        }
    }
    // Fall back to the first balanced object containing "action".
    balanced_object(text)
}

/// Find the first balanced `{...}` region (string-aware) in `text`.
fn balanced_object(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let start = text.find('{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for i in start..bytes.len() {
        let c = bytes[i] as char;
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn short_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_models::types::Usage;

    fn completion(content: &str, calls: Vec<ToolCallRequest>) -> Completion {
        Completion {
            content: content.to_string(),
            tool_calls: calls,
            usage: Usage::default(),
            finish_reason: "stop".into(),
            provider_private: None,
        }
    }

    #[test]
    fn native_tool_call_takes_precedence() {
        let c = completion(
            "some prose",
            vec![ToolCallRequest {
                id: "call_1".into(),
                name: "fs.read_file".into(),
                arguments: "{\"path\":\"a\"}".into(),
            }],
        );
        assert!(matches!(parse(&c, true), Ok(AgentAction::ToolCall(_))));
    }

    #[test]
    fn compat_json_block_parsed() {
        let text = r#"I'll read the file now.
```json
{"action": "tool", "tool": "fs.read_file", "arguments": {"path": "src/main.rs"}}
```"#;
        let action = parse_compat(text).expect("parse");
        match action {
            AgentAction::ToolCall(c) => {
                assert_eq!(c.name, "fs.read_file");
                assert!(c.arguments.contains("src/main.rs"));
            }
            _ => panic!("expected tool call"),
        }
    }

    #[test]
    fn compat_finish_parsed() {
        let text = r#"{"action": "finish", "message": "All done."}"#;
        assert_eq!(
            parse_compat(text).expect("parse"),
            AgentAction::Finish {
                message: "All done.".into()
            }
        );
    }

    #[test]
    fn prose_without_action_finishes_in_compat_mode() {
        assert_eq!(
            parse_compat("Let me think about this problem carefully."),
            Ok(AgentAction::Message(
                "Let me think about this problem carefully.".into()
            ))
        );
    }

    #[test]
    fn does_not_execute_commands_in_prose() {
        // A command mentioned in prose is not an action.
        let text = "You should run `rm -rf /` to clean up. But I won't.";
        assert_eq!(
            parse_compat(text),
            Ok(AgentAction::Message(text.to_string()))
        );
    }

    #[test]
    fn malformed_action_is_rejected_not_executed() {
        let text = r#"```json
{"action":"tool","tool":"terminal.run","arguments":
```"#;
        assert!(parse_compat(text).is_err());
    }

    #[test]
    fn ordinary_json_in_prose_is_a_terminal_message() {
        let text = r#"The result is {"status":"ok"}."#;
        assert_eq!(
            parse_compat(text),
            Ok(AgentAction::Message(text.to_string()))
        );
    }

    #[test]
    fn braces_in_strings_do_not_break_parsing() {
        let text = r#"{"action": "tool", "tool": "fs.create_file", "arguments": {"path": "a.rs", "content": "fn main() { let x = {1}; }"}}"#;
        let action = parse_compat(text).expect("parse");
        assert!(matches!(action, AgentAction::ToolCall(_)));
    }

    #[test]
    fn native_prose_becomes_final_message() {
        let c = completion("Here is my analysis.", vec![]);
        assert_eq!(
            parse(&c, true),
            Ok(AgentAction::Message("Here is my analysis.".into()))
        );
    }
}
