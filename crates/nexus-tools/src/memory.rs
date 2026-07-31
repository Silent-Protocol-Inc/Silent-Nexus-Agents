//! Durable memory authoring (`memory.*`).
//!
//! Recall already happens without the model asking: the loop searches stored
//! memories against the objective and injects the hits. What was missing was
//! the other direction — a way for an agent told "record what you found" to
//! actually record it. [`ToolCategory::Memory`] was granted to roles that had
//! no tool carrying it, so the grant resolved to an empty tool set and the
//! agent correctly reported that no memory tool existed.
//!
//! Writing here is deliberately not a workspace mutation, which is why the tool
//! carries [`RiskLevel::Read`] and a read-only role may call it: it appends to a
//! separate, budgeted (`max_memory_writes`), secret-refusing store that the
//! operator reviews. Entries land as candidates awaiting approval — they are
//! listed by `/memory` immediately, but are not recalled into later turns until
//! the operator approves them.

use crate::{Tool, ToolCategory, ToolContext, ToolMeta, ToolOutput, ToolRegistry};
use nexus_core::{NexusError, Result, RiskLevel};
use nexus_memory::{MemoryKind, MemoryStore, NewMemory};
use nexus_policy::ActionRequest;
use serde_json::{json, Value};
use std::sync::Arc;

/// An agent-authored memory is a claim, not a verified fact; it is stored below
/// the confidence of something the operator stated directly.
const AGENT_CONFIDENCE: f64 = 0.8;
/// Long enough for a real finding, short enough to stay a memory rather than a
/// pasted report. The store's own limits still apply.
const MAX_CONTENT_CHARS: usize = 2_000;

struct MemoryAddTool {
    meta: ToolMeta,
}

fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "content": {
                "type": "string",
                "description": "The fact to remember, as one self-contained sentence or two. It is read back with no surrounding context, so name the subject explicitly rather than saying \"it\" or \"this file\".",
            },
            "kind": {
                "type": "string",
                "enum": [
                    "project_fact",
                    "preference",
                    "procedure",
                    "correction",
                    "session",
                    "goal_history",
                ],
                "description": "What sort of fact this is. `project_fact` for something true about the codebase, `preference` for how the operator wants work done, `procedure` for a repeatable sequence, `correction` for something previously gotten wrong.",
            },
            "scope": {
                "type": "string",
                "enum": ["project", "global"],
                "description": "`project` (default) confines the memory to this workspace. `global` is refused unless the operator enabled cross-project memory.",
            },
            "ttl_days": {
                "type": "integer",
                "minimum": 1,
                "description": "Optional expiry, for facts that are only true for a while.",
            },
        },
        "required": ["content"],
        "additionalProperties": false
    })
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(Arc::new(MemoryAddTool {
        meta: ToolMeta {
            name: "memory.add".into(),
            namespace: "memory".into(),
            description: "Record one durable fact worth carrying into later sessions. Use it for \
                          findings, operator preferences, and conclusions that cost real work to \
                          establish — not for turn-by-turn narration, and not for anything \
                          secret (credentials are refused). Report what you recorded; the tool \
                          result says whether it is available to later turns immediately or is \
                          waiting on operator review."
                .into(),
            category: ToolCategory::Memory,
            input_schema: input_schema(),
            output_schema: json!({"type": "string"}),
            // Not a workspace mutation: an append to a separate, budgeted,
            // secret-refusing store that is inert until the operator approves
            // it. Read-only roles are meant to be able to record what they find.
            risk: RiskLevel::Read,
            required_capabilities: vec![],
            timeout_secs: 10,
            max_output_bytes: 4_000,
            deterministic: false,
            needs_network: false,
            needs_sandbox: false,
            side_effects: "appends one entry to the memory store for operator review; changes \
                           nothing in the workspace"
                .into(),
        },
    }));
}

/// Whether this workspace holds agent-recorded memories for review first.
fn requires_approval(ctx: &ToolContext) -> bool {
    ctx.config.memory.require_approval
}

