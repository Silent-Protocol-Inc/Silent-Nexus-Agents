//! The promotion ledger and rollback manager.
//!
//! Governance requires every promotion to be *attributable* and *reversible*.
//! This module is where that stops being a slogan: [`PromotionLedger::record`]
//! refuses to write a promotion that has no author and no way back, so a
//! candidate cannot reach the promoted state by simply not filling in the
//! rollback plan. Recording the promotion is the act that makes it real, and the
//! ledger will not record an irreversible one.
//!
//! Rollback is deliberately on this side of the fence — it lives in WARP, not in
//! the RSI engine, so the pipeline being rolled back is not the thing deciding
//! whether to roll back.

use crate::canary::{HealthBreach, HealthThresholds};
use nexus_core::store::Store;
use nexus_core::{NexusError, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// A recorded promotion, with everything needed to undo it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionRecord {
    pub id: String,
    pub workspace_key: String,
    pub candidate_id: String,
    pub version: String,
    pub parent_version: String,
    /// Commit that carried the change, for code-plane promotions.
    #[serde(default)]
    pub promoted_commit: Option<String>,
    /// Who or what authorised it — a human for tier 3, the gate for tier 1.
    pub promoted_by: String,
    /// Redacted snapshot of the configuration this promotion replaced.
    #[serde(default)]
    pub config_snapshot: String,
    /// Harness checkpoint captured before the change, when there is one.
    #[serde(default)]
    pub checkpoint_id: Option<String>,
    /// The concrete way back: a command, or a `git revert <sha>`.
    pub rollback_command: String,
    pub health_thresholds: HealthThresholds,
    pub governance_version: u32,
    pub promoted_at: String,
}

impl PromotionRecord {
    /// A promotion is reversible if it has either a command or a checkpoint.
    pub fn is_reversible(&self) -> bool {
        !self.rollback_command.trim().is_empty() || self.checkpoint_id.is_some()
    }
}

/// Why a rollback happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackTrigger {
    CriticalError,
    SecurityViolation,
    QualityRegression,
    FailureSpike,
    UserCorrectionSpike,
    TokenSpike,
    ToolFailureSpike,
    CorruptedMemory,
    InvalidMigration,
    PolicyBreach,
    /// A human asked for it.
    Manual,
}

impl RollbackTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CriticalError => "critical_error",
            Self::SecurityViolation => "security_violation",
            Self::QualityRegression => "quality_regression",
            Self::FailureSpike => "failure_spike",
            Self::UserCorrectionSpike => "user_correction_spike",
            Self::TokenSpike => "token_spike",
            Self::ToolFailureSpike => "tool_failure_spike",
            Self::CorruptedMemory => "corrupted_memory",
            Self::InvalidMigration => "invalid_migration",
            Self::PolicyBreach => "policy_breach",
            Self::Manual => "manual",
        }
    }

    /// Pick the trigger a set of health breaches implies.
    pub fn from_breaches(breaches: &[HealthBreach]) -> Self {
        if breaches.iter().any(|b| b.metric == "security_violations") {
            Self::SecurityViolation
        } else if breaches.iter().any(|b| b.metric == "success_rate") {
            Self::QualityRegression
        } else if breaches.iter().any(|b| b.metric == "error_rate") {
            Self::FailureSpike
        } else if breaches.iter().any(|b| b.metric == "tool_failure_rate") {
            Self::ToolFailureSpike
        } else if breaches.iter().any(|b| b.metric == "user_correction_rate") {
            Self::UserCorrectionSpike
        } else if breaches.iter().any(|b| b.metric == "avg_tokens") {
            Self::TokenSpike
        } else {
            Self::CriticalError
        }
    }
}

/// A recorded rollback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RollbackRecord {
    pub id: String,
    pub workspace_key: String,
    pub promotion_id: String,
    pub candidate_id: String,
    pub trigger: RollbackTrigger,
    pub detail: String,
    /// The version restored.
    pub restored_version: String,
    /// The command an operator must run for a code-plane revert, if any.
    #[serde(default)]
    pub rollback_command: String,
    #[serde(default)]
    pub breaches: Vec<HealthBreach>,
    pub rolled_back_at: String,
}

/// Append-only storage for promotions and rollbacks.
pub struct PromotionLedger {
    store: Store,
}

