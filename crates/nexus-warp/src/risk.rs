//! Risk classification.
//!
//! Every candidate gets a tier 0–4 before any promotion question is asked. Two
//! properties matter more than the exact table below:
//!
//! * **Classification only ever moves up.** The tier WARP assigns is the maximum
//!   of what the candidate declared and what WARP computes — a candidate cannot
//!   talk its way down into a weaker gate by labelling itself `Low`.
//! * **Governance is absolute.** Any [`nexus_core::governance`] violation pins
//!   the candidate at [`RiskTier::Prohibited`], which the promotion gate
//!   auto-rejects. That check runs first and no later signal can undo it.

use nexus_core::governance::{self, CandidateFacts, GovernanceReview};
use nexus_core::harness::{ImprovementPlane, ImprovementProposal, ImprovementTarget, RiskTier};
use serde::{Deserialize, Serialize};

/// Permission/impact keywords that force at least Tier 3 (human approval).
const HIGH_RISK_KEYWORDS: &[&str] = &[
    "network",
    "credential",
    "token",
    "secret",
    "deploy",
    "publish",
    "push",
    "release",
    "install",
    "mcp",
    "permission",
    "sudo",
    "root",
    "delete",
];

/// The outcome of classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskAssessment {
    pub candidate_id: String,
    /// The effective tier — never lower than the declared one.
    pub tier: RiskTier,
    /// The tier the candidate claimed for itself.
    pub declared_tier: RiskTier,
    pub governance: GovernanceReview,
    /// Human-readable reasons, in the order they were applied.
    pub rationale: Vec<String>,
}

impl RiskAssessment {
    /// Prohibited candidates are auto-rejected; nothing downstream may run.
    pub fn is_prohibited(&self) -> bool {
        self.tier == RiskTier::Prohibited
    }

    /// Tier 2+ must be observed in shadow before it can be promoted.
    pub fn requires_shadow(&self) -> bool {
        self.tier >= RiskTier::Moderate && self.tier != RiskTier::Prohibited
    }

    /// Tier 3+ requires a human signature.
    pub fn requires_human_approval(&self) -> bool {
        self.tier >= RiskTier::High
    }
}

/// Assigns risk tiers from a candidate's target, blast radius, and permissions.
pub struct RiskClassifier;

impl RiskClassifier {
    /// The tier a target carries before any other signal.
    fn baseline_for(target: ImprovementTarget) -> RiskTier {
        match target {
            // Reversible presentation/data with no behavioural reach.
            ImprovementTarget::Memory | ImprovementTarget::TimelinePresentation => RiskTier::Low,
            // Changes how the agent behaves, but inside the sandbox of one turn.
            ImprovementTarget::Skill
            | ImprovementTarget::Prompt
            | ImprovementTarget::ContextRouter
            | ImprovementTarget::RetrievalPolicy
            | ImprovementTarget::ToolRouter
            | ImprovementTarget::PlannerPolicy
            | ImprovementTarget::AgentRole
            | ImprovementTarget::RetryPolicy
            | ImprovementTarget::ErrorRecovery
            | ImprovementTarget::TokenBudgetPolicy => RiskTier::Moderate,
            // Changing how candidates are judged is one step from judging
            // yourself — always a human decision.
            ImprovementTarget::EvaluationPolicy => RiskTier::High,
            // Code-plane: rebuilt and shipped through a human-approved release.
            ImprovementTarget::HarnessComponent => RiskTier::High,
        }
    }

