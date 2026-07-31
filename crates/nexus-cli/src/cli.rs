//! `snx` command-line surface (clap derive).

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "snx",
    version = nexus_core::brand::VERSION,
    about = "NEXUS by Silent Protocol — local intelligence, controlled execution.",
    long_about = "NEXUS (snx), by Silent Protocol: a local-first agentic CLI harness. Every model \
action passes through schema validation, capability checks, workspace confinement, \
policy/approval, a real sandbox, output limits, secret redaction, and audit logging.",
    propagate_version = true
)]
pub struct Cli {
    /// Disable colored output (also honors NO_COLOR and config).
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Emit machine-readable JSON where supported.
    #[arg(long, global = true)]
    pub json: bool,

    /// Increase log verbosity.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Render inline without the alternate screen (alias: --no-alt-screen).
    #[arg(long, alias = "no-alt-screen", global = true)]
    pub inline: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Launch the interactive TUI (default when no command is given).
    Chat,

    /// Show the canonical NEXUS identity, version, and build information.
    #[command(visible_alias = "version")]
    About(AboutArgs),

    /// Run one objective to completion, non-interactively.
    Run(RunArgs),

    /// Live harness status: session, goal, model, sandbox, git, context.
    Status,

    /// Plan limits and usage per provider (codex: 5h/weekly windows, resets).
    Usage,

    /// Show or pin the active model (same behavior as /model in the TUI).
    #[command(subcommand)]
    Model(ModelCmd),

    /// List resumable sessions/goals, or resume one by id.
    Resume {
        /// Session or goal id to resume (omit to list).
        id: Option<String>,
    },

    /// Create a checkpoint and linked child session for interrupted work.
    Continue {
        /// Session id; defaults to the most recent workspace session.
        id: Option<String>,
    },

    /// Build a structured handoff for a session, save/copy it, and optionally
    /// create a linked fresh rollover session.
    Summary {
        /// Session id; defaults to the last or most recent workspace session.
        #[arg(long)]
        session: Option<String>,
    },

    /// Run the configured test command (or an explicit one) in the sandbox.
    Test {
        /// Command to run; defaults to `[general] test_command` from config.
        command: Vec<String>,
    },

    /// Local Git branch operations. Remote changes are connector workflows.
    #[command(subcommand)]
    Branch(BranchCmd),

    /// Review selected files and create a local Git commit.
    Commit(CommitArgs),

    /// Show or set the UI theme.
    Theme {
        /// Theme name (omit to list).
        name: Option<String>,
    },

    /// Show or set deliberation depth: how much execution insight is shown and
    /// how much optional planning the agent performs.
    Thinking {
        /// `off`, `on`, or `auto` (omit, or `status`, to show the current mode).
        mode: Option<String>,
    },

    /// Show or set how much the agent narrates its own work.
    Narrate {
        /// `off`, `compact`, `auto`, or `verbose` (omit, or `status`, to show
        /// the current mode).
        mode: Option<String>,
    },

    /// Persistent, verifiable goals.
    #[command(subcommand)]
    Goal(GoalCmd),

    /// Conversation sessions.
    #[command(subcommand)]
    Session(SessionCmd),

    /// Durable agent personas with project/global inheritance.
    #[command(subcommand)]
    Persona(PersonaCmd),

    /// Approved workflow profile traits and review queue.
    #[command(subcommand)]
    Profile(ProfileCmd),

    /// Long-term memory (approval-gated, secret-refusing).
    #[command(subcommand)]
    Memory(MemoryCmd),

    /// Governed self-improvement: candidates, evidence, promotions, governance.
    #[command(subcommand)]
    Rsi(RsiCmd),

    /// Versioned, inspectable skills.
    #[command(subcommand)]
    Skill(SkillCmd),

    /// Model Context Protocol servers (client + server).
    #[command(subcommand)]
    Mcp(McpCmd),

