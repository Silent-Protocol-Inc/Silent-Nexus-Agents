//! Requirement compilation.
//!
//! A candidate states its goals as [`SuccessMetric`]s. WARP compiles those into
//! machine-checkable requirements and, crucially, separates the **hard
//! constraints** (vetoes such as "task success must not decrease") from soft
//! targets. Nothing downstream may trade a hard constraint away for a soft gain.

use nexus_core::harness::{ImprovementProposal, SuccessMetric};
use serde::{Deserialize, Serialize};

/// One compiled criterion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledRequirement {
    pub id: String,
    pub description: String,
    pub baseline: Option<f64>,
    pub target: Option<f64>,
    /// A hard constraint is a veto — it cannot be averaged away.
    pub hard: bool,
}

impl From<&SuccessMetric> for CompiledRequirement {
    fn from(metric: &SuccessMetric) -> Self {
        Self {
            id: metric.id.clone(),
            description: metric.description.clone(),
            baseline: metric.baseline,
            target: metric.target,
            hard: metric.hard_constraint,
        }
    }
}

/// The compiled requirements for a candidate, hard constraints kept distinct.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RequirementSet {
    pub hard: Vec<CompiledRequirement>,
    pub soft: Vec<CompiledRequirement>,
}

impl RequirementSet {
    pub fn is_empty(&self) -> bool {
        self.hard.is_empty() && self.soft.is_empty()
    }
}

/// Compiles a candidate's success metrics into a [`RequirementSet`].
pub struct RequirementCompiler;

impl RequirementCompiler {
    /// Compile from a proposal. A hard non-regression guard is always present:
    /// if the candidate did not declare one, WARP injects it, so no candidate can
    /// escape the "task success must not decrease" veto by omitting it.
    pub fn compile(proposal: &ImprovementProposal) -> RequirementSet {
        let mut set = RequirementSet::default();
        for metric in &proposal.success_metrics {
            let requirement = CompiledRequirement::from(metric);
            if requirement.hard {
                set.hard.push(requirement);
            } else {
                set.soft.push(requirement);
            }
        }
        if !set
            .hard
            .iter()
            .any(|r| r.id == "task_success_must_not_decrease")
        {
            set.hard.push(CompiledRequirement {
                id: "task_success_must_not_decrease".into(),
                description: "overall task success rate must not fall (injected by WARP)".into(),
                baseline: None,
                target: None,
                hard: true,
            });
        }
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::harness::{ImprovementCategory, ImprovementProposal};

    fn proposal(metrics: Vec<SuccessMetric>) -> ImprovementProposal {
        let mut p =
            ImprovementProposal::new(ImprovementCategory::Tool, "p", "c").expect("proposal");
        p.success_metrics = metrics;
        p
    }

    #[test]
    fn hard_and_soft_constraints_are_separated() {
        let set = RequirementCompiler::compile(&proposal(vec![
            SuccessMetric {
                id: "latency".into(),
                description: "faster".into(),
                baseline: Some(100.0),
                target: Some(80.0),
                hard_constraint: false,
            },
            SuccessMetric {
                id: "no_secret_exposure".into(),
                description: "never leak".into(),
                baseline: None,
                target: None,
                hard_constraint: true,
            },
        ]));
        assert!(set.soft.iter().any(|r| r.id == "latency"));
        assert!(set.hard.iter().any(|r| r.id == "no_secret_exposure"));
    }

    #[test]
    fn non_regression_guard_is_always_injected() {
        // A candidate that declared no metrics still gets the veto.
        let set = RequirementCompiler::compile(&proposal(vec![]));
        assert!(set
            .hard
            .iter()
            .any(|r| r.id == "task_success_must_not_decrease"));
    }

    #[test]
    fn declared_guard_is_not_duplicated() {
        let set = RequirementCompiler::compile(&proposal(vec![SuccessMetric {
            id: "task_success_must_not_decrease".into(),
            description: "declared".into(),
            baseline: None,
            target: None,
            hard_constraint: true,
        }]));
        assert_eq!(
            set.hard
                .iter()
                .filter(|r| r.id == "task_success_must_not_decrease")
                .count(),
            1
        );
    }
}
