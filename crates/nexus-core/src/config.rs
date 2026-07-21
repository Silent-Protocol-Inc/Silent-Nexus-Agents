//! Layered configuration.
//!
//! Precedence (lowest to highest):
//! 1. built-in defaults;
//! 2. global config (`~/.config/silent-nexus/config.toml` on Linux);
//! 3. project config (`<workspace>/.nexus/config.toml`);
//! 4. environment overrides (`SNX_*`);
//! 5. explicit CLI flags (applied by the caller).
//!
//! Secrets are never stored inline: model entries reference an environment
//! variable (`api_key_env`) or a keyring entry name (`api_key_ref`).

use crate::error::{NexusError, Result};
use crate::thinking::ThinkingMode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Current config schema version. Bump when making breaking changes and add a
/// migration arm in the private `migrate_value` implementation.
pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Config schema version; migrated automatically when older.
    pub version: u32,
    pub general: GeneralConfig,
    /// Named model endpoints, e.g. `[models.local_main]`.
    pub models: BTreeMap<String, ModelConfig>,
    pub routing: RoutingConfig,
    pub policy: PolicyConfig,
    pub sandbox: SandboxConfig,
    pub web: WebConfig,
    pub memory: MemoryConfig,
    pub limits: LimitsConfig,
    /// Registered MCP servers, e.g. `[mcp.my_server]`.
    pub mcp: BTreeMap<String, McpServerConfig>,
    /// Terminal UI preferences, e.g. `[tui.activity]`.
    pub tui: TuiConfig,
    /// Deliberation behavior (`[thinking]`).
    pub thinking: ThinkingConfig,
}

/// How much *optional* deliberation the harness performs, and whether the live
/// activity component renders.
///
/// This block owns behavior only. Presentation of the activity component —
/// preview line count, expand shortcut, animation, reduced motion — lives in
/// [`ActivityConfig`] (`[tui.activity]`) and is deliberately not duplicated
/// here, so there is exactly one source of truth per knob.
///
/// Note that `[thinking].mode` and `[tui.activity].mode` are **different
/// axes** and take different values. `[thinking].mode` is `off|on|auto` and
/// controls deliberation; `[tui.activity].mode` is `default|detailed|debug`
/// and controls timeline verbosity (`/view`). Changing one never changes the
/// other.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ThinkingConfig {
    /// Deliberation mode: `off`, `on`, or `auto`. This is the default for a
    /// fresh workspace; an explicit `/thinking` choice is stored in UI state
    /// and takes precedence from then on.
    pub mode: ThinkingMode,
    /// Allow `on`/`auto` to promote a turn to grounded, staged execution.
    pub deep_planning: bool,
    /// Render provider-supplied reasoning *summaries* when a provider emits
    /// them. Raw provider reasoning is destroyed at ingestion and is never
    /// shown regardless of this setting.
    pub summarize_provider_reasoning: bool,
    /// Anti-flicker floor: leave the activity component hidden until a turn
    /// has run at least this long, so sub-second turns never flash it. Zero
    /// disables the delay.
    pub minimum_duration_ms: u64,
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self {
            mode: ThinkingMode::Auto,
            deep_planning: true,
            summarize_provider_reasoning: true,
            minimum_duration_ms: 500,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct TuiConfig {
    /// Live activity / inference timeline behavior (`[tui.activity]`).
    pub activity: ActivityConfig,
}

/// Controls the three-layer activity timeline. The improved defaults need no
/// configuration; these only exist to override them.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ActivityConfig {
    /// Timeline verbosity: `default`, `detailed`, or `debug`.
    pub mode: String,
    /// Max wrapped lines in the NEXUS processing preview (1–10).
    pub reasoning_preview_lines: u8,
    /// Shortcut that toggles the expanded activity detail.
    pub expand_shortcut: String,
    /// Show diagnostic-only events in the main timeline without debug mode.
    pub show_diagnostics: bool,
    /// Show token usage in progress/completion summaries.
    pub show_token_usage: bool,
    /// Processing animation style: `nexus`, `minimal`, or `off`.
    pub animation: String,
    /// Animation cadence: `slow`, `normal`, or `fast`.
    pub animation_speed: String,
    /// Force reduced motion regardless of the terminal/global setting.
    pub reduced_motion: bool,
    /// Coalesce related events (tool start/complete, retries) into one entry.
    pub coalesce_events: bool,
}