impl PromotionLedger {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    /// Record a promotion. Rejects an unattributable or irreversible one — that
    /// refusal is the enforcement point for the governance rule, not a warning.
    pub fn record(&self, record: &PromotionRecord) -> Result<()> {
        if record.promoted_by.trim().is_empty() {
            return Err(NexusError::PolicyDenied(
                "a promotion must record who authorised it".into(),
            ));
        }
        if !record.is_reversible() {
            return Err(NexusError::PolicyDenied(
                "a promotion must record a rollback command or a checkpoint".into(),
            ));
        }
        let payload = serde_json::to_string(record)
            .map_err(|e| NexusError::Other(format!("serialize promotion: {e}")))?;
        self.store.with_retry(|conn| {
            conn.execute(
                "INSERT INTO rsi_promotions
                 (id,workspace_key,candidate_id,version,parent_version,promoted_commit,
                  promoted_at,schema_version,payload_json)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,1,?8)",
                rusqlite::params![
                    record.id,
                    record.workspace_key,
                    record.candidate_id,
                    record.version,
                    record.parent_version,
                    record.promoted_commit,
                    record.promoted_at,
                    payload,
                ],
            )?;
            Ok(())
        })
    }

    pub fn promotion(&self, id: &str) -> Result<Option<PromotionRecord>> {
        self.store.with(|conn| {
            let payload: Option<String> = conn
                .query_row(
                    "SELECT payload_json FROM rsi_promotions WHERE id=?1",
                    rusqlite::params![id],
                    |row| row.get(0),
                )
                .optional()?;
            match payload {
                Some(json) => Ok(Some(from_json(&json)?)),
                None => Ok(None),
            }
        })
    }

    /// The most recent promotion for a workspace.
    pub fn latest(&self, workspace_key: &str) -> Result<Option<PromotionRecord>> {
        self.store.with(|conn| {
            let payload: Option<String> = conn
                .query_row(
                    "SELECT payload_json FROM rsi_promotions
                     WHERE workspace_key=?1 ORDER BY promoted_at DESC, rowid DESC LIMIT 1",
                    rusqlite::params![workspace_key],
                    |row| row.get(0),
                )
                .optional()?;
            match payload {
                Some(json) => Ok(Some(from_json(&json)?)),
                None => Ok(None),
            }
        })
    }

    fn record_rollback(&self, record: &RollbackRecord) -> Result<()> {
        let payload = serde_json::to_string(record)
            .map_err(|e| NexusError::Other(format!("serialize rollback: {e}")))?;
        self.store.with_retry(|conn| {
            conn.execute(
                "INSERT INTO rsi_rollbacks
                 (id,workspace_key,promotion_id,candidate_id,trigger,rolled_back_at,
                  schema_version,payload_json)
                 VALUES (?1,?2,?3,?4,?5,?6,1,?7)",
                rusqlite::params![
                    record.id,
                    record.workspace_key,
                    record.promotion_id,
                    record.candidate_id,
                    record.trigger.as_str(),
                    record.rolled_back_at,
                    payload,
                ],
            )?;
            Ok(())
        })
    }

    /// Rollbacks recorded against a promotion, newest first.
    pub fn rollbacks(&self, promotion_id: &str) -> Result<Vec<RollbackRecord>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT payload_json FROM rsi_rollbacks
                 WHERE promotion_id=?1 ORDER BY rolled_back_at DESC, rowid DESC",
            )?;
            let rows = stmt.query_map(rusqlite::params![promotion_id], |row| {
                row.get::<_, String>(0)
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(from_json(&row?)?);
            }
            Ok(out)
        })
    }
}

