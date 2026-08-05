//! Application context: loads configuration and constructs every runtime
//! service that both the CLI and the TUI share, wired with the safety
//! invariants from `nexus-core` (workspace confinement, redaction, audit,
//! sandbox). This is the single place command behavior comes from — the CLI
//! and the TUI are thin front-ends over it.

use crate::credentials::CredentialStore;
use crate::uistate::UiStateFile;
use nexus_agent::{AgentRuntime, SessionStore, TurnLimits};
use nexus_core::artifacts::ArtifactStore;
use nexus_core::config::{Config, ConfigPaths, ModelConfig};
use nexus_core::redact::Redactor;
use nexus_core::store::Store;
use nexus_core::workspace::WorkspaceGuard;
use nexus_core::Result;
use nexus_goals::GoalStore;
use nexus_index::Indexer;
use nexus_memory::MemoryStore;
use nexus_models::ModelManager;
use nexus_observability::AuditLog;
use nexus_policy::PolicyEngine;
use nexus_sandbox::SandboxManager;
use nexus_tools::{ToolContext, ToolRegistry};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Everything a command needs, built once from configuration.
pub struct App {
    pub workspace: PathBuf,
    pub workspace_key: String,
    pub config: Arc<Config>,
    pub paths: ConfigPaths,
    /// Private cross-workspace store for profile cards, pure profile memory,
    /// and non-sensitive global harness patterns. It uses the same schema and
    /// migration runner as the workspace store.
    pub global_store: Store,
    pub store: Store,
    pub redactor: Arc<Redactor>,
    pub guard: Arc<WorkspaceGuard>,
    pub artifacts: ArtifactStore,
    pub sandbox: Arc<SandboxManager>,
    pub sandbox_notes: Vec<String>,
    pub no_color: bool,
    /// Secure credential storage (`<config>/auth`).
    pub credentials: CredentialStore,
    /// Persisted operator state (active model, theme, history). No secrets.
    pub ui_state: Mutex<UiStateFile>,
    /// Model the operator pinned via `/connect` / `snx model use`, when it still
    /// exists in the configuration. Pinning overrides task routing.
    pub pinned_model: Option<String>,
    /// Attended-process-only Full Access override. It is never serialized and
    /// is reset at bootstrap and whenever a session is attached/resumed.
    session_full_access: AtomicBool,
    /// Whether the operator has answered full access's one question about the
    /// safety class — privilege escalation, denied commands, locked reads.
    ///
    /// Deliberately process state and never written down. The answer covers
    /// the session, so it has to outlive a single turn (each turn builds a
    /// fresh policy engine); it must not outlive the session, so persisting it
    /// beside the ordinary approval grants would be wrong — resuming later
    /// would silently inherit it. Leaving — exit, disconnect, crash — asks
    /// again, which is what makes the answer mean "this sitting".
    full_access_safety_granted: Arc<AtomicBool>,
}

