//! The unified command registry.
//!
//! One table defines every command: canonical name, aliases, summary, usage,
//! category, which surfaces support it (TUI / non-interactive CLI), whether it
//! opens an interactive view, and whether it needs confirmation. Both `snx`
//! subcommands and TUI slash commands resolve here, so the two surfaces
//! cannot drift.

/// Stable handler identity; `exec` dispatches on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandId {
    Help,
    New,
    Clear,
    Title,
    Summary,
    Continue,
    Exit,
    Status,
    Usage,
    Setup,
    Init,
    Model,
    Models,
    Login,
    Logout,
    Auth,
    Agent,
    Agents,
    Persona,
    Profile,
    Goal,
    Goals,
    Plan,
    Task,
    Subagents,
    Resume,
    Sessions,
    Pause,
    Cancel,
    Context,
    Details,
    Transcript,
    Export,
    Compact,
    Memory,
    Skills,
    Mcp,
    Connector,
    Tools,
    Permissions,
    Sandbox,
    Diff,
    Changes,
    Revert,
    Branch,
    Commit,
    Test,
    Logs,
    Audit,
    Config,
    Theme,
    Thinking,
    Welcome,
    About,
    Btw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCategory {
    Session,
    Goals,
    Models,
    Auth,
    Workspace,
    Inspection,
    System,
}

impl CommandCategory {
    pub fn label(&self) -> &'static str {
        match self {
            CommandCategory::Session => "session",
            CommandCategory::Goals => "goals",
            CommandCategory::Models => "models",
            CommandCategory::Auth => "auth",
            CommandCategory::Workspace => "workspace",
            CommandCategory::Inspection => "inspection",
            CommandCategory::System => "system",
        }
    }
}

/// One command's metadata. `usage` documents arguments; an empty `usage`
/// means the command takes none.
#[derive(Debug, Clone, Copy)]
pub struct CommandDef {
    pub id: CommandId,
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub summary: &'static str,
    pub usage: &'static str,
    pub category: CommandCategory,
    /// Available as a TUI slash command.
    pub interactive: bool,
    /// Available through the non-interactive `snx` CLI.
    pub non_interactive: bool,
    /// Opens an interactive view/modal in the TUI (vs. printing a report).
    pub opens_view: bool,
    /// Destructive or irreversible: the TUI asks before executing.
    pub requires_confirmation: bool,
}

