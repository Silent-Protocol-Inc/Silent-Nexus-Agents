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

pub use action::{AgentAction, COMPAT_INSTRUCTIONS};
pub use agents::AgentRole;
pub use custom_agents::{AgentCatalog, CustomAgentDefinition};
pub use loop_engine::{
    AgentLoop, ApprovalDecision, ApprovalHandler, LoopEvent, LoopOutcome, TurnLimits,
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
    pub store: Store,
    pub limits: TurnLimits,
}
