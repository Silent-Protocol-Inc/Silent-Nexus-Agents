//! The RSI observation taxonomy.
//!
//! Observations are structured [`nexus_core::harness::HarnessEvent`]s, not
//! free-form logs. `event_type` values are stable strings (namespaced `rsi.`)
//! so later phases — evaluation, candidate generation, replay — can query and
//! group evidence deterministically. Severity is orthogonal and drives triage.

/// Stable `event_type` values. Changing a string is a data-migration concern, so
/// treat these as an append-only vocabulary.
pub mod event_type {
    pub const TASK_COMPLETED: &str = "rsi.task_completed";
    pub const TASK_FAILED: &str = "rsi.task_failed";
    pub const TEST_RESULT: &str = "rsi.test_result";
    pub const LINT_RESULT: &str = "rsi.lint_result";
    pub const TOOL_SUCCESS: &str = "rsi.tool_success";
    pub const TOOL_FAILURE: &str = "rsi.tool_failure";
    pub const REPEATED_READ: &str = "rsi.repeated_read";
    pub const INVALID_PATH: &str = "rsi.invalid_path";
    pub const UNSUPPORTED_COMMAND: &str = "rsi.unsupported_command";
    pub const PERMISSION_DENIED: &str = "rsi.permission_denied";
    pub const USER_CORRECTION: &str = "rsi.user_correction";
    pub const PLAN_DEVIATION: &str = "rsi.plan_deviation";
    pub const REVERTED_PATCH: &str = "rsi.reverted_patch";
    pub const CONTEXT_OVERFLOW: &str = "rsi.context_overflow";
    pub const TOKEN_USAGE: &str = "rsi.token_usage";
    pub const CACHE_PERFORMANCE: &str = "rsi.cache_performance";
    pub const LATENCY: &str = "rsi.latency";
    pub const REVIEWER_REJECTION: &str = "rsi.reviewer_rejection";
    pub const STALE_MEMORY: &str = "rsi.stale_memory";
    pub const FAILED_ASSUMPTION: &str = "rsi.failed_assumption";
    pub const RETRY: &str = "rsi.retry";
    pub const PROVIDER_FAILURE: &str = "rsi.provider_failure";
    pub const MODEL_FAILURE: &str = "rsi.model_failure";
    pub const AGENT_ROLE_FAILURE: &str = "rsi.agent_role_failure";
}

/// Severity for RSI triage. Higher severities are the strongest signal that a
/// candidate improvement may be warranted.
pub mod severity {
    pub const INFO: &str = "info";
    pub const NOTICE: &str = "notice";
    pub const WARNING: &str = "warning";
    pub const ERROR: &str = "error";
}
