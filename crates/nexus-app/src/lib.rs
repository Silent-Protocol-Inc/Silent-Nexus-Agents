//! nexus-app: the shared application service layer.
//!
//! One command registry, one parser, and one set of services power both the
//! non-interactive `snx` CLI and the interactive TUI, so the two surfaces can
//! never drift apart:
//!
//! ```text
//! Non-interactive CLI ─┐            ┌─ Interactive TUI slash commands
//!                      ▼            ▼
//!            nexus-app (registry / parse / exec / services)
//! ```

pub mod app;
pub mod boot;
pub mod claude;
pub mod clipboard;
pub mod codex;
pub mod connectors;
pub mod control_plane;
pub mod credentials;
pub mod exec;
pub mod gitx;
pub mod parse;
pub mod persona_service;
pub mod profile_capture;
pub mod profile_port;
pub mod providers;
pub mod registry;
pub mod report;
pub mod rsi;
pub mod services;
pub mod status;
pub mod uistate;
pub mod worker;

pub use app::App;
pub use control_plane::{HarnessAction, HarnessActionResult, HarnessControlPlane, LearningOutcome};
pub use exec::{apply_confirmed, execute, ConfirmedAction, Effect, ExecCtx, View};
pub use parse::{classify, Input, SlashCommand};
pub use report::{Item, Report, Sev};

/// Theme names the TUI implements. Lives here so `/theme` validation and the
/// CLI share one list with the renderer.
pub fn theme_names() -> &'static [&'static str] {
    &[
        "nexus-dark",
        "cyberpunk",
        "edgerunner",
        "ghost",
        "synthwave",
        "neon-noir",
        "acid-rain",
        "redline",
        "icewire",
        "matrix",
        "ultraviolet",
        "solar-flare",
        "mono",
    ]
}