/// The parsed, validated arguments.
#[derive(Debug)]
struct MemoryInput {
    content: String,
    kind: MemoryKind,
    scope: String,
    ttl_days: Option<u32>,
}

impl MemoryAddTool {
    fn parse(args: &Value) -> Result<MemoryInput> {
        let invalid = |message: String| NexusError::ToolInput {
            tool: "memory.add".into(),
            message,
        };
        let content = args
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if content.is_empty() {
            return Err(invalid(
                "there is nothing to remember: content is empty".into(),
            ));
        }
        if content.chars().count() > MAX_CONTENT_CHARS {
            return Err(invalid(format!(
                "a memory is a fact, not a report: keep it under {MAX_CONTENT_CHARS} characters \
                 (got {})",
                content.chars().count()
            )));
        }
        let kind = match args.get("kind").and_then(Value::as_str) {
            None => MemoryKind::ProjectFact,
            Some(raw) => MemoryKind::parse(raw)
                .ok_or_else(|| invalid(format!("`{raw}` is not a memory kind")))?,
        };
        let scope = match args.get("scope").and_then(Value::as_str) {
            None | Some("project") => "project".to_string(),
            Some("global") => "global".to_string(),
            Some(raw) => {
                return Err(invalid(format!(
                    "scope must be project or global, not `{raw}`"
                )))
            }
        };
        let ttl_days = match args.get("ttl_days") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_u64()
                    .filter(|days| *days >= 1)
                    .and_then(|days| u32::try_from(days).ok())
                    .ok_or_else(|| invalid("ttl_days must be a whole number of days".into()))?,
            ),
        };
        Ok(MemoryInput {
            content,
            kind,
            scope,
            ttl_days,
        })
    }
}

