//! Independent evaluators.
//!
//! Several evaluator roles judge a candidate from isolated contexts. Two rules
//! keep them honest:
//!
//! 1. **The author's reasoning is excluded.** [`EvaluatorInput`] carries only
//!    requirements, the candidate delta, objective results, and baseline/candidate
//!    outputs — never the proposing model's rationale or confidence. The type has
//!    no field for it, so the exclusion is structural, not a convention.
//! 2. **Objective evidence outranks opinion.** If deterministic/replay/adversarial
//!    stages already produced hard failures, the pool returns `Rejected` no matter
//!    how the evaluators vote. A model can never talk WARP past a failed test.

use crate::requirements::RequirementSet;
use crate::{ValidationReport, Verdict};
use serde::{Deserialize, Serialize};

/// The distinct evaluator perspectives. Each runs in its own context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluatorRole {
    Requirement,
    Correctness,
    Regression,
    Security,
    Efficiency,
    Usability,
}

impl EvaluatorRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requirement => "requirement",
            Self::Correctness => "correctness",
            Self::Regression => "regression",
            Self::Security => "security",
            Self::Efficiency => "efficiency",
            Self::Usability => "usability",
        }
    }

    /// Roles whose hard failure is a safety/correctness veto regardless of gains.
    pub fn is_veto_role(self) -> bool {
        matches!(self, Self::Correctness | Self::Regression | Self::Security)
    }
}

/// What an evaluator is allowed to see. Deliberately **without** any author
/// reasoning or confidence field — that is the isolation boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluatorInput {
    pub requirements: RequirementSet,
    /// The candidate's diff or configuration delta (already redacted).
    pub candidate_delta: String,
    /// Hard failures already produced by objective stages (deterministic, replay,
    /// adversarial). Non-empty means the objective verdict is already "reject".
    pub objective_failures: Vec<String>,
    pub baseline_output: String,
    pub candidate_output: String,
    pub evidence: Vec<String>,
}

impl EvaluatorInput {
    /// Build from objective material only. There is intentionally no constructor
    /// that accepts author rationale.
    pub fn from_objective(
        requirements: RequirementSet,
        candidate_delta: impl Into<String>,
        objective_failures: Vec<String>,
    ) -> Self {
        Self {
            requirements,
            candidate_delta: candidate_delta.into(),
            objective_failures,
            baseline_output: String::new(),
            candidate_output: String::new(),
            evidence: Vec::new(),
        }
    }
}

/// An evaluator's recommendation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluatorRecommendation {
    Approve,
    Revise,
    Reject,
}

/// One evaluator's structured verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluatorVerdict {
    pub role: EvaluatorRole,
    pub recommendation: EvaluatorRecommendation,
    pub confidence: f64,
    pub hard_failures: Vec<String>,
    pub soft_failures: Vec<String>,
    pub evidence: Vec<String>,
    /// Claims the candidate made that the evidence does not support.
    pub unsupported_claims: Vec<String>,
    pub recommended_action: String,
}

/// Produces a verdict for one role. Injectable so the aggregation policy is tested
/// without a live model; the production impl runs each role in an isolated
/// subagent context via a configured provider.
pub trait Evaluator {
    fn evaluate(&self, role: EvaluatorRole, input: &EvaluatorInput) -> EvaluatorVerdict;
}

/// Runs the configured evaluator roles and aggregates their verdicts.
pub struct EvaluatorPool {
    roles: Vec<EvaluatorRole>,
}

impl Default for EvaluatorPool {
    fn default() -> Self {
        Self::standard()
    }
}

impl EvaluatorPool {
    /// The six standard perspectives.
    pub fn standard() -> Self {
        Self {
            roles: vec![
                EvaluatorRole::Requirement,
                EvaluatorRole::Correctness,
                EvaluatorRole::Regression,
                EvaluatorRole::Security,
                EvaluatorRole::Efficiency,
                EvaluatorRole::Usability,
            ],
        }
    }

    pub fn with_roles(roles: Vec<EvaluatorRole>) -> Self {
        Self { roles }
    }

