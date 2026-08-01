//! Shadow execution.
//!
//! A shadow run puts the candidate on a *real* task, next to the baseline, and
//! throws its side effects away. The candidate sees real inputs; the world sees
//! nothing. Two mechanisms make that true:
//!
//! * **The effect firewall.** Every tool call is classified by
//!   [`nexus_core::risk::RiskLevel`]. Only `Read` executes. Writes, deletes,
//!   network calls, privileged operations, and anything with an external side
//!   effect are *intercepted*: recorded as intent, never run. A runner that
//!   reports having executed an intercepted effect is a **containment breach**
//!   and a hard veto — the candidate is rejected, not merely noted.
//! * **Divergence measurement, not scoring.** The report keeps agreement rate,
//!   quality/token/latency deltas, and the divergences themselves as separate
//!   numbers. Nothing here collapses into a single figure a candidate could
//!   optimise against, and a success-rate regression is a veto regardless of how
//!   much faster or cheaper the candidate was.

use crate::{ValidationReport, Verdict};
use nexus_core::risk::RiskLevel;
use serde::{Deserialize, Serialize};

/// What the firewall did with an attempted effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectDisposition {
    /// Read-only: allowed to execute against real state.
    Executed,
    /// Mutating or outward-facing: recorded as intent, never run.
    Intercepted,
}

/// One tool call a run attempted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectRecord {
    pub tool: String,
    pub risk: RiskLevel,
    /// What the firewall decided.
    pub disposition: EffectDisposition,
    /// Set by the runner when the effect actually ran. For an `Intercepted`
    /// effect this must be false; true means containment failed.
    #[serde(default)]
    pub executed: bool,
    #[serde(default)]
    pub detail: String,
}

/// Decides which effects a shadow run may actually perform.
pub struct EffectFirewall;

impl EffectFirewall {
    /// Only reads survive contact with the real world.
    pub fn disposition(risk: RiskLevel) -> EffectDisposition {
        match risk {
            RiskLevel::Read => EffectDisposition::Executed,
            RiskLevel::Network
            | RiskLevel::Write
            | RiskLevel::Destructive
            | RiskLevel::Privileged
            | RiskLevel::ExternalSideEffect => EffectDisposition::Intercepted,
        }
    }

    /// Classify an attempted call before a runner performs it.
    pub fn intercept(tool: impl Into<String>, risk: RiskLevel) -> EffectRecord {
        EffectRecord {
            tool: tool.into(),
            risk,
            disposition: Self::disposition(risk),
            executed: false,
            detail: String::new(),
        }
    }
}

/// One task a shadow run is measured on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowTask {
    pub id: String,
    /// Redacted description of what the user asked for.
    pub objective: String,
}

/// What one arm (baseline or candidate) did on one task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShadowObservation {
    pub task_id: String,
    pub succeeded: bool,
    /// Redacted final output, used only for agreement comparison.
    pub output: String,
    /// Independent quality score in `[0,1]` from the objective stages — not the
    /// candidate's own opinion of itself.
    pub quality: f64,
    pub tokens: u64,
    pub latency_ms: u64,
    pub effects: Vec<EffectRecord>,
}

impl ShadowObservation {
    pub fn new(task_id: impl Into<String>, succeeded: bool) -> Self {
        Self {
            task_id: task_id.into(),
            succeeded,
            output: String::new(),
            quality: 0.0,
            tokens: 0,
            latency_ms: 0,
            effects: Vec::new(),
        }
    }
}

/// Runs one arm of a shadow comparison. Implementations drive a real session;
/// the trait exists so the divergence and containment policy below is tested
/// without a live model.
pub trait ShadowRunner {
    fn run_baseline(&self, task: &ShadowTask) -> ShadowObservation;
    fn run_candidate(&self, task: &ShadowTask) -> ShadowObservation;
}

/// One task where the two arms disagreed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Divergence {
    pub task_id: String,
    pub baseline_output: String,
    pub candidate_output: String,
    /// True when the candidate failed a task the baseline completed.
    pub candidate_regressed: bool,
}

/// The measured result of a shadow run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShadowReport {
    pub candidate_id: String,
    pub tasks: usize,
    /// Fraction of tasks where both arms produced the same output.
    pub agreement_rate: f64,
    pub baseline_success_rate: f64,
    pub candidate_success_rate: f64,
    pub quality_delta: f64,
    pub token_delta: i64,
    pub latency_delta_ms: i64,
    /// Effects the firewall stopped — expected, and informative.
    pub intercepted_effects: Vec<EffectRecord>,
    /// Effects that ran despite interception. Must always be empty.
    pub containment_breaches: Vec<EffectRecord>,
    pub divergences: Vec<Divergence>,
}

/// Executes and compares shadow runs.
pub struct ShadowExecutor {
    /// A candidate may not fail more tasks than the baseline.
    max_success_regression: f64,
}

impl Default for ShadowExecutor {
    fn default() -> Self {
        Self {
            max_success_regression: 0.0,
        }
    }
}