impl App {
    /// Load configuration for the current working directory and build the
    /// core services shared by every command.
    pub async fn bootstrap(no_color_flag: bool) -> Result<Self> {
        let workspace = std::env::current_dir()
            .map_err(|e| nexus_core::NexusError::Other(format!("cannot read current dir: {e}")))?;
        let workspace = workspace.canonicalize().unwrap_or(workspace);
        let bootstrap_paths = ConfigPaths::discover(&workspace)?;
        nexus_core::permissions::repair_private_tree(&bootstrap_paths.project_dir)?;
        nexus_core::permissions::repair_private_tree(&bootstrap_paths.global_dir)?;
        nexus_core::permissions::repair_private_tree(&bootstrap_paths.auth_dir)?;
        nexus_core::permissions::repair_private_tree(&bootstrap_paths.state_dir)?;
        // Full access used to be silently reverted to the defaults here, on
        // every start. `/permissions` wrote it to the managed overrides, the
        // file kept saying `full-access`, and the running process quietly did
        // not — so the mode could not be turned on at all, and nothing on any
        // surface said why. `snx config show` reported the defaults over a file
        // that said otherwise.
        //
        // Persisting it is what the operator asked for by choosing it. What
        // full access does not do is grow while nobody is watching: it never
        // makes destructive or external side effects automatic, and terminal
        // execution on a host backend still requires an attended session, so an
        // unattended or background run cannot inherit it into a host command.
        let (mut config, paths) = Config::load(&workspace)?;
        let credentials = CredentialStore::new(&paths.auth_dir);
        let ui_state = UiStateFile::load(&paths.ui_state_file)?;
        let allow_existing_codex = ui_state.state.codex_use_existing;
        let allow_existing_claude = ui_state.state.claude_use_existing;
        let codex_available = nexus_models::codex_auth::load_with_consent(allow_existing_codex)
            .ok()
            .flatten()
            .is_some();
        install_codex_default_model(&mut config, codex_available);

        // Resolve credential-store API key references before the config is
        // frozen; the secrets ride in `SecretString` (redacted Debug/serialize).
        for (name, model) in config.models.iter_mut() {
            if let Some(key_ref) = model.api_key_ref.clone() {
                match credentials.resolve_ref(&key_ref)? {
                    Some(secret) => model.resolved_api_key = Some(secret),
                    None => {
                        tracing::warn!(
                            model = %name,
                            key_ref = %key_ref,
                            "credential reference is missing; provider remains repairable through /connect"
                        );
                    }
                }
            }
            if model.auth.as_deref() == Some("codex") || model.provider == "codex" {
                model.allow_existing_codex = allow_existing_codex;
            }
            if model.provider == "claude-plan" {
                model.allow_existing_claude = allow_existing_claude;
            }
        }

        // Operator model pin: overrides task routing while the model exists.
        let pinned_model = ui_state
            .state
            .active_model
            .clone()
            .filter(|m| config.models.contains_key(m));
        if let Some(pinned) = &pinned_model {
            prefer_model_routes(&mut config, pinned);
            // A pin changes the preferred model, not the independently
            // approved fallback. Collapsing both to the same entry silently
            // disabled recovery for real sessions.
        }

        config.validate()?;
        let config = Arc::new(config);

        // State directory holds the database and artifacts for this workspace.
        nexus_core::permissions::repair_private_tree(&paths.state_dir)?;
        let store = Store::open(&paths.state_dir.join("nexus.db"))?;
        let global_state_dir = paths.global_dir.join("state");
        nexus_core::permissions::repair_private_tree(&global_state_dir)?;
        let global_store = Store::open(&global_state_dir.join("nexus.db"))?;
        // A `refreshing` row cannot survive a clean process lifetime. Convert
        // leftovers before any new refresh generation begins; live concurrent
        // refreshes are never mistaken for restart interruptions.
        global_store.with_retry(|connection| {
            connection.execute(
                "UPDATE provider_catalog_cache
                 SET health=CASE WHEN inventory_json='[]' THEN 'error' ELSE 'stale' END,
                     last_error='refresh interrupted by restart', updated_at=?1
                 WHERE health='refreshing'",
                [nexus_core::now_rfc3339()],
            )?;
            Ok(())
        })?;

        // Redactor learns every secret value the process can see so none of
        // them appear in logs, audit records, or terminal output.
        let redactor = Redactor::new();
        redactor.register_env();
        let mut uses_codex = false;
        for model in config.models.values() {
            if let Some(key) = &model.api_key_env {
                if let Ok(value) = std::env::var(key) {
                    redactor.register(&value);
                }
            }
            if let Some(secret) = &model.resolved_api_key {
                redactor.register(secret.expose());
            }
            if model.auth.as_deref() == Some("codex") {
                uses_codex = true;
            }
        }
        if uses_codex {
            if let Ok(Some(cred)) =
                nexus_models::codex_auth::load_with_consent(allow_existing_codex)
            {
                redactor.register(&cred.bearer);
            }
        }
        let redactor = Arc::new(redactor);

        // Created here so the guard shares the session's safety answer: a
        // session that has answered full access's one question can reach the
        // paths the guard otherwise keeps away from. Confinement is untouched.
        let full_access_safety_granted = Arc::new(AtomicBool::new(false));
        let guard = Arc::new(
            WorkspaceGuard::new(&workspace, &config.policy.denied_paths)?
                .with_unlock(full_access_safety_granted.clone()),
        );
        let artifacts = ArtifactStore::new(&paths.state_dir, store.clone())?;

        let sandbox_mgr =
            SandboxManager::select(&config.sandbox.backend, &config.sandbox.container_image)
                .await?;
        let sandbox_notes = sandbox_mgr.selection_notes.clone();
        let sandbox = Arc::new(sandbox_mgr);

        let no_color =
            no_color_flag || config.general.no_color || std::env::var_os("NO_COLOR").is_some();

        Ok(Self {
            workspace_key: workspace.display().to_string(),
            workspace,
            config,
            paths,
            global_store,
            store,
            redactor,
            guard,
            artifacts,
            sandbox,
            sandbox_notes,
            no_color,
            credentials,
            ui_state: Mutex::new(ui_state),
            pinned_model,
            session_full_access: AtomicBool::new(false),
            full_access_safety_granted,
        })
    }

