//! Nexus RSI — the governed self-improvement engine (observation end).
//!
//! This crate records structured, secret-safe behavioural evidence as
//! [`HarnessEvent`]s. It is the "Observe" of Observe→Diagnose→Propose→…; higher
//! layers (WARP) validate candidates and governance (in `nexus-core`) constrains
//! promotion. RSI depends only on `nexus-core`, so it can never reach up into the
//! agent loop or the harness it observes — a deliberate dependency boundary.
//!
//! Every free-text field is passed through a [`Redactor`] before persistence, and
//! observation is gated so a disabled or degraded RSI never affects normal use.

pub mod events;

pub use events::{event_type, severity};

use nexus_core::harness::{HarnessEvent, HarnessRepository};
use nexus_core::redact::Redactor;
use nexus_core::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Correlation ids shared by the observations of one turn. Cloned cheaply and
/// stamped onto each [`HarnessEvent`].
#[derive(Clone, Default, Debug)]
pub struct ObservationContext {
    pub session_id: Option<String>,
    pub goal_id: Option<String>,
    pub plan_id: Option<String>,
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    pub run_id: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
}

impl ObservationContext {
    pub fn for_session(session_id: impl Into<String>) -> Self {
        Self {
            session_id: Some(session_id.into()),
            ..Self::default()
        }
    }
}

/// Records behavioural evidence as [`HarnessEvent`]s. Free text (summaries and
/// string metadata values) is redacted first; when disabled, every method is a
/// cheap no-op that returns `Ok(None)`.
pub struct ObservationCollector {
    repo: HarnessRepository,
    redactor: Arc<Redactor>,
    enabled: bool,
}

impl ObservationCollector {
    pub fn new(repo: HarnessRepository, redactor: Arc<Redactor>, enabled: bool) -> Self {
        Self {
            repo,
            redactor,
            enabled,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Build and persist one observation. Returns the event id, or `Ok(None)`
    /// when RSI observation is disabled. All string content is redacted.
    pub fn observe(
        &self,
        event_type: &str,
        severity: &str,
        summary: impl AsRef<str>,
        ctx: &ObservationContext,
        metadata: BTreeMap<String, Value>,
    ) -> Result<Option<String>> {
        if !self.enabled {
            return Ok(None);
        }
        let mut event = HarnessEvent::new(event_type, self.redactor.redact(summary.as_ref()));
        event.severity = severity.to_string();
        event.session_id = ctx.session_id.clone();
        event.goal_id = ctx.goal_id.clone();
        event.plan_id = ctx.plan_id.clone();
        event.task_id = ctx.task_id.clone();
        event.agent_id = ctx.agent_id.clone();
        event.run_id = ctx.run_id.clone();
        event.provider = ctx.provider.clone();
        event.model = ctx.model.clone();
        event.metadata = self.redact_metadata(metadata);
        let id = event.id.clone();
        // Observation must never break a turn: log and swallow storage errors at
        // the call site's discretion, but surface them here for callers that care.
        self.repo.append_event(&event)?;
        Ok(Some(id))
    }

    /// A completed turn: the primary Level-0 signal. `steps`/`tokens` feed later
    /// efficiency analysis; the objective is redacted.
    pub fn task_completed(
        &self,
        ctx: &ObservationContext,
        objective: &str,
        steps: u64,
        tokens: u64,
    ) -> Result<Option<String>> {
        let mut meta = BTreeMap::new();
        meta.insert("steps".into(), Value::from(steps));
        meta.insert("tokens".into(), Value::from(tokens));
        self.observe(
            event_type::TASK_COMPLETED,
            severity::INFO,
            format!("completed: {objective}"),
            ctx,
            meta,
        )
    }

    /// A turn that stopped without finishing (guard, budget, provider limit, …).
    pub fn task_failed(
        &self,
        ctx: &ObservationContext,
        objective: &str,
        stopped_reason: &str,
    ) -> Result<Option<String>> {
        let mut meta = BTreeMap::new();
        meta.insert("stopped_reason".into(), Value::from(stopped_reason));
        self.observe(
            event_type::TASK_FAILED,
            severity::WARNING,
            format!("stopped ({stopped_reason}): {objective}"),
            ctx,
            meta,
        )
    }

    /// A tool call that failed — the strongest per-step improvement signal.
    pub fn tool_failure(
        &self,
        ctx: &ObservationContext,
        tool: &str,
        detail: &str,
    ) -> Result<Option<String>> {
        let mut meta = BTreeMap::new();
        meta.insert("tool".into(), Value::from(tool));
        self.observe(
            event_type::TOOL_FAILURE,
            severity::ERROR,
            format!("tool `{tool}` failed: {detail}"),
            ctx,
            meta,
        )
    }

    fn redact_metadata(&self, metadata: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
        metadata
            .into_iter()
            .map(|(k, v)| (k, self.redact_value(v)))
            .collect()
    }

    fn redact_value(&self, value: Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.redactor.redact(&s)),
            Value::Array(items) => {
                Value::Array(items.into_iter().map(|v| self.redact_value(v)).collect())
            }
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(k, v)| (k, self.redact_value(v)))
                    .collect(),
            ),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::store::Store;