    /// Discover and safely import Codex MCP servers or Agent Skills.
    #[command(subcommand)]
    Connector(ConnectorCmd),

    /// Execution sandbox inspection and self-test.
    #[command(subcommand)]
    Sandbox(SandboxCmd),

    /// Code intelligence index.
    #[command(subcommand)]
    Index(IndexCmd),

    /// Available tools and their risk levels.
    #[command(subcommand)]
    Tools(ToolsCmd),

    /// Read-only provider-grouped model catalog and health.
    Catalog(CatalogArgs),

    /// Provider authentication (Codex "Sign in with ChatGPT" session).
    #[command(subcommand)]
    Auth(AuthCmd),

    /// Configuration inspection.
    #[command(subcommand)]
    Config(ConfigCmd),

    /// Show recent audit events.
    Audit(AuditArgs),

    /// Show where structured logs are written.
    Logs,

    /// First-run onboarding: detect local runtimes/models + GPU and write a
    /// ready-to-use starter config.
    Setup(SetupArgs),

    /// Detect supported project instruction files and create AGENTS.md only
    /// when no usable instructions exist.
    Init,

    /// Environment and readiness diagnostics.
    Doctor(DoctorArgs),

    /// Explicit database, state, artifact, and backup maintenance.
    #[command(subcommand)]
    Maintenance(MaintenanceCmd),

    /// Generate a shell completion script.
    Completion { shell: clap_complete::Shell },

    /// Internal per-workspace durable task worker.
    #[command(hide = true)]
    Worker {
        #[arg(long, default_value_t = 300)]
        idle_secs: u64,
    },
}

#[derive(clap::Args, Debug, Default)]
pub struct AboutArgs {
    /// Use the compact lockup (used by installers and short status output).
    #[arg(long)]
    pub compact: bool,

    /// Render only the reusable lockup.
    #[arg(long, hide = true)]
    pub brand_only: bool,
}

#[derive(clap::Args, Debug)]
pub struct RunArgs {
    /// The objective for the agent to accomplish.
    pub objective: Vec<String>,
    /// Agent role, or the name of a custom agent. `nexus` is the flagship and
    /// the default; `/agents` in the TUI lists them all with descriptions.
    #[arg(long)]
    pub agent: Option<String>,
    /// Continue an existing session by id.
    #[arg(long)]
    pub session: Option<String>,
    /// Auto-approve escalated actions (explicit, audited authorization).
    #[arg(long)]
    pub yes: bool,
}

#[derive(Subcommand, Debug)]
pub enum ModelCmd {
    /// Show the active model and the configured list.
    Show,
    /// Pin a configured model as active (overrides task routing).
    Use { name: String },
    /// Clear the pin; config routing applies again.
    Clear,
    /// Probe every configured provider's reachability.
    Health,
    /// Run a minimal safe prompt against a model and report latency.
    Test { name: String },
}

