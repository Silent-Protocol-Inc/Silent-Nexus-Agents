//! nexus-agent: the controlled agent loop.
//!
//! Pipeline per turn (see docs/architecture.md):
//! objective → classify → load policy → retrieve memory → select agent+model
//! → select minimal tools → plan → request action → parse → validate schema
//! → evaluate policy → approve if required → execute in sandbox → capture →
//! redact → audit → verify → continue/recover/finish.
//!
//! The model can never bypass schema validation, capability checks, path
//! restrictions, approval policy, sandboxing, timeouts, output limits, secret
//! redaction, or audit logging — every one of those lives on the harness side
//! of this boundary.

pub mod action;
pub mod agents;
pub mod classify;
pub mod custom_agents;
pub mod loop_engine;
pub mod session;
pub mod subagent;
pub mod thinking;

pub use action::{AgentAction, COMPAT_INSTRUCTIONS};
pub use agents::AgentRole;
pub use custom_agents::{AgentCatalog, CustomAgentDefinition};
pub use loop_engine::{
    AgentLoop, ApprovalDecision, ApprovalHandler, CacheTokens, LoopEvent, LoopOutcome,
    PlanDecision, PlanReviewRequest, PlanReviewResponse, PlanReviewStage, TurnLimits,
};
pub use session::{SessionMeta, SessionStore, SessionUsage};

use nexus_core::redact::Redactor;
use nexus_core::store::Store;
use nexus_models::ModelManager;
use nexus_observability::AuditLog;
use nexus_tools::{ToolContext, ToolRegistry};
use std::sync::Arc;

/// Shared runtime services the agent loop depends on.
#[derive(Clone)]
pub struct AgentRuntime {
    pub models: Arc<ModelManager>,
    pub tools: Arc<ToolRegistry>,
    pub policy: Arc<nexus_policy::PolicyEngine>,
    pub tool_ctx: ToolContext,
    pub audit: AuditLog,
    pub sessions: SessionStore,
    pub redactor: Arc<Redactor>,
    /// Profile cards, profile-only memory, and explicitly enabled global
    /// harness patterns live outside any one workspace database.
    pub global_store: Store,
    /// Canonical state scoped to the active workspace.
    pub store: Store,
    pub limits: TurnLimits,
    /// Current delegation nesting. The main agent starts at zero and each
    /// orchestrated child receives a runtime incremented by exactly one.
    pub recursion_depth: u8,
    /// Operator's deliberation preference. Affects optional planning,
    /// grounding, and retry tolerance only — never a safety ceiling.
    pub thinking: nexus_core::thinking::ThinkingMode,
    /// Whether `on`/`auto` may promote a turn to grounded, staged execution.
    pub deep_planning: bool,
    /// Plan mode. The turn runs under a scope that refuses to change anything
    /// and the agent's job is to author a plan for approval, not to act.
    pub plan_mode: bool,
}
