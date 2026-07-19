//! Layered policy engine.
//!
//! Policy layers, evaluated most-specific first:
//! execution decision → session grants → goal → agent → project → user/global
//! → built-in defaults. The first layer with an opinion wins, except that
//! **deny is sticky**: once any layer denies, no lower-precedence layer can
//! re-allow, and destructive/external risk can never resolve better than
//! `ask` from configuration alone.

pub mod commands;

use nexus_core::config::PolicyConfig;
use nexus_core::{Decision, RiskLevel};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::RwLock;

/// The action being evaluated, in normalized form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    /// Tool name, e.g. `fs.write_file`, `terminal.run`.
    pub tool: String,
    /// Declared risk level of the tool for this invocation.
    pub risk: RiskLevel,
    /// Workspace-relative paths this action touches, when applicable.
    pub paths: Vec<String>,
    /// Stable file-format ids included in this operation.
    #[serde(default)]
    pub formats: Vec<String>,
    /// Normalized command line (terminal tools).
    pub command: Option<String>,
    /// Structured analysis for terminal commands. Raw shell and unprovable
    /// argv are explicitly one-time-only.
    pub command_analysis: Option<commands::CommandAnalysis>,
    /// Network destination host (web/network tools).
    pub destination: Option<String>,
    /// One-line human summary shown in approval prompts.
    pub summary: String,
}

impl ActionRequest {
    pub fn session_grant_allowed(&self) -> bool {
        let proved_command = self.command.is_some()
            && self
                .command_analysis
                .as_ref()
                .is_some_and(commands::CommandAnalysis::session_grant_allowed);
        let scoped_filesystem = self.command.is_none()
            && !self.paths.is_empty()
            && (self.tool.starts_with("fs.") || self.tool.starts_with("repo."));
        (proved_command || scoped_filesystem) && self.risk < RiskLevel::Destructive
    }
}

/// Result of a policy evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyOutcome {
    pub decision: Decision,
    /// Which layer decided, e.g. `builtin`, `project`, `session_grant`, `scope`.
    pub layer: String,
    pub reason: String,
}

impl PolicyOutcome {
    fn new(decision: Decision, layer: &str, reason: impl Into<String>) -> Self {
        Self {
            decision,
            layer: layer.to_string(),
            reason: reason.into(),
        }
    }
}

/// Optional per-goal or per-agent policy narrowing. A scope can only make
/// policy stricter (deny tools, restrict paths); it cannot grant what the
/// project policy denies.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolicyScope {
    /// Tools explicitly denied in this scope.
    pub denied_tools: Vec<String>,
    /// If non-empty, only these tool name prefixes are allowed.
    pub allowed_tool_prefixes: Vec<String>,
    /// Additional path prefixes writes are restricted to.
    pub allowed_paths: Vec<String>,
    /// Paths prohibited in this scope.
    pub prohibited_paths: Vec<String>,
}

pub struct PolicyEngine {
    config: PolicyConfig,
    /// `allow_session` grants accumulated during this session, keyed by a
    /// grant token (tool name or an exact normalized command).
    session_grants: RwLock<HashSet<String>>,
    /// Scopes pushed by the active goal/agent (strictest wins).
    scopes: RwLock<Vec<(String, PolicyScope)>>,
}

impl PolicyEngine {
    pub fn new(config: PolicyConfig) -> Self {
        Self {
            config,
            session_grants: RwLock::new(HashSet::new()),
            scopes: RwLock::new(Vec::new()),
        }
    }

    pub fn config(&self) -> &PolicyConfig {
        &self.config
    }

    /// Push a named scope (e.g. for a running goal). Pop with [`Self::pop_scope`].
    pub fn push_scope(&self, name: &str, scope: PolicyScope) {
        if let Ok(mut s) = self.scopes.write() {
            s.push((name.to_string(), scope));
        }
    }

    pub fn pop_scope(&self, name: &str) {
        if let Ok(mut s) = self.scopes.write() {
            if let Some(pos) = s.iter().rposition(|(n, _)| n == name) {
                s.remove(pos);
            }
        }
    }

    /// Record an `allow_session` grant after user approval.
    pub fn grant_session(&self, token: &str) {
        if let Ok(mut g) = self.session_grants.write() {
            g.insert(token.to_string());
        }
    }

