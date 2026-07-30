//! The promotion gate.
//!
//! This is the only door to `Promoted`, and it is deliberately unfriendly. Its
//! contract:
//!
//! * **Fail closed.** Missing WARP, a missing stage report, or an `Inconclusive`
//!   verdict is a rejection, never a pass. Silence is not consent.
//! * **Hard vetoes are never averaged.** A security failure, permission
//!   expansion, audit tampering, validation bypass, critical regression, or
//!   secret exposure rejects the candidate regardless of how much it improved
//!   elsewhere. There is no score to trade against.
//! * **Nobody promotes their own work.** Tier 3 needs a human signature, and the
//!   signer may not be the candidate's author.
//! * **Configuration cannot unlock governance.** `[promotion]` can make the gate
//!   *stricter*. The Tier-3 human-approval and Tier-4 auto-reject requirements
//!   are enforced in code and ignore any config that tries to switch them off —
//!   including `/permissions full access`.

use crate::risk::RiskAssessment;
use crate::{ValidationReport, Verdict};
use nexus_core::governance::{self, CandidateFacts, GOVERNANCE_VERSION};
use nexus_core::harness::{ImprovementPlane, RiskTier};
use serde::{Deserialize, Serialize};

/// Stages that must all be present and `Passed` before promotion is considered.
pub const REQUIRED_STAGES: &[&str] = &["deterministic", "replay", "adversarial", "evaluators"];

/// Why a candidate was refused. Each variant is a veto: it cannot be outweighed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VetoKind {
    /// A governance rule fired.
    Governance,
    /// Tier 4 — prohibited autonomous change.
    ProhibitedTier,
    /// A security check or evaluator failed.
    SecurityFailure,
    /// The candidate widens permissions or reach.
    PermissionExpansion,
    /// The candidate touches the audit trail.
    AuditTampering,
    /// The candidate weakens, skips, or removes validation.
    ValidationBypass,
    /// A metric regressed past a hard constraint.
    CriticalRegression,
    /// A secret appeared where it must not.
    SecretExposure,
    /// A required stage is missing, inconclusive, or never ran.
    ValidationIncomplete,
    /// WARP itself was unavailable — fail closed.
    WarpUnavailable,
    /// Tier 3 without a human signature.
    MissingHumanApproval,
    /// The approver is the author.
    SelfAuthorization,
    /// A stage rejected the candidate for a reason none of the above name.
    StageFailure,
}

impl VetoKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Governance => "governance",
            Self::ProhibitedTier => "prohibited_tier",
            Self::SecurityFailure => "security_failure",
            Self::PermissionExpansion => "permission_expansion",
            Self::AuditTampering => "audit_tampering",
            Self::ValidationBypass => "validation_bypass",
            Self::CriticalRegression => "critical_regression",
            Self::SecretExposure => "secret_exposure",
            Self::ValidationIncomplete => "validation_incomplete",
            Self::WarpUnavailable => "warp_unavailable",
            Self::MissingHumanApproval => "missing_human_approval",
            Self::SelfAuthorization => "self_authorization",
            Self::StageFailure => "stage_failure",
        }
    }
}

/// One recorded veto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Veto {
    pub kind: VetoKind,
    pub detail: String,
    /// The stage or subsystem that produced it.
    pub source: String,
}

/// What the gate decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionDecision {
    /// All gates cleared for this tier — the candidate may be promoted.
    Promote,
    /// Must run in shadow before the question is asked again.
    RequireShadow,
    /// Needs a human signature from someone other than the author.
    RequireHumanApproval,
    /// Soft issues only; revise and resubmit.
    RequireRevision,
    /// A veto fired. Terminal for this candidate revision.
    Reject,
}

impl PromotionDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Promote => "promote",
            Self::RequireShadow => "require_shadow",
            Self::RequireHumanApproval => "require_human_approval",
            Self::RequireRevision => "require_revision",
            Self::Reject => "reject",
        }
    }
}

/// A human sign-off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanApproval {
    pub approver: String,
    pub approved_at: String,
    #[serde(default)]
    pub note: String,
}

