//! nexus-tools: the typed tool system.
//!
//! Every tool declares [`ToolMeta`] (schemas, risk, limits, side effects).
//! The [`ToolRegistry`] groups tools into [`ToolCategory`]s so the agent can
//! surface a *minimal* subset per turn — small local models get 5–10 relevant
//! tools, never the whole catalog.
//!
//! Execution invariants enforced here, not left to callers:
//! * arguments are validated against the input JSON Schema before dispatch;
//! * every path passes the workspace guard;
//! * output is redacted, control-sanitized, and truncated (full output goes
//!   to the artifact store);
//! * a per-tool timeout applies.

pub mod diag;
pub mod fs;
pub mod html;
pub mod memory;
pub mod net_guard;
pub mod plan;
pub mod pty;
pub mod repo;
pub mod terminal;
pub mod web;

use nexus_core::artifacts::ArtifactStore;
use nexus_core::config::Config;
use nexus_core::ids::SessionId;
use nexus_core::redact::Redactor;
use nexus_core::workspace::WorkspaceGuard;
use nexus_core::{NexusError, Result, RiskLevel};
use nexus_policy::ActionRequest;
use nexus_sandbox::SandboxManager;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Functional grouping used for lazy tool discovery and routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    Filesystem,
    Repo,
    Terminal,
    Web,
    Diagnostics,
    Memory,
    Goal,
    Mcp,
}

impl ToolCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolCategory::Filesystem => "filesystem",
            ToolCategory::Repo => "repo",
            ToolCategory::Terminal => "terminal",
            ToolCategory::Web => "web",
            ToolCategory::Diagnostics => "diagnostics",
            ToolCategory::Memory => "memory",
            ToolCategory::Goal => "goal",
            ToolCategory::Mcp => "mcp",
        }
    }
}

/// Static description of a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolMeta {
    /// Fully-qualified name, `namespace.action`, e.g. `fs.read_file`.
    pub name: String,
    pub namespace: String,
    pub description: String,
    pub category: ToolCategory,
    /// JSON Schema of the arguments object.
    pub input_schema: Value,
    /// JSON Schema of the (successful) output content shape.
    pub output_schema: Value,
    /// Base risk; `action_request` may escalate per-invocation.
    pub risk: RiskLevel,
    pub required_capabilities: Vec<String>,
    pub timeout_secs: u64,
    pub max_output_bytes: usize,
    pub deterministic: bool,
    pub needs_network: bool,
    pub needs_sandbox: bool,
    /// Human-readable side-effect declaration shown in approvals.
    pub side_effects: String,
}

/// Shared services available to executing tools.
#[derive(Clone)]
pub struct ToolContext {
    pub workspace: Arc<WorkspaceGuard>,
    pub sandbox: Arc<SandboxManager>,
    pub artifacts: ArtifactStore,
    pub redactor: Arc<Redactor>,
    pub config: Arc<Config>,
    pub store: nexus_core::store::Store,
    pub session: Option<SessionId>,
    pub authorization: ExecutionAuthorization,
}

#[derive(Clone, Default)]
pub struct ExecutionAuthorization {
    unsafe_host_once: Arc<AtomicBool>,
}

impl ExecutionAuthorization {
    pub fn authorize_unsafe_host_once(&self) {
        self.unsafe_host_once.store(true, Ordering::Release);
    }

    pub fn consume_unsafe_host_once(&self) -> bool {
        self.unsafe_host_once.swap(false, Ordering::AcqRel)
    }
}

/// Sanitized, truncated result surfaced to the model and UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    /// Set when the full output was larger than the preview and was stored.
    pub artifact_id: Option<String>,
    /// Machine-readable extras (exit codes, counts, hashes, …).
    pub metadata: Value,
    pub truncated: bool,
}

impl ToolOutput {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            artifact_id: None,
            metadata: Value::Null,
            truncated: false,
        }
    }

    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = metadata;
        self
    }
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn meta(&self) -> &ToolMeta;

    /// Build the normalized [`ActionRequest`] used for policy evaluation and
    /// approval prompts. May escalate risk based on concrete arguments.
    fn action_request(&self, args: &Value) -> Result<ActionRequest>;

    /// Execute with already-validated arguments.
    async fn execute(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutput>;
}

