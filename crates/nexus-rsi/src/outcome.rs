//! Outcome evaluation.
//!
//! Nexus must not treat its own confidence as proof. An [`OutcomeRecord`] keeps
//! the quality of a completed task as **separate dimensions**, each resolved from
//! the strongest available evidence tier — a deterministic test outranks an
//! agent's self-assessment and cannot be averaged away by it. A `final_score` is
//! offered only as a convenience summary; it is never a substitute for the
//! per-dimension detail and never overrides a hard evidence failure.

use crate::prefixed_id;
use nexus_core::store::Store;
use nexus_core::Result;
use serde::{Deserialize, Serialize};

/// Evidence sources, strongest first. `rank()` is the authority order used to
/// resolve a dimension when sources disagree (lower rank wins).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceTier {
    DeterministicTest,
    IntegrationCheck,
    StaticAnalysis,
    ExpectedOutput,
    IndependentReviewer,
    UserAcceptance,
    /// The agent's own opinion — always the weakest signal.
    AgentSelfAssessment,
}

impl EvidenceTier {
    /// Authority rank; lower is stronger.
    pub fn rank(self) -> u8 {
        match self {
            Self::DeterministicTest => 0,
            Self::IntegrationCheck => 1,
            Self::StaticAnalysis => 2,
            Self::ExpectedOutput => 3,
            Self::IndependentReviewer => 4,
            Self::UserAcceptance => 5,
            Self::AgentSelfAssessment => 6,
        }
    }

    /// Deterministic/objective evidence that a model verdict cannot overturn.
    pub fn is_objective(self) -> bool {
        matches!(
            self,
            Self::DeterministicTest | Self::IntegrationCheck | Self::StaticAnalysis
        )
    }
}

/// The quality dimensions this task should assess. `Independent* -> UserAcceptance ->
/// self` is only the confidence order; correctness/safety carry the veto weight in
/// later WARP stages, which is why they are never collapsed into one number here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Correctness,
    Completeness,
    Safety,
    Efficiency,
    Reliability,
    Usability,
    Maintainability,
    Generalisation,
}

/// One piece of evidence bearing on one dimension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceLink {
    pub tier: EvidenceTier,
    pub dimension: String,
    pub summary: String,
    pub source_ref: String,
    /// Normalised score in [0.0, 1.0] this evidence assigns to the dimension.
    pub score: f64,
    /// Whether the evidence represents a pass (a hard fail on an objective tier
    /// is a veto that no weaker/soft evidence can lift).
    pub passed: bool,
}

/// Per-dimension quality. `None` means "no evidence gathered", which is honest
/// and distinct from a measured zero.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QualityDimensions {
    pub correctness: Option<f64>,
    pub completeness: Option<f64>,
    pub safety: Option<f64>,
    pub efficiency: Option<f64>,
    pub reliability: Option<f64>,
    pub usability: Option<f64>,
    pub maintainability: Option<f64>,
    pub generalisation: Option<f64>,
}

/// A multi-dimensional outcome for one completed task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutcomeRecord {
    pub id: String,
    pub workspace_key: String,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub completion_status: String,
    pub dimensions: QualityDimensions,
    pub evidence: Vec<EvidenceLink>,
    /// Confidence bounded by the strongest evidence tier actually present.
    pub confidence: f64,
    /// Convenience summary only. `None` unless every measured dimension agrees it
    /// is meaningful; never used to hide a per-dimension failure.
    pub final_score: Option<f64>,
    pub created_at: String,
}

fn dimension_key(dimension: Dimension) -> &'static str {
    match dimension {
        Dimension::Correctness => "correctness",
        Dimension::Completeness => "completeness",
        Dimension::Safety => "safety",
        Dimension::Efficiency => "efficiency",
        Dimension::Reliability => "reliability",
        Dimension::Usability => "usability",
        Dimension::Maintainability => "maintainability",
        Dimension::Generalisation => "generalisation",
    }
}