    pub fn set_session_full_access(&self, enabled: bool) {
        self.session_full_access.store(enabled, Ordering::Release);
    }

    pub fn session_full_access(&self) -> bool {
        self.session_full_access.load(Ordering::Acquire)
    }

    /// Record that full access's safety question has been answered for this
    /// session. Replayed into every later turn's policy engine.
    pub fn grant_full_access_safety(&self) {
        self.full_access_safety_granted
            .store(true, Ordering::Release);
    }

    pub fn full_access_safety_granted(&self) -> bool {
        self.full_access_safety_granted.load(Ordering::Acquire)
    }

    pub fn reset_session_full_access(&self) {
        self.full_access_safety_granted
            .store(false, Ordering::Release);
        self.set_session_full_access(false);
    }

    /// Build the tool execution context bound to an optional session.
    pub fn tool_context(&self, session: Option<nexus_core::SessionId>) -> ToolContext {
        let mut config = (*self.config).clone();
        if self.session_full_access() {
            config.policy.reads = "allow".into();
            config.policy.writes = "allow".into();
            config.policy.commands = "allow".into();
            config.policy.network = "allow".into();
            config.policy.downloads = "allow".into();
            config.policy.read_formats.clear();
        }
        ToolContext {
            workspace: self.guard.clone(),
            sandbox: self.sandbox.clone(),
            artifacts: self.artifacts.clone(),
            redactor: self.redactor.clone(),
            config: Arc::new(config),
            store: self.store.clone(),
            session,
            authorization: nexus_tools::ExecutionAuthorization::default(),
            // Attached by `with_profile_tools` on the paths that own a
            // reference-counted handle. Absent here rather than faked: a
            // `profile.*` call with no service behind it must say so, not
            // report a success that stored nothing.
            profile: None,
        }
    }

    /// Give this runtime's `profile.*` tools a way to reach the control plane.
    ///
    /// Separate from [`App::runtime`] because the port needs an owned handle and
    /// the runtime builder only has a borrow. Every path that actually runs an
    /// agent calls this; the ones that do not — a bare registry in a test —
    /// leave the tools refusing with a reason.
    pub fn with_profile_tools(self: &Arc<Self>, mut runtime: AgentRuntime) -> AgentRuntime {
        let session = runtime
            .tool_ctx
            .session
            .as_ref()
            .map(|id| id.as_str().to_string());
        runtime.tool_ctx.profile = Some(crate::profile_port::AppProfilePort::new(
            self.clone(),
            session,
        ));
        runtime
    }

