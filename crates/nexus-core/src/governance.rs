//! Immutable governance for self-improvement.
//!
//! This module is the constitution the RSI/WARP pipeline is judged against. It
//! lives in `nexus-core` on purpose: `nexus-rsi` and `nexus-warp` *depend on*
//! this crate, so a candidate produced by that pipeline can never reach around
//! and rewrite the rules that govern it. The ruleset is `const` — there is no
//! setter, no table, no config key that edits it. Changing governance requires a
//! human writing Rust and shipping a release.
//!
//! Three layers enforce that, weakest last:
//!
//! 1. **Dependency direction** — the pipeline cannot import its way upward.
//! 2. **Protected components** — any candidate whose blast radius touches
//!    governance, audit, policy/permissions, or the validation layer is
//!    classified `Prohibited` and auto-rejected ([`PROTECTED_COMPONENTS`]).
//! 3. **Intent screening** — a text screen for the classic bypass phrasings
//!    ([`PROHIBITED_INTENTS`]). This is defence in depth and nothing more: it is
//!    a keyword screen, it is evadable by rewording, and it is deliberately not
//!    the thing the safety argument rests on. Layers 1 and 2 are.

use serde::{Deserialize, Serialize};

/// Bumped whenever the ruleset below changes. Recorded on every promotion so an
/// audit can tell which constitution was in force.
pub const GOVERNANCE_VERSION: u32 = 1;

/// One non-negotiable rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GovernanceRule {
    pub id: &'static str,
    pub statement: &'static str,
}

/// The complete ruleset. Compile-time, ordered, and not editable at runtime.
pub const GOVERNANCE_RULES: &[GovernanceRule] = &[
    GovernanceRule {
        id: "correctness_over_speed",
        statement: "Correctness and safety outrank speed, cost, and convenience.",
    },
    GovernanceRule {
        id: "evidence_over_confidence",
        statement: "Deterministic evidence outranks model confidence; a stated \
                    certainty is never a substitute for a passing test.",
    },
    GovernanceRule {
        id: "security_never_averaged",
        statement: "A security or correctness failure is a veto — it is never \
                    averaged against gains elsewhere.",
    },
    GovernanceRule {
        id: "user_intent_outranks_agent",
        statement: "User intent outranks agent preference; self-improvement never \
                    rewrites the user's goal.",
    },
    GovernanceRule {
        id: "no_self_expanded_permissions",
        statement: "No candidate may widen its own permissions, network reach, or \
                    credential access.",
    },
    GovernanceRule {
        id: "validation_is_not_self_editable",
        statement: "The validation, governance, audit, and policy layers may not be \
                    modified by the pipeline they constrain.",
    },
    GovernanceRule {
        id: "untested_cannot_promote",
        statement: "An unvalidated change can never be promoted; missing or \
                    incomplete validation fails closed.",
    },
    GovernanceRule {
        id: "audit_is_append_only",
        statement: "The audit trail is append-only; no candidate may edit or delete \
                    a past record.",
    },
    GovernanceRule {
        id: "promotions_attributable_and_reversible",
        statement: "Every promotion records its author, approver, and rollback path.",
    },
    GovernanceRule {
        id: "unverified_memory_is_not_truth",
        statement: "Unverified memory is a candidate, not a fact.",
    },
    GovernanceRule {
        id: "separation_of_powers",
        statement: "No single role may define success, make the change, judge the \
                    result, authorize it, and promote it.",
    },
];

/// Look a rule up by id.
pub fn rule(id: &str) -> Option<&'static GovernanceRule> {
    GOVERNANCE_RULES.iter().find(|r| r.id == id)
}

/// Component substrings no autonomous candidate may modify. Matched
/// case-insensitively against a candidate's `affected_components`, so both a
/// path (`crates/nexus-core/src/governance.rs`) and a bare name (`governance`)
/// are caught.
pub const PROTECTED_COMPONENTS: &[&str] = &[
    "governance",
    "audit",
    "nexus-policy",
    "nexus_policy",
    "permissions.rs",
    "permission",
    "nexus-warp",
    "nexus_warp",
    "warp",
    "validation",
    "validator",
    "promotion",
    "rollback",
    "redact",
    "secret",
    "sandbox",
];

