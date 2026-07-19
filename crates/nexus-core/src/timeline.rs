//! Durable execution timeline and transcript storage.
//!
//! The timeline is the canonical operator-visible execution record. Native
//! events are append-only; a running card may be updated in place by stable
//! event id as streamed text or lifecycle status arrives. Sessions created by
//! older releases are projected from messages, tools, approvals, and audit
//! events without rewriting those records.

use crate::ids::{SessionId, SpanId, TraceId, TurnId};
use crate::orchestration::{ContextManifest, StageStatus, ValidationEvidence, WorkBreakdown};
use crate::store::Store;
use crate::{NexusError, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Blocked,
    Cancelled,
    Skipped,
    Waiting,
}

impl TimelineStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
            Self::Cancelled => "cancelled",
            Self::Skipped => "skipped",
            Self::Waiting => "waiting",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "failed" | "error" => Self::Failed,
            "blocked" | "denied" => Self::Blocked,
            "cancelled" | "canceled" => Self::Cancelled,
            "skipped" => Self::Skipped,
            "waiting" => Self::Waiting,
            _ => Self::Completed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecyclePhase {
    Proposed,
    Policy,
    Approval,
    Started,
    Progress,
    Completed,
    Failed,
    Cancelled,
    Checkpoint,
    Message,
}

impl LifecyclePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Policy => "policy",
            Self::Approval => "approval",
            Self::Started => "started",
            Self::Progress => "progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Checkpoint => "checkpoint",
            Self::Message => "message",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "proposed" => Self::Proposed,
            "policy" => Self::Policy,
            "approval" => Self::Approval,
            "started" => Self::Started,
            "progress" => Self::Progress,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "checkpoint" => Self::Checkpoint,
            "message" => Self::Message,
            _ => Self::Completed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineSource {
    Native,
    LegacyProjection,
    Command,
    Background,
}

impl TimelineSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::LegacyProjection => "legacy_projection",
            Self::Command => "command",
            Self::Background => "background",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "legacy_projection" => Self::LegacyProjection,
            "command" => Self::Command,
            "background" => Self::Background,
            _ => Self::Native,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptDetail {
    #[default]
    Compact,
    Expanded,
    Raw,
}

impl TranscriptDetail {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Expanded => "expanded",
            Self::Raw => "raw",
        }
    }
}