#[derive(Subcommand, Debug)]
pub enum BranchCmd {
    List,
    Status,
    Diff {
        #[arg(long)]
        staged: bool,
        path: Option<String>,
    },
    Stage {
        #[arg(required = true)]
        paths: Vec<String>,
    },
    Unstage {
        #[arg(required = true)]
        paths: Vec<String>,
    },
    Restore {
        path: String,
        #[arg(long)]
        yes: bool,
    },
    Log {
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    Create {
        name: String,
    },
    Switch {
        name: String,
        #[arg(long)]
        yes: bool,
    },
    Delete {
        name: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(clap::Args, Debug)]
pub struct CommitArgs {
    /// Commit message.
    #[arg(short, long)]
    pub message: String,
    /// Selected path to stage and commit (repeatable).
    #[arg(long = "file", short = 'f', required = true)]
    pub files: Vec<String>,
    /// Skip the final confirmation after the diff preview.
    #[arg(long)]
    pub yes: bool,

    /// Explicitly allow repository hooks during this typed commit.
    #[arg(long)]
    pub allow_hooks: bool,
}

#[derive(Subcommand, Debug)]
pub enum GoalCmd {
    /// List goals in this workspace.
    List,
    /// Pause a goal (the active one when no id is given).
    Pause { id: Option<String> },
    /// Resume a paused goal.
    Resume { id: Option<String> },
    /// Cancel a goal (terminal; asks unless --yes).
    Cancel {
        id: Option<String>,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Show the plan of the active (or given) goal.
    Plan { id: Option<String> },
    /// Create a new goal.
    New {
        title: Vec<String>,
        /// Acceptance criterion (repeatable). A goal only completes when each
        /// has evidence.
        #[arg(long = "criterion", short = 'c')]
        criteria: Vec<String>,
        #[arg(long)]
        objective: Option<String>,
        /// Total provider tokens allowed for this goal; zero means unlimited.
        #[arg(long, default_value_t = 0)]
        token_budget: i64,
    },
    /// Show a goal and its steps/evidence.
    Show { id: String },
    /// Verify a goal's acceptance criteria against recorded evidence.
    Verify { id: String },
    /// List goals recoverable after an interrupted run.
    Recover,
    /// Export a goal as JSON.
    Export { id: String },
}

#[derive(Subcommand, Debug)]
pub enum SessionCmd {
    /// List recent sessions.
    List,
    /// Show a session's messages.
    Show { id: String },
    /// Persist a title for a session.
    Title { id: String, title: Vec<String> },
}

#[derive(Subcommand, Debug)]
pub enum PersonaCmd {
    List,
    Create {
        name: String,
        instructions: Vec<String>,
        #[arg(long, default_value = "project")]
        scope: String,
        #[arg(long)]
        parent: Option<String>,
        #[arg(long, default_value = "")]
        description: String,
    },
    Clone {
        source: String,
        new_name: String,
        #[arg(long, default_value = "project")]
        scope: String,
    },
    Edit {
        id: String,
        instructions: Vec<String>,
    },
    Delete {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    Select {
        id: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProfileCmd {
    List {
        #[arg(long)]
        all: bool,
    },
    Add {
        key: String,
        value: Vec<String>,
    },
    Select {
        name: String,
    },
    Approve {
        id: String,
    },
    Reject {
        id: String,
    },
    Delete {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    Proposals {
        #[arg(long)]
        all: bool,
    },
    ApproveProposal {
        id: String,
    },
    RejectProposal {
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum RsiCmd {
    /// Observation state, candidate queue, and the last promotion.
    Status,
    /// Every candidate with its declared and classified risk tier.
    Candidates,
    /// One candidate: evidence, success metrics, WARP classification.
    Show { id: String },
    /// Redacted harness events the candidates are built from.
    Observations,
    /// Multi-dimensional outcome scores for finished tasks.
    Outcomes,
    /// Promotions recorded for this workspace.
    Promotions,
    /// Rollbacks recorded against the latest promotion.
    Rollbacks,
    /// The compile-time governance ruleset.
    Governance,
}

#[derive(Subcommand, Debug)]
pub enum MemoryCmd {
    /// List stored memories.
    List {
        #[arg(long)]
        all: bool,
    },
    /// Add a memory (secrets are refused).
    Add {
        content: Vec<String>,
        #[arg(long, default_value = "project_fact")]
        kind: String,
        #[arg(long, default_value = "project")]
        scope: String,
    },
    /// Full-text search memories.
    Search { query: Vec<String> },
    /// Approve a memory that required review.
    Approve { id: String },
    /// Delete a memory.
    Forget {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Delete expired memories.
    Prune,
    /// Export all memories as JSON.
    Export,
}

#[derive(Subcommand, Debug)]
pub enum SkillCmd {
    /// List skills.
    List,
    /// Show a skill manifest.
    Show { name: String },
    /// Enable a skill (verifies referenced tools exist).
    Enable { name: String },
    /// Disable a skill.
    Disable { name: String },
    /// Import a skill manifest JSON file (stored disabled).
    Import { file: String },
    /// Export a skill manifest as JSON.
    Export { name: String },
}

#[derive(Subcommand, Debug)]
pub enum McpCmd {
    /// List registered MCP servers.
    List,
    /// Register an MCP server (stdio). This is an explicit user action.
    Add {
        name: String,
        /// Command to launch the server.
        #[arg(long)]
        command: String,
        /// Arguments passed to the server command.
        #[arg(long = "arg")]
        args: Vec<String>,
    },
    /// Remove a registered server.
    Remove { name: String },
    /// Mark a server trusted (its tools may run under normal policy).
    Trust { name: String },
    /// Mark a server untrusted (its tools require per-call approval).
    Untrust { name: String },
    /// Connect to a registered server and list its tools.
    Tools { name: String },
    /// Run NEXUS as an MCP server over stdio (curated read-only tools).
    Serve,
}

#[derive(Subcommand, Debug)]
pub enum ConnectorCmd {
    List,
    Show {
        id: String,
    },
    Import {
        id: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum SandboxCmd {
    /// Show the active backend and its honest isolation level.
    Status,
    /// Run a command inside the sandbox and report the outcome.
    Test {
        /// Command to run (defaults to a harmless probe).
        command: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum IndexCmd {
    /// Build or refresh the workspace index.
    Build,
    /// Show index statistics.
    Status,
    /// Find a symbol by name.
    Symbol { name: String },
    /// List symbols in a workspace file.
    File { path: String },
    /// Clear the index.
    Clean,
}

#[derive(Subcommand, Debug)]
pub enum ToolsCmd {
    /// List all tools and their risk levels.
    List,
    /// Show a tool's schema and metadata.
    Show { name: String },
}

#[derive(Subcommand, Debug)]
pub enum CatalogCmd {
    /// List the provider-grouped catalog.
    List,
    /// Refresh and report every eligible provider's health.
    Health,
}

#[derive(clap::Args, Debug, Default)]
pub struct CatalogArgs {
    #[command(subcommand)]
    pub command: Option<CatalogCmd>,
}

#[derive(Subcommand, Debug)]
pub enum AuthCmd {
    /// Show Codex sessions (isolated + your CLI's) and credential storage.
    Status,
    /// Log in to the isolated NEXUS Codex profile (your own `codex`
    /// CLI login is never modified).
    Login(AuthLoginArgs),
    /// Remove the isolated NEXUS Codex session only.
    Logout,
    /// List stored credential profiles (metadata only).
    Profiles,
    /// Delete a stored credential profile.
    Remove { provider: String, profile: String },
}

#[derive(clap::Args, Debug, Default)]
pub struct AuthLoginArgs {
    /// Use a one-time device code. Best for SSH/headless machines.
    #[arg(long, conflicts_with_all = ["api_key", "import", "use_existing"])]
    pub device: bool,

    /// Read an OpenAI API key and store it via the official Codex CLI
    /// (into the isolated profile).
    #[arg(long = "api-key", conflicts_with_all = ["device", "import", "use_existing"])]
    pub api_key: bool,

    /// Copy your existing Codex CLI session into the isolated NEXUS
    /// profile (the original is read once and never modified).
    #[arg(long, conflicts_with_all = ["device", "api_key", "use_existing"])]
    pub import: bool,

    /// Explicitly allow read-only use of the existing Codex CLI login without
    /// copying it into the isolated NEXUS profile.
    #[arg(long, conflicts_with_all = ["device", "api_key", "import"])]
    pub use_existing: bool,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Print the effective configuration.
    Show,
    /// Print the effective turn, token, cost, and delegation budgets.
    /// Edit them interactively with `/config budgets` in the TUI.
    Budgets,
    /// Print the config file locations.
    Path,
    /// Print the configuration JSON schema.
    Schema,
    /// Set one configuration value as a managed override.
    ///
    /// The value is TOML, so strings need quoting: `snx config set
    /// limits.self_hosted_context_window 65536`, `snx config set
    /// sandbox.backend '"none"'`.
    Set {
        /// Dotted path, e.g. `limits.self_hosted_context_window`.
        path: String,
        /// TOML value.
        value: String,
        /// Write the workspace override instead of the global one.
        #[arg(long)]
        workspace: bool,
    },
    /// Drop a managed override so the value is inherited again.
    Reset {
        /// Dotted path, e.g. `limits.self_hosted_context_window`.
        path: String,
        /// Clear the workspace override instead of the global one.
        #[arg(long)]
        workspace: bool,
    },
}

#[derive(clap::Args, Debug)]
pub struct SetupArgs {
    /// Write the project config (`.nexus/config.toml`) instead of the global one.
    #[arg(long)]
    pub project: bool,
    /// Refresh discovery and managed models; existing hand-written config is
    /// never overwritten.
    #[arg(long)]
    pub force: bool,
}

#[derive(clap::Args, Debug, Default)]
pub struct DoctorArgs {
    /// Include state integrity, permissions, release metadata, isolation, and
    /// binary-integrity checks.
    #[arg(long)]
    pub deep: bool,
}

#[derive(Subcommand, Debug)]
pub enum MaintenanceCmd {
    /// Check database integrity, permissions, WAL state, storage, and artifacts.
    Check,
    /// Create an atomic SQLite and artifact snapshot at a new directory.
    Backup { directory: String },
    /// Run PRAGMA optimize and a WAL checkpoint; optionally VACUUM.
    Optimize {
        #[arg(long)]
        vacuum: bool,
    },
}

#[derive(clap::Args, Debug)]
pub struct AuditArgs {
    /// Filter by audit event kind.
    #[arg(long)]
    pub kind: Option<String>,
    /// Maximum events to show.
    #[arg(long, default_value_t = 30)]
    pub limit: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn command_tree_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_every_thinking_mode() {
        for word in ["off", "on", "auto", "status"] {
            let cli = Cli::try_parse_from(["snx", "thinking", word]).expect("parse");
            match cli.command {
                Some(Command::Thinking { mode }) => assert_eq!(mode.as_deref(), Some(word)),
                _ => panic!("expected thinking"),
            }
        }
    }

    #[test]
    fn bare_thinking_reports_status() {
        let cli = Cli::try_parse_from(["snx", "thinking"]).expect("parse");
        match cli.command {
            Some(Command::Thinking { mode }) => assert!(mode.is_none()),
            _ => panic!("expected thinking"),
        }
    }

    #[test]
    fn an_unknown_thinking_mode_parses_and_is_rejected_at_runtime() {
        // clap accepts any string; the mode is validated by ThinkingMode so the
        // error can name all three valid values.
        let cli = Cli::try_parse_from(["snx", "thinking", "sometimes"]).expect("parse");
        match cli.command {
            Some(Command::Thinking { mode }) => {
                assert!(mode
                    .as_deref()
                    .expect("mode present")
                    .parse::<nexus_core::ThinkingMode>()
                    .is_err());
            }
            _ => panic!("expected thinking"),
        }
    }

    #[test]
    fn parses_run_with_flags() {
        let cli = Cli::try_parse_from(["snx", "run", "--agent", "planner", "--yes", "do", "it"])
            .expect("parse");
        match cli.command {
            Some(Command::Run(a)) => {
                assert_eq!(a.agent.as_deref(), Some("planner"));
                assert!(a.yes);
                assert_eq!(a.objective, vec!["do", "it"]);
            }
            _ => panic!("expected run"),
        }
    }

    #[test]
    fn no_subcommand_is_allowed() {
        let cli = Cli::try_parse_from(["snx"]).expect("parse");
        assert!(cli.command.is_none());
    }

    #[test]
    fn version_alias_uses_the_canonical_about_surface() {
        let cli = Cli::try_parse_from(["snx", "version", "--compact"]).expect("parse");
        match cli.command {
            Some(Command::About(args)) => assert!(args.compact),
            _ => panic!("expected about"),
        }
    }

    #[test]
    fn catalog_replaces_models_without_a_compatibility_alias() {
        assert!(matches!(
            Cli::try_parse_from(["snx", "catalog", "health"])
                .expect("catalog")
                .command,
            Some(Command::Catalog(CatalogArgs {
                command: Some(CatalogCmd::Health)
            }))
        ));
        assert!(matches!(
            Cli::try_parse_from(["snx", "catalog"])
                .expect("bare catalog")
                .command,
            Some(Command::Catalog(CatalogArgs { command: None }))
        ));
        let error = Cli::try_parse_from(["snx", "models", "health"])
            .expect_err("removed command must stay unknown")
            .to_string();
        assert!(error.contains("unrecognized subcommand 'models'"));
    }

    #[test]
    fn parses_auth_login_modes() {
        let cli = Cli::try_parse_from(["snx", "auth", "login", "--device"]).expect("parse");
        match cli.command {
            Some(Command::Auth(AuthCmd::Login(a))) => assert!(a.device),
            _ => panic!("expected auth login"),
        }

        let cli = Cli::try_parse_from(["snx", "auth", "login", "--api-key"]).expect("parse");
        match cli.command {
            Some(Command::Auth(AuthCmd::Login(a))) => assert!(a.api_key),
            _ => panic!("expected auth login"),
        }
    }

    #[test]
    fn auth_login_modes_conflict() {
        let err = Cli::try_parse_from(["snx", "auth", "login", "--device", "--api-key"])
            .expect_err("must conflict");
        assert!(err.to_string().contains("cannot be used with"));
    }

    #[test]
    fn parses_interactive_upgrade_public_surfaces() {
        assert!(matches!(
            Cli::try_parse_from(["snx", "init"]).expect("init").command,
            Some(Command::Init)
        ));
        assert!(matches!(
            Cli::try_parse_from(["snx", "doctor", "--deep"])
                .expect("doctor")
                .command,
            Some(Command::Doctor(DoctorArgs { deep: true }))
        ));
        assert!(matches!(
            Cli::try_parse_from(["snx", "maintenance", "optimize", "--vacuum"])
                .expect("maintenance")
                .command,
            Some(Command::Maintenance(MaintenanceCmd::Optimize {
                vacuum: true
            }))
        ));
        assert!(matches!(
            Cli::try_parse_from(["snx", "resume", "session_1"])
                .expect("resume")
                .command,
            Some(Command::Resume { id: Some(id) }) if id == "session_1"
        ));
        assert!(matches!(
            Cli::try_parse_from(["snx", "summary", "--session", "session_1"])
                .expect("summary")
                .command,
            Some(Command::Summary { session: Some(id) }) if id == "session_1"
        ));
        assert!(matches!(
            Cli::try_parse_from(["snx", "profile", "select", "focused"])
                .expect("profile")
                .command,
            Some(Command::Profile(ProfileCmd::Select { name })) if name == "focused"
        ));
        assert!(matches!(
            Cli::try_parse_from(["snx", "connector", "show", "mcp:test"])
                .expect("connector")
                .command,
            Some(Command::Connector(ConnectorCmd::Show { id })) if id == "mcp:test"
        ));
        assert!(matches!(
            Cli::try_parse_from(["snx", "branch", "diff", "--staged", "src/lib.rs"])
                .expect("branch")
                .command,
            Some(Command::Branch(BranchCmd::Diff {
                staged: true,
                path: Some(path)
            })) if path == "src/lib.rs"
        ));
        assert!(matches!(
            Cli::try_parse_from([
                "snx",
                "commit",
                "--message",
                "safe change",
                "--file",
                "src/lib.rs"
            ])
            .expect("commit")
            .command,
            Some(Command::Commit(CommitArgs { files, .. })) if files == vec!["src/lib.rs"]
        ));
    }
}