/// The single command table.
pub const COMMANDS: &[CommandDef] = &[
    CommandDef {
        id: CommandId::Help,
        name: "help",
        aliases: &["h", "?"],
        summary: "Command reference and keybindings",
        usage: "[command]",
        category: CommandCategory::System,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::New,
        name: "new",
        aliases: &[],
        summary: "Start a fresh session (transcript and context)",
        usage: "",
        category: CommandCategory::Session,
        interactive: true,
        non_interactive: false,
        opens_view: false,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Clear,
        name: "clear",
        aliases: &[],
        summary: "Clear the visible transcript (session history is kept)",
        usage: "",
        category: CommandCategory::Session,
        interactive: true,
        non_interactive: false,
        opens_view: false,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Title,
        name: "title",
        aliases: &["rename"],
        summary: "Edit and persist the active session title",
        usage: "[new title]",
        category: CommandCategory::Session,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Summary,
        name: "summary",
        aliases: &["handoff"],
        summary: "Preview, save, copy, and optionally roll over a session",
        usage: "",
        category: CommandCategory::Session,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Continue,
        name: "continue",
        aliases: &["checkpoint"],
        summary: "Checkpoint active work into a linked continuation session",
        usage: "[session-id]",
        category: CommandCategory::Session,
        interactive: true,
        non_interactive: true,
        opens_view: false,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Exit,
        name: "exit",
        aliases: &["quit", "q"],
        summary: "Leave the TUI",
        usage: "",
        category: CommandCategory::System,
        interactive: true,
        non_interactive: false,
        opens_view: false,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Status,
        name: "status",
        aliases: &[],
        summary: "Live harness status: session, goal, model, sandbox, git",
        usage: "",
        category: CommandCategory::Inspection,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Model,
        name: "model",
        aliases: &[],
        summary: "Pick the active model from connected providers (interactive)",
        usage: "[use <name> | clear]",
        category: CommandCategory::Models,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Usage,
        name: "usage",
        aliases: &["limits"],
        summary: "Plan limits and usage per provider (codex: 5h/weekly windows, resets)",
        usage: "",
        category: CommandCategory::Inspection,
        interactive: true,
        non_interactive: true,
        opens_view: false,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Setup,
        name: "setup",
        aliases: &["onboard"],
        summary: "First-run onboarding: detect runtimes/models and write a starter config",
        usage: "",
        category: CommandCategory::System,
        interactive: true,
        non_interactive: true,
        opens_view: false,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Init,
        name: "init",
        aliases: &[],
        summary: "Detect or create canonical project instructions",
        usage: "[preview|write|git]",
        category: CommandCategory::Workspace,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Models,
        name: "models",
        aliases: &[],
        summary: "List configured models and their health",
        usage: "[health]",
        category: CommandCategory::Models,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Login,
        name: "connect",
        aliases: &["login", "provider"],
        summary: "Connect / authenticate a provider (interactive menu)",
        usage: "[provider]",
        category: CommandCategory::Auth,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Logout,
        name: "logout",
        aliases: &[],
        summary: "Remove a NEXUS credential profile",
        usage: "[provider]",
        category: CommandCategory::Auth,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: true,
    },
    CommandDef {
        id: CommandId::Auth,
        name: "auth",
        aliases: &[],
        summary: "Credential store status and profiles",
        usage: "[status | profiles | use-existing | revoke-existing | use-existing-claude | revoke-existing-claude | remove <provider> <profile>]",
        category: CommandCategory::Auth,
        interactive: true,
        non_interactive: true,
        opens_view: false,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Agent,
        name: "agent",
        aliases: &[],
        summary: "Show or set the active agent role",
        usage: "[role]",
        category: CommandCategory::Session,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Agents,
        name: "agents",
        aliases: &[],
        summary: "List agent roles and their charters",
        usage: "",
        category: CommandCategory::Session,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Persona,
        name: "persona",
        aliases: &[],
        summary: "Create, inherit, edit, and select agent personas",
        usage: "[list|create|clone|edit|delete|select]",
        category: CommandCategory::Session,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Profile,
        name: "profile",
        aliases: &["preferences"],
        summary: "Approved workflow traits and review queue",
        usage: "[list|review|select|add|approve|reject|delete|proposals|approve-proposal|reject-proposal]",
        category: CommandCategory::Session,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Goal,
        name: "goal",
        aliases: &[],
        summary: "Create or manage a goal (fast path: /goal <objective>)",
        usage: "[<objective> | show <id> | verify <id> | export <id>]",
        category: CommandCategory::Goals,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Goals,
        name: "goals",
        aliases: &[],
        summary: "List goals in this workspace",
        usage: "",
        category: CommandCategory::Goals,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Plan,
        name: "plan",
        aliases: &[],
        summary: "Create, approve, run, pause, replan, verify, and export durable plans",
        usage: "[create|edit|approve|run|pause|resume|replan|verify|history|export]",
        category: CommandCategory::Goals,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Task,
        name: "task",
        aliases: &["tasks"],
        summary: "Create and manage persistent background tasks",
        usage: "[create|list|show|logs|pause|resume|cancel|retry|attach|result|cleanup]",
        category: CommandCategory::Goals,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Subagents,
        name: "subagents",
        aliases: &["delegates"],
        summary: "Spawn, inspect, steer, wait for, collect, or cancel subagents",
        usage: "[spawn|fanout|list|tree|show|steer|cancel|wait|collect|retry]",
        category: CommandCategory::Session,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Resume,
        name: "resume",
        aliases: &[],
        summary: "Resume a session or recover an interrupted goal",
        usage: "[session-id | goal-id]",
        category: CommandCategory::Session,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Sessions,
        name: "sessions",
        aliases: &[],
        summary: "List recent sessions",
        usage: "",
        category: CommandCategory::Session,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Pause,
        name: "pause",
        aliases: &[],
        summary: "Pause the active goal",
        usage: "[goal-id]",
        category: CommandCategory::Goals,
        interactive: true,
        non_interactive: true,
        opens_view: false,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Cancel,
        name: "cancel",
        aliases: &[],
        summary: "Cancel the active goal (terminal state)",
        usage: "[goal-id]",
        category: CommandCategory::Goals,
        interactive: true,
        non_interactive: true,
        opens_view: false,
        requires_confirmation: true,
    },
    CommandDef {
        id: CommandId::Context,
        name: "context",
        aliases: &[],
        summary: "Context window usage for the active session",
        usage: "",
        category: CommandCategory::Inspection,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Details,
        name: "details",
        aliases: &[],
        summary: "Transcript card detail level",
        usage: "[compact|expanded|raw]",
        category: CommandCategory::Inspection,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Transcript,
        name: "transcript",
        aliases: &["filter"],
        summary: "Filter the durable execution timeline",
        usage: "[all|messages|plans|tools|diffs|agents|warnings|errors]",
        category: CommandCategory::Inspection,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Export,
        name: "export",
        aliases: &[],
        summary: "Export the redacted timeline as Markdown or JSONL",
        usage: "<markdown|jsonl> [path]",
        category: CommandCategory::Inspection,
        interactive: true,
        non_interactive: true,
        opens_view: false,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Compact,
        name: "compact",
        aliases: &[],
        summary: "Compact the active session's context",
        usage: "",
        category: CommandCategory::Session,
        interactive: true,
        non_interactive: false,
        opens_view: false,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Memory,
        name: "memory",
        aliases: &[],
        summary: "Long-term memory: list, search, add, forget",
        usage: "[search <text> | add <text> | forget <id>]",
        category: CommandCategory::Workspace,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Skills,
        name: "skills",
        aliases: &[],
        summary: "List and toggle skills",
        usage: "[enable <name> | disable <name>]",
        category: CommandCategory::Workspace,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Mcp,
        name: "mcp",
        aliases: &[],
        summary: "MCP servers: list, trust, tools",
        usage: "[trust <name> | untrust <name> | tools <name>]",
        category: CommandCategory::Workspace,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Connector,
        name: "connector",
        aliases: &["connectors"],
        summary: "Discover and safely import MCP servers or Agent Skills",
        usage: "[list|show <candidate-id>|import <candidate-id>]",
        category: CommandCategory::Workspace,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Tools,
        name: "tools",
        aliases: &[],
        summary: "Available tools and their risk levels",
        usage: "[name]",
        category: CommandCategory::Inspection,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Permissions,
        name: "permissions",
        aliases: &[],
        summary: "Approval mode: read-only, default, auto-edit, or full access",
        usage: "[show|read-only|default|auto-edit|full-access]",
        category: CommandCategory::Inspection,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Sandbox,
        name: "sandbox",
        aliases: &[],
        summary: "Sandbox backend, isolation level, and self-test",
        usage: "[show|on|off|backend <name>|network <mode>|test [command…]]",
        category: CommandCategory::Inspection,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Diff,
        name: "diff",
        aliases: &[],
        summary: "Working-tree diff of the workspace",
        usage: "[path]",
        category: CommandCategory::Workspace,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Changes,
        name: "changes",
        aliases: &[],
        summary: "Files changed this session and in the working tree",
        usage: "",
        category: CommandCategory::Workspace,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Revert,
        name: "revert",
        aliases: &[],
        summary: "Discard working-tree changes to a file (git checkout)",
        usage: "<path>",
        category: CommandCategory::Workspace,
        interactive: true,
        non_interactive: true,
        opens_view: false,
        requires_confirmation: true,
    },
    CommandDef {
        id: CommandId::Branch,
        name: "branch",
        aliases: &[],
        summary: "Local Git status, diff, stage, restore, log, and branches",
        usage: "[list|status|diff [--staged] [path]|stage <path>...|unstage <path>...|restore <path>|create <name>|switch <name>|delete <name>|log]",
        category: CommandCategory::Workspace,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Commit,
        name: "commit",
        aliases: &[],
        summary: "Review selected files and create a local Git commit",
        usage: "[<message> -- <path>...]",
        category: CommandCategory::Workspace,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: true,
    },
    CommandDef {
        id: CommandId::Test,
        name: "test",
        aliases: &[],
        summary: "Run the configured test command in the sandbox",
        usage: "[command…]",
        category: CommandCategory::Workspace,
        interactive: true,
        non_interactive: true,
        opens_view: false,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Logs,
        name: "logs",
        aliases: &[],
        summary: "Log locations and recent structured log lines",
        usage: "",
        category: CommandCategory::Inspection,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Audit,
        name: "audit",
        aliases: &[],
        summary: "Recent audit events",
        usage: "[kind]",
        category: CommandCategory::Inspection,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Config,
        name: "config",
        aliases: &[],
        summary: "Effective configuration (secrets redacted)",
        usage: "[path]",
        category: CommandCategory::Inspection,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Theme,
        name: "theme",
        aliases: &[],
        summary: "Switch the UI theme",
        usage: "[name]",
        category: CommandCategory::System,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Thinking,
        name: "thinking",
        aliases: &["trace"],
        summary: "Reasoning summaries and operational trace visibility",
        usage: "[on|off|toggle]",
        category: CommandCategory::System,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Welcome,
        name: "welcome",
        aliases: &[],
        summary: "Open the NEXUS welcome and onboarding screen",
        usage: "",
        category: CommandCategory::System,
        interactive: true,
        non_interactive: true,
        opens_view: true,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::About,
        name: "about",
        aliases: &["version"],
        summary: "Version, brand, and build information",
        usage: "",
        category: CommandCategory::System,
        interactive: true,
        non_interactive: true,
        opens_view: false,
        requires_confirmation: false,
    },
    CommandDef {
        id: CommandId::Btw,
        name: "btw",
        aliases: &["note"],
        summary: "Concurrent read-only sidecar over transcript, activity, status, and diff",
        usage: "<question> [--add]",
        category: CommandCategory::Session,
        interactive: true,
        non_interactive: false,
        opens_view: false,
        requires_confirmation: false,
    },
];

