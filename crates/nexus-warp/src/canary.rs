//! Canary rollout and health monitoring.
//!
//! A promoted candidate does not arrive everywhere at once. It reaches 5% of
//! sessions, then 15, 30, 50, and only then everyone — and at every step the
//! [`HealthMonitor`] compares the canary arm against the baseline arm on live
//! traffic.
//!
//! Two properties are worth stating plainly:
//!
//! * **Assignment is deterministic.** A session is in the canary arm or it is
//!   not, decided by hashing `candidate_id + session_id`. A session never flips
//!   arms mid-flight, so the comparison measures the candidate rather than the
//!   noise of reassignment.
//! * **Too little evidence never advances anything.** Below
//!   `min_observations` the monitor answers `Insufficient`, which holds the
//!   rollout where it is. A quiet window is not a healthy window.

use serde::{Deserialize, Serialize};

/// The rollout ladder, in percent of sessions.
pub const CANARY_STAGES: &[u8] = &[5, 15, 30, 50, 100];

/// Live metrics for one arm of a canary.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct HealthMetrics {
    pub observations: usize,
    pub success_rate: f64,
    pub error_rate: f64,
    pub tool_failure_rate: f64,
    /// How often the user had to correct the agent.
    pub user_correction_rate: f64,
    pub avg_latency_ms: f64,
    pub avg_tokens: f64,
    /// Security violations observed in this arm. Any value above zero is
    /// critical on its own.
    pub security_violations: usize,
}

/// Limits the canary arm must stay within, expressed against the baseline arm.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HealthThresholds {
    /// Absolute drop in task success rate that triggers rollback.
    pub max_success_rate_drop: f64,
    pub max_error_rate_increase: f64,
    pub max_tool_failure_rate_increase: f64,
    pub max_user_correction_rate_increase: f64,
    /// Relative latency increase, e.g. 0.25 = 25% slower.
    pub max_latency_increase: f64,
    /// Relative token increase.
    pub max_token_increase: f64,
    /// Observations required in *each* arm before health means anything.
    pub min_observations: usize,
}

impl Default for HealthThresholds {
    fn default() -> Self {
        Self {
            max_success_rate_drop: 0.02,
            max_error_rate_increase: 0.02,
            max_tool_failure_rate_increase: 0.05,
            max_user_correction_rate_increase: 0.05,
            max_latency_increase: 0.25,
            max_token_increase: 0.25,
            min_observations: 20,
        }
    }
}

/// One threshold that was exceeded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthBreach {
    pub metric: String,
    pub baseline: f64,
    pub canary: f64,
    pub threshold: f64,
    /// Critical breaches roll back immediately; non-critical hold the rollout.
    pub critical: bool,
}

/// What the monitor concluded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HealthStatus {
    /// Within every threshold, with enough samples to say so.
    Healthy,
    /// Not enough traffic yet — hold, never advance.
    Insufficient { observed: usize, required: usize },
    /// One or more thresholds exceeded.
    Breached { breaches: Vec<HealthBreach> },
}

impl HealthStatus {
    pub fn is_healthy(&self) -> bool {
        matches!(self, Self::Healthy)
    }

    /// True when at least one breach demands an immediate rollback.
    pub fn demands_rollback(&self) -> bool {
        match self {
            Self::Breached { breaches } => breaches.iter().any(|b| b.critical),
            _ => false,
        }
    }
}

/// Compares the canary arm against the baseline arm.
#[derive(Debug, Clone, Copy, Default)]
pub struct HealthMonitor {
    thresholds: HealthThresholds,
}

impl HealthMonitor {
    pub fn new(thresholds: HealthThresholds) -> Self {
        Self { thresholds }
    }

    pub fn thresholds(&self) -> &HealthThresholds {
        &self.thresholds
    }