impl FromStr for TranscriptDetail {
    type Err = NexusError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "compact" => Ok(Self::Compact),
            "expanded" => Ok(Self::Expanded),
            "raw" => Ok(Self::Raw),
            _ => Err(NexusError::Config(format!(
                "unknown detail level `{value}`; expected compact|expanded|raw"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptFilter {
    #[default]
    All,
    Messages,
    Plans,
    Tools,
    Diffs,
    Agents,
    Warnings,
    Errors,
}

impl TranscriptFilter {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Messages => "messages",
            Self::Plans => "plans",
            Self::Tools => "tools",
            Self::Diffs => "diffs",
            Self::Agents => "agents",
            Self::Warnings => "warnings",
            Self::Errors => "errors",
        }
    }

    pub fn matches(self, event: &TimelineEvent) -> bool {
        match self {
            Self::All => true,
            Self::Messages => matches!(
                event.kind,
                TimelineKind::UserMessage { .. }
                    | TimelineKind::AssistantMessage { .. }
                    | TimelineKind::FinalAnswer { .. }
                    | TimelineKind::Notice { .. }
            ),
            Self::Plans => matches!(
                event.kind,
                TimelineKind::WorkBreakdown { .. }
                    | TimelineKind::PlanRevision { .. }
                    | TimelineKind::StageChanged { .. }
            ),
            Self::Tools => matches!(
                event.kind,
                TimelineKind::ToolProposal { .. }
                    | TimelineKind::PolicyDecision { .. }
                    | TimelineKind::Approval { .. }
                    | TimelineKind::ToolExecution { .. }
                    | TimelineKind::ToolProgress { .. }
                    | TimelineKind::SandboxCommand { .. }
            ),
            Self::Diffs => matches!(
                event.kind,
                TimelineKind::FileMutation { .. }
                    | TimelineKind::GitStatus { .. }
                    | TimelineKind::Diff { .. }
                    | TimelineKind::Validation { .. }
            ),
            Self::Agents => matches!(
                event.kind,
                TimelineKind::BackgroundTask { .. }
                    | TimelineKind::AgentRun { .. }
                    | TimelineKind::Checkpoint { .. }
            ),
            Self::Warnings => {
                matches!(
                    event.kind,
                    TimelineKind::Retry { .. }
                        | TimelineKind::ProviderLimit { .. }
                        | TimelineKind::Cancellation { .. }
                ) || matches!(
                    event.status,
                    TimelineStatus::Waiting | TimelineStatus::Blocked | TimelineStatus::Cancelled
                )
            }
            Self::Errors => {
                matches!(event.kind, TimelineKind::Error { .. })
                    || event.status == TimelineStatus::Failed
            }
        }
    }
}

impl FromStr for TranscriptFilter {
    type Err = NexusError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "all" => Ok(Self::All),
            "messages" => Ok(Self::Messages),
            "plans" => Ok(Self::Plans),
            "tools" => Ok(Self::Tools),
            "diffs" => Ok(Self::Diffs),
            "agents" => Ok(Self::Agents),
            "warnings" => Ok(Self::Warnings),
            "errors" => Ok(Self::Errors),
            _ => Err(NexusError::Config(format!(
                "unknown transcript filter `{value}`; expected all|messages|plans|tools|diffs|agents|warnings|errors"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactReference {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub bytes: Option<u64>,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineSearchHit {
    pub event_id: String,
    pub session_id: SessionId,
    pub sequence: u64,
    pub summary: String,
    pub rank: f64,
}

/// Typed redacted payload carried by one timeline event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub enum TimelineKind {
    UserMessage {
        text: String,
    },
    AssistantMessage {
        text: String,
        streaming: bool,
    },
    Classification {
        class: String,
        model: String,
        agent: String,
    },
    ModelRouting {
        provider: String,
        model: String,
        reason: String,
    },
    ProviderActivity {
        provider: String,
        model: String,
        effort: String,
        reasoning_enabled: bool,
    },
    ReasoningSummary {
        text: String,
    },
    WorkBreakdown {
        breakdown: WorkBreakdown,
    },
    PlanRevision {
        plan_id: String,
        from_version: u32,
        to_version: u32,
        diff: String,
        approval_required: bool,
    },
    StageChanged {
        plan_id: String,
        stage_id: String,
        title: String,
        status: StageStatus,
        next_action: Option<String>,
    },
    ToolProposal {
        tool: String,
        arguments: Value,
        summary: String,
        risk: String,
    },
    PolicyDecision {
        tool: String,
        decision: String,
        layer: String,
        reason: String,
    },
    Approval {
        tool: String,
        decision: Option<String>,
        summary: String,
        edited: bool,
    },
    ToolExecution {
        tool: String,
        arguments: Value,
        output_preview: String,
        exit_status: Option<String>,
        affected_paths: Vec<String>,
    },
    ToolProgress {
        tool: String,
        message: String,
        completed_units: Option<u64>,
        total_units: Option<u64>,
    },
    FileMutation {
        path: String,
        operation: String,
        bytes: Option<u64>,
    },
    GitStatus {
        branch: Option<String>,
        head: Option<String>,
        modified: Vec<String>,
        staged: Vec<String>,
        untracked: Vec<String>,
    },
    Diff {
        path: Option<String>,
        insertions: usize,
        deletions: usize,
        preview: String,
    },
    Validation {
        evidence: ValidationEvidence,
    },
    ContextPacked {
        manifest_id: String,
        total_tokens: usize,
        estimated: bool,
        omitted: usize,
    },
    Compaction {
        before_tokens: usize,
        after_tokens: usize,
        summarized_messages: usize,
        preserved: Vec<String>,
    },
    Retry {
        attempt: u32,
        max: u32,
        reason: String,
    },
    ProviderLimit {
        provider: String,
        limit_kind: String,
        reset_at: Option<String>,
        message: String,
    },
    Error {
        class: String,
        message: String,
        retryable: bool,
    },
    Cancellation {
        reason: String,
        by: String,
    },
    BackgroundTask {
        task_id: String,
        title: String,
        status: String,
        owner: String,
    },
    AgentRun {
        agent_id: String,
        parent_agent_id: Option<String>,
        role: String,
        status: String,
        objective: String,
    },
    Checkpoint {
        artifact_id: Option<String>,
        child_session_id: Option<String>,
        next_action: String,
    },
    FinalAnswer {
        text: String,
    },
    SlashCommand {
        command: String,
        arguments: Vec<String>,
        result: Option<String>,
    },
    SandboxCommand {
        command: Vec<String>,
        backend: String,
        output_preview: String,
    },
    Notice {
        text: String,
        severity: String,
    },
    LegacyAudit {
        audit_kind: String,
        payload: Value,
    },
}

impl TimelineKind {
    pub fn type_label(&self) -> &'static str {
        match self {
            Self::UserMessage { .. } => "user_message",
            Self::AssistantMessage { .. } => "assistant_message",
            Self::Classification { .. } => "classification",
            Self::ModelRouting { .. } => "model_routing",
            Self::ProviderActivity { .. } => "provider_activity",
            Self::ReasoningSummary { .. } => "reasoning_summary",
            Self::WorkBreakdown { .. } => "work_breakdown",
            Self::PlanRevision { .. } => "plan_revision",
            Self::StageChanged { .. } => "stage_changed",
            Self::ToolProposal { .. } => "tool_proposal",
            Self::PolicyDecision { .. } => "policy_decision",
            Self::Approval { .. } => "approval",
            Self::ToolExecution { .. } => "tool_execution",
            Self::ToolProgress { .. } => "tool_progress",
            Self::FileMutation { .. } => "file_mutation",
            Self::GitStatus { .. } => "git_status",
            Self::Diff { .. } => "diff",
            Self::Validation { .. } => "validation",
            Self::ContextPacked { .. } => "context_packed",
            Self::Compaction { .. } => "compaction",
            Self::Retry { .. } => "retry",
            Self::ProviderLimit { .. } => "provider_limit",
            Self::Error { .. } => "error",
            Self::Cancellation { .. } => "cancellation",
            Self::BackgroundTask { .. } => "background_task",
            Self::AgentRun { .. } => "agent_run",
            Self::Checkpoint { .. } => "checkpoint",
            Self::FinalAnswer { .. } => "final_answer",
            Self::SlashCommand { .. } => "slash_command",
            Self::SandboxCommand { .. } => "sandbox_command",
            Self::Notice { .. } => "notice",
            Self::LegacyAudit { .. } => "legacy_audit",
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Self::UserMessage { text }
            | Self::ReasoningSummary { text }
            | Self::FinalAnswer { text }
            | Self::Notice { text, .. } => Some(text),
            Self::AssistantMessage { text, .. } => Some(text),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub id: String,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub parent_span_id: Option<SpanId>,
    pub sequence: u64,
    pub timestamp: String,
    pub phase: LifecyclePhase,
    pub status: TimelineStatus,
    pub duration_ms: Option<u64>,
    pub summary: String,
    pub kind: TimelineKind,
    pub artifact_refs: Vec<ArtifactReference>,
    pub risk: Option<String>,
    pub source: TimelineSource,
}

impl TimelineEvent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_id: SessionId,
        turn_id: TurnId,
        trace_id: TraceId,
        span_id: SpanId,
        parent_span_id: Option<SpanId>,
        phase: LifecyclePhase,
        status: TimelineStatus,
        summary: impl Into<String>,
        kind: TimelineKind,
    ) -> Self {
        Self {
            id: format!("evt_{}", uuid::Uuid::new_v4().simple()),
            session_id,
            turn_id,
            trace_id,
            span_id,
            parent_span_id,
            sequence: 0,
            timestamp: crate::now_rfc3339(),
            phase,
            status,
            duration_ms: None,
            summary: summary.into(),
            kind,
            artifact_refs: Vec::new(),
            risk: None,
            source: TimelineSource::Native,
        }
    }

    pub fn searchable_text(&self) -> String {
        let payload = serde_json::to_string(&self.kind).unwrap_or_default();
        format!("{} {}", self.summary, payload).to_ascii_lowercase()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineSpan {
    pub id: SpanId,
    pub trace_id: TraceId,
    pub parent_span_id: Option<SpanId>,
    pub name: String,
    pub status: TimelineStatus,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionViewState {
    pub session_id: SessionId,
    pub last_read_sequence: u64,
    pub selected_filter: TranscriptFilter,
    pub detail_level: TranscriptDetail,
    pub collapsed_cards: BTreeSet<String>,
    pub search_query: Option<String>,
    pub updated_at: String,
}

impl SessionViewState {
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            last_read_sequence: 0,
            selected_filter: TranscriptFilter::All,
            detail_level: TranscriptDetail::Compact,
            collapsed_cards: BTreeSet::new(),
            search_query: None,
            updated_at: crate::now_rfc3339(),
        }
    }
}

#[derive(Clone)]
pub struct TimelineStore {
    store: Store,
}

impl TimelineStore {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn append(&self, mut event: TimelineEvent) -> Result<TimelineEvent> {
        self.store.with_retry(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result: Result<()> = (|| {
                let sequence: i64 = conn.query_row(
                    "SELECT COALESCE(MAX(sequence), 0) + 1
                 FROM timeline_events WHERE session_id = ?1",
                    [event.session_id.as_str()],
                    |row| row.get(0),
                )?;
                event.sequence = sequence.max(1) as u64;
                let payload = serde_json::to_string(&event.kind)?;
                let artifacts = serde_json::to_string(&event.artifact_refs)?;
                conn.execute(
                    "INSERT INTO timeline_events
                 (id, session_id, turn_id, trace_id, span_id, parent_span_id,
                  sequence, at, event_type, phase, status, duration_ms, summary,
                  payload, artifact_refs, risk, source)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
                    params![
                        event.id,
                        event.session_id.as_str(),
                        event.turn_id.as_str(),
                        event.trace_id.as_str(),
                        event.span_id.as_str(),
                        event.parent_span_id.as_ref().map(SpanId::as_str),
                        sequence,
                        event.timestamp,
                        event.kind.type_label(),
                        event.phase.as_str(),
                        event.status.as_str(),
                        event.duration_ms.map(|value| value as i64),
                        event.summary,
                        payload,
                        artifacts,
                        event.risk,
                        event.source.as_str(),
                    ],
                )?;
                Ok(())
            })();
            match result {
                Ok(()) => conn.execute_batch("COMMIT")?,
                Err(error) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    return Err(error);
                }
            }
            Ok(event.clone())
        })
    }

    /// Update a stable running card as streamed text or lifecycle status
    /// arrives. Sequence and timestamp remain unchanged, so the viewport does
    /// not jump and exports contain one lifecycle card rather than duplicates.
    pub fn update(&self, event: &TimelineEvent) -> Result<()> {
        self.store.with(|conn| {
            let changed = conn.execute(
                "UPDATE timeline_events SET phase=?1, status=?2, duration_ms=?3,
                 summary=?4, event_type=?5, payload=?6, artifact_refs=?7, risk=?8
                 WHERE id=?9 AND session_id=?10",
                params![
                    event.phase.as_str(),
                    event.status.as_str(),
                    event.duration_ms.map(|value| value as i64),
                    event.summary,
                    event.kind.type_label(),
                    serde_json::to_string(&event.kind)?,
                    serde_json::to_string(&event.artifact_refs)?,
                    event.risk,
                    event.id,
                    event.session_id.as_str(),
                ],
            )?;
            if changed == 0 {
                return Err(NexusError::NotFound(format!(
                    "timeline event `{}`",
                    event.id
                )));
            }
            Ok(())
        })
    }

    /// Close every in-flight card when the foreground turn is cancelled.
    /// Stable ids and sequences are preserved so the transcript never leaves
    /// a phantom running assistant/tool card after Ctrl+C.
    pub fn cancel_running(&self, session_id: &str, reason: &str) -> Result<usize> {
        if !self.has_native_events(session_id)? {
            return Ok(0);
        }
        self.store.with_retry(|connection| {
            let changed = connection.execute(
                "UPDATE timeline_events
                 SET phase='cancelled',
                     status='cancelled',
                     summary=summary || ' · ' || ?2,
                     payload=CASE
                       WHEN event_type='assistant_message'
                         THEN json_set(payload, '$.data.streaming', json('false'))
                       WHEN event_type='tool_execution'
                         THEN json_set(payload, '$.data.exit_status', 'cancelled')
                       ELSE payload
                     END
                 WHERE session_id=?1 AND status='running'",
                params![session_id, reason],
            )?;
            Ok(changed)
        })
    }

    pub fn get(&self, id: &str) -> Result<TimelineEvent> {
        self.store.with(|conn| {
            conn.query_row(
                "SELECT id, session_id, turn_id, trace_id, span_id, parent_span_id,
                        sequence, at, phase, status, duration_ms, summary, payload,
                        artifact_refs, risk, source
                 FROM timeline_events WHERE id=?1",
                [id],
                row_to_event,
            )
            .map_err(|_| NexusError::NotFound(format!("timeline event `{id}`")))
        })
    }

    pub fn has_native_events(&self, session_id: &str) -> Result<bool> {
        self.store.with(|conn| {
            Ok(conn
                .prepare("SELECT 1 FROM timeline_events WHERE session_id=?1 LIMIT 1")?
                .exists([session_id])?)
        })
    }

    /// Latest page, or events before an existing sequence for upward paging.
    pub fn page(
        &self,
        session_id: &str,
        before_sequence: Option<u64>,
        limit: usize,
        filter: TranscriptFilter,
    ) -> Result<Vec<TimelineEvent>> {
        if !self.has_native_events(session_id)? {
            let mut projected = self.project_legacy(session_id)?;
            projected.retain(|event| {
                filter.matches(event)
                    && before_sequence.is_none_or(|before| event.sequence < before)
            });
            let start = projected.len().saturating_sub(limit.max(1));
            return Ok(projected.split_off(start));
        }
        let wanted = limit.max(1);
        let fetch_limit = wanted.saturating_mul(4).clamp(64, 4_096);
        let mut cursor = before_sequence;
        let mut events = Vec::with_capacity(wanted);
        while events.len() < wanted {
            let batch = self.store.with(|conn| {
                let mut out = Vec::new();
                if let Some(before) = cursor {
                    let mut stmt = conn.prepare(
                        "SELECT id, session_id, turn_id, trace_id, span_id, parent_span_id,
                                sequence, at, phase, status, duration_ms, summary, payload,
                                artifact_refs, risk, source
                         FROM timeline_events
                         WHERE session_id=?1 AND sequence < ?2
                         ORDER BY sequence DESC LIMIT ?3",
                    )?;
                    let rows = stmt.query_map(
                        params![session_id, before as i64, fetch_limit as i64],
                        row_to_event,
                    )?;
                    for row in rows {
                        out.push(row?);
                    }
                } else {
                    let mut stmt = conn.prepare(
                        "SELECT id, session_id, turn_id, trace_id, span_id, parent_span_id,
                                sequence, at, phase, status, duration_ms, summary, payload,
                                artifact_refs, risk, source
                         FROM timeline_events
                         WHERE session_id=?1
                         ORDER BY sequence DESC LIMIT ?2",
                    )?;
                    let rows =
                        stmt.query_map(params![session_id, fetch_limit as i64], row_to_event)?;
                    for row in rows {
                        out.push(row?);
                    }
                }
                Ok(out)
            })?;
            if batch.is_empty() {
                break;
            }
            cursor = batch.last().map(|event| event.sequence);
            let batch_len = batch.len();
            for event in batch {
                if filter.matches(&event) {
                    events.push(event);
                    if events.len() == wanted {
                        break;
                    }
                }
            }
            if batch_len < fetch_limit {
                break;
            }
        }
        events.reverse();
        Ok(events)
    }

    pub fn all(&self, session_id: &str, filter: TranscriptFilter) -> Result<Vec<TimelineEvent>> {
        if !self.has_native_events(session_id)? {
            let mut projected = self.project_legacy(session_id)?;
            projected.retain(|event| filter.matches(event));
            return Ok(projected);
        }
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, session_id, turn_id, trace_id, span_id, parent_span_id,
                        sequence, at, phase, status, duration_ms, summary, payload,
                        artifact_refs, risk, source
                 FROM timeline_events WHERE session_id=?1 ORDER BY sequence",
            )?;
            let rows = stmt.query_map([session_id], row_to_event)?;
            let mut out = Vec::new();
            for row in rows {
                let event = row?;
                if filter.matches(&event) {
                    out.push(event);
                }
            }
            Ok(out)
        })
    }

    /// Background-origin events newer than a sequence. Used by an attached
    /// TUI to receive durable worker activity without duplicating foreground
    /// loop cards already streamed over the in-process event channel.
    pub fn background_after(
        &self,
        session_id: &str,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<TimelineEvent>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, session_id, turn_id, trace_id, span_id, parent_span_id,
                        sequence, at, phase, status, duration_ms, summary, payload,
                        artifact_refs, risk, source
                 FROM timeline_events
                 WHERE session_id=?1 AND sequence>?2 AND source='background'
                 ORDER BY sequence LIMIT ?3",
            )?;
            let rows = stmt.query_map(
                params![session_id, after_sequence as i64, limit.max(1) as i64],
                row_to_event,
            )?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    pub fn search(
        &self,
        session_id: &str,
        query: &str,
        filter: TranscriptFilter,
    ) -> Result<Vec<TimelineEvent>> {
        let hits = self.search_hits(session_id, query, filter, 500)?;
        hits.into_iter()
            .map(|hit| self.get(&hit.event_id))
            .collect()
    }

    pub fn search_hits(
        &self,
        session_id: &str,
        query: &str,
        filter: TranscriptFilter,
        limit: usize,
    ) -> Result<Vec<TimelineSearchHit>> {
        let needle = query.trim().to_ascii_lowercase();
        let query = fts_query(query);
        if query.is_empty() || needle.is_empty() {
            return Ok(Vec::new());
        }
        if !self.has_native_events(session_id)? {
            return Ok(self
                .project_legacy(session_id)?
                .into_iter()
                .filter(|event| filter.matches(event) && event.searchable_text().contains(&needle))
                .take(limit.clamp(1, 500))
                .map(|event| TimelineSearchHit {
                    event_id: event.id,
                    session_id: event.session_id,
                    sequence: event.sequence,
                    summary: event.summary,
                    rank: 0.0,
                })
                .collect());
        }
        let wanted = limit.clamp(1, 500);
        let mut hits = Vec::with_capacity(wanted);
        let mut offset = 0usize;
        const CHUNK: usize = 512;
        while hits.len() < wanted {
            let candidates = self.store.with(|connection| {
                let mut statement = connection.prepare(
                    "SELECT t.id,t.session_id,t.sequence,t.summary,bm25(timeline_fts)
                     FROM timeline_fts
                     JOIN timeline_events AS t ON t.rowid=timeline_fts.rowid
                     WHERE t.session_id=?1 AND timeline_fts MATCH ?2
                     ORDER BY bm25(timeline_fts), t.sequence DESC
                     LIMIT ?3 OFFSET ?4",
                )?;
                let rows = statement.query_map(
                    params![session_id, query, CHUNK as i64, offset as i64],
                    |row| {
                        Ok(TimelineSearchHit {
                            event_id: row.get(0)?,
                            session_id: SessionId::from(row.get::<_, String>(1)?),
                            sequence: row.get::<_, i64>(2)?.max(0) as u64,
                            summary: row.get(3)?,
                            rank: row.get(4)?,
                        })
                    },
                )?;
                let mut candidates = Vec::new();
                for row in rows {
                    candidates.push(row?);
                }
                Ok(candidates)
            })?;
            if candidates.is_empty() {
                break;
            }
            let count = candidates.len();
            for hit in candidates {
                if filter.matches(&self.get(&hit.event_id)?) {
                    hits.push(hit);
                    if hits.len() == wanted {
                        break;
                    }
                }
            }
            offset += count;
            if count < CHUNK {
                break;
            }
        }
        Ok(hits)
    }

    pub fn page_around(
        &self,
        session_id: &str,
        sequence: u64,
        radius: usize,
        filter: TranscriptFilter,
    ) -> Result<Vec<TimelineEvent>> {
        if !self.has_native_events(session_id)? {
            let mut events = self.project_legacy(session_id)?;
            events.retain(|event| {
                filter.matches(event)
                    && event.sequence >= sequence.saturating_sub(radius as u64)
                    && event.sequence <= sequence.saturating_add(radius as u64)
            });
            return Ok(events);
        }
        let lower = sequence.saturating_sub(radius as u64);
        let upper = sequence.saturating_add(radius as u64);
        self.store.with(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, session_id, turn_id, trace_id, span_id, parent_span_id,
                        sequence, at, phase, status, duration_ms, summary, payload,
                        artifact_refs, risk, source
                 FROM timeline_events
                 WHERE session_id=?1 AND sequence BETWEEN ?2 AND ?3
                 ORDER BY sequence",
            )?;
            let rows = statement.query_map(
                params![session_id, lower as i64, upper as i64],
                row_to_event,
            )?;
            let mut events = Vec::new();
            for row in rows {
                let event = row?;
                if filter.matches(&event) {
                    events.push(event);
                }
            }
            Ok(events)
        })
    }

    pub fn latest_sequence(&self, session_id: &str) -> Result<u64> {
        if !self.has_native_events(session_id)? {
            return Ok(self
                .project_legacy(session_id)?
                .last()
                .map(|event| event.sequence)
                .unwrap_or(0));
        }
        self.store.with(|conn| {
            Ok(conn.query_row(
                "SELECT COALESCE(MAX(sequence), 0) FROM timeline_events WHERE session_id=?1",
                [session_id],
                |row| row.get::<_, i64>(0),
            )? as u64)
        })
    }

    pub fn view_state(&self, session_id: &str) -> Result<SessionViewState> {
        self.store.with(|conn| {
            let row = conn
                .query_row(
                    "SELECT last_read_sequence, selected_filter, detail_level,
                            collapsed_cards, search_query, updated_at
                     FROM session_view_state WHERE session_id=?1",
                    [session_id],
                    |row| {
                        let filter: String = row.get(1)?;
                        let detail: String = row.get(2)?;
                        let collapsed: String = row.get(3)?;
                        Ok(SessionViewState {
                            session_id: SessionId::from(session_id),
                            last_read_sequence: row.get::<_, i64>(0)?.max(0) as u64,
                            selected_filter: TranscriptFilter::from_str(&filter)
                                .unwrap_or_default(),
                            detail_level: TranscriptDetail::from_str(&detail).unwrap_or_default(),
                            collapsed_cards: serde_json::from_str(&collapsed).unwrap_or_default(),
                            search_query: row.get(4)?,
                            updated_at: row.get(5)?,
                        })
                    },
                )
                .optional()?;
            Ok(row.unwrap_or_else(|| SessionViewState::new(SessionId::from(session_id))))
        })
    }

    pub fn save_view_state(&self, state: &SessionViewState) -> Result<()> {
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO session_view_state
                 (session_id,last_read_sequence,selected_filter,detail_level,
                  collapsed_cards,search_query,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(session_id) DO UPDATE SET
                   last_read_sequence=excluded.last_read_sequence,
                   selected_filter=excluded.selected_filter,
                   detail_level=excluded.detail_level,
                   collapsed_cards=excluded.collapsed_cards,
                   search_query=excluded.search_query,
                   updated_at=excluded.updated_at",
                params![
                    state.session_id.as_str(),
                    state.last_read_sequence as i64,
                    state.selected_filter.as_str(),
                    state.detail_level.as_str(),
                    serde_json::to_string(&state.collapsed_cards)?,
                    state.search_query,
                    crate::now_rfc3339(),
                ],
            )?;
            Ok(())
        })
    }

    pub fn mark_read(&self, session_id: &str, sequence: u64) -> Result<()> {
        let mut state = self.view_state(session_id)?;
        state.last_read_sequence = state.last_read_sequence.max(sequence);
        self.save_view_state(&state)
    }

    pub fn unread_count(&self, session_id: &str) -> Result<u64> {
        let state = self.view_state(session_id)?;
        Ok(self
            .latest_sequence(session_id)?
            .saturating_sub(state.last_read_sequence))
    }

    pub fn save_manifest(&self, manifest: &ContextManifest) -> Result<()> {
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO context_manifests
                 (id,session_id,turn_id,trace_id,created_at,provider,model,estimated,
                  provider_input_tokens,total_tokens,reserved_output_tokens,context_window,
                  categories_json,omissions_json,payload)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
                 ON CONFLICT(id) DO UPDATE SET
                   estimated=excluded.estimated,
                   provider_input_tokens=excluded.provider_input_tokens,
                   total_tokens=excluded.total_tokens,
                   categories_json=excluded.categories_json,
                   omissions_json=excluded.omissions_json,
                   payload=excluded.payload",
                params![
                    manifest.id.as_str(),
                    manifest.session_id.as_str(),
                    manifest.turn_id.as_str(),
                    manifest.trace_id.as_str(),
                    manifest.created_at,
                    manifest.provider,
                    manifest.model,
                    i64::from(manifest.estimated),
                    manifest.provider_input_tokens.map(|tokens| tokens as i64),
                    manifest.total_tokens as i64,
                    manifest.reserved_output_tokens as i64,
                    manifest.context_window as i64,
                    serde_json::to_string(&manifest.sources)?,
                    serde_json::to_string(&manifest.omissions)?,
                    serde_json::to_string(manifest)?,
                ],
            )?;
            Ok(())
        })
    }

    pub fn latest_manifest(&self, session_id: &str) -> Result<Option<ContextManifest>> {
        self.store.with(|conn| {
            let payload: Option<String> = conn
                .query_row(
                    "SELECT payload FROM context_manifests
                     WHERE session_id=?1 ORDER BY created_at DESC LIMIT 1",
                    [session_id],
                    |row| row.get(0),
                )
                .optional()?;
            payload
                .map(|payload| serde_json::from_str(&payload).map_err(NexusError::from))
                .transpose()
        })
    }

    pub fn export_markdown(&self, session_id: &str, filter: TranscriptFilter) -> Result<String> {
        let events = self.all(session_id, filter)?;
        let mut out = format!("# NEXUS transcript `{session_id}`\n\n");
        for event in events {
            let artifacts = event
                .artifact_refs
                .iter()
                .map(|artifact| format!("`{}` ({})", artifact.id, artifact.label))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                out,
                "## {} · {} · {}\n\n{}\n",
                event.sequence,
                event.kind.type_label(),
                event.status.as_str(),
                event.summary
            )
            .expect("writing to String cannot fail");
            if let Some(text) = event.kind.text() {
                writeln!(out, "{text}\n").expect("writing to String cannot fail");
            }
            if !artifacts.is_empty() {
                writeln!(out, "Artifacts: {artifacts}\n").expect("writing to String cannot fail");
            }
        }
        Ok(out)
    }

    pub fn export_jsonl(&self, session_id: &str, filter: TranscriptFilter) -> Result<String> {
        let mut out = String::new();
        for event in self.all(session_id, filter)? {
            out.push_str(&serde_json::to_string(&event)?);
            out.push('\n');
        }
        Ok(out)
    }

    /// Project an older session into the typed timeline without mutating the
    /// legacy rows. The projection is complete, ordered, and deterministic.
    pub fn project_legacy(&self, session_id: &str) -> Result<Vec<TimelineEvent>> {
        #[derive(Debug)]
        struct Timed {
            at: String,
            order: i64,
            event: TimelineEvent,
        }

        let mut timed = self.store.with(|conn| {
            let mut out = Vec::<Timed>::new();

            let mut messages = conn.prepare(
                "SELECT id, turn, role, content, tool_call_id, tool_name, created_at
                 FROM messages WHERE session_id=?1 ORDER BY id",
            )?;
            let rows = messages.query_map([session_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?;
            for row in rows {
                let (id, turn, role, content, call_id, tool_name, at) = row?;
                let turn_id = TurnId::from(format!("legacy_turn_{turn}"));
                let trace_id = TraceId::from(format!("legacy_trace_{turn}"));
                let span_id = SpanId::from(format!("legacy_message_{id}"));
                let (kind, summary) = match role.as_str() {
                    "user" => (
                        TimelineKind::UserMessage {
                            text: content.clone(),
                        },
                        first_line(&content, 120),
                    ),
                    "assistant" => {
                        let text = legacy_assistant_text(&content);
                        (
                            TimelineKind::AssistantMessage {
                                text: text.clone(),
                                streaming: false,
                            },
                            first_line(&text, 120),
                        )
                    }
                    "tool" => (
                        TimelineKind::ToolExecution {
                            tool: tool_name.unwrap_or_else(|| "legacy tool".into()),
                            arguments: Value::Object(Default::default()),
                            output_preview: content.clone(),
                            exit_status: Some("completed".into()),
                            affected_paths: Vec::new(),
                        },
                        format!(
                            "tool result {}",
                            call_id.unwrap_or_else(|| format!("message {id}"))
                        ),
                    ),
                    _ => (
                        TimelineKind::Notice {
                            text: content.clone(),
                            severity: "info".into(),
                        },
                        first_line(&content, 120),
                    ),
                };
                let mut event = TimelineEvent::new(
                    SessionId::from(session_id),
                    turn_id,
                    trace_id,
                    span_id,
                    None,
                    LifecyclePhase::Message,
                    TimelineStatus::Completed,
                    summary,
                    kind,
                );
                event.id = format!("legacy_message_{id}");
                event.timestamp = at.clone();
                event.source = TimelineSource::LegacyProjection;
                out.push(Timed {
                    at,
                    order: id,
                    event,
                });
            }

            let mut tools = conn.prepare(
                "SELECT id, trace_id, tool, arguments, risk, decision, exit_status,
                        output_preview, artifact_id, started_at, finished_at, duration_ms
                 FROM tool_calls WHERE session_id=?1 ORDER BY started_at, id",
            )?;
            let rows = tools.query_map([session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                ))
            })?;
            for (index, row) in rows.enumerate() {
                let (
                    id,
                    trace,
                    tool,
                    arguments,
                    risk,
                    decision,
                    exit_status,
                    output,
                    artifact,
                    started,
                    finished,
                    duration,
                ) = row?;
                let status = exit_status
                    .as_deref()
                    .map(TimelineStatus::parse)
                    .unwrap_or(TimelineStatus::Completed);
                let mut event = TimelineEvent::new(
                    SessionId::from(session_id),
                    TurnId::from("legacy_turn_tool"),
                    TraceId::from(trace),
                    SpanId::from(format!("legacy_tool_{id}")),
                    None,
                    LifecyclePhase::Completed,
                    status,
                    format!("{tool} · {decision}"),
                    TimelineKind::ToolExecution {
                        tool,
                        arguments: serde_json::from_str(&arguments).unwrap_or(Value::Null),
                        output_preview: output,
                        exit_status,
                        affected_paths: Vec::new(),
                    },
                );
                event.id = format!("legacy_tool_{id}");
                event.timestamp = finished.unwrap_or_else(|| started.clone());
                event.duration_ms = duration.map(|value| value.max(0) as u64);
                event.risk = Some(risk);
                event.source = TimelineSource::LegacyProjection;
                if let Some(artifact) = artifact {
                    event.artifact_refs.push(ArtifactReference {
                        id: artifact,
                        kind: "tool_output".into(),
                        label: "full tool output".into(),
                        bytes: None,
                        content_type: None,
                    });
                }
                out.push(Timed {
                    at: started,
                    order: 1_000_000 + index as i64,
                    event,
                });
            }

            let mut approvals = conn.prepare(
                "SELECT id, tool, summary, risk, requested_at, resolved_at,
                        approved, edited_command
                 FROM approvals WHERE session_id=?1 ORDER BY requested_at, id",
            )?;
            let rows = approvals.query_map([session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<bool>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            })?;
            for (index, row) in rows.enumerate() {
                let (id, tool, summary, risk, requested, resolved, approved, edited) = row?;
                let decision = approved.map(|approved| {
                    if approved {
                        "approved".to_string()
                    } else {
                        "denied".to_string()
                    }
                });
                let status = match approved {
                    Some(true) => TimelineStatus::Completed,
                    Some(false) => TimelineStatus::Blocked,
                    None => TimelineStatus::Waiting,
                };
                let mut event = TimelineEvent::new(
                    SessionId::from(session_id),
                    TurnId::from("legacy_turn_approval"),
                    TraceId::from(format!("legacy_trace_approval_{id}")),
                    SpanId::from(format!("legacy_approval_{id}")),
                    None,
                    LifecyclePhase::Approval,
                    status,
                    summary.clone(),
                    TimelineKind::Approval {
                        tool,
                        decision,
                        summary,
                        edited: edited.is_some(),
                    },
                );
                event.id = format!("legacy_approval_{id}");
                event.timestamp = resolved.unwrap_or_else(|| requested.clone());
                event.risk = Some(risk);
                event.source = TimelineSource::LegacyProjection;
                out.push(Timed {
                    at: requested,
                    order: 2_000_000 + index as i64,
                    event,
                });
            }

            let mut audit = conn.prepare(
                "SELECT id, trace_id, at, kind, payload
                 FROM audit_events WHERE session_id=?1 ORDER BY id",
            )?;
            let rows = audit.query_map([session_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?;
            for row in rows {
                let (id, trace, at, audit_kind, payload) = row?;
                let mut event = TimelineEvent::new(
                    SessionId::from(session_id),
                    TurnId::from("legacy_turn_audit"),
                    TraceId::from(trace),
                    SpanId::from(format!("legacy_audit_{id}")),
                    None,
                    LifecyclePhase::Completed,
                    if audit_kind == "failure" {
                        TimelineStatus::Failed
                    } else {
                        TimelineStatus::Completed
                    },
                    audit_kind.replace('_', " "),
                    TimelineKind::LegacyAudit {
                        audit_kind,
                        payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                    },
                );
                event.id = format!("legacy_audit_{id}");
                event.timestamp = at.clone();
                event.source = TimelineSource::LegacyProjection;
                out.push(Timed {
                    at,
                    order: 3_000_000 + id,
                    event,
                });
            }
            Ok(out)
        })?;
        timed.sort_by(|a, b| a.at.cmp(&b.at).then(a.order.cmp(&b.order)));
        let mut events = Vec::with_capacity(timed.len());
        for (index, mut item) in timed.into_iter().enumerate() {
            item.event.sequence = index as u64 + 1;
            events.push(item.event);
        }
        Ok(events)
    }
}

fn fts_query(query: &str) -> String {
    query
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|token| !token.is_empty())
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<TimelineEvent> {
    let payload: String = row.get(12)?;
    let artifacts: String = row.get(13)?;
    Ok(TimelineEvent {
        id: row.get(0)?,
        session_id: SessionId::from(row.get::<_, String>(1)?),
        turn_id: TurnId::from(row.get::<_, String>(2)?),
        trace_id: TraceId::from(row.get::<_, String>(3)?),
        span_id: SpanId::from(row.get::<_, String>(4)?),
        parent_span_id: row.get::<_, Option<String>>(5)?.map(SpanId::from),
        sequence: row.get::<_, i64>(6)?.max(0) as u64,
        timestamp: row.get(7)?,
        phase: LifecyclePhase::parse(&row.get::<_, String>(8)?),
        status: TimelineStatus::parse(&row.get::<_, String>(9)?),
        duration_ms: row
            .get::<_, Option<i64>>(10)?
            .map(|value| value.max(0) as u64),
        summary: row.get(11)?,
        kind: serde_json::from_str(&payload).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                12,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        artifact_refs: serde_json::from_str(&artifacts).unwrap_or_default(),
        risk: row.get(14)?,
        source: TimelineSource::parse(&row.get::<_, String>(15)?),
    })
}

fn first_line(text: &str, max_chars: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let mut value: String = line.chars().take(max_chars).collect();
    if line.chars().count() > max_chars {
        value.push('…');
    }
    value
}

fn legacy_assistant_text(content: &str) -> String {
    serde_json::from_str::<Value>(content)
        .ok()
        .and_then(|value| value.get("text").and_then(Value::as_str).map(String::from))
        .unwrap_or_else(|| content.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::{WorkBreakdown, WorkEstimate};

    fn seeded() -> (Store, SessionId, TimelineStore) {
        let store = Store::open_in_memory().expect("store");
        let id = SessionId::generate();
        let now = crate::now_rfc3339();
        store
            .with(|conn| {
                conn.execute(
                    "INSERT INTO sessions
                     (id,title,workspace,created_at,updated_at,model,agent,status)
                     VALUES (?1,'','/ws',?2,?2,'m','a','active')",
                    params![id.as_str(), now],
                )?;
                Ok(())
            })
            .expect("session");
        let timeline = TimelineStore::new(store.clone());
        (store, id, timeline)
    }

    fn event(session: &SessionId, kind: TimelineKind, summary: &str) -> TimelineEvent {
        TimelineEvent::new(
            session.clone(),
            TurnId::generate(),
            TraceId::generate(),
            SpanId::generate(),
            None,
            LifecyclePhase::Completed,
            TimelineStatus::Completed,
            summary,
            kind,
        )
    }

    #[test]
    fn native_events_are_ordered_and_filterable() {
        let (_store, session, timeline) = seeded();
        timeline
            .append(event(
                &session,
                TimelineKind::UserMessage {
                    text: "hello".into(),
                },
                "hello",
            ))
            .expect("append");
        timeline
            .append(event(
                &session,
                TimelineKind::ToolExecution {
                    tool: "fs.read".into(),
                    arguments: serde_json::json!({"path":"a"}),
                    output_preview: "ok".into(),
                    exit_status: Some("ok".into()),
                    affected_paths: vec!["a".into()],
                },
                "read a",
            ))
            .expect("append");
        let all = timeline
            .all(session.as_str(), TranscriptFilter::All)
            .expect("all");
        assert_eq!(all.len(), 2);
        assert_eq!((all[0].sequence, all[1].sequence), (1, 2));
        assert_eq!(
            timeline
                .all(session.as_str(), TranscriptFilter::Messages)
                .expect("messages")
                .len(),
            1
        );
        assert_eq!(
            timeline
                .search(session.as_str(), "fs.read", TranscriptFilter::All)
                .expect("search")
                .len(),
            1
        );
    }

    #[test]
    fn running_card_updates_without_changing_sequence() {
        let (_store, session, timeline) = seeded();
        let mut card = event(
            &session,
            TimelineKind::AssistantMessage {
                text: "partial".into(),
                streaming: true,
            },
            "assistant response",
        );
        card.status = TimelineStatus::Running;
        card = timeline.append(card).expect("append");
        card.status = TimelineStatus::Completed;
        card.kind = TimelineKind::AssistantMessage {
            text: "partial and complete".into(),
            streaming: false,
        };
        timeline.update(&card).expect("update");
        let loaded = timeline.get(&card.id).expect("get");
        assert_eq!(loaded.sequence, 1);
        assert_eq!(loaded.status, TimelineStatus::Completed);
        assert!(loaded.searchable_text().contains("complete"));
    }

    #[test]
    fn cancellation_closes_running_cards_without_changing_identity() {
        let (_store, session, timeline) = seeded();
        let mut card = event(
            &session,
            TimelineKind::AssistantMessage {
                text: "partial response".into(),
                streaming: true,
            },
            "assistant response",
        );
        card.status = TimelineStatus::Running;
        card = timeline.append(card).expect("append");
        assert_eq!(
            timeline
                .cancel_running(session.as_str(), "operator cancelled")
                .expect("cancel"),
            1
        );
        let loaded = timeline.get(&card.id).expect("get");
        assert_eq!(loaded.status, TimelineStatus::Cancelled);
        assert_eq!(loaded.phase, LifecyclePhase::Cancelled);
        assert!(matches!(
            loaded.kind,
            TimelineKind::AssistantMessage {
                streaming: false,
                ..
            }
        ));
    }

    #[test]
    fn view_state_and_unread_count_are_durable() {
        let (_store, session, timeline) = seeded();
        timeline
            .append(event(
                &session,
                TimelineKind::Notice {
                    text: "one".into(),
                    severity: "info".into(),
                },
                "one",
            ))
            .expect("append");
        assert_eq!(timeline.unread_count(session.as_str()).expect("unread"), 1);
        let mut state = timeline.view_state(session.as_str()).expect("state");
        state.detail_level = TranscriptDetail::Raw;
        state.selected_filter = TranscriptFilter::Tools;
        state.last_read_sequence = 1;
        timeline.save_view_state(&state).expect("save");
        assert_eq!(timeline.unread_count(session.as_str()).expect("unread"), 0);
        let loaded = timeline.view_state(session.as_str()).expect("load");
        assert_eq!(loaded.detail_level, TranscriptDetail::Raw);
        assert_eq!(loaded.selected_filter, TranscriptFilter::Tools);
    }

    #[test]
    fn legacy_sessions_project_all_messages_and_tools() {
        let (store, session, timeline) = seeded();
        store
            .with(|conn| {
                conn.execute(
                    "INSERT INTO messages
                     (session_id,turn,role,content,created_at)
                     VALUES (?1,1,'user','hello','2026-01-01T00:00:00Z')",
                    [session.as_str()],
                )?;
                conn.execute(
                    "INSERT INTO messages
                     (session_id,turn,role,content,created_at)
                     VALUES (?1,1,'assistant','world','2026-01-01T00:00:01Z')",
                    [session.as_str()],
                )?;
                conn.execute(
                    "INSERT INTO tool_calls
                     (id,session_id,trace_id,tool,arguments,risk,decision,
                      exit_status,output_preview,started_at,finished_at,duration_ms)
                     VALUES ('call_1',?1,'trace_1','fs.read','{}','read','allow',
                             'ok','content','2026-01-01T00:00:02Z',
                             '2026-01-01T00:00:03Z',10)",
                    [session.as_str()],
                )?;
                Ok(())
            })
            .expect("legacy rows");
        let projected = timeline
            .all(session.as_str(), TranscriptFilter::All)
            .expect("project");
        assert_eq!(projected.len(), 3);
        assert!(projected
            .iter()
            .all(|event| event.source == TimelineSource::LegacyProjection));
        assert_eq!(projected.last().map(|event| event.sequence), Some(3));
    }

    #[test]
    fn paged_resume_reconstructs_more_than_thirty_events() {
        let (_store, session, timeline) = seeded();
        for index in 1..=250 {
            timeline
                .append(event(
                    &session,
                    TimelineKind::Notice {
                        text: format!("event {index}"),
                        severity: "info".into(),
                    },
                    &format!("event {index}"),
                ))
                .expect("append");
        }
        let latest = timeline
            .page(session.as_str(), None, 100, TranscriptFilter::All)
            .expect("latest");
        assert_eq!(
            (
                latest.first().map(|event| event.sequence),
                latest.last().map(|event| event.sequence)
            ),
            (Some(151), Some(250))
        );
        let middle = timeline
            .page(
                session.as_str(),
                latest.first().map(|event| event.sequence),
                100,
                TranscriptFilter::All,
            )
            .expect("middle");
        let oldest = timeline
            .page(
                session.as_str(),
                middle.first().map(|event| event.sequence),
                100,
                TranscriptFilter::All,
            )
            .expect("oldest");
        assert_eq!(oldest.len() + middle.len() + latest.len(), 250);
        assert_eq!(oldest.first().map(|event| event.sequence), Some(1));
    }

    #[test]
    fn filtered_paging_scans_until_the_requested_match_count() {
        let (_store, session, timeline) = seeded();
        for index in 1..=500 {
            let kind = if index % 50 == 0 {
                TimelineKind::Error {
                    class: "test".into(),
                    message: format!("error {index}"),
                    retryable: false,
                }
            } else {
                TimelineKind::Notice {
                    text: format!("event {index}"),
                    severity: "info".into(),
                }
            };
            timeline
                .append(event(&session, kind, &format!("event {index}")))
                .expect("append");
        }

        let errors = timeline
            .page(session.as_str(), None, 5, TranscriptFilter::Errors)
            .expect("filtered page");
        assert_eq!(errors.len(), 5);
        assert_eq!(
            errors
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![300, 350, 400, 450, 500]
        );
    }

    #[test]
    fn hundred_thousand_event_timeline_meets_release_host_query_budgets() {
        use std::time::{Duration, Instant};

        let (store, session, timeline) = seeded();
        store
            .with(|connection| {
                connection.execute_batch("BEGIN IMMEDIATE")?;
                let insertion = (|| -> Result<()> {
                    let mut statement = connection.prepare(
                        "INSERT INTO timeline_events
                         (id,session_id,turn_id,trace_id,span_id,sequence,at,event_type,
                          phase,status,summary,payload,artifact_refs,source)
                         VALUES (?1,?2,'turn','trace','span',?3,?4,'notice',
                                 'completed','completed',?5,?6,'[]','native')",
                    )?;
                    for index in 1..=100_000u64 {
                        let text = if index == 99_999 {
                            "release needle 99999".to_string()
                        } else {
                            format!("timeline event {index}")
                        };
                        let payload = serde_json::to_string(&TimelineKind::Notice {
                            text: text.clone(),
                            severity: "info".into(),
                        })?;
                        statement.execute(params![
                            format!("event_{index}"),
                            session.as_str(),
                            index as i64,
                            crate::now_rfc3339(),
                            text,
                            payload,
                        ])?;
                    }
                    Ok(())
                })();
                match insertion {
                    Ok(()) => connection.execute_batch("COMMIT")?,
                    Err(error) => {
                        let _ = connection.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                }
                Ok(())
            })
            .expect("bulk insert");

        let latest_started = Instant::now();
        let latest = timeline
            .page(session.as_str(), None, 100, TranscriptFilter::All)
            .expect("latest page");
        let latest_elapsed = latest_started.elapsed();
        assert_eq!(latest.len(), 100);
        assert!(
            latest_elapsed < Duration::from_millis(50),
            "latest page took {latest_elapsed:?}"
        );

        let search_started = Instant::now();
        let hits = timeline
            .search_hits(
                session.as_str(),
                "release needle 99999",
                TranscriptFilter::All,
                10,
            )
            .expect("indexed search");
        let search_elapsed = search_started.elapsed();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].sequence, 99_999);
        assert!(
            search_elapsed < Duration::from_millis(100),
            "indexed search took {search_elapsed:?}"
        );

        let surrounding = timeline
            .page_around(session.as_str(), hits[0].sequence, 2, TranscriptFilter::All)
            .expect("surrounding page");
        assert_eq!(
            surrounding
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![99_997, 99_998, 99_999, 100_000]
        );
    }

    #[test]
    fn exports_preserve_redacted_event_stream() {
        let (_store, session, timeline) = seeded();
        timeline
            .append(event(
                &session,
                TimelineKind::WorkBreakdown {
                    breakdown: WorkBreakdown::generate(
                        "read a file",
                        WorkEstimate {
                            predicted_actions: 1,
                            predictable: true,
                            ..Default::default()
                        },
                    ),
                },
                "direct work",
            ))
            .expect("append");
        let markdown = timeline
            .export_markdown(session.as_str(), TranscriptFilter::All)
            .expect("markdown");
        assert!(markdown.contains("work_breakdown"));
        let jsonl = timeline
            .export_jsonl(session.as_str(), TranscriptFilter::All)
            .expect("jsonl");
        assert_eq!(jsonl.lines().count(), 1);
        serde_json::from_str::<TimelineEvent>(jsonl.trim()).expect("valid json event");
    }
}
