//! nexus-sandbox: execution isolation backends.
//!
//! Every backend reports an honest [`IsolationReport`]; the UI displays the
//! *actual* isolation level, never an aspirational one. Backends:
//!
//! * [`process::ProcessBackend`] — approval-only host process: scrubbed
//!   environment, resource limits (CPU, memory, processes), process-group
//!   kill, working-directory validation, and optional Linux network namespace.
//!   These are guardrails, not containment.
//! * [`container::ContainerBackend`] — Docker/Podman container: read-only
//!   base filesystem, workspace mount, tmpfs, no network by default.
//! * [`mock::MockBackend`] — deterministic backend for tests.
//!
//! A remote-sandbox adapter can implement [`SandboxBackend`] later; it is not
//! required for local operation.

pub mod container;
pub mod mock;
mod output;
pub mod process;

use nexus_core::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Network access mode for a sandboxed execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    Off,
    /// Network reachable; DNS/private-range filtering is enforced at the tool
    /// layer (web tools), not by the OS sandbox.
    Restricted,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationStrength {
    /// Ephemeral container with a constrained filesystem and network.
    Strong,
    /// Host process with guardrails; safety depends on one-time approval.
    ApprovalOnlyHost,
    /// Path validation only.
    None,
    /// Deterministic test double.
    Mock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilesystemAccess {
    NoWorkspace,
    ReadOnly,
    WorkspaceWrite,
}

/// What to execute.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecSpec {
    /// Program to run (no shell interpretation).
    pub program: String,
    pub args: Vec<String>,
    /// When true, run `program args…` through `sh -c` as a single string.
    /// Only used for user-approved raw command lines.
    pub shell: bool,
    /// Working directory (must already be validated by the workspace guard).
    pub cwd: PathBuf,
    /// Extra environment variables (merged over the scrubbed allowlist).
    pub env: BTreeMap<String, String>,
    /// Environment allowlist from config.
    pub env_allowlist: Vec<String>,
    /// Requested network mode.
    pub network: NetworkMode,
    /// Maximum network mode explicitly authorized for this execution.
    pub approved_network: NetworkMode,
    /// Workspace mount access granted to the execution.
    pub filesystem_access: FilesystemAccess,
    /// Workspace-relative paths hidden from model-run containers.
    pub sensitive_path_masks: Vec<PathBuf>,
    /// Required for every host-process execution originating from a model.
    pub unsafe_host_approved: bool,
    pub timeout_secs: u64,
    pub cpu_limit_secs: u64,
    pub memory_limit_mb: u64,
    /// Bytes of stdout+stderr to keep in memory before killing the process.
    pub output_hard_cap: usize,
    /// Optional stdin content.
    pub stdin: Option<String>,
}

impl ExecSpec {
    pub fn command_line(&self) -> String {
        if self.shell {
            self.program.clone()
        } else {
            let mut parts = vec![self.program.clone()];
            parts.extend(self.args.iter().cloned());
            parts.join(" ")
        }
    }

    pub fn effective_network(&self) -> NetworkMode {
        self.network.min(self.approved_network)
    }
}

/// A chunk of live output for streaming display.
#[derive(Debug, Clone)]
pub enum OutputChunk {
    Stdout(String),
    Stderr(String),
}

/// Result of a sandboxed execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecOutcome {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub timed_out: bool,
    /// True when output hit the hard cap and the process was terminated.
    pub output_capped: bool,
    /// Backend that actually ran this.
    pub backend: String,
    /// Honest isolation summary shown to the user.
    pub isolation: String,
}

/// Honest description of what a backend does and does not isolate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsolationReport {
    pub backend: String,
    pub strength: IsolationStrength,
    /// Short label surfaced in the status bar, e.g. `approval-only-host`.
    pub level: String,
    pub filesystem: String,
    pub filesystem_access: FilesystemAccess,
    pub network: String,
    pub resources: String,
    pub caveats: Vec<String>,
}

#[async_trait::async_trait]
pub trait SandboxBackend: Send + Sync {
    fn name(&self) -> &'static str;

    fn strength(&self) -> IsolationStrength;

    /// Honest isolation description for the given network mode.
    fn isolation(&self, network: NetworkMode) -> IsolationReport;

    /// Check whether this backend can run on this host right now.
    async fn availability(&self) -> std::result::Result<String, String>;

    /// Execute a spec, optionally streaming chunks to `live`.
    async fn execute(
        &self,
        spec: ExecSpec,
        live: Option<mpsc::UnboundedSender<OutputChunk>>,
    ) -> Result<ExecOutcome>;
}

/// Selects and holds the active backend.
pub struct SandboxManager {
    backend: Box<dyn SandboxBackend>,
    /// Backends that were considered and why they were not chosen.
    pub selection_notes: Vec<String>,
}

