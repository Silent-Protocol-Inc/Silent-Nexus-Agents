//! Durable orchestration types shared by the agent loop, storage, CLI, and TUI.
//!
//! These records deliberately contain summaries and redacted evidence only.
//! Hidden chain-of-thought, raw credentials, and runtime secret values are
//! never represented by this module.

use crate::ids::{AgentId, ManifestId, PlanId, SessionId, TaskId, TraceId, TurnId};
use crate::store::Store;
use crate::{NexusError, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkBreakdownKind {
    Direct,
    Tracked,
    Planned,
}

impl WorkBreakdownKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Tracked => "tracked",
            Self::Planned => "planned",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Blocked,
    Skipped,
}

impl StageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
            Self::Skipped => "skipped",
        }
    }

    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Blocked | Self::Skipped
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkBudget {
    pub max_actions: Option<u32>,
    pub max_tokens: Option<u64>,
    pub max_runtime_ms: Option<u64>,
    pub actions_used: u32,
    pub tokens_used: u64,
    pub runtime_used_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationEvidence {
    pub label: String,
    pub status: StageStatus,
    pub command: Option<String>,
    pub summary: String,
    pub artifact_id: Option<String>,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage {
    pub id: String,
    pub sequence: u32,
    pub title: String,
    pub description: String,
    pub status: StageStatus,
    pub owner: String,
    pub budget: WorkBudget,
    pub evidence: Vec<String>,
    pub changed_files: Vec<String>,
    pub validation: Vec<ValidationEvidence>,
    pub next_action: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

impl Stage {
    pub fn new(sequence: u32, title: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: format!("stage_{sequence}"),
            sequence,
            title: title.into(),
            description: description.into(),
            status: StageStatus::Pending,
            owner: "main".into(),
            budget: WorkBudget::default(),
            evidence: Vec::new(),
            changed_files: Vec::new(),
            validation: Vec::new(),
            next_action: None,
            started_at: None,
            finished_at: None,
        }
    }

    pub fn start(&mut self) {
        self.status = StageStatus::Running;
        self.started_at.get_or_insert_with(crate::now_rfc3339);
    }

    pub fn finish(&mut self, status: StageStatus) {
        self.status = status;
        if status.terminal() {
            self.finished_at = Some(crate::now_rfc3339());
        }
    }
}

/// Harness-side estimate used to select Direct, Tracked, or Planned work.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkEstimate {
    pub predicted_actions: u32,
    pub writes: bool,
    pub predictable: bool,
    pub multi_file: bool,
    pub cross_subsystem: bool,
    pub migration: bool,
    pub background_work: bool,
    pub subagents: bool,
    pub destructive: bool,
    pub external: bool,
    pub needs_grounding: bool,
    pub rationale: Vec<String>,
}

impl WorkEstimate {
    /// Conservative deterministic estimate used before the model is called.
    /// It is intentionally promotable: later tool selection can increase the
    /// classification, but never silently decrease it.
    pub fn from_objective(objective: &str) -> Self {
        let lower = objective.to_ascii_lowercase();
        let writes = contains_any(
            &lower,
            &[
                "implement",
                "fix",
                "change",
                "edit",
                "update",
                "add",
                "remove",
                "create",
                "upgrade",
                "refactor",
                "redesign",
                "migrate",
                "install",
            ],
        );
        let multi_file = contains_any(
            &lower,
            &[
                "multi-file",
                "multiple files",
                "across the repo",
                "across the workspace",
                "all crates",
                "all packages",
            ],
        );
        let cross_subsystem = contains_any(
            &lower,
            &[
                "cross-subsystem",
                "end-to-end",
                "frontend and backend",
                "storage and ui",
                "provider and tui",
                "architecture",
            ],
        );
        let migration = contains_any(&lower, &["migration", "schema change", "database upgrade"]);
        let background_work =
            contains_any(&lower, &["background task", "worker", "daemon", "queue"]);
        let subagents = contains_any(&lower, &["subagent", "fanout", "fan-out", "delegate"]);
        let destructive = contains_any(
            &lower,
            &["delete", "reset", "drop table", "destroy", "purge"],
        );
        let external = contains_any(
            &lower,
            &["publish", "deploy", "push", "release", "send", "upload"],
        );
        let enumerated = objective
            .lines()
            .filter(|line| {
                let line = line.trim_start();
                line.starts_with("- ")
                    || line.starts_with("* ")
                    || line
                        .split_once('.')
                        .is_some_and(|(n, _)| n.chars().all(|c| c.is_ascii_digit()))
            })
            .count() as u32;
        let conjunctions = lower.matches(" and ").count() as u32;
        let predicted_actions = if enumerated > 0 {
            enumerated
        } else if writes {
            (1 + conjunctions).clamp(1, 6)
        } else {
            1
        };
        let needs_grounding = contains_any(
            &lower,
            &[
                "repo",
                "workspace",
                "existing",
                "current implementation",
                "diagnose",
                "inspect",
            ],
        );
        let predictable = !contains_any(
            &lower,
            &[
                "investigate",
                "unknown",
                "architecture",
                "redesign",
                "diagnose",
            ],
        );
        let mut rationale = Vec::new();
        if multi_file {
            rationale.push("multi-file scope".into());
        }
        if cross_subsystem {
            rationale.push("cross-subsystem scope".into());
        }
        if migration {
            rationale.push("migration".into());
        }
        if background_work {
            rationale.push("background work".into());
        }
        if subagents {
            rationale.push("subagents".into());
        }
        if destructive {
            rationale.push("destructive action".into());
        }
        if external {
            rationale.push("external side effect".into());
        }
        if predicted_actions > 1 {
            rationale.push(format!("{predicted_actions} predicted actions"));
        }
        Self {
            predicted_actions,
            writes,
            predictable,
            multi_file,
            cross_subsystem,
            migration,
            background_work,
            subagents,
            destructive,
            external,
            needs_grounding,
            rationale,
        }
    }

    /// Shrink the decomposition for weak or compatibility-mode models: force
    /// grounding and drop the predictability assumption so writing work runs
    /// as smaller tracked stages instead of one direct leap.
    pub fn constrained_for_weak_model(mut self) -> Self {
        self.predictable = false;
        self.needs_grounding = true;
        self.rationale
            .push("constrained model: smaller validated stages".into());
        self
    }

    pub fn classify(&self) -> WorkBreakdownKind {
        if self.multi_file
            || self.cross_subsystem
            || self.migration
            || self.background_work
            || self.subagents
            || self.destructive
            || self.external
            || self.predicted_actions > 5
        {
            WorkBreakdownKind::Planned
        } else if (2..=5).contains(&self.predicted_actions) || (self.writes && !self.predictable) {
            WorkBreakdownKind::Tracked
        } else {
            WorkBreakdownKind::Direct
        }
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkBreakdown {
    pub id: PlanId,
    pub version: u32,
    pub objective: String,
    pub kind: WorkBreakdownKind,
    pub approved: bool,
    pub paused: bool,
    pub rationale: Vec<String>,
    pub stages: Vec<Stage>,
    pub current_stage: Option<String>,
    pub next_stage: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl WorkBreakdown {
    pub fn generate(objective: impl Into<String>, estimate: WorkEstimate) -> Self {
        let objective = objective.into();
        let kind = estimate.classify();
        let mut stages = match kind {
            WorkBreakdownKind::Direct => vec![Stage::new(
                1,
                "Active turn",
                "Complete the bounded request and report the result.",
            )],
            WorkBreakdownKind::Tracked => {
                let mut stages = Vec::new();
                if estimate.needs_grounding {
                    stages.push(Stage::new(
                        1,
                        "Grounding",
                        "Inspect the relevant repository state and constraints.",
                    ));
                }
                let seq = stages.len() as u32 + 1;
                stages.push(Stage::new(
                    seq,
                    if estimate.writes {
                        "Implementation"
                    } else {
                        "Analysis"
                    },
                    if estimate.writes {
                        "Apply the contained change under normal approvals."
                    } else {
                        "Produce the requested evidence-backed result."
                    },
                ));
                stages.push(Stage::new(
                    seq + 1,
                    "Validation",
                    "Verify the result and attach concise evidence.",
                ));
                stages
            }
            WorkBreakdownKind::Planned => {
                let mut grounding = Stage::new(
                    1,
                    "Grounding",
                    "Read-only inspection permitted before plan approval.",
                );
                if estimate.needs_grounding {
                    grounding.start();
                }
                let mut approval = Stage::new(
                    2,
                    "Plan approval",
                    "Review the versioned plan before the first write or external action.",
                );
                if !estimate.needs_grounding {
                    approval.start();
                }
                let mut implementation = Stage::new(
                    3,
                    "Implementation",
                    "Execute approved stages with one writer at a time.",
                );
                implementation.status = StageStatus::Blocked;
                implementation.next_action = Some("approve the plan".into());
                vec![
                    grounding,
                    approval,
                    implementation,
                    Stage::new(4, "Validation", "Run required checks and collect evidence."),
                ]
            }
        };
        if kind != WorkBreakdownKind::Planned {
            if let Some(first) = stages.first_mut() {
                first.start();
            }
        }
        let current_stage = stages
            .iter()
            .find(|stage| stage.status == StageStatus::Running)
            .map(|stage| stage.id.clone());
        let next_stage = next_stage_after(&stages, current_stage.as_deref());
        let now = crate::now_rfc3339();
        Self {
            id: PlanId::generate(),
            version: 1,
            objective,
            kind,
            approved: kind != WorkBreakdownKind::Planned,
            paused: false,
            rationale: estimate.rationale,
            stages,
            current_stage,
            next_stage,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    pub fn progress(&self) -> (usize, usize) {
        (
            self.stages
                .iter()
                .filter(|stage| {
                    matches!(stage.status, StageStatus::Completed | StageStatus::Skipped)
                })
                .count(),
            self.stages.len(),
        )
    }

    pub fn transition_to(&mut self, title: &str) -> Vec<Stage> {
        let target_id = self
            .stages
            .iter()
            .find(|stage| stage.title == title)
            .map(|stage| stage.id.clone());
        let Some(target_id) = target_id else {
            return Vec::new();
        };
        if self.current_stage.as_deref() == Some(target_id.as_str()) {
            return Vec::new();
        }
        let mut changed = Vec::new();
        if let Some(current_id) = self.current_stage.clone() {
            if let Some(current) = self
                .stages
                .iter_mut()
                .find(|stage| stage.id == current_id && stage.status == StageStatus::Running)
            {
                current.finish(StageStatus::Completed);
                changed.push(current.clone());
            }
        }
        if let Some(target) = self.stages.iter_mut().find(|stage| stage.id == target_id) {
            target.start();
            target.next_action = None;
            changed.push(target.clone());
        }
        self.current_stage = Some(target_id);
        self.next_stage = next_stage_after(&self.stages, self.current_stage.as_deref());
        self.updated_at = crate::now_rfc3339();
        changed
    }

    pub fn record_current_evidence(
        &mut self,
        evidence: impl Into<String>,
        changed_paths: &[String],
        validation: Option<ValidationEvidence>,
    ) -> Option<Stage> {
        let current_id = self.current_stage.clone()?;
        let stage = self
            .stages
            .iter_mut()
            .find(|stage| stage.id == current_id)?;
        let evidence = evidence.into();
        if !evidence.is_empty() && !stage.evidence.contains(&evidence) {
            stage.evidence.push(evidence);
        }
        for path in changed_paths {
            if !stage.changed_files.contains(path) {
                stage.changed_files.push(path.clone());
            }
        }
        if let Some(validation) = validation {
            stage.validation.push(validation);
        }
        self.updated_at = crate::now_rfc3339();
        Some(stage.clone())
    }

    pub fn finish_current(&mut self, status: StageStatus) -> Option<Stage> {
        let current_id = self.current_stage.clone()?;
        let stage = self
            .stages
            .iter_mut()
            .find(|stage| stage.id == current_id)?;
        stage.finish(status);
        let changed = stage.clone();
        self.current_stage = None;
        self.next_stage = next_stage_after(&self.stages, None);
        self.updated_at = crate::now_rfc3339();
        Some(changed)
    }

    pub fn approve(&mut self) {
        self.approved = true;
        if let Some(stage) = self
            .stages
            .iter_mut()
            .find(|stage| stage.title == "Grounding" && stage.status == StageStatus::Running)
        {
            stage.finish(StageStatus::Completed);
        }
        if let Some(stage) = self
            .stages
            .iter_mut()
            .find(|stage| stage.title == "Plan approval")
        {
            stage.finish(StageStatus::Completed);
        }
        if let Some(stage) = self
            .stages
            .iter_mut()
            .find(|stage| stage.title == "Implementation")
        {
            stage.start();
            stage.next_action = None;
            self.current_stage = Some(stage.id.clone());
        }
        self.next_stage = next_stage_after(&self.stages, self.current_stage.as_deref());
        self.updated_at = crate::now_rfc3339();
    }

    /// Promote visible work when a turn grows beyond its initial estimate.
    /// Scope is never silently demoted.
    pub fn promote(&mut self, estimate: WorkEstimate) -> Option<PlanPromotion> {
        let target = estimate.classify();
        if target <= self.kind {
            return None;
        }
        let from = self.kind;
        let prior_evidence: Vec<String> = self
            .stages
            .iter()
            .flat_map(|stage| stage.evidence.clone())
            .collect();
        let prior_changed_files: Vec<String> = self
            .stages
            .iter()
            .flat_map(|stage| stage.changed_files.clone())
            .collect();
        let prior_validation: Vec<ValidationEvidence> = self
            .stages
            .iter()
            .flat_map(|stage| stage.validation.clone())
            .collect();
        let mut replacement = Self::generate(self.objective.clone(), estimate);
        let evidence_target = if !prior_changed_files.is_empty() {
            replacement
                .stages
                .iter()
                .position(|stage| stage.title == "Implementation")
        } else {
            replacement
                .stages
                .iter()
                .position(|stage| stage.title == "Grounding")
                .or_else(|| {
                    replacement.stages.iter().position(|stage| {
                        matches!(stage.title.as_str(), "Analysis" | "Implementation")
                    })
                })
        };
        if let Some(index) = evidence_target {
            let stage = &mut replacement.stages[index];
            for evidence in prior_evidence {
                if !stage.evidence.contains(&evidence) {
                    stage.evidence.push(evidence);
                }
            }
            for path in prior_changed_files {
                if !stage.changed_files.contains(&path) {
                    stage.changed_files.push(path);
                }
            }
        }
        if let Some(stage) = replacement
            .stages
            .iter_mut()
            .find(|stage| stage.title == "Validation")
        {
            stage.validation.extend(prior_validation);
        }
        if target == WorkBreakdownKind::Planned
            && replacement
                .stages
                .iter()
                .any(|stage| !stage.evidence.is_empty() || !stage.changed_files.is_empty())
        {
            if let Some(stage) = replacement
                .stages
                .iter_mut()
                .find(|stage| stage.title == "Grounding")
            {
                stage.finish(StageStatus::Completed);
            }
            if let Some(stage) = replacement
                .stages
                .iter_mut()
                .find(|stage| stage.title == "Plan approval")
            {
                stage.start();
                replacement.current_stage = Some(stage.id.clone());
            }
            replacement.next_stage =
                next_stage_after(&replacement.stages, replacement.current_stage.as_deref());
        }
        self.kind = replacement.kind;
        self.approved = replacement.approved;
        self.rationale = replacement.rationale.clone();
        self.stages = replacement.stages;
        self.current_stage = replacement.current_stage;
        self.next_stage = replacement.next_stage;
        self.version += 1;
        self.updated_at = crate::now_rfc3339();
        Some(PlanPromotion {
            from,
            to: target,
            version: self.version,
            reason: self.rationale.join(", "),
        })
    }
}

fn next_stage_after(stages: &[Stage], current: Option<&str>) -> Option<String> {
    let current_seq = current.and_then(|id| {
        stages
            .iter()
            .find(|stage| stage.id == id)
            .map(|stage| stage.sequence)
    });
    stages
        .iter()
        .filter(|stage| {
            matches!(stage.status, StageStatus::Pending | StageStatus::Blocked)
                && current_seq.is_none_or(|sequence| stage.sequence > sequence)
        })
        .min_by_key(|stage| stage.sequence)
        .map(|stage| stage.id.clone())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanPromotion {
    pub from: WorkBreakdownKind,
    pub to: WorkBreakdownKind,
    pub version: u32,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextCategory {
    ImmutableSafety,
    ProviderPolicy,
    SandboxPolicy,
    ProjectInstructions,
    Agent,
    Persona,
    Profile,
    Memory,
    ApprovedPlan,
    ActiveTasks,
    SessionSummary,
    RecentTranscript,
    ToolResults,
    Artifacts,
}

impl ContextCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::ImmutableSafety => "immutable safety",
            Self::ProviderPolicy => "provider policy",
            Self::SandboxPolicy => "sandbox/policy",
            Self::ProjectInstructions => "project instructions",
            Self::Agent => "agent",
            Self::Persona => "persona",
            Self::Profile => "approved profile",
            Self::Memory => "retrieved memories",
            Self::ApprovedPlan => "approved plan",
            Self::ActiveTasks => "active tasks",
            Self::SessionSummary => "session summary",
            Self::RecentTranscript => "recent transcript",
            Self::ToolResults => "tool results",
            Self::Artifacts => "artifacts",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSource {
    pub category: ContextCategory,
    pub label: String,
    pub token_count: usize,
    pub estimated: bool,
    pub included: bool,
    pub reason: Option<String>,
    /// Digest of the redacted source. The source body is intentionally absent.
    pub digest: String,
}

impl ContextSource {
    pub fn included(
        category: ContextCategory,
        label: impl Into<String>,
        token_count: usize,
        estimated: bool,
        redacted_source: &str,
    ) -> Self {
        Self {
            category,
            label: label.into(),
            token_count,
            estimated,
            included: true,
            reason: None,
            digest: hex::encode(Sha256::digest(redacted_source.as_bytes())),
        }
    }

    pub fn omitted(
        category: ContextCategory,
        label: impl Into<String>,
        token_count: usize,
        reason: impl Into<String>,
        redacted_source: &str,
    ) -> Self {
        Self {
            category,
            label: label.into(),
            token_count,
            estimated: true,
            included: false,
            reason: Some(reason.into()),
            digest: hex::encode(Sha256::digest(redacted_source.as_bytes())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextOmission {
    pub category: ContextCategory,
    pub label: String,
    pub token_count: usize,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextManifest {
    pub id: ManifestId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub trace_id: TraceId,
    pub provider: String,
    pub model: String,
    pub context_window: usize,
    pub reserved_output_tokens: usize,
    pub total_tokens: usize,
    pub provider_input_tokens: Option<usize>,
    pub estimated: bool,
    pub sources: Vec<ContextSource>,
    pub omissions: Vec<ContextOmission>,
    pub created_at: String,
}

impl ContextManifest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_id: SessionId,
        turn_id: TurnId,
        trace_id: TraceId,
        provider: impl Into<String>,
        model: impl Into<String>,
        context_window: usize,
        reserved_output_tokens: usize,
        sources: Vec<ContextSource>,
        omissions: Vec<ContextOmission>,
    ) -> Self {
        let total_tokens = sources
            .iter()
            .filter(|source| source.included)
            .map(|source| source.token_count)
            .sum();
        let estimated = sources.iter().any(|source| source.estimated);
        Self {
            id: ManifestId::generate(),
            session_id,
            turn_id,
            trace_id,
            provider: provider.into(),
            model: model.into(),
            context_window,
            reserved_output_tokens,
            total_tokens,
            provider_input_tokens: None,
            estimated,
            sources,
            omissions,
            created_at: crate::now_rfc3339(),
        }
    }

    pub fn observe_provider_input(&mut self, tokens: usize) {
        self.provider_input_tokens = Some(tokens);
        self.total_tokens = tokens;
        self.estimated = false;
    }

    pub fn tokens_by_category(&self) -> BTreeMap<ContextCategory, usize> {
        let mut out = BTreeMap::new();
        for source in self.sources.iter().filter(|source| source.included) {
            *out.entry(source.category).or_default() += source.token_count;
        }
        out
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffStatistics {
    pub files: usize,
    pub insertions: usize,
    pub deletions: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsageSnapshot {
    pub context_window: usize,
    pub input_tokens: usize,
    pub reserved_output_tokens: usize,
    pub estimated: bool,
    pub compaction_count: u32,
    pub cumulative_input_tokens: u64,
    pub cumulative_output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub id: TaskId,
    pub title: String,
    pub status: String,
    pub owner: String,
    pub writer: bool,
    pub duration_ms: u64,
    pub waiting_approval: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunSnapshot {
    pub id: AgentId,
    pub parent_id: Option<AgentId>,
    pub role: String,
    pub status: String,
    pub model: String,
    pub current_stage: Option<String>,
    pub duration_ms: u64,
    pub unread_events: u64,
    pub waiting_approval: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveWorkSnapshot {
    pub session_id: Option<SessionId>,
    pub session_title: String,
    pub workspace: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub model: String,
    pub provider: String,
    pub effort: Option<String>,
    pub agent: String,
    pub permission_mode: String,
    pub objective: Option<String>,
    pub turn_state: String,
    pub work: Option<WorkBreakdown>,
    pub active_foreground_tool: Option<String>,
    pub background_tasks: Vec<TaskSnapshot>,
    pub subagents: Vec<AgentRunSnapshot>,
    pub modified_files: Vec<String>,
    pub staged_files: Vec<String>,
    pub untracked_files: Vec<String>,
    pub diff: DiffStatistics,
    pub validation_completed: Vec<ValidationEvidence>,
    pub validation_pending: Vec<String>,
    pub validation_failed: Vec<ValidationEvidence>,
    pub waiting_approvals: Vec<String>,
    pub blockers: Vec<String>,
    pub provider_reset_at: Option<String>,
    pub retry_state: Option<String>,
    pub context: ContextUsageSnapshot,
    pub updated_at: String,
}

impl ActiveWorkSnapshot {
    pub fn empty(workspace: impl Into<String>) -> Self {
        Self {
            session_id: None,
            session_title: String::new(),
            workspace: workspace.into(),
            branch: None,
            head: None,
            model: String::new(),
            provider: String::new(),
            effort: None,
            agent: String::new(),
            permission_mode: String::new(),
            objective: None,
            turn_state: "idle".into(),
            work: None,
            active_foreground_tool: None,
            background_tasks: Vec::new(),
            subagents: Vec::new(),
            modified_files: Vec::new(),
            staged_files: Vec::new(),
            untracked_files: Vec::new(),
            diff: DiffStatistics::default(),
            validation_completed: Vec::new(),
            validation_pending: Vec::new(),
            validation_failed: Vec::new(),
            waiting_approvals: Vec::new(),
            blockers: Vec::new(),
            provider_reset_at: None,
            retry_state: None,
            context: ContextUsageSnapshot::default(),
            updated_at: crate::now_rfc3339(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptionKind {
    Quota,
    Plan,
    Rate,
    Context,
    Authentication,
    InvalidRequest,
    Transport,
    Cancellation,
    Crash,
}

impl InterruptionKind {
    pub fn as_str(self) -> &'static str {
        interruption_kind_label(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptionClassification {
    pub kind: InterruptionKind,
    pub retryable: bool,
    pub reset_at: Option<String>,
}

pub fn classify_interruption(error: &NexusError) -> InterruptionClassification {
    let raw = error.to_string();
    let text = raw.to_ascii_lowercase();
    let kind = if text.contains("cancelled") || text.contains("canceled") {
        InterruptionKind::Cancellation
    } else if text.contains("context length")
        || text.contains("context window")
        || text.contains("too many tokens")
        || text.contains("maximum context")
    {
        InterruptionKind::Context
    } else if text.contains("insufficient_quota")
        || text.contains("quota exceeded")
        || text.contains("usage quota")
    {
        InterruptionKind::Quota
    } else if text.contains("plan limit")
        || text.contains("subscription limit")
        || text.contains("not included in your plan")
    {
        InterruptionKind::Plan
    } else if text.contains("http 429")
        || text.contains("rate limit")
        || text.contains("rate_limit")
        || text.contains("overloaded")
    {
        InterruptionKind::Rate
    } else if text.contains("http 401")
        || text.contains("authentication")
        || text.contains("unauthorized")
        || text.contains("api key")
        || text.contains("login required")
    {
        InterruptionKind::Authentication
    } else if text.contains("http 400")
        || text.contains("http 404")
        || text.contains("http 422")
        || matches!(error, NexusError::InvalidAction(_))
    {
        InterruptionKind::InvalidRequest
    } else if matches!(
        error,
        NexusError::Provider { .. } | NexusError::ModelTimeout(_)
    ) {
        InterruptionKind::Transport
    } else {
        InterruptionKind::Crash
    };
    InterruptionClassification {
        kind,
        retryable: matches!(
            kind,
            InterruptionKind::Quota
                | InterruptionKind::Plan
                | InterruptionKind::Rate
                | InterruptionKind::Context
                | InterruptionKind::Transport
        ) && error.is_provider_retryable(),
        reset_at: extract_reset_at(&raw),
    }
}

fn extract_reset_at(message: &str) -> Option<String> {
    let lower = message.to_ascii_lowercase();
    for marker in ["reset_at=", "reset-at=", "resets_at=", "resets at "] {
        if let Some(index) = lower.find(marker) {
            let tail = &message[index + marker.len()..];
            let value = tail
                .trim_start_matches(['"', '\''])
                .split(|character: char| {
                    character.is_whitespace() || matches!(character, ',' | ';' | '"' | '\'')
                })
                .next()
                .unwrap_or_default()
                .trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInterruption {
    pub id: crate::ids::InterruptionId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub trace_id: TraceId,
    pub kind: InterruptionKind,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub message: String,
    pub reset_at: Option<String>,
    pub retryable: bool,
    pub checkpoint_artifact: Option<String>,
    pub child_session_id: Option<SessionId>,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
    Blocked,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Blocked => "blocked",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "running" => Self::Running,
            "paused" => Self::Paused,
            "completed" | "done" => Self::Completed,
            "failed" | "error" => Self::Failed,
            "cancelled" | "canceled" => Self::Cancelled,
            "blocked" => Self::Blocked,
            _ => Self::Queued,
        }
    }

    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundTask {
    pub id: TaskId,
    pub session_id: SessionId,
    pub plan_id: Option<String>,
    pub stage_id: Option<String>,
    pub title: String,
    pub objective: String,
    pub status: TaskStatus,
    pub owner: String,
    pub writer: bool,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub budget: WorkBudget,
    pub result: Option<ValueEnvelope>,
    pub error: Option<String>,
    pub attempts: u32,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<String>,
    pub heartbeat_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueEnvelope {
    pub summary: String,
    pub artifact_ids: Vec<String>,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRun {
    pub id: AgentId,
    pub session_id: SessionId,
    pub parent_run_id: Option<AgentId>,
    pub task_id: Option<TaskId>,
    pub role: String,
    pub objective: String,
    pub status: TaskStatus,
    pub depth: u8,
    pub model: String,
    pub permission_mode: String,
    pub budget: WorkBudget,
    pub unread_events: u64,
    pub result: Option<ValueEnvelope>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanApprovalRecord {
    pub id: String,
    pub plan_id: PlanId,
    pub version: u32,
    pub approved: Option<bool>,
    pub scope_diff: PlanScopeDiff,
    pub requested_at: String,
    pub resolved_at: Option<String>,
    pub approver: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanScopeDiff {
    pub added_stages: Vec<String>,
    pub removed_stages: Vec<String>,
    pub permission_expanded: bool,
    pub destructive_added: bool,
    pub external_added: bool,
    pub budget_increased: bool,
    pub summary: String,
}

impl PlanScopeDiff {
    pub fn requires_approval(&self) -> bool {
        self.permission_expanded
            || self.destructive_added
            || self.external_added
            || self.budget_increased
            || !self.added_stages.is_empty()
    }
}

/// SQLite-backed plan, task, subagent, and interruption state.
#[derive(Clone)]
pub struct OrchestrationStore {
    store: Store,
}

impl OrchestrationStore {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn save_plan(
        &self,
        session_id: &str,
        work: &WorkBreakdown,
        status: &str,
        created_by: &str,
    ) -> Result<()> {
        let body = serde_json::to_string(work)?;
        let scope_hash = hex::encode(Sha256::digest(body.as_bytes()));
        self.store.with(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result: rusqlite::Result<()> = (|| {
                conn.execute(
                    "INSERT INTO plan_versions
                     (id,session_id,version,title,objective,breakdown_kind,status,
                      scope_hash,body_json,created_by,created_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
                     ON CONFLICT(id,version) DO UPDATE SET
                       status=excluded.status,
                       scope_hash=excluded.scope_hash,
                       body_json=excluded.body_json",
                    params![
                        work.id.as_str(),
                        session_id,
                        work.version as i64,
                        plan_title(&work.objective),
                        work.objective,
                        work.kind.as_str(),
                        status,
                        scope_hash,
                        body,
                        created_by,
                        work.created_at,
                    ],
                )?;
                for stage in &work.stages {
                    conn.execute(
                        "INSERT INTO plan_steps
                         (id,plan_id,plan_version,seq,title,description,status,owner,
                          budget_json,evidence_json,changed_files_json,validation_json,
                          next_action,started_at,finished_at)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
                         ON CONFLICT(id) DO UPDATE SET
                           status=excluded.status,
                           owner=excluded.owner,
                           budget_json=excluded.budget_json,
                           evidence_json=excluded.evidence_json,
                           changed_files_json=excluded.changed_files_json,
                           validation_json=excluded.validation_json,
                           next_action=excluded.next_action,
                           started_at=excluded.started_at,
                           finished_at=excluded.finished_at",
                        params![
                            persisted_stage_id(work, stage),
                            work.id.as_str(),
                            work.version as i64,
                            stage.sequence as i64,
                            stage.title,
                            stage.description,
                            stage.status.as_str(),
                            stage.owner,
                            serde_json::to_string(&stage.budget).map_err(to_sql_json_error)?,
                            serde_json::to_string(&stage.evidence).map_err(to_sql_json_error)?,
                            serde_json::to_string(&stage.changed_files)
                                .map_err(to_sql_json_error)?,
                            serde_json::to_string(&stage.validation).map_err(to_sql_json_error)?,
                            stage.next_action,
                            stage.started_at,
                            stage.finished_at,
                        ],
                    )?;
                }
                Ok(())
            })();
            match result {
                Ok(()) => {
                    conn.execute_batch("COMMIT")?;
                    Ok(())
                }
                Err(error) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    Err(error.into())
                }
            }
        })
    }

    pub fn latest_plan(&self, session_id: &str) -> Result<Option<WorkBreakdown>> {
        self.store.with(|conn| {
            let payload: Option<String> = conn
                .query_row(
                    "SELECT body_json FROM plan_versions
                     WHERE session_id=?1 ORDER BY created_at DESC, version DESC LIMIT 1",
                    [session_id],
                    |row| row.get(0),
                )
                .optional()?;
            payload
                .map(|payload| serde_json::from_str(&payload).map_err(NexusError::from))
                .transpose()
        })
    }

    /// Copy the latest plan into a continuation session with a new plan id
    /// while preserving the approved version, active stage, evidence, and
    /// validation state.
    pub fn clone_latest_plan(
        &self,
        source_session_id: &str,
        target_session_id: &str,
        created_by: &str,
    ) -> Result<Option<WorkBreakdown>> {
        let Some(mut work) = self.latest_plan(source_session_id)? else {
            return Ok(None);
        };
        let source_id = work.id.clone();
        let now = crate::now_rfc3339();
        work.id = PlanId::generate();
        work.created_at = now.clone();
        work.updated_at = now;
        work.rationale.push(format!(
            "continued from plan {} v{} in session {}",
            source_id.as_str(),
            work.version,
            source_session_id
        ));
        let status = if work.paused {
            "paused"
        } else if work.approved {
            "approved"
        } else {
            "awaiting_approval"
        };
        self.save_plan(target_session_id, &work, status, created_by)?;
        Ok(Some(work))
    }

    pub fn plan(&self, plan_id: &str, version: Option<u32>) -> Result<WorkBreakdown> {
        self.store.with(|conn| {
            let payload: Option<String> = match version {
                Some(version) => conn
                    .query_row(
                        "SELECT body_json FROM plan_versions WHERE id=?1 AND version=?2",
                        params![plan_id, version as i64],
                        |row| row.get(0),
                    )
                    .optional()?,
                None => conn
                    .query_row(
                        "SELECT body_json FROM plan_versions
                         WHERE id=?1 ORDER BY version DESC LIMIT 1",
                        [plan_id],
                        |row| row.get(0),
                    )
                    .optional()?,
            };
            let payload =
                payload.ok_or_else(|| NexusError::NotFound(format!("plan `{plan_id}`")))?;
            Ok(serde_json::from_str(&payload)?)
        })
    }

    pub fn plan_history(&self, plan_id: &str) -> Result<Vec<WorkBreakdown>> {
        self.store.with(|conn| {
            let mut stmt =
                conn.prepare("SELECT body_json FROM plan_versions WHERE id=?1 ORDER BY version")?;
            let rows = stmt.query_map([plan_id], |row| row.get::<_, String>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(serde_json::from_str(&row?)?);
            }
            Ok(out)
        })
    }

    pub fn request_plan_approval(
        &self,
        work: &WorkBreakdown,
        diff: &PlanScopeDiff,
    ) -> Result<PlanApprovalRecord> {
        let record = PlanApprovalRecord {
            id: format!("plan_appr_{}", uuid::Uuid::new_v4().simple()),
            plan_id: work.id.clone(),
            version: work.version,
            approved: None,
            scope_diff: diff.clone(),
            requested_at: crate::now_rfc3339(),
            resolved_at: None,
            approver: None,
        };
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO plan_approvals
                 (id,plan_id,plan_version,approved,scope_diff,requested_at)
                 VALUES (?1,?2,?3,NULL,?4,?5)",
                params![
                    record.id,
                    record.plan_id.as_str(),
                    record.version as i64,
                    serde_json::to_string(diff)?,
                    record.requested_at,
                ],
            )?;
            Ok(())
        })?;
        Ok(record)
    }

    pub fn resolve_plan_approval(
        &self,
        approval_id: &str,
        approved: bool,
        approver: &str,
    ) -> Result<()> {
        self.store.with(|conn| {
            let changed = conn.execute(
                "UPDATE plan_approvals SET approved=?1,resolved_at=?2,approver=?3
                 WHERE id=?4 AND approved IS NULL",
                params![
                    i64::from(approved),
                    crate::now_rfc3339(),
                    approver,
                    approval_id
                ],
            )?;
            if changed == 0 {
                return Err(NexusError::NotFound(format!(
                    "pending plan approval `{approval_id}`"
                )));
            }
            Ok(())
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_task(
        &self,
        session_id: &str,
        title: &str,
        objective: &str,
        owner: &str,
        writer: bool,
        plan_id: Option<&str>,
        stage_id: Option<&str>,
        budget: WorkBudget,
    ) -> Result<BackgroundTask> {
        let now = crate::now_rfc3339();
        let id = TaskId::generate();
        let task = BackgroundTask {
            branch: writer.then(|| format!("snx/task/{}", id.as_str())),
            id,
            session_id: SessionId::from(session_id),
            plan_id: plan_id.map(String::from),
            stage_id: stage_id.map(String::from),
            title: title.trim().to_string(),
            objective: objective.trim().to_string(),
            status: TaskStatus::Queued,
            owner: owner.to_string(),
            writer,
            worktree: None,
            budget,
            result: None,
            error: None,
            attempts: 0,
            lease_owner: None,
            lease_expires_at: None,
            heartbeat_at: None,
            created_at: now.clone(),
            updated_at: now,
            started_at: None,
            finished_at: None,
        };
        if task.title.is_empty() || task.objective.is_empty() {
            return Err(NexusError::Config(
                "task title and objective must be non-empty".into(),
            ));
        }
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO background_tasks
                 (id,session_id,plan_id,stage_id,title,objective,status,owner,writer,
                  branch,worktree,budget_json,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?13)",
                params![
                    task.id.as_str(),
                    task.session_id.as_str(),
                    task.plan_id,
                    task.stage_id,
                    task.title,
                    task.objective,
                    task.status.as_str(),
                    task.owner,
                    i64::from(task.writer),
                    task.branch,
                    task.worktree,
                    serde_json::to_string(&task.budget)?,
                    task.created_at,
                ],
            )?;
            Ok(())
        })?;
        Ok(task)
    }

    pub fn task(&self, task_id: &str) -> Result<BackgroundTask> {
        self.store.with(|conn| {
            conn.query_row(
                "SELECT id,session_id,plan_id,stage_id,title,objective,status,owner,
                        writer,branch,worktree,budget_json,result_json,error,attempts,
                        lease_owner,lease_expires_at,heartbeat_at,created_at,updated_at,
                        started_at,finished_at
                 FROM background_tasks WHERE id=?1",
                [task_id],
                row_to_task,
            )
            .map_err(|_| NexusError::NotFound(format!("task `{task_id}`")))
        })
    }

    pub fn tasks(
        &self,
        session_id: Option<&str>,
        include_terminal: bool,
    ) -> Result<Vec<BackgroundTask>> {
        self.store.with(|conn| {
            let sql = match (session_id.is_some(), include_terminal) {
                (true, true) => {
                    "SELECT id,session_id,plan_id,stage_id,title,objective,status,owner,
                            writer,branch,worktree,budget_json,result_json,error,attempts,
                            lease_owner,lease_expires_at,heartbeat_at,created_at,updated_at,
                            started_at,finished_at
                     FROM background_tasks WHERE session_id=?1 ORDER BY created_at"
                }
                (true, false) => {
                    "SELECT id,session_id,plan_id,stage_id,title,objective,status,owner,
                            writer,branch,worktree,budget_json,result_json,error,attempts,
                            lease_owner,lease_expires_at,heartbeat_at,created_at,updated_at,
                            started_at,finished_at
                     FROM background_tasks WHERE session_id=?1
                       AND status NOT IN ('completed','failed','cancelled')
                     ORDER BY created_at"
                }
                (false, true) => {
                    "SELECT id,session_id,plan_id,stage_id,title,objective,status,owner,
                            writer,branch,worktree,budget_json,result_json,error,attempts,
                            lease_owner,lease_expires_at,heartbeat_at,created_at,updated_at,
                            started_at,finished_at
                     FROM background_tasks ORDER BY created_at"
                }
                (false, false) => {
                    "SELECT id,session_id,plan_id,stage_id,title,objective,status,owner,
                            writer,branch,worktree,budget_json,result_json,error,attempts,
                            lease_owner,lease_expires_at,heartbeat_at,created_at,updated_at,
                            started_at,finished_at
                     FROM background_tasks
                     WHERE status NOT IN ('completed','failed','cancelled')
                     ORDER BY created_at"
                }
            };
            let mut stmt = conn.prepare(sql)?;
            let mut out = Vec::new();
            if let Some(session_id) = session_id {
                let rows = stmt.query_map([session_id], row_to_task)?;
                for row in rows {
                    out.push(row?);
                }
            } else {
                let rows = stmt.query_map([], row_to_task)?;
                for row in rows {
                    out.push(row?);
                }
            }
            Ok(out)
        })
    }

    /// Link `task_id` so the scheduler leases it only after `depends_on`
    /// completes. Rejects unknown tasks, self-dependencies, cross-session
    /// edges, and edges that would create a cycle.
    pub fn add_task_dependency(&self, task_id: &str, depends_on: &str) -> Result<()> {
        if task_id == depends_on {
            return Err(NexusError::Config("a task cannot depend on itself".into()));
        }
        self.store.with(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<()> {
                let session_of = |id: &str| -> Result<String> {
                    conn.query_row(
                        "SELECT session_id FROM background_tasks WHERE id=?1",
                        [id],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(|_| NexusError::NotFound(format!("task `{id}`")))
                };
                let task_session = session_of(task_id)?;
                let dep_session = session_of(depends_on)?;
                if task_session != dep_session {
                    return Err(NexusError::Config(
                        "task dependencies must stay within one session".into(),
                    ));
                }
                let mut edge_stmt = conn.prepare(
                    "SELECT d.task_id, d.depends_on_task_id
                     FROM background_task_dependencies d
                     JOIN background_tasks t ON t.id = d.task_id
                     WHERE t.session_id = ?1",
                )?;
                let mut outgoing: HashMap<String, Vec<String>> = HashMap::new();
                for edge in edge_stmt.query_map([&task_session], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })? {
                    let (from, to) = edge?;
                    outgoing.entry(from).or_default().push(to);
                }
                // Adding task_id → depends_on creates a cycle iff task_id is
                // already reachable from depends_on along dependency edges.
                let mut stack = vec![depends_on.to_string()];
                let mut seen = BTreeSet::new();
                while let Some(node) = stack.pop() {
                    if node == task_id {
                        return Err(NexusError::Config("dependency would create a cycle".into()));
                    }
                    if seen.insert(node.clone()) {
                        if let Some(next) = outgoing.get(&node) {
                            stack.extend(next.iter().cloned());
                        }
                    }
                }
                conn.execute(
                    "INSERT OR IGNORE INTO background_task_dependencies
                     (task_id, depends_on_task_id, created_at) VALUES (?1,?2,?3)",
                    params![task_id, depends_on, crate::now_rfc3339()],
                )?;
                Ok(())
            })();
            match result {
                Ok(()) => {
                    conn.execute_batch("COMMIT")?;
                    Ok(())
                }
                Err(error) => {
                    conn.execute_batch("ROLLBACK")?;
                    Err(error)
                }
            }
        })
    }

    /// Dependencies of one task as (dependency id, dependency status).
    pub fn task_dependencies(&self, task_id: &str) -> Result<Vec<(String, TaskStatus)>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT d.depends_on_task_id, dep.status
                 FROM background_task_dependencies d
                 JOIN background_tasks dep ON dep.id = d.depends_on_task_id
                 WHERE d.task_id = ?1
                 ORDER BY d.depends_on_task_id",
            )?;
            let mut out = Vec::new();
            for row in stmt.query_map([task_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    TaskStatus::parse(&row.get::<_, String>(1)?),
                ))
            })? {
                out.push(row?);
            }
            Ok(out)
        })
    }

    /// All dependency edges for a session as (task_id, depends_on_task_id).
    pub fn dependency_edges(&self, session_id: &str) -> Result<Vec<(String, String)>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT d.task_id, d.depends_on_task_id
                 FROM background_task_dependencies d
                 JOIN background_tasks t ON t.id = d.task_id
                 WHERE t.session_id = ?1
                 ORDER BY d.task_id, d.depends_on_task_id",
            )?;
            let mut out = Vec::new();
            for row in stmt.query_map([session_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })? {
                out.push(row?);
            }
            Ok(out)
        })
    }

    /// Claim one queued task with a SQLite lease. The worker supplies whether
    /// the single writer slot is available; reader concurrency is enforced by
    /// counting active read leases.
    pub fn lease_next(
        &self,
        worker: &str,
        lease_expires_at: &str,
        max_readers: usize,
        writer_available: bool,
    ) -> Result<Option<BackgroundTask>> {
        self.store.with(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result: rusqlite::Result<Option<String>> = (|| {
                let now = crate::now_rfc3339();
                // Dependency bookkeeping happens under the same lock as the
                // lease so workers can never observe a half-updated graph.
                // A queued task with a failed or cancelled dependency parks
                // as 'blocked'; a scheduler-blocked task whose dependencies
                // all completed (e.g. after a retry) re-queues itself.
                conn.execute(
                    "UPDATE background_tasks
                     SET status='blocked',
                         error='blocked: dependency failed or was cancelled',
                         updated_at=?1
                     WHERE status='queued' AND EXISTS (
                         SELECT 1 FROM background_task_dependencies d
                         JOIN background_tasks dep ON dep.id = d.depends_on_task_id
                         WHERE d.task_id = background_tasks.id
                           AND dep.status IN ('failed','cancelled'))",
                    params![now],
                )?;
                conn.execute(
                    "UPDATE background_tasks
                     SET status='queued', error=NULL, updated_at=?1
                     WHERE status='blocked'
                       AND error LIKE 'blocked: dependency%'
                       AND NOT EXISTS (
                         SELECT 1 FROM background_task_dependencies d
                         JOIN background_tasks dep ON dep.id = d.depends_on_task_id
                         WHERE d.task_id = background_tasks.id
                           AND dep.status <> 'completed')",
                    params![now],
                )?;
                let running_readers: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM background_tasks
                     WHERE status='running' AND writer=0",
                    [],
                    |row| row.get(0),
                )?;
                let running_writers: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM background_tasks
                     WHERE status='running' AND writer=1",
                    [],
                    |row| row.get(0),
                )?;
                let allow_reader = running_readers < max_readers as i64;
                let allow_writer = writer_available && running_writers == 0;
                let task_id: Option<String> = conn
                    .query_row(
                        "SELECT id FROM background_tasks
                         WHERE status='queued'
                           AND ((writer=1 AND ?1=1) OR (writer=0 AND ?2=1))
                           AND NOT EXISTS (
                             SELECT 1 FROM background_task_dependencies d
                             JOIN background_tasks dep ON dep.id = d.depends_on_task_id
                             WHERE d.task_id = background_tasks.id
                               AND dep.status <> 'completed')
                         ORDER BY writer DESC, created_at LIMIT 1",
                        params![i64::from(allow_writer), i64::from(allow_reader)],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(task_id) = &task_id {
                    conn.execute(
                        "UPDATE background_tasks SET status='running',lease_owner=?1,
                         lease_expires_at=?2,heartbeat_at=?3,started_at=COALESCE(started_at,?3),
                         attempts=attempts+1,updated_at=?3 WHERE id=?4",
                        params![worker, lease_expires_at, now, task_id],
                    )?;
                }
                Ok(task_id)
            })();
            match result {
                Ok(task_id) => {
                    conn.execute_batch("COMMIT")?;
                    task_id
                        .map(|task_id| {
                            conn.query_row(
                                "SELECT id,session_id,plan_id,stage_id,title,objective,status,owner,
                                        writer,branch,worktree,budget_json,result_json,error,attempts,
                                        lease_owner,lease_expires_at,heartbeat_at,created_at,updated_at,
                                        started_at,finished_at
                                 FROM background_tasks WHERE id=?1",
                                [task_id],
                                row_to_task,
                            )
                            .map_err(NexusError::from)
                        })
                        .transpose()
                }
                Err(error) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    Err(error.into())
                }
            }
        })
    }

    pub fn heartbeat_task(
        &self,
        task_id: &str,
        worker: &str,
        lease_expires_at: &str,
    ) -> Result<()> {
        self.store.with(|conn| {
            let changed = conn.execute(
                "UPDATE background_tasks SET heartbeat_at=?1,lease_expires_at=?2,
                 updated_at=?1 WHERE id=?3 AND lease_owner=?4 AND status='running'",
                params![crate::now_rfc3339(), lease_expires_at, task_id, worker],
            )?;
            if changed == 0 {
                return Err(NexusError::NotFound(format!(
                    "active lease for task `{task_id}`"
                )));
            }
            Ok(())
        })
    }

    pub fn set_task_workspace(
        &self,
        task_id: &str,
        branch: Option<&str>,
        worktree: Option<&str>,
    ) -> Result<()> {
        self.store.with(|conn| {
            let changed = conn.execute(
                "UPDATE background_tasks SET branch=?1,worktree=?2,updated_at=?3 WHERE id=?4",
                params![branch, worktree, crate::now_rfc3339(), task_id],
            )?;
            if changed == 0 {
                return Err(NexusError::NotFound(format!("task `{task_id}`")));
            }
            Ok(())
        })
    }

    pub fn set_task_status(
        &self,
        task_id: &str,
        status: TaskStatus,
        result: Option<&ValueEnvelope>,
        error: Option<&str>,
    ) -> Result<()> {
        self.store.with(|conn| {
            let now = crate::now_rfc3339();
            let finished = status.terminal().then_some(now.as_str());
            let changed = conn.execute(
                "UPDATE background_tasks SET status=?1,result_json=?2,error=?3,
                 lease_owner=NULL,lease_expires_at=NULL,heartbeat_at=NULL,
                 updated_at=?4,finished_at=COALESCE(?5,finished_at) WHERE id=?6",
                params![
                    status.as_str(),
                    result.map(serde_json::to_string).transpose()?,
                    error,
                    now,
                    finished,
                    task_id,
                ],
            )?;
            if changed == 0 {
                return Err(NexusError::NotFound(format!("task `{task_id}`")));
            }
            Ok(())
        })
    }

    /// Reassign a not-yet-running task to a different owner (agent role or
    /// worker identity). Running and terminal tasks keep their owner so the
    /// audit trail stays coherent with the lease that executed them.
    pub fn assign_task(&self, task_id: &str, owner: &str) -> Result<()> {
        let owner = owner.trim();
        if owner.is_empty() {
            return Err(NexusError::Config("task owner must be non-empty".into()));
        }
        let task = self.task(task_id)?;
        if !matches!(
            task.status,
            TaskStatus::Queued | TaskStatus::Blocked | TaskStatus::Paused
        ) {
            return Err(NexusError::Other(format!(
                "task `{task_id}` is {}, only queued/blocked/paused tasks can be reassigned",
                task.status.as_str()
            )));
        }
        self.store.with(|conn| {
            conn.execute(
                "UPDATE background_tasks SET owner=?1,updated_at=?2 WHERE id=?3",
                params![owner, crate::now_rfc3339(), task_id],
            )?;
            Ok(())
        })
    }

    pub fn retry_task(&self, task_id: &str) -> Result<()> {
        let task = self.task(task_id)?;
        if !matches!(task.status, TaskStatus::Failed | TaskStatus::Cancelled) {
            return Err(NexusError::Other(format!(
                "task `{task_id}` is {}, not failed/cancelled",
                task.status.as_str()
            )));
        }
        self.store.with(|conn| {
            conn.execute(
                "UPDATE background_tasks SET status='queued',result_json=NULL,error=NULL,
                 finished_at=NULL,updated_at=?1 WHERE id=?2",
                params![crate::now_rfc3339(), task_id],
            )?;
            Ok(())
        })
    }

    /// Recover stale running tasks after a worker crash. Side effects are
    /// still protected by tool-call idempotency records when the task resumes.
    pub fn recover_stale_tasks(&self, now: &str) -> Result<usize> {
        self.store.with(|conn| {
            Ok(conn.execute(
                "UPDATE background_tasks SET status='queued',lease_owner=NULL,
                 lease_expires_at=NULL,heartbeat_at=NULL,
                 error='worker lease expired; recovered for retry',updated_at=?1
                 WHERE status='running' AND lease_expires_at IS NOT NULL
                   AND lease_expires_at < ?1",
                [now],
            )?)
        })
    }

    pub fn cleanup_tasks(&self, session_id: &str) -> Result<usize> {
        self.store.with(|conn| {
            Ok(conn.execute(
                "DELETE FROM background_tasks WHERE session_id=?1
                 AND status IN ('completed','failed','cancelled')",
                [session_id],
            )?)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_agent_run(
        &self,
        session_id: &str,
        parent_run_id: Option<&str>,
        task_id: Option<&str>,
        role: &str,
        objective: &str,
        model: &str,
        permission_mode: &str,
        budget: WorkBudget,
    ) -> Result<AgentRun> {
        let depth = if let Some(parent) = parent_run_id {
            let parent = self.agent_run(parent)?;
            if parent.depth >= 2 {
                return Err(NexusError::PolicyDenied(
                    "subagent delegation depth is limited to 2".into(),
                ));
            }
            let children = self
                .agent_runs(session_id)?
                .iter()
                .filter(|run| {
                    run.parent_run_id.as_ref().map(AgentId::as_str) == Some(parent.id.as_str())
                })
                .count();
            if children >= 8 {
                return Err(NexusError::PolicyDenied(
                    "an orchestrator may create at most 8 children".into(),
                ));
            }
            parent.depth + 1
        } else {
            let root_children = self
                .agent_runs(session_id)?
                .iter()
                .filter(|run| run.parent_run_id.is_none())
                .count();
            if root_children >= 8 {
                return Err(NexusError::PolicyDenied(
                    "an orchestrator may create at most 8 root children".into(),
                ));
            }
            0
        };
        let now = crate::now_rfc3339();
        let run = AgentRun {
            id: AgentId::generate(),
            session_id: SessionId::from(session_id),
            parent_run_id: parent_run_id.map(AgentId::from),
            task_id: task_id.map(TaskId::from),
            role: role.to_string(),
            objective: objective.to_string(),
            status: TaskStatus::Queued,
            depth,
            model: model.to_string(),
            permission_mode: permission_mode.to_string(),
            budget,
            unread_events: 0,
            result: None,
            error: None,
            created_at: now.clone(),
            updated_at: now,
            started_at: None,
            finished_at: None,
        };
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO agent_runs
                 (id,session_id,parent_run_id,task_id,role,objective,status,depth,
                  model,permission_mode,budget_json,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?12)",
                params![
                    run.id.as_str(),
                    run.session_id.as_str(),
                    run.parent_run_id.as_ref().map(AgentId::as_str),
                    run.task_id.as_ref().map(TaskId::as_str),
                    run.role,
                    run.objective,
                    run.status.as_str(),
                    run.depth as i64,
                    run.model,
                    run.permission_mode,
                    serde_json::to_string(&run.budget)?,
                    run.created_at,
                ],
            )?;
            Ok(())
        })?;
        Ok(run)
    }

    pub fn agent_run(&self, run_id: &str) -> Result<AgentRun> {
        self.store.with(|conn| {
            conn.query_row(
                "SELECT id,session_id,parent_run_id,task_id,role,objective,status,
                        depth,model,permission_mode,budget_json,unread_events,
                        result_json,error,created_at,updated_at,started_at,finished_at
                 FROM agent_runs WHERE id=?1",
                [run_id],
                row_to_agent_run,
            )
            .map_err(|_| NexusError::NotFound(format!("agent run `{run_id}`")))
        })
    }

    pub fn agent_runs(&self, session_id: &str) -> Result<Vec<AgentRun>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id,session_id,parent_run_id,task_id,role,objective,status,
                        depth,model,permission_mode,budget_json,unread_events,
                        result_json,error,created_at,updated_at,started_at,finished_at
                 FROM agent_runs WHERE session_id=?1 ORDER BY created_at",
            )?;
            let rows = stmt.query_map([session_id], row_to_agent_run)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    pub fn agent_run_for_task(&self, task_id: &str) -> Result<Option<AgentRun>> {
        self.store.with(|conn| {
            conn.query_row(
                "SELECT id,session_id,parent_run_id,task_id,role,objective,status,
                        depth,model,permission_mode,budget_json,unread_events,
                        result_json,error,created_at,updated_at,started_at,finished_at
                 FROM agent_runs WHERE task_id=?1 ORDER BY created_at DESC LIMIT 1",
                [task_id],
                row_to_agent_run,
            )
            .optional()
            .map_err(NexusError::from)
        })
    }

    pub fn set_agent_run_status(
        &self,
        run_id: &str,
        status: TaskStatus,
        result: Option<&ValueEnvelope>,
        error: Option<&str>,
    ) -> Result<()> {
        self.store.with(|conn| {
            let now = crate::now_rfc3339();
            let started = (status == TaskStatus::Running).then_some(now.as_str());
            let finished = status.terminal().then_some(now.as_str());
            let changed = conn.execute(
                "UPDATE agent_runs SET status=?1,result_json=?2,error=?3,
                 updated_at=?4,started_at=COALESCE(started_at,?5),
                 finished_at=COALESCE(finished_at,?6) WHERE id=?7",
                params![
                    status.as_str(),
                    result.map(serde_json::to_string).transpose()?,
                    error,
                    now,
                    started,
                    finished,
                    run_id,
                ],
            )?;
            if changed == 0 {
                return Err(NexusError::NotFound(format!("agent run `{run_id}`")));
            }
            Ok(())
        })
    }

    pub fn increment_agent_unread(&self, run_id: &str) -> Result<()> {
        self.store.with(|conn| {
            let changed = conn.execute(
                "UPDATE agent_runs SET unread_events=unread_events+1,updated_at=?1 WHERE id=?2",
                params![crate::now_rfc3339(), run_id],
            )?;
            if changed == 0 {
                return Err(NexusError::NotFound(format!("agent run `{run_id}`")));
            }
            Ok(())
        })
    }

    pub fn mark_agent_runs_read(&self, session_id: &str) -> Result<()> {
        self.store.with(|conn| {
            conn.execute(
                "UPDATE agent_runs SET unread_events=0,updated_at=?1 WHERE session_id=?2",
                params![crate::now_rfc3339(), session_id],
            )?;
            Ok(())
        })
    }

    pub fn record_interruption(&self, interruption: &SessionInterruption) -> Result<()> {
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO session_interruptions
                 (id,session_id,turn_id,trace_id,kind,provider,model,message,reset_at,
                  retryable,checkpoint_artifact,child_session_id,created_at,resolved_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                params![
                    interruption.id.as_str(),
                    interruption.session_id.as_str(),
                    interruption.turn_id.as_str(),
                    interruption.trace_id.as_str(),
                    interruption_kind_label(interruption.kind),
                    interruption.provider,
                    interruption.model,
                    interruption.message,
                    interruption.reset_at,
                    i64::from(interruption.retryable),
                    interruption.checkpoint_artifact,
                    interruption
                        .child_session_id
                        .as_ref()
                        .map(SessionId::as_str),
                    interruption.created_at,
                    interruption.resolved_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn latest_interruption(&self, session_id: &str) -> Result<Option<SessionInterruption>> {
        self.store.with(|conn| {
            conn.query_row(
                "SELECT id,session_id,turn_id,trace_id,kind,provider,model,message,
                        reset_at,retryable,checkpoint_artifact,child_session_id,
                        created_at,resolved_at
                 FROM session_interruptions
                 WHERE session_id=?1 AND resolved_at IS NULL
                 ORDER BY created_at DESC LIMIT 1",
                [session_id],
                row_to_interruption,
            )
            .optional()
            .map_err(NexusError::from)
        })
    }

    pub fn link_interruption_child(
        &self,
        interruption_id: &str,
        child_session_id: &str,
        checkpoint_artifact: Option<&str>,
    ) -> Result<()> {
        self.store.with(|conn| {
            let changed = conn.execute(
                "UPDATE session_interruptions
                 SET child_session_id=?1,checkpoint_artifact=?2,resolved_at=?3
                 WHERE id=?4 AND resolved_at IS NULL",
                params![
                    child_session_id,
                    checkpoint_artifact,
                    crate::now_rfc3339(),
                    interruption_id
                ],
            )?;
            if changed == 0 {
                return Err(NexusError::NotFound(format!(
                    "unresolved interruption `{interruption_id}`"
                )));
            }
            Ok(())
        })
    }
}

fn plan_title(objective: &str) -> String {
    let mut title: String = objective
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(80)
        .collect();
    if objective.lines().next().unwrap_or("").chars().count() > 80 {
        title.push('…');
    }
    if title.trim().is_empty() {
        "Untitled plan".into()
    } else {
        title
    }
}

fn persisted_stage_id(work: &WorkBreakdown, stage: &Stage) -> String {
    format!("{}:v{}:{}", work.id.as_str(), work.version, stage.id)
}

fn to_sql_json_error(error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<BackgroundTask> {
    let budget: String = row.get(11)?;
    let result: Option<String> = row.get(12)?;
    Ok(BackgroundTask {
        id: TaskId::from(row.get::<_, String>(0)?),
        session_id: SessionId::from(row.get::<_, String>(1)?),
        plan_id: row.get(2)?,
        stage_id: row.get(3)?,
        title: row.get(4)?,
        objective: row.get(5)?,
        status: TaskStatus::parse(&row.get::<_, String>(6)?),
        owner: row.get(7)?,
        writer: row.get::<_, i64>(8)? != 0,
        branch: row.get(9)?,
        worktree: row.get(10)?,
        budget: serde_json::from_str(&budget).unwrap_or_default(),
        result: result.and_then(|value| serde_json::from_str(&value).ok()),
        error: row.get(13)?,
        attempts: row.get::<_, i64>(14)?.max(0) as u32,
        lease_owner: row.get(15)?,
        lease_expires_at: row.get(16)?,
        heartbeat_at: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
        started_at: row.get(20)?,
        finished_at: row.get(21)?,
    })
}

fn row_to_agent_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentRun> {
    let budget: String = row.get(10)?;
    let result: Option<String> = row.get(12)?;
    Ok(AgentRun {
        id: AgentId::from(row.get::<_, String>(0)?),
        session_id: SessionId::from(row.get::<_, String>(1)?),
        parent_run_id: row.get::<_, Option<String>>(2)?.map(AgentId::from),
        task_id: row.get::<_, Option<String>>(3)?.map(TaskId::from),
        role: row.get(4)?,
        objective: row.get(5)?,
        status: TaskStatus::parse(&row.get::<_, String>(6)?),
        depth: row.get::<_, i64>(7)?.clamp(0, u8::MAX as i64) as u8,
        model: row.get(8)?,
        permission_mode: row.get(9)?,
        budget: serde_json::from_str(&budget).unwrap_or_default(),
        unread_events: row.get::<_, i64>(11)?.max(0) as u64,
        result: result.and_then(|value| serde_json::from_str(&value).ok()),
        error: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
        started_at: row.get(16)?,
        finished_at: row.get(17)?,
    })
}

fn row_to_interruption(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionInterruption> {
    Ok(SessionInterruption {
        id: crate::ids::InterruptionId::from(row.get::<_, String>(0)?),
        session_id: SessionId::from(row.get::<_, String>(1)?),
        turn_id: TurnId::from(row.get::<_, String>(2)?),
        trace_id: TraceId::from(row.get::<_, String>(3)?),
        kind: parse_interruption_kind(&row.get::<_, String>(4)?),
        provider: row.get(5)?,
        model: row.get(6)?,
        message: row.get(7)?,
        reset_at: row.get(8)?,
        retryable: row.get::<_, i64>(9)? != 0,
        checkpoint_artifact: row.get(10)?,
        child_session_id: row.get::<_, Option<String>>(11)?.map(SessionId::from),
        created_at: row.get(12)?,
        resolved_at: row.get(13)?,
    })
}

fn interruption_kind_label(kind: InterruptionKind) -> &'static str {
    match kind {
        InterruptionKind::Quota => "quota",
        InterruptionKind::Plan => "plan",
        InterruptionKind::Rate => "rate",
        InterruptionKind::Context => "context",
        InterruptionKind::Authentication => "authentication",
        InterruptionKind::InvalidRequest => "invalid_request",
        InterruptionKind::Transport => "transport",
        InterruptionKind::Cancellation => "cancellation",
        InterruptionKind::Crash => "crash",
    }
}

fn parse_interruption_kind(value: &str) -> InterruptionKind {
    match value {
        "quota" => InterruptionKind::Quota,
        "plan" => InterruptionKind::Plan,
        "rate" => InterruptionKind::Rate,
        "context" => InterruptionKind::Context,
        "authentication" => InterruptionKind::Authentication,
        "invalid_request" => InterruptionKind::InvalidRequest,
        "transport" => InterruptionKind::Transport,
        "cancellation" => InterruptionKind::Cancellation,
        _ => InterruptionKind::Crash,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn orchestration_store() -> (Store, OrchestrationStore, String) {
        let store = Store::open_in_memory().expect("store");
        let session = SessionId::generate().to_string();
        store
            .with(|conn| {
                conn.execute(
                    "INSERT INTO sessions
                     (id,title,workspace,created_at,updated_at,model,agent,status)
                     VALUES (?1,'','/workspace',?2,?2,'mock','orchestrator','active')",
                    params![session, crate::now_rfc3339()],
                )?;
                Ok(())
            })
            .expect("session");
        (store.clone(), OrchestrationStore::new(store), session)
    }

    #[test]
    fn complexity_policy_matches_release_contract() {
        let direct = WorkEstimate {
            predicted_actions: 1,
            writes: true,
            predictable: true,
            ..Default::default()
        };
        assert_eq!(direct.classify(), WorkBreakdownKind::Direct);

        let tracked = WorkEstimate {
            predicted_actions: 3,
            writes: true,
            predictable: true,
            ..Default::default()
        };
        assert_eq!(tracked.classify(), WorkBreakdownKind::Tracked);

        let planned = WorkEstimate {
            predicted_actions: 2,
            multi_file: true,
            ..Default::default()
        };
        assert_eq!(planned.classify(), WorkBreakdownKind::Planned);
    }

    #[test]
    fn planned_work_blocks_implementation_until_approval() {
        let mut work = WorkBreakdown::generate(
            "migrate storage and redesign the TUI",
            WorkEstimate {
                predicted_actions: 8,
                migration: true,
                needs_grounding: true,
                ..Default::default()
            },
        );
        assert_eq!(work.kind, WorkBreakdownKind::Planned);
        assert!(!work.approved);
        assert_eq!(
            work.stages
                .iter()
                .find(|stage| stage.title == "Implementation")
                .map(|stage| stage.status),
            Some(StageStatus::Blocked)
        );
        work.approve();
        assert!(work.approved);
        assert_eq!(
            work.stages
                .iter()
                .find(|stage| stage.title == "Implementation")
                .map(|stage| stage.status),
            Some(StageStatus::Running)
        );
    }

    #[test]
    fn promotion_is_visible_and_versioned() {
        let mut work = WorkBreakdown::generate(
            "read one file",
            WorkEstimate {
                predicted_actions: 1,
                predictable: true,
                ..Default::default()
            },
        );
        work.record_current_evidence("read src/lib.rs", &["src/lib.rs".into()], None);
        let promotion = work
            .promote(WorkEstimate {
                predicted_actions: 6,
                multi_file: true,
                rationale: vec!["scope expanded to multiple files".into()],
                ..Default::default()
            })
            .expect("promoted");
        assert_eq!(promotion.from, WorkBreakdownKind::Direct);
        assert_eq!(promotion.to, WorkBreakdownKind::Planned);
        assert_eq!(work.version, 2);
        assert!(work
            .stages
            .iter()
            .any(|stage| stage.evidence.contains(&"read src/lib.rs".to_string())));
        assert!(work
            .stages
            .iter()
            .any(|stage| stage.changed_files.contains(&"src/lib.rs".to_string())));
    }

    #[test]
    fn continuation_clones_plan_with_stage_state_and_new_identity() {
        let (store, orchestration, source_session) = orchestration_store();
        let target_session = SessionId::generate().to_string();
        store
            .with(|conn| {
                conn.execute(
                    "INSERT INTO sessions
                     (id,title,workspace,created_at,updated_at,model,agent,status)
                     VALUES (?1,'','/workspace',?2,?2,'mock','orchestrator','active')",
                    params![target_session, crate::now_rfc3339()],
                )?;
                Ok(())
            })
            .expect("target session");
        let mut work = WorkBreakdown::generate(
            "continue a planned migration",
            WorkEstimate {
                predicted_actions: 7,
                migration: true,
                needs_grounding: true,
                ..Default::default()
            },
        );
        work.approve();
        work.stages[0].evidence.push("schema inspected".into());
        orchestration
            .save_plan(&source_session, &work, "approved", "test")
            .expect("source plan");

        let cloned = orchestration
            .clone_latest_plan(&source_session, &target_session, "continuation")
            .expect("clone")
            .expect("plan");
        assert_ne!(cloned.id, work.id);
        assert_eq!(cloned.version, work.version);
        assert_eq!(cloned.current_stage, work.current_stage);
        assert_eq!(cloned.stages[0].evidence, vec!["schema inspected"]);
        assert!(cloned.approved);
        assert_eq!(
            orchestration
                .latest_plan(&target_session)
                .expect("target")
                .expect("target plan")
                .id,
            cloned.id
        );
    }

    #[test]
    fn stage_transitions_preserve_evidence_and_pending_validation() {
        let mut work = WorkBreakdown::generate(
            "inspect existing code and update one file",
            WorkEstimate {
                predicted_actions: 3,
                writes: true,
                predictable: true,
                needs_grounding: true,
                ..Default::default()
            },
        );
        work.record_current_evidence("read current implementation", &[], None);
        let changed = work.transition_to("Implementation");
        assert_eq!(changed.len(), 2);
        work.record_current_evidence("updated src/lib.rs", &["src/lib.rs".into()], None);
        let completed = work
            .finish_current(StageStatus::Completed)
            .expect("implementation");
        assert_eq!(completed.title, "Implementation");
        assert_eq!(
            work.next_stage.as_deref(),
            work.stages
                .iter()
                .find(|stage| stage.title == "Validation")
                .map(|stage| stage.id.as_str())
        );
        assert_eq!(work.progress(), (2, 3));
    }

    #[test]
    fn task_leases_enforce_three_readers_and_one_writer() {
        let (_store, orchestration, session) = orchestration_store();
        let writer = orchestration
            .create_task(
                &session,
                "writer",
                "change one file",
                "implementer",
                true,
                None,
                None,
                WorkBudget::default(),
            )
            .expect("writer");
        let expected_branch = format!("snx/task/{}", writer.id.as_str());
        assert_eq!(writer.branch.as_deref(), Some(expected_branch.as_str()));
        for index in 0..4 {
            orchestration
                .create_task(
                    &session,
                    &format!("reader {index}"),
                    "inspect",
                    "reviewer",
                    false,
                    None,
                    None,
                    WorkBudget::default(),
                )
                .expect("reader");
        }
        let expiry = "2999-01-01T00:00:00Z";
        assert!(orchestration
            .lease_next("worker", expiry, 3, true)
            .expect("lease writer")
            .is_some_and(|task| task.writer));
        for _ in 0..3 {
            assert!(orchestration
                .lease_next("worker", expiry, 3, true)
                .expect("lease reader")
                .is_some_and(|task| !task.writer));
        }
        assert!(orchestration
            .lease_next("worker", expiry, 3, true)
            .expect("capacity")
            .is_none());
    }

    #[test]
    fn orchestrator_root_fanout_is_bounded_to_eight_children() {
        let (_store, orchestration, session) = orchestration_store();
        for index in 0..8 {
            orchestration
                .create_agent_run(
                    &session,
                    None,
                    None,
                    "reviewer",
                    &format!("review area {index}"),
                    "mock",
                    "default",
                    WorkBudget::default(),
                )
                .expect("root child");
        }
        let ninth = orchestration.create_agent_run(
            &session,
            None,
            None,
            "reviewer",
            "review another area",
            "mock",
            "default",
            WorkBudget::default(),
        );
        assert!(matches!(ninth, Err(NexusError::PolicyDenied(_))));
    }

    #[test]
    fn plan_task_agent_and_interruption_records_roundtrip() {
        let (_store, orchestration, session) = orchestration_store();
        let work = WorkBreakdown::generate(
            "tracked change",
            WorkEstimate {
                predicted_actions: 3,
                writes: true,
                predictable: true,
                ..Default::default()
            },
        );
        orchestration
            .save_plan(&session, &work, "approved", "test")
            .expect("save plan");
        assert_eq!(
            orchestration
                .latest_plan(&session)
                .expect("load plan")
                .map(|loaded| loaded.id),
            Some(work.id)
        );
        let task = orchestration
            .create_task(
                &session,
                "review",
                "inspect timeline",
                "reviewer",
                false,
                None,
                None,
                WorkBudget::default(),
            )
            .expect("task");
        let run = orchestration
            .create_agent_run(
                &session,
                None,
                Some(task.id.as_str()),
                "reviewer",
                "inspect timeline",
                "mock",
                "read-only",
                WorkBudget::default(),
            )
            .expect("run");
        assert_eq!(
            orchestration
                .agent_run_for_task(task.id.as_str())
                .expect("run by task")
                .map(|loaded| loaded.id),
            Some(run.id)
        );
        let interruption = SessionInterruption {
            id: crate::InterruptionId::generate(),
            session_id: SessionId::from(session.clone()),
            turn_id: TurnId::generate(),
            trace_id: TraceId::generate(),
            kind: InterruptionKind::Rate,
            provider: Some("test".into()),
            model: Some("mock".into()),
            message: "HTTP 429".into(),
            reset_at: Some("2999-01-01T00:00:00Z".into()),
            retryable: true,
            checkpoint_artifact: None,
            child_session_id: None,
            created_at: crate::now_rfc3339(),
            resolved_at: None,
        };
        orchestration
            .record_interruption(&interruption)
            .expect("interruption");
        assert_eq!(
            orchestration
                .latest_interruption(&session)
                .expect("latest")
                .map(|loaded| loaded.kind),
            Some(InterruptionKind::Rate)
        );
    }

    #[test]
    fn interruption_classifier_distinguishes_provider_failures() {
        let rate = NexusError::Provider {
            provider: "test".into(),
            message: "HTTP 429 rate limit; reset_at=2027-01-01T00:00:00Z".into(),
        };
        let classified = classify_interruption(&rate);
        assert_eq!(classified.kind, InterruptionKind::Rate);
        assert_eq!(classified.reset_at.as_deref(), Some("2027-01-01T00:00:00Z"));
        let invalid = NexusError::Provider {
            provider: "test".into(),
            message: "HTTP 400 invalid request".into(),
        };
        assert_eq!(
            classify_interruption(&invalid).kind,
            InterruptionKind::InvalidRequest
        );
    }

    #[test]
    fn weak_model_constraint_shrinks_decomposition() {
        let estimate = WorkEstimate {
            predicted_actions: 1,
            writes: true,
            predictable: true,
            ..Default::default()
        };
        assert_eq!(
            estimate.classify(),
            WorkBreakdownKind::Direct,
            "a strong model may take the direct path"
        );
        let constrained = estimate.constrained_for_weak_model();
        assert_eq!(
            constrained.classify(),
            WorkBreakdownKind::Tracked,
            "a constrained model must decompose the same write into tracked stages"
        );
        assert!(constrained.needs_grounding);
        assert!(!constrained.predictable);
    }

    fn quick_task(
        orchestration: &OrchestrationStore,
        session: &str,
        title: &str,
    ) -> BackgroundTask {
        orchestration
            .create_task(
                session,
                title,
                &format!("objective for {title}"),
                "worker",
                false,
                None,
                None,
                WorkBudget::default(),
            )
            .expect("task")
    }

    #[test]
    fn dependency_blocks_lease_until_completed() {
        let (_store, orchestration, session) = orchestration_store();
        let first = quick_task(&orchestration, &session, "first");
        let second = quick_task(&orchestration, &session, "second");
        orchestration
            .add_task_dependency(second.id.as_str(), first.id.as_str())
            .expect("dependency");

        let leased = orchestration
            .lease_next("w1", "2999-01-01T00:00:00Z", 3, true)
            .expect("lease")
            .expect("first is ready");
        assert_eq!(leased.id, first.id, "only the dependency-free task leases");
        assert!(
            orchestration
                .lease_next("w1", "2999-01-01T00:00:00Z", 3, true)
                .expect("lease")
                .is_none(),
            "dependent task must not lease while its dependency runs"
        );

        orchestration
            .set_task_status(first.id.as_str(), TaskStatus::Completed, None, None)
            .expect("complete");
        let unblocked = orchestration
            .lease_next("w1", "2999-01-01T00:00:00Z", 3, true)
            .expect("lease")
            .expect("second is ready after completion");
        assert_eq!(unblocked.id, second.id);
    }

    #[test]
    fn failed_dependency_parks_dependent_until_retry_completes() {
        let (_store, orchestration, session) = orchestration_store();
        let first = quick_task(&orchestration, &session, "first");
        let second = quick_task(&orchestration, &session, "second");
        orchestration
            .add_task_dependency(second.id.as_str(), first.id.as_str())
            .expect("dependency");
        orchestration
            .set_task_status(first.id.as_str(), TaskStatus::Failed, None, Some("boom"))
            .expect("fail");

        assert!(
            orchestration
                .lease_next("w1", "2999-01-01T00:00:00Z", 3, true)
                .expect("lease")
                .is_none(),
            "nothing is leasable with the dependency failed"
        );
        let parked = orchestration.task(second.id.as_str()).expect("task");
        assert_eq!(parked.status, TaskStatus::Blocked);
        assert!(parked.error.unwrap_or_default().starts_with("blocked:"));

        // A retry that completes the dependency re-queues the dependent.
        orchestration
            .set_task_status(first.id.as_str(), TaskStatus::Completed, None, None)
            .expect("complete");
        let unblocked = orchestration
            .lease_next("w1", "2999-01-01T00:00:00Z", 3, true)
            .expect("lease")
            .expect("dependent re-queued after dependency completion");
        assert_eq!(unblocked.id, second.id);
    }

    #[test]
    fn dependency_cycles_and_self_edges_are_rejected() {
        let (_store, orchestration, session) = orchestration_store();
        let first = quick_task(&orchestration, &session, "first");
        let second = quick_task(&orchestration, &session, "second");
        let third = quick_task(&orchestration, &session, "third");
        assert!(orchestration
            .add_task_dependency(first.id.as_str(), first.id.as_str())
            .is_err());
        orchestration
            .add_task_dependency(second.id.as_str(), first.id.as_str())
            .expect("edge");
        orchestration
            .add_task_dependency(third.id.as_str(), second.id.as_str())
            .expect("edge");
        assert!(
            orchestration
                .add_task_dependency(first.id.as_str(), third.id.as_str())
                .is_err(),
            "transitive cycle must be rejected"
        );
        let deps = orchestration
            .task_dependencies(third.id.as_str())
            .expect("deps");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].0, second.id.as_str());
        assert_eq!(
            orchestration
                .dependency_edges(&session)
                .expect("edges")
                .len(),
            2
        );
    }

    #[test]
    fn assign_task_only_touches_pending_work() {
        let (_store, orchestration, session) = orchestration_store();
        let task = quick_task(&orchestration, &session, "reassignable");
        assert!(orchestration.assign_task(task.id.as_str(), "  ").is_err());
        orchestration
            .assign_task(task.id.as_str(), "researcher")
            .expect("assign");
        assert_eq!(
            orchestration.task(task.id.as_str()).expect("task").owner,
            "researcher"
        );
        orchestration
            .set_task_status(task.id.as_str(), TaskStatus::Completed, None, None)
            .expect("complete");
        assert!(
            orchestration
                .assign_task(task.id.as_str(), "reviewer")
                .is_err(),
            "terminal tasks keep their owner"
        );
        assert!(orchestration.assign_task("t_missing", "reviewer").is_err());
    }
}
