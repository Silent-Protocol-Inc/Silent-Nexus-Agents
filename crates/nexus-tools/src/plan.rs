//! Plan authoring (`plan.*`).
//!
//! One tool, offered only while plan mode is active. It is how the agent hands
//! back a plan it has researched: the loop reads the structured result, records
//! it, and puts it in front of the operator for approval.
//!
//! Nothing here touches the workspace. The tool's whole effect is to return
//! what the agent decided, which is why it carries [`RiskLevel::Read`] — the
//! plan it describes is not acted on until a human approves it.

use crate::{Tool, ToolCategory, ToolContext, ToolMeta, ToolOutput, ToolRegistry};
use nexus_core::{NexusError, Result, RiskLevel};
use nexus_policy::ActionRequest;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

/// One step of an authored plan, as the model supplies it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepInput {
    /// Imperative one-line title, e.g. "Clamp the discovered context window".
    pub title: String,
    /// What changes and why.
    pub detail: String,
    /// Files this step touches. Empty is allowed for a step that only runs
    /// something or decides something.
    #[serde(default)]
    pub files: Vec<String>,
    /// How the step is shown to have worked.
    #[serde(default)]
    pub verification: Option<String>,
}

/// The whole submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanSubmission {
    pub objective: String,
    #[serde(default)]
    pub findings: Vec<String>,
    pub steps: Vec<PlanStepInput>,
}

/// Guardrails on the shape of a submission. A plan with fifty steps or a
/// one-word step is not a plan the operator can meaningfully approve.
const MAX_STEPS: usize = 30;
const MIN_TITLE_CHARS: usize = 3;

struct PlanSubmitTool {
    meta: ToolMeta,
}

fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "objective": {
                "type": "string",
                "description": "What the plan accomplishes, in one sentence.",
            },
            "findings": {
                "type": "array",
                "items": {"type": "string"},
                "description": "What inspecting the workspace established. Cite concrete files or behavior; this is the evidence the plan rests on.",
            },
            "steps": {
                "type": "array",
                "description": "Ordered steps. Each names real files where it touches them.",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Imperative one-line summary of the step."},
                        "detail": {"type": "string", "description": "What changes and why."},
                        "files": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Workspace-relative paths this step touches.",
                        },
                        "verification": {"type": "string", "description": "How this step is shown to have worked."},
                    },
                    "required": ["title", "detail"],
                    "additionalProperties": false,
                },
            },
        },
        "required": ["objective", "steps"],
        "additionalProperties": false
    })
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(Arc::new(PlanSubmitTool {
        meta: ToolMeta {
            name: "plan.submit".into(),
            namespace: "plan".into(),
            description: "Submit a researched plan for operator approval. Call this once, after \
                          inspecting the workspace, when you know which files change and why. \
                          Nothing is executed until the operator approves."
                .into(),
            category: ToolCategory::Goal,
            input_schema: input_schema(),
            output_schema: json!({"type": "string"}),
            risk: RiskLevel::Read,
            required_capabilities: vec![],
            timeout_secs: 15,
            max_output_bytes: 32_000,
            deterministic: true,
            needs_network: false,
            needs_sandbox: false,
            side_effects: "records a plan draft for approval; changes nothing in the workspace"
                .into(),
        },
    }));
}