fn from_json<T: serde::de::DeserializeOwned>(json: &str) -> rusqlite::Result<T> {
    serde_json::from_str(json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

/// Restores the state a promotion replaced.
pub struct RollbackManager {
    ledger: PromotionLedger,
}

impl RollbackManager {
    pub fn new(ledger: PromotionLedger) -> Self {
        Self { ledger }
    }

    pub fn ledger(&self) -> &PromotionLedger {
        &self.ledger
    }

    /// Roll a promotion back. Returns the recorded rollback.
    ///
    /// An unknown promotion id is an error rather than a no-op: "there is
    /// nothing to undo" is not something this function may assume on its own.
    pub fn rollback(
        &self,
        promotion_id: &str,
        trigger: RollbackTrigger,
        detail: impl Into<String>,
        breaches: Vec<HealthBreach>,
    ) -> Result<RollbackRecord> {
        let promotion = self.ledger.promotion(promotion_id)?.ok_or_else(|| {
            NexusError::PolicyDenied(format!(
                "unknown promotion `{promotion_id}` cannot be rolled back"
            ))
        })?;
        let record = RollbackRecord {
            id: format!("rbk_{}", uuid::Uuid::new_v4().simple()),
            workspace_key: promotion.workspace_key.clone(),
            promotion_id: promotion.id.clone(),
            candidate_id: promotion.candidate_id.clone(),
            trigger,
            detail: detail.into(),
            restored_version: promotion.parent_version.clone(),
            rollback_command: promotion.rollback_command.clone(),
            breaches,
            rolled_back_at: nexus_core::now_rfc3339(),
        };
        self.ledger.record_rollback(&record)?;
        Ok(record)
    }

    /// Roll back because health broke, choosing the trigger from the breaches.
    pub fn rollback_for_health(
        &self,
        promotion_id: &str,
        breaches: Vec<HealthBreach>,
    ) -> Result<RollbackRecord> {
        let trigger = RollbackTrigger::from_breaches(&breaches);
        let detail = breaches
            .iter()
            .map(|b| {
                format!(
                    "{}: {:.3} → {:.3} (limit {:.3})",
                    b.metric, b.baseline, b.canary, b.threshold
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        self.rollback(promotion_id, trigger, detail, breaches)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> (tempfile::TempDir, PromotionLedger) {
        let dir = tempfile::tempdir().expect("dir");
        let store = Store::open(&dir.path().join("nexus.db")).expect("store");
        (dir, PromotionLedger::new(store))
    }

    fn promotion() -> PromotionRecord {
        PromotionRecord {
            id: "promo-1".into(),
            workspace_key: "ws".into(),
            candidate_id: "cnd-1".into(),
            version: "2.11.0+cnd-1".into(),
            parent_version: "2.11.0".into(),
            promoted_commit: None,
            promoted_by: "human:sans".into(),
            config_snapshot: "{\"retrieval\":{\"dedupe\":false}}".into(),
            checkpoint_id: Some("ckpt-1".into()),
            rollback_command: "snx rsi rollback promo-1".into(),
            health_thresholds: HealthThresholds::default(),
            governance_version: nexus_core::governance::GOVERNANCE_VERSION,
            promoted_at: nexus_core::now_rfc3339(),
        }
    }

    #[test]
    fn a_promotion_round_trips_with_its_way_back() {
        let (_dir, ledger) = ledger();
        let record = promotion();
        ledger.record(&record).expect("record");
        let loaded = ledger.promotion("promo-1").expect("load").expect("present");
        assert_eq!(loaded, record);
        assert_eq!(
            ledger.latest("ws").expect("latest").expect("present").id,
            "promo-1"
        );
    }

    #[test]
    fn an_irreversible_promotion_cannot_be_recorded() {
        let (_dir, ledger) = ledger();
        let mut record = promotion();
        record.rollback_command = "  ".into();
        record.checkpoint_id = None;
        let err = ledger.record(&record).expect_err("must refuse");
        assert!(matches!(err, NexusError::PolicyDenied(_)));
        assert!(ledger.promotion("promo-1").expect("load").is_none());
    }

    #[test]
    fn an_unattributed_promotion_cannot_be_recorded() {
        let (_dir, ledger) = ledger();
        let mut record = promotion();
        record.promoted_by = String::new();
        assert!(ledger.record(&record).is_err());
    }

    #[test]
    fn rolling_back_restores_the_parent_version_and_is_recorded() {
        let (_dir, ledger) = ledger();
        ledger.record(&promotion()).expect("record");
        let manager = RollbackManager::new(ledger);
        let record = manager
            .rollback(
                "promo-1",
                RollbackTrigger::Manual,
                "operator asked",
                Vec::new(),
            )
            .expect("rollback");
        assert_eq!(record.restored_version, "2.11.0");
        assert_eq!(record.rollback_command, "snx rsi rollback promo-1");
        let recorded = manager.ledger().rollbacks("promo-1").expect("rollbacks");
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].trigger, RollbackTrigger::Manual);
    }

    #[test]
    fn health_breaches_choose_the_trigger_and_explain_themselves() {
        let (_dir, ledger) = ledger();
        ledger.record(&promotion()).expect("record");
        let manager = RollbackManager::new(ledger);
        let breaches = vec![HealthBreach {
            metric: "success_rate".into(),
            baseline: 0.92,
            canary: 0.80,
            threshold: 0.02,
            critical: true,
        }];
        let record = manager
            .rollback_for_health("promo-1", breaches)
            .expect("rollback");
        assert_eq!(record.trigger, RollbackTrigger::QualityRegression);
        assert!(record.detail.contains("success_rate"));
        assert_eq!(record.breaches.len(), 1);
    }

    #[test]
    fn a_security_breach_outranks_other_triggers() {
        let breaches = vec![
            HealthBreach {
                metric: "avg_tokens".into(),
                baseline: 100.0,
                canary: 300.0,
                threshold: 0.25,
                critical: false,
            },
            HealthBreach {
                metric: "security_violations".into(),
                baseline: 0.0,
                canary: 1.0,
                threshold: 0.0,
                critical: true,
            },
        ];
        assert_eq!(
            RollbackTrigger::from_breaches(&breaches),
            RollbackTrigger::SecurityViolation
        );
    }

    #[test]
    fn an_unknown_promotion_cannot_be_rolled_back_silently() {
        let (_dir, ledger) = ledger();
        let manager = RollbackManager::new(ledger);
        let err = manager
            .rollback("promo-missing", RollbackTrigger::Manual, "", Vec::new())
            .expect_err("must error");
        assert!(matches!(err, NexusError::PolicyDenied(_)));
    }
}