    fn collector(enabled: bool) -> (ObservationCollector, HarnessRepository) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("nexus.db")).expect("store");
        // Keep the dir alive for the test process.
        std::mem::forget(dir);
        let repo = HarnessRepository::new(store.clone());
        (
            ObservationCollector::new(
                HarnessRepository::new(store),
                Arc::new(Redactor::new()),
                enabled,
            ),
            repo,
        )
    }

    #[test]
    fn disabled_collector_records_nothing() {
        let (rsi, repo) = collector(false);
        let ctx = ObservationContext::for_session("s1");
        assert_eq!(
            rsi.task_completed(&ctx, "do a thing", 3, 100)
                .expect("no-op ok"),
            None
        );
        assert!(repo.session_events("s1", 10).expect("events").is_empty());
    }

    #[test]
    fn observations_are_persisted_and_correlated() {
        let (rsi, repo) = collector(true);
        let ctx = ObservationContext {
            session_id: Some("s1".into()),
            task_id: Some("t1".into()),
            provider: Some("luna".into()),
            ..Default::default()
        };
        rsi.task_completed(&ctx, "build the thing", 5, 2048)
            .expect("record completion");
        rsi.tool_failure(&ctx, "fs.read_file", "no such path")
            .expect("record failure");
        let events = repo.session_events("s1", 10).expect("events");
        assert_eq!(events.len(), 2);
        assert!(events
            .iter()
            .any(|e| e.event_type == event_type::TOOL_FAILURE
                && e.severity == severity::ERROR
                && e.task_id.as_deref() == Some("t1")
                && e.provider.as_deref() == Some("luna")));
    }

    #[test]
    fn secrets_are_redacted_from_summary_and_metadata() {
        let (rsi, repo) = collector(true);
        let ctx = ObservationContext::for_session("s1");
        let secret = "sk-abcdefghijklmnopqrstuvwx";
        let mut meta = BTreeMap::new();
        meta.insert("detail".into(), Value::from(format!("token {secret}")));
        rsi.observe(
            event_type::TOOL_FAILURE,
            severity::ERROR,
            format!("failed with {secret}"),
            &ctx,
            meta,
        )
        .expect("record observation");
        let events = repo.session_events("s1", 10).expect("events");
        let event = events.first().expect("one event");
        assert!(
            !event.summary.contains(secret),
            "secret leaked into summary"
        );
        let detail = event
            .metadata
            .get("detail")
            .and_then(Value::as_str)
            .expect("detail present");
        assert!(!detail.contains(secret), "secret leaked into metadata");
    }
}