impl SandboxManager {
    /// Choose a backend according to config (`auto` prefers container, falls
    /// back to approval-only host execution). `none` is an explicit opt-out that still
    /// records honest isolation (`path-validation-only`).
    pub async fn select(preference: &str, container_image: &str) -> Result<Self> {
        let mut notes = Vec::new();
        match preference {
            "container" | "auto" => {
                let container = container::ContainerBackend::detect(container_image).await;
                match container {
                    Ok(backend) => {
                        return Ok(Self {
                            backend: Box::new(backend),
                            selection_notes: notes,
                        })
                    }
                    Err(reason) => {
                        notes.push(format!("container backend unavailable: {reason}"));
                        if preference == "container" {
                            return Err(nexus_core::NexusError::SandboxUnavailable(
                                "container".into(),
                                reason,
                            ));
                        }
                    }
                }
                notes.push("falling back to approval-only host backend".into());
                Ok(Self {
                    backend: Box::new(process::ProcessBackend::new(false)),
                    selection_notes: notes,
                })
            }
            "process" => Ok(Self {
                backend: Box::new(process::ProcessBackend::new(false)),
                selection_notes: notes,
            }),
            "none" => Ok(Self {
                backend: Box::new(process::ProcessBackend::new(true)),
                selection_notes: vec![
                    "sandbox disabled by configuration: commands run as unrestricted local processes (path validation only)".into(),
                ],
            }),
            other => Err(nexus_core::NexusError::Config(format!(
                "unknown sandbox backend `{other}`"
            ))),
        }
    }

    /// Wrap a specific backend (tests).
    pub fn with_backend(backend: Box<dyn SandboxBackend>) -> Self {
        Self {
            backend,
            selection_notes: vec![],
        }
    }

    pub fn backend(&self) -> &dyn SandboxBackend {
        self.backend.as_ref()
    }

    pub fn strong_isolation(&self) -> bool {
        self.backend.strength() == IsolationStrength::Strong
    }

    pub async fn execute(
        &self,
        spec: ExecSpec,
        live: Option<mpsc::UnboundedSender<OutputChunk>>,
    ) -> Result<ExecOutcome> {
        self.backend.execute(spec, live).await
    }
}

/// Build the scrubbed environment for a sandboxed process: allowlisted
/// variables only, and never sensitive-looking keys.
pub fn scrubbed_env(
    allowlist: &[String],
    extra: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for key in allowlist {
        if forbidden_environment_key(key) || nexus_core::redact::Redactor::is_sensitive_env_key(key)
        {
            continue;
        }
        if let Ok(v) = std::env::var(key) {
            env.insert(key.clone(), v);
        }
    }
    for (k, v) in extra {
        if !forbidden_environment_key(k) && !nexus_core::redact::Redactor::is_sensitive_env_key(k) {
            env.insert(k.clone(), v.clone());
        }
    }
    env
}

fn forbidden_environment_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper.starts_with("GIT_")
        || upper.starts_with("LD_")
        || upper.starts_with("DYLD_")
        || matches!(
            upper.as_str(),
            "BASH_ENV"
                | "ENV"
                | "CDPATH"
                | "PYTHONPATH"
                | "PYTHONHOME"
                | "RUBYOPT"
                | "PERL5OPT"
                | "NODE_OPTIONS"
                | "RUSTC_WRAPPER"
                | "RUSTC_WORKSPACE_WRAPPER"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubbed_env_drops_sensitive_keys() {
        std::env::set_var("SNX_TEST_PLAIN", "1");
        std::env::set_var("SNX_TEST_API_KEY", "secret");
        let env = scrubbed_env(
            &["SNX_TEST_PLAIN".into(), "SNX_TEST_API_KEY".into()],
            &BTreeMap::new(),
        );
        assert!(env.contains_key("SNX_TEST_PLAIN"));
        assert!(!env.contains_key("SNX_TEST_API_KEY"));
        std::env::remove_var("SNX_TEST_PLAIN");
        std::env::remove_var("SNX_TEST_API_KEY");
    }

    #[test]
    fn extra_env_also_scrubbed() {
        let mut extra = BTreeMap::new();
        extra.insert("MY_TOKEN".to_string(), "x".to_string());
        extra.insert("RUST_BACKTRACE".to_string(), "1".to_string());
        extra.insert("GIT_CONFIG_COUNT".to_string(), "1".to_string());
        let env = scrubbed_env(&[], &extra);
        assert!(!env.contains_key("MY_TOKEN"));
        assert!(!env.contains_key("GIT_CONFIG_COUNT"));
        assert!(env.contains_key("RUST_BACKTRACE"));
    }
}
