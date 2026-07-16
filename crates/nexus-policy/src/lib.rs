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
    /// Normalized command line (terminal tools).
    pub command: Option<String>,
    /// Network destination host (web/network tools).
    pub destination: Option<String>,
    /// One-line human summary shown in approval prompts.
    pub summary: String,
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
        match &action.command {
            Some(cmd) => match commands::normalized(cmd) {
                Some(command) => format!("cmd:{command}"),
                None => format!("tool:{}", action.tool),
            },
            None => format!("tool:{}", action.tool),
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
            if let Some(reason) = commands::hard_denied(cmd, &self.config.denied_commands) {
                return PolicyOutcome::new(Decision::Deny, "builtin", reason);
            }
        }

        // 2. Scope restrictions (goal/agent) — can only tighten.
        if let Some(outcome) = self.evaluate_scopes(action) {
            return outcome;
        }

        // 3. Explicitly allowed command prefixes from project/user config.
        if let Some(cmd) = &action.command {
            if commands::prefix_allowed(cmd, &self.config.allowed_commands) {
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
            if grants.contains(&Self::grant_token(action)) && action.risk < RiskLevel::Destructive {
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
            command: None,
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
        assert_eq!(e.evaluate(&a).decision, Decision::Deny);
    }

    #[test]
    fn session_grant_allows_repeat() {
        let e = engine();
        let mut a = action("terminal.run", RiskLevel::Write);
        a.command = Some("cargo check".into());
        assert_eq!(e.evaluate(&a).decision, Decision::Ask);
        e.grant_session(&PolicyEngine::grant_token(&a));
        assert_eq!(e.evaluate(&a).decision, Decision::AllowSession);
        // A different command, even for the same program, is not covered.
        a.command = Some("cargo publish".into());
        assert_eq!(e.evaluate(&a).decision, Decision::Ask);
        a.command = Some("npm install".into());
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
        assert_eq!(e.evaluate(&a).decision, Decision::Allow);
    }
}
