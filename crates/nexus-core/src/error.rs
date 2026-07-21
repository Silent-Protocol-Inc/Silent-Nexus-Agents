use thiserror::Error;

pub type Result<T, E = NexusError> = std::result::Result<T, E>;

/// Unified error type for Silent Nexus subsystems.
///
/// Variants are grouped by which layer produces them; the agent loop uses the
/// grouping to decide whether an error is recoverable (fed back to the model),
/// a policy stop (surfaced for approval), or fatal (aborts the turn).
#[derive(Debug, Error)]
pub enum NexusError {
    // --- Configuration ---
    #[error("configuration error: {0}")]
    Config(String),
    #[error("configuration file {path}: {message}")]
    ConfigFile { path: String, message: String },

    // --- Safety boundaries ---
    #[error("path escapes workspace boundary: {0}")]
    PathEscape(String),
    #[error("path is denied by policy: {0}")]
    PathDenied(String),
    #[error("action denied by policy: {0}")]
    PolicyDenied(String),
    #[error("approval required: {0}")]
    ApprovalRequired(String),
    #[error("network destination blocked: {0}")]
    NetworkBlocked(String),

    // --- Model providers ---
    #[error("provider `{provider}` error: {message}")]
    Provider { provider: String, message: String },
    #[error("model produced invalid action: {0}")]
    InvalidAction(String),
    #[error("model request timed out after {0}s")]
    ModelTimeout(u64),
    #[error(
        "no first token after {0}s — the model may still be loading; raise \
         `first_token_timeout_secs` or lower `context_window` for this model"
    )]
    ModelFirstTokenTimeout(u64),

    // --- Tools ---
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("tool `{tool}` input invalid: {message}")]
    ToolInput { tool: String, message: String },
    #[error("tool `{tool}` failed: {message}")]
    ToolFailed { tool: String, message: String },
    #[error("tool `{tool}` timed out after {seconds}s")]
    ToolTimeout { tool: String, seconds: u64 },

    // --- Sandbox ---
    #[error("sandbox error: {0}")]
    Sandbox(String),
    #[error("sandbox backend `{0}` unavailable: {1}")]
    SandboxUnavailable(String, String),

    // --- Persistence ---
    #[error("storage error: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("migration error: {0}")]
    Migration(String),
    #[error("not found: {0}")]
    NotFound(String),

    // --- Budgets & limits ---
    #[error("budget exhausted: {0}")]
    BudgetExhausted(String),
    #[error("output limit exceeded ({limit} bytes); full output stored as artifact")]
    OutputLimit { limit: usize },

    // --- Generic ---
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

impl NexusError {
    /// True when the agent loop may report this error back to the model and
    /// let it retry with corrected input (bounded by the retry budget).
    pub fn is_model_recoverable(&self) -> bool {
        matches!(
            self,
            NexusError::InvalidAction(_)
                | NexusError::UnknownTool(_)
                | NexusError::ToolInput { .. }
                | NexusError::ToolFailed { .. }
                | NexusError::NotFound(_)
                | NexusError::PathDenied(_)
                | NexusError::PathEscape(_)
        )
    }

    /// True when reissuing the same provider request may succeed.
    ///
    /// Provider transport errors do not currently carry a structured status,
    /// so HTTP statuses are parsed from the provider's stable `HTTP NNN ...`
    /// prefix. Deterministic client errors must not consume the agent retry
    /// budget by resending an identical invalid payload.
    pub fn is_provider_retryable(&self) -> bool {
        match self {
            NexusError::ModelTimeout(_) => true,
            // Deliberately not retryable. Waiting out the first-token
            // allowance and getting nothing means the model is too large for
            // the requested context or the server is wedged — neither is fixed
            // by paying that wait again, and the error says which knob to turn.
            NexusError::ModelFirstTokenTimeout(_) => false,
            NexusError::Provider { message, .. } => match provider_http_status(message) {
                Some(408 | 409 | 425 | 429) => true,
                Some(status) if status >= 500 => true,
                Some(status) if status >= 400 => false,
                _ => true,
            },
            _ => false,
        }
    }

    /// True when the error means "stop and ask the user", not "retry".
    pub fn is_policy_stop(&self) -> bool {
        matches!(
            self,
            NexusError::PolicyDenied(_)
                | NexusError::ApprovalRequired(_)
                | NexusError::BudgetExhausted(_)
        )
    }

    pub fn other(msg: impl Into<String>) -> Self {
        NexusError::Other(msg.into())
    }
}

fn provider_http_status(message: &str) -> Option<u16> {
    message
        .strip_prefix("HTTP ")?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

impl From<anyhow::Error> for NexusError {
    fn from(e: anyhow::Error) -> Self {
        NexusError::Other(format!("{e:#}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(message: &str) -> NexusError {
        NexusError::Provider {
            provider: "test".into(),
            message: message.into(),
        }
    }

    #[test]
    fn deterministic_provider_client_errors_are_not_retryable() {
        assert!(!provider("HTTP 400 Bad Request: invalid name").is_provider_retryable());
        assert!(!provider("HTTP 401 Unauthorized").is_provider_retryable());
        assert!(!provider("HTTP 403 Forbidden").is_provider_retryable());
    }

    #[test]
    fn transient_provider_failures_remain_retryable() {
        assert!(provider("request failed: connection reset").is_provider_retryable());
        assert!(provider("HTTP 408 Request Timeout").is_provider_retryable());
        assert!(provider("HTTP 429 Too Many Requests").is_provider_retryable());
        assert!(provider("HTTP 503 Service Unavailable").is_provider_retryable());
        assert!(NexusError::ModelTimeout(30).is_provider_retryable());
    }

    #[test]
    fn a_first_token_timeout_is_reported_rather_than_retried() {
        // Retrying means waiting the whole allowance again for a condition the
        // operator has to fix, so the message has to reach them the first time.
        assert!(!NexusError::ModelFirstTokenTimeout(600).is_provider_retryable());
        let text = NexusError::ModelFirstTokenTimeout(600).to_string();
        assert!(text.contains("first_token_timeout_secs"), "{text}");
        assert!(text.contains("context_window"), "{text}");
    }
}
