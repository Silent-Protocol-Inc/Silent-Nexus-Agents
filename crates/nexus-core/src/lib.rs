//! nexus-core: foundation types and safety primitives for Silent Nexus.
//!
//! Everything in this crate is deliberately model-agnostic and UI-agnostic.
//! Higher layers (agent loop, tools, TUI) depend on these invariants:
//!
//! * every path a tool touches passes through [`workspace::WorkspaceGuard`];
//! * every string shown to a terminal passes through [`sanitize`];
//! * every string persisted or displayed passes through [`redact::Redactor`];
//! * every state mutation is recorded through [`events::AuditEvent`].

pub mod artifacts;
pub mod atomic;
pub mod brand;
pub mod config;
pub mod error;
pub mod events;
pub mod file_formats;
pub mod git;
pub mod governance;
pub mod gpu;
pub mod harness;
pub mod ids;
pub mod instructions;
pub mod maintenance;
pub mod orchestration;
pub mod permissions;
pub mod redact;
pub mod risk;
pub mod sanitize;
pub mod secret;
pub mod store;
pub mod thinking;
pub mod timeline;
pub mod workspace;

pub use error::{NexusError, Result};
pub use harness::HarnessRepository;
pub use ids::*;
pub use risk::{Decision, RiskLevel};
pub use secret::SecretString;
pub use thinking::ThinkingMode;

/// Current UTC timestamp in RFC 3339 format with millisecond precision.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Current Unix timestamp in milliseconds.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