const ALL_DIMENSIONS: [Dimension; 8] = [
    Dimension::Correctness,
    Dimension::Completeness,
    Dimension::Safety,
    Dimension::Efficiency,
    Dimension::Reliability,
    Dimension::Usability,
    Dimension::Maintainability,
    Dimension::Generalisation,
];

/// Builds [`OutcomeRecord`]s from evidence, applying the evidence hierarchy.
pub struct OutcomeEvaluator;

impl OutcomeEvaluator {
    /// Resolve one dimension from its evidence: the strongest tier wins, and a
    /// failing objective verdict pins the dimension to its (low) score even if a
    /// weaker tier claims success. `None` when no evidence bears on it.
    pub fn resolve_dimension(dimension: Dimension, evidence: &[EvidenceLink]) -> Option<f64> {
        let key = dimension_key(dimension);
        let relevant: Vec<&EvidenceLink> = evidence.iter().filter(|e| e.dimension == key).collect();
        if relevant.is_empty() {
            return None;
        }
        // An objective failure is a veto: return its score directly.
        if let Some(veto) = relevant
            .iter()
            .filter(|e| e.tier.is_objective() && !e.passed)
            .min_by_key(|e| e.tier.rank())
        {
            return Some(veto.score.clamp(0.0, 1.0));
        }
        // Otherwise the strongest-tier evidence decides.
        relevant
            .iter()
            .min_by_key(|e| e.tier.rank())
            .map(|e| e.score.clamp(0.0, 1.0))
    }

    /// Evaluate a task. `completion_status` is the harness's own terminal state;
    /// dimensions and confidence come from `evidence` alone.
    pub fn evaluate(
        workspace_key: impl Into<String>,
        session_id: Option<String>,
        task_id: Option<String>,
        completion_status: impl Into<String>,
        evidence: Vec<EvidenceLink>,
    ) -> OutcomeRecord {
        let resolve = |d: Dimension| Self::resolve_dimension(d, &evidence);
        let dimensions = QualityDimensions {
            correctness: resolve(Dimension::Correctness),
            completeness: resolve(Dimension::Completeness),
            safety: resolve(Dimension::Safety),
            efficiency: resolve(Dimension::Efficiency),
            reliability: resolve(Dimension::Reliability),
            usability: resolve(Dimension::Usability),
            maintainability: resolve(Dimension::Maintainability),
            generalisation: resolve(Dimension::Generalisation),
        };

        // Confidence is capped by the strongest evidence tier present: a record
        // backed only by self-assessment can never read as highly confident.
        let confidence = evidence
            .iter()
            .map(|e| e.tier)
            .min_by_key(|t| t.rank())
            .map(|t| match t {
                EvidenceTier::DeterministicTest => 0.95,
                EvidenceTier::IntegrationCheck => 0.85,
                EvidenceTier::StaticAnalysis => 0.7,
                EvidenceTier::ExpectedOutput => 0.6,
                EvidenceTier::IndependentReviewer => 0.55,
                EvidenceTier::UserAcceptance => 0.8,
                EvidenceTier::AgentSelfAssessment => 0.25,
            })
            .unwrap_or(0.0);

        // A summary score is only offered when at least correctness and safety
        // were measured — the two dimensions that must never be silently missing.
        let measured: Vec<f64> = ALL_DIMENSIONS.iter().filter_map(|&d| resolve(d)).collect();
        let final_score = if dimensions.correctness.is_some()
            && dimensions.safety.is_some()
            && !measured.is_empty()
        {
            Some(measured.iter().sum::<f64>() / measured.len() as f64)
        } else {
            None
        };

        OutcomeRecord {
            id: prefixed_id("outcome"),
            workspace_key: workspace_key.into(),
            session_id,
            task_id,
            completion_status: completion_status.into(),
            dimensions,
            evidence,
            confidence,
            final_score,
            created_at: nexus_core::now_rfc3339(),
        }
    }
}

/// Persistence for [`OutcomeRecord`]s in the `rsi_outcomes` table (migration 0011).
pub struct OutcomeStore {
    store: Store,
}