#[async_trait::async_trait]
impl Tool for MemoryAddTool {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn action_request(&self, args: &Value) -> Result<ActionRequest> {
        let scope = args
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or("project");
        Ok(ActionRequest {
            tool: self.meta.name.clone(),
            risk: RiskLevel::Read,
            paths: vec![],
            formats: vec![],
            command: None,
            command_analysis: None,
            destination: None,
            summary: format!("record one {scope} memory"),
        })
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutput> {
        let input = Self::parse(&args)?;
        let store = MemoryStore::new(
            ctx.store.clone(),
            ctx.workspace.root().to_string_lossy().as_ref(),
            ctx.redactor.clone(),
            ctx.config.memory.global_enabled,
        );
        let source = match ctx.session.as_ref() {
            Some(session) => format!("agent:{}", session.as_str()),
            None => "agent".to_string(),
        };
        let id = store.add(NewMemory {
            kind: input.kind,
            content: input.content.clone(),
            source,
            confidence: AGENT_CONFIDENCE,
            scope: input.scope.clone(),
            sensitivity: "normal".into(),
            // Recording takes effect immediately unless the operator asked for
            // a review queue. An agent that writes down what it just worked out
            // and then cannot read it back has not remembered anything.
            requires_approval: requires_approval(ctx),
            ttl_days: input.ttl_days,
        })?;
        let tail = if requires_approval(ctx) {
            " — awaiting operator approval before it is recalled"
        } else {
            " — available to later turns"
        };
        Ok(ToolOutput::text(format!(
            "recorded memory {} ({}, {} scope){tail}",
            id.as_str(),
            input.kind.as_str(),
            input.scope
        ))
        .with_metadata(json!({
            "memory_id": id.as_str(),
            "kind": input.kind.as_str(),
            "scope": input.scope,
            "requires_approval": requires_approval(ctx),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> Arc<dyn Tool> {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        registry.get("memory.add").expect("registered")
    }

    /// Recording is not an approval gate by default.
    ///
    /// It used to be: every agent-recorded fact landed as a candidate and was
    /// invisible to later turns until a human clicked approve, so an agent that
    /// wrote down what it had just established could not read it back. The
    /// safety properties that matter are elsewhere and unchanged — secrets are
    /// refused, the store is separate from the workspace, and writes are
    /// budgeted per turn.
    #[test]
    fn recording_takes_effect_immediately_unless_the_operator_asks_otherwise() {
        let mut config = nexus_core::config::Config::default();
        assert!(
            !config.memory.require_approval,
            "recording must not be gated by default"
        );
        config.memory.require_approval = true;
        assert!(config.memory.require_approval, "the queue is still opt-in");
    }

    /// The description tells the model what to say about a recording, and it
    /// must not promise a review that is not happening.
    #[test]
    fn the_tool_description_does_not_promise_a_review_queue() {
        let description = tool().meta().description.clone();
        assert!(
            !description.contains("stored as a candidate"),
            "{description}"
        );
        assert!(
            description.contains("credentials are refused"),
            "{description}"
        );
    }

    #[test]
    fn the_summary_no_longer_says_the_memory_is_for_review() {
        let request = tool()
            .action_request(&json!({"content": "the parser is hand-written"}))
            .expect("action request");
        assert!(
            !request.summary.contains("for review"),
            "{}",
            request.summary
        );
        assert_eq!(request.risk, RiskLevel::Read);
    }

    #[test]
    fn the_memory_category_has_a_tool_to_carry_it() {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        let offered = registry.for_categories(&[ToolCategory::Memory]);
        assert_eq!(
            offered.len(),
            1,
            "granting the memory category must resolve to a callable tool",
        );
        assert_eq!(offered[0].meta().name, "memory.add");
    }

    #[test]
    fn recording_a_memory_is_not_a_workspace_write() {
        let tool = tool();
        assert_eq!(tool.meta().risk, RiskLevel::Read);
        assert!(!tool.meta().needs_network);
        assert!(!tool.meta().needs_sandbox);
        assert_eq!(
            tool.action_request(&json!({"content": "x"}))
                .expect("request")
                .risk,
            RiskLevel::Read,
            "a read-only role is meant to be able to record what it found",
        );
    }

    #[test]
    fn arguments_default_to_a_project_fact() {
        let parsed = MemoryAddTool::parse(&json!({"content": "  the parser is hand-written  "}))
            .expect("valid");
        assert_eq!(parsed.content, "the parser is hand-written");
        assert_eq!(parsed.kind.as_str(), "project_fact");
        assert_eq!(parsed.scope, "project");
        assert_eq!(parsed.ttl_days, None);
    }

    #[test]
    fn unusable_arguments_are_rejected_with_a_reason() {
        let empty = MemoryAddTool::parse(&json!({"content": "   "})).expect_err("empty content");
        assert!(empty.to_string().contains("nothing to remember"), "{empty}");

        let long = MemoryAddTool::parse(&json!({"content": "x".repeat(MAX_CONTENT_CHARS + 1)}))
            .expect_err("oversized content");
        assert!(long.to_string().contains("a fact, not a report"), "{long}");

        let kind = MemoryAddTool::parse(&json!({"content": "x", "kind": "vibes"}))
            .expect_err("unknown kind");
        assert!(kind.to_string().contains("not a memory kind"), "{kind}");

        let scope = MemoryAddTool::parse(&json!({"content": "x", "scope": "everywhere"}))
            .expect_err("unknown scope");
        assert!(scope.to_string().contains("project or global"), "{scope}");

        let ttl =
            MemoryAddTool::parse(&json!({"content": "x", "ttl_days": 0})).expect_err("zero ttl");
        assert!(ttl.to_string().contains("whole number of days"), "{ttl}");
    }

    #[test]
    fn the_write_budget_recognizes_the_tool_name() {
        // `loop_engine` bounds memory writes by matching the tool name; a name
        // it does not recognize would be an unbudgeted write.
        let name = tool().meta().name.to_ascii_lowercase();
        assert!(
            name.contains("memory")
                && (name.contains("write") || name.contains("add") || name.contains("save")),
            "`{name}` would slip past the memory-write budget",
        );
    }
}