    /// Construct the full agent runtime for a session.
    pub fn runtime(&self, session: Option<nexus_core::SessionId>) -> Result<AgentRuntime> {
        let mut runtime_config = (*self.config).clone();
        if self.session_full_access() {
            runtime_config.policy.reads = "allow".into();
            runtime_config.policy.writes = "allow".into();
            runtime_config.policy.commands = "allow".into();
            runtime_config.policy.network = "allow".into();
            runtime_config.policy.downloads = "allow".into();
            runtime_config.policy.read_formats.clear();
        }
        if let Some(session_id) = session.as_ref() {
            if let Ok(meta) = self.sessions().get(session_id.as_str()) {
                if runtime_config.models.contains_key(&meta.model) {
                    prefer_model_routes(&mut runtime_config, &meta.model);
                    // Preserve the configured fallback as a distinct policy
                    // choice; the session model is only the preferred route.
                }
            }
        }
        let runtime_config = Arc::new(runtime_config);
        let models = Arc::new(ModelManager::from_config(&runtime_config)?);
        let tools = Arc::new(ToolRegistry::with_builtins());
        let policy = Arc::new(PolicyEngine::new(runtime_config.policy.clone()));
        let plan_mode = self.read_ui_state(|state| state.plan_mode);
        if plan_mode {
            // Installed here rather than in the loop so every entry point —
            // TUI turn, `snx run`, a delegated subagent — is covered by the
            // same refusal. The engine is rebuilt per runtime, so the scope
            // cannot outlive the turn that asked for it.
            policy.push_scope(
                nexus_policy::PLAN_MODE_SCOPE,
                nexus_policy::PolicyScope::plan_mode(),
            );
        }
        // The safety answer covers the session but is not written down, so it
        // is replayed here rather than loaded with the persisted grants below.
        if self.full_access_safety_granted() {
            policy.grant_session(nexus_policy::PolicyEngine::FULL_ACCESS_SAFETY_GRANT);
        }
        let full_access_safety = self.full_access_safety_granted.clone();
        let audit = self.audit();
        let sessions = SessionStore::new(self.store.clone());
        if let Some(session_id) = session.as_ref() {
            let meta = sessions.get(session_id.as_str())?;
            for grant in sessions.approval_grants(session_id.as_str())? {
                policy.grant_session(&grant);
            }
            for grant in sessions.workspace_approval_grants(&meta.workspace)? {
                policy.grant_session(&grant);
            }
        }
        Ok(AgentRuntime {
            full_access_safety: Some(full_access_safety),
            models,
            tools,
            policy,
            tool_ctx: ToolContext {
                workspace: self.guard.clone(),
                sandbox: self.sandbox.clone(),
                artifacts: self.artifacts.clone(),
                redactor: self.redactor.clone(),
                config: runtime_config,
                store: self.store.clone(),
                session,
                authorization: nexus_tools::ExecutionAuthorization::default(),
                profile: None,
            },
            audit,
            sessions,
            redactor: self.redactor.clone(),
            global_store: self.global_store.clone(),
            store: self.store.clone(),
            limits: TurnLimits {
                max_steps: self.config.limits.max_steps_per_turn,
                max_retries: self.config.limits.max_retries,
                max_repeated_calls: self.config.limits.max_repeated_calls,
                max_model_calls: self.config.limits.max_model_calls_per_turn,
                max_tool_calls: self.config.limits.max_tool_calls_per_turn,
                max_failures: self.config.limits.max_failures_per_turn,
                // Weighted now: the ceiling compares against what the turn
                // cost, not against the raw sum of every token that crossed
                // the wire. `weighted_token_ceiling` falls back to these same
                // legacy keys when the guard names no ceiling of its own.
                max_total_tokens: self.config.limits.weighted_token_ceiling(false),
                self_hosted_max_total_tokens: self.config.limits.weighted_token_ceiling(true),
                max_cost_micros: self.config.limits.max_cost_micros_per_turn,
                max_duration_ms: u64::from(self.config.limits.max_turn_runtime_min)
                    .saturating_mul(60_000),
                max_memory_writes: self.config.limits.max_memory_writes_per_turn,
                max_subagents: self.config.limits.max_subagents_per_run,
                max_recursion_depth: self.config.limits.max_recursion_depth,
                no_progress_limit: self
                    .config
                    .limits
                    .local_runaway_guard
                    .max_no_progress_cycles,
            },
            recursion_depth: 0,
            // An explicit `/thinking` choice is persisted in UI state and
            // outranks the config default from then on.
            thinking: self.read_ui_state(|state| state.thinking()),
            deep_planning: self.config.thinking.deep_planning,
            plan_mode,
            // Same precedence for narration: an explicit `/narrate` choice
            // outranks the config default. A different axis from `thinking` —
            // this one changes what is said, not what is done.
            narration: self.narration_mode(),
            narration_max_steps: self.config.narration.max_steps(),
            narration_refine: self.config.narration.refine_wording,
        })
    }

    /// Construct a runtime that persists into this workspace's database but
    /// confines tool access to an isolated task worktree.
    pub fn runtime_in_workspace(
        &self,
        session: Option<nexus_core::SessionId>,
        workspace: &std::path::Path,
    ) -> Result<AgentRuntime> {
        let mut runtime = self.runtime(session)?;
        runtime.tool_ctx.workspace = Arc::new(
            WorkspaceGuard::new(workspace, &runtime.tool_ctx.config.policy.denied_paths)?
                .with_unlock(self.full_access_safety_granted.clone()),
        );
        Ok(runtime)
    }

    pub fn audit(&self) -> AuditLog {
        AuditLog::new(self.store.clone(), self.redactor.clone())
    }