/// Registry of all available tools.
#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registry with every built-in tool family registered.
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        fs::register(&mut r);
        repo::register(&mut r);
        terminal::register(&mut r);
        web::register(&mut r);
        diag::register(&mut r);
        plan::register(&mut r);
        memory::register(&mut r);
        r
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.meta().name.clone(), tool);
    }

    pub fn get(&self, name: &str) -> Result<Arc<dyn Tool>> {
        self.tools
            .get(name)
            .cloned()
            .ok_or_else(|| NexusError::UnknownTool(name.to_string()))
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub fn all(&self) -> impl Iterator<Item = &Arc<dyn Tool>> {
        self.tools.values()
    }

    /// Minimal tool subset for the given categories (lazy discovery).
    pub fn for_categories(&self, categories: &[ToolCategory]) -> Vec<Arc<dyn Tool>> {
        self.tools
            .values()
            .filter(|t| categories.contains(&t.meta().category))
            .cloned()
            .collect()
    }

    /// Validate `args` against the tool's input schema; returns detailed,
    /// model-correctable error messages.
    pub fn validate_args(&self, name: &str, args: &Value) -> Result<()> {
        let tool = self.get(name)?;
        let schema = &tool.meta().input_schema;
        let validator = jsonschema::validator_for(schema).map_err(|e| NexusError::ToolInput {
            tool: name.to_string(),
            message: format!("schema compile error: {e}"),
        })?;
        let errors: Vec<String> = validator
            .iter_errors(args)
            .map(|e| format!("{} (at {})", e, e.instance_path))
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(NexusError::ToolInput {
                tool: name.to_string(),
                message: errors.join("; "),
            })
        }
    }
}

/// Post-process raw tool output: redact, sanitize, truncate, store artifact.
pub async fn finalize_output(
    ctx: &ToolContext,
    meta: &ToolMeta,
    raw: String,
    metadata: Value,
) -> Result<ToolOutput> {
    let redacted = ctx.redactor.redact(&raw);
    let sanitized = nexus_core::sanitize::sanitize_terminal(&redacted);
    let (preview, truncated) =
        nexus_core::sanitize::truncate_output(&sanitized, meta.max_output_bytes);
    let artifact_id = if truncated {
        let record = ctx.artifacts.put(
            ctx.session.as_ref(),
            "tool_output",
            "text/plain",
            sanitized.as_bytes(),
            None,
        )?;
        Some(record.id.as_str().to_string())
    } else {
        None
    };
    Ok(ToolOutput {
        content: preview,
        artifact_id,
        metadata,
        truncated,
    })
}

/// Helper: JSON schema for an object with required string fields and
/// optional extras. Keeps tool definitions terse and consistent.
pub fn object_schema(required: &[(&str, &str, &str)], optional: &[(&str, &str, &str)]) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required_names = Vec::new();
    for (name, ty, desc) in required.iter().chain(optional.iter()) {
        let mut prop = serde_json::Map::new();
        match *ty {
            "string[]" => {
                prop.insert("type".into(), "array".into());
                prop.insert("items".into(), serde_json::json!({"type": "string"}));
            }
            other => {
                prop.insert("type".into(), other.into());
            }
        }
        prop.insert("description".into(), (*desc).into());
        properties.insert((*name).to_string(), Value::Object(prop));
    }
    for (name, _, _) in required {
        required_names.push((*name).to_string());
    }
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required_names,
        "additionalProperties": false
    })
}

/// Convert registry tools into model-facing specs.
pub fn to_model_specs(tools: &[Arc<dyn Tool>]) -> Vec<nexus_models_spec::ToolSpecLite> {
    tools
        .iter()
        .map(|t| nexus_models_spec::ToolSpecLite {
            name: t.meta().name.clone(),
            description: t.meta().description.clone(),
            parameters: t.meta().input_schema.clone(),
        })
        .collect()
}

