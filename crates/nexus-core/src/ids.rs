use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! typed_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Generate a new random id with the type prefix, e.g. `sess_1f3a…`.
            pub fn generate() -> Self {
                let u = uuid::Uuid::new_v4().simple().to_string();
                Self(format!("{}_{}", $prefix, &u[..12]))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }
    };
}

typed_id!(
    /// A durable conversation session.
    SessionId, "sess");
typed_id!(
    /// A persistent goal managed by the goal engine.
    GoalId, "goal");
typed_id!(
    /// One step inside a goal plan.
    StepId, "step");
typed_id!(
    /// A single tool invocation.
    ToolCallId, "call");
typed_id!(
    /// A stored memory record.
    MemoryId, "mem");
typed_id!(
    /// A reusable skill package.
    SkillId, "skill");
typed_id!(
    /// A registered MCP server.
    McpServerId, "mcp");
typed_id!(
    /// A stored artifact (full tool output, downloaded file, etc.).
    ArtifactId, "art");
typed_id!(
    /// A spawned subagent.
    AgentId, "agent");
typed_id!(
    /// Trace id correlating all events of one agent turn.
    TraceId, "trace");
typed_id!(
    /// One logical user/agent turn inside a session.
    TurnId, "turn");
typed_id!(
    /// One lifecycle span inside a trace.
    SpanId, "span");
typed_id!(
    /// A durable background task.
    TaskId, "task");
typed_id!(
    /// A versioned orchestration plan.
    PlanId, "plan");
typed_id!(
    /// A persisted provider-request context manifest.
    ManifestId, "ctx");
typed_id!(
    /// A classified session interruption.
    InterruptionId, "interrupt");
typed_id!(
    /// An approval request presented to the user.
    ApprovalId, "appr");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_prefixed_and_unique() {
        let a = SessionId::generate();
        let b = SessionId::generate();
        assert!(a.as_str().starts_with("sess_"));
        assert_ne!(a, b);
    }
}