    /// Canonical adaptive-harness service used by menus and compatibility
    /// commands. It routes global/profile and workspace records safely.
    pub fn harness(&self) -> crate::control_plane::HarnessControlPlane<'_> {
        crate::control_plane::HarnessControlPlane::new(self)
    }

    pub fn sessions(&self) -> SessionStore {
        SessionStore::new(self.store.clone())
    }

    pub fn goals(&self) -> GoalStore {
        GoalStore::new(self.store.clone())
    }

    pub fn timeline(&self) -> nexus_core::timeline::TimelineStore {
        nexus_core::timeline::TimelineStore::new(self.store.clone())
    }

    pub fn orchestration(&self) -> nexus_core::orchestration::OrchestrationStore {
        nexus_core::orchestration::OrchestrationStore::new(self.store.clone())
    }

    pub fn memory(&self) -> MemoryStore {
        MemoryStore::new(
            self.store.clone(),
            &self.workspace_key,
            self.redactor.clone(),
            self.config.memory.global_enabled,
        )
    }

    pub fn personas(&self) -> nexus_memory::PersonaStore {
        nexus_memory::PersonaStore::new(self.store.clone(), &self.workspace_key)
    }

    pub fn profiles(&self) -> nexus_memory::ProfileStore {
        nexus_memory::ProfileStore::new(self.store.clone(), &self.workspace_key)
    }

    pub fn rsi(&self) -> nexus_memory::RsiStore {
        nexus_memory::RsiStore::new(self.store.clone(), &self.workspace_key)
    }

    pub fn indexer(&self) -> Indexer {
        Indexer::new(self.store.clone())
    }

    pub fn tools(&self) -> ToolRegistry {
        ToolRegistry::with_builtins()
    }

    pub fn mcp_registry(&self) -> nexus_mcp::McpRegistry {
        nexus_mcp::McpRegistry::new(self.store.clone())
    }

    pub fn skills(&self) -> nexus_skills::SkillStore {
        nexus_skills::SkillStore::new(self.store.clone())
    }

    /// Update and persist the operator UI state.
    pub fn update_ui_state(&self, f: impl FnOnce(&mut crate::uistate::UiState)) -> Result<()> {
        let mut guard = self
            .ui_state
            .lock()
            .map_err(|_| nexus_core::NexusError::Other("ui state lock poisoned".into()))?;
        guard.update(f)
    }

    /// Read a value out of the operator UI state.
    ///
    /// This is the in-memory copy, which may predate another process's write.
    /// Use [`App::read_fresh_ui_state`] wherever the value decides what a turn
    /// does rather than merely what a frame draws.
    pub fn read_ui_state<T>(&self, f: impl FnOnce(&crate::uistate::UiState) -> T) -> T {
        match self.ui_state.lock() {
            Ok(guard) => f(&guard.state),
            Err(poisoned) => f(&poisoned.into_inner().state),
        }
    }

    /// Read the operator's choices as they are on disk right now.
    ///
    /// A `snx` command and a running TUI share this file. The persona, model,
    /// and agent recorded on a new session are *decisions*, and taking them from
    /// a copy loaded at launch is how a session ended up running a persona the
    /// operator had already replaced from the other process.
    ///
    /// A failed reload is not fatal — it falls back to the in-memory copy rather
    /// than refusing to start a session — but it is never silently preferred.
    pub fn read_fresh_ui_state<T>(&self, f: impl FnOnce(&crate::uistate::UiState) -> T) -> T {
        if let Ok(mut guard) = self.ui_state.lock() {
            if let Err(error) = guard.reload() {
                tracing::warn!(
                    %error,
                    "could not re-read operator state; using the copy loaded at startup"
                );
            }
            return f(&guard.state);
        }
        self.read_ui_state(f)
    }

    /// The model recorded on new sessions: the operator's pin, then routing
    /// default, then any configured model, then an honest placeholder.
    pub fn any_model_name(&self) -> String {
        self.pinned_model
            .clone()
            .or_else(|| self.config.routing.fallback.clone())
            .or_else(|| self.config.routing.coding.clone())
            .or_else(|| self.config.models.keys().next().cloned())
            .unwrap_or_else(|| "unconfigured".to_string())
    }

    /// The agent role for new sessions: operator selection, then config.
    pub fn active_agent(&self) -> String {
        self.read_ui_state(|s| s.active_agent.clone())
            .unwrap_or_else(|| self.config.general.default_agent.clone())
    }

    pub fn agent_catalog(&self) -> Result<nexus_agent::AgentCatalog> {
        nexus_agent::AgentCatalog::load(
            &self.paths.global_dir.join("agents"),
            &self.paths.project_dir.join("agents"),
        )
    }

    pub fn resolve_agent(
        &self,
        name: &str,
    ) -> Result<(
        nexus_agent::AgentRole,
        Option<nexus_agent::CustomAgentDefinition>,
    )> {
        if let Some(role) = nexus_agent::AgentRole::parse(name) {
            return Ok((role, None));
        }
        let catalog = self.agent_catalog()?;
        let definition = catalog
            .get(name)
            .cloned()
            .ok_or_else(|| nexus_core::NexusError::NotFound(format!("agent `{name}`")))?;
        let role = definition.base_role()?;
        Ok((role, Some(definition)))
    }

    /// Theme name: operator selection, then config.
    pub fn theme_name(&self) -> String {
        self.read_ui_state(|s| s.theme.clone())
            .unwrap_or_else(|| self.config.general.theme.clone())
    }

    /// Narration mode: an explicit `/narrate` choice outranks the config
    /// default, exactly as `/thinking` and `/view` do for their own axes.
    pub fn narration_mode(&self) -> nexus_core::timeline::NarrationMode {
        self.read_ui_state(|s| s.narration())
            .unwrap_or_else(|| self.config.narration.mode())
    }
}