/// Look a command up by canonical name or alias (case-insensitive).
pub fn find(name: &str) -> Option<&'static CommandDef> {
    let name = name.to_ascii_lowercase();
    COMMANDS
        .iter()
        .find(|c| c.name == name || c.aliases.contains(&name.as_str()))
}

/// Fuzzy-match commands for the palette: subsequence match on name/aliases,
/// ranked by (prefix match, match tightness, name length). An empty query
/// returns everything in table order.
pub fn fuzzy(query: &str) -> Vec<&'static CommandDef> {
    let q = query.to_ascii_lowercase();
    if q.is_empty() {
        return COMMANDS.iter().collect();
    }
    let mut scored: Vec<(i64, &CommandDef)> = COMMANDS
        .iter()
        .filter_map(|c| {
            let best = std::iter::once(c.name)
                .chain(c.aliases.iter().copied())
                .filter_map(|n| subsequence_score(&q, n))
                .max();
            best.map(|s| (s, c))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(b.1.name)));
    scored.into_iter().map(|(_, c)| c).collect()
}

/// Score `query` as a subsequence of `target`; `None` when it isn't one.
/// Higher is better: prefix matches and tight matches win.
fn subsequence_score(query: &str, target: &str) -> Option<i64> {
    let mut score: i64 = 0;
    let mut t = target.chars().enumerate();
    let mut last_index: i64 = -1;
    for qc in query.chars() {
        let (idx, _) = t.by_ref().find(|(_, tc)| *tc == qc)?;
        let idx = idx as i64;
        if idx == last_index + 1 {
            score += 8; // consecutive
        }
        score -= idx - last_index; // gaps cost
        last_index = idx;
    }
    if target.starts_with(query) {
        score += 100;
    }
    score += 20 - target.len() as i64; // shorter names first on ties
    Some(score)
}