impl ShadowExecutor {
    pub fn new(max_success_regression: f64) -> Self {
        Self {
            max_success_regression,
        }
    }

    /// Run every task through both arms and produce the report plus a WARP
    /// validation verdict for the `shadow` stage.
    pub fn run(
        &self,
        runner: &impl ShadowRunner,
        candidate_id: &str,
        tasks: &[ShadowTask],
    ) -> (ShadowReport, ValidationReport) {
        let mut report = ShadowReport {
            candidate_id: candidate_id.to_string(),
            tasks: tasks.len(),
            agreement_rate: 0.0,
            baseline_success_rate: 0.0,
            candidate_success_rate: 0.0,
            quality_delta: 0.0,
            token_delta: 0,
            latency_delta_ms: 0,
            intercepted_effects: Vec::new(),
            containment_breaches: Vec::new(),
            divergences: Vec::new(),
        };
        let mut validation = ValidationReport::new(candidate_id, "shadow");

        if tasks.is_empty() {
            validation.verdict = Verdict::Rejected;
            validation
                .hard_failures
                .push("shadow: no tasks to observe — cannot claim a clean run".into());
            return (report, validation);
        }

        let (mut agreements, mut baseline_ok, mut candidate_ok) = (0usize, 0usize, 0usize);
        let (mut baseline_quality, mut candidate_quality) = (0.0f64, 0.0f64);
        let (mut baseline_tokens, mut candidate_tokens) = (0i64, 0i64);
        let (mut baseline_latency, mut candidate_latency) = (0i64, 0i64);

        for task in tasks {
            let base = runner.run_baseline(task);
            let cand = runner.run_candidate(task);

            baseline_ok += usize::from(base.succeeded);
            candidate_ok += usize::from(cand.succeeded);
            baseline_quality += base.quality;
            candidate_quality += cand.quality;
            baseline_tokens += base.tokens as i64;
            candidate_tokens += cand.tokens as i64;
            baseline_latency += base.latency_ms as i64;
            candidate_latency += cand.latency_ms as i64;

            for effect in &cand.effects {
                if effect.disposition == EffectDisposition::Intercepted {
                    if effect.executed {
                        report.containment_breaches.push(effect.clone());
                    } else {
                        report.intercepted_effects.push(effect.clone());
                    }
                }
            }

            if base.output == cand.output {
                agreements += 1;
            } else {
                report.divergences.push(Divergence {
                    task_id: task.id.clone(),
                    baseline_output: base.output.clone(),
                    candidate_output: cand.output.clone(),
                    candidate_regressed: base.succeeded && !cand.succeeded,
                });
            }
        }

        let n = tasks.len() as f64;
        report.agreement_rate = agreements as f64 / n;
        report.baseline_success_rate = baseline_ok as f64 / n;
        report.candidate_success_rate = candidate_ok as f64 / n;
        report.quality_delta = (candidate_quality - baseline_quality) / n;
        report.token_delta = candidate_tokens - baseline_tokens;
        report.latency_delta_ms = candidate_latency - baseline_latency;

        // Containment first: a shadow run that touched the world is not a
        // shadow run, whatever it measured.
        for breach in &report.containment_breaches {
            validation.hard_failures.push(format!(
                "shadow containment breach: `{}` ({}) executed a {} effect",
                breach.tool, breach.detail, breach.risk
            ));
        }

        let regression = report.baseline_success_rate - report.candidate_success_rate;
        if regression > self.max_success_regression {
            validation.hard_failures.push(format!(
                "task success regressed {:.1}% in shadow ({:.2} → {:.2})",
                regression * 100.0,
                report.baseline_success_rate,
                report.candidate_success_rate
            ));
        }

        for divergence in report.divergences.iter().filter(|d| d.candidate_regressed) {
            validation.hard_failures.push(format!(
                "task `{}` succeeded on baseline and failed on candidate",
                divergence.task_id
            ));
        }

        if report.quality_delta < 0.0 {
            validation.soft_failures.push(format!(
                "average quality fell {:.3} against baseline",
                -report.quality_delta
            ));
        }
        if report.token_delta > 0 {
            validation.soft_failures.push(format!(
                "spends {} more tokens than baseline",
                report.token_delta
            ));
        }

        validation.verdict = if !validation.hard_failures.is_empty() {
            Verdict::Rejected
        } else if !validation.soft_failures.is_empty() {
            Verdict::NeedsRevision
        } else {
            Verdict::Passed
        };
        (report, validation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tasks(n: usize) -> Vec<ShadowTask> {
        (0..n)
            .map(|i| ShadowTask {
                id: format!("t{i}"),
                objective: "summarise the changed files".into(),
            })
            .collect()
    }

    /// Baseline always succeeds; the candidate is configured per test.
    struct Runner {
        candidate_fails: Vec<String>,
        candidate_output: String,
        candidate_tokens: u64,
        candidate_quality: f64,
        candidate_effects: Vec<EffectRecord>,
    }

    impl Default for Runner {
        fn default() -> Self {
            Self {
                candidate_fails: Vec::new(),
                candidate_output: "answer".into(),
                candidate_tokens: 100,
                candidate_quality: 0.8,
                candidate_effects: Vec::new(),
            }
        }
    }

    impl ShadowRunner for Runner {
        fn run_baseline(&self, task: &ShadowTask) -> ShadowObservation {
            let mut o = ShadowObservation::new(&task.id, true);
            o.output = "answer".into();
            o.quality = 0.8;
            o.tokens = 100;
            o.latency_ms = 1000;
            o
        }

        fn run_candidate(&self, task: &ShadowTask) -> ShadowObservation {
            let failed = self.candidate_fails.contains(&task.id);
            let mut o = ShadowObservation::new(&task.id, !failed);
            o.output = self.candidate_output.clone();
            o.quality = self.candidate_quality;
            o.tokens = self.candidate_tokens;
            o.latency_ms = 900;
            o.effects = self.candidate_effects.clone();
            o
        }
    }

    #[test]
    fn the_firewall_executes_reads_and_intercepts_everything_else() {
        assert_eq!(
            EffectFirewall::disposition(RiskLevel::Read),
            EffectDisposition::Executed
        );
        for risk in [
            RiskLevel::Network,
            RiskLevel::Write,
            RiskLevel::Destructive,
            RiskLevel::Privileged,
            RiskLevel::ExternalSideEffect,
        ] {
            assert_eq!(
                EffectFirewall::disposition(risk),
                EffectDisposition::Intercepted,
                "{risk} must not run in shadow"
            );
        }
    }

    #[test]
    fn an_agreeing_candidate_passes_and_records_no_effects() {
        let (report, validation) =
            ShadowExecutor::default().run(&Runner::default(), "cnd-1", &tasks(5));
        assert_eq!(validation.verdict, Verdict::Passed);
        assert_eq!(report.agreement_rate, 1.0);
        assert!(report.divergences.is_empty());
        assert!(report.containment_breaches.is_empty());
    }

    #[test]
    fn intercepted_writes_are_recorded_but_never_executed() {
        let runner = Runner {
            candidate_effects: vec![EffectFirewall::intercept("write_file", RiskLevel::Write)],
            ..Runner::default()
        };
        let (report, validation) = ShadowExecutor::default().run(&runner, "cnd-1", &tasks(3));
        assert_eq!(report.intercepted_effects.len(), 3);
        assert!(report.intercepted_effects.iter().all(|e| !e.executed));
        assert!(report.containment_breaches.is_empty());
        assert_eq!(validation.verdict, Verdict::Passed);
    }

    #[test]
    fn an_effect_that_escaped_containment_is_a_hard_veto() {
        let mut escaped = EffectFirewall::intercept("git_push", RiskLevel::ExternalSideEffect);
        escaped.executed = true;
        escaped.detail = "pushed to origin".into();
        let runner = Runner {
            candidate_effects: vec![escaped],
            ..Runner::default()
        };
        let (report, validation) = ShadowExecutor::default().run(&runner, "cnd-1", &tasks(2));
        assert_eq!(report.containment_breaches.len(), 2);
        assert_eq!(validation.verdict, Verdict::Rejected);
        assert!(validation.hard_failures[0].contains("containment breach"));
    }

    #[test]
    fn a_success_regression_is_a_veto_even_when_cheaper_and_faster() {
        let runner = Runner {
            candidate_fails: vec!["t0".into(), "t1".into()],
            candidate_tokens: 10, // much cheaper
            ..Runner::default()
        };
        let (report, validation) = ShadowExecutor::default().run(&runner, "cnd-1", &tasks(4));
        assert!(report.token_delta < 0, "candidate really is cheaper");
        assert!(report.latency_delta_ms < 0, "and faster");
        assert_eq!(validation.verdict, Verdict::Rejected);
        assert!(validation
            .hard_failures
            .iter()
            .any(|f| f.contains("success regressed")));
    }

    #[test]
    fn divergent_output_without_regression_is_measured_not_vetoed() {
        let runner = Runner {
            candidate_output: "a differently worded answer".into(),
            ..Runner::default()
        };
        let (report, validation) = ShadowExecutor::default().run(&runner, "cnd-1", &tasks(4));
        assert_eq!(report.agreement_rate, 0.0);
        assert_eq!(report.divergences.len(), 4);
        assert!(report.divergences.iter().all(|d| !d.candidate_regressed));
        assert_eq!(validation.verdict, Verdict::Passed);
    }

    #[test]
    fn lower_quality_asks_for_revision_rather_than_rejecting() {
        let runner = Runner {
            candidate_quality: 0.6,
            ..Runner::default()
        };
        let (report, validation) = ShadowExecutor::default().run(&runner, "cnd-1", &tasks(4));
        assert!(report.quality_delta < 0.0);
        assert_eq!(validation.verdict, Verdict::NeedsRevision);
    }

    #[test]
    fn a_shadow_run_with_no_tasks_cannot_pass() {
        let (_r, validation) = ShadowExecutor::default().run(&Runner::default(), "cnd-1", &[]);
        assert_eq!(validation.verdict, Verdict::Rejected);
    }
}
