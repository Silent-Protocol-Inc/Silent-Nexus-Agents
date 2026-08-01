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

/// Name of the scope plan mode installs. Appears verbatim in denial reasons.
pub const PLAN_MODE_SCOPE: &str = "plan-mode";

impl PolicyScope {
    /// The scope plan mode runs under: inspect the workspace, submit a plan,
    /// change nothing.
    ///
    /// Deliberately an allowlist rather than a list of denied tools. A denylist
    /// silently admits whatever is added to the tool catalog next; with an
    /// allowlist a new tool is refused until someone decides it belongs in a
    /// mode whose whole promise is that nothing happens without approval.
    ///
    /// `repo.check` is absent on purpose — it runs the project's build and test
    /// commands, which is execution, however read-only its intent.
    pub fn plan_mode() -> Self {
        Self {
            allowed_tool_prefixes: [
                "fs.read_file",
                "fs.list_dir",
                "fs.find_files",
                "fs.search_text",
                "repo.git_",
                "diag.",
                "plan.submit",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            ..Default::default()
        }
    }
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

    /// Refusals that are safety rules rather than operator preferences.
    ///
    /// Privilege escalation, the denied-command list, terminal Git side
    /// effects, and reads of locked paths (`.git`, `.env`, keystores). These
    /// are not positions to reconsider per action, so no permission mode
    /// converts them — a mode is how much the operator wants to be asked, not
    /// a way to leave the security model.
    const IMMUTABLE_REFUSALS: &'static [&'static str] = &["builtin", "hard_safety"];

    fn is_immutable(outcome: &PolicyOutcome) -> bool {
        outcome.decision == Decision::Deny
            && Self::IMMUTABLE_REFUSALS.contains(&outcome.layer.as_str())
    }

    /// Session grant that stands in for the safety class under full access.
    ///
    /// One token for the whole class rather than one per action: the operator
    /// is answering "yes, this session may do privileged things", not
    /// re-deciding it per command. It lives only in this process, so leaving
    /// the session — exit, disconnect, crash — asks again next time.
    pub const FULL_ACCESS_SAFETY_GRANT: &'static str = "full-access:safety";

    /// Layer reported when full access turns a safety refusal into the one
    /// prompt that covers the session.
    pub const FULL_ACCESS_SAFETY_LAYER: &str = "full_access_safety";

    fn holds_full_access_safety_grant(&self) -> bool {
        self.session_grants
            .read()
            .is_ok_and(|grants| grants.contains(Self::FULL_ACCESS_SAFETY_GRANT))
    }

    /// Apply what the permission mode means to an evaluated outcome.
    ///
    /// The modes are a ladder of how much the operator wants to be interrupted,
    /// and the layers below did not know which rung they were on — so a role
    /// that narrows the tool set refused outright even in the mode whose whole
    /// point is that the operator decides.
    ///
    /// * **auto-edit** — a configured refusal becomes a question. The operator
    ///   is still asked; they are no longer told no.
    /// * **full access** — it is permitted, and recorded. Every action still
    ///   goes through the audit log, so "without asking" never means "without
    ///   a record".
    ///
    /// Neither touches [`IMMUTABLE_REFUSALS`](Self::IMMUTABLE_REFUSALS).
    fn apply_permission_mode(&self, outcome: PolicyOutcome) -> PolicyOutcome {
        if Self::is_immutable(&outcome) {
            // Under full access the safety class is asked about once and then
            // stands for the session. Every other mode keeps refusing: a mode
            // is how much the operator wants to be asked, and only the most
            // permissive one puts this on the table at all.
            if !self.config.is_full_access() {
                return outcome;
            }
            if self.holds_full_access_safety_grant() {
                return PolicyOutcome::new(
                    Decision::Allow,
                    Self::FULL_ACCESS_SAFETY_LAYER,
                    format!("{} — approved for this session", outcome.reason),
                );
            }
            return PolicyOutcome::new(
                Decision::Ask,
                Self::FULL_ACCESS_SAFETY_LAYER,
                format!(
                    "{} — approving permits privileged actions for the rest of this session",
                    outcome.reason
                ),
            );
        }
        if self.config.is_full_access() {
            if outcome.decision == Decision::Allow {
                return outcome;
            }
            return PolicyOutcome::new(
                Decision::Allow,
                &outcome.layer.clone(),
                format!("{} — full access permits and records it", outcome.reason),
            );
        }
        if outcome.decision == Decision::Deny && self.config.is_auto_edit() {
            return PolicyOutcome::new(
                Decision::Ask,
                &outcome.layer.clone(),
                format!("{} — auto-edit asks instead of refusing", outcome.reason),
            );
        }
        outcome
    }