/// The tunable part of promotion policy. Every field may only *tighten* the
/// built-in rules; see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PromotionPolicy {
    /// Tier 1 may promote itself once every gate passed.
    pub allow_tier_1_auto_promotion: bool,
    /// Tier 2 may promote after a clean shadow run without a human.
    pub allow_tier_2_auto_promotion_after_shadow: bool,
    /// Present for config parity and **always clamped to false** — Tier 3 needs
    /// a human. Setting it true is recorded and ignored.
    pub allow_tier_3_auto_promotion: bool,
    /// Tier 2 must be observed in shadow first.
    pub shadow_required_for_tier_2: bool,
}

impl Default for PromotionPolicy {
    fn default() -> Self {
        Self {
            allow_tier_1_auto_promotion: true,
            allow_tier_2_auto_promotion_after_shadow: false,
            allow_tier_3_auto_promotion: false,
            shadow_required_for_tier_2: true,
        }
    }
}

/// Everything the gate is allowed to consider. Note what is absent: the
/// candidate's own confidence, its author's argument, and any aggregate score.
#[derive(Debug, Clone, Copy)]
pub struct PromotionRequest<'a> {
    pub assessment: &'a RiskAssessment,
    /// Data-plane changes can be applied live; code-plane ships via release.
    pub plane: ImprovementPlane,
    pub created_by: &'a str,
    /// Reports from every stage that ran.
    pub reports: &'a [ValidationReport],
    /// False when WARP could not run at all.
    pub warp_available: bool,
    /// A clean shadow run has completed for this candidate revision.
    pub shadow_completed: bool,
    pub approval: Option<&'a HumanApproval>,
}

/// The gate's full, auditable answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionOutcome {
    pub candidate_id: String,
    pub decision: PromotionDecision,
    pub tier: RiskTier,
    pub governance_version: u32,
    pub vetoes: Vec<Veto>,
    pub rationale: Vec<String>,
    pub decided_at: String,
}

impl PromotionOutcome {
    pub fn promotes(&self) -> bool {
        self.decision == PromotionDecision::Promote
    }
}

/// Applies governance, risk tier, and validation evidence to a promotion request.
pub struct PromotionGate {
    policy: PromotionPolicy,
}

impl Default for PromotionGate {
    fn default() -> Self {
        Self::new(PromotionPolicy::default())
    }
}

impl PromotionGate {
    pub fn new(policy: PromotionPolicy) -> Self {
        Self { policy }
    }

    pub fn policy(&self) -> &PromotionPolicy {
        &self.policy
    }