/// Phrasings that describe a governance bypass rather than an improvement.
/// Defence in depth only — see the module docs.
pub const PROHIBITED_INTENTS: &[&str] = &[
    "disable validation",
    "skip validation",
    "bypass validation",
    "disable the tests",
    "skip the tests",
    "skip tests",
    "disable tests",
    "weaken security",
    "disable security",
    "relax security",
    "disable sandbox",
    "edit the audit",
    "rewrite the audit",
    "delete audit",
    "clear the audit",
    "tamper",
    "expand permissions",
    "grant itself",
    "grant myself",
    "self-approve",
    "self approve",
    "bypass approval",
    "bypass human",
    "without human approval",
    "hide the change",
    "hide changes",
    "conceal",
    "suppress the warning",
    "edit governance",
    "change governance",
    "override governance",
];

/// Why a candidate is refused by governance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GovernanceViolation {
    /// The candidate's blast radius includes a protected component.
    ProtectedComponent {
        component: String,
        matched: String,
        rule: String,
    },
    /// The candidate's own description states a bypass intent.
    ProhibitedIntent { phrase: String, rule: String },
    /// The candidate asks for a permission its risk tier may not self-grant.
    PermissionExpansion { permission: String, rule: String },
    /// The author and the approver are the same party.
    SelfAuthorization { party: String, rule: String },
}

impl GovernanceViolation {
    pub fn rule_id(&self) -> &str {
        match self {
            Self::ProtectedComponent { rule, .. }
            | Self::ProhibitedIntent { rule, .. }
            | Self::PermissionExpansion { rule, .. }
            | Self::SelfAuthorization { rule, .. } => rule,
        }
    }

    /// One-line, audit-friendly rendering.
    pub fn describe(&self) -> String {
        match self {
            Self::ProtectedComponent {
                component, matched, ..
            } => format!("touches protected component `{component}` (matched `{matched}`)"),
            Self::ProhibitedIntent { phrase, .. } => {
                format!("states a prohibited intent: `{phrase}`")
            }
            Self::PermissionExpansion { permission, .. } => {
                format!("requests self-expanded permission `{permission}`")
            }
            Self::SelfAuthorization { party, .. } => {
                format!("author and approver are the same party: `{party}`")
            }
        }
    }
}

/// Permissions a candidate may never grant itself as part of its own change.
const SELF_GRANT_DENIED_PERMISSIONS: &[&str] = &[
    "permissions.write",
    "policy.write",
    "governance.write",
    "audit.write",
    "audit.delete",
    "validation.disable",
    "promote.self",
];

/// The facts governance judges. Deliberately a plain borrowed struct so this
/// module has no dependency on the pipeline's types and stays trivially
/// testable.
#[derive(Debug, Clone, Copy)]
pub struct CandidateFacts<'a> {
    pub candidate_id: &'a str,
    pub affected_components: &'a [String],
    pub required_permissions: &'a [String],
    /// Free text the candidate wrote about itself (problem + proposed change +
    /// rationale, concatenated by the caller).
    pub narrative: &'a str,
    pub created_by: &'a str,
    /// Who signed off, when someone has.
    pub approved_by: Option<&'a str>,
}

/// The result of a governance review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GovernanceReview {
    pub candidate_id: String,
    pub governance_version: u32,
    pub violations: Vec<GovernanceViolation>,
}

