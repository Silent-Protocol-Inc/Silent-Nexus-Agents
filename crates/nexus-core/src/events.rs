//! Structured audit events.
//!
//! Every consequential action produces an [`AuditEvent`] persisted to the
//! `audit_events` table (see `migrations/`). Events carry a [`TraceId`] so an
//! entire agent turn — model request, policy decision, approval, sandbox
//! execution, verification — can be reconstructed in order.

use crate::ids::{GoalId, SessionId, ToolCallId, TraceId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub enum AuditKind {
    SessionStarted {
        workspace: String,
    },
    SessionEnded {
        reason: String,
    },
    ModelRequested {
        model: String,
        provider: String,
        input_tokens_est: usize,
    },
    ModelResponded {
        model: String,
        output_tokens_est: usize,
        latency_ms: u64,
    },
    ModelRouted {
        task_class: String,
        model: String,
        reason: String,
    },
    ContextCompacted {
        before_tokens: usize,
        after_tokens: usize,
        preserved: Vec<String>,
    },
    ToolRequested {
        tool: String,
        call_id: ToolCallId,
        risk: String,
        summary: String,
    },
    PolicyDecision {
        tool: String,
        decision: String,
        layer: String,
        reason: String,
    },
    ApprovalRequested {
        tool: String,
        summary: String,
    },
    ApprovalResolved {
        tool: String,
        approved: bool,
        edited: bool,
    },
    ApprovalGrantChanged {
        operation: String,
        scope: String,
        token: String,
    },
    SandboxExecuted {
        backend: String,
        isolation: String,
        exit_code: Option<i32>,
        duration_ms: u64,
    },
    FileMutated {
        path: String,
        operation: String,
        bytes: usize,
    },
    NetworkAccess {
        url: String,
        method: String,
        status: Option<u16>,
        bytes: usize,
    },
    GoalTransition {
        goal_id: GoalId,
        from: String,
        to: String,
        reason: String,
    },
    RetryAttempt {
        context: String,
        attempt: u32,
        max: u32,
    },
    Failure {
        context: String,
        error: String,
    },
    VerificationResult {
        subject: String,
        passed: bool,
        evidence: String,
    },
    McpServerEvent {
        server: String,
        event: String,
    },
    MemoryEvent {
        operation: String,
        memory_kind: String,
    },
    SkillEvent {
        skill: String,
        operation: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub trace_id: TraceId,
    pub session_id: Option<SessionId>,
    pub timestamp: String,
    #[serde(flatten)]
    pub kind: AuditKind,
}

impl AuditEvent {
    pub fn new(trace_id: TraceId, session_id: Option<SessionId>, kind: AuditKind) -> Self {
        Self {
            trace_id,
            session_id,
            timestamp: crate::now_rfc3339(),
            kind,
        }
    }

    /// Short machine-readable label for the event kind (used as the DB column
    /// and for filtering in `snx audit inspect`).
    pub fn kind_label(&self) -> &'static str {
        match &self.kind {
            AuditKind::SessionStarted { .. } => "session_started",
            AuditKind::SessionEnded { .. } => "session_ended",
            AuditKind::ModelRequested { .. } => "model_requested",
            AuditKind::ModelResponded { .. } => "model_responded",
            AuditKind::ModelRouted { .. } => "model_routed",
            AuditKind::ContextCompacted { .. } => "context_compacted",
            AuditKind::ToolRequested { .. } => "tool_requested",
            AuditKind::PolicyDecision { .. } => "policy_decision",
            AuditKind::ApprovalRequested { .. } => "approval_requested",
            AuditKind::ApprovalResolved { .. } => "approval_resolved",
            AuditKind::ApprovalGrantChanged { .. } => "approval_grant_changed",
            AuditKind::SandboxExecuted { .. } => "sandbox_executed",
            AuditKind::FileMutated { .. } => "file_mutated",
            AuditKind::NetworkAccess { .. } => "network_access",
            AuditKind::GoalTransition { .. } => "goal_transition",
            AuditKind::RetryAttempt { .. } => "retry_attempt",
            AuditKind::Failure { .. } => "failure",
            AuditKind::VerificationResult { .. } => "verification_result",
            AuditKind::McpServerEvent { .. } => "mcp_server_event",
            AuditKind::MemoryEvent { .. } => "memory_event",
            AuditKind::SkillEvent { .. } => "skill_event",
        }
    }
}