    pub fn evaluate(&self, request: PromotionRequest<'_>) -> PromotionOutcome {
        let assessment = request.assessment;
        let mut outcome = PromotionOutcome {
            candidate_id: assessment.candidate_id.clone(),
            decision: PromotionDecision::Reject,
            tier: assessment.tier,
            governance_version: GOVERNANCE_VERSION,
            vetoes: Vec::new(),
            rationale: Vec::new(),
            decided_at: nexus_core::now_rfc3339(),
        };

        // 1. WARP must have been able to judge at all.
        if !request.warp_available {
            outcome.vetoes.push(Veto {
                kind: VetoKind::WarpUnavailable,
                detail: "WARP was unavailable; promotion fails closed".into(),
                source: "warp".into(),
            });
            outcome.rationale.push("fail-closed: no validator".into());
            return outcome;
        }

        // 2. Governance, then tier 4. Neither is negotiable.
        for violation in &assessment.governance.violations {
            outcome.vetoes.push(Veto {
                kind: VetoKind::Governance,
                detail: format!("{} [{}]", violation.describe(), violation.rule_id()),
                source: "governance".into(),
            });
        }
        if assessment.tier == RiskTier::Prohibited {
            outcome.vetoes.push(Veto {
                kind: VetoKind::ProhibitedTier,
                detail: "tier 4 candidates are auto-rejected".into(),
                source: "risk_classifier".into(),
            });
        }
        if !outcome.vetoes.is_empty() {
            outcome.rationale.push("governance veto".into());
            return outcome;
        }

        // 3. Every required stage must be present and Passed.
        let mut needs_revision = false;
        for stage in REQUIRED_STAGES {
            match request.reports.iter().find(|r| r.stage == *stage) {
                None => outcome.vetoes.push(Veto {
                    kind: VetoKind::ValidationIncomplete,
                    detail: format!("stage `{stage}` never ran"),
                    source: (*stage).into(),
                }),
                Some(report) => match report.verdict {
                    Verdict::Passed => {}
                    Verdict::Inconclusive => outcome.vetoes.push(Veto {
                        kind: VetoKind::ValidationIncomplete,
                        detail: format!("stage `{stage}` was inconclusive"),
                        source: (*stage).into(),
                    }),
                    Verdict::NeedsRevision => needs_revision = true,
                    Verdict::Rejected => {
                        if report.hard_failures.is_empty() {
                            outcome.vetoes.push(Veto {
                                kind: VetoKind::StageFailure,
                                detail: format!("stage `{stage}` rejected the candidate"),
                                source: (*stage).into(),
                            });
                        }
                    }
                },
            }
        }

        // 4. Hard failures from any stage — including stages beyond the required
        //    set — are vetoes on their own, whatever that stage's verdict said.
        for report in request.reports {
            for failure in &report.hard_failures {
                outcome.vetoes.push(Veto {
                    kind: classify_failure(failure),
                    detail: failure.clone(),
                    source: report.stage.clone(),
                });
            }
        }

        if !outcome.vetoes.is_empty() {
            outcome
                .rationale
                .push(format!("{} hard veto(es)", outcome.vetoes.len()));
            return outcome;
        }
        if needs_revision {
            outcome.decision = PromotionDecision::RequireRevision;
            outcome
                .rationale
                .push("soft issues raised; revise and resubmit".into());
            return outcome;
        }

        // 5. Self-authorization is a governance question, asked again now that we
        //    know who signed.
        if let Some(approval) = request.approval {
            let review = governance::review(CandidateFacts {
                candidate_id: &assessment.candidate_id,
                affected_components: &[],
                required_permissions: &[],
                narrative: "",
                created_by: request.created_by,
                approved_by: Some(&approval.approver),
            });
            if !review.permits() {
                for violation in &review.violations {
                    outcome.vetoes.push(Veto {
                        kind: VetoKind::SelfAuthorization,
                        detail: violation.describe(),
                        source: "governance".into(),
                    });
                }
                outcome
                    .rationale
                    .push("approver may not be the author".into());
                return outcome;
            }
        }

        // 6. Tier gates. Everything above passed, so this is the only remaining
        //    question: is this tier allowed to promote in this situation?
        outcome
            .rationale
            .extend(assessment.rationale.iter().cloned());
        let shadow_needed = self.policy.shadow_required_for_tier_2
            && assessment.requires_shadow()
            && request.plane == ImprovementPlane::Data;

        outcome.decision = match assessment.tier {
            RiskTier::Observation => {
                outcome
                    .rationale
                    .push("tier 0: observation-only, auto-promoted".into());
                PromotionDecision::Promote
            }
            RiskTier::Low => {
                if self.policy.allow_tier_1_auto_promotion {
                    outcome
                        .rationale
                        .push("tier 1: all gates passed, auto-promotion enabled".into());
                    PromotionDecision::Promote
                } else {
                    outcome
                        .rationale
                        .push("tier 1: auto-promotion disabled by policy".into());
                    PromotionDecision::RequireHumanApproval
                }
            }
            RiskTier::Moderate => {
                if shadow_needed && !request.shadow_completed {
                    outcome
                        .rationale
                        .push("tier 2: shadow run required before promotion".into());
                    PromotionDecision::RequireShadow
                } else if self.policy.allow_tier_2_auto_promotion_after_shadow {
                    outcome
                        .rationale
                        .push("tier 2: clean shadow run, auto-promotion enabled".into());
                    PromotionDecision::Promote
                } else {
                    outcome
                        .rationale
                        .push("tier 2: shadow clean, human approval still required".into());
                    self.tier_gate_with_approval(request, &mut outcome)
                }
            }
            RiskTier::High => {
                if self.policy.allow_tier_3_auto_promotion {
                    outcome.rationale.push(
                        "tier 3: allow_tier_3_auto_promotion is set and ignored — governance \
                         requires a human"
                            .into(),
                    );
                }
                if shadow_needed && !request.shadow_completed {
                    outcome
                        .rationale
                        .push("tier 3: shadow run required before approval".into());
                    PromotionDecision::RequireShadow
                } else {
                    if request.plane == ImprovementPlane::Code {
                        outcome.rationale.push(
                            "code-plane: promotion means a validated branch for human review, \
                             never a live swap"
                                .into(),
                        );
                    }
                    self.tier_gate_with_approval(request, &mut outcome)
                }
            }
            // Handled in step 2; unreachable in practice.
            RiskTier::Prohibited => PromotionDecision::Reject,
        };
        outcome
    }