impl Default for ActivityConfig {
    fn default() -> Self {
        Self {
            mode: "default".into(),
            reasoning_preview_lines: 3,
            expand_shortcut: "ctrl+e".into(),
            show_diagnostics: false,
            show_token_usage: false,
            animation: "nexus".into(),
            animation_speed: "normal".into(),
            reduced_motion: false,
            coalesce_events: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct GeneralConfig {
    /// UI theme name (`nexus-dark`, `no-color`).
    pub theme: String,
    /// Disable all color output (also honors NO_COLOR).
    pub no_color: bool,
    /// Disable animations (also honors reduced-motion preferences).
    pub reduced_motion: bool,
    /// Default agent profile used for plain `snx run`.
    pub default_agent: String,
    /// Command run by `/test` and `snx test` (e.g. `cargo test`). Unset means
    /// the test command is honestly reported as not configured.
    pub test_command: Option<String>,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            theme: "nexus-dark".into(),
            no_color: false,
            reduced_motion: false,
            default_agent: "nexus".into(),
            test_command: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ModelConfig {
    /// Provider kind: `llamacpp`, `ollama`, `openai_compatible`,
    /// `claude-plan`, `anthropic`, `custom_http`, or `mock`.
    pub provider: String,
    /// Base URL of the endpoint, e.g. `http://127.0.0.1:8080/v1`.
    pub base_url: String,
    /// Model identifier passed to the provider.
    pub model: String,
    /// Environment variable holding the API key, if any. The value itself is
    /// never written to config or logs.
    pub api_key_env: Option<String>,
    /// Name of a credential-store entry holding the API key (managed by
    /// `snx auth` / the `/login` flow). The secret lives in the restricted
    /// credential store, never in this file.
    pub api_key_ref: Option<String>,
    /// API key resolved at bootstrap from `api_key_ref`. Never serialized;
    /// formats as `[redacted]`.
    #[serde(skip)]
    #[schemars(skip)]
    pub resolved_api_key: Option<crate::secret::SecretString>,
    /// Runtime-only consent to read the user's existing Codex CLI login when
    /// the isolated NEXUS profile is absent. Never serialized: the durable
    /// consent bit lives in UI state and is copied here at bootstrap.
    #[serde(skip)]
    #[schemars(skip)]
    pub allow_existing_codex: bool,
    /// Runtime-only consent to let the `claude-plan` bridge use the operator's
    /// existing Claude Code subscription login. Never serialized: the durable
    /// consent bit lives in UI state and is copied here at bootstrap.
    #[serde(skip)]
    #[schemars(skip)]
    pub allow_existing_claude: bool,
    /// Credential source when not using `api_key_env`. Currently `codex` reuses
    /// the OpenAI Codex CLI's OAuth session (`~/.codex/auth.json`), so the
    /// device/browser login is performed by the trusted `codex` CLI and Silent
    /// Nexus consumes the resulting token. `None`/empty = use `api_key_env`.
    pub auth: Option<String>,
    /// Context window in tokens (prompt + completion). Self-hosted servers
    /// allocate a KV cache this large before answering, so this is a runtime
    /// cost, not a capability claim — see `context_ceiling`.
    pub context_window: usize,
    /// Largest context the provider says the model architecture supports.
    /// Informational, and the ceiling `context_window` may be raised to.
    pub context_ceiling: Option<usize>,
    /// Maximum completion tokens per request.
    pub max_output_tokens: usize,
    /// Whether limits are operator-owned or refreshed from provider metadata.
    pub limit_mode: LimitMode,
    /// Provenance for the effective cached context limit.
    pub context_limit_source: LimitSource,
    /// Provenance for the effective cached output limit.
    pub output_limit_source: LimitSource,
    /// Role hint for routing: `router`, `executor`, `planner`, `verifier`, `embedder`.
    pub role: String,
    /// Override native tool-calling support detection. When `false`, the
    /// structured-action compatibility layer is used.
    pub native_tool_calls: Option<bool>,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Reasoning effort for models that honor it (`low`, `medium`, `high`, …).
    /// Used by the `codex` provider; ignored elsewhere.
    pub reasoning_effort: Option<String>,
    /// Stall timeout in seconds: how long a stream may go silent between
    /// chunks, and the total deadline for a non-streaming request to a
    /// metered provider.
    pub timeout_secs: u64,
    /// Seconds allowed for the *first* token. A self-hosted server has to load
    /// the model and run prefill before it can emit anything, which is not the
    /// same thing as a stalled stream. `None` uses the provider default:
    /// [`SELF_HOSTED_FIRST_TOKEN_SECS`] for `ollama`/`llamacpp`, otherwise
    /// `timeout_secs`.
    pub first_token_timeout_secs: Option<u64>,
    /// How long Ollama keeps the model resident after a request (its
    /// `keep_alive`). `None` sends [`OLLAMA_DEFAULT_KEEP_ALIVE`] so the next
    /// turn does not pay another cold load. Ignored by other providers.
    pub keep_alive: Option<String>,
    /// Verify TLS certificates for remote endpoints. Disabling this is an
    /// explicit advanced setting and never the default.
    pub tls_verify: bool,
}

/// Context window a self-hosted model is configured with when the provider
/// only reports the architecture maximum. The reported maximum is a capability
/// ceiling; requesting it forces the server to allocate a KV cache that large
/// before the first token, which on a host without spare memory takes long
/// enough to look like a hang. The operator raises it deliberately, per model.
///
/// Kept equal to [`ModelConfig::default()`]'s window: a 9B model asked for 32k
/// on a CPU-only server produced no first token in ten minutes, while the same
/// server answers at this size. Erring low costs some context on a capable
/// host; erring high makes a modest host look broken.
pub const SELF_HOSTED_DEFAULT_CONTEXT: usize = 8_192;

/// Completion ceiling for a discovered self-hosted model whose provider
/// reports no output limit. The 1024 general default truncates mid-answer.
pub const SELF_HOSTED_DEFAULT_OUTPUT: usize = 4_096;

/// First-token allowance for self-hosted providers: enough for a cold load and
/// prefill of a large model on a CPU-only host.
pub const SELF_HOSTED_FIRST_TOKEN_SECS: u64 = 600;

/// Default Ollama `keep_alive`.
pub const OLLAMA_DEFAULT_KEEP_ALIVE: &str = "30m";

impl ModelConfig {
    /// Whether this entry talks to a server the operator runs. Deliberately
    /// keyed on the provider kind rather than on the URL: a self-hosted server
    /// is frequently reached over the network, and its cost, latency, and
    /// warm-up behaviour are properties of the software, not of the host.
    pub fn is_self_hosted(&self) -> bool {
        matches!(self.provider.as_str(), "ollama" | "llamacpp")
    }

    /// Effective first-token allowance in seconds.
    pub fn first_token_timeout_secs(&self) -> u64 {
        self.first_token_timeout_secs
            .filter(|secs| *secs > 0)
            .unwrap_or(if self.is_self_hosted() {
                SELF_HOSTED_FIRST_TOKEN_SECS
            } else {
                self.timeout_secs
            })
            .max(1)
    }

    /// Effective Ollama `keep_alive`.
    pub fn keep_alive(&self) -> &str {
        self.keep_alive
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(OLLAMA_DEFAULT_KEEP_ALIVE)
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: "llamacpp".into(),
            base_url: "http://127.0.0.1:8080/v1".into(),
            model: String::new(),
            api_key_env: None,
            api_key_ref: None,
            resolved_api_key: None,
            allow_existing_codex: false,
            allow_existing_claude: false,
            auth: None,
            context_window: 8192,
            context_ceiling: None,
            max_output_tokens: 1024,
            limit_mode: LimitMode::Manual,
            context_limit_source: LimitSource::ConfiguredConservative,
            output_limit_source: LimitSource::ConfiguredConservative,
            role: "executor".into(),
            native_tool_calls: None,
            temperature: None,
            reasoning_effort: None,
            timeout_secs: 120,
            first_token_timeout_secs: None,
            keep_alive: None,
            tls_verify: true,
        }
    }
}

/// Ownership of model token limits. Missing fields deserialize as `manual`,
/// preserving every existing 1.x configuration verbatim.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LimitMode {
    Auto,
    #[default]
    Manual,
}

/// Origin of an effective model limit. These values are safe to persist and
/// display; they contain no endpoint or credential data.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LimitSource {
    ProviderMetadata,
    BundledCatalog,
    #[default]
    ConfiguredConservative,
}

impl std::fmt::Display for LimitSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ProviderMetadata => "provider metadata",
            Self::BundledCatalog => "bundled catalog",
            Self::ConfiguredConservative => "configured conservative",
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct RoutingConfig {
    /// Model name for trivial classification/routing turns.
    pub simple: Option<String>,
    /// Model name for coding tasks.
    pub coding: Option<String>,
    /// Model name for planning.
    pub planning: Option<String>,
    /// Fallback when a routed model is unavailable.
    pub fallback: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct PolicyConfig {
    /// Decision for workspace reads: allow | ask | deny.
    pub reads: String,
    /// Per-format workspace read decisions. Workspace config naturally
    /// overlays global keys through the schema-v1 layered TOML merge.
    pub read_formats: BTreeMap<String, String>,
    /// Decision for workspace writes.
    pub writes: String,
    /// Decision for shell commands not otherwise classified.
    pub commands: String,
    /// Decision for internet search/fetch.
    pub network: String,
    /// Decision for file downloads.
    pub downloads: String,
    /// Decision for destructive operations (delete, reset). `deny` or `ask` only.
    pub destructive: String,
    /// Decision for external side effects (git push, publish). `deny` or `ask` only.
    pub external: String,
    /// Command prefixes always denied (e.g. `sudo`).
    pub denied_commands: Vec<String>,
    /// Command prefixes allowed without prompting (e.g. `cargo check`).
    pub allowed_commands: Vec<String>,
    /// Additional denied path globs.
    pub denied_paths: Vec<String>,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            reads: "allow".into(),
            read_formats: BTreeMap::new(),
            writes: "ask".into(),
            commands: "ask".into(),
            network: "allow".into(),
            downloads: "ask".into(),
            destructive: "ask".into(),
            external: "ask".into(),
            denied_commands: vec![
                "sudo".into(),
                "su".into(),
                "doas".into(),
                "shutdown".into(),
                "reboot".into(),
                "mkfs".into(),
                "dd".into(),
            ],
            allowed_commands: vec![],
            denied_paths: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SandboxConfig {
    /// Preferred backend: `auto`, `process`, `container`, `none`.
    /// `auto` picks the strongest available backend.
    pub backend: String,
    /// Container image used by the container backend.
    pub container_image: String,
    /// CPU seconds limit per execution.
    pub cpu_limit_secs: u64,
    /// Memory limit in megabytes.
    pub memory_limit_mb: u64,
    /// Wall-clock timeout per execution in seconds.
    pub timeout_secs: u64,
    /// Maximum captured output bytes before truncation to artifact.
    pub max_output_bytes: usize,
    /// Network mode inside the sandbox: `off`, `restricted`, `full`.
    pub network: String,
    /// Environment variables forwarded into sandboxed processes.
    pub env_allowlist: Vec<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            backend: "auto".into(),
            container_image: "debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818".into(),
            cpu_limit_secs: 120,
            memory_limit_mb: 1024,
            timeout_secs: 120,
            max_output_bytes: 200_000,
            network: "off".into(),
            env_allowlist: vec![
                "PATH".into(),
                "LANG".into(),
                "TERM".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct WebConfig {
    /// Allow outbound web access at all.
    pub enabled: bool,
    /// Search provider: `duckduckgo` (no key required) or `none`.
    pub search_provider: String,
    /// Maximum download size in bytes for page retrieval.
    pub max_fetch_bytes: usize,
    /// Hosts always allowed (exact or `*.suffix`).
    pub allowlist: Vec<String>,
    /// Hosts always denied.
    pub denylist: Vec<String>,
    /// Permit fetching loopback addresses (off by default; needed only for
    /// deliberate local-service testing).
    pub allow_loopback: bool,
    /// Per-host minimum delay between requests, milliseconds.
    pub per_host_delay_ms: u64,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            search_provider: "duckduckgo".into(),
            max_fetch_bytes: 2_000_000,
            allowlist: vec![],
            denylist: vec![],
            allow_loopback: false,
            per_host_delay_ms: 1_000,
            timeout_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct MemoryConfig {
    /// Enable persistent memory.
    pub enabled: bool,
    /// Allow global (cross-project) memory. Off by default.
    pub global_enabled: bool,
    /// Days after which unverified memories expire (0 = never).
    pub default_ttl_days: u32,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            global_enabled: false,
            default_ttl_days: 90,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct LimitsConfig {
    /// Maximum agent-loop steps per turn.
    pub max_steps_per_turn: u32,
    /// Maximum consecutive failed/invalid model actions before stopping.
    pub max_retries: u32,
    /// Maximum identical tool calls before loop detection stops the turn.
    pub max_repeated_calls: u32,
    /// Maximum provider requests in one foreground turn.
    pub max_model_calls_per_turn: u32,
    /// Maximum tool executions in one foreground turn.
    pub max_tool_calls_per_turn: u32,
    /// Maximum recoverable failures before the turn stops.
    pub max_failures_per_turn: u32,
    /// Aggregate input and output token ceiling per turn.
    pub max_tokens_per_turn: usize,
    /// Aggregate token ceiling for a turn routed to a self-hosted provider
    /// (`ollama`, `llamacpp`). Those tokens are not metered, so the ceiling
    /// above — which exists to bound spend — is the wrong guard for them.
    pub self_hosted_max_tokens_per_turn: usize,
    /// Provider-reported monetary ceiling in micro-units per turn. Zero
    /// disables cost enforcement; non-zero fails closed for adapters that do
    /// not expose monetary usage.
    pub max_cost_micros_per_turn: u64,
    /// Foreground turn wall-clock ceiling in minutes.
    pub max_turn_runtime_min: u32,
    /// Maximum durable memory writes initiated by one turn.
    pub max_memory_writes_per_turn: u32,
    /// Maximum subagents created by one root run.
    pub max_subagents_per_run: u32,
    /// Maximum delegation ancestry depth.
    pub max_recursion_depth: u8,
    /// Default per-goal step budget.
    pub goal_step_budget: u32,
    /// Default per-goal wall-clock budget in minutes.
    pub goal_runtime_budget_min: u32,
    /// Tokens reserved for the model completion when packing context.
    pub completion_reserve_tokens: usize,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            max_steps_per_turn: 24,
            max_retries: 3,
            max_repeated_calls: 3,
            max_model_calls_per_turn: 24,
            max_tool_calls_per_turn: 48,
            max_failures_per_turn: 6,
            max_tokens_per_turn: 250_000,
            self_hosted_max_tokens_per_turn: 5_000_000,
            max_cost_micros_per_turn: 0,
            max_turn_runtime_min: 30,
            max_memory_writes_per_turn: 8,
            max_subagents_per_run: 8,
            max_recursion_depth: 2,
            goal_step_budget: 200,
            goal_runtime_budget_min: 120,
            completion_reserve_tokens: 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct McpServerConfig {
    /// Transport: `stdio` or `http`.
    pub transport: String,
    /// Command to launch (stdio transport).
    pub command: String,
    pub args: Vec<String>,
    /// URL (http transport).
    pub url: Option<String>,
    /// Enabled state.
    pub enabled: bool,
    /// Trust level: `untrusted` (tools require approval) or `trusted`.
    pub trust: String,
    /// Environment variables forwarded to the server process.
    pub env_allowlist: Vec<String>,
    /// Startup/request timeout seconds.
    pub timeout_secs: u64,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            transport: "stdio".into(),
            command: String::new(),
            args: vec![],
            url: None,
            enabled: false,
            trust: "untrusted".into(),
            env_allowlist: vec!["PATH".into(), "HOME".into(), "LANG".into()],
            timeout_secs: 30,
        }
    }
}

/// Shape of the machine-managed models file (`models.toml`).
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(default)]
struct ManagedModels {
    version: u32,
    models: BTreeMap<String, ModelConfig>,
}

/// Where config and state live for a given workspace.
#[derive(Debug, Clone)]
pub struct ConfigPaths {
    pub global_dir: PathBuf,
    pub global_file: PathBuf,
    pub project_dir: PathBuf,
    pub project_file: PathBuf,
    pub state_dir: PathBuf,
    /// Machine-managed model definitions written by interactive flows
    /// (`/connect` custom endpoints, discovered local models). Layered between
    /// the global and project config files. Never holds secrets.
    pub managed_models_file: PathBuf,
    /// Machine-managed setting overrides written by interactive flows
    /// (`/permissions`, `/sandbox`). Merged last so interactive choices win
    /// over every config file. Never holds secrets.
    pub managed_overrides_file: PathBuf,
    /// Workspace-scoped managed overrides written by interactive `/config`.
    pub workspace_overrides_file: PathBuf,
    /// Restricted credential/auth root (`auth/` under the global config dir):
    /// per-provider profiles, including the isolated Codex home.
    pub auth_dir: PathBuf,
    /// Versioned UI/harness state (active model, theme, history). No secrets.
    pub ui_state_file: PathBuf,
}

impl ConfigPaths {
    pub fn discover(workspace: &Path) -> Result<Self> {
        let base = directories::ProjectDirs::from("top", "silentprotocol", "silent-nexus")
            .ok_or_else(|| NexusError::Config("cannot determine user config directory".into()))?;
        let global_dir = base.config_dir().to_path_buf();
        let project_dir = workspace.join(".nexus");
        let state_dir = project_dir.join("state");
        Ok(Self {
            global_file: global_dir.join("config.toml"),
            managed_models_file: global_dir.join("models.toml"),
            managed_overrides_file: global_dir.join("overrides.toml"),
            workspace_overrides_file: project_dir.join("overrides.toml"),
            auth_dir: global_dir.join("auth"),
            global_dir,
            project_file: project_dir.join("config.toml"),
            ui_state_file: state_dir.join("ui-state.json"),
            state_dir,
            project_dir,
        })
    }
}

impl Config {
    /// Load layered configuration for `workspace`.
    pub fn load(workspace: &Path) -> Result<(Self, ConfigPaths)> {
        let paths = ConfigPaths::discover(workspace)?;
        let mut value = toml::Value::Table(Default::default());
        for file in [
            &paths.global_file,
            &paths.managed_models_file,
            &paths.project_file,
            &paths.managed_overrides_file,
            &paths.workspace_overrides_file,
        ] {
            if file.exists() {
                let text = std::fs::read_to_string(file)?;
                let parsed: toml::Value =
                    toml::from_str(&text).map_err(|e| NexusError::ConfigFile {
                        path: file.display().to_string(),
                        message: friendly_toml_error(&e),
                    })?;
                let migrated = migrate_value(parsed, file)?;
                merge_toml(&mut value, migrated);
            }
        }
        apply_env_overrides(&mut value);
        let mut config: Config = value
            .try_into()
            .map_err(|e| NexusError::Config(friendly_toml_error_de(&e)))?;
        if config.version == 0 {
            config.version = CONFIG_VERSION;
        }
        config.validate()?;
        Ok((config, paths))
    }

    /// Read the machine-managed model definitions (the `/connect` flows own this
    /// file). Missing file = empty set.
    pub fn load_managed_models(paths: &ConfigPaths) -> Result<BTreeMap<String, ModelConfig>> {
        if !paths.managed_models_file.exists() {
            return Ok(BTreeMap::new());
        }
        let text = std::fs::read_to_string(&paths.managed_models_file)?;
        let parsed: ManagedModels = toml::from_str(&text).map_err(|e| NexusError::ConfigFile {
            path: paths.managed_models_file.display().to_string(),
            message: friendly_toml_error(&e),
        })?;
        Ok(parsed.models)
    }

    /// Persist the machine-managed model definitions. The file never carries
    /// secrets: `ModelConfig` stores only `api_key_env`/`api_key_ref` names.
    pub fn save_managed_models(
        paths: &ConfigPaths,
        models: &BTreeMap<String, ModelConfig>,
    ) -> Result<()> {
        let body = ManagedModels {
            version: CONFIG_VERSION,
            models: models.clone(),
        };
        let mut text = String::from(
            "# Managed by snx (`/connect`, `snx model`). Edit config.toml for manual entries.\n",
        );
        text.push_str(
            &toml::to_string_pretty(&body)
                .map_err(|e| NexusError::Config(format!("serializing managed models: {e}")))?,
        );
        if let Some(parent) = paths.managed_models_file.parent() {
            crate::permissions::repair_private_tree(parent)?;
        }
        crate::atomic::atomic_write_private(&paths.managed_models_file, text.as_bytes())?;
        Ok(())
    }

    /// Read-modify-write the machine-managed overrides file. The closure
    /// mutates the root TOML table; writing one setting never clobbers
    /// another. The resulting config must still validate — callers get the
    /// error before anything is persisted.
    pub fn update_managed_overrides(
        paths: &ConfigPaths,
        f: impl FnOnce(&mut toml::value::Table),
    ) -> Result<()> {
        let mut table = if paths.managed_overrides_file.exists() {
            let text = std::fs::read_to_string(&paths.managed_overrides_file)?;
            toml::from_str::<toml::value::Table>(&text).map_err(|e| NexusError::ConfigFile {
                path: paths.managed_overrides_file.display().to_string(),
                message: friendly_toml_error(&e),
            })?
        } else {
            toml::value::Table::new()
        };
        f(&mut table);
        let mut effective = toml::Value::Table(Default::default());
        for file in [
            &paths.global_file,
            &paths.managed_models_file,
            &paths.project_file,
        ] {
            if file.exists() {
                let source = std::fs::read_to_string(file)?;
                let parsed: toml::Value =
                    toml::from_str(&source).map_err(|error| NexusError::ConfigFile {
                        path: file.display().to_string(),
                        message: friendly_toml_error(&error),
                    })?;
                merge_toml(&mut effective, migrate_value(parsed, file)?);
            }
        }
        merge_toml(&mut effective, toml::Value::Table(table.clone()));
        apply_env_overrides(&mut effective);
        let candidate: Config = effective
            .try_into()
            .map_err(|error| NexusError::Config(friendly_toml_error_de(&error)))?;
        candidate.validate()?;
        let mut text = String::from(
            "# Managed by snx (`/permissions`, `/sandbox`). Merged last over all config files.\n",
        );
        text.push_str(
            &toml::to_string_pretty(&toml::Value::Table(table))
                .map_err(|e| NexusError::Config(format!("serializing overrides: {e}")))?,
        );
        if let Some(parent) = paths.managed_overrides_file.parent() {
            crate::permissions::repair_private_tree(parent)?;
        }
        crate::atomic::atomic_write_private(&paths.managed_overrides_file, text.as_bytes())?;
        Ok(())
    }

    /// Atomically update one machine-managed scope without touching either
    /// hand-written TOML file. Workspace overrides layer after global ones.
    pub fn update_scoped_overrides(
        paths: &ConfigPaths,
        workspace: bool,
        f: impl FnOnce(&mut toml::value::Table),
    ) -> Result<()> {
        let target = if workspace {
            &paths.workspace_overrides_file
        } else {
            &paths.managed_overrides_file
        };
        let mut table = if target.exists() {
            toml::from_str::<toml::value::Table>(&std::fs::read_to_string(target)?).map_err(
                |error| NexusError::ConfigFile {
                    path: target.display().to_string(),
                    message: friendly_toml_error(&error),
                },
            )?
        } else {
            toml::value::Table::new()
        };
        f(&mut table);

        let mut effective = toml::Value::Table(Default::default());
        for file in [
            &paths.global_file,
            &paths.managed_models_file,
            &paths.project_file,
            &paths.managed_overrides_file,
            &paths.workspace_overrides_file,
        ] {
            if file == target || !file.exists() {
                continue;
            }
            let parsed: toml::Value =
                toml::from_str(&std::fs::read_to_string(file)?).map_err(|error| {
                    NexusError::ConfigFile {
                        path: file.display().to_string(),
                        message: friendly_toml_error(&error),
                    }
                })?;
            merge_toml(&mut effective, migrate_value(parsed, file)?);
        }
        merge_toml(&mut effective, toml::Value::Table(table.clone()));
        apply_env_overrides(&mut effective);
        let candidate: Config = effective
            .try_into()
            .map_err(|error| NexusError::Config(friendly_toml_error_de(&error)))?;
        candidate.validate()?;

        let mut text =
            String::from("# Managed by snx `/config`; hand-written TOML is never replaced.\n");
        text.push_str(
            &toml::to_string_pretty(&toml::Value::Table(table))
                .map_err(|error| NexusError::Config(format!("serializing overrides: {error}")))?,
        );
        if let Some(parent) = target.parent() {
            crate::permissions::repair_private_tree(parent)?;
        }
        crate::atomic::atomic_write_private(target, text.as_bytes())
    }

    /// Update one read-format rule without replacing unrelated hand-written
    /// configuration. Workspace is the interactive default; `global=true`
    /// writes the user defaults file explicitly.
    pub fn update_read_format(
        paths: &ConfigPaths,
        global: bool,
        format: &str,
        decision: &str,
    ) -> Result<()> {
        if format.trim().is_empty() || !["allow", "ask", "deny"].contains(&decision) {
            return Err(NexusError::Config(
                "format must be non-empty and decision must be allow|ask|deny".into(),
            ));
        }
        let target = if global {
            &paths.global_file
        } else {
            &paths.project_file
        };
        let mut table = if target.exists() {
            let text = std::fs::read_to_string(target)?;
            toml::from_str::<toml::value::Table>(&text).map_err(|error| NexusError::ConfigFile {
                path: target.display().to_string(),
                message: friendly_toml_error(&error),
            })?
        } else {
            let mut table = toml::value::Table::new();
            table.insert(
                "version".into(),
                toml::Value::Integer(CONFIG_VERSION.into()),
            );
            table
        };
        let policy = table
            .entry("policy")
            .or_insert_with(|| toml::Value::Table(Default::default()));
        let policy = policy.as_table_mut().ok_or_else(|| {
            NexusError::Config("existing `policy` value must be a TOML table".into())
        })?;
        let formats = policy
            .entry("read_formats")
            .or_insert_with(|| toml::Value::Table(Default::default()));
        let formats = formats.as_table_mut().ok_or_else(|| {
            NexusError::Config("existing `policy.read_formats` value must be a TOML table".into())
        })?;
        formats.insert(format.to_string(), toml::Value::String(decision.into()));
        let text = toml::to_string_pretty(&table)
            .map_err(|error| NexusError::Config(format!("serializing config: {error}")))?;
        if let Some(parent) = target.parent() {
            crate::permissions::repair_private_tree(parent)?;
        }
        crate::atomic::atomic_write_private(target, text.as_bytes())
    }

    /// Validate cross-field invariants with actionable messages.
    pub fn validate(&self) -> Result<()> {
        let activity = &self.tui.activity;
        if crate::timeline::ActivityMode::parse(&activity.mode).is_none() {
            return Err(NexusError::Config(format!(
                "tui.activity.mode must be one of default|detailed|debug, got `{}`",
                activity.mode
            )));
        }
        if !(1..=10).contains(&activity.reasoning_preview_lines) {
            return Err(NexusError::Config(format!(
                "tui.activity.reasoning_preview_lines must be 1-10, got {}",
                activity.reasoning_preview_lines
            )));
        }
        if !["slow", "normal", "fast"].contains(&activity.animation_speed.as_str()) {
            return Err(NexusError::Config(format!(
                "tui.activity.animation_speed must be one of slow|normal|fast, got `{}`",
                activity.animation_speed
            )));
        }
        if !["nexus", "minimal", "off"].contains(&activity.animation.as_str()) {
            return Err(NexusError::Config(format!(
                "tui.activity.animation must be one of nexus|minimal|off, got `{}`",
                activity.animation
            )));
        }
        // A long floor would hide the component for entire turns rather than
        // just smoothing sub-second flicker, which is not what it is for.
        if self.thinking.minimum_duration_ms > 5_000 {
            return Err(NexusError::Config(format!(
                "thinking.minimum_duration_ms must be 0-5000, got {}",
                self.thinking.minimum_duration_ms
            )));
        }
        let decision_fields = [
            ("policy.reads", &self.policy.reads),
            ("policy.writes", &self.policy.writes),
            ("policy.commands", &self.policy.commands),
            ("policy.network", &self.policy.network),
            ("policy.downloads", &self.policy.downloads),
            ("policy.destructive", &self.policy.destructive),
            ("policy.external", &self.policy.external),
        ];
        for (name, v) in decision_fields {
            if !["allow", "ask", "deny"].contains(&v.as_str()) {
                return Err(NexusError::Config(format!(
                    "{name} must be one of allow|ask|deny, got `{v}`"
                )));
            }
        }
        for (format, decision) in &self.policy.read_formats {
            if format.trim().is_empty() || !["allow", "ask", "deny"].contains(&decision.as_str()) {
                return Err(NexusError::Config(format!(
                    "policy.read_formats.{format} must be one of allow|ask|deny"
                )));
            }
        }
        for (name, v) in [
            ("policy.destructive", &self.policy.destructive),
            ("policy.external", &self.policy.external),
        ] {
            if v == "allow" {
                return Err(NexusError::Config(format!(
                    "{name} may not be `allow`; destructive and external actions always require at least `ask`"
                )));
            }
        }
        for (key, m) in &self.models {
            const PROVIDERS: &[&str] = &[
                "llamacpp",
                "ollama",
                "openai",
                "openai_compatible",
                "custom_http",
                "codex",
                "claude-plan",
                "anthropic",
                "mock",
            ];
            if !PROVIDERS.contains(&m.provider.as_str()) {
                return Err(NexusError::Config(format!(
                    "models.{key}.provider `{}` unknown; expected one of {}",
                    m.provider,
                    PROVIDERS.join("|")
                )));
            }
            if let Some(auth) = m.auth.as_deref().filter(|a| !a.is_empty()) {
                if auth != "codex" {
                    return Err(NexusError::Config(format!(
                        "models.{key}.auth `{auth}` unknown; the only supported value is `codex`"
                    )));
                }
                if !matches!(
                    m.provider.as_str(),
                    "openai" | "openai_compatible" | "custom_http" | "codex"
                ) {
                    return Err(NexusError::Config(format!(
                        "models.{key}.auth = \"codex\" requires an OpenAI-style provider \
                         (openai | openai_compatible | custom_http | codex), not `{}`",
                        m.provider
                    )));
                }
            }
            // `openai` and Codex-authed models have well-known default
            // endpoints, so base_url is optional; every other networked
            // provider must set it.
            let base_optional = matches!(
                m.provider.as_str(),
                "mock" | "openai" | "codex" | "claude-plan" | "anthropic"
            ) || m.auth.as_deref() == Some("codex");
            if !base_optional && m.base_url.is_empty() {
                return Err(NexusError::Config(format!(
                    "models.{key}.base_url is required for provider `{}`",
                    m.provider
                )));
            }
            if m.context_window < 1024 {
                return Err(NexusError::Config(format!(
                    "models.{key}.context_window must be >= 1024"
                )));
            }
        }
        for (route, target) in [
            ("routing.simple", &self.routing.simple),
            ("routing.coding", &self.routing.coding),
            ("routing.planning", &self.routing.planning),
            ("routing.fallback", &self.routing.fallback),
        ] {
            if let Some(t) = target {
                if !self.models.contains_key(t) {
                    return Err(NexusError::Config(format!(
                        "{route} references model `{t}` which is not defined under [models]"
                    )));
                }
            }
        }
        if !["off", "restricted", "full"].contains(&self.sandbox.network.as_str()) {
            return Err(NexusError::Config(
                "sandbox.network must be off|restricted|full".into(),
            ));
        }
        if !["auto", "process", "container", "none"].contains(&self.sandbox.backend.as_str()) {
            return Err(NexusError::Config(
                "sandbox.backend must be auto|process|container|none".into(),
            ));
        }
        let pinned_image = self
            .sandbox
            .container_image
            .split_once("@sha256:")
            .is_some_and(|(name, digest)| {
                !name.trim().is_empty()
                    && digest.len() == 64
                    && digest
                        .chars()
                        .all(|character| character.is_ascii_hexdigit())
            });
        if !pinned_image {
            return Err(NexusError::Config(
                "sandbox.container_image must be pinned as name@sha256:<64 hex characters>".into(),
            ));
        }
        for (key, s) in &self.mcp {
            if !["stdio", "http"].contains(&s.transport.as_str()) {
                return Err(NexusError::Config(format!(
                    "mcp.{key}.transport must be stdio|http"
                )));
            }
            if s.transport == "stdio" && s.command.is_empty() {
                return Err(NexusError::Config(format!(
                    "mcp.{key}.command is required for stdio transport"
                )));
            }
            if s.transport == "http" && s.url.is_none() {
                return Err(NexusError::Config(format!(
                    "mcp.{key}.url is required for http transport"
                )));
            }
        }
        Ok(())
    }

    /// JSON Schema for the config file (used by `snx config schema` and docs).
    pub fn json_schema() -> serde_json::Value {
        let schema = schemars::schema_for!(Config);
        serde_json::to_value(schema).unwrap_or_default()
    }
}

/// Migrate an older on-disk config value to the current version.
fn migrate_value(mut value: toml::Value, file: &Path) -> Result<toml::Value> {
    let version = value
        .get("version")
        .and_then(|v| v.as_integer())
        .unwrap_or(CONFIG_VERSION as i64) as u32;
    if version > CONFIG_VERSION {
        return Err(NexusError::ConfigFile {
            path: file.display().to_string(),
            message: format!(
                "config version {version} is newer than this build supports ({CONFIG_VERSION}); upgrade snx"
            ),
        });
    }
    // Version 1 is current; future migrations chain here:
    // if version < 2 { …rewrite value…; }
    repair_discovered_self_hosted_limits(&mut value);
    if let Some(table) = value.as_table_mut() {
        table.insert(
            "version".into(),
            toml::Value::Integer(CONFIG_VERSION as i64),
        );
    }
    Ok(value)
}

/// Repair self-hosted model entries that discovery wrote with the provider's
/// architecture maximum as their runtime context window.
///
/// `/api/show` reports the largest context the model *can* address. Persisting
/// that as `context_window` made every request ask the server to allocate a KV
/// cache of that size up front — on a CPU-only host a 256k cache takes long
/// enough that the request looks like a hang and trips the timeout. The
/// reported maximum is kept as `context_ceiling` so nothing is lost and the
/// operator can raise the window deliberately.
///
/// Only entries discovery owns are touched: `limit_mode = "auto"` with
/// `context_limit_source = "provider_metadata"`. A hand-written window is the
/// operator's decision and is left exactly as written. In-memory only — the
/// corrected values reach disk the next time the entry is saved — and
/// idempotent, because a repaired entry already carries `context_ceiling`.
fn repair_discovered_self_hosted_limits(value: &mut toml::Value) {
    let Some(models) = value.get_mut("models").and_then(toml::Value::as_table_mut) else {
        return;
    };
    for (_, model) in models.iter_mut() {
        let Some(model) = model.as_table_mut() else {
            continue;
        };
        let self_hosted = matches!(
            model.get("provider").and_then(toml::Value::as_str),
            Some("ollama" | "llamacpp")
        );
        let discovery_owned = model.get("limit_mode").and_then(toml::Value::as_str) == Some("auto")
            && model
                .get("context_limit_source")
                .and_then(toml::Value::as_str)
                == Some("provider_metadata");
        if !self_hosted || !discovery_owned || model.contains_key("context_ceiling") {
            continue;
        }
        let window = model
            .get("context_window")
            .and_then(toml::Value::as_integer)
            .unwrap_or(0);
        if window > SELF_HOSTED_DEFAULT_CONTEXT as i64 {
            model.insert("context_ceiling".into(), toml::Value::Integer(window));
            model.insert(
                "context_window".into(),
                toml::Value::Integer(SELF_HOSTED_DEFAULT_CONTEXT as i64),
            );
        }
        // The same entries were saved with the general 1024-token completion
        // default, which truncates mid-answer on a model this size.
        let conservative_output = model
            .get("output_limit_source")
            .and_then(toml::Value::as_str)
            == Some("configured_conservative");
        if conservative_output
            && model
                .get("max_output_tokens")
                .and_then(toml::Value::as_integer)
                == Some(1024)
        {
            model.insert(
                "max_output_tokens".into(),
                toml::Value::Integer(SELF_HOSTED_DEFAULT_OUTPUT as i64),
            );
        }
    }
}

/// Deep-merge `overlay` into `base` (tables merge; scalars/arrays replace).
fn merge_toml(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(b), toml::Value::Table(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(existing) => merge_toml(existing, v),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (b, o) => *b = o,
    }
}

/// Environment overrides: `SNX_SECTION__FIELD=value`, e.g.
/// `SNX_POLICY__WRITES=allow`, `SNX_SANDBOX__NETWORK=off`.
fn apply_env_overrides(value: &mut toml::Value) {
    for (key, val) in std::env::vars() {
        let Some(rest) = key.strip_prefix("SNX_") else {
            continue;
        };
        let parts: Vec<String> = rest.split("__").map(|p| p.to_lowercase()).collect();
        if parts.len() < 2 || parts.iter().any(|p| p.is_empty()) {
            continue;
        }
        let toml_val = if let Ok(i) = val.parse::<i64>() {
            toml::Value::Integer(i)
        } else if let Ok(b) = val.parse::<bool>() {
            toml::Value::Boolean(b)
        } else {
            toml::Value::String(val)
        };
        let mut cursor = &mut *value;
        for part in &parts[..parts.len() - 1] {
            if !cursor.is_table() {
                *cursor = toml::Value::Table(Default::default());
            }
            let table = cursor.as_table_mut().expect("just ensured table");
            cursor = table
                .entry(part.clone())
                .or_insert_with(|| toml::Value::Table(Default::default()));
        }
        if let Some(table) = cursor.as_table_mut() {
            table.insert(parts[parts.len() - 1].clone(), toml_val);
        }
    }
}

fn friendly_toml_error(e: &toml::de::Error) -> String {
    e.to_string().lines().collect::<Vec<_>>().join(" ")
}

fn friendly_toml_error_de(e: &toml::de::Error) -> String {
    format!("invalid configuration: {}", friendly_toml_error(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        Config::default()
            .validate()
            .expect("defaults must be valid");
    }

    #[test]
    fn thinking_defaults_are_auto_and_deliberate() {
        let thinking = Config::default().thinking;
        assert_eq!(thinking.mode, ThinkingMode::Auto);
        assert!(thinking.deep_planning);
        assert!(thinking.summarize_provider_reasoning);
        assert_eq!(thinking.minimum_duration_ms, 500);
    }

    #[test]
    fn thinking_block_parses_from_toml() {
        let config: Config = toml::from_str(
            r#"
            [thinking]
            mode = "on"
            deep_planning = false
            minimum_duration_ms = 250
            "#,
        )
        .expect("parses");
        assert_eq!(config.thinking.mode, ThinkingMode::On);
        assert!(!config.thinking.deep_planning);
        assert_eq!(config.thinking.minimum_duration_ms, 250);
        // Unset keys still come from Default, not from zero values.
        assert!(config.thinking.summarize_provider_reasoning);
        config.validate().expect("valid");
    }

    #[test]
    fn config_without_thinking_block_uses_defaults() {
        // A 2.3.0 config predates the block entirely and must still load.
        let config: Config = toml::from_str("[general]\ntheme = \"nexus-dark\"\n").expect("parses");
        assert_eq!(config.thinking.mode, ThinkingMode::Auto);
        config.validate().expect("valid");
    }

    #[test]
    fn thinking_block_rejects_unknown_keys() {
        let err = toml::from_str::<Config>("[thinking]\nshow_raw_reasoning = true\n")
            .expect_err("unknown key must be rejected");
        assert!(err.to_string().contains("show_raw_reasoning"));
    }

    #[test]
    fn thinking_mode_rejects_unknown_value() {
        assert!(toml::from_str::<Config>("[thinking]\nmode = \"sometimes\"\n").is_err());
    }

    #[test]
    fn excessive_minimum_duration_is_rejected() {
        let mut config = Config::default();
        config.thinking.minimum_duration_ms = 6_000;
        let err = config.validate().expect_err("must fail").to_string();
        assert!(err.contains("minimum_duration_ms"));
    }

    #[test]
    fn thinking_and_activity_modes_are_independent_axes() {
        // The two `mode` keys share a name but not a value space; a config
        // setting both must round-trip each independently.
        let config: Config = toml::from_str(
            r#"
            [thinking]
            mode = "off"

            [tui.activity]
            mode = "debug"
            "#,
        )
        .expect("parses");
        assert_eq!(config.thinking.mode, ThinkingMode::Off);
        assert_eq!(config.tui.activity.mode, "debug");
        config.validate().expect("valid");
    }

    #[test]
    fn destructive_allow_is_rejected() {
        let mut c = Config::default();
        c.policy.destructive = "allow".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn container_image_must_be_digest_pinned() {
        let mut config = Config::default();
        config.sandbox.container_image = "debian:bookworm-slim".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn unknown_provider_is_rejected_with_hint() {
        let mut c = Config::default();
        c.models.insert(
            "m".into(),
            ModelConfig {
                provider: "gpt4all".into(),
                ..Default::default()
            },
        );
        let err = c.validate().expect_err("must fail").to_string();
        assert!(err.contains("llamacpp"));
    }

    #[test]
    fn routing_must_reference_defined_models() {
        let mut c = Config::default();
        c.routing.coding = Some("missing".into());
        assert!(c.validate().is_err());
    }

    #[test]
    fn merge_overlays_tables() {
        let mut base: toml::Value =
            toml::from_str("[policy]\nwrites='ask'\nreads='allow'").expect("toml");
        let overlay: toml::Value = toml::from_str("[policy]\nwrites='allow'").expect("toml");
        merge_toml(&mut base, overlay);
        assert_eq!(base["policy"]["writes"].as_str(), Some("allow"));
        assert_eq!(base["policy"]["reads"].as_str(), Some("allow"));
    }

    #[test]
    fn managed_overrides_read_modify_write_preserves_other_tables() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = ConfigPaths {
            global_dir: dir.path().to_path_buf(),
            global_file: dir.path().join("config.toml"),
            project_dir: dir.path().join(".nexus"),
            project_file: dir.path().join(".nexus/config.toml"),
            state_dir: dir.path().join(".nexus/state"),
            managed_models_file: dir.path().join("models.toml"),
            managed_overrides_file: dir.path().join("overrides.toml"),
            workspace_overrides_file: dir.path().join(".nexus/overrides.toml"),
            auth_dir: dir.path().join("auth"),
            ui_state_file: dir.path().join("ui-state.json"),
        };
        Config::update_managed_overrides(&paths, |root| {
            let mut policy = toml::value::Table::new();
            policy.insert("writes".into(), toml::Value::String("allow".into()));
            root.insert("policy".into(), toml::Value::Table(policy));
        })
        .expect("first write");
        Config::update_managed_overrides(&paths, |root| {
            let mut sandbox = toml::value::Table::new();
            sandbox.insert("backend".into(), toml::Value::String("none".into()));
            root.insert("sandbox".into(), toml::Value::Table(sandbox));
        })
        .expect("second write");
        let text = std::fs::read_to_string(&paths.managed_overrides_file).expect("read");
        let parsed: toml::value::Table = toml::from_str(&text).expect("parse");
        assert_eq!(
            parsed["policy"]["writes"].as_str(),
            Some("allow"),
            "second write must not clobber the first"
        );
        assert_eq!(parsed["sandbox"]["backend"].as_str(), Some("none"));
    }

    fn migrated(text: &str) -> toml::Value {
        migrate_value(
            toml::from_str(text).expect("parse"),
            std::path::Path::new("models.toml"),
        )
        .expect("migrate")
    }

    #[test]
    fn a_discovered_self_hosted_context_maximum_becomes_a_ceiling_not_a_request() {
        let value = migrated(
            r#"
            [models.qwen]
            provider = "ollama"
            model = "qwen3.5:9b"
            context_window = 262144
            max_output_tokens = 1024
            limit_mode = "auto"
            context_limit_source = "provider_metadata"
            output_limit_source = "configured_conservative"
            "#,
        );
        let model = &value["models"]["qwen"];
        assert_eq!(
            model["context_window"].as_integer(),
            Some(SELF_HOSTED_DEFAULT_CONTEXT as i64),
            "the runtime window must not ask for a 256k KV cache",
        );
        assert_eq!(
            model["context_ceiling"].as_integer(),
            Some(262144),
            "the reported maximum must survive so it can be raised back",
        );
        assert_eq!(
            model["max_output_tokens"].as_integer(),
            Some(SELF_HOSTED_DEFAULT_OUTPUT as i64),
        );

        // Idempotent: a repaired entry carries a ceiling and is left alone.
        let again = migrated(&toml::to_string(&value).expect("serialize"));
        assert_eq!(
            again["models"]["qwen"]["context_window"].as_integer(),
            Some(SELF_HOSTED_DEFAULT_CONTEXT as i64),
        );
        assert_eq!(
            again["models"]["qwen"]["context_ceiling"].as_integer(),
            Some(262144),
        );
    }

    #[test]
    fn a_hand_written_context_window_is_the_operators_decision() {
        let value = migrated(
            r#"
            [models.big]
            provider = "ollama"
            model = "gemma4:26b"
            context_window = 131072
            max_output_tokens = 1024
            limit_mode = "manual"
            context_limit_source = "configured_conservative"
            output_limit_source = "configured_conservative"

            [models.remote]
            provider = "anthropic"
            model = "claude"
            context_window = 200000
            limit_mode = "auto"
            context_limit_source = "provider_metadata"
            "#,
        );
        assert_eq!(
            value["models"]["big"]["context_window"].as_integer(),
            Some(131072)
        );
        assert!(value["models"]["big"].get("context_ceiling").is_none());
        assert_eq!(
            value["models"]["big"]["max_output_tokens"].as_integer(),
            Some(1024)
        );
        assert_eq!(
            value["models"]["remote"]["context_window"].as_integer(),
            Some(200000),
            "only self-hosted servers pay for the window up front",
        );
    }

    #[test]
    fn self_hosted_models_get_a_first_token_allowance_for_cold_loads() {
        let ollama = ModelConfig {
            provider: "ollama".into(),
            ..Default::default()
        };
        assert!(ollama.is_self_hosted());
        assert_eq!(
            ollama.first_token_timeout_secs(),
            SELF_HOSTED_FIRST_TOKEN_SECS,
        );
        assert_eq!(ollama.keep_alive(), OLLAMA_DEFAULT_KEEP_ALIVE);

        let metered = ModelConfig {
            provider: "anthropic".into(),
            timeout_secs: 90,
            ..Default::default()
        };
        assert!(!metered.is_self_hosted());
        assert_eq!(
            metered.first_token_timeout_secs(),
            90,
            "a metered provider answers or fails inside its normal timeout",
        );

        let explicit = ModelConfig {
            provider: "ollama".into(),
            first_token_timeout_secs: Some(1800),
            keep_alive: Some(" 2h ".into()),
            ..Default::default()
        };
        assert_eq!(explicit.first_token_timeout_secs(), 1800);
        assert_eq!(explicit.keep_alive(), "2h");
    }
}