    /// Evaluate an action against every layer.
    pub fn evaluate(&self, action: &ActionRequest) -> PolicyOutcome {
        self.apply_permission_mode(self.evaluate_inner(action))
    }

    fn evaluate_inner(&self, action: &ActionRequest) -> PolicyOutcome {
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

    fn full_access_engine() -> PolicyEngine {
        PolicyEngine::new(PolicyConfig {
            writes: "allow".into(),
            commands: "allow".into(),
            downloads: "allow".into(),
            ..PolicyConfig::default()
        })
    }

    fn auto_edit_engine() -> PolicyEngine {
        PolicyEngine::new(PolicyConfig {
            writes: "allow".into(),
            ..PolicyConfig::default()
        })
    }

    /// The modes are a ladder of how much the operator wants to be interrupted,
    /// and the layers below did not know which rung they were on. A read-only
    /// role installs a scope denying the tools it may not use, and that refusal
    /// reached the operator as a dead end in every mode — including the ones
    /// chosen precisely so they could decide.
    #[test]
    fn each_permission_mode_decides_what_a_configured_refusal_means() {
        let scope = PolicyScope {
            denied_tools: vec!["fs.write_file".into()],
            ..Default::default()
        };
        let request = action("fs.write_file", RiskLevel::Write);

        // default: still refused.
        let strict = engine();
        strict.push_scope("reviewer", scope.clone());
        assert_eq!(strict.evaluate(&request).decision, Decision::Deny);

        // auto-edit: asked, not refused.
        let auto = auto_edit_engine();
        auto.push_scope("reviewer", scope.clone());
        let outcome = auto.evaluate(&request);
        assert_eq!(outcome.decision, Decision::Ask, "{}", outcome.reason);
        assert!(outcome.reason.contains("auto-edit"), "{}", outcome.reason);

        // full access: permitted, and recorded.
        let full = full_access_engine();
        full.push_scope("reviewer", scope);
        let outcome = full.evaluate(&request);
        assert_eq!(outcome.decision, Decision::Allow, "{}", outcome.reason);
        assert!(outcome.reason.contains("full access"), "{}", outcome.reason);
    }

    /// Full access asks about nothing, including the categories configuration
    /// alone can never resolve better than `ask`.
    #[test]
    fn full_access_does_not_ask() {
        let full = full_access_engine();
        for risk in [
            RiskLevel::Read,
            RiskLevel::Network,
            RiskLevel::Write,
            RiskLevel::Destructive,
            RiskLevel::ExternalSideEffect,
        ] {
            let outcome = full.evaluate(&action("fs.write_file", risk));
            assert_eq!(
                outcome.decision,
                Decision::Allow,
                "{risk:?} still interrupts: {}",
                outcome.reason
            );
        }
    }

    /// The safety class is not an ordinary preference, so the permissive modes
    /// do not silently absorb it. Default and auto-edit still refuse outright.
    #[test]
    fn only_full_access_puts_the_safety_class_on_the_table() {
        for engine in [engine(), auto_edit_engine()] {
            for request in safety_class() {
                assert_eq!(
                    engine.evaluate(&request).decision,
                    Decision::Deny,
                    "{} escaped the safety class",
                    request.tool
                );
            }
        }
    }

    /// Under full access it is asked once and the answer stands for the
    /// session. Asking per command would make the mode's promise empty; never
    /// asking would remove the last boundary. The grant lives in this process
    /// only, so leaving the session asks again.
    #[test]
    fn full_access_asks_once_for_the_safety_class_then_holds_it_for_the_session() {
        let full = full_access_engine();
        for request in safety_class() {
            let outcome = full.evaluate(&request);
            assert_eq!(
                outcome.decision,
                Decision::Ask,
                "{} did not ask: {}",
                request.tool,
                outcome.reason
            );
            assert_eq!(outcome.layer, PolicyEngine::FULL_ACCESS_SAFETY_LAYER);
            assert!(outcome.reason.contains("rest of this session"));
        }

        full.grant_session(PolicyEngine::FULL_ACCESS_SAFETY_GRANT);
        for request in safety_class() {
            let outcome = full.evaluate(&request);
            assert_eq!(
                outcome.decision,
                Decision::Allow,
                "{} asked again after the session was approved",
                request.tool
            );
        }

        // A fresh process is a fresh session: grants are in-memory only.
        let restarted = full_access_engine();
        assert_eq!(
            restarted.evaluate(&safety_class()[0]).decision,
            Decision::Ask,
            "the grant survived the session it belonged to"
        );
    }

    /// The actions the safety class covers.
    fn safety_class() -> Vec<ActionRequest> {
        let privileged = action("terminal.run", RiskLevel::Privileged);
        let mut sudo = action("terminal.run", RiskLevel::Write);
        sudo.command = Some("sudo rm -rf /".into());
        let mut locked = action("fs.read_file", RiskLevel::Read);
        locked.paths = vec![".env".into()];
        vec![privileged, sudo, locked]
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
    fn plan_mode_refuses_every_way_of_changing_something() {
        let e = engine();
        e.push_scope(PLAN_MODE_SCOPE, PolicyScope::plan_mode());

        for (tool, risk) in [
            ("fs.create_file", RiskLevel::Write),
            ("fs.patch_file", RiskLevel::Write),
            ("fs.delete", RiskLevel::Destructive),
            ("fs.copy", RiskLevel::Write),
            ("terminal.run", RiskLevel::Write),
            ("terminal.run_program", RiskLevel::Write),
            ("web.fetch", RiskLevel::Read),
            ("web.download", RiskLevel::Write),
            ("repo.check", RiskLevel::Write),
            ("mcp.anything", RiskLevel::Read),
        ] {
            let outcome = e.evaluate(&action(tool, risk));
            assert_eq!(
                outcome.decision,
                Decision::Deny,
                "`{tool}` must be refused while planning",
            );
            assert!(
                outcome.reason.contains(PLAN_MODE_SCOPE),
                "the denial must name the scope so the operator knows why: {}",
                outcome.reason,
            );
        }

        // Reading the workspace and submitting the plan are the whole point.
        for tool in [
            "fs.read_file",
            "fs.list_dir",
            "fs.find_files",
            "fs.search_text",
            "repo.git_status",
            "repo.git_diff",
            "diag.system",
            "plan.submit",
        ] {
            assert_ne!(
                e.evaluate(&action(tool, RiskLevel::Read)).decision,
                Decision::Deny,
                "`{tool}` must survive plan mode",
            );
        }

        // A tool nobody has classified yet is refused rather than admitted.
        assert_eq!(
            e.evaluate(&action("some.future_tool", RiskLevel::Read))
                .decision,
            Decision::Deny,
            "the allowlist must fail closed for tools added later",
        );

        e.pop_scope(PLAN_MODE_SCOPE);
        assert_ne!(
            e.evaluate(&action("fs.create_file", RiskLevel::Write))
                .decision,
            Decision::Deny,
            "leaving plan mode restores the operator's normal permissions",
        );
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