    /// Promote only with a human signature; otherwise ask for one.
    fn tier_gate_with_approval(
        &self,
        request: PromotionRequest<'_>,
        outcome: &mut PromotionOutcome,
    ) -> PromotionDecision {
        match request.approval {
            Some(approval) => {
                outcome
                    .rationale
                    .push(format!("approved by `{}`", approval.approver));
                PromotionDecision::Promote
            }
            None => {
                outcome.vetoes.push(Veto {
                    kind: VetoKind::MissingHumanApproval,
                    detail: "no human approval recorded".into(),
                    source: "promotion_gate".into(),
                });
                PromotionDecision::RequireHumanApproval
            }
        }
    }
}

/// Map a hard-failure message onto the veto it represents. Unmatched failures
/// stay vetoes — the fallback is `StageFailure`, never "ignore".
fn classify_failure(failure: &str) -> VetoKind {
    let f = failure.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| f.contains(w));
    if has(&["secret", "api key", "credential leak", "token leaked"]) {
        VetoKind::SecretExposure
    } else if has(&["audit"]) {
        VetoKind::AuditTampering
    } else if has(&[
        "validation",
        "skipped test",
        "skip test",
        "disabled test",
        "bypass",
    ]) {
        VetoKind::ValidationBypass
    } else if has(&["permission", "escalat", "privilege"]) {
        VetoKind::PermissionExpansion
    } else if has(&["security", "injection", "sandbox"]) {
        VetoKind::SecurityFailure
    } else if has(&["regress", "success rate", "must not decrease"]) {
        VetoKind::CriticalRegression
    } else {
        VetoKind::StageFailure
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::risk::RiskClassifier;
    use nexus_core::harness::{
        ImprovementCategory, ImprovementProposal, ImprovementTarget, SuccessMetric,
    };

    fn proposal(target: ImprovementTarget) -> ImprovementProposal {
        let mut p = ImprovementProposal::new(
            ImprovementCategory::Tool,
            "repeated failures",
            "route around the failing tool",
        )
        .expect("proposal");
        p.target = target;
        p.risk_tier = nexus_core::harness::RiskTier::Low;
        p.created_by = "improvement_planner".into();
        p.success_metrics = vec![SuccessMetric {
            id: "task_success_must_not_decrease".into(),
            description: "no regression".into(),
            baseline: None,
            target: None,
            hard_constraint: true,
        }];
        p
    }

    fn passing_reports(candidate_id: &str) -> Vec<ValidationReport> {
        REQUIRED_STAGES
            .iter()
            .map(|stage| {
                let mut r = ValidationReport::new(candidate_id, *stage);
                r.verdict = Verdict::Passed;
                r
            })
            .collect()
    }

    fn request<'a>(
        assessment: &'a RiskAssessment,
        reports: &'a [ValidationReport],
    ) -> PromotionRequest<'a> {
        PromotionRequest {
            assessment,
            plane: ImprovementPlane::Data,
            created_by: "improvement_planner",
            reports,
            warp_available: true,
            shadow_completed: false,
            approval: None,
        }
    }

    #[test]
    fn tier_one_auto_promotes_after_every_gate_passes() {
        let p = proposal(ImprovementTarget::Memory);
        let a = RiskClassifier::classify(&p);
        let reports = passing_reports(&p.id);
        let outcome = PromotionGate::default().evaluate(request(&a, &reports));
        assert_eq!(outcome.decision, PromotionDecision::Promote);
        assert!(outcome.vetoes.is_empty());
    }

    #[test]
    fn tier_two_requires_shadow_then_a_human() {
        let p = proposal(ImprovementTarget::ToolRouter);
        let a = RiskClassifier::classify(&p);
        let reports = passing_reports(&p.id);
        let gate = PromotionGate::default();

        let first = gate.evaluate(request(&a, &reports));
        assert_eq!(first.decision, PromotionDecision::RequireShadow);

        let mut req = request(&a, &reports);
        req.shadow_completed = true;
        let second = gate.evaluate(req);
        assert_eq!(second.decision, PromotionDecision::RequireHumanApproval);

        let approval = HumanApproval {
            approver: "human:sans".into(),
            approved_at: nexus_core::now_rfc3339(),
            note: String::new(),
        };
        req.approval = Some(&approval);
        assert_eq!(gate.evaluate(req).decision, PromotionDecision::Promote);
    }

    #[test]
    fn tier_two_auto_promotes_after_shadow_only_when_configured() {
        let p = proposal(ImprovementTarget::ToolRouter);
        let a = RiskClassifier::classify(&p);
        let reports = passing_reports(&p.id);
        let gate = PromotionGate::new(PromotionPolicy {
            allow_tier_2_auto_promotion_after_shadow: true,
            ..PromotionPolicy::default()
        });
        let mut req = request(&a, &reports);
        req.shadow_completed = true;
        assert_eq!(gate.evaluate(req).decision, PromotionDecision::Promote);
    }

    #[test]
    fn tier_three_auto_promotion_config_cannot_bypass_a_human() {
        let p = proposal(ImprovementTarget::HarnessComponent);
        let a = RiskClassifier::classify(&p);
        assert_eq!(a.tier, RiskTier::High);
        let reports = passing_reports(&p.id);
        // A config that tries to unlock tier 3 — plus code-plane, which skips the
        // shadow requirement. The human requirement must still stand.
        let gate = PromotionGate::new(PromotionPolicy {
            allow_tier_3_auto_promotion: true,
            ..PromotionPolicy::default()
        });
        let mut req = request(&a, &reports);
        req.plane = ImprovementPlane::Code;
        let outcome = gate.evaluate(req);
        assert_eq!(outcome.decision, PromotionDecision::RequireHumanApproval);
        assert!(outcome
            .vetoes
            .iter()
            .any(|v| v.kind == VetoKind::MissingHumanApproval));
        assert!(outcome.rationale.iter().any(|r| r.contains("ignored")));
    }

    #[test]
    fn tier_four_is_auto_rejected() {
        let mut p = proposal(ImprovementTarget::HarnessComponent);
        p.affected_components = vec!["crates/nexus-core/src/governance.rs".into()];
        let a = RiskClassifier::classify(&p);
        let reports = passing_reports(&p.id);
        let outcome = PromotionGate::default().evaluate(request(&a, &reports));
        assert_eq!(outcome.decision, PromotionDecision::Reject);
        assert!(outcome
            .vetoes
            .iter()
            .any(|v| v.kind == VetoKind::ProhibitedTier));
        assert!(outcome
            .vetoes
            .iter()
            .any(|v| v.kind == VetoKind::Governance));
    }

    #[test]
    fn a_security_failure_cannot_be_averaged_away_by_wins_elsewhere() {
        let p = proposal(ImprovementTarget::Memory);
        let a = RiskClassifier::classify(&p);
        let mut reports = passing_reports(&p.id);
        // Every stage passed except one security hard failure buried in the
        // adversarial stage — which itself even reports Passed.
        reports[2]
            .hard_failures
            .push("security: prompt injection was executed".into());
        let outcome = PromotionGate::default().evaluate(request(&a, &reports));
        assert_eq!(outcome.decision, PromotionDecision::Reject);
        assert_eq!(outcome.vetoes[0].kind, VetoKind::SecurityFailure);
    }

    #[test]
    fn each_veto_class_is_recognised() {
        let cases = [
            ("wrote a secret to the transcript", VetoKind::SecretExposure),
            ("audit row was updated", VetoKind::AuditTampering),
            (
                "skipped test suite to cut latency",
                VetoKind::ValidationBypass,
            ),
            (
                "requested permission escalation",
                VetoKind::PermissionExpansion,
            ),
            ("sandbox escape observed", VetoKind::SecurityFailure),
            (
                "task success rate regressed 6%",
                VetoKind::CriticalRegression,
            ),
            ("cargo build: exit 101", VetoKind::StageFailure),
        ];
        for (text, expected) in cases {
            assert_eq!(classify_failure(text), expected, "for {text}");
        }
    }

    #[test]
    fn a_missing_stage_fails_closed() {
        let p = proposal(ImprovementTarget::Memory);
        let a = RiskClassifier::classify(&p);
        let mut reports = passing_reports(&p.id);
        reports.retain(|r| r.stage != "replay");
        let outcome = PromotionGate::default().evaluate(request(&a, &reports));
        assert_eq!(outcome.decision, PromotionDecision::Reject);
        assert!(outcome
            .vetoes
            .iter()
            .any(|v| v.kind == VetoKind::ValidationIncomplete && v.detail.contains("replay")));
    }

    #[test]
    fn an_inconclusive_stage_is_not_a_pass() {
        let p = proposal(ImprovementTarget::Memory);
        let a = RiskClassifier::classify(&p);
        let mut reports = passing_reports(&p.id);
        reports[0].verdict = Verdict::Inconclusive;
        let outcome = PromotionGate::default().evaluate(request(&a, &reports));
        assert_eq!(outcome.decision, PromotionDecision::Reject);
    }

    #[test]
    fn warp_unavailable_fails_closed() {
        let p = proposal(ImprovementTarget::Memory);
        let a = RiskClassifier::classify(&p);
        let reports = passing_reports(&p.id);
        let mut req = request(&a, &reports);
        req.warp_available = false;
        let outcome = PromotionGate::default().evaluate(req);
        assert_eq!(outcome.decision, PromotionDecision::Reject);
        assert_eq!(outcome.vetoes[0].kind, VetoKind::WarpUnavailable);
    }

    #[test]
    fn the_author_cannot_sign_off_on_its_own_candidate() {
        let p = proposal(ImprovementTarget::ToolRouter);
        let a = RiskClassifier::classify(&p);
        let reports = passing_reports(&p.id);
        let approval = HumanApproval {
            approver: "improvement_planner".into(),
            approved_at: nexus_core::now_rfc3339(),
            note: String::new(),
        };
        let mut req = request(&a, &reports);
        req.shadow_completed = true;
        req.approval = Some(&approval);
        let outcome = PromotionGate::default().evaluate(req);
        assert_eq!(outcome.decision, PromotionDecision::Reject);
        assert_eq!(outcome.vetoes[0].kind, VetoKind::SelfAuthorization);
    }

    #[test]
    fn soft_issues_ask_for_a_revision_rather_than_rejecting() {
        let p = proposal(ImprovementTarget::Memory);
        let a = RiskClassifier::classify(&p);
        let mut reports = passing_reports(&p.id);
        reports[3].verdict = Verdict::NeedsRevision;
        reports[3]
            .soft_failures
            .push("usability: unclear name".into());
        let outcome = PromotionGate::default().evaluate(request(&a, &reports));
        assert_eq!(outcome.decision, PromotionDecision::RequireRevision);
        assert!(outcome.vetoes.is_empty());
    }
}
