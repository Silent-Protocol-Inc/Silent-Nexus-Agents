//! WARP — Watch, Assess, Replay, Promote.
//!
//! WARP is the independent validation shell around Nexus RSI. A candidate's own
//! author-context never decides its fate here: deterministic checks run in an
//! isolated environment, and an objective failure is a **hard veto** that no
//! model verdict can average away. This crate depends only on `nexus-core`, so
//! it cannot be reached or edited by the RSI candidate pipeline it judges.
//!
//! P5 delivers the deterministic core: candidate isolation, requirement
//! compilation, and the deterministic validator (including the code-plane
//! build/test/lint/schema gate). Replay, adversarial suites, independent
//! evaluators, risk/promotion, shadow, canary, and rollback build on top.

pub mod adversarial;
pub mod deterministic;
pub mod evaluators;
pub mod isolation;
pub mod promotion;
pub mod replay;
pub mod requirements;
pub mod risk;
pub mod shadow;

pub use adversarial::{
    builtin_catalog, AdversarialCategory, AdversarialReport, AdversarialScenario, AdversarialSuite,
    Expectation, ScenarioOutcome, ScenarioRunner,
};
pub use deterministic::{
    Check, CheckKind, CheckOutcome, CheckRunner, DeterministicValidator, ProcessCheckRunner,
};
pub use evaluators::{
    Evaluator, EvaluatorInput, EvaluatorPool, EvaluatorRecommendation, EvaluatorRole,
    EvaluatorVerdict,
};
pub use isolation::{Isolate, IsolationProvider, OverlayIsolation, WorktreeIsolation};
pub use promotion::{
    HumanApproval, PromotionDecision, PromotionGate, PromotionOutcome, PromotionPolicy,
    PromotionRequest, Veto, VetoKind, REQUIRED_STAGES,
};
pub use replay::{
    load_fixtures, load_fixtures_strict, MetricDelta, ReplayEngine, ReplayOutcome, ReplayReport,
    ReplayRunner, TaskFixture,
};
pub use requirements::{CompiledRequirement, RequirementCompiler, RequirementSet};
pub use risk::{RiskAssessment, RiskClassifier};
pub use shadow::{
    Divergence, EffectDisposition, EffectFirewall, EffectRecord, ShadowExecutor, ShadowObservation,
    ShadowReport, ShadowRunner, ShadowTask,
};

use serde::{Deserialize, Serialize};

/// The result of a WARP validation stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Every gate passed.
    Passed,
    /// A hard veto fired — the candidate cannot advance.
    Rejected,
    /// Only soft issues; the candidate may be revised and re-submitted.
    NeedsRevision,
    /// Validation could not run to completion (e.g. isolation failed). Treated
    /// as fail-closed by the promotion gate — never as a pass.
    Inconclusive,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Rejected => "rejected",
            Self::NeedsRevision => "needs_revision",
            Self::Inconclusive => "inconclusive",
        }
    }

    /// Only `Passed` allows a candidate to advance.
    pub fn advances(self) -> bool {
        matches!(self, Self::Passed)
    }
}

/// A structured validation report for one stage of one candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub candidate_id: String,
    pub stage: String,
    pub verdict: Verdict,
    pub checks: Vec<CheckOutcome>,
    /// Objective failures that veto promotion, most-important first.
    pub hard_failures: Vec<String>,
    /// Advisory issues that suggest revision but are not vetoes.
    pub soft_failures: Vec<String>,
    pub created_at: String,
}

impl ValidationReport {
    pub fn new(candidate_id: impl Into<String>, stage: impl Into<String>) -> Self {
        Self {
            candidate_id: candidate_id.into(),
            stage: stage.into(),
            verdict: Verdict::Inconclusive,
            checks: Vec::new(),
            hard_failures: Vec::new(),
            soft_failures: Vec::new(),
            created_at: nexus_core::now_rfc3339(),
        }
    }
}