    /// Evaluate a candidate. Returns each verdict plus the aggregated report.
    pub fn evaluate(
        &self,
        evaluator: &impl Evaluator,
        candidate_id: &str,
        input: &EvaluatorInput,
    ) -> (Vec<EvaluatorVerdict>, ValidationReport) {
        let verdicts: Vec<EvaluatorVerdict> = self
            .roles
            .iter()
            .map(|&role| evaluator.evaluate(role, input))
            .collect();

        let mut report = ValidationReport::new(candidate_id, "evaluators");

        // Objective evidence outranks every opinion: if a prior objective stage
        // already failed hard, this stage is Rejected no matter how roles voted.
        if !input.objective_failures.is_empty() {
            report.verdict = Verdict::Rejected;
            report.hard_failures = input.objective_failures.clone();
            return (verdicts, report);
        }

        for verdict in &verdicts {
            for failure in &verdict.hard_failures {
                report
                    .hard_failures
                    .push(format!("{}: {failure}", verdict.role.as_str()));
            }
            for soft in &verdict.soft_failures {
                report
                    .soft_failures
                    .push(format!("{}: {soft}", verdict.role.as_str()));
            }
            for claim in &verdict.unsupported_claims {
                report.soft_failures.push(format!(
                    "{} unsupported claim: {claim}",
                    verdict.role.as_str()
                ));
            }
        }

        let any_reject = verdicts
            .iter()
            .any(|v| v.recommendation == EvaluatorRecommendation::Reject);
        let any_revise = verdicts
            .iter()
            .any(|v| v.recommendation == EvaluatorRecommendation::Revise);

        report.verdict = if !report.hard_failures.is_empty() || any_reject {
            Verdict::Rejected
        } else if any_revise {
            Verdict::NeedsRevision
        } else {
            Verdict::Passed
        };
        (verdicts, report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(objective_failures: Vec<String>) -> EvaluatorInput {
        EvaluatorInput::from_objective(RequirementSet::default(), "delta", objective_failures)
    }

    fn verdict(role: EvaluatorRole, rec: EvaluatorRecommendation) -> EvaluatorVerdict {
        EvaluatorVerdict {
            role,
            recommendation: rec,
            confidence: 0.8,
            hard_failures: Vec::new(),
            soft_failures: Vec::new(),
            evidence: Vec::new(),
            unsupported_claims: Vec::new(),
            recommended_action: String::new(),
        }
    }

    struct FixedEvaluator {
        rec: EvaluatorRecommendation,
        security_hard: Option<String>,
    }
    impl Evaluator for FixedEvaluator {
        fn evaluate(&self, role: EvaluatorRole, _input: &EvaluatorInput) -> EvaluatorVerdict {
            let mut v = verdict(role, self.rec);
            if role == EvaluatorRole::Security {
                if let Some(f) = &self.security_hard {
                    v.hard_failures.push(f.clone());
                    v.recommendation = EvaluatorRecommendation::Reject;
                }
            }
            v
        }
    }

    #[test]
    fn unanimous_approval_passes() {
        let e = FixedEvaluator {
            rec: EvaluatorRecommendation::Approve,
            security_hard: None,
        };
        let (verdicts, report) = EvaluatorPool::standard().evaluate(&e, "cnd-1", &input(vec![]));
        assert_eq!(verdicts.len(), 6);
        assert_eq!(report.verdict, Verdict::Passed);
    }

    #[test]
    fn objective_failure_overrides_unanimous_approval() {
        // Every evaluator approves, but a deterministic test already failed.
        let e = FixedEvaluator {
            rec: EvaluatorRecommendation::Approve,
            security_hard: None,
        };
        let (_verdicts, report) = EvaluatorPool::standard().evaluate(
            &e,
            "cnd-1",
            &input(vec!["cargo test: 1 failed".into()]),
        );
        assert_eq!(report.verdict, Verdict::Rejected);
        assert!(report
            .hard_failures
            .iter()
            .any(|f| f.contains("cargo test")));
    }

    #[test]
    fn a_security_hard_failure_vetoes() {
        let e = FixedEvaluator {
            rec: EvaluatorRecommendation::Approve,
            security_hard: Some("writes a secret to disk".into()),
        };
        let (_v, report) = EvaluatorPool::standard().evaluate(&e, "cnd-1", &input(vec![]));
        assert_eq!(report.verdict, Verdict::Rejected);
        assert!(report
            .hard_failures
            .iter()
            .any(|f| f.starts_with("security")));
    }

    #[test]
    fn a_revise_without_hard_failure_needs_revision() {
        let e = FixedEvaluator {
            rec: EvaluatorRecommendation::Revise,
            security_hard: None,
        };
        let (_v, report) = EvaluatorPool::standard().evaluate(&e, "cnd-1", &input(vec![]));
        assert_eq!(report.verdict, Verdict::NeedsRevision);
    }
}