    pub fn assess(&self, baseline: &HealthMetrics, canary: &HealthMetrics) -> HealthStatus {
        let observed = baseline.observations.min(canary.observations);
        let mut breaches = Vec::new();

        // A security violation is critical at any sample size — it is evidence,
        // not a rate that needs a confidence interval.
        if canary.security_violations > 0 {
            breaches.push(HealthBreach {
                metric: "security_violations".into(),
                baseline: baseline.security_violations as f64,
                canary: canary.security_violations as f64,
                threshold: 0.0,
                critical: true,
            });
        }
        if !breaches.is_empty() {
            return HealthStatus::Breached { breaches };
        }

        if observed < self.thresholds.min_observations {
            return HealthStatus::Insufficient {
                observed,
                required: self.thresholds.min_observations,
            };
        }

        let t = &self.thresholds;
        let drop = baseline.success_rate - canary.success_rate;
        if drop > t.max_success_rate_drop {
            breaches.push(HealthBreach {
                metric: "success_rate".into(),
                baseline: baseline.success_rate,
                canary: canary.success_rate,
                threshold: t.max_success_rate_drop,
                critical: true,
            });
        }
        if canary.error_rate - baseline.error_rate > t.max_error_rate_increase {
            breaches.push(HealthBreach {
                metric: "error_rate".into(),
                baseline: baseline.error_rate,
                canary: canary.error_rate,
                threshold: t.max_error_rate_increase,
                critical: true,
            });
        }
        if canary.tool_failure_rate - baseline.tool_failure_rate > t.max_tool_failure_rate_increase
        {
            breaches.push(HealthBreach {
                metric: "tool_failure_rate".into(),
                baseline: baseline.tool_failure_rate,
                canary: canary.tool_failure_rate,
                threshold: t.max_tool_failure_rate_increase,
                critical: false,
            });
        }
        if canary.user_correction_rate - baseline.user_correction_rate
            > t.max_user_correction_rate_increase
        {
            breaches.push(HealthBreach {
                metric: "user_correction_rate".into(),
                baseline: baseline.user_correction_rate,
                canary: canary.user_correction_rate,
                threshold: t.max_user_correction_rate_increase,
                critical: false,
            });
        }
        if let Some(ratio) = relative_increase(baseline.avg_latency_ms, canary.avg_latency_ms) {
            if ratio > t.max_latency_increase {
                breaches.push(HealthBreach {
                    metric: "avg_latency_ms".into(),
                    baseline: baseline.avg_latency_ms,
                    canary: canary.avg_latency_ms,
                    threshold: t.max_latency_increase,
                    critical: false,
                });
            }
        }
        if let Some(ratio) = relative_increase(baseline.avg_tokens, canary.avg_tokens) {
            if ratio > t.max_token_increase {
                breaches.push(HealthBreach {
                    metric: "avg_tokens".into(),
                    baseline: baseline.avg_tokens,
                    canary: canary.avg_tokens,
                    threshold: t.max_token_increase,
                    critical: false,
                });
            }
        }

        if breaches.is_empty() {
            HealthStatus::Healthy
        } else {
            HealthStatus::Breached { breaches }
        }
    }
}

fn relative_increase(baseline: f64, canary: f64) -> Option<f64> {
    if baseline <= 0.0 {
        return None;
    }
    Some((canary - baseline) / baseline)
}

/// What the rollout should do next.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum CanaryDecision {
    /// Widen to the next stage.
    Advance { to_percent: u8 },
    /// Stay where we are — usually not enough traffic yet.
    Hold { reason: String },
    /// 100% reached and healthy.
    Complete,
    /// Roll back now.
    Rollback { breaches: Vec<HealthBreach> },
}

/// The live state of one canary rollout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanaryState {
    pub candidate_id: String,
    pub promotion_id: String,
    /// Index into [`CANARY_STAGES`].
    pub stage: usize,
    pub percent: u8,
    pub started_at: String,
    pub updated_at: String,
}

impl CanaryState {
    pub fn start(candidate_id: impl Into<String>, promotion_id: impl Into<String>) -> Self {
        let now = nexus_core::now_rfc3339();
        Self {
            candidate_id: candidate_id.into(),
            promotion_id: promotion_id.into(),
            stage: 0,
            percent: CANARY_STAGES[0],
            started_at: now.clone(),
            updated_at: now,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.percent >= 100
    }
}

/// Drives the rollout ladder.
#[derive(Debug, Clone, Copy, Default)]
pub struct CanaryManager {
    monitor: HealthMonitor,
}

impl CanaryManager {
    pub fn new(monitor: HealthMonitor) -> Self {
        Self { monitor }
    }

    /// Whether a given session runs the candidate at the current percentage.
    /// Deterministic in `(candidate_id, session_id)`, so a session keeps its arm.
    pub fn in_canary(candidate_id: &str, session_id: &str, percent: u8) -> bool {
        if percent == 0 {
            return false;
        }
        if percent >= 100 {
            return true;
        }
        bucket(candidate_id, session_id) < u64::from(percent)
    }