impl OutcomeStore {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn save(&self, outcome: &OutcomeRecord) -> Result<()> {
        let payload = serde_json::to_string(outcome)
            .map_err(|e| nexus_core::NexusError::Other(format!("serialize outcome: {e}")))?;
        self.store.with_retry(|conn| {
            conn.execute(
                "INSERT INTO rsi_outcomes
                 (id,workspace_key,session_id,task_id,completion_status,final_score,
                  confidence,created_at,schema_version,payload_json)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,1,?9)",
                rusqlite::params![
                    outcome.id,
                    outcome.workspace_key,
                    outcome.session_id,
                    outcome.task_id,
                    outcome.completion_status,
                    outcome.final_score,
                    outcome.confidence,
                    outcome.created_at,
                    payload,
                ],
            )?;
            Ok(())
        })
    }

    /// Recent outcomes for a workspace, newest first.
    pub fn recent(&self, workspace_key: &str, limit: usize) -> Result<Vec<OutcomeRecord>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT payload_json FROM rsi_outcomes
                 WHERE workspace_key=?1 ORDER BY created_at DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![workspace_key, limit as i64], |row| {
                row.get::<_, String>(0)
            })?;
            let mut out = Vec::new();
            for row in rows {
                let payload = row?;
                let record: OutcomeRecord = serde_json::from_str(&payload).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                out.push(record);
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(tier: EvidenceTier, dim: &str, score: f64, passed: bool) -> EvidenceLink {
        EvidenceLink {
            tier,
            dimension: dim.to_string(),
            summary: "e".into(),
            source_ref: "ref".into(),
            score,
            passed,
        }
    }

    #[test]
    fn deterministic_failure_vetoes_optimistic_self_assessment() {
        // The agent claims correctness is great; the test says it failed.
        let evidence = vec![
            ev(EvidenceTier::AgentSelfAssessment, "correctness", 0.95, true),
            ev(EvidenceTier::DeterministicTest, "correctness", 0.0, false),
        ];
        assert_eq!(
            OutcomeEvaluator::resolve_dimension(Dimension::Correctness, &evidence),
            Some(0.0),
            "objective failure must win over self-assessment"
        );
    }

    #[test]
    fn stronger_tier_wins_when_all_pass() {
        let evidence = vec![
            ev(EvidenceTier::AgentSelfAssessment, "efficiency", 0.5, true),
            ev(EvidenceTier::IntegrationCheck, "efficiency", 0.9, true),
        ];
        assert_eq!(
            OutcomeEvaluator::resolve_dimension(Dimension::Efficiency, &evidence),
            Some(0.9)
        );
    }

    #[test]
    fn self_assessment_only_yields_low_confidence_and_no_summary_score() {
        let evidence = vec![ev(
            EvidenceTier::AgentSelfAssessment,
            "usability",
            1.0,
            true,
        )];
        let outcome = OutcomeEvaluator::evaluate("/ws", None, None, "finished", evidence);
        assert!(outcome.confidence <= 0.3, "self-assessment is weak");
        // correctness + safety were never measured → no misleading summary score.
        assert_eq!(outcome.final_score, None);
        assert_eq!(outcome.dimensions.correctness, None);
    }

    #[test]
    fn outcomes_round_trip_through_the_store() {
        let dir = tempfile::tempdir().expect("dir");
        let store = Store::open(&dir.path().join("nexus.db")).expect("store");
        let outcomes = OutcomeStore::new(store);
        let evidence = vec![
            ev(EvidenceTier::DeterministicTest, "correctness", 1.0, true),
            ev(EvidenceTier::DeterministicTest, "safety", 1.0, true),
        ];
        let record = OutcomeEvaluator::evaluate(
            "/ws",
            Some("s1".into()),
            Some("t1".into()),
            "finished",
            evidence,
        );
        assert!(record.final_score.is_some());
        outcomes.save(&record).expect("save");
        let loaded = outcomes.recent("/ws", 10).expect("recent");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, record.id);
        assert_eq!(loaded[0].dimensions.correctness, Some(1.0));
    }
}
