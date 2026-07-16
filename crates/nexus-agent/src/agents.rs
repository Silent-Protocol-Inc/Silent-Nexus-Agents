//! Agent roles and their contracts.
//!
//! Each role has its own prompt, permitted tool categories, task class hint,
//! and budget. Subagents do NOT inherit all parent permissions — a researcher
//! spawned by the orchestrator gets web/read tools only, never the terminal.

use nexus_core::RiskLevel;
use nexus_models::types::TaskClass;
use nexus_tools::ToolCategory;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Orchestrator,
    Planner,
    Implementer,
    Researcher,
    Debugger,
    Reviewer,
    Verifier,
    SecurityReviewer,
    Documentation,
    Architect,
    ProductManager,
    TestEngineer,
    PerformanceEngineer,
    Devops,
    DatabaseEngineer,
    MigrationSpecialist,
    AccessibilityReviewer,
    DependencyAuditor,
    IncidentResponder,
    ReleaseManager,
    UiUxReviewer,
}

impl AgentRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentRole::Orchestrator => "orchestrator",
            AgentRole::Planner => "planner",
            AgentRole::Implementer => "implementer",
            AgentRole::Researcher => "researcher",
            AgentRole::Debugger => "debugger",
            AgentRole::Reviewer => "reviewer",
            AgentRole::Verifier => "verifier",
            AgentRole::SecurityReviewer => "security_reviewer",
            AgentRole::Documentation => "documentation",
            AgentRole::Architect => "architect",
            AgentRole::ProductManager => "product_manager",
            AgentRole::TestEngineer => "test_engineer",
            AgentRole::PerformanceEngineer => "performance_engineer",
            AgentRole::Devops => "devops",
            AgentRole::DatabaseEngineer => "database_engineer",
            AgentRole::MigrationSpecialist => "migration_specialist",
            AgentRole::AccessibilityReviewer => "accessibility_reviewer",
            AgentRole::DependencyAuditor => "dependency_auditor",
            AgentRole::IncidentResponder => "incident_responder",
            AgentRole::ReleaseManager => "release_manager",
            AgentRole::UiUxReviewer => "ui_ux_reviewer",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "orchestrator" => AgentRole::Orchestrator,
            "planner" => AgentRole::Planner,
            "implementer" => AgentRole::Implementer,
            "researcher" => AgentRole::Researcher,
            "debugger" => AgentRole::Debugger,
            "reviewer" => AgentRole::Reviewer,
            "verifier" => AgentRole::Verifier,
            "security_reviewer" => AgentRole::SecurityReviewer,
            "documentation" => AgentRole::Documentation,
            "architect" => AgentRole::Architect,
            "product_manager" => AgentRole::ProductManager,
            "test_engineer" => AgentRole::TestEngineer,
            "performance_engineer" => AgentRole::PerformanceEngineer,
            "devops" => AgentRole::Devops,
            "database_engineer" => AgentRole::DatabaseEngineer,
            "migration_specialist" => AgentRole::MigrationSpecialist,
            "accessibility_reviewer" => AgentRole::AccessibilityReviewer,
            "dependency_auditor" => AgentRole::DependencyAuditor,
            "incident_responder" => AgentRole::IncidentResponder,
            "release_manager" => AgentRole::ReleaseManager,
            "ui_ux_reviewer" => AgentRole::UiUxReviewer,
            _ => return None,
        })
    }

    pub fn all() -> &'static [AgentRole] {
        use AgentRole::*;
        &[
            Orchestrator,
            Planner,
            Implementer,
            Researcher,
            Debugger,
            Reviewer,
            Verifier,
            SecurityReviewer,
            Documentation,
            Architect,
            ProductManager,
            TestEngineer,
            PerformanceEngineer,
            Devops,
            DatabaseEngineer,
            MigrationSpecialist,
            AccessibilityReviewer,
            DependencyAuditor,
            IncidentResponder,
            ReleaseManager,
            UiUxReviewer,
        ]
    }

    pub fn can_delegate(&self) -> bool {
        matches!(self, AgentRole::Orchestrator)
    }

    /// Highest risk this role may propose. Policy and explicit approvals still
    /// decide whether a concrete action runs; this cap prevents read-only
    /// roles from reaching those gates at all.
    pub fn max_risk(&self) -> RiskLevel {
        match self {
            AgentRole::Orchestrator | AgentRole::Devops | AgentRole::ReleaseManager => {
                RiskLevel::ExternalSideEffect
            }
            role if role.can_write() => RiskLevel::Destructive,
            role if role.tool_categories().contains(&ToolCategory::Web) => RiskLevel::Network,
            _ => RiskLevel::Read,
        }
    }

    /// Tool categories this role may use. Read-only roles get no terminal or
    /// write tools.
    pub fn tool_categories(&self) -> Vec<ToolCategory> {
        use ToolCategory::*;
        match self {
            AgentRole::Orchestrator => {
                vec![Filesystem, Repo, Terminal, Web, Diagnostics, Memory, Goal]
            }
            AgentRole::Planner => vec![Filesystem, Repo, Diagnostics],
            AgentRole::Implementer => vec![Filesystem, Repo, Terminal, Diagnostics],
            AgentRole::Researcher => vec![Web, Filesystem],
            AgentRole::Debugger => vec![Filesystem, Repo, Terminal, Diagnostics],
            AgentRole::Reviewer => vec![Filesystem, Repo, Diagnostics],
            AgentRole::Verifier => vec![Repo, Terminal, Filesystem],
            AgentRole::SecurityReviewer => vec![Filesystem, Repo, Diagnostics],
            AgentRole::Documentation => vec![Filesystem, Repo],
            AgentRole::Architect | AgentRole::ProductManager => {
                vec![Filesystem, Repo, Diagnostics]
            }
            AgentRole::TestEngineer | AgentRole::PerformanceEngineer => {
                vec![Filesystem, Repo, Terminal, Diagnostics]
            }
            AgentRole::Devops
            | AgentRole::DatabaseEngineer
            | AgentRole::MigrationSpecialist
            | AgentRole::IncidentResponder
            | AgentRole::ReleaseManager => {
                vec![Filesystem, Repo, Terminal, Diagnostics, Web]
            }
            AgentRole::AccessibilityReviewer
            | AgentRole::DependencyAuditor
            | AgentRole::UiUxReviewer => vec![Filesystem, Repo, Diagnostics, Web],
        }
    }

    /// Whether this role may mutate the workspace at all.
    pub fn can_write(&self) -> bool {
        matches!(
            self,
            AgentRole::Orchestrator
                | AgentRole::Implementer
                | AgentRole::Debugger
                | AgentRole::TestEngineer
                | AgentRole::PerformanceEngineer
                | AgentRole::Devops
                | AgentRole::DatabaseEngineer
                | AgentRole::MigrationSpecialist
                | AgentRole::IncidentResponder
                | AgentRole::ReleaseManager
        )
    }

    pub fn task_class(&self) -> TaskClass {
        match self {
            AgentRole::Planner => TaskClass::Planning,
            AgentRole::Researcher => TaskClass::Research,
            AgentRole::Verifier | AgentRole::Reviewer | AgentRole::SecurityReviewer => {
                TaskClass::Verification
            }
            AgentRole::Architect
            | AgentRole::ProductManager
            | AgentRole::MigrationSpecialist
            | AgentRole::ReleaseManager => TaskClass::Planning,
            AgentRole::AccessibilityReviewer
            | AgentRole::DependencyAuditor
            | AgentRole::UiUxReviewer => TaskClass::Verification,
            AgentRole::TestEngineer | AgentRole::PerformanceEngineer => TaskClass::Verification,
            _ => TaskClass::Coding,
        }
    }

    /// The output contract the role must satisfy (surfaced in its prompt).
    pub fn output_contract(&self) -> &'static str {
        match self {
            AgentRole::Orchestrator => {
                "Coordinate work and produce a final answer with a summary of changes."
            }
            AgentRole::Planner => "Produce a numbered, minimal, verifiable plan. Do not implement.",
            AgentRole::Implementer => {
                "Make the smallest change that satisfies the task; report files changed."
            }
            AgentRole::Researcher => {
                "Answer with cited sources (title + URL). Do not modify the workspace."
            }
            AgentRole::Debugger => "Identify root cause with evidence, then apply a minimal fix.",
            AgentRole::Reviewer => "List findings by severity; suggest fixes. Read-only.",
            AgentRole::Verifier => {
                "Independently verify each acceptance criterion and report pass/fail with evidence."
            }
            AgentRole::SecurityReviewer => {
                "Report security-relevant findings (injection, secrets, unsafe ops). Read-only."
            }
            AgentRole::Documentation => {
                "Write clear, accurate docs for the change. No overclaiming."
            }
            AgentRole::Architect => {
                "Define boundaries, interfaces, risks, and migration shape. Do not implement."
            }
            AgentRole::ProductManager => {
                "Clarify user outcome, scope, acceptance criteria, and sequencing. Do not implement."
            }
            AgentRole::TestEngineer => {
                "Design and implement focused tests, then report exact pass/fail evidence."
            }
            AgentRole::PerformanceEngineer => {
                "Measure before changing, optimize the verified bottleneck, and report benchmarks."
            }
            AgentRole::Devops => {
                "Make reversible infrastructure or automation changes with validation and rollback notes."
            }
            AgentRole::DatabaseEngineer => {
                "Preserve data integrity, compatibility, and rollback safety for database changes."
            }
            AgentRole::MigrationSpecialist => {
                "Produce and execute an explicit staged migration with compatibility checks."
            }
            AgentRole::AccessibilityReviewer => {
                "Audit accessibility with concrete findings and evidence. Read-only."
            }
            AgentRole::DependencyAuditor => {
                "Audit dependency provenance, compatibility, and security risk. Read-only."
            }
            AgentRole::IncidentResponder => {
                "Stabilize the incident, preserve evidence, make the smallest safe fix, and verify recovery."
            }
            AgentRole::ReleaseManager => {
                "Coordinate release readiness, evidence, rollback, and explicit publication approval."
            }
            AgentRole::UiUxReviewer => {
                "Review usability, hierarchy, responsiveness, and accessibility with actionable evidence. Read-only."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_roles_lack_terminal() {
        for role in [
            AgentRole::Researcher,
            AgentRole::Reviewer,
            AgentRole::SecurityReviewer,
        ] {
            assert!(!role.tool_categories().contains(&ToolCategory::Terminal));
            assert!(!role.can_write());
        }
    }

    #[test]
    fn implementer_can_write_and_use_terminal() {
        assert!(AgentRole::Implementer.can_write());
        assert!(AgentRole::Implementer
            .tool_categories()
            .contains(&ToolCategory::Terminal));
        assert_eq!(AgentRole::Implementer.max_risk(), RiskLevel::Destructive);
        assert_eq!(
            AgentRole::Orchestrator.max_risk(),
            RiskLevel::ExternalSideEffect
        );
    }

    #[test]
    fn role_roundtrip() {
        for role in AgentRole::all() {
            assert_eq!(AgentRole::parse(role.as_str()), Some(*role));
        }
    }
}