impl GovernanceReview {
    /// True only when nothing fired. Governance never returns "probably fine".
    pub fn permits(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn describe(&self) -> Vec<String> {
        self.violations.iter().map(|v| v.describe()).collect()
    }
}

/// Reviews a candidate against the constitution.
///
/// This is a pure function of the facts: no I/O, no config, no model. Anything
/// it flags is `Prohibited` — there is no severity dial and no partial credit.
pub fn review(facts: CandidateFacts<'_>) -> GovernanceReview {
    let mut violations = Vec::new();

    for component in facts.affected_components {
        let lowered = component.to_ascii_lowercase();
        if let Some(matched) = PROTECTED_COMPONENTS.iter().find(|p| lowered.contains(**p)) {
            violations.push(GovernanceViolation::ProtectedComponent {
                component: component.clone(),
                matched: (*matched).to_string(),
                rule: "validation_is_not_self_editable".into(),
            });
        }
    }

    let narrative = facts.narrative.to_ascii_lowercase();
    for phrase in PROHIBITED_INTENTS {
        if narrative.contains(phrase) {
            violations.push(GovernanceViolation::ProhibitedIntent {
                phrase: (*phrase).to_string(),
                rule: "no_self_expanded_permissions".into(),
            });
        }
    }

    for permission in facts.required_permissions {
        let lowered = permission.to_ascii_lowercase();
        if SELF_GRANT_DENIED_PERMISSIONS
            .iter()
            .any(|p| lowered == *p || lowered.contains(p))
        {
            violations.push(GovernanceViolation::PermissionExpansion {
                permission: permission.clone(),
                rule: "no_self_expanded_permissions".into(),
            });
        }
    }

    if let Some(approver) = facts.approved_by {
        let author = facts.created_by.trim();
        if !author.is_empty() && approver.trim().eq_ignore_ascii_case(author) {
            violations.push(GovernanceViolation::SelfAuthorization {
                party: author.to_string(),
                rule: "separation_of_powers".into(),
            });
        }
    }

    GovernanceReview {
        candidate_id: facts.candidate_id.to_string(),
        governance_version: GOVERNANCE_VERSION,
        violations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(
        components: &'a [String],
        permissions: &'a [String],
        narrative: &'a str,
    ) -> CandidateFacts<'a> {
        CandidateFacts {
            candidate_id: "cnd-1",
            affected_components: components,
            required_permissions: permissions,
            narrative,
            created_by: "improvement_planner",
            approved_by: None,
        }
    }

    #[test]
    fn every_rule_has_a_unique_id_and_statement() {
        for r in GOVERNANCE_RULES {
            assert!(!r.id.is_empty() && !r.statement.is_empty());
            assert_eq!(
                GOVERNANCE_RULES.iter().filter(|o| o.id == r.id).count(),
                1,
                "duplicate rule id {}",
                r.id
            );
        }
        assert!(rule("security_never_averaged").is_some());
        assert!(rule("no_such_rule").is_none());
    }

    #[test]
    fn a_benign_data_plane_candidate_is_permitted() {
        let components = vec!["retrieval cache".to_string()];
        let review = review(facts(&components, &[], "dedupe repeated file reads"));
        assert!(review.permits(), "{:?}", review.violations);
    }

    #[test]
    fn touching_the_governance_module_is_prohibited() {
        let components = vec!["crates/nexus-core/src/governance.rs".to_string()];
        let review = review(facts(&components, &[], "improve rule lookup speed"));
        assert!(!review.permits());
        assert_eq!(
            review.violations[0].rule_id(),
            "validation_is_not_self_editable"
        );
    }

    #[test]
    fn touching_the_validation_layer_is_prohibited() {
        for component in [
            "nexus-warp/src/deterministic.rs",
            "audit_events",
            "nexus-policy",
            "src/permissions.rs",
        ] {
            let components = vec![component.to_string()];
            let review = review(facts(&components, &[], "refactor"));
            assert!(!review.permits(), "{component} should be protected");
        }
    }

    #[test]
    fn a_stated_bypass_intent_is_prohibited() {
        let review = review(facts(
            &[],
            &[],
            "Latency is dominated by the suite, so skip tests on the hot path.",
        ));
        assert!(!review.permits());
        assert!(review.describe()[0].contains("skip tests"));
    }

    #[test]
    fn self_granted_permissions_are_prohibited() {
        let permissions = vec!["permissions.write".to_string()];
        let review = review(facts(&[], &permissions, "needs to adjust its own grants"));
        assert!(!review.permits());
        assert!(matches!(
            review.violations[0],
            GovernanceViolation::PermissionExpansion { .. }
        ));
    }

    #[test]
    fn an_author_cannot_approve_its_own_candidate() {
        let mut f = facts(&[], &[], "fine change");
        f.approved_by = Some("improvement_planner");
        let review = review(f);
        assert!(!review.permits());
        assert_eq!(review.violations[0].rule_id(), "separation_of_powers");
    }

    #[test]
    fn a_distinct_approver_is_accepted() {
        let mut f = facts(&[], &[], "fine change");
        f.approved_by = Some("human:sans");
        assert!(review(f).permits());
    }
}