/// "Did you mean …" candidates for an unknown command: edit distance ≤ 2
/// against names and aliases, or a name that starts with the input.
pub fn suggest(unknown: &str) -> Vec<&'static CommandDef> {
    let q = unknown.to_ascii_lowercase();
    let mut out: Vec<(usize, &CommandDef)> = COMMANDS
        .iter()
        .filter_map(|c| {
            let best = std::iter::once(c.name)
                .chain(c.aliases.iter().copied())
                .map(|n| {
                    if n.starts_with(&q) || q.starts_with(n) {
                        1
                    } else {
                        levenshtein(&q, n)
                    }
                })
                .min()
                .unwrap_or(usize::MAX);
            (best <= 2).then_some((best, c))
        })
        .collect();
    out.sort_by_key(|(d, c)| (*d, c.name));
    out.into_iter().map(|(_, c)| c).take(3).collect()
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_names_are_unique_and_lowercase() {
        let mut seen = std::collections::HashSet::new();
        for c in COMMANDS {
            assert_eq!(c.name, c.name.to_ascii_lowercase());
            assert!(seen.insert(c.name), "duplicate command {}", c.name);
            for a in c.aliases {
                assert!(seen.insert(*a), "alias {a} collides");
            }
        }
    }

    #[test]
    fn find_resolves_aliases_case_insensitively() {
        assert_eq!(find("q").map(|c| c.name), Some("exit"));
        assert_eq!(find("Note").map(|c| c.name), Some("btw"));
        assert_eq!(find("STATUS").map(|c| c.name), Some("status"));
        assert!(find("nope").is_none());
    }

    #[test]
    fn fuzzy_go_prefers_goal_family() {
        let hits: Vec<&str> = fuzzy("go").iter().map(|c| c.name).take(3).collect();
        assert!(hits.contains(&"goal"), "got {hits:?}");
        assert!(hits.contains(&"goals"), "got {hits:?}");
    }

    #[test]
    fn fuzzy_empty_returns_all() {
        assert_eq!(fuzzy("").len(), COMMANDS.len());
    }

    #[test]
    fn suggests_goal_for_gola() {
        let names: Vec<&str> = suggest("gola").iter().map(|c| c.name).collect();
        assert!(names.contains(&"goal"), "got {names:?}");
    }

    #[test]
    fn suggests_nothing_for_gibberish() {
        assert!(suggest("xyzzyplugh").is_empty());
    }

    #[test]
    fn every_required_command_is_registered() {
        for name in [
            "help",
            "new",
            "clear",
            "title",
            "summary",
            "continue",
            "exit",
            "status",
            "usage",
            "setup",
            "init",
            "connect",
            "login", // alias of /connect
            "model",
            "models",
            "login",
            "logout",
            "agent",
            "agents",
            "persona",
            "profile",
            "goal",
            "goals",
            "plan",
            "task",
            "subagents",
            "resume",
            "pause",
            "cancel",
            "context",
            "details",
            "transcript",
            "export",
            "compact",
            "memory",
            "skills",
            "mcp",
            "connector",
            "tools",
            "permissions",
            "sandbox",
            "diff",
            "changes",
            "revert",
            "branch",
            "commit",
            "test",
            "logs",
            "config",
            "theme",
            "thinking",
            "about",
            "btw",
        ] {
            let def = find(name).unwrap_or_else(|| panic!("missing command {name}"));
            assert!(def.interactive, "{name} must be TUI-available");
        }
    }
}