    /// Classify a proposal.
    pub fn classify(proposal: &ImprovementProposal) -> RiskAssessment {
        let narrative = format!(
            "{}\n{}\n{}",
            proposal.problem, proposal.proposed_change, proposal.root_cause_hypothesis
        );
        let review = governance::review(CandidateFacts {
            candidate_id: &proposal.id,
            affected_components: &proposal.affected_components,
            required_permissions: &proposal.required_permissions,
            narrative: &narrative,
            created_by: &proposal.created_by,
            approved_by: None,
        });

        let declared = proposal.risk_tier;
        let mut rationale = Vec::new();

        if !review.permits() {
            let mut reasons = vec!["governance violation → prohibited".to_string()];
            reasons.extend(review.describe());
            return RiskAssessment {
                candidate_id: proposal.id.clone(),
                tier: RiskTier::Prohibited,
                declared_tier: declared,
                governance: review,
                rationale: reasons,
            };
        }

        let mut tier = Self::baseline_for(proposal.target);
        rationale.push(format!(
            "target `{}` baselines at {}",
            proposal.target.as_str(),
            tier.as_str()
        ));

        if proposal.target.plane() == ImprovementPlane::Code {
            tier = tier.max(RiskTier::High);
            rationale
                .push("code-plane change: ships only through a human-approved release".to_string());
        }

        for permission in &proposal.required_permissions {
            let lowered = permission.to_ascii_lowercase();
            if let Some(word) = HIGH_RISK_KEYWORDS.iter().find(|k| lowered.contains(**k)) {
                tier = tier.max(RiskTier::High);
                rationale.push(format!(
                    "permission `{permission}` involves `{word}` → human approval"
                ));
            }
        }

        for component in &proposal.affected_components {
            let lowered = component.to_ascii_lowercase();
            if let Some(word) = HIGH_RISK_KEYWORDS.iter().find(|k| lowered.contains(**k)) {
                tier = tier.max(RiskTier::High);
                rationale.push(format!(
                    "component `{component}` involves `{word}` → human approval"
                ));
            }
        }

        if declared > tier {
            rationale.push(format!(
                "declared tier {} is stricter than computed {} — declared wins",
                declared.as_str(),
                tier.as_str()
            ));
            tier = declared;
        } else if declared < tier {
            rationale.push(format!(
                "declared tier {} raised to {} by classification",
                declared.as_str(),
                tier.as_str()
            ));
        }

        RiskAssessment {
            candidate_id: proposal.id.clone(),
            tier,
            declared_tier: declared,
            governance: review,
            rationale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::harness::{ImprovementCategory, ImprovementProposal};

    fn proposal(target: ImprovementTarget, declared: RiskTier) -> ImprovementProposal {
        let mut p = ImprovementProposal::new(
            ImprovementCategory::Tool,
            "repeated tool failures",
            "route around the failing tool",
        )
        .expect("proposal");
        p.target = target;
        p.risk_tier = declared;
        p.created_by = "improvement_planner".into();
        p
    }

    #[test]
    fn memory_candidates_are_tier_one() {
        let a = RiskClassifier::classify(&proposal(ImprovementTarget::Memory, RiskTier::Low));
        assert_eq!(a.tier, RiskTier::Low);
        assert!(!a.requires_shadow());
        assert!(!a.requires_human_approval());
    }

    #[test]
    fn tool_routing_is_tier_two_and_needs_shadow() {
        let a = RiskClassifier::classify(&proposal(ImprovementTarget::ToolRouter, RiskTier::Low));
        assert_eq!(a.tier, RiskTier::Moderate);
        assert!(a.requires_shadow());
        assert!(!a.requires_human_approval());
    }

    #[test]
    fn a_candidate_cannot_declare_its_way_into_a_weaker_gate() {
        // Declares Observation; the target says code-plane.
        let a = RiskClassifier::classify(&proposal(
            ImprovementTarget::HarnessComponent,
            RiskTier::Observation,
        ));
        assert_eq!(a.tier, RiskTier::High);
        assert_eq!(a.declared_tier, RiskTier::Observation);
        assert!(a.requires_human_approval());
    }

    #[test]
    fn a_stricter_declared_tier_is_respected() {
        let a = RiskClassifier::classify(&proposal(ImprovementTarget::Memory, RiskTier::High));
        assert_eq!(a.tier, RiskTier::High);
    }

    #[test]
    fn evaluation_policy_changes_always_reach_a_human() {
        let a = RiskClassifier::classify(&proposal(
            ImprovementTarget::EvaluationPolicy,
            RiskTier::Low,
        ));
        assert_eq!(a.tier, RiskTier::High);
    }

    #[test]
    fn network_and_credential_permissions_escalate_to_tier_three() {
        let mut p = proposal(ImprovementTarget::RetrievalPolicy, RiskTier::Low);
        p.required_permissions = vec!["network.fetch".into()];
        let a = RiskClassifier::classify(&p);
        assert_eq!(a.tier, RiskTier::High);
        assert!(a.rationale.iter().any(|r| r.contains("network")));
    }

    #[test]
    fn a_candidate_touching_governance_is_prohibited() {
        let mut p = proposal(ImprovementTarget::HarnessComponent, RiskTier::Low);
        p.affected_components = vec!["crates/nexus-core/src/governance.rs".into()];
        let a = RiskClassifier::classify(&p);
        assert!(a.is_prohibited());
        assert!(!a.governance.permits());
    }

    #[test]
    fn a_candidate_that_wants_to_skip_tests_is_prohibited() {
        let mut p = proposal(ImprovementTarget::PlannerPolicy, RiskTier::Low);
        p.proposed_change = "skip tests when the diff is small to cut latency".into();
        assert!(RiskClassifier::classify(&p).is_prohibited());
    }
}