/// Tiny mirror of nexus-models' ToolSpec to avoid a dependency cycle;
/// nexus-agent converts between them.
pub mod nexus_models_spec {
    #[derive(Debug, Clone)]
    pub struct ToolSpecLite {
        pub name: String,
        pub description: String,
        pub parameters: serde_json::Value,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use nexus_core::store::Store;

    /// Full ToolContext over a temp workspace with a mock sandbox.
    pub fn context(dir: &std::path::Path) -> ToolContext {
        let store = Store::open_in_memory().expect("store");
        let artifacts =
            ArtifactStore::new(&dir.join(".nexus/state"), store.clone()).expect("artifacts");
        let authorization = ExecutionAuthorization::default();
        authorization.authorize_unsafe_host_once();
        ToolContext {
            workspace: Arc::new(WorkspaceGuard::new(dir, &[]).expect("guard")),
            sandbox: Arc::new(SandboxManager::with_backend(Box::new(
                nexus_sandbox::process::ProcessBackend::new(false),
            ))),
            artifacts,
            redactor: Arc::new(Redactor::new()),
            config: Arc::new(Config::default()),
            store,
            session: None,
            authorization,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The loop offers a plan-mode turn the categories it needs and then keeps
    /// only what the policy scope would allow anyway, so the model is never
    /// shown a tool that will be refused. This pins that intersection against
    /// the real builtin registry: a new write tool landing in `Filesystem`
    /// must not quietly appear in a planning turn.
    #[test]
    fn plan_mode_is_offered_reading_tools_and_the_submission_tool_only() {
        let r = ToolRegistry::with_builtins();
        let scope = nexus_policy::PolicyScope::plan_mode();
        let offered: Vec<String> = r
            .for_categories(&[
                ToolCategory::Filesystem,
                ToolCategory::Repo,
                ToolCategory::Diagnostics,
                ToolCategory::Goal,
            ])
            .into_iter()
            .filter(|t| {
                scope
                    .allowed_tool_prefixes
                    .iter()
                    .any(|prefix| t.meta().name.starts_with(prefix.as_str()))
            })
            .map(|t| t.meta().name.to_string())
            .collect();

        assert!(
            offered.iter().any(|name| name == "plan.submit"),
            "planning has no way to submit its plan: {offered:?}"
        );
        assert!(
            offered.iter().any(|name| name == "fs.read_file"),
            "planning cannot read the repository: {offered:?}"
        );
        for tool in &offered {
            assert!(
                !tool.contains("write")
                    && !tool.contains("edit")
                    && !tool.contains("delete")
                    && !tool.contains("apply"),
                "`{tool}` can change the workspace and must not be offered while planning"
            );
        }
        assert!(
            offered.iter().all(|name| name != "repo.check"),
            "repo.check runs builds and tests; planning is read-only: {offered:?}"
        );
    }

    #[test]
    fn registry_groups_by_category() {
        let r = ToolRegistry::with_builtins();
        let fs_tools = r.for_categories(&[ToolCategory::Filesystem]);
        assert!(fs_tools.len() >= 10);
        assert!(fs_tools
            .iter()
            .all(|t| t.meta().category == ToolCategory::Filesystem));
        let subset = r.for_categories(&[ToolCategory::Diagnostics]);
        assert!(subset.len() < fs_tools.len());
    }

    #[test]
    fn unknown_tool_is_a_typed_error() {
        let r = ToolRegistry::with_builtins();
        assert!(matches!(
            r.get("fs.expunge_everything"),
            Err(NexusError::UnknownTool(_))
        ));
    }

    #[test]
    fn args_validated_against_schema() {
        let r = ToolRegistry::with_builtins();
        // Missing required `path`.
        let err = r
            .validate_args("fs.read_file", &serde_json::json!({}))
            .expect_err("must fail");
        assert!(err.to_string().contains("path"));
        // Unknown extra property rejected.
        assert!(r
            .validate_args(
                "fs.read_file",
                &serde_json::json!({"path": "x", "bogus": 1})
            )
            .is_err());
        assert!(r
            .validate_args("fs.read_file", &serde_json::json!({"path": "src/main.rs"}))
            .is_ok());
    }
}