    /// Decide the next move and apply it to `state` when it advances.
    pub fn step(
        &self,
        state: &mut CanaryState,
        baseline: &HealthMetrics,
        canary: &HealthMetrics,
    ) -> CanaryDecision {
        match self.monitor.assess(baseline, canary) {
            HealthStatus::Breached { breaches } => {
                if breaches.iter().any(|b| b.critical) {
                    CanaryDecision::Rollback { breaches }
                } else {
                    CanaryDecision::Hold {
                        reason: format!(
                            "{} non-critical breach(es) at {}%",
                            breaches.len(),
                            state.percent
                        ),
                    }
                }
            }
            HealthStatus::Insufficient { observed, required } => CanaryDecision::Hold {
                reason: format!("{observed}/{required} observations at {}%", state.percent),
            },
            HealthStatus::Healthy => {
                if state.is_complete() {
                    return CanaryDecision::Complete;
                }
                state.stage += 1;
                state.percent = CANARY_STAGES[state.stage.min(CANARY_STAGES.len() - 1)];
                state.updated_at = nexus_core::now_rfc3339();
                CanaryDecision::Advance {
                    to_percent: state.percent,
                }
            }
        }
    }
}

/// FNV-1a over `candidate_id:session_id`, mapped into 0..100.
fn bucket(candidate_id: &str, session_id: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in candidate_id
        .as_bytes()
        .iter()
        .chain(b":")
        .chain(session_id.as_bytes())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash % 100
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(observations: usize, success_rate: f64) -> HealthMetrics {
        HealthMetrics {
            observations,
            success_rate,
            error_rate: 0.01,
            tool_failure_rate: 0.02,
            user_correction_rate: 0.03,
            avg_latency_ms: 1000.0,
            avg_tokens: 500.0,
            security_violations: 0,
        }
    }

    #[test]
    fn arm_assignment_is_stable_and_roughly_proportional() {
        let sessions: Vec<String> = (0..1000).map(|i| format!("session-{i}")).collect();
        let in_arm = |percent: u8| {
            sessions
                .iter()
                .filter(|s| CanaryManager::in_canary("cnd-1", s, percent))
                .count()
        };
        let five = in_arm(5);
        assert!((20..=90).contains(&five), "5% arm held {five}/1000");
        assert!(in_arm(50) > five);
        assert_eq!(in_arm(100), 1000);
        assert_eq!(in_arm(0), 0);

        // Stable across calls, and the 5% arm is a subset of the 50% arm.
        for session in &sessions {
            assert_eq!(
                CanaryManager::in_canary("cnd-1", session, 5),
                CanaryManager::in_canary("cnd-1", session, 5)
            );
            if CanaryManager::in_canary("cnd-1", session, 5) {
                assert!(CanaryManager::in_canary("cnd-1", session, 50));
            }
        }
    }

    #[test]
    fn a_healthy_canary_climbs_the_ladder_to_complete() {
        let manager = CanaryManager::default();
        let mut state = CanaryState::start("cnd-1", "promo-1");
        assert_eq!(state.percent, 5);
        let (base, canary) = (metrics(100, 0.9), metrics(100, 0.91));
        for expected in [15u8, 30, 50, 100] {
            assert_eq!(
                manager.step(&mut state, &base, &canary),
                CanaryDecision::Advance {
                    to_percent: expected
                }
            );
        }
        assert_eq!(
            manager.step(&mut state, &base, &canary),
            CanaryDecision::Complete
        );
    }

    #[test]
    fn a_quiet_window_holds_rather_than_advancing() {
        let manager = CanaryManager::default();
        let mut state = CanaryState::start("cnd-1", "promo-1");
        let decision = manager.step(&mut state, &metrics(3, 0.9), &metrics(3, 1.0));
        assert!(matches!(decision, CanaryDecision::Hold { .. }));
        assert_eq!(state.percent, 5, "a hold must not widen the rollout");
    }

    #[test]
    fn a_success_regression_rolls_back() {
        let manager = CanaryManager::default();
        let mut state = CanaryState::start("cnd-1", "promo-1");
        let decision = manager.step(&mut state, &metrics(100, 0.92), &metrics(100, 0.80));
        match decision {
            CanaryDecision::Rollback { breaches } => {
                assert_eq!(breaches[0].metric, "success_rate");
                assert!(breaches[0].critical);
            }
            other => panic!("expected rollback, got {other:?}"),
        }
    }

    #[test]
    fn one_security_violation_rolls_back_before_any_sample_threshold() {
        let manager = CanaryManager::default();
        let mut state = CanaryState::start("cnd-1", "promo-1");
        let mut canary = metrics(2, 1.0);
        canary.security_violations = 1;
        let decision = manager.step(&mut state, &metrics(2, 0.9), &canary);
        match decision {
            CanaryDecision::Rollback { breaches } => {
                assert_eq!(breaches[0].metric, "security_violations")
            }
            other => panic!("expected rollback, got {other:?}"),
        }
    }

    #[test]
    fn a_slower_but_correct_canary_holds_instead_of_rolling_back() {
        let manager = CanaryManager::default();
        let mut state = CanaryState::start("cnd-1", "promo-1");
        let mut canary = metrics(100, 0.91);
        canary.avg_latency_ms = 2000.0;
        let decision = manager.step(&mut state, &metrics(100, 0.9), &canary);
        assert!(matches!(decision, CanaryDecision::Hold { .. }));
        assert_eq!(state.percent, 5);
    }

    #[test]
    fn a_zero_baseline_never_divides_by_zero() {
        let monitor = HealthMonitor::default();
        let mut base = metrics(100, 0.9);
        base.avg_latency_ms = 0.0;
        base.avg_tokens = 0.0;
        let canary = metrics(100, 0.91);
        assert!(monitor.assess(&base, &canary).is_healthy());
    }
}
