//! Observability: tracing subscriber setup and the persistent audit log.
//!
//! Two output channels:
//! * **tracing** — human-readable (or JSON) diagnostic logs, written to
//!   `.nexus/state/logs/` with daily rotation; controlled by `SNX_LOG`.
//! * **audit** — structured [`AuditEvent`]s persisted to SQLite through
//!   [`AuditLog`]; every payload is redacted before it is stored.

use nexus_core::events::AuditEvent;
use nexus_core::ids::{SessionId, TraceId};
use nexus_core::redact::Redactor;
use nexus_core::store::Store;
use nexus_core::Result;
use std::path::Path;
use std::sync::Arc;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Guard that must stay alive for the duration of the program so buffered
/// log lines are flushed.
pub struct LogGuard {
    _file_guard: Option<tracing_appender::non_blocking::WorkerGuard>,
}

/// Initialize tracing. `log_dir` receives rotated JSON logs; stderr gets
/// human-readable output at the level in `SNX_LOG` (default `warn` so the TUI
/// stays clean).
pub fn init_tracing(log_dir: Option<&Path>, verbose: bool) -> Result<LogGuard> {
    let default_level = if verbose { "info" } else { "warn" };
    let filter =
        EnvFilter::try_from_env("SNX_LOG").unwrap_or_else(|_| EnvFilter::new(default_level));

    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(false)
        .compact();

    let mut file_guard = None;
    let file_layer = if let Some(dir) = log_dir {
        std::fs::create_dir_all(dir)?;
        let appender = tracing_appender::rolling::daily(dir, "snx.jsonl");
        let (writer, guard) = tracing_appender::non_blocking(appender);
        file_guard = Some(guard);
        Some(
            fmt::layer()
                .json()
                .with_writer(writer)
                .with_current_span(true),
        )
    } else {
        None
    };

    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer);
    // Ignore "already set" so tests can call this repeatedly.
    let _ = registry.try_init();

    Ok(LogGuard {
        _file_guard: file_guard,
    })
}

/// Persistent, redacted audit trail.
#[derive(Clone)]
pub struct AuditLog {
    store: Store,
    redactor: Arc<Redactor>,
}

impl AuditLog {
    pub fn new(store: Store, redactor: Arc<Redactor>) -> Self {
        Self { store, redactor }
    }

    /// Record an event. Serialization happens here so the payload is redacted
    /// exactly once, at the boundary.
    pub fn record(&self, event: &AuditEvent) -> Result<()> {
        let payload = serde_json::to_string(event)?;
        let payload = self.redactor.redact(&payload);
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO audit_events (trace_id, session_id, at, kind, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    event.trace_id.as_str(),
                    event.session_id.as_ref().map(|s| s.as_str()),
                    event.timestamp,
                    event.kind_label(),
                    payload,
                ],
            )?;
            Ok(())
        })
    }

    /// Convenience: build and record in one call.
    pub fn emit(
        &self,
        trace: &TraceId,
        session: Option<&SessionId>,
        kind: nexus_core::events::AuditKind,
    ) {
        let event = AuditEvent::new(trace.clone(), session.cloned(), kind);
        if let Err(e) = self.record(&event) {
            tracing::error!(error = %e, "failed to persist audit event");
        }
    }

    /// Fetch recent events, optionally filtered by kind or trace.
    pub fn query(
        &self,
        kind: Option<&str>,
        trace: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(i64, String, String, String)>> {
        self.store.with(|conn| {
            let mut sql = String::from("SELECT id, at, kind, payload FROM audit_events WHERE 1=1");
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            if let Some(k) = kind {
                sql.push_str(" AND kind = ?");
                params.push(Box::new(k.to_string()));
            }
            if let Some(t) = trace {
                sql.push_str(" AND trace_id = ?");
                params.push(Box::new(t.to_string()));
            }
            sql.push_str(" ORDER BY id DESC LIMIT ?");
            params.push(Box::new(limit as i64));
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(
                rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::events::AuditKind;

    #[test]
    fn audit_events_are_redacted_before_persist() {
        let store = Store::open_in_memory().expect("store");
        let redactor = Arc::new(Redactor::new());
        redactor.register("hunter2secret");
        let audit = AuditLog::new(store, redactor);
        let trace = TraceId::generate();
        audit.emit(
            &trace,
            None,
            AuditKind::Failure {
                context: "test".into(),
                error: "leaked key hunter2secret in output".into(),
            },
        );
        let rows = audit.query(Some("failure"), None, 10).expect("query");
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].3.contains("hunter2secret"));
        assert!(rows[0].3.contains("[REDACTED]"));
    }

    #[test]
    fn query_filters_by_trace() {
        let store = Store::open_in_memory().expect("store");
        let audit = AuditLog::new(store, Arc::new(Redactor::new()));
        let t1 = TraceId::generate();
        let t2 = TraceId::generate();
        audit.emit(
            &t1,
            None,
            AuditKind::SessionStarted {
                workspace: "/a".into(),
            },
        );
        audit.emit(
            &t2,
            None,
            AuditKind::SessionStarted {
                workspace: "/b".into(),
            },
        );
        let rows = audit.query(None, Some(t1.as_str()), 10).expect("query");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].3.contains("/a"));
    }
}