    /// Grant token for an action: command actions key on the exact normalized
    /// argv, while non-command actions key on the tool name.
    pub fn grant_token(action: &ActionRequest) -> String {
        match (&action.command, &action.command_analysis) {
            (Some(_), Some(analysis)) if analysis.session_grant_allowed() => {
                let command = serde_json::to_string(&analysis.approval_scope()).unwrap_or_default();
                format!("cmd:{command}")
            }
            (None, _) if action.session_grant_allowed() => {
                let paths = serde_json::to_string(&action.paths).unwrap_or_default();
                let formats = serde_json::to_string(&action.formats).unwrap_or_default();
                format!("path:{}:{paths}:{formats}", action.tool)
            }
            _ => format!("once-only:{}", action.tool),
        }
    }

    /// Evaluate an action against every layer.
    pub fn evaluate(&self, action: &ActionRequest) -> PolicyOutcome {
        // 1. Hard built-in denials that no configuration can override.
        if action.risk == RiskLevel::Privileged {
            return PolicyOutcome::new(
                Decision::Deny,
                "builtin",
                "privileged actions are denied by default",
            );
        }
        if let Some(cmd) = &action.command {
            let denied = action
                .command_analysis
                .as_ref()
                .and_then(|analysis| {
                    commands::hard_denied_analysis(analysis, &self.config.denied_commands)
                })
                .or_else(|| commands::hard_denied(cmd, &self.config.denied_commands));
            if let Some(reason) = denied {
                return PolicyOutcome::new(Decision::Deny, "builtin", reason);
            }
        }

        if action.risk == RiskLevel::Read && !action.paths.is_empty() {
            for path in &action.paths {
                if nexus_core::file_formats::classify(std::path::Path::new(path)).hard_denied {
                    return PolicyOutcome::new(
                        Decision::Deny,
                        "hard_safety",
                        format!("read of locked path `{path}` is denied"),
                    );
                }
            }
            let traversal = matches!(
                action.tool.as_str(),
                "fs.list_dir" | "fs.search_text" | "fs.find_files" | "repo.status" | "repo.diff"
            );
            let mut ask = Vec::new();
            for format in &action.formats {
                let decision = self
                    .config
                    .read_formats
                    .get(format)
                    .or_else(|| self.config.read_formats.get("other"))
                    .map(String::as_str)
                    .unwrap_or(self.config.reads.as_str());
                if decision == "deny" && !traversal {
                    return PolicyOutcome::new(
                        Decision::Deny,
                        "read_format",
                        format!("format `{format}` is denied"),
                    );
                }
                if decision == "ask" {
                    ask.push(format.clone());
                }
            }
            if !ask.is_empty() {
                ask.sort();
                ask.dedup();
                if self.session_grants.read().is_ok_and(|grants| {
                    grants.contains(&Self::grant_token(action)) && action.session_grant_allowed()
                }) {
                    return PolicyOutcome::new(
                        Decision::AllowSession,
                        "session_grant",
                        "approved formats and normalized paths earlier this session",
                    );
                }
                return PolicyOutcome::new(
                    Decision::Ask,
                    "read_format",
                    format!("read formats require approval: {}", ask.join(", ")),
                );
            }
        }

        // 2. Scope restrictions (goal/agent) — can only tighten.
        if let Some(outcome) = self.evaluate_scopes(action) {
            return outcome;
        }

        // 3. Explicitly allowed command prefixes from project/user config.
        if action.command.is_some() {
            let allowed = action.command_analysis.as_ref().is_some_and(|analysis| {
                commands::prefix_allowed_analysis(analysis, &self.config.allowed_commands)
            });
            if allowed {
                // Destructive risk still needs an ask even when allowlisted.
                if action.risk >= RiskLevel::Destructive {
                    return PolicyOutcome::new(
                        Decision::Ask,
                        "builtin",
                        "destructive actions always require confirmation",
                    );
                }
                return PolicyOutcome::new(
                    Decision::Allow,
                    "project",
                    "command allowlisted in policy.allowed_commands",
                );
            }
        }

        // 4. Session grants from earlier `allow_session` approvals.
        if let Ok(grants) = self.session_grants.read() {
            if grants.contains(&Self::grant_token(action)) && action.session_grant_allowed() {
                return PolicyOutcome::new(
                    Decision::AllowSession,
                    "session_grant",
                    "approved earlier this session",
                );
            }
        }

        // 5. Category defaults from configuration.
        let (category, configured) = self.category_decision(action);
        let decision = parse_decision(configured);
        // Destructive/external can never resolve better than Ask via config;
        // Config::validate enforces this, and we enforce it again here.
        let decision = if action.risk >= RiskLevel::Destructive && decision == Decision::Allow {
            Decision::Ask
        } else {
            decision
        };
        PolicyOutcome::new(
            decision,
            "project",
            format!("policy.{category} = {configured}"),
        )
    }