fn prefer_model_routes(config: &mut Config, model_name: &str) {
    config.routing.simple = Some(model_name.to_string());
    config.routing.coding = Some(model_name.to_string());
    config.routing.planning = Some(model_name.to_string());
}

/// When no models are configured but a Codex session exists, install the
/// default `codex` model so `snx` works out of the box after `codex login`.
/// The model id comes from the account's cached plan listing (refreshed by
/// `/login` and the Codex provider menu); inference goes through the ChatGPT
/// backend the plan token is entitled to — never `api.openai.com`.
pub fn install_codex_default_model(config: &mut Config, codex_session_available: bool) -> bool {
    if !config.models.is_empty() || !codex_session_available {
        return false;
    }

    let name = "codex".to_string();
    let plan = crate::codex::cached_plan_models();
    let default = plan.iter().find(|m| m.is_default).or_else(|| plan.first());
    let model = ModelConfig {
        provider: "codex".into(),
        base_url: String::new(),
        // Fall back to the codex CLI's current frontier default when the plan
        // has not been listed yet.
        model: default.map(|m| m.id.clone()).unwrap_or("gpt-5.5".into()),
        reasoning_effort: default.and_then(|m| m.default_reasoning_effort.clone()),
        context_window: 128_000,
        max_output_tokens: 8192,
        role: "executor".into(),
        ..Default::default()
    };

    config.models.insert(name.clone(), model);
    config.routing.simple.get_or_insert_with(|| name.clone());
    config.routing.coding.get_or_insert_with(|| name.clone());
    config.routing.planning.get_or_insert_with(|| name.clone());
    config.routing.fallback.get_or_insert(name);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_codex_model_when_session_exists_and_no_models_are_configured() {
        let mut config = Config::default();

        assert!(install_codex_default_model(&mut config, true));

        let model = config.models.get("codex").expect("codex model");
        assert_eq!(model.provider, "codex");
        assert!(
            model.base_url.is_empty(),
            "backend is implied by the provider, never api.openai.com"
        );
        assert!(!model.model.is_empty());
        assert_eq!(config.routing.fallback.as_deref(), Some("codex"));
        config.validate().expect("generated model is valid");
    }

    #[test]
    fn does_not_override_existing_models() {
        let mut config = Config::default();
        config.models.insert("local".into(), ModelConfig::default());

        assert!(!install_codex_default_model(&mut config, true));
        assert!(!config.models.contains_key("codex"));
    }

    #[test]
    fn preferred_model_override_preserves_distinct_fallback_policy() {
        let mut config = Config::default();
        config.routing.fallback = Some("approved-fallback".into());

        prefer_model_routes(&mut config, "session-preferred");

        assert_eq!(config.routing.simple.as_deref(), Some("session-preferred"));
        assert_eq!(config.routing.coding.as_deref(), Some("session-preferred"));
        assert_eq!(
            config.routing.planning.as_deref(),
            Some("session-preferred")
        );
        assert_eq!(
            config.routing.fallback.as_deref(),
            Some("approved-fallback")
        );
    }
}