impl PlanSubmitTool {
    fn parse(args: Value) -> Result<PlanSubmission> {
        let submission: PlanSubmission =
            serde_json::from_value(args).map_err(|error| NexusError::ToolInput {
                tool: "plan.submit".into(),
                message: format!("expected an objective and a list of steps: {error}"),
            })?;
        let invalid = |message: String| NexusError::ToolInput {
            tool: "plan.submit".into(),
            message,
        };
        if submission.objective.trim().is_empty() {
            return Err(invalid("the objective cannot be empty".into()));
        }
        if submission.steps.is_empty() {
            return Err(invalid(
                "a plan needs at least one step; answer normally if there is nothing to plan"
                    .into(),
            ));
        }
        if submission.steps.len() > MAX_STEPS {
            return Err(invalid(format!(
                "{} steps is more than an operator can review at once; group them (max {MAX_STEPS})",
                submission.steps.len()
            )));
        }
        for (index, step) in submission.steps.iter().enumerate() {
            if step.title.trim().chars().count() < MIN_TITLE_CHARS {
                return Err(invalid(format!("step {} needs a real title", index + 1)));
            }
            if step.detail.trim().is_empty() {
                return Err(invalid(format!(
                    "step {} (`{}`) needs to say what changes and why",
                    index + 1,
                    step.title.trim()
                )));
            }
        }
        Ok(submission)
    }
}

#[async_trait::async_trait]
impl Tool for PlanSubmitTool {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn action_request(&self, _args: &Value) -> Result<ActionRequest> {
        Ok(ActionRequest {
            tool: self.meta.name.clone(),
            // Submitting a plan proposes work; it does not perform any. The
            // approval that gates the work happens after this returns.
            risk: RiskLevel::Read,
            paths: vec![],
            formats: vec![],
            command: None,
            command_analysis: None,
            destination: None,
            summary: "submit a plan for approval".into(),
        })
    }

    async fn execute(&self, _ctx: &ToolContext, args: Value) -> Result<ToolOutput> {
        let submission = Self::parse(args)?;
        let summary = format!(
            "plan submitted for approval: {} step(s)",
            submission.steps.len()
        );
        Ok(ToolOutput {
            content: summary,
            artifact_id: None,
            // The loop reads this back to build the durable plan. Keeping the
            // structure here rather than persisting from inside the tool leaves
            // every orchestration write in one place.
            metadata: serde_json::to_value(&submission).unwrap_or(Value::Null),
            truncated: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn submission(steps: Value) -> Value {
        json!({"objective": "make the thing work", "steps": steps})
    }

    #[test]
    fn a_submission_carries_its_steps_through_unchanged() {
        let parsed = PlanSubmitTool::parse(submission(json!([
            {
                "title": "Clamp the discovered context window",
                "detail": "Record the reported maximum and request a smaller one.",
                "files": ["crates/nexus-app/src/providers.rs"],
                "verification": "cargo test -p nexus-app",
            },
            {"title": "Regenerate the schema", "detail": "The gate fails on drift."},
        ])))
        .expect("valid submission");
        assert_eq!(parsed.steps.len(), 2);
        assert_eq!(parsed.steps[0].files, ["crates/nexus-app/src/providers.rs"]);
        assert_eq!(
            parsed.steps[1].files,
            Vec::<String>::new(),
            "a step that touches no file is still a step",
        );
    }

    #[test]
    fn a_plan_that_says_nothing_is_rejected_with_a_reason() {
        let empty =
            PlanSubmitTool::parse(submission(json!([]))).expect_err("an empty plan is not a plan");
        assert!(empty.to_string().contains("at least one step"), "{empty}");

        let thin = PlanSubmitTool::parse(submission(json!([
            {"title": "Fix it", "detail": "   "}
        ])))
        .expect_err("a step must say what changes");
        assert!(thin.to_string().contains("what changes and why"), "{thin}");

        let unnamed = PlanSubmitTool::parse(submission(json!([
            {"title": "x", "detail": "something"}
        ])))
        .expect_err("a step needs a real title");
        assert!(unnamed.to_string().contains("real title"), "{unnamed}");
    }

    #[test]
    fn submitting_a_plan_is_not_itself_a_write() {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        let tool = registry.get("plan.submit").expect("registered");
        assert_eq!(tool.meta().risk, RiskLevel::Read);
        assert_eq!(tool.meta().category, ToolCategory::Goal);
        assert!(!tool.meta().needs_network);
        assert_eq!(
            tool.action_request(&json!({})).expect("request").risk,
            RiskLevel::Read,
            "the approval that gates the work comes after this returns",
        );
    }
}
