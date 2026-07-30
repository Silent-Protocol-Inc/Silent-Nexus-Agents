//! Historical replay.
//!
//! WARP re-runs a candidate against **sanitized** fixtures distilled from past
//! tasks and compares it to the baseline over several samples (models are
//! nondeterministic, so one run proves nothing). A candidate that lowers the
//! success rate on any fixture is a hard regression — no efficiency gain buys
//! that back. Fixtures never carry raw transcripts or secrets: the objective is
//! redacted at construction, so replay cannot leak what observation protected.

use crate::{ValidationReport, Verdict};
use nexus_core::redact::Redactor;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// A sanitized, replayable task. Built from stored summaries, never raw history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskFixture {
    pub id: String,
    /// Redacted objective — safe to store and replay.
    pub objective: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    /// Permissions and tools the replay must match so the comparison is fair.
    pub permissions: Vec<String>,
    pub tools: Vec<String>,
}

impl TaskFixture {
    /// Construct a fixture, redacting the objective so no secret is ever stored.
    pub fn sanitized(
        redactor: &Redactor,
        id: impl Into<String>,
        objective: &str,
        provider: Option<String>,
        model: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            objective: redactor.redact(objective),
            provider,
            model,
            permissions: Vec::new(),
            tools: Vec::new(),
        }
    }
}

/// The result of replaying one fixture once.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayOutcome {
    pub succeeded: bool,
    /// Lower-is-better cost metrics (tokens, tool_calls, latency_ms, …).
    pub metrics: BTreeMap<String, f64>,
}

/// Runs a fixture under a named variant (`baseline` or `candidate`). Injectable so
/// the comparison logic is tested without driving the whole agent loop.
pub trait ReplayRunner {
    fn run(&self, fixture: &TaskFixture, variant: &str) -> ReplayOutcome;
}

/// A per-metric comparison of baseline vs candidate means.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricDelta {
    pub metric: String,
    pub baseline_mean: f64,
    pub candidate_mean: f64,
    /// True when the candidate is at least as good (lower-is-better metrics).
    pub improved: bool,
}

/// The result of a replay comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayReport {
    pub fixtures: usize,
    pub samples: usize,
    pub baseline_success_rate: f64,
    pub candidate_success_rate: f64,
    /// Fixtures where the candidate's success rate fell below baseline — hard.
    pub regressions: Vec<String>,
    pub metric_deltas: Vec<MetricDelta>,
    pub verdict: Verdict,
}

/// Compares a candidate to the baseline over sanitized fixtures.
pub struct ReplayEngine {
    samples: usize,
}

impl ReplayEngine {
    pub fn new(samples: usize) -> Self {
        Self {
            samples: samples.max(1),
        }
    }

    pub fn compare(&self, runner: &impl ReplayRunner, fixtures: &[TaskFixture]) -> ReplayReport {
        let mut regressions = Vec::new();
        let (mut base_success, mut cand_success) = (0.0f64, 0.0f64);
        let mut base_metric: BTreeMap<String, (f64, usize)> = BTreeMap::new();
        let mut cand_metric: BTreeMap<String, (f64, usize)> = BTreeMap::new();
        let total_runs = (fixtures.len() * self.samples).max(1) as f64;

        // Replay is a safety gate, so “nothing was replayed” is never a pass.
        // This prevents a missing fixture directory or an accidentally empty
        // fixture export from weakening promotion.
        if fixtures.is_empty() {
            return ReplayReport {
                fixtures: 0,
                samples: self.samples,
                baseline_success_rate: 0.0,
                candidate_success_rate: 0.0,
                regressions: vec!["no replay fixtures available".into()],
                metric_deltas: Vec::new(),
                verdict: Verdict::Rejected,
            };
        }

        for fixture in fixtures {
            let (mut fb, mut fc) = (0usize, 0usize);
            for _ in 0..self.samples {
                let b = runner.run(fixture, "baseline");
                let c = runner.run(fixture, "candidate");
                if b.succeeded {
                    fb += 1;
                    base_success += 1.0;
                }
                if c.succeeded {
                    fc += 1;
                    cand_success += 1.0;
                }
                accumulate(&mut base_metric, &b.metrics);
                accumulate(&mut cand_metric, &c.metrics);
            }
            // A per-fixture success drop is a hard regression.
            if fc < fb {
                regressions.push(format!(
                    "{}: success {}/{} → {}/{}",
                    fixture.id, fb, self.samples, fc, self.samples
                ));
            }
        }

        let metric_deltas = base_metric
            .keys()
            .chain(cand_metric.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|metric| {
                let baseline_mean = mean(base_metric.get(metric));
                let candidate_mean = mean(cand_metric.get(metric));
                MetricDelta {
                    metric: metric.clone(),
                    baseline_mean,
                    candidate_mean,
                    improved: candidate_mean <= baseline_mean,
                }
            })
            .collect();

        let verdict = if regressions.is_empty() {
            Verdict::Passed
        } else {
            Verdict::Rejected
        };

        ReplayReport {
            fixtures: fixtures.len(),
            samples: self.samples,
            baseline_success_rate: base_success / total_runs,
            candidate_success_rate: cand_success / total_runs,
            regressions,
            metric_deltas,
            verdict,
        }
    }