    fn evaluate_scopes(&self, action: &ActionRequest) -> Option<PolicyOutcome> {
        let scopes = self.scopes.read().ok()?;
        for (name, scope) in scopes.iter() {
            if scope.denied_tools.iter().any(|t| t == &action.tool) {
                return Some(PolicyOutcome::new(
                    Decision::Deny,
                    "scope",
                    format!("tool `{}` denied by scope `{name}`", action.tool),
                ));
            }
            if !scope.allowed_tool_prefixes.is_empty()
                && !scope
                    .allowed_tool_prefixes
                    .iter()
                    .any(|p| action.tool.starts_with(p.as_str()))
            {
                return Some(PolicyOutcome::new(
                    Decision::Deny,
                    "scope",
                    format!(
                        "tool `{}` outside allowed set of scope `{name}`",
                        action.tool
                    ),
                ));
            }
            for path in &action.paths {
                if scope
                    .prohibited_paths
                    .iter()
                    .any(|p| path.starts_with(p.as_str()))
                {
                    return Some(PolicyOutcome::new(
                        Decision::Deny,
                        "scope",
                        format!("path `{path}` prohibited by scope `{name}`"),
                    ));
                }
                if action.risk >= RiskLevel::Write
                    && !scope.allowed_paths.is_empty()
                    && !scope
                        .allowed_paths
                        .iter()
                        .any(|p| path.starts_with(p.as_str()))
                {
                    return Some(PolicyOutcome::new(
                        Decision::Deny,
                        "scope",
                        format!("write to `{path}` outside allowed paths of scope `{name}`"),
                    ));
                }
            }
        }
        None
    }

    fn category_decision(&self, action: &ActionRequest) -> (&'static str, &str) {
        match action.risk {
            RiskLevel::Read => ("reads", self.config.reads.as_str()),
            RiskLevel::Network => {
                if action.tool.contains("download") {
                    ("downloads", self.config.downloads.as_str())
                } else {
                    ("network", self.config.network.as_str())
                }
            }
            RiskLevel::Write => {
                if action.command.is_some() {
                    ("commands", self.config.commands.as_str())
                } else {
                    ("writes", self.config.writes.as_str())
                }
            }
            RiskLevel::Destructive => ("destructive", self.config.destructive.as_str()),
            RiskLevel::Privileged => ("destructive", "deny"),
            RiskLevel::ExternalSideEffect => ("external", self.config.external.as_str()),
        }
    }
}

