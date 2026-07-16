use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Risk classification of a tool or action. Ordered from least to most risky:
/// comparisons like `risk >= RiskLevel::Write` are meaningful.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Reads data within the workspace; no side effects.
    Read,
    /// Reaches the network (search, fetch); no local mutation.
    Network,
    /// Mutates files inside the workspace.
    Write,
    /// Deletes data or performs hard-to-reverse local operations.
    Destructive,
    /// Requires elevated privileges (denied by default).
    Privileged,
    /// Side effects visible outside this machine (push, publish, post).
    ExternalSideEffect,
}

impl fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            RiskLevel::Read => "read",
            RiskLevel::Network => "network",
            RiskLevel::Write => "write",
            RiskLevel::Destructive => "destructive",
            RiskLevel::Privileged => "privileged",
            RiskLevel::ExternalSideEffect => "external_side_effect",
        };
        f.write_str(s)
    }
}

/// A policy decision about a proposed action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Always allowed at this layer.
    Allow,
    /// Allowed exactly once; next occurrence re-evaluates.
    AllowOnce,
    /// Allowed for the remainder of the current session.
    AllowSession,
    /// The user must be asked.
    Ask,
    /// Denied; not presented for approval.
    Deny,
}

impl Decision {
    pub fn permits_execution(self) -> bool {
        matches!(
            self,
            Decision::Allow | Decision::AllowOnce | Decision::AllowSession
        )
    }
}

impl fmt::Display for Decision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Decision::Allow => "allow",
            Decision::AllowOnce => "allow_once",
            Decision::AllowSession => "allow_session",
            Decision::Ask => "ask",
            Decision::Deny => "deny",
        };
        f.write_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_ordering_matches_severity() {
        assert!(RiskLevel::Read < RiskLevel::Network);
        assert!(RiskLevel::Network < RiskLevel::Write);
        assert!(RiskLevel::Write < RiskLevel::Destructive);
        assert!(RiskLevel::Destructive < RiskLevel::Privileged);
        assert!(RiskLevel::Privileged < RiskLevel::ExternalSideEffect);
    }

    #[test]
    fn decision_execution_semantics() {
        assert!(Decision::Allow.permits_execution());
        assert!(!Decision::Ask.permits_execution());
        assert!(!Decision::Deny.permits_execution());
    }
}