    /// Fold a [`ReplayReport`] into a [`ValidationReport`] for the pipeline.
    pub fn to_validation(candidate_id: &str, report: &ReplayReport) -> ValidationReport {
        let mut validation = ValidationReport::new(candidate_id, "replay");
        validation.verdict = report.verdict;
        validation.hard_failures = report.regressions.clone();
        validation
    }
}

fn accumulate(acc: &mut BTreeMap<String, (f64, usize)>, metrics: &BTreeMap<String, f64>) {
    for (k, v) in metrics {
        let entry = acc.entry(k.clone()).or_insert((0.0, 0));
        entry.0 += *v;
        entry.1 += 1;
    }
}

fn mean(entry: Option<&(f64, usize)>) -> f64 {
    match entry {
        Some((sum, count)) if *count > 0 => sum / *count as f64,
        // Missing telemetry is not zero cost. Treating it as zero lets a
        // candidate omit an expensive metric and appear improved.
        _ => f64::INFINITY,
    }
}

/// Load fixtures from a directory of JSON files (one fixture per file). Missing
/// directory yields an empty set rather than an error.
pub fn load_fixtures(dir: &Path) -> Vec<TaskFixture> {
    let mut fixtures = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return fixtures;
    };
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(entry.path()) {
            if let Ok(fixture) = serde_json::from_str::<TaskFixture>(&text) {
                fixtures.push(fixture);
            }
        }
    }
    fixtures.sort_by(|a, b| a.id.cmp(&b.id));
    fixtures
}

/// Strict fixture loading for release/promotion paths. Missing directories,
/// unreadable files, malformed JSON, and empty sets are all errors rather than
/// silently shrinking the validation corpus.
pub fn load_fixtures_strict(dir: &Path) -> Result<Vec<TaskFixture>, String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| format!("cannot read replay fixture directory: {error}"))?;
    let mut fixtures = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot enumerate replay fixtures: {error}"))?;
        if entry.path().extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let path = entry.path();
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read replay fixture {}: {error}", path.display()))?;
        let fixture = serde_json::from_str::<TaskFixture>(&text)
            .map_err(|error| format!("invalid replay fixture {}: {error}", path.display()))?;
        fixtures.push(fixture);
    }
    if fixtures.is_empty() {
        return Err("replay fixture directory contains no JSON fixtures".into());
    }
    fixtures.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(fixtures)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A runner where the candidate flips one fixture from success to failure.
    struct RegressingRunner;
    impl ReplayRunner for RegressingRunner {
        fn run(&self, fixture: &TaskFixture, variant: &str) -> ReplayOutcome {
            let mut metrics = BTreeMap::new();
            metrics.insert("tokens".to_string(), 100.0);
            let succeeded = !(fixture.id == "f2" && variant == "candidate");
            ReplayOutcome { succeeded, metrics }
        }
    }

    /// A runner where the candidate keeps success and cuts tokens.
    struct ImprovingRunner;
    impl ReplayRunner for ImprovingRunner {
        fn run(&self, _fixture: &TaskFixture, variant: &str) -> ReplayOutcome {
            let mut metrics = BTreeMap::new();
            metrics.insert(
                "tokens".to_string(),
                if variant == "candidate" { 80.0 } else { 100.0 },
            );
            ReplayOutcome {
                succeeded: true,
                metrics,
            }
        }
    }

    fn fixtures() -> Vec<TaskFixture> {
        vec![
            TaskFixture::sanitized(&Redactor::new(), "f1", "do x", None, None),
            TaskFixture::sanitized(&Redactor::new(), "f2", "do y", None, None),
        ]
    }

    #[test]
    fn success_regression_is_a_hard_veto() {
        let report = ReplayEngine::new(2).compare(&RegressingRunner, &fixtures());
        assert_eq!(report.verdict, Verdict::Rejected);
        assert!(report.regressions.iter().any(|r| r.starts_with("f2")));
    }

    #[test]
    fn faster_but_still_correct_passes_and_shows_improvement() {
        let report = ReplayEngine::new(3).compare(&ImprovingRunner, &fixtures());
        assert_eq!(report.verdict, Verdict::Passed);
        assert!(report.regressions.is_empty());
        let tokens = report
            .metric_deltas
            .iter()
            .find(|d| d.metric == "tokens")
            .expect("tokens delta");
        assert!(tokens.improved);
        assert!(tokens.candidate_mean < tokens.baseline_mean);
    }

    #[test]
    fn an_empty_fixture_set_is_a_hard_rejection() {
        let report = ReplayEngine::new(2).compare(&ImprovingRunner, &[]);
        assert_eq!(report.verdict, Verdict::Rejected);
        assert_eq!(report.regressions, vec!["no replay fixtures available"]);
    }

    #[test]
    fn fixtures_never_carry_secrets() {
        let secret = "sk-abcdefghijklmnopqrstuvwx";
        let fixture = TaskFixture::sanitized(
            &Redactor::new(),
            "f1",
            &format!("use the key {secret} to log in"),
            None,
            None,
        );
        assert!(!fixture.objective.contains(secret));
    }
}