fn parse_decision(s: &str) -> Decision {
    match s {
        "allow" => Decision::Allow,
        "deny" => Decision::Deny,
        _ => Decision::Ask,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> PolicyEngine {
        PolicyEngine::new(PolicyConfig::default())
    }

    fn action(tool: &str, risk: RiskLevel) -> ActionRequest {
        ActionRequest {
            tool: tool.into(),
            risk,
            paths: vec![],
            formats: vec![],
            command: None,
            command_analysis: None,
            destination: None,
            summary: String::new(),
        }
    }

    #[test]
    fn reads_allowed_by_default() {
        let out = engine().evaluate(&action("fs.read_file", RiskLevel::Read));
        assert_eq!(out.decision, Decision::Allow);
    }

    #[test]
    fn format_rules_bind_grants_and_new_denials_win() {
        let mut config = PolicyConfig::default();
        config.read_formats.insert("toml".into(), "ask".into());
        let engine = PolicyEngine::new(config);
        let mut request = action("fs.read_file", RiskLevel::Read);
        request.paths = vec!["Cargo.toml".into()];
        request.formats = vec!["toml".into()];
        assert_eq!(engine.evaluate(&request).decision, Decision::Ask);
        engine.grant_session(&PolicyEngine::grant_token(&request));
        assert_eq!(engine.evaluate(&request).decision, Decision::AllowSession);

        let mut denied = PolicyConfig::default();
        denied.read_formats.insert("toml".into(), "deny".into());
        let denied_engine = PolicyEngine::new(denied);
        denied_engine.grant_session(&PolicyEngine::grant_token(&request));
        assert_eq!(denied_engine.evaluate(&request).decision, Decision::Deny);
    }

    #[test]
    fn hard_sensitive_paths_are_never_unlockable() {
        let mut request = action("fs.read_file", RiskLevel::Read);
        request.paths = vec![".env.example".into()];
        request.formats = vec!["other".into()];
        assert_eq!(engine().evaluate(&request).decision, Decision::Deny);
    }

    #[test]
    fn writes_ask_by_default() {
        let out = engine().evaluate(&action("fs.write_file", RiskLevel::Write));
        assert_eq!(out.decision, Decision::Ask);
    }

    #[test]
    fn privileged_always_denied() {
        let cfg = PolicyConfig {
            commands: "allow".into(),
            ..Default::default()
        };
        let e = PolicyEngine::new(cfg);
        let out = e.evaluate(&action("terminal.run", RiskLevel::Privileged));
        assert_eq!(out.decision, Decision::Deny);
    }

    #[test]
    fn sudo_hard_denied_even_when_commands_allowed() {
        let cfg = PolicyConfig {
            commands: "allow".into(),
            ..Default::default()
        };
        let e = PolicyEngine::new(cfg);
        let mut a = action("terminal.run", RiskLevel::Write);
        a.command = Some("sudo rm -rf /".into());
        a.command_analysis = Some(commands::analyze_shell("sudo rm -rf /"));
        assert_eq!(e.evaluate(&a).decision, Decision::Deny);
    }

    #[test]
    fn session_grant_allows_repeat() {
        let e = engine();
        let mut a = action("terminal.run", RiskLevel::Write);
        a.command = Some("cargo check".into());
        a.command_analysis = Some(commands::analyze_argv("cargo", &["check".into()]));
        assert_eq!(e.evaluate(&a).decision, Decision::Ask);
        e.grant_session(&PolicyEngine::grant_token(&a));
        assert_eq!(e.evaluate(&a).decision, Decision::AllowSession);
        // A different command, even for the same program, is not covered.
        a.command = Some("cargo publish".into());
        a.command_analysis = Some(commands::analyze_argv("cargo", &["publish".into()]));
        assert_eq!(e.evaluate(&a).decision, Decision::Ask);
        a.command = Some("npm install".into());
        a.command_analysis = Some(commands::analyze_argv("npm", &["install".into()]));
        assert_eq!(e.evaluate(&a).decision, Decision::Ask);
    }

    #[test]
    fn session_grant_never_covers_destructive() {
        let e = engine();
        let mut a = action("fs.delete_file", RiskLevel::Destructive);
        a.paths = vec!["src/main.rs".into()];
        e.grant_session(&PolicyEngine::grant_token(&a));
        assert_eq!(e.evaluate(&a).decision, Decision::Ask);
    }

    #[test]
    fn scope_restricts_write_paths() {
        let e = engine();
        e.push_scope(
            "goal_x",
            PolicyScope {
                allowed_paths: vec!["src/".into()],
                ..Default::default()
            },
        );
        let mut a = action("fs.write_file", RiskLevel::Write);
        a.paths = vec!["docs/notes.md".into()];
        assert_eq!(e.evaluate(&a).decision, Decision::Deny);
        a.paths = vec!["src/lib.rs".into()];
        assert_eq!(e.evaluate(&a).decision, Decision::Ask);
        e.pop_scope("goal_x");
        a.paths = vec!["docs/notes.md".into()];
        assert_eq!(e.evaluate(&a).decision, Decision::Ask);
    }

    #[test]
    fn allowlisted_commands_allowed() {
        let cfg = PolicyConfig {
            allowed_commands: vec!["cargo check".into()],
            ..Default::default()
        };
        let e = PolicyEngine::new(cfg);
        let mut a = action("terminal.run", RiskLevel::Write);
        a.command = Some("cargo check --all".into());
        a.command_analysis = Some(commands::analyze_argv(
            "cargo",
            &["check".into(), "--all".into()],
        ));
        assert_eq!(e.evaluate(&a).decision, Decision::Allow);
    }
}
