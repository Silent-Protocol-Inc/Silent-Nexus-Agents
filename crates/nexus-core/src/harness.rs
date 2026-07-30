//! Canonical, provider-neutral persistence contracts for the adaptive harness.
//!
//! This module deliberately owns state and invariants, not rendering or model
//! behavior. Records use indexed relational ownership keys for isolation and
//! JSON payloads for forward-compatible domain detail. Queries always narrow
//! by an explicit scope before applying content matching.

use crate::store::Store;
use crate::{NexusError, Result};
use rusqlite::{params, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

pub const HARNESS_SCHEMA_VERSION: u32 = 2;

fn stable_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

/// Hash one checkpointed file exactly the way the loop hashes it when the
/// checkpoint is written (length + mtime nanos + first 1MiB of content), so
/// resume-time drift checks compare like with like. `None` when the path is
/// missing, unreadable, or not a regular file.
pub fn checkpoint_file_hash(path: &std::path::Path) -> Option<String> {
    use std::io::Read;
    const MAX_FILE_BYTES: u64 = 1024 * 1024;
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_FILE_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    let mut hasher = Sha256::new();
    hasher.update(metadata.len().to_be_bytes());
    if let Ok(modified) = metadata.modified() {
        if let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH) {
            hasher.update(since_epoch.as_nanos().to_be_bytes());
        }
    }
    hasher.update(&bytes);
    Some(hex::encode(hasher.finalize()))
}

fn encode<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

fn decode<T: DeserializeOwned>(payload: String) -> Result<T> {
    Ok(serde_json::from_str(&payload)?)
}

fn digest(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn normalized_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn default_sensitivity() -> String {
    "normal".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveHarnessContext {
    pub id: String,
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub profile_id: Option<String>,
    pub persona_id: Option<String>,
    pub persona_version: Option<u32>,
    pub agent_id: Option<String>,
    pub goal_id: Option<String>,
    pub plan_id: Option<String>,
    pub plan_version: Option<u32>,
    pub task_id: Option<String>,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub status: String,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
}

impl ActiveHarnessContext {
    pub fn new(workspace_id: impl Into<String>, session_id: Option<String>) -> Self {
        let now = crate::now_rfc3339();
        Self {
            id: stable_id("hctx"),
            workspace_id: workspace_id.into(),
            session_id,
            profile_id: None,
            persona_id: None,
            persona_version: None,
            agent_id: None,
            goal_id: None,
            plan_id: None,
            plan_version: None,
            task_id: None,
            provider_id: None,
            model_id: None,
            status: "active".into(),
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileStatus {
    Active,
    Inactive,
    Archived,
    Deleted,
}

impl ProfileStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Archived => "archived",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ProfileIdentity {
    pub pronouns: Option<String>,
    pub languages: Vec<String>,
    pub timezone: Option<String>,
    pub region: Option<String>,
    pub occupation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ProfilePreferences {
    pub communication_style: Vec<String>,
    pub response_format: Vec<String>,
    pub technical_stack: Vec<String>,
    pub tool_preferences: Vec<String>,
    pub provider_preferences: Vec<String>,
    pub model_preferences: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: String,
    pub display_name: String,
    pub preferred_name: Option<String>,
    pub aliases: Vec<String>,
    pub status: ProfileStatus,
    pub identity: ProfileIdentity,
    pub preferences: ProfilePreferences,
    pub projects: Vec<String>,
    pub constraints: Vec<String>,
    pub metadata: BTreeMap<String, Value>,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
    pub last_seen_at: Option<String>,
}

impl UserProfile {
    pub fn new(display_name: impl Into<String>) -> Result<Self> {
        let display_name = display_name.into();
        if display_name.trim().is_empty() {
            return Err(NexusError::Config(
                "profile display name is required".into(),
            ));
        }
        let now = crate::now_rfc3339();
        Ok(Self {
            id: stable_id("profile"),
            display_name: display_name.trim().to_string(),
            preferred_name: None,
            aliases: Vec::new(),
            status: ProfileStatus::Active,
            identity: ProfileIdentity::default(),
            preferences: ProfilePreferences::default(),
            projects: Vec::new(),
            constraints: Vec::new(),
            metadata: BTreeMap::new(),
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
            last_seen_at: None,
        })
    }

    pub fn is_default(&self) -> bool {
        self.metadata
            .get("is_default")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| normalized_name(&self.display_name) == "default")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileFactSource {
    UserExplicit,
    UserConfirmed,
    Imported,
}

impl ProfileFactSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::UserExplicit => "user_explicit",
            Self::UserConfirmed => "user_confirmed",
            Self::Imported => "imported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileFactStatus {
    Candidate,
    Active,
    Superseded,
    Rejected,
    Deleted,
}

impl ProfileFactStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Rejected => "rejected",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileFact {
    pub id: String,
    pub profile_id: String,
    pub key: String,
    pub value: Value,
    pub source_type: ProfileFactSource,
    pub source_ref: Option<String>,
    pub confidence: f64,
    pub sensitivity: String,
    pub status: ProfileFactStatus,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: Option<String>,
}

impl ProfileFact {
    pub fn explicit_name(profile_id: impl Into<String>, name: impl Into<String>) -> Self {
        let now = crate::now_rfc3339();
        Self {
            id: stable_id("pfact"),
            profile_id: profile_id.into(),
            key: "identity.name".into(),
            value: Value::String(name.into()),
            source_type: ProfileFactSource::UserExplicit,
            source_ref: None,
            confidence: 1.0,
            sensitivity: "normal".into(),
            status: ProfileFactStatus::Active,
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
            expires_at: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityConflictStatus {
    Pending,
    Resolved,
    Dismissed,
}

impl IdentityConflictStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Resolved => "resolved",
            Self::Dismissed => "dismissed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityConflict {
    pub id: String,
    pub active_profile_id: Option<String>,
    pub candidate_profile_id: Option<String>,
    pub asserted_name: String,
    pub matching_profile_ids: Vec<String>,
    pub source_ref: Option<String>,
    pub status: IdentityConflictStatus,
    pub resolution: Option<String>,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome", content = "record")]
pub enum IdentityResolution {
    Created(UserProfile),
    Activated(UserProfile),
    Conflict(IdentityConflict),
}

/// Explicit operator decision for a pending identity conflict. Separate
/// people are never merged implicitly; a profile id is always named when an
/// existing card should become active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "decision", content = "profile_id")]
pub enum IdentityConflictDecision {
    SwitchExisting(String),
    CreateSeparate,
    KeepActive,
    TemporaryContext,
    Dismiss,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityConflictResolution {
    pub conflict: IdentityConflict,
    pub selected_profile: Option<UserProfile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    Working,
    Session,
    Episodic,
    Semantic,
    Procedural,
}

impl MemoryType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Session => "session",
            Self::Episodic => "episodic",
            Self::Semantic => "semantic",
            Self::Procedural => "procedural",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MemoryScope {
    pub global: bool,
    pub profile_id: Option<String>,
    pub workspace_id: Option<String>,
    pub project_id: Option<String>,
    pub session_id: Option<String>,
    pub goal_id: Option<String>,
    pub plan_id: Option<String>,
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
}

impl MemoryScope {
    pub fn global() -> Self {
        Self {
            global: true,
            ..Self::default()
        }
    }

    pub fn profile(profile_id: impl Into<String>) -> Self {
        Self {
            profile_id: Some(profile_id.into()),
            ..Self::default()
        }
    }

    pub fn workspace(workspace_id: impl Into<String>) -> Self {
        Self {
            workspace_id: Some(workspace_id.into()),
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<()> {
        let has_private_dimension = self.profile_id.is_some()
            || self.workspace_id.is_some()
            || self.project_id.is_some()
            || self.session_id.is_some()
            || self.goal_id.is_some()
            || self.plan_id.is_some()
            || self.task_id.is_some()
            || self.agent_id.is_some();
        if self.global && has_private_dimension {
            return Err(NexusError::Config(
                "global memory cannot carry a private scope dimension".into(),
            ));
        }
        if !self.global && !has_private_dimension {
            return Err(NexusError::Config(
                "memory requires at least one explicit scope dimension".into(),
            ));
        }
        Ok(())
    }

    pub fn fingerprint(&self) -> Result<String> {
        self.validate()?;
        Ok(digest(&encode(self)?))
    }

    fn primary(&self) -> (&'static str, String) {
        for (kind, value) in [
            ("task", self.task_id.as_ref()),
            ("plan", self.plan_id.as_ref()),
            ("goal", self.goal_id.as_ref()),
            ("session", self.session_id.as_ref()),
            ("agent", self.agent_id.as_ref()),
            ("project", self.project_id.as_ref()),
            ("profile", self.profile_id.as_ref()),
            ("workspace", self.workspace_id.as_ref()),
        ] {
            if let Some(value) = value {
                return (kind, value.clone());
            }
        }
        ("global", "global".into())
    }
}

/// Build the exact memory-scope allowlist for one active harness context.
/// Both the application menus and the agent context compiler use this helper,
/// so retrieval cannot drift into a broader semantic-only query path.
pub fn authorized_memory_scopes(
    context: &ActiveHarnessContext,
    global_enabled: bool,
) -> Result<(Vec<MemoryScope>, Vec<MemoryScope>)> {
    let mut global = Vec::new();
    if global_enabled {
        global.push(MemoryScope::global());
    }
    if let Some(profile_id) = context.profile_id.clone() {
        global.push(MemoryScope::profile(profile_id));
    }

    let mut workspace = vec![MemoryScope::workspace(context.workspace_id.clone())];
    if let Some(profile_id) = context.profile_id.clone() {
        workspace.push(MemoryScope {
            profile_id: Some(profile_id),
            workspace_id: Some(context.workspace_id.clone()),
            ..MemoryScope::default()
        });
    }
    for scope in [
        context.session_id.clone().map(|session_id| MemoryScope {
            session_id: Some(session_id),
            ..MemoryScope::default()
        }),
        context.goal_id.clone().map(|goal_id| MemoryScope {
            goal_id: Some(goal_id),
            ..MemoryScope::default()
        }),
        context.plan_id.clone().map(|plan_id| MemoryScope {
            plan_id: Some(plan_id),
            ..MemoryScope::default()
        }),
        context.task_id.clone().map(|task_id| MemoryScope {
            task_id: Some(task_id),
            ..MemoryScope::default()
        }),
        context.agent_id.clone().map(|agent_id| MemoryScope {
            agent_id: Some(agent_id),
            ..MemoryScope::default()
        }),
    ]
    .into_iter()
    .flatten()
    {
        workspace.push(scope);
    }

    let contextual = MemoryScope {
        profile_id: context.profile_id.clone(),
        workspace_id: Some(context.workspace_id.clone()),
        session_id: context.session_id.clone(),
        goal_id: context.goal_id.clone(),
        plan_id: context.plan_id.clone(),
        task_id: context.task_id.clone(),
        agent_id: context.agent_id.clone(),
        ..MemoryScope::default()
    };
    workspace.push(contextual);

    for scopes in [&mut global, &mut workspace] {
        let mut seen = BTreeSet::new();
        let mut unique = Vec::with_capacity(scopes.len());
        for scope in scopes.drain(..) {
            if seen.insert(scope.fingerprint()?) {
                unique.push(scope);
            }
        }
        *scopes = unique;
    }
    Ok((global, workspace))
}

/// Canonical ranking score for scope-authorized memories. Scope filtering has
/// already happened by the time this runs; the score only orders records that
/// the caller is permitted to see. Explicit objective-term overlap dominates,
/// then stored importance and confidence, then recency, so retrieval stays
/// deterministic and never depends on a semantic-similarity service.
pub fn canonical_memory_score(record: &MemoryRecord, objective: &str) -> f64 {
    let mut haystack = record.content.to_lowercase();
    if let Some(summary) = &record.summary {
        haystack.push(' ');
        haystack.push_str(&summary.to_lowercase());
    }
    for tag in &record.tags {
        haystack.push(' ');
        haystack.push_str(&tag.to_lowercase());
    }

    let mut considered = 0usize;
    let mut matched = 0usize;
    for term in objective.to_lowercase().split_whitespace() {
        let term = term.trim_matches(|c: char| !c.is_alphanumeric());
        if term.chars().count() < 3 {
            continue;
        }
        considered += 1;
        if haystack.contains(term) {
            matched += 1;
        }
    }
    let relevance = if considered == 0 {
        0.0
    } else {
        matched as f64 / considered as f64
    };

    let mut score = relevance * 4.0
        + record.importance.clamp(0.0, 1.0) * 2.0
        + record.confidence.clamp(0.0, 1.0);
    if let Ok(updated) = chrono::DateTime::parse_from_rfc3339(&record.updated_at) {
        let days = (chrono::Utc::now() - updated.with_timezone(&chrono::Utc))
            .num_days()
            .max(0) as f64;
        score += (1.0 - days / 365.0).max(0.0);
    }
    score
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySourceType {
    UserExplicit,
    UserConfirmed,
    ToolObservation,
    TaskResult,
    AgentSummary,
    Imported,
}

impl MemorySourceType {
    fn as_str(self) -> &'static str {
        match self {
            Self::UserExplicit => "user_explicit",
            Self::UserConfirmed => "user_confirmed",
            Self::ToolObservation => "tool_observation",
            Self::TaskResult => "task_result",
            Self::AgentSummary => "agent_summary",
            Self::Imported => "imported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Candidate,
    Active,
    Superseded,
    Rejected,
    Archived,
    Deleted,
}

impl MemoryStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Rejected => "rejected",
            Self::Archived => "archived",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: String,
    pub memory_type: MemoryType,
    pub scope: MemoryScope,
    pub content: String,
    pub summary: Option<String>,
    pub tags: Vec<String>,
    pub source_refs: Vec<String>,
    pub source_type: MemorySourceType,
    #[serde(default = "default_sensitivity")]
    pub sensitivity: String,
    pub confidence: f64,
    pub importance: f64,
    pub access_count: u64,
    pub supersedes_id: Option<String>,
    pub status: MemoryStatus,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
    pub last_accessed_at: Option<String>,
    pub expires_at: Option<String>,
}

impl MemoryRecord {
    pub fn new(
        memory_type: MemoryType,
        scope: MemoryScope,
        content: impl Into<String>,
        source_type: MemorySourceType,
    ) -> Result<Self> {
        scope.validate()?;
        let content = content.into();
        if content.trim().is_empty() {
            return Err(NexusError::Config("memory content is required".into()));
        }
        let now = crate::now_rfc3339();
        Ok(Self {
            id: stable_id("hmem"),
            memory_type,
            scope,
            content,
            summary: None,
            tags: Vec::new(),
            source_refs: Vec::new(),
            source_type,
            sensitivity: default_sensitivity(),
            confidence: 1.0,
            importance: 0.5,
            access_count: 0,
            supersedes_id: None,
            status: MemoryStatus::Candidate,
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
            last_accessed_at: None,
            expires_at: None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonaSource {
    BuiltIn,
    UserCreated,
    Imported,
    Generated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonaStatus {
    Active,
    Inactive,
    Archived,
}

impl PersonaStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonaVersion {
    pub persona_id: String,
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub version: u32,
    pub source: PersonaSource,
    pub scope_kind: String,
    pub scope_key: String,
    pub behavioral_tags: Vec<String>,
    pub default_agent_ids: Vec<String>,
    pub status: PersonaStatus,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
}

impl PersonaVersion {
    pub fn first(name: impl Into<String>, system_prompt: impl Into<String>) -> Result<Self> {
        let name = name.into();
        let system_prompt = system_prompt.into();
        if name.trim().is_empty() || system_prompt.trim().is_empty() {
            return Err(NexusError::Config(
                "persona name and system prompt are required".into(),
            ));
        }
        let now = crate::now_rfc3339();
        Ok(Self {
            persona_id: stable_id("persona"),
            name,
            description: String::new(),
            system_prompt,
            version: 1,
            source: PersonaSource::UserCreated,
            scope_kind: "global".into(),
            scope_key: String::new(),
            behavioral_tags: Vec::new(),
            default_agent_ids: Vec::new(),
            status: PersonaStatus::Inactive,
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonaAssignment {
    pub id: String,
    pub persona_id: String,
    pub persona_version: u32,
    pub target_kind: String,
    pub target_id: String,
    pub status: String,
    pub precedence: i32,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyLevel {
    Advisory,
    Supervised,
    Bounded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub capabilities: Vec<String>,
    pub supported_task_types: Vec<String>,
    pub preferred_tools: Vec<String>,
    pub restricted_tools: Vec<String>,
    pub required_model_capabilities: Vec<String>,
    pub preferred_model_capabilities: Vec<String>,
    pub context_policy_id: String,
    pub memory_policy_id: String,
    pub default_persona_id: Option<String>,
    pub autonomy_level: AutonomyLevel,
    pub can_plan: bool,
    pub can_delegate: bool,
    pub can_review: bool,
    pub can_modify_files: bool,
    pub can_run_commands: bool,
    pub can_access_network: bool,
    pub can_request_approval: bool,
    pub status: String,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
}

impl AgentDefinition {
    pub fn advisory(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(NexusError::Config("agent name is required".into()));
        }
        let now = crate::now_rfc3339();
        Ok(Self {
            id: stable_id("agentdef"),
            name,
            description: String::new(),
            capabilities: Vec::new(),
            supported_task_types: Vec::new(),
            preferred_tools: Vec::new(),
            restricted_tools: Vec::new(),
            required_model_capabilities: Vec::new(),
            preferred_model_capabilities: Vec::new(),
            context_policy_id: "default".into(),
            memory_policy_id: "default".into(),
            default_persona_id: None,
            autonomy_level: AutonomyLevel::Advisory,
            can_plan: false,
            can_delegate: false,
            can_review: false,
            can_modify_files: false,
            can_run_commands: false,
            can_access_network: false,
            can_request_approval: true,
            status: "active".into(),
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Draft,
    Defined,
    Planning,
    Active,
    Blocked,
    Paused,
    Validating,
    Completed,
    Failed,
    Cancelled,
    Archived,
}

impl GoalStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Defined => "defined",
            Self::Planning => "planning",
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::Paused => "paused",
            Self::Validating => "validating",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceReference {
    pub criterion: String,
    pub summary: String,
    pub source_ref: String,
    pub passed: bool,
    pub observed_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub objective: String,
    pub success_criteria: Vec<String>,
    pub constraints: Vec<String>,
    pub scope: Vec<String>,
    pub owner_profile_id: Option<String>,
    pub workspace_id: String,
    pub project_id: Option<String>,
    pub selected_agent_id: Option<String>,
    pub selected_persona_id: Option<String>,
    pub priority: i32,
    pub status: GoalStatus,
    pub active_plan_id: Option<String>,
    pub active_plan_version: Option<u32>,
    pub risks: Vec<String>,
    pub checkpoint_ids: Vec<String>,
    pub validation_evidence: Vec<EvidenceReference>,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
}

impl Goal {
    pub fn new(objective: impl Into<String>, workspace_id: impl Into<String>) -> Result<Self> {
        let objective = objective.into();
        if objective.trim().is_empty() {
            return Err(NexusError::Config("goal objective is required".into()));
        }
        let now = crate::now_rfc3339();
        Ok(Self {
            id: stable_id("hgoal"),
            objective,
            success_criteria: Vec::new(),
            constraints: Vec::new(),
            scope: Vec::new(),
            owner_profile_id: None,
            workspace_id: workspace_id.into(),
            project_id: None,
            selected_agent_id: None,
            selected_persona_id: None,
            priority: 0,
            status: GoalStatus::Draft,
            active_plan_id: None,
            active_plan_version: None,
            risks: Vec::new(),
            checkpoint_ids: Vec::new(),
            validation_evidence: Vec::new(),
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn completion_evidenced(&self) -> bool {
        !self.success_criteria.is_empty()
            && self.success_criteria.iter().all(|criterion| {
                self.validation_evidence
                    .iter()
                    .any(|evidence| evidence.criterion == *criterion && evidence.passed)
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanAssumption {
    pub id: String,
    pub statement: String,
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanPhase {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub milestones: Vec<String>,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanDecision {
    pub id: String,
    pub question: String,
    pub decision: Option<String>,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanAlternative {
    pub id: String,
    pub description: String,
    pub tradeoffs: Vec<String>,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRisk {
    pub id: String,
    pub description: String,
    pub likelihood: String,
    pub impact: String,
    pub mitigation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationGate {
    pub id: String,
    pub description: String,
    pub required_evidence: Vec<String>,
    pub passed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Draft,
    Analyzing,
    Proposed,
    UnderReview,
    Approved,
    Executing,
    NeedsRevision,
    Superseded,
    Completed,
    Cancelled,
}

impl PlanStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Analyzing => "analyzing",
            Self::Proposed => "proposed",
            Self::UnderReview => "under_review",
            Self::Approved => "approved",
            Self::Executing => "executing",
            Self::NeedsRevision => "needs_revision",
            Self::Superseded => "superseded",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub version: u32,
    pub goal_id: String,
    pub title: String,
    pub summary: String,
    pub status: PlanStatus,
    pub assumptions: Vec<PlanAssumption>,
    pub constraints: Vec<String>,
    pub phases: Vec<PlanPhase>,
    pub decisions: Vec<PlanDecision>,
    pub alternatives: Vec<PlanAlternative>,
    pub dependencies: Vec<String>,
    pub risks: Vec<PlanRisk>,
    pub validation_gates: Vec<ValidationGate>,
    pub rollback_strategy: Option<String>,
    pub task_ids: Vec<String>,
    pub assigned_agent_ids: Vec<String>,
    pub proposed_subagent_ids: Vec<String>,
    pub provider_requirements: Vec<String>,
    pub model_requirements: Vec<String>,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
    pub approved_at: Option<String>,
}

impl Plan {
    pub fn new(goal_id: impl Into<String>, title: impl Into<String>) -> Result<Self> {
        let title = title.into();
        if title.trim().is_empty() {
            return Err(NexusError::Config("plan title is required".into()));
        }
        let now = crate::now_rfc3339();
        Ok(Self {
            id: stable_id("hplan"),
            version: 1,
            goal_id: goal_id.into(),
            title,
            summary: String::new(),
            status: PlanStatus::Draft,
            assumptions: Vec::new(),
            constraints: Vec::new(),
            phases: Vec::new(),
            decisions: Vec::new(),
            alternatives: Vec::new(),
            dependencies: Vec::new(),
            risks: Vec::new(),
            validation_gates: Vec::new(),
            rollback_strategy: None,
            task_ids: Vec::new(),
            assigned_agent_ids: Vec::new(),
            proposed_subagent_ids: Vec::new(),
            provider_requirements: Vec::new(),
            model_requirements: Vec::new(),
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
            approved_at: None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Draft,
    Pending,
    Ready,
    Running,
    Blocked,
    Waiting,
    Paused,
    Validating,
    Completed,
    Failed,
    Cancelled,
    Superseded,
}

impl TaskStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Waiting => "waiting",
            Self::Paused => "paused",
            Self::Validating => "validating",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Superseded => "superseded",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub goal_id: Option<String>,
    pub plan_id: Option<String>,
    pub plan_version: Option<u32>,
    pub phase_id: Option<String>,
    pub parent_task_id: Option<String>,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    pub priority: i32,
    pub dependencies: Vec<String>,
    pub assigned_agent_id: Option<String>,
    pub assigned_subagent_id: Option<String>,
    pub assigned_model_ref: Option<String>,
    pub allowed_tools: Vec<String>,
    pub restricted_tools: Vec<String>,
    pub inputs: Vec<Value>,
    pub expected_outputs: Vec<Value>,
    pub acceptance_criteria: Vec<String>,
    pub attempt_count: u32,
    pub max_attempts: u32,
    pub timeout_ms: Option<u64>,
    pub checkpoint_refs: Vec<String>,
    pub artifact_refs: Vec<String>,
    pub validation_evidence: Vec<EvidenceReference>,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
}

impl Task {
    pub fn new(title: impl Into<String>, description: impl Into<String>) -> Result<Self> {
        let title = title.into();
        if title.trim().is_empty() {
            return Err(NexusError::Config("task title is required".into()));
        }
        let now = crate::now_rfc3339();
        Ok(Self {
            id: stable_id("htask"),
            goal_id: None,
            plan_id: None,
            plan_version: None,
            phase_id: None,
            parent_task_id: None,
            title,
            description: description.into(),
            status: TaskStatus::Draft,
            priority: 0,
            dependencies: Vec::new(),
            assigned_agent_id: None,
            assigned_subagent_id: None,
            assigned_model_ref: None,
            allowed_tools: Vec::new(),
            restricted_tools: Vec::new(),
            inputs: Vec::new(),
            expected_outputs: Vec::new(),
            acceptance_criteria: Vec::new(),
            attempt_count: 0,
            max_attempts: 3,
            timeout_ms: None,
            checkpoint_refs: Vec::new(),
            artifact_refs: Vec::new(),
            validation_evidence: Vec::new(),
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEdge {
    pub plan_id: String,
    pub plan_version: u32,
    pub from_task_id: String,
    pub to_task_id: String,
    pub relation: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Draft,
    Configured,
    Queued,
    Running,
    Waiting,
    Paused,
    Completed,
    UnderReview,
    Accepted,
    Rejected,
    Failed,
    Cancelled,
}

impl SubagentStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Configured => "configured",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::UnderReview => "under_review",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentSpec {
    pub id: String,
    pub role: String,
    pub assignment: String,
    pub context_scope: MemoryScope,
    pub allowed_tools: Vec<String>,
    pub restricted_tools: Vec<String>,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub token_budget: u64,
    pub cost_budget_micros: u64,
    pub time_limit_ms: u64,
    pub retry_limit: u32,
    pub output_contract: String,
    pub parent_goal_id: Option<String>,
    pub parent_plan_id: Option<String>,
    pub parent_plan_version: Option<u32>,
    pub parent_task_id: Option<String>,
    pub parent_agent_id: String,
    pub memory_access_policy: String,
    pub recursion_depth: u32,
    pub status: SubagentStatus,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
}

impl SubagentSpec {
    fn assignment_fingerprint(&self) -> String {
        digest(&format!(
            "{}\u{0}{}\u{0}{}",
            self.parent_agent_id,
            self.role,
            self.assignment.trim()
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopLimits {
    pub max_iterations: u32,
    pub max_model_calls: u32,
    pub max_tool_calls: u32,
    pub max_retries: u32,
    pub max_tokens: u64,
    pub max_cost_micros: u64,
    pub max_runtime_ms: u64,
    pub max_failures: u32,
    pub max_recursion_depth: u32,
    pub max_subagents: u32,
    pub max_concurrency: u32,
    pub max_memory_writes: u32,
    pub no_progress_limit: u32,
}

impl Default for LoopLimits {
    fn default() -> Self {
        Self {
            max_iterations: 24,
            max_model_calls: 24,
            max_tool_calls: 64,
            max_retries: 3,
            max_tokens: 128_000,
            max_cost_micros: 0,
            max_runtime_ms: 30 * 60 * 1_000,
            max_failures: 5,
            max_recursion_depth: 2,
            max_subagents: 8,
            max_concurrency: 4,
            max_memory_writes: 16,
            no_progress_limit: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopStatus {
    Understanding,
    ResolvingIdentity,
    CompilingContext,
    Planning,
    Acting,
    Observing,
    Evaluating,
    Repairing,
    Waiting,
    Validating,
    Persisting,
    Completed,
    Failed,
    Cancelled,
}

impl LoopStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Understanding => "understanding",
            Self::ResolvingIdentity => "resolving_identity",
            Self::CompilingContext => "compiling_context",
            Self::Planning => "planning",
            Self::Acting => "acting",
            Self::Observing => "observing",
            Self::Evaluating => "evaluating",
            Self::Repairing => "repairing",
            Self::Waiting => "waiting",
            Self::Validating => "validating",
            Self::Persisting => "persisting",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopStopReason {
    IterationLimit,
    ModelCallLimit,
    ToolCallLimit,
    RetryLimit,
    TokenBudget,
    CostBudget,
    TimeBudget,
    FailureBudget,
    RecursionLimit,
    SubagentLimit,
    MemoryWriteLimit,
    NoProgress,
    ApprovalRequired,
    Cancelled,
    AcceptanceCriteriaSatisfied,
    PlanRevisionRequired,
    RequiredCapabilityUnavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopState {
    pub run_id: String,
    pub session_id: String,
    pub profile_id: Option<String>,
    pub goal_id: Option<String>,
    pub plan_id: Option<String>,
    pub plan_version: Option<u32>,
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    pub iteration: u32,
    pub model_call_count: u32,
    pub tool_call_count: u32,
    pub retry_count: u32,
    pub token_count: u64,
    pub cost_micros: u64,
    pub failure_count: u32,
    pub recursion_depth: u32,
    pub subagent_count: u32,
    pub memory_write_count: u32,
    pub no_progress_count: u32,
    pub progress_fingerprint: Option<String>,
    pub limits: LoopLimits,
    pub started_at_ms: i64,
    pub deadline_ms: Option<i64>,
    pub status: LoopStatus,
    pub stop_reason: Option<LoopStopReason>,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
}

impl LoopState {
    pub fn new(session_id: impl Into<String>, limits: LoopLimits) -> Self {
        let now = crate::now_rfc3339();
        Self {
            run_id: stable_id("run"),
            session_id: session_id.into(),
            profile_id: None,
            goal_id: None,
            plan_id: None,
            plan_version: None,
            task_id: None,
            agent_id: None,
            iteration: 0,
            model_call_count: 0,
            tool_call_count: 0,
            retry_count: 0,
            token_count: 0,
            cost_micros: 0,
            failure_count: 0,
            recursion_depth: 0,
            subagent_count: 0,
            memory_write_count: 0,
            no_progress_count: 0,
            progress_fingerprint: None,
            limits,
            started_at_ms: crate::now_ms(),
            deadline_ms: None,
            status: LoopStatus::Understanding,
            stop_reason: None,
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    pub fn observe_progress(&mut self, fingerprint: impl Into<String>) -> bool {
        let fingerprint = fingerprint.into();
        if self.progress_fingerprint.as_deref() == Some(&fingerprint) {
            self.no_progress_count = self.no_progress_count.saturating_add(1);
        } else {
            self.progress_fingerprint = Some(fingerprint);
            self.no_progress_count = 0;
        }
        self.updated_at = crate::now_rfc3339();
        self.no_progress_count >= self.limits.no_progress_limit
    }

    pub fn limit_stop(&self, now_ms: i64) -> Option<LoopStopReason> {
        if self.iteration >= self.limits.max_iterations {
            Some(LoopStopReason::IterationLimit)
        } else if self.model_call_count >= self.limits.max_model_calls {
            Some(LoopStopReason::ModelCallLimit)
        } else if self.tool_call_count >= self.limits.max_tool_calls {
            Some(LoopStopReason::ToolCallLimit)
        } else if self.retry_count >= self.limits.max_retries {
            Some(LoopStopReason::RetryLimit)
        } else if self.token_count >= self.limits.max_tokens {
            Some(LoopStopReason::TokenBudget)
        } else if self.limits.max_cost_micros > 0 && self.cost_micros >= self.limits.max_cost_micros
        {
            Some(LoopStopReason::CostBudget)
        } else if self.deadline_ms.is_some_and(|deadline| now_ms >= deadline)
            || now_ms.saturating_sub(self.started_at_ms) >= self.limits.max_runtime_ms as i64
        {
            Some(LoopStopReason::TimeBudget)
        } else if self.failure_count >= self.limits.max_failures {
            Some(LoopStopReason::FailureBudget)
        } else if self.recursion_depth >= self.limits.max_recursion_depth {
            Some(LoopStopReason::RecursionLimit)
        } else if self.subagent_count >= self.limits.max_subagents {
            Some(LoopStopReason::SubagentLimit)
        } else if self.memory_write_count >= self.limits.max_memory_writes {
            Some(LoopStopReason::MemoryWriteLimit)
        } else if self.no_progress_count >= self.limits.no_progress_limit {
            Some(LoopStopReason::NoProgress)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub session_id: String,
    pub run_id: Option<String>,
    pub active_context: ActiveHarnessContext,
    pub completed_actions: Vec<String>,
    pub pending_approvals: Vec<String>,
    pub memory_refs: Vec<String>,
    pub artifact_refs: Vec<String>,
    pub validation_state: BTreeMap<String, Value>,
    pub subagent_ids: Vec<String>,
    pub failure_state: Option<String>,
    pub environment_fingerprint: String,
    pub file_hashes: BTreeMap<String, String>,
    pub assumptions: BTreeMap<String, String>,
    pub status: String,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
}

impl Checkpoint {
    pub fn new(
        session_id: impl Into<String>,
        active_context: ActiveHarnessContext,
        environment_fingerprint: impl Into<String>,
    ) -> Self {
        let now = crate::now_rfc3339();
        Self {
            id: stable_id("checkpoint"),
            session_id: session_id.into(),
            run_id: None,
            active_context,
            completed_actions: Vec::new(),
            pending_approvals: Vec::new(),
            memory_refs: Vec::new(),
            artifact_refs: Vec::new(),
            validation_state: BTreeMap::new(),
            subagent_ids: Vec::new(),
            failure_state: None,
            environment_fingerprint: environment_fingerprint.into(),
            file_hashes: BTreeMap::new(),
            assumptions: BTreeMap::new(),
            status: "active".into(),
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryAssessment {
    pub checkpoint_id: String,
    pub environment_changed: bool,
    pub changed_files: Vec<String>,
    pub missing_files: Vec<String>,
    pub stale_assumptions: Vec<String>,
    pub provider_available: bool,
    pub model_available: bool,
    pub safe_to_resume_exactly: bool,
    pub recommended_strategy: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImprovementCategory {
    Prompt,
    Persona,
    Routing,
    Context,
    Memory,
    Plan,
    Agent,
    Skill,
    Tool,
    Mcp,
    Provider,
    Workflow,
    Test,
    Documentation,
}

impl ImprovementCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Persona => "persona",
            Self::Routing => "routing",
            Self::Context => "context",
            Self::Memory => "memory",
            Self::Plan => "plan",
            Self::Agent => "agent",
            Self::Skill => "skill",
            Self::Tool => "tool",
            Self::Mcp => "mcp",
            Self::Provider => "provider",
            Self::Workflow => "workflow",
            Self::Test => "test",
            Self::Documentation => "documentation",
        }
    }
}

/// The typed subject of an improvement. `plane()` separates **data-plane**
/// targets (config/data the running harness can self-apply after WARP) from the
/// **code-plane** `HarnessComponent` (Rust source — validated in a worktree but
/// shipped only through a human-approved release, never a live hot-swap).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImprovementTarget {
    Memory,
    Skill,
    Prompt,
    ContextRouter,
    RetrievalPolicy,
    ToolRouter,
    PlannerPolicy,
    AgentRole,
    RetryPolicy,
    ErrorRecovery,
    TimelinePresentation,
    TokenBudgetPolicy,
    EvaluationPolicy,
    /// Rust harness source. Conservative default for un-annotated legacy rows.
    #[default]
    HarnessComponent,
}

/// Whether an improvement changes data the running process can apply, or Rust
/// source that requires a rebuild + human-approved release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImprovementPlane {
    Data,
    Code,
}

impl ImprovementTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Skill => "skill",
            Self::Prompt => "prompt",
            Self::ContextRouter => "context_router",
            Self::RetrievalPolicy => "retrieval_policy",
            Self::ToolRouter => "tool_router",
            Self::PlannerPolicy => "planner_policy",
            Self::AgentRole => "agent_role",
            Self::RetryPolicy => "retry_policy",
            Self::ErrorRecovery => "error_recovery",
            Self::TimelinePresentation => "timeline_presentation",
            Self::TokenBudgetPolicy => "token_budget_policy",
            Self::EvaluationPolicy => "evaluation_policy",
            Self::HarnessComponent => "harness_component",
        }
    }

    /// Code-plane iff the target is Rust harness source; everything else is data.
    pub fn plane(self) -> ImprovementPlane {
        match self {
            Self::HarnessComponent => ImprovementPlane::Code,
            _ => ImprovementPlane::Data,
        }
    }
}

/// Governed risk tier. Higher tiers demand stricter gates; `Prohibited` is
/// auto-rejected by WARP. Defaults to `High` so an un-annotated candidate is
/// treated conservatively (human approval) rather than auto-promotable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RiskTier {
    /// Tier 0 — observation only (telemetry, lessons, reports).
    Observation,
    /// Tier 1 — low risk; auto-promote only after all WARP gates pass.
    Low,
    /// Tier 2 — moderate; shadow required before promotion.
    Moderate,
    /// Tier 3 — high; explicit human approval required.
    #[default]
    High,
    /// Tier 4 — prohibited autonomous change; WARP auto-rejects.
    Prohibited,
}

impl RiskTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::Low => "low",
            Self::Moderate => "moderate",
            Self::High => "high",
            Self::Prohibited => "prohibited",
        }
    }

    /// Numeric tier 0–4.
    pub fn level(self) -> u8 {
        match self {
            Self::Observation => 0,
            Self::Low => 1,
            Self::Moderate => 2,
            Self::High => 3,
            Self::Prohibited => 4,
        }
    }
}

/// A machine-checkable success criterion for a candidate. `hard_constraint`
/// criteria are vetoes: WARP cannot average them away against soft gains.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuccessMetric {
    pub id: String,
    pub description: String,
    /// Measured baseline value, when known.
    pub baseline: Option<f64>,
    /// Target the candidate must reach (direction is described in `description`).
    pub target: Option<f64>,
    /// When true, a miss is a hard failure regardless of other gains.
    pub hard_constraint: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImprovementStatus {
    Observed,
    Draft,
    Proposed,
    Approved,
    NeedsRevision,
    Rejected,
    Testing,
    Validated,
    Shadow,
    Canary,
    Applied,
    Promoted,
    RolledBack,
    Deprecated,
}

impl ImprovementStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Draft => "draft",
            Self::Proposed => "proposed",
            Self::Approved => "approved",
            Self::NeedsRevision => "needs_revision",
            Self::Rejected => "rejected",
            Self::Testing => "testing",
            Self::Validated => "validated",
            Self::Shadow => "shadow",
            Self::Canary => "canary",
            Self::Applied => "applied",
            Self::Promoted => "promoted",
            Self::RolledBack => "rolled_back",
            Self::Deprecated => "deprecated",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImprovementProposal {
    pub id: String,
    pub category: ImprovementCategory,
    pub problem: String,
    pub evidence: Vec<EvidenceReference>,
    pub proposed_change: String,
    pub expected_benefit: String,
    pub risks: Vec<String>,
    pub required_permissions: Vec<String>,
    pub validation_plan: Vec<String>,
    pub rollback_plan: Vec<String>,
    pub status: ImprovementStatus,
    pub approval_required: bool,
    pub measurements: BTreeMap<String, Value>,
    /// Typed subject of the change. Defaults conservatively (`HarnessComponent`)
    /// for rows written before schema v2.
    #[serde(default)]
    pub target: ImprovementTarget,
    /// Where the change applies (workspace/project/session/…).
    #[serde(default)]
    pub scope: MemoryScope,
    /// Governed risk tier. Defaults to `High` (human approval) when absent.
    #[serde(default)]
    pub risk_tier: RiskTier,
    #[serde(default)]
    pub root_cause_hypothesis: String,
    #[serde(default)]
    pub success_metrics: Vec<SuccessMetric>,
    #[serde(default)]
    pub affected_components: Vec<String>,
    #[serde(default)]
    pub baseline_version: String,
    #[serde(default)]
    pub candidate_version: String,
    /// Role/agent that authored the candidate (never the promoting authority).
    #[serde(default)]
    pub created_by: String,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
    pub reviewed_at: Option<String>,
}

impl ImprovementProposal {
    pub fn new(
        category: ImprovementCategory,
        problem: impl Into<String>,
        proposed_change: impl Into<String>,
    ) -> Result<Self> {
        let problem = problem.into();
        let proposed_change = proposed_change.into();
        if problem.trim().is_empty() || proposed_change.trim().is_empty() {
            return Err(NexusError::Config(
                "improvement problem and proposed change are required".into(),
            ));
        }
        let now = crate::now_rfc3339();
        Ok(Self {
            id: stable_id("improvement"),
            category,
            problem,
            evidence: Vec::new(),
            proposed_change,
            expected_benefit: String::new(),
            risks: Vec::new(),
            required_permissions: Vec::new(),
            validation_plan: Vec::new(),
            rollback_plan: Vec::new(),
            status: ImprovementStatus::Draft,
            approval_required: true,
            measurements: BTreeMap::new(),
            target: ImprovementTarget::default(),
            scope: MemoryScope::default(),
            risk_tier: RiskTier::default(),
            root_cause_hypothesis: String::new(),
            success_metrics: Vec::new(),
            affected_components: Vec::new(),
            baseline_version: String::new(),
            candidate_version: String::new(),
            created_by: String::new(),
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
            reviewed_at: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderPrivacyGrant {
    pub id: String,
    pub provider_id: String,
    pub scope: MemoryScope,
    pub privacy_policy_ref: String,
    pub approved_by: String,
    pub status: String,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
    pub revoked_at: Option<String>,
}

impl ProviderPrivacyGrant {
    pub fn new(
        provider_id: impl Into<String>,
        scope: MemoryScope,
        privacy_policy_ref: impl Into<String>,
        approved_by: impl Into<String>,
    ) -> Result<Self> {
        scope.validate()?;
        let now = crate::now_rfc3339();
        Ok(Self {
            id: stable_id("privacy"),
            provider_id: provider_id.into(),
            scope,
            privacy_policy_ref: privacy_policy_ref.into(),
            approved_by: approved_by.into(),
            status: "active".into(),
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
            revoked_at: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelAssignment {
    pub id: String,
    pub provider_id: String,
    pub model_id: String,
    pub target_kind: String,
    pub target_id: String,
    pub required_capabilities: Vec<String>,
    pub fallback_priority: i32,
    pub status: String,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskAttempt {
    pub id: String,
    pub task_id: String,
    pub attempt_number: u32,
    pub status: String,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub observations: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub error_summary: Option<String>,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceAccessMode {
    Read,
    Write,
}

impl ResourceAccessMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceClaim {
    pub id: String,
    pub task_id: String,
    pub resource_kind: String,
    pub resource_key: String,
    pub access_mode: ResourceAccessMode,
    pub status: String,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: Option<String>,
}

impl ResourceClaim {
    pub fn new(
        task_id: &str,
        resource_kind: &str,
        resource_key: &str,
        access_mode: ResourceAccessMode,
        expires_at: Option<String>,
    ) -> Self {
        let now = crate::now_rfc3339();
        Self {
            id: stable_id("claim"),
            task_id: task_id.to_string(),
            resource_kind: resource_kind.to_string(),
            resource_key: resource_key.to_string(),
            access_mode,
            status: "active".to_string(),
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
            expires_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    ApprovedOnce,
    ApprovedForTask,
    Rejected,
    Expired,
    Cancelled,
}

impl ApprovalStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::ApprovedOnce => "approved_once",
            Self::ApprovedForTask => "approved_for_task",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub run_id: Option<String>,
    pub requesting_agent_id: Option<String>,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub action: String,
    pub reason: String,
    pub target: String,
    pub affected_resources: Vec<String>,
    pub risk_class: String,
    pub rollback: String,
    pub grant_scope: String,
    pub status: ApprovalStatus,
    pub decision_note: Option<String>,
    pub schema_version: u32,
    pub created_at: String,
    pub updated_at: String,
    pub resolved_at: Option<String>,
}

impl ApprovalRequest {
    pub fn pending(action: impl Into<String>, risk_class: impl Into<String>) -> Result<Self> {
        let action = action.into();
        if action.trim().is_empty() {
            return Err(NexusError::Config("approval action is required".into()));
        }
        let now = crate::now_rfc3339();
        Ok(Self {
            id: stable_id("approval"),
            session_id: None,
            task_id: None,
            run_id: None,
            requesting_agent_id: None,
            provider_id: None,
            model_id: None,
            action,
            reason: String::new(),
            target: String::new(),
            affected_resources: Vec::new(),
            risk_class: risk_class.into(),
            rollback: String::new(),
            grant_scope: "once".into(),
            status: ApprovalStatus::Pending,
            decision_note: None,
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
            resolved_at: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessEvent {
    pub id: String,
    pub event_type: String,
    pub timestamp: String,
    pub session_id: Option<String>,
    pub profile_id: Option<String>,
    pub goal_id: Option<String>,
    pub plan_id: Option<String>,
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    pub subagent_id: Option<String>,
    pub run_id: Option<String>,
    /// Severity for RSI triage: `info` | `notice` | `warning` | `error`.
    #[serde(default = "default_event_severity")]
    pub severity: String,
    /// Provider/model in effect when the event was observed, when known.
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Improvement candidate this event is evidence for, when applicable.
    #[serde(default)]
    pub candidate_id: Option<String>,
    pub summary: String,
    pub metadata: BTreeMap<String, Value>,
    pub sensitivity: String,
    pub schema_version: u32,
}

fn default_event_severity() -> String {
    "info".to_string()
}

impl HarnessEvent {
    pub fn new(event_type: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            id: stable_id("hevent"),
            event_type: event_type.into(),
            timestamp: crate::now_rfc3339(),
            session_id: None,
            profile_id: None,
            goal_id: None,
            plan_id: None,
            task_id: None,
            agent_id: None,
            subagent_id: None,
            run_id: None,
            severity: default_event_severity(),
            provider: None,
            model: None,
            candidate_id: None,
            summary: summary.into(),
            metadata: BTreeMap::new(),
            sensitivity: "normal".into(),
            schema_version: HARNESS_SCHEMA_VERSION,
        }
    }
}

/// SQLite repository for all adaptive harness records.
#[derive(Debug, Clone)]
pub struct HarnessRepository {
    store: Store,
}

impl HarnessRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn set_active_context(
        &self,
        mut context: ActiveHarnessContext,
    ) -> Result<ActiveHarnessContext> {
        context.updated_at = crate::now_rfc3339();
        self.store.with_retry(|conn| {
            let existing: Option<String> = conn
                .query_row(
                    "SELECT id FROM harness_active_contexts
                     WHERE workspace_id=?1 AND COALESCE(session_id,'')=COALESCE(?2,'')",
                    params![context.workspace_id, context.session_id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(existing) = existing {
                context.id = existing;
            }
            conn.execute(
                "INSERT INTO harness_active_contexts
                 (id,workspace_id,session_id,profile_id,persona_id,persona_version,agent_id,
                  goal_id,plan_id,plan_version,task_id,provider_id,model_id,status,
                  schema_version,payload_json,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)
                 ON CONFLICT(id) DO UPDATE SET
                  profile_id=excluded.profile_id,persona_id=excluded.persona_id,
                  persona_version=excluded.persona_version,agent_id=excluded.agent_id,
                  goal_id=excluded.goal_id,plan_id=excluded.plan_id,
                  plan_version=excluded.plan_version,task_id=excluded.task_id,
                  provider_id=excluded.provider_id,model_id=excluded.model_id,
                  status=excluded.status,schema_version=excluded.schema_version,
                  payload_json=excluded.payload_json,updated_at=excluded.updated_at",
                params![
                    context.id,
                    context.workspace_id,
                    context.session_id,
                    context.profile_id,
                    context.persona_id,
                    context.persona_version,
                    context.agent_id,
                    context.goal_id,
                    context.plan_id,
                    context.plan_version,
                    context.task_id,
                    context.provider_id,
                    context.model_id,
                    context.status,
                    context.schema_version,
                    encode(&context)?,
                    context.created_at,
                    context.updated_at,
                ],
            )?;
            Ok(context.clone())
        })
    }

    pub fn active_context(
        &self,
        workspace_id: &str,
        session_id: Option<&str>,
    ) -> Result<Option<ActiveHarnessContext>> {
        self.store.with(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM harness_active_contexts
                     WHERE workspace_id=?1 AND COALESCE(session_id,'')=COALESCE(?2,'')
                       AND status='active'",
                    params![workspace_id, session_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            payload.map(decode).transpose()
        })
    }

    pub fn create_profile(&self, profile: &UserProfile) -> Result<()> {
        self.store
            .with_retry(|conn| Self::insert_profile_conn(conn, profile))
    }

    /// Persist edits to an existing profile card without replacing its
    /// identity or provenance records. Creation uses `create_profile`; this
    /// method deliberately cannot upsert a missing id.
    pub fn update_profile(&self, profile: &UserProfile) -> Result<UserProfile> {
        if profile.display_name.trim().is_empty() {
            return Err(NexusError::Config(
                "profile display name is required".into(),
            ));
        }
        let mut profile = profile.clone();
        profile.display_name = profile.display_name.trim().to_string();
        profile.updated_at = crate::now_rfc3339();
        let payload = checked_payload(&profile, "profile")?;
        self.store.with_retry(|conn| {
            let changed = conn.execute(
                "UPDATE harness_profiles
                 SET display_name=?1,normalized_name=?2,status=?3,schema_version=?4,
                     payload_json=?5,updated_at=?6,last_seen_at=?7
                 WHERE id=?8",
                params![
                    profile.display_name,
                    normalized_name(&profile.display_name),
                    profile.status.as_str(),
                    profile.schema_version,
                    payload,
                    profile.updated_at,
                    profile.last_seen_at,
                    profile.id,
                ],
            )?;
            if changed != 1 {
                return Err(NexusError::NotFound(format!("profile `{}`", profile.id)));
            }
            Ok(profile.clone())
        })
    }

    /// Soft archive, restore, or delete a profile. Records and provenance stay
    /// durable; the application control plane separately prevents changing an
    /// actively selected card without an explicit switch.
    pub fn set_profile_status(
        &self,
        profile_id: &str,
        status: ProfileStatus,
    ) -> Result<UserProfile> {
        let mut profile = self.profile(profile_id)?;
        if profile.is_default() && status == ProfileStatus::Deleted {
            return Err(NexusError::PolicyDenied(
                "the default profile cannot be deleted".into(),
            ));
        }
        profile.status = status;
        self.update_profile(&profile)
    }

    fn insert_profile_conn(conn: &rusqlite::Connection, profile: &UserProfile) -> Result<()> {
        let payload = checked_payload(profile, "profile")?;
        conn.execute(
            "INSERT INTO harness_profiles
             (id,display_name,normalized_name,status,schema_version,payload_json,
              created_at,updated_at,last_seen_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                profile.id,
                profile.display_name,
                normalized_name(&profile.display_name),
                profile.status.as_str(),
                profile.schema_version,
                payload,
                profile.created_at,
                profile.updated_at,
                profile.last_seen_at,
            ],
        )?;
        Ok(())
    }

    pub fn profile(&self, profile_id: &str) -> Result<UserProfile> {
        self.store.with(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM harness_profiles WHERE id=?1",
                    [profile_id],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|_| NexusError::NotFound(format!("profile `{profile_id}`")))?;
            decode(payload)
        })
    }

    pub fn profiles_named(&self, display_name: &str) -> Result<Vec<UserProfile>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT payload_json FROM harness_profiles
                 WHERE normalized_name=?1 AND status NOT IN ('archived','deleted')
                 ORDER BY updated_at DESC",
            )?;
            let rows = stmt.query_map([normalized_name(display_name)], |row| {
                row.get::<_, String>(0)
            })?;
            let mut profiles = Vec::new();
            for row in rows {
                profiles.push(decode(row?)?);
            }
            Ok(profiles)
        })
    }

    pub fn add_profile_fact(&self, fact: &ProfileFact) -> Result<()> {
        if !(0.0..=1.0).contains(&fact.confidence) {
            return Err(NexusError::Config(
                "profile fact confidence must be between 0 and 1".into(),
            ));
        }
        self.store
            .with_retry(|conn| Self::insert_profile_fact_conn(conn, fact))
    }

    fn insert_profile_fact_conn(conn: &rusqlite::Connection, fact: &ProfileFact) -> Result<()> {
        if !conn
            .prepare("SELECT 1 FROM harness_profiles WHERE id=?1 AND status!='deleted'")?
            .exists([&fact.profile_id])?
        {
            return Err(NexusError::NotFound(format!(
                "profile `{}`",
                fact.profile_id
            )));
        }
        let payload = checked_payload(fact, "profile fact")?;
        conn.execute(
            "INSERT INTO harness_profile_facts
             (id,profile_id,fact_key,status,source_type,confidence,sensitivity,
              schema_version,payload_json,created_at,updated_at,expires_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                fact.id,
                fact.profile_id,
                fact.key,
                fact.status.as_str(),
                fact.source_type.as_str(),
                fact.confidence,
                fact.sensitivity,
                fact.schema_version,
                payload,
                fact.created_at,
                fact.updated_at,
                fact.expires_at,
            ],
        )?;
        Ok(())
    }

    pub fn profile_facts(
        &self,
        profile_id: &str,
        include_candidates: bool,
    ) -> Result<Vec<ProfileFact>> {
        self.store.with(|conn| {
            let sql = if include_candidates {
                "SELECT payload_json FROM harness_profile_facts
                 WHERE profile_id=?1 AND status NOT IN ('deleted','rejected') ORDER BY created_at"
            } else {
                "SELECT payload_json FROM harness_profile_facts
                 WHERE profile_id=?1 AND status='active' ORDER BY created_at"
            };
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([profile_id], |row| row.get::<_, String>(0))?;
            let mut facts = Vec::new();
            for row in rows {
                facts.push(decode(row?)?);
            }
            Ok(facts)
        })
    }

    /// Change one fact's lifecycle state after proving that it belongs to the
    /// reviewed profile. The ownership predicate is part of both the read and
    /// the update, preventing a stale UI action from mutating another
    /// profile's fact by id alone.
    pub fn set_profile_fact_status(
        &self,
        profile_id: &str,
        fact_id: &str,
        status: ProfileFactStatus,
    ) -> Result<ProfileFact> {
        self.store.with_retry(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<ProfileFact> {
                let payload = conn
                    .query_row(
                        "SELECT payload_json FROM harness_profile_facts
                         WHERE id=?1 AND profile_id=?2",
                        params![fact_id, profile_id],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(|_| {
                        NexusError::NotFound(format!(
                            "profile fact `{fact_id}` for profile `{profile_id}`"
                        ))
                    })?;
                let mut fact: ProfileFact = decode(payload)?;
                fact.status = status;
                fact.updated_at = crate::now_rfc3339();
                let changed = conn.execute(
                    "UPDATE harness_profile_facts
                     SET status=?1,payload_json=?2,updated_at=?3
                     WHERE id=?4 AND profile_id=?5",
                    params![
                        fact.status.as_str(),
                        checked_payload(&fact, "profile fact")?,
                        fact.updated_at,
                        fact_id,
                        profile_id,
                    ],
                )?;
                if changed != 1 {
                    return Err(NexusError::NotFound(format!(
                        "profile fact `{fact_id}` for profile `{profile_id}`"
                    )));
                }
                Ok(fact)
            })();
            finish_transaction(conn, result)
        })
    }

    /// Resolve a high-confidence explicit name without ever overwriting a
    /// different active profile. Creation/fact/conflict writes are atomic.
    pub fn resolve_explicit_identity(
        &self,
        active_profile_id: Option<&str>,
        asserted_name: &str,
        source_ref: Option<&str>,
    ) -> Result<IdentityResolution> {
        if asserted_name.trim().is_empty() {
            return Err(NexusError::Config("asserted name is required".into()));
        }
        self.store.with_retry(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<IdentityResolution> {
                let active = active_profile_id
                    .map(|id| {
                        conn.query_row(
                            "SELECT payload_json FROM harness_profiles WHERE id=?1",
                            [id],
                            |row| row.get::<_, String>(0),
                        )
                        .map_err(|_| NexusError::NotFound(format!("profile `{id}`")))
                        .and_then(decode::<UserProfile>)
                    })
                    .transpose()?;
                let mut stmt = conn.prepare(
                    "SELECT payload_json FROM harness_profiles
                     WHERE normalized_name=?1 AND status NOT IN ('archived','deleted')
                     ORDER BY updated_at DESC",
                )?;
                let rows = stmt.query_map([normalized_name(asserted_name)], |row| {
                    row.get::<_, String>(0)
                })?;
                let mut matches = Vec::<UserProfile>::new();
                for row in rows {
                    matches.push(decode(row?)?);
                }

                if let Some(active) = &active {
                    if normalized_name(&active.display_name) == normalized_name(asserted_name) {
                        let mut fact =
                            ProfileFact::explicit_name(active.id.clone(), asserted_name.trim());
                        fact.source_ref = source_ref.map(str::to_string);
                        Self::insert_profile_fact_conn(conn, &fact)?;
                        return Ok(IdentityResolution::Activated(active.clone()));
                    }
                    if !active.is_default() {
                        let conflict = Self::insert_identity_conflict_conn(
                            conn,
                            Some(active.id.clone()),
                            matches.first().map(|profile| profile.id.clone()),
                            asserted_name,
                            matches.iter().map(|profile| profile.id.clone()).collect(),
                            source_ref,
                        )?;
                        return Ok(IdentityResolution::Conflict(conflict));
                    }
                }

                match matches.as_slice() {
                    [profile] => {
                        let mut fact =
                            ProfileFact::explicit_name(profile.id.clone(), asserted_name.trim());
                        fact.source_ref = source_ref.map(str::to_string);
                        Self::insert_profile_fact_conn(conn, &fact)?;
                        Ok(IdentityResolution::Activated(profile.clone()))
                    }
                    [] => {
                        let profile = UserProfile::new(asserted_name.trim())?;
                        Self::insert_profile_conn(conn, &profile)?;
                        let mut fact =
                            ProfileFact::explicit_name(profile.id.clone(), asserted_name.trim());
                        fact.source_ref = source_ref.map(str::to_string);
                        Self::insert_profile_fact_conn(conn, &fact)?;
                        Ok(IdentityResolution::Created(profile))
                    }
                    _ => Ok(IdentityResolution::Conflict(
                        Self::insert_identity_conflict_conn(
                            conn,
                            active.map(|profile| profile.id),
                            None,
                            asserted_name,
                            matches.iter().map(|profile| profile.id.clone()).collect(),
                            source_ref,
                        )?,
                    )),
                }
            })();
            finish_transaction(conn, result)
        })
    }

    /// Resolve a queued identity conflict transactionally. The selected
    /// profile is returned to the application control plane, which owns the
    /// active-context switch. This method never merges or overwrites cards.
    pub fn resolve_identity_conflict(
        &self,
        conflict_id: &str,
        decision: IdentityConflictDecision,
    ) -> Result<IdentityConflictResolution> {
        self.store.with_retry(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<IdentityConflictResolution> {
                let payload = conn
                    .query_row(
                        "SELECT payload_json FROM harness_identity_conflicts WHERE id=?1",
                        [conflict_id],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(|_| {
                        NexusError::NotFound(format!("identity conflict `{conflict_id}`"))
                    })?;
                let mut conflict: IdentityConflict = decode(payload)?;
                if conflict.status != IdentityConflictStatus::Pending {
                    return Err(NexusError::Config(format!(
                        "identity conflict `{conflict_id}` is already resolved"
                    )));
                }

                let (resolution, status, selected_profile) = match decision.clone() {
                    IdentityConflictDecision::SwitchExisting(profile_id) => {
                        if !conflict
                            .matching_profile_ids
                            .iter()
                            .any(|candidate| candidate == &profile_id)
                            && conflict.candidate_profile_id.as_deref() != Some(profile_id.as_str())
                        {
                            return Err(NexusError::PolicyDenied(
                                "selected profile is not one of the reviewed identity matches"
                                    .into(),
                            ));
                        }
                        let profile: UserProfile = conn
                            .query_row(
                                "SELECT payload_json FROM harness_profiles WHERE id=?1",
                                [&profile_id],
                                |row| row.get::<_, String>(0),
                            )
                            .map_err(|_| NexusError::NotFound(format!("profile `{profile_id}`")))
                            .and_then(decode)?;
                        (
                            "switch_existing".to_string(),
                            IdentityConflictStatus::Resolved,
                            Some(profile),
                        )
                    }
                    IdentityConflictDecision::CreateSeparate => {
                        let profile = UserProfile::new(&conflict.asserted_name)?;
                        Self::insert_profile_conn(conn, &profile)?;
                        let mut fact = ProfileFact::explicit_name(
                            profile.id.clone(),
                            conflict.asserted_name.clone(),
                        );
                        fact.source_ref.clone_from(&conflict.source_ref);
                        Self::insert_profile_fact_conn(conn, &fact)?;
                        (
                            "create_separate".to_string(),
                            IdentityConflictStatus::Resolved,
                            Some(profile),
                        )
                    }
                    IdentityConflictDecision::KeepActive => (
                        "keep_active".to_string(),
                        IdentityConflictStatus::Resolved,
                        conflict
                            .active_profile_id
                            .as_deref()
                            .map(|profile_id| {
                                conn.query_row(
                                    "SELECT payload_json FROM harness_profiles WHERE id=?1",
                                    [profile_id],
                                    |row| row.get::<_, String>(0),
                                )
                                .map_err(|_| {
                                    NexusError::NotFound(format!("profile `{profile_id}`"))
                                })
                                .and_then(decode)
                            })
                            .transpose()?,
                    ),
                    IdentityConflictDecision::TemporaryContext => (
                        "temporary_context".to_string(),
                        IdentityConflictStatus::Resolved,
                        None,
                    ),
                    IdentityConflictDecision::Dismiss => (
                        "dismissed".to_string(),
                        IdentityConflictStatus::Dismissed,
                        None,
                    ),
                };
                let now = crate::now_rfc3339();
                conflict.status = status;
                conflict.resolution = Some(resolution);
                conflict.updated_at = now.clone();
                conflict.resolved_at = Some(now.clone());
                conn.execute(
                    "UPDATE harness_identity_conflicts
                     SET status=?1,payload_json=?2,updated_at=?3,resolved_at=?3 WHERE id=?4",
                    params![
                        conflict.status.as_str(),
                        checked_payload(&conflict, "identity conflict")?,
                        now,
                        conflict_id
                    ],
                )?;
                Ok(IdentityConflictResolution {
                    conflict,
                    selected_profile,
                })
            })();
            finish_transaction(conn, result)
        })
    }

    fn insert_identity_conflict_conn(
        conn: &rusqlite::Connection,
        active_profile_id: Option<String>,
        candidate_profile_id: Option<String>,
        asserted_name: &str,
        matching_profile_ids: Vec<String>,
        source_ref: Option<&str>,
    ) -> Result<IdentityConflict> {
        let now = crate::now_rfc3339();
        let conflict = IdentityConflict {
            id: stable_id("identity_conflict"),
            active_profile_id,
            candidate_profile_id,
            asserted_name: asserted_name.trim().to_string(),
            matching_profile_ids,
            source_ref: source_ref.map(str::to_string),
            status: IdentityConflictStatus::Pending,
            resolution: None,
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
            resolved_at: None,
        };
        conn.execute(
            "INSERT INTO harness_identity_conflicts
             (id,active_profile_id,candidate_profile_id,status,schema_version,payload_json,
              created_at,updated_at,resolved_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                conflict.id,
                conflict.active_profile_id,
                conflict.candidate_profile_id,
                conflict.status.as_str(),
                conflict.schema_version,
                encode(&conflict)?,
                conflict.created_at,
                conflict.updated_at,
                conflict.resolved_at,
            ],
        )?;
        Ok(conflict)
    }

    pub fn save_memory(&self, memory: &MemoryRecord) -> Result<String> {
        memory.scope.validate()?;
        if memory.scope.global {
            let sensitivity = memory
                .sensitivity
                .trim()
                .to_ascii_lowercase()
                .replace(['-', ' '], "_");
            if !matches!(sensitivity.as_str(), "public" | "non_sensitive" | "system") {
                return Err(NexusError::PolicyDenied(
                    "global memory requires an explicit public, non_sensitive, or system classification"
                        .into(),
                ));
            }
        }
        if !(0.0..=1.0).contains(&memory.confidence) || !(0.0..=1.0).contains(&memory.importance) {
            return Err(NexusError::Config(
                "memory confidence and importance must be between 0 and 1".into(),
            ));
        }
        let scope_fingerprint = memory.scope.fingerprint()?;
        let content_hash = digest(&memory.content);
        let (scope_kind, scope_key) = memory.scope.primary();
        let payload = checked_payload(memory, "memory")?;
        self.store.with_retry(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<String> {
                let existing: Option<String> = conn
                    .query_row(
                        "SELECT id FROM harness_memories
                         WHERE scope_fingerprint=?1 AND memory_type=?2 AND content_hash=?3
                           AND status NOT IN ('deleted','rejected')",
                        params![scope_fingerprint, memory.memory_type.as_str(), content_hash],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(existing) = existing {
                    return Ok(existing);
                }
                conn.execute(
                    "INSERT INTO harness_memories
                     (id,memory_type,scope_fingerprint,scope_kind,scope_key,profile_id,
                      workspace_id,project_id,session_id,goal_id,plan_id,task_id,agent_id,
                      status,source_type,confidence,importance,content,content_hash,
                      schema_version,payload_json,created_at,updated_at,last_accessed_at,expires_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,
                             ?16,?17,?18,?19,?20,?21,?22,?23,?24,?25)",
                    params![
                        memory.id,
                        memory.memory_type.as_str(),
                        scope_fingerprint,
                        scope_kind,
                        scope_key,
                        memory.scope.profile_id,
                        memory.scope.workspace_id,
                        memory.scope.project_id,
                        memory.scope.session_id,
                        memory.scope.goal_id,
                        memory.scope.plan_id,
                        memory.scope.task_id,
                        memory.scope.agent_id,
                        memory.status.as_str(),
                        memory.source_type.as_str(),
                        memory.confidence,
                        memory.importance,
                        memory.content,
                        content_hash,
                        memory.schema_version,
                        payload,
                        memory.created_at,
                        memory.updated_at,
                        memory.last_accessed_at,
                        memory.expires_at,
                    ],
                )?;
                Ok(memory.id.clone())
            })();
            finish_transaction(conn, result)
        })
    }

    /// Retrieve only records whose exact scope is explicitly authorized.
    /// Content matching is applied inside each already-scoped indexed query.
    pub fn query_memories(
        &self,
        allowed_scopes: &[MemoryScope],
        query: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>> {
        if allowed_scopes.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut fingerprints = Vec::with_capacity(allowed_scopes.len());
        for scope in allowed_scopes {
            fingerprints.push(scope.fingerprint()?);
        }
        self.store.with_retry(|conn| {
            let mut records = Vec::new();
            let mut seen = HashSet::new();
            let pattern = format!("%{}%", query.unwrap_or_default().replace('%', "\\%"));
            for fingerprint in &fingerprints {
                if records.len() >= limit {
                    break;
                }
                let remaining = limit.saturating_sub(records.len()) as i64;
                let mut stmt = conn.prepare(
                    "SELECT payload_json FROM harness_memories
                     WHERE scope_fingerprint=?1 AND status='active'
                       AND (expires_at IS NULL OR expires_at>?2)
                       AND (?3='' OR content LIKE ?4 ESCAPE '\\')
                     ORDER BY importance DESC, updated_at DESC LIMIT ?5",
                )?;
                let rows = stmt.query_map(
                    params![
                        fingerprint,
                        crate::now_rfc3339(),
                        query.unwrap_or_default(),
                        pattern,
                        remaining
                    ],
                    |row| row.get::<_, String>(0),
                )?;
                for row in rows {
                    let memory: MemoryRecord = decode(row?)?;
                    if seen.insert(memory.id.clone()) {
                        records.push(memory);
                    }
                }
            }
            let accessed_at = crate::now_rfc3339();
            for memory in &mut records {
                memory.access_count = memory.access_count.saturating_add(1);
                memory.last_accessed_at = Some(accessed_at.clone());
                let payload = encode(memory)?;
                conn.execute(
                    "UPDATE harness_memories
                     SET payload_json=?1,last_accessed_at=?2,updated_at=?2 WHERE id=?3",
                    params![payload, accessed_at, memory.id],
                )?;
            }
            Ok(records)
        })
    }

    /// Bounded dashboard listing for explicitly authorized scopes. Unlike
    /// retrieval, this does not change access counters and may include
    /// candidate, superseded, rejected, or archived records for review.
    pub fn list_memories(
        &self,
        allowed_scopes: &[MemoryScope],
        include_inactive: bool,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>> {
        if allowed_scopes.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let fingerprints = allowed_scopes
            .iter()
            .map(MemoryScope::fingerprint)
            .collect::<Result<Vec<_>>>()?;
        self.store.with(|conn| {
            let mut records = Vec::new();
            let mut seen = HashSet::new();
            for fingerprint in &fingerprints {
                if records.len() >= limit {
                    break;
                }
                let mut stmt = conn.prepare(
                    "SELECT payload_json FROM harness_memories
                     WHERE scope_fingerprint=?1
                       AND ((?2=1 AND status!='deleted') OR
                            (?2=0 AND status='active'
                             AND (expires_at IS NULL OR expires_at>?3)))
                     ORDER BY importance DESC,updated_at DESC LIMIT ?4",
                )?;
                let rows = stmt.query_map(
                    params![
                        fingerprint,
                        include_inactive,
                        crate::now_rfc3339(),
                        limit.saturating_sub(records.len()) as i64
                    ],
                    |row| row.get::<_, String>(0),
                )?;
                for row in rows {
                    let memory: MemoryRecord = decode(row?)?;
                    if seen.insert(memory.id.clone()) {
                        records.push(memory);
                    }
                }
            }
            Ok(records)
        })
    }

    pub fn memory_in_scopes(
        &self,
        memory_id: &str,
        allowed_scopes: &[MemoryScope],
    ) -> Result<MemoryRecord> {
        let allowed: BTreeSet<String> = allowed_scopes
            .iter()
            .map(MemoryScope::fingerprint)
            .collect::<Result<_>>()?;
        self.store.with(|conn| {
            let (scope, payload) = conn
                .query_row(
                    "SELECT scope_fingerprint,payload_json FROM harness_memories WHERE id=?1",
                    [memory_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(|_| NexusError::NotFound(format!("memory `{memory_id}`")))?;
            if !allowed.contains(&scope) {
                return Err(NexusError::PolicyDenied(format!(
                    "memory `{memory_id}` is outside the authorized scope"
                )));
            }
            decode(payload)
        })
    }

    pub fn set_memory_status(&self, memory_id: &str, status: MemoryStatus) -> Result<()> {
        self.store.with_retry(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM harness_memories WHERE id=?1",
                    [memory_id],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|_| NexusError::NotFound(format!("memory `{memory_id}`")))?;
            let mut memory: MemoryRecord = decode(payload)?;
            memory.status = status;
            memory.updated_at = crate::now_rfc3339();
            conn.execute(
                "UPDATE harness_memories SET status=?1,payload_json=?2,updated_at=?3 WHERE id=?4",
                params![
                    status.as_str(),
                    encode(&memory)?,
                    memory.updated_at,
                    memory_id
                ],
            )?;
            Ok(())
        })
    }

    pub fn save_persona_version(&self, persona: &PersonaVersion) -> Result<()> {
        if persona.version == 0
            || persona.name.trim().is_empty()
            || persona.system_prompt.trim().is_empty()
        {
            return Err(NexusError::Config(
                "persona name, positive version, and system prompt are required".into(),
            ));
        }
        let payload = checked_payload(persona, "persona")?;
        self.store.with_retry(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<()> {
                let latest: Option<u32> = conn.query_row(
                    "SELECT MAX(version) FROM harness_persona_versions WHERE persona_id=?1",
                    [&persona.persona_id],
                    |row| row.get::<_, Option<u32>>(0),
                )?;
                let expected = latest.map_or(1, |version| version.saturating_add(1));
                if persona.version != expected {
                    return Err(NexusError::Config(format!(
                        "persona version must be {expected}, got {}",
                        persona.version
                    )));
                }
                conn.execute(
                    "INSERT INTO harness_persona_versions
                     (persona_id,version,name,scope_kind,scope_key,status,schema_version,
                      payload_json,created_at,updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        persona.persona_id,
                        persona.version,
                        persona.name,
                        persona.scope_kind,
                        persona.scope_key,
                        persona.status.as_str(),
                        persona.schema_version,
                        payload,
                        persona.created_at,
                        persona.updated_at,
                    ],
                )?;
                Ok(())
            })();
            finish_transaction(conn, result)
        })
    }

    pub fn assign_persona(&self, assignment: &PersonaAssignment) -> Result<()> {
        self.store.with_retry(|conn| {
            conn.execute(
                "INSERT INTO harness_persona_assignments
                 (id,persona_id,persona_version,target_kind,target_id,status,precedence,
                  schema_version,payload_json,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![
                    assignment.id,
                    assignment.persona_id,
                    assignment.persona_version,
                    assignment.target_kind,
                    assignment.target_id,
                    assignment.status,
                    assignment.precedence,
                    assignment.schema_version,
                    encode(assignment)?,
                    assignment.created_at,
                    assignment.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn save_agent_definition(&self, agent: &AgentDefinition) -> Result<()> {
        let payload = checked_payload(agent, "agent definition")?;
        self.store.with_retry(|conn| {
            conn.execute(
                "INSERT INTO harness_agent_definitions
                 (id,name,status,schema_version,payload_json,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name,status=excluded.status,
                    schema_version=excluded.schema_version,payload_json=excluded.payload_json,
                    updated_at=excluded.updated_at",
                params![
                    agent.id,
                    agent.name,
                    agent.status,
                    agent.schema_version,
                    payload,
                    agent.created_at,
                    agent.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn save_goal(&self, goal: &Goal) -> Result<()> {
        if goal.objective.trim().is_empty() || goal.workspace_id.trim().is_empty() {
            return Err(NexusError::Config(
                "goal objective and workspace are required".into(),
            ));
        }
        if goal.status == GoalStatus::Completed && !goal.completion_evidenced() {
            return Err(NexusError::Config(
                "goal completion requires passing evidence for every criterion".into(),
            ));
        }
        let payload = checked_payload(goal, "goal")?;
        self.store.with_retry(|conn| {
            conn.execute(
                "INSERT INTO harness_goals
                 (id,owner_profile_id,workspace_id,project_id,status,priority,active_plan_id,
                  active_plan_version,schema_version,payload_json,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
                 ON CONFLICT(id) DO UPDATE SET owner_profile_id=excluded.owner_profile_id,
                  project_id=excluded.project_id,status=excluded.status,priority=excluded.priority,
                  active_plan_id=excluded.active_plan_id,
                  active_plan_version=excluded.active_plan_version,
                  schema_version=excluded.schema_version,payload_json=excluded.payload_json,
                  updated_at=excluded.updated_at",
                params![
                    goal.id,
                    goal.owner_profile_id,
                    goal.workspace_id,
                    goal.project_id,
                    goal.status.as_str(),
                    goal.priority,
                    goal.active_plan_id,
                    goal.active_plan_version,
                    goal.schema_version,
                    payload,
                    goal.created_at,
                    goal.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn goal(&self, goal_id: &str) -> Result<Goal> {
        self.store.with(|conn| Self::goal_conn(conn, goal_id))
    }

    fn goal_conn(conn: &rusqlite::Connection, goal_id: &str) -> Result<Goal> {
        let payload = conn
            .query_row(
                "SELECT payload_json FROM harness_goals WHERE id=?1",
                [goal_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|_| NexusError::NotFound(format!("goal `{goal_id}`")))?;
        decode(payload)
    }

    pub fn save_plan(&self, plan: &Plan) -> Result<()> {
        if plan.version == 0 {
            return Err(NexusError::Config("plan version must be positive".into()));
        }
        let payload = checked_payload(plan, "plan")?;
        self.store.with_retry(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<()> {
                Self::goal_conn(conn, &plan.goal_id)?;
                let latest: Option<u32> = conn.query_row(
                    "SELECT MAX(version) FROM harness_plans WHERE id=?1",
                    [&plan.id],
                    |row| row.get::<_, Option<u32>>(0),
                )?;
                let expected = latest.map_or(1, |version| version.saturating_add(1));
                if plan.version != expected {
                    return Err(NexusError::Config(format!(
                        "plan version must be {expected}, got {}",
                        plan.version
                    )));
                }
                conn.execute(
                    "INSERT INTO harness_plans
                     (id,version,goal_id,status,schema_version,payload_json,
                      created_at,updated_at,approved_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![
                        plan.id,
                        plan.version,
                        plan.goal_id,
                        plan.status.as_str(),
                        plan.schema_version,
                        payload,
                        plan.created_at,
                        plan.updated_at,
                        plan.approved_at,
                    ],
                )?;
                Ok(())
            })();
            finish_transaction(conn, result)
        })
    }

    pub fn plan(&self, plan_id: &str, version: u32) -> Result<Plan> {
        self.store
            .with(|conn| Self::plan_conn(conn, plan_id, version))
    }

    fn plan_conn(conn: &rusqlite::Connection, plan_id: &str, version: u32) -> Result<Plan> {
        let payload = conn
            .query_row(
                "SELECT payload_json FROM harness_plans WHERE id=?1 AND version=?2",
                params![plan_id, version],
                |row| row.get::<_, String>(0),
            )
            .map_err(|_| NexusError::NotFound(format!("plan `{plan_id}` v{version}")))?;
        decode(payload)
    }

    /// Approve a plan and atomically make it the goal's active strategy.
    pub fn approve_plan(&self, plan_id: &str, version: u32) -> Result<()> {
        self.store.with_retry(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<()> {
                let mut plan = Self::plan_conn(conn, plan_id, version)?;
                if !matches!(
                    plan.status,
                    PlanStatus::Draft
                        | PlanStatus::Analyzing
                        | PlanStatus::Proposed
                        | PlanStatus::UnderReview
                        | PlanStatus::NeedsRevision
                ) {
                    return Err(NexusError::Config(format!(
                        "plan in status {:?} cannot be approved",
                        plan.status
                    )));
                }
                if plan.phases.is_empty() {
                    return Err(NexusError::Config(
                        "plan approval requires at least one executable phase".into(),
                    ));
                }
                if plan.validation_gates.is_empty() {
                    return Err(NexusError::Config(
                        "plan approval requires at least one validation gate".into(),
                    ));
                }
                if plan
                    .rollback_strategy
                    .as_deref()
                    .is_none_or(|rollback| rollback.trim().is_empty())
                {
                    return Err(NexusError::Config(
                        "plan approval requires a rollback strategy".into(),
                    ));
                }
                let now = crate::now_rfc3339();
                plan.status = PlanStatus::Approved;
                plan.approved_at = Some(now.clone());
                plan.updated_at = now.clone();
                conn.execute(
                    "UPDATE harness_plans SET status='approved',payload_json=?1,
                     approved_at=?2,updated_at=?2 WHERE id=?3 AND version=?4",
                    params![encode(&plan)?, now, plan_id, version],
                )?;

                let mut goal = Self::goal_conn(conn, &plan.goal_id)?;
                goal.active_plan_id = Some(plan_id.to_string());
                goal.active_plan_version = Some(version);
                goal.status = GoalStatus::Active;
                goal.updated_at = now.clone();
                conn.execute(
                    "UPDATE harness_goals SET active_plan_id=?1,active_plan_version=?2,
                     status='active',payload_json=?3,updated_at=?4 WHERE id=?5",
                    params![plan_id, version, encode(&goal)?, now, goal.id],
                )?;
                Ok(())
            })();
            finish_transaction(conn, result)
        })
    }

    pub fn save_task(&self, task: &Task) -> Result<()> {
        if task.status == TaskStatus::Completed
            && (task.acceptance_criteria.is_empty()
                || task.acceptance_criteria.iter().any(|criterion| {
                    !task
                        .validation_evidence
                        .iter()
                        .any(|evidence| evidence.criterion == *criterion && evidence.passed)
                }))
        {
            return Err(NexusError::Config(
                "task completion requires passing evidence for every acceptance criterion".into(),
            ));
        }
        let payload = checked_payload(task, "task")?;
        self.store.with_retry(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<()> {
                if let (Some(plan_id), Some(version)) = (&task.plan_id, task.plan_version) {
                    let mut plan = Self::plan_conn(conn, plan_id, version)?;
                    if task.goal_id.as_deref() != Some(plan.goal_id.as_str()) {
                        return Err(NexusError::Config(
                            "task goal must match its plan goal".into(),
                        ));
                    }
                    if let Some(parent_id) = task.parent_task_id.as_deref() {
                        let parent = Self::task_conn(conn, parent_id)?;
                        if parent.plan_id.as_deref() != Some(plan_id)
                            || parent.plan_version != Some(version)
                        {
                            return Err(NexusError::Config(
                                "parent task must belong to the same plan version".into(),
                            ));
                        }
                    }
                    if !plan.task_ids.contains(&task.id) {
                        plan.task_ids.push(task.id.clone());
                        plan.updated_at = crate::now_rfc3339();
                        conn.execute(
                            "UPDATE harness_plans SET payload_json=?1,updated_at=?2
                             WHERE id=?3 AND version=?4",
                            params![encode(&plan)?, plan.updated_at, plan_id, version],
                        )?;
                    }
                } else if task.plan_id.is_some() || task.plan_version.is_some() {
                    return Err(NexusError::Config(
                        "task plan id and version must be set together".into(),
                    ));
                }
                if let Some(goal_id) = task.goal_id.as_deref() {
                    Self::goal_conn(conn, goal_id)?;
                }
                conn.execute(
                    "INSERT INTO harness_tasks
                     (id,goal_id,plan_id,plan_version,phase_id,parent_task_id,assigned_agent_id,
                      assigned_subagent_id,status,priority,schema_version,payload_json,
                      created_at,updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
                     ON CONFLICT(id) DO UPDATE SET phase_id=excluded.phase_id,
                      parent_task_id=excluded.parent_task_id,
                      assigned_agent_id=excluded.assigned_agent_id,
                      assigned_subagent_id=excluded.assigned_subagent_id,status=excluded.status,
                      priority=excluded.priority,schema_version=excluded.schema_version,
                      payload_json=excluded.payload_json,updated_at=excluded.updated_at",
                    params![
                        task.id,
                        task.goal_id,
                        task.plan_id,
                        task.plan_version,
                        task.phase_id,
                        task.parent_task_id,
                        task.assigned_agent_id,
                        task.assigned_subagent_id,
                        task.status.as_str(),
                        task.priority,
                        task.schema_version,
                        payload,
                        task.created_at,
                        task.updated_at,
                    ],
                )?;
                Ok(())
            })();
            finish_transaction(conn, result)
        })
    }

    pub fn task(&self, task_id: &str) -> Result<Task> {
        self.store.with(|conn| Self::task_conn(conn, task_id))
    }

    fn task_conn(conn: &rusqlite::Connection, task_id: &str) -> Result<Task> {
        let payload = conn
            .query_row(
                "SELECT payload_json FROM harness_tasks WHERE id=?1",
                [task_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|_| NexusError::NotFound(format!("task `{task_id}`")))?;
        decode(payload)
    }

    pub fn plan_tasks(&self, plan_id: &str, version: u32) -> Result<Vec<Task>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT payload_json FROM harness_tasks
                 WHERE plan_id=?1 AND plan_version=?2 ORDER BY priority DESC,created_at",
            )?;
            let rows = stmt.query_map(params![plan_id, version], |row| row.get::<_, String>(0))?;
            let mut tasks = Vec::new();
            for row in rows {
                tasks.push(decode(row?)?);
            }
            Ok(tasks)
        })
    }

    /// Add one blocking dependency. Both tasks must belong to the same plan
    /// version, and the complete graph is cycle-checked before commit.
    pub fn add_task_dependency(
        &self,
        plan_id: &str,
        version: u32,
        from_task_id: &str,
        to_task_id: &str,
    ) -> Result<()> {
        if from_task_id == to_task_id {
            return Err(NexusError::Config("a task cannot depend on itself".into()));
        }
        self.store.with_retry(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<()> {
                let from = Self::task_conn(conn, from_task_id)?;
                let mut to = Self::task_conn(conn, to_task_id)?;
                for task in [&from, &to] {
                    if task.plan_id.as_deref() != Some(plan_id)
                        || task.plan_version != Some(version)
                    {
                        return Err(NexusError::Config(
                            "dependency tasks must belong to the same plan version".into(),
                        ));
                    }
                }
                conn.execute(
                    "INSERT OR IGNORE INTO harness_task_edges
                     (plan_id,plan_version,from_task_id,to_task_id,relation,created_at)
                     VALUES (?1,?2,?3,?4,'blocks',?5)",
                    params![
                        plan_id,
                        version,
                        from_task_id,
                        to_task_id,
                        crate::now_rfc3339()
                    ],
                )?;
                if Self::task_graph_has_cycle_conn(conn, plan_id, version)? {
                    return Err(NexusError::Config(
                        "task dependency would create a cycle".into(),
                    ));
                }
                if !to.dependencies.iter().any(|id| id == from_task_id) {
                    to.dependencies.push(from_task_id.to_string());
                    to.updated_at = crate::now_rfc3339();
                    conn.execute(
                        "UPDATE harness_tasks SET payload_json=?1,updated_at=?2 WHERE id=?3",
                        params![encode(&to)?, to.updated_at, to.id],
                    )?;
                }
                Ok(())
            })();
            finish_transaction(conn, result)
        })
    }

    pub fn task_graph_has_cycle(&self, plan_id: &str, version: u32) -> Result<bool> {
        self.store
            .with(|conn| Self::task_graph_has_cycle_conn(conn, plan_id, version))
    }

    fn task_graph_has_cycle_conn(
        conn: &rusqlite::Connection,
        plan_id: &str,
        version: u32,
    ) -> Result<bool> {
        let mut nodes = BTreeSet::new();
        let mut task_stmt =
            conn.prepare("SELECT id FROM harness_tasks WHERE plan_id=?1 AND plan_version=?2")?;
        for row in task_stmt.query_map(params![plan_id, version], |row| row.get::<_, String>(0))? {
            nodes.insert(row?);
        }
        let mut outgoing: HashMap<String, Vec<String>> = HashMap::new();
        let mut indegree: HashMap<String, usize> =
            nodes.iter().map(|node| (node.clone(), 0)).collect();
        let mut edge_stmt = conn.prepare(
            "SELECT from_task_id,to_task_id FROM harness_task_edges
             WHERE plan_id=?1 AND plan_version=?2 AND relation='blocks'",
        )?;
        let edges = edge_stmt.query_map(params![plan_id, version], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for edge in edges {
            let (from, to) = edge?;
            outgoing.entry(from).or_default().push(to.clone());
            *indegree.entry(to).or_default() += 1;
        }
        let mut ready: VecDeque<String> = indegree
            .iter()
            .filter(|(_, degree)| **degree == 0)
            .map(|(node, _)| node.clone())
            .collect();
        let mut visited = 0usize;
        while let Some(node) = ready.pop_front() {
            visited += 1;
            if let Some(children) = outgoing.get(&node) {
                for child in children {
                    if let Some(degree) = indegree.get_mut(child) {
                        *degree = degree.saturating_sub(1);
                        if *degree == 0 {
                            ready.push_back(child.clone());
                        }
                    }
                }
            }
        }
        Ok(visited != indegree.len())
    }

    pub fn save_subagent_spec(&self, spec: &SubagentSpec) -> Result<()> {
        spec.context_scope.validate()?;
        if spec.assignment.trim().is_empty()
            || spec.role.trim().is_empty()
            || spec.output_contract.trim().is_empty()
        {
            return Err(NexusError::Config(
                "subagent role, assignment, and output contract are required".into(),
            ));
        }
        if spec.recursion_depth > 2 {
            return Err(NexusError::PolicyDenied(
                "subagent recursion depth is limited to 2".into(),
            ));
        }
        let payload = checked_payload(spec, "subagent specification")?;
        self.store.with_retry(|conn| {
            conn.execute(
                "INSERT INTO harness_subagent_specs
                 (id,parent_agent_id,parent_goal_id,parent_plan_id,parent_plan_version,
                  parent_task_id,status,assignment_fingerprint,schema_version,payload_json,
                  created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
                 ON CONFLICT(id) DO UPDATE SET status=excluded.status,
                  schema_version=excluded.schema_version,payload_json=excluded.payload_json,
                  updated_at=excluded.updated_at",
                params![
                    spec.id,
                    spec.parent_agent_id,
                    spec.parent_goal_id,
                    spec.parent_plan_id,
                    spec.parent_plan_version,
                    spec.parent_task_id,
                    spec.status.as_str(),
                    spec.assignment_fingerprint(),
                    spec.schema_version,
                    payload,
                    spec.created_at,
                    spec.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn subagent_spec(&self, spec_id: &str) -> Result<SubagentSpec> {
        self.store.with(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM harness_subagent_specs WHERE id=?1",
                    [spec_id],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|_| NexusError::NotFound(format!("subagent spec `{spec_id}`")))?;
            decode(payload)
        })
    }

    pub fn save_loop_state(&self, state: &LoopState) -> Result<()> {
        self.store.with_retry(|conn| {
            conn.execute(
                "INSERT INTO harness_loop_states
                 (run_id,session_id,profile_id,goal_id,plan_id,plan_version,task_id,agent_id,
                  status,progress_fingerprint,no_progress_count,schema_version,payload_json,
                  created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
                 ON CONFLICT(run_id) DO UPDATE SET status=excluded.status,
                  progress_fingerprint=excluded.progress_fingerprint,
                  no_progress_count=excluded.no_progress_count,
                  schema_version=excluded.schema_version,payload_json=excluded.payload_json,
                  updated_at=excluded.updated_at",
                params![
                    state.run_id,
                    state.session_id,
                    state.profile_id,
                    state.goal_id,
                    state.plan_id,
                    state.plan_version,
                    state.task_id,
                    state.agent_id,
                    state.status.as_str(),
                    state.progress_fingerprint,
                    state.no_progress_count,
                    state.schema_version,
                    encode(state)?,
                    state.created_at,
                    state.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn loop_state(&self, run_id: &str) -> Result<LoopState> {
        self.store.with(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM harness_loop_states WHERE run_id=?1",
                    [run_id],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|_| NexusError::NotFound(format!("loop run `{run_id}`")))?;
            decode(payload)
        })
    }

    pub fn save_checkpoint(&self, checkpoint: &Checkpoint) -> Result<()> {
        if checkpoint.session_id.trim().is_empty()
            || checkpoint.environment_fingerprint.trim().is_empty()
        {
            return Err(NexusError::Config(
                "checkpoint session and environment fingerprint are required".into(),
            ));
        }
        let payload = checked_payload(checkpoint, "checkpoint")?;
        self.store.with_retry(|conn| {
            conn.execute(
                "INSERT INTO harness_checkpoints
                 (id,session_id,run_id,goal_id,plan_id,plan_version,task_id,status,
                  environment_fingerprint,schema_version,payload_json,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
                 ON CONFLICT(id) DO UPDATE SET status=excluded.status,
                  environment_fingerprint=excluded.environment_fingerprint,
                  schema_version=excluded.schema_version,payload_json=excluded.payload_json,
                  updated_at=excluded.updated_at",
                params![
                    checkpoint.id,
                    checkpoint.session_id,
                    checkpoint.run_id,
                    checkpoint.active_context.goal_id,
                    checkpoint.active_context.plan_id,
                    checkpoint.active_context.plan_version,
                    checkpoint.active_context.task_id,
                    checkpoint.status,
                    checkpoint.environment_fingerprint,
                    checkpoint.schema_version,
                    payload,
                    checkpoint.created_at,
                    checkpoint.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn checkpoint(&self, checkpoint_id: &str) -> Result<Checkpoint> {
        self.store.with(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM harness_checkpoints WHERE id=?1",
                    [checkpoint_id],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|_| NexusError::NotFound(format!("checkpoint `{checkpoint_id}`")))?;
            decode(payload)
        })
    }

    pub fn latest_checkpoint(&self, session_id: &str) -> Result<Option<Checkpoint>> {
        self.store.with(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM harness_checkpoints
                     WHERE session_id=?1 AND status='active' ORDER BY created_at DESC LIMIT 1",
                    [session_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            payload.map(decode).transpose()
        })
    }

    pub fn assess_recovery(
        &self,
        checkpoint_id: &str,
        current_environment_fingerprint: &str,
        current_file_hashes: &BTreeMap<String, String>,
        current_assumptions: &BTreeMap<String, String>,
        provider_available: bool,
        model_available: bool,
    ) -> Result<RecoveryAssessment> {
        let checkpoint = self.checkpoint(checkpoint_id)?;
        let mut changed_files = Vec::new();
        let mut missing_files = Vec::new();
        for (path, expected_hash) in &checkpoint.file_hashes {
            match current_file_hashes.get(path) {
                Some(hash) if hash != expected_hash => changed_files.push(path.clone()),
                None => missing_files.push(path.clone()),
                _ => {}
            }
        }
        let mut stale_assumptions = Vec::new();
        for (key, expected) in &checkpoint.assumptions {
            if current_assumptions.get(key) != Some(expected) {
                stale_assumptions.push(key.clone());
            }
        }
        let environment_changed =
            checkpoint.environment_fingerprint != current_environment_fingerprint;
        let safe_to_resume_exactly = !environment_changed
            && changed_files.is_empty()
            && missing_files.is_empty()
            && stale_assumptions.is_empty()
            && provider_available
            && model_available;
        let recommended_strategy = if safe_to_resume_exactly {
            "resume_exactly"
        } else if !provider_available || !model_available {
            "change_model_or_provider"
        } else if !stale_assumptions.is_empty() {
            "revise_plan"
        } else {
            "revalidate_first"
        }
        .to_string();
        Ok(RecoveryAssessment {
            checkpoint_id: checkpoint.id,
            environment_changed,
            changed_files,
            missing_files,
            stale_assumptions,
            provider_available,
            model_available,
            safe_to_resume_exactly,
            recommended_strategy,
        })
    }

    pub fn save_improvement(&self, proposal: &ImprovementProposal) -> Result<()> {
        let payload = checked_payload(proposal, "improvement proposal")?;
        self.store.with_retry(|conn| {
            conn.execute(
                "INSERT INTO harness_improvement_proposals
                 (id,category,status,approval_required,schema_version,payload_json,
                  created_at,updated_at,reviewed_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(id) DO UPDATE SET status=excluded.status,
                  schema_version=excluded.schema_version,payload_json=excluded.payload_json,
                  updated_at=excluded.updated_at,reviewed_at=excluded.reviewed_at",
                params![
                    proposal.id,
                    proposal.category.as_str(),
                    proposal.status.as_str(),
                    proposal.approval_required,
                    proposal.schema_version,
                    payload,
                    proposal.created_at,
                    proposal.updated_at,
                    proposal.reviewed_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn improvement(&self, proposal_id: &str) -> Result<ImprovementProposal> {
        self.store
            .with(|conn| Self::improvement_conn(conn, proposal_id))
    }

    fn improvement_conn(
        conn: &rusqlite::Connection,
        proposal_id: &str,
    ) -> Result<ImprovementProposal> {
        let payload = conn
            .query_row(
                "SELECT payload_json FROM harness_improvement_proposals WHERE id=?1",
                [proposal_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|_| NexusError::NotFound(format!("improvement `{proposal_id}`")))?;
        decode(payload)
    }

    pub fn transition_improvement(&self, proposal_id: &str, next: ImprovementStatus) -> Result<()> {
        self.store.with_retry(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<()> {
                let mut proposal = Self::improvement_conn(conn, proposal_id)?;
                use ImprovementStatus::*;
                let allowed = matches!(
                    (proposal.status, next),
                    // Discovery → drafting.
                    (Observed, Draft)
                        // Drafting → review.
                        | (Draft, Proposed)
                        | (Proposed, Approved)
                        | (Proposed, Rejected)
                        | (Proposed, NeedsRevision)
                        // Revision loop.
                        | (NeedsRevision, Draft)
                        | (NeedsRevision, Proposed)
                        | (NeedsRevision, Rejected)
                        // Experiment / WARP validation.
                        | (Approved, Testing)
                        | (Approved, Rejected)
                        | (Testing, Validated)
                        | (Testing, Rejected)
                        | (Testing, NeedsRevision)
                        // Validated → promote directly (low tier) or via shadow/canary.
                        | (Validated, Shadow)
                        | (Validated, Promoted)
                        | (Validated, Rejected)
                        | (Shadow, Canary)
                        | (Shadow, Promoted)
                        | (Shadow, Rejected)
                        | (Shadow, NeedsRevision)
                        | (Canary, Promoted)
                        | (Canary, RolledBack)
                        | (Canary, Rejected)
                        // Post-promotion.
                        | (Promoted, RolledBack)
                        | (Promoted, Deprecated)
                        | (RolledBack, Deprecated)
                        // Legacy compatibility (pre-v2 flow).
                        | (Approved, Applied)
                        | (Testing, Applied)
                        | (Applied, RolledBack)
                        | (Applied, Deprecated)
                );
                if !allowed {
                    return Err(NexusError::PolicyDenied(format!(
                        "invalid improvement transition {:?} -> {next:?}",
                        proposal.status
                    )));
                }
                if next == ImprovementStatus::Promoted {
                    if proposal.risk_tier == RiskTier::Prohibited {
                        return Err(NexusError::PolicyDenied(
                            "prohibited improvements can never be promoted".into(),
                        ));
                    }
                    if proposal.risk_tier >= RiskTier::Moderate
                        && !matches!(proposal.status, Shadow | Canary)
                    {
                        return Err(NexusError::PolicyDenied(format!(
                            "{:?} improvements require shadow/canary validation before promotion",
                            proposal.risk_tier
                        )));
                    }
                    if proposal.risk_tier >= RiskTier::High && proposal.reviewed_at.is_none() {
                        return Err(NexusError::PolicyDenied(
                            "high-risk improvements require explicit review before promotion"
                                .into(),
                        ));
                    }
                }
                let now = crate::now_rfc3339();
                proposal.status = next;
                proposal.updated_at = now.clone();
                if matches!(
                    next,
                    ImprovementStatus::Approved | ImprovementStatus::Rejected
                ) {
                    proposal.reviewed_at = Some(now.clone());
                }
                let payload = checked_payload(&proposal, "improvement proposal")?;
                conn.execute(
                    "UPDATE harness_improvement_proposals
                     SET status=?1,payload_json=?2,updated_at=?3,reviewed_at=?4 WHERE id=?5",
                    params![
                        next.as_str(),
                        payload,
                        now,
                        proposal.reviewed_at,
                        proposal_id
                    ],
                )?;
                Ok(())
            })();
            finish_transaction(conn, result)
        })
    }

    pub fn save_provider_privacy_grant(&self, grant: &ProviderPrivacyGrant) -> Result<()> {
        grant.scope.validate()?;
        let scope_fingerprint = grant.scope.fingerprint()?;
        let (scope_kind, scope_key) = grant.scope.primary();
        let payload = checked_payload(grant, "provider privacy grant")?;
        self.store.with_retry(|conn| {
            conn.execute(
                "INSERT INTO harness_provider_privacy_grants
                 (id,provider_id,scope_fingerprint,scope_kind,scope_key,status,schema_version,
                  payload_json,created_at,updated_at,revoked_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
                 ON CONFLICT(id) DO UPDATE SET status=excluded.status,
                  schema_version=excluded.schema_version,payload_json=excluded.payload_json,
                  updated_at=excluded.updated_at,revoked_at=excluded.revoked_at",
                params![
                    grant.id,
                    grant.provider_id,
                    scope_fingerprint,
                    scope_kind,
                    scope_key,
                    grant.status,
                    grant.schema_version,
                    payload,
                    grant.created_at,
                    grant.updated_at,
                    grant.revoked_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn provider_allowed_for_scope(
        &self,
        provider_id: &str,
        scope: &MemoryScope,
    ) -> Result<bool> {
        let fingerprint = scope.fingerprint()?;
        self.store.with(|conn| {
            Ok(conn
                .prepare(
                    "SELECT 1 FROM harness_provider_privacy_grants
                     WHERE provider_id=?1 AND scope_fingerprint=?2 AND status='active'",
                )?
                .exists(params![provider_id, fingerprint])?)
        })
    }

    pub fn save_model_assignment(&self, assignment: &ModelAssignment) -> Result<()> {
        if assignment.provider_id.trim().is_empty()
            || assignment.model_id.trim().is_empty()
            || assignment.target_kind.trim().is_empty()
            || assignment.target_id.trim().is_empty()
        {
            return Err(NexusError::Config(
                "model assignment provider, model, and target are required".into(),
            ));
        }
        let payload = checked_payload(assignment, "model assignment")?;
        self.store.with_retry(|conn| {
            conn.execute(
                "INSERT INTO harness_model_assignments
                 (id,provider_id,model_id,target_kind,target_id,status,fallback_priority,
                  schema_version,payload_json,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
                 ON CONFLICT(id) DO UPDATE SET provider_id=excluded.provider_id,
                  model_id=excluded.model_id,status=excluded.status,
                  fallback_priority=excluded.fallback_priority,
                  schema_version=excluded.schema_version,payload_json=excluded.payload_json,
                  updated_at=excluded.updated_at",
                params![
                    assignment.id,
                    assignment.provider_id,
                    assignment.model_id,
                    assignment.target_kind,
                    assignment.target_id,
                    assignment.status,
                    assignment.fallback_priority,
                    assignment.schema_version,
                    payload,
                    assignment.created_at,
                    assignment.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn model_assignments(
        &self,
        target_kind: &str,
        target_id: &str,
    ) -> Result<Vec<ModelAssignment>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT payload_json FROM harness_model_assignments
                 WHERE target_kind=?1 AND target_id=?2 AND status='active'
                 ORDER BY fallback_priority",
            )?;
            let rows = stmt.query_map(params![target_kind, target_id], |row| {
                row.get::<_, String>(0)
            })?;
            decode_rows(rows)
        })
    }

    pub fn save_task_attempt(&self, attempt: &TaskAttempt) -> Result<()> {
        if attempt.attempt_number == 0 {
            return Err(NexusError::Config(
                "task attempt number must be positive".into(),
            ));
        }
        let payload = checked_payload(attempt, "task attempt")?;
        self.store.with_retry(|conn| {
            Self::task_conn(conn, &attempt.task_id)?;
            conn.execute(
                "INSERT INTO harness_task_attempts
                 (id,task_id,attempt_number,status,provider_id,model_id,schema_version,
                  payload_json,created_at,updated_at,finished_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
                 ON CONFLICT(id) DO UPDATE SET status=excluded.status,
                  schema_version=excluded.schema_version,payload_json=excluded.payload_json,
                  updated_at=excluded.updated_at,finished_at=excluded.finished_at",
                params![
                    attempt.id,
                    attempt.task_id,
                    attempt.attempt_number,
                    attempt.status,
                    attempt.provider_id,
                    attempt.model_id,
                    attempt.schema_version,
                    payload,
                    attempt.created_at,
                    attempt.updated_at,
                    attempt.finished_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn task_attempts(&self, task_id: &str) -> Result<Vec<TaskAttempt>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT payload_json FROM harness_task_attempts
                 WHERE task_id=?1 ORDER BY attempt_number",
            )?;
            let rows = stmt.query_map([task_id], |row| row.get::<_, String>(0))?;
            decode_rows(rows)
        })
    }

    /// Acquire a scoped read/write resource claim transactionally. A writer
    /// conflicts with every other active claim; readers conflict with writers.
    pub fn claim_resource(&self, claim: &ResourceClaim) -> Result<()> {
        if claim.resource_kind.trim().is_empty() || claim.resource_key.trim().is_empty() {
            return Err(NexusError::Config(
                "resource claim kind and key are required".into(),
            ));
        }
        self.store.with_retry(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<()> {
                // Claims may be held by plan tasks or background worker tasks.
                if Self::task_conn(conn, &claim.task_id).is_err() {
                    let in_background: bool = conn
                        .query_row(
                            "SELECT 1 FROM background_tasks WHERE id=?1",
                            [claim.task_id.as_str()],
                            |_| Ok(true),
                        )
                        .optional()?
                        .unwrap_or(false);
                    if !in_background {
                        return Err(NexusError::NotFound(format!("task `{}`", claim.task_id)));
                    }
                }
                let conflict: Option<String> = conn
                    .query_row(
                        "SELECT task_id FROM harness_resource_claims
                         WHERE resource_kind=?1 AND resource_key=?2 AND status='active'
                           AND (expires_at IS NULL OR expires_at>?3)
                           AND task_id<>?4 AND (access_mode='write' OR ?5='write') LIMIT 1",
                        params![
                            claim.resource_kind,
                            claim.resource_key,
                            crate::now_rfc3339(),
                            claim.task_id,
                            claim.access_mode.as_str()
                        ],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(owner) = conflict {
                    return Err(NexusError::PolicyDenied(format!(
                        "resource is already claimed by task `{owner}`"
                    )));
                }
                conn.execute(
                    "INSERT INTO harness_resource_claims
                     (id,task_id,resource_kind,resource_key,access_mode,status,schema_version,
                      payload_json,created_at,updated_at,expires_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                    params![
                        claim.id,
                        claim.task_id,
                        claim.resource_kind,
                        claim.resource_key,
                        claim.access_mode.as_str(),
                        claim.status,
                        claim.schema_version,
                        encode(claim)?,
                        claim.created_at,
                        claim.updated_at,
                        claim.expires_at,
                    ],
                )?;
                Ok(())
            })();
            finish_transaction(conn, result)
        })
    }

    pub fn release_resource_claim(&self, claim_id: &str) -> Result<()> {
        self.store.with_retry(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM harness_resource_claims WHERE id=?1",
                    [claim_id],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|_| NexusError::NotFound(format!("resource claim `{claim_id}`")))?;
            let mut claim: ResourceClaim = decode(payload)?;
            claim.status = "released".into();
            claim.updated_at = crate::now_rfc3339();
            conn.execute(
                "UPDATE harness_resource_claims
                 SET status='released',payload_json=?1,updated_at=?2 WHERE id=?3",
                params![encode(&claim)?, claim.updated_at, claim_id],
            )?;
            Ok(())
        })
    }

    pub fn save_approval_request(&self, request: &ApprovalRequest) -> Result<()> {
        let payload = encode(request)?;
        if contains_likely_secret(&payload) {
            return Err(NexusError::PolicyDenied(
                "refusing to persist an approval request containing a likely secret".into(),
            ));
        }
        self.store.with_retry(|conn| {
            conn.execute(
                "INSERT INTO harness_approval_requests
                 (id,session_id,task_id,run_id,requesting_agent_id,provider_id,model_id,
                  risk_class,status,grant_scope,schema_version,payload_json,
                  created_at,updated_at,resolved_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
                 ON CONFLICT(id) DO UPDATE SET status=excluded.status,
                  grant_scope=excluded.grant_scope,schema_version=excluded.schema_version,
                  payload_json=excluded.payload_json,updated_at=excluded.updated_at,
                  resolved_at=excluded.resolved_at",
                params![
                    request.id,
                    request.session_id,
                    request.task_id,
                    request.run_id,
                    request.requesting_agent_id,
                    request.provider_id,
                    request.model_id,
                    request.risk_class,
                    request.status.as_str(),
                    request.grant_scope,
                    request.schema_version,
                    payload,
                    request.created_at,
                    request.updated_at,
                    request.resolved_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn resolve_approval_request(
        &self,
        request_id: &str,
        decision: ApprovalStatus,
        decision_note: Option<&str>,
    ) -> Result<()> {
        if decision == ApprovalStatus::Pending {
            return Err(NexusError::Config(
                "approval resolution cannot remain pending".into(),
            ));
        }
        self.store.with_retry(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<()> {
                let payload = conn
                    .query_row(
                        "SELECT payload_json FROM harness_approval_requests
                         WHERE id=?1 AND status='pending'",
                        [request_id],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(|_| {
                        NexusError::NotFound(format!("pending approval `{request_id}`"))
                    })?;
                let mut request: ApprovalRequest = decode(payload)?;
                request.status = decision;
                request.decision_note = decision_note.map(str::to_string);
                let now = crate::now_rfc3339();
                request.updated_at = now.clone();
                request.resolved_at = Some(now.clone());
                let encoded = encode(&request)?;
                if contains_likely_secret(&encoded) {
                    return Err(NexusError::PolicyDenied(
                        "refusing to persist an approval decision containing a likely secret"
                            .into(),
                    ));
                }
                conn.execute(
                    "UPDATE harness_approval_requests SET status=?1,payload_json=?2,
                     updated_at=?3,resolved_at=?3 WHERE id=?4",
                    params![decision.as_str(), encoded, now, request_id],
                )?;
                Ok(())
            })();
            finish_transaction(conn, result)
        })
    }

    pub fn approval_requests(
        &self,
        session_id: Option<&str>,
        pending_only: bool,
    ) -> Result<Vec<ApprovalRequest>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT payload_json FROM harness_approval_requests
                 WHERE (?1 IS NULL OR session_id=?1) AND (?2=0 OR status='pending')
                 ORDER BY created_at DESC",
            )?;
            let rows = stmt.query_map(params![session_id, pending_only], |row| {
                row.get::<_, String>(0)
            })?;
            decode_rows(rows)
        })
    }

    pub fn append_event(&self, event: &HarnessEvent) -> Result<()> {
        let payload = encode(event)?;
        if contains_likely_secret(&payload) {
            return Err(NexusError::PolicyDenied(
                "refusing to persist a harness event containing a likely secret".into(),
            ));
        }
        self.store.with_retry(|conn| {
            conn.execute(
                "INSERT INTO harness_events
                 (id,event_type,at,session_id,profile_id,goal_id,plan_id,task_id,agent_id,
                  subagent_id,run_id,sensitivity,schema_version,payload_json)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                params![
                    event.id,
                    event.event_type,
                    event.timestamp,
                    event.session_id,
                    event.profile_id,
                    event.goal_id,
                    event.plan_id,
                    event.task_id,
                    event.agent_id,
                    event.subagent_id,
                    event.run_id,
                    event.sensitivity,
                    event.schema_version,
                    payload,
                ],
            )?;
            Ok(())
        })
    }

    pub fn session_events(&self, session_id: &str, limit: usize) -> Result<Vec<HarnessEvent>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT payload_json FROM harness_events
                 WHERE session_id=?1 ORDER BY at DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![session_id, limit as i64], |row| {
                row.get::<_, String>(0)
            })?;
            let mut events = Vec::new();
            for row in rows {
                events.push(decode(row?)?);
            }
            Ok(events)
        })
    }

    pub fn profiles(&self, include_archived: bool) -> Result<Vec<UserProfile>> {
        self.store.with(|conn| {
            let sql = if include_archived {
                "SELECT payload_json FROM harness_profiles
                 WHERE status!='deleted' ORDER BY updated_at DESC"
            } else {
                "SELECT payload_json FROM harness_profiles
                 WHERE status NOT IN ('archived','deleted') ORDER BY updated_at DESC"
            };
            collect_payloads(conn, sql, [])
        })
    }

    pub fn identity_conflicts(&self, pending_only: bool) -> Result<Vec<IdentityConflict>> {
        self.store.with(|conn| {
            let sql = if pending_only {
                "SELECT payload_json FROM harness_identity_conflicts
                 WHERE status='pending' ORDER BY created_at DESC"
            } else {
                "SELECT payload_json FROM harness_identity_conflicts ORDER BY created_at DESC"
            };
            collect_payloads(conn, sql, [])
        })
    }

    pub fn persona_versions(
        &self,
        scope_kind: Option<&str>,
        scope_key: Option<&str>,
    ) -> Result<Vec<PersonaVersion>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT payload_json FROM harness_persona_versions
                 WHERE (?1 IS NULL OR scope_kind=?1) AND (?2 IS NULL OR scope_key=?2)
                 ORDER BY name,version DESC",
            )?;
            let rows = stmt.query_map(params![scope_kind, scope_key], |row| {
                row.get::<_, String>(0)
            })?;
            decode_rows(rows)
        })
    }

    /// Load one immutable persona version by its canonical identity. Prompt
    /// composition uses this exact lookup so an active context cannot drift
    /// to a newer version without an explicit selection.
    pub fn persona_version(&self, persona_id: &str, version: u32) -> Result<PersonaVersion> {
        self.store.with(|conn| {
            let payload = conn
                .query_row(
                    "SELECT payload_json FROM harness_persona_versions
                     WHERE persona_id=?1 AND version=?2",
                    params![persona_id, version],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|_| {
                    NexusError::NotFound(format!("persona `{persona_id}` version {version}"))
                })?;
            decode(payload)
        })
    }

    pub fn agent_definitions(&self, include_archived: bool) -> Result<Vec<AgentDefinition>> {
        self.store.with(|conn| {
            let sql = if include_archived {
                "SELECT payload_json FROM harness_agent_definitions ORDER BY name"
            } else {
                "SELECT payload_json FROM harness_agent_definitions
                 WHERE status='active' ORDER BY name"
            };
            collect_payloads(conn, sql, [])
        })
    }

    pub fn goals(&self, workspace_id: &str, status: Option<GoalStatus>) -> Result<Vec<Goal>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT payload_json FROM harness_goals
                 WHERE workspace_id=?1 AND (?2 IS NULL OR status=?2)
                 ORDER BY priority DESC,updated_at DESC",
            )?;
            let status = status.map(GoalStatus::as_str);
            let rows =
                stmt.query_map(params![workspace_id, status], |row| row.get::<_, String>(0))?;
            decode_rows(rows)
        })
    }

    pub fn plans_for_goal(&self, goal_id: &str) -> Result<Vec<Plan>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT payload_json FROM harness_plans
                 WHERE goal_id=?1 ORDER BY created_at DESC,version DESC",
            )?;
            let rows = stmt.query_map([goal_id], |row| row.get::<_, String>(0))?;
            decode_rows(rows)
        })
    }

    pub fn tasks(
        &self,
        plan_id: &str,
        plan_version: u32,
        status: Option<TaskStatus>,
    ) -> Result<Vec<Task>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT payload_json FROM harness_tasks
                 WHERE plan_id=?1 AND plan_version=?2 AND (?3 IS NULL OR status=?3)
                 ORDER BY priority DESC,created_at",
            )?;
            let status = status.map(TaskStatus::as_str);
            let rows = stmt.query_map(params![plan_id, plan_version, status], |row| {
                row.get::<_, String>(0)
            })?;
            decode_rows(rows)
        })
    }

    pub fn subagent_specs(
        &self,
        parent_task_id: Option<&str>,
        status: Option<SubagentStatus>,
    ) -> Result<Vec<SubagentSpec>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT payload_json FROM harness_subagent_specs
                 WHERE (?1 IS NULL OR parent_task_id=?1) AND (?2 IS NULL OR status=?2)
                 ORDER BY updated_at DESC",
            )?;
            let status = status.map(SubagentStatus::as_str);
            let rows = stmt.query_map(params![parent_task_id, status], |row| {
                row.get::<_, String>(0)
            })?;
            decode_rows(rows)
        })
    }

    pub fn loop_states(
        &self,
        session_id: &str,
        status: Option<LoopStatus>,
    ) -> Result<Vec<LoopState>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT payload_json FROM harness_loop_states
                 WHERE session_id=?1 AND (?2 IS NULL OR status=?2) ORDER BY updated_at DESC",
            )?;
            let status = status.map(LoopStatus::as_str);
            let rows =
                stmt.query_map(params![session_id, status], |row| row.get::<_, String>(0))?;
            decode_rows(rows)
        })
    }

    pub fn checkpoints(&self, session_id: &str, include_inactive: bool) -> Result<Vec<Checkpoint>> {
        self.store.with(|conn| {
            let sql = if include_inactive {
                "SELECT payload_json FROM harness_checkpoints
                 WHERE session_id=?1 ORDER BY created_at DESC"
            } else {
                "SELECT payload_json FROM harness_checkpoints
                 WHERE session_id=?1 AND status='active' ORDER BY created_at DESC"
            };
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([session_id], |row| row.get::<_, String>(0))?;
            decode_rows(rows)
        })
    }

    pub fn improvement_proposals(
        &self,
        status: Option<ImprovementStatus>,
    ) -> Result<Vec<ImprovementProposal>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT payload_json FROM harness_improvement_proposals
                 WHERE (?1 IS NULL OR status=?1) ORDER BY updated_at DESC",
            )?;
            let status = status.map(ImprovementStatus::as_str);
            let rows = stmt.query_map(params![status], |row| row.get::<_, String>(0))?;
            decode_rows(rows)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn events(
        &self,
        session_id: Option<&str>,
        profile_id: Option<&str>,
        goal_id: Option<&str>,
        plan_id: Option<&str>,
        task_id: Option<&str>,
        run_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<HarnessEvent>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT payload_json FROM harness_events
                 WHERE (?1 IS NULL OR session_id=?1)
                   AND (?2 IS NULL OR profile_id=?2)
                   AND (?3 IS NULL OR goal_id=?3)
                   AND (?4 IS NULL OR plan_id=?4)
                   AND (?5 IS NULL OR task_id=?5)
                   AND (?6 IS NULL OR run_id=?6)
                 ORDER BY at DESC LIMIT ?7",
            )?;
            let rows = stmt.query_map(
                params![
                    session_id,
                    profile_id,
                    goal_id,
                    plan_id,
                    task_id,
                    run_id,
                    limit as i64
                ],
                |row| row.get::<_, String>(0),
            )?;
            decode_rows(rows)
        })
    }
}

fn finish_transaction<T>(conn: &rusqlite::Connection, result: Result<T>) -> Result<T> {
    match result {
        Ok(value) => {
            conn.execute_batch("COMMIT")?;
            Ok(value)
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn contains_likely_secret(payload: &str) -> bool {
    let lower = payload.to_ascii_lowercase();
    lower.contains("bearer ")
        || lower.contains("\"authorization\":")
        || lower.contains("\"api_key\":")
        || lower.contains("sk-")
}

fn checked_payload<T: Serialize>(value: &T, record_kind: &str) -> Result<String> {
    let payload = encode(value)?;
    let pattern_redacted = crate::redact::Redactor::new().redact(&payload);
    if pattern_redacted != payload || contains_likely_secret(&payload) {
        return Err(NexusError::PolicyDenied(format!(
            "refusing to persist {record_kind} containing a likely secret"
        )));
    }
    Ok(payload)
}

fn decode_rows<T, F>(rows: rusqlite::MappedRows<'_, F>) -> Result<Vec<T>>
where
    T: DeserializeOwned,
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String>,
{
    let mut values = Vec::new();
    for row in rows {
        values.push(decode(row?)?);
    }
    Ok(values)
}

fn collect_payloads<T, P>(conn: &rusqlite::Connection, sql: &str, parameters: P) -> Result<Vec<T>>
where
    T: DeserializeOwned,
    P: rusqlite::Params,
{
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(parameters, |row| row.get::<_, String>(0))?;
    decode_rows(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> HarnessRepository {
        HarnessRepository::new(Store::open_in_memory().expect("in-memory store"))
    }

    #[test]
    fn explicit_identity_creates_profile_and_conflict_never_overwrites_active_person() {
        let repository = repository();
        let mut default = UserProfile::new("Default").expect("default profile");
        default
            .metadata
            .insert("is_default".into(), Value::Bool(true));
        repository.create_profile(&default).expect("save default");

        let sans = match repository
            .resolve_explicit_identity(Some(&default.id), "Sans", Some("turn_1"))
            .expect("resolve Sans")
        {
            IdentityResolution::Created(profile) => profile,
            other => panic!("expected creation, got {other:?}"),
        };
        let facts = repository
            .profile_facts(&sans.id, false)
            .expect("Sans facts");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].source_type, ProfileFactSource::UserExplicit);
        assert_eq!(facts[0].source_ref.as_deref(), Some("turn_1"));

        let alex = UserProfile::new("Alex").expect("Alex profile");
        repository.create_profile(&alex).expect("save Alex");
        let conflict = match repository
            .resolve_explicit_identity(Some(&alex.id), "Sans", Some("turn_2"))
            .expect("identity conflict")
        {
            IdentityResolution::Conflict(conflict) => conflict,
            other => panic!("expected conflict, got {other:?}"),
        };
        assert_eq!(
            conflict.active_profile_id.as_deref(),
            Some(alex.id.as_str())
        );
        assert_eq!(
            conflict.candidate_profile_id.as_deref(),
            Some(sans.id.as_str())
        );
        assert_eq!(
            repository
                .profile(&alex.id)
                .expect("reload Alex")
                .display_name,
            "Alex"
        );
        assert!(repository
            .profile_facts(&alex.id, true)
            .expect("Alex facts")
            .is_empty());

        let resolution = repository
            .resolve_identity_conflict(
                &conflict.id,
                IdentityConflictDecision::SwitchExisting(sans.id.clone()),
            )
            .expect("resolve conflict");
        assert_eq!(
            resolution
                .selected_profile
                .as_ref()
                .map(|profile| profile.id.as_str()),
            Some(sans.id.as_str())
        );
        assert_eq!(resolution.conflict.status, IdentityConflictStatus::Resolved);
        assert!(repository
            .resolve_identity_conflict(&conflict.id, IdentityConflictDecision::KeepActive,)
            .is_err());
    }

    #[test]
    fn profile_fact_review_enforces_exact_profile_ownership() {
        let repository = repository();
        let first = UserProfile::new("First").expect("first profile");
        let second = UserProfile::new("Second").expect("second profile");
        repository.create_profile(&first).expect("save first");
        repository.create_profile(&second).expect("save second");

        let now = crate::now_rfc3339();
        let fact = ProfileFact {
            id: "fact_candidate".into(),
            profile_id: first.id.clone(),
            key: "preferences.response_format".into(),
            value: Value::String("concise".into()),
            source_type: ProfileFactSource::Imported,
            source_ref: Some("import:test".into()),
            confidence: 0.7,
            sensitivity: "normal".into(),
            status: ProfileFactStatus::Candidate,
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
            expires_at: None,
        };
        repository.add_profile_fact(&fact).expect("save fact");

        assert!(matches!(
            repository.set_profile_fact_status(&second.id, &fact.id, ProfileFactStatus::Active,),
            Err(NexusError::NotFound(_))
        ));
        assert_eq!(
            repository
                .profile_facts(&first.id, true)
                .expect("unchanged candidate")[0]
                .status,
            ProfileFactStatus::Candidate
        );

        let approved = repository
            .set_profile_fact_status(&first.id, &fact.id, ProfileFactStatus::Active)
            .expect("approve owned fact");
        assert_eq!(approved.status, ProfileFactStatus::Active);
    }

    #[test]
    fn profile_archive_restore_and_soft_delete_preserve_records() {
        let repository = repository();
        let profile = UserProfile::new("Archive Me").expect("profile");
        repository.create_profile(&profile).expect("save profile");

        let archived = repository
            .set_profile_status(&profile.id, ProfileStatus::Archived)
            .expect("archive profile");
        assert_eq!(archived.status, ProfileStatus::Archived);
        assert!(repository.profiles(false).expect("active list").is_empty());
        assert_eq!(repository.profiles(true).expect("archive list").len(), 1);

        let restored = repository
            .set_profile_status(&profile.id, ProfileStatus::Active)
            .expect("restore profile");
        assert_eq!(restored.status, ProfileStatus::Active);
        repository
            .set_profile_status(&profile.id, ProfileStatus::Deleted)
            .expect("soft delete profile");
        assert!(repository.profiles(true).expect("visible list").is_empty());
        assert_eq!(
            repository
                .profile(&profile.id)
                .expect("durable record")
                .status,
            ProfileStatus::Deleted
        );

        let mut default = UserProfile::new("Default").expect("default");
        default
            .metadata
            .insert("is_default".into(), Value::Bool(true));
        repository.create_profile(&default).expect("save default");
        assert!(matches!(
            repository.set_profile_status(&default.id, ProfileStatus::Deleted),
            Err(NexusError::PolicyDenied(_))
        ));
    }

    #[test]
    fn memory_queries_enforce_exact_scope_before_content_matching() {
        let repository = repository();
        let scope_a = MemoryScope::profile("profile_a");
        let scope_b = MemoryScope::profile("profile_b");
        let mut memory_a = MemoryRecord::new(
            MemoryType::Semantic,
            scope_a.clone(),
            "project alpha uses Rust",
            MemorySourceType::UserExplicit,
        )
        .expect("memory A");
        memory_a.status = MemoryStatus::Active;
        let mut memory_b = MemoryRecord::new(
            MemoryType::Semantic,
            scope_b.clone(),
            "project alpha contains private profile B details",
            MemorySourceType::UserExplicit,
        )
        .expect("memory B");
        memory_b.status = MemoryStatus::Active;
        repository.save_memory(&memory_a).expect("save A");
        repository.save_memory(&memory_b).expect("save B");

        let found = repository
            .query_memories(std::slice::from_ref(&scope_a), Some("project alpha"), 10)
            .expect("scoped query");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, memory_a.id);
        assert!(matches!(
            repository.memory_in_scopes(&memory_b.id, &[scope_a]),
            Err(NexusError::PolicyDenied(_))
        ));
        assert_eq!(
            repository
                .memory_in_scopes(&memory_b.id, &[scope_b])
                .expect("authorized memory")
                .id,
            memory_b.id
        );
    }

    #[test]
    fn canonical_memory_score_ranks_objective_overlap_then_importance() {
        let scope = MemoryScope::profile("profile_a");
        let mut relevant = MemoryRecord::new(
            MemoryType::Semantic,
            scope.clone(),
            "deployment pipeline uses staging validation",
            MemorySourceType::UserExplicit,
        )
        .expect("relevant memory");
        relevant.importance = 0.2;
        let mut unrelated = MemoryRecord::new(
            MemoryType::Semantic,
            scope.clone(),
            "user prefers concise answers",
            MemorySourceType::UserExplicit,
        )
        .expect("unrelated memory");
        unrelated.importance = 0.2;

        let objective = "validate the staging deployment pipeline";
        assert!(
            canonical_memory_score(&relevant, objective)
                > canonical_memory_score(&unrelated, objective)
        );

        // With no usable objective terms, stored importance breaks the tie.
        let mut important = unrelated.clone();
        important.importance = 0.9;
        assert!(canonical_memory_score(&important, "") > canonical_memory_score(&unrelated, ""));

        // Tags participate in overlap so curated labels stay retrievable.
        let mut tagged = MemoryRecord::new(
            MemoryType::Procedural,
            scope,
            "run the release checklist",
            MemorySourceType::UserConfirmed,
        )
        .expect("tagged memory");
        tagged.importance = 0.2;
        tagged.tags = vec!["deployment".into(), "pipeline".into()];
        assert!(
            canonical_memory_score(&tagged, objective)
                > canonical_memory_score(&unrelated, objective)
        );
    }

    #[test]
    fn global_memory_requires_explicit_non_sensitive_classification() {
        let repository = repository();
        let mut memory = MemoryRecord::new(
            MemoryType::Procedural,
            MemoryScope::global(),
            "Run the formatter before tests",
            MemorySourceType::AgentSummary,
        )
        .expect("global memory");
        memory.status = MemoryStatus::Active;
        assert!(matches!(
            repository.save_memory(&memory),
            Err(NexusError::PolicyDenied(_))
        ));

        memory.sensitivity = "non_sensitive".into();
        repository.save_memory(&memory).expect("safe global memory");
        assert_eq!(
            repository
                .query_memories(&[MemoryScope::global()], Some("formatter"), 5)
                .expect("global query")
                .len(),
            1
        );
    }

    #[test]
    fn plan_task_links_are_transactional_and_cycles_are_rejected() {
        let repository = repository();
        let goal = Goal::new("Ship the harness", "workspace").expect("goal");
        repository.save_goal(&goal).expect("save goal");
        let plan = Plan::new(&goal.id, "Implementation plan").expect("plan");
        repository.save_plan(&plan).expect("save plan");

        let make_task = |title: &str| {
            let mut task = Task::new(title, title).expect("task");
            task.goal_id = Some(goal.id.clone());
            task.plan_id = Some(plan.id.clone());
            task.plan_version = Some(plan.version);
            task.status = TaskStatus::Pending;
            task
        };
        let first = make_task("schema");
        let second = make_task("repository");
        let third = make_task("tests");
        for task in [&first, &second, &third] {
            repository.save_task(task).expect("save task");
        }
        repository
            .add_task_dependency(&plan.id, plan.version, &first.id, &second.id)
            .expect("first blocks second");
        repository
            .add_task_dependency(&plan.id, plan.version, &second.id, &third.id)
            .expect("second blocks third");
        assert!(repository
            .add_task_dependency(&plan.id, plan.version, &third.id, &first.id)
            .is_err());
        assert!(!repository
            .task_graph_has_cycle(&plan.id, plan.version)
            .expect("cycle check"));
        assert_eq!(
            repository.task(&third.id).expect("third task").dependencies,
            vec![second.id.clone()]
        );
        let stored_plan = repository
            .plan(&plan.id, plan.version)
            .expect("stored plan");
        assert_eq!(stored_plan.task_ids.len(), 3);
    }

    #[test]
    fn plan_and_task_completion_require_structured_evidence_gates() {
        let repository = repository();
        let goal = Goal::new("Ship with evidence", "workspace").expect("goal");
        repository.save_goal(&goal).expect("save goal");

        let mut plan = Plan::new(&goal.id, "Evidence plan").expect("plan");
        repository.save_plan(&plan).expect("save plan");
        assert!(repository.approve_plan(&plan.id, plan.version).is_err());

        plan.version += 1;
        plan.status = PlanStatus::UnderReview;
        plan.phases.push(PlanPhase {
            id: "phase-validation".into(),
            title: "Validate".into(),
            summary: "Run the acceptance checks".into(),
            milestones: vec!["checks pass".into()],
            status: "pending".into(),
        });
        plan.validation_gates.push(ValidationGate {
            id: "gate-tests".into(),
            description: "Automated tests pass".into(),
            required_evidence: vec!["test report".into()],
            passed: false,
        });
        plan.rollback_strategy = Some("restore the previous reviewed plan version".into());
        repository.save_plan(&plan).expect("save complete plan");
        repository
            .approve_plan(&plan.id, plan.version)
            .expect("approve complete plan");

        let mut task = Task::new("Run tests", "Execute the validation suite").expect("task");
        task.goal_id = Some(goal.id.clone());
        task.plan_id = Some(plan.id.clone());
        task.plan_version = Some(plan.version);
        task.phase_id = Some("phase-validation".into());
        task.acceptance_criteria = vec!["suite passes".into()];
        task.status = TaskStatus::Completed;
        assert!(repository.save_task(&task).is_err());
        task.validation_evidence.push(EvidenceReference {
            criterion: "suite passes".into(),
            summary: "all focused tests passed".into(),
            source_ref: "artifact:test-report".into(),
            passed: true,
            observed_at: crate::now_rfc3339(),
        });
        repository.save_task(&task).expect("save evidenced task");
    }

    #[test]
    fn loop_limits_and_no_progress_fingerprint_stop_deterministically() {
        let limits = LoopLimits {
            max_model_calls: 2,
            no_progress_limit: 2,
            ..LoopLimits::default()
        };
        let mut state = LoopState::new("session", limits);
        assert!(!state.observe_progress("hash-a"));
        assert!(!state.observe_progress("hash-a"));
        assert!(state.observe_progress("hash-a"));
        assert_eq!(
            state.limit_stop(crate::now_ms()),
            Some(LoopStopReason::NoProgress)
        );
        state.observe_progress("hash-b");
        state.model_call_count = 2;
        assert_eq!(
            state.limit_stop(crate::now_ms()),
            Some(LoopStopReason::ModelCallLimit)
        );

        let repository = repository();
        repository.save_loop_state(&state).expect("save loop");
        assert_eq!(
            repository
                .loop_state(&state.run_id)
                .expect("load loop")
                .model_call_count,
            2
        );
    }

    #[test]
    fn checkpoint_recovery_detects_environment_files_and_assumptions() {
        let repository = repository();
        let context = ActiveHarnessContext::new("workspace", Some("session".into()));
        let mut checkpoint = Checkpoint::new("session", context, "env-a");
        checkpoint
            .file_hashes
            .insert("src/lib.rs".into(), "aaa".into());
        checkpoint
            .assumptions
            .insert("provider".into(), "online".into());
        repository
            .save_checkpoint(&checkpoint)
            .expect("save checkpoint");

        let exact = repository
            .assess_recovery(
                &checkpoint.id,
                "env-a",
                &BTreeMap::from([("src/lib.rs".into(), "aaa".into())]),
                &BTreeMap::from([("provider".into(), "online".into())]),
                true,
                true,
            )
            .expect("exact assessment");
        assert!(exact.safe_to_resume_exactly);
        assert_eq!(exact.recommended_strategy, "resume_exactly");

        let stale = repository
            .assess_recovery(
                &checkpoint.id,
                "env-b",
                &BTreeMap::from([("src/lib.rs".into(), "bbb".into())]),
                &BTreeMap::new(),
                true,
                true,
            )
            .expect("stale assessment");
        assert!(!stale.safe_to_resume_exactly);
        assert_eq!(stale.changed_files, vec!["src/lib.rs"]);
        assert_eq!(stale.stale_assumptions, vec!["provider"]);
        assert_eq!(stale.recommended_strategy, "revise_plan");
    }

    #[test]
    fn improvements_require_review_testing_and_support_rollback() {
        let repository = repository();
        let proposal = ImprovementProposal::new(
            ImprovementCategory::Tool,
            "Repeated tool failure",
            "Introduce a bounded adapter",
        )
        .expect("proposal");
        repository
            .save_improvement(&proposal)
            .expect("save proposal");
        assert!(repository
            .transition_improvement(&proposal.id, ImprovementStatus::Applied)
            .is_err());
        for status in [
            ImprovementStatus::Proposed,
            ImprovementStatus::Approved,
            ImprovementStatus::Testing,
            ImprovementStatus::Applied,
            ImprovementStatus::RolledBack,
        ] {
            repository
                .transition_improvement(&proposal.id, status)
                .expect("valid transition");
        }
        assert_eq!(
            repository
                .improvement(&proposal.id)
                .expect("load proposal")
                .status,
            ImprovementStatus::RolledBack
        );
    }

    #[test]
    fn governed_candidate_walks_the_full_rsi_state_machine() {
        let repository = repository();
        let mut proposal = ImprovementProposal::new(
            ImprovementCategory::Context,
            "Duplicate repository retrieval during planning",
            "Add a cache-invalidation policy to the context router",
        )
        .expect("proposal");
        // Typed RSI fields default conservatively for un-annotated candidates.
        assert_eq!(proposal.target, ImprovementTarget::HarnessComponent);
        assert_eq!(proposal.risk_tier, RiskTier::High);
        proposal.target = ImprovementTarget::ContextRouter;
        proposal.risk_tier = RiskTier::Moderate;
        proposal.created_by = "improvement_planner".into();
        proposal.status = ImprovementStatus::Observed;
        repository.save_improvement(&proposal).expect("save");

        // The moderate-tier path must pass through shadow before promotion.
        for status in [
            ImprovementStatus::Draft,
            ImprovementStatus::Proposed,
            ImprovementStatus::Approved,
            ImprovementStatus::Testing,
            ImprovementStatus::Validated,
            ImprovementStatus::Shadow,
            ImprovementStatus::Canary,
            ImprovementStatus::Promoted,
        ] {
            repository
                .transition_improvement(&proposal.id, status)
                .expect("valid transition");
        }
        // Validated cannot leap straight to Canary (must shadow first, already past).
        assert!(repository
            .transition_improvement(&proposal.id, ImprovementStatus::Validated)
            .is_err());

        // Typed fields survive the JSON payload round-trip.
        let loaded = repository.improvement(&proposal.id).expect("load");
        assert_eq!(loaded.status, ImprovementStatus::Promoted);
        assert_eq!(loaded.target, ImprovementTarget::ContextRouter);
        assert_eq!(loaded.target.plane(), ImprovementPlane::Data);
        assert_eq!(loaded.risk_tier, RiskTier::Moderate);
        assert_eq!(loaded.created_by, "improvement_planner");
        assert_eq!(
            ImprovementTarget::HarnessComponent.plane(),
            ImprovementPlane::Code
        );
        assert!(RiskTier::Prohibited > RiskTier::High);
    }

    #[test]
    fn canonical_text_writes_reject_secrets_without_persisting_or_echoing_them() {
        let repository = repository();
        let secret = "sk-abcdefghijklmnopqrstuvwx";
        let profile = UserProfile::new("Safe profile").expect("profile");
        repository.create_profile(&profile).expect("save profile");

        let mut fact = ProfileFact::explicit_name(&profile.id, "safe");
        fact.value = Value::String(format!("credential {secret}"));
        let mut memory = MemoryRecord::new(
            MemoryType::Semantic,
            MemoryScope::profile(&profile.id),
            format!("credential {secret}"),
            MemorySourceType::UserExplicit,
        )
        .expect("memory");
        memory.status = MemoryStatus::Active;
        let persona =
            PersonaVersion::first("unsafe", format!("Use {secret}")).expect("persona definition");
        let mut unsafe_goal = Goal::new("unsafe", "workspace").expect("unsafe goal");
        unsafe_goal.objective = format!("Use {secret}");

        let safe_goal = Goal::new("safe goal", "workspace").expect("safe goal");
        repository.save_goal(&safe_goal).expect("save safe goal");
        let mut plan = Plan::new(&safe_goal.id, "plan").expect("plan");
        plan.summary = format!("credential {secret}");
        let mut task = Task::new("task", format!("credential {secret}")).expect("task");
        task.goal_id = Some(safe_goal.id.clone());
        let improvement = ImprovementProposal::new(
            ImprovementCategory::Tool,
            "failure",
            format!("configure {secret}"),
        )
        .expect("improvement");
        let now = crate::now_rfc3339();
        let subagent = SubagentSpec {
            id: stable_id("subagent"),
            role: "reviewer".into(),
            assignment: format!("inspect {secret}"),
            context_scope: MemoryScope::workspace("workspace"),
            allowed_tools: Vec::new(),
            restricted_tools: Vec::new(),
            provider_id: None,
            model_id: None,
            token_budget: 1_000,
            cost_budget_micros: 0,
            time_limit_ms: 1_000,
            retry_limit: 1,
            output_contract: "summary".into(),
            parent_goal_id: Some(safe_goal.id.clone()),
            parent_plan_id: None,
            parent_plan_version: None,
            parent_task_id: None,
            parent_agent_id: "orchestrator".into(),
            memory_access_policy: "scoped".into(),
            recursion_depth: 0,
            status: SubagentStatus::Draft,
            schema_version: HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
        };

        let errors = [
            repository.add_profile_fact(&fact).expect_err("secret fact"),
            repository.save_memory(&memory).expect_err("secret memory"),
            repository
                .save_persona_version(&persona)
                .expect_err("secret persona"),
            repository.save_goal(&unsafe_goal).expect_err("secret goal"),
            repository.save_plan(&plan).expect_err("secret plan"),
            repository.save_task(&task).expect_err("secret task"),
            repository
                .save_improvement(&improvement)
                .expect_err("secret improvement"),
            repository
                .save_subagent_spec(&subagent)
                .expect_err("secret subagent"),
        ];
        for error in errors {
            assert!(matches!(error, NexusError::PolicyDenied(_)));
            assert!(!error.to_string().contains(secret));
        }

        repository
            .store()
            .with(|conn| {
                for table in [
                    "harness_profile_facts",
                    "harness_memories",
                    "harness_persona_versions",
                    "harness_plans",
                    "harness_tasks",
                    "harness_improvement_proposals",
                    "harness_subagent_specs",
                ] {
                    let count: i64 =
                        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                            row.get(0)
                        })?;
                    assert_eq!(count, 0, "{table} must remain empty");
                }
                let leaked: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM harness_goals WHERE payload_json LIKE ?1",
                    [format!("%{secret}%")],
                    |row| row.get(0),
                )?;
                assert_eq!(leaked, 0);
                Ok(())
            })
            .expect("inspect persistence");
    }

    #[test]
    fn checkpoint_file_hash_tracks_content_drift() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("tracked.txt");
        std::fs::write(&path, "original").expect("write");
        let first = checkpoint_file_hash(&path).expect("hash");
        assert_eq!(
            checkpoint_file_hash(&path).expect("hash"),
            first,
            "identical file must hash identically"
        );
        std::fs::write(&path, "modified").expect("write");
        assert_ne!(
            checkpoint_file_hash(&path).expect("hash"),
            first,
            "content drift must change the hash"
        );
        assert!(
            checkpoint_file_hash(&directory.path().join("missing.txt")).is_none(),
            "missing files hash to None"
        );
        assert!(
            checkpoint_file_hash(directory.path()).is_none(),
            "directories hash to None"
        );
    }

    #[test]
    fn background_writer_tasks_serialize_on_resource_claims() {
        let store = Store::open_in_memory().expect("in-memory store");
        let now = crate::now_rfc3339();
        store
            .with(|conn| {
                conn.execute(
                    "INSERT INTO sessions
                     (id,title,workspace,created_at,updated_at,model,agent,status)
                     VALUES ('s1','','/workspace',?1,?1,'mock','orchestrator','active')",
                    params![now],
                )?;
                for id in ["bg_first", "bg_second"] {
                    conn.execute(
                        "INSERT INTO background_tasks
                         (id,session_id,title,objective,writer,created_at,updated_at)
                         VALUES (?1,'s1','title','objective',1,?2,?2)",
                        params![id, now],
                    )?;
                }
                Ok(())
            })
            .expect("fixture rows");
        let repository = HarnessRepository::new(store);

        let claim = ResourceClaim::new(
            "bg_first",
            "git-repository",
            "/workspace",
            ResourceAccessMode::Write,
            None,
        );
        repository
            .claim_resource(&claim)
            .expect("first writer claim");

        let rival = ResourceClaim::new(
            "bg_second",
            "git-repository",
            "/workspace",
            ResourceAccessMode::Write,
            None,
        );
        assert!(
            repository.claim_resource(&rival).is_err(),
            "a second writer on the same repository must be denied"
        );

        repository
            .release_resource_claim(&claim.id)
            .expect("release");
        repository
            .claim_resource(&rival)
            .expect("claim after release");

        let ghost = ResourceClaim::new(
            "bg_missing",
            "git-repository",
            "/workspace",
            ResourceAccessMode::Write,
            None,
        );
        assert!(
            repository.claim_resource(&ghost).is_err(),
            "claims from unknown task ids are rejected"
        );
    }
    /// A row written before schema v2 has none of the RSI fields. It must still
    /// load — and it must land on the *conservative* defaults, not the
    /// permissive ones: an un-annotated candidate is code-plane and tier 3, so
    /// an upgrade can never turn old rows into auto-promotable ones.
    #[test]
    fn a_pre_v2_proposal_row_loads_with_conservative_defaults() {
        let repository = repository();
        let legacy = serde_json::json!({
            "id": "imp_legacy",
            "category": "tool",
            "problem": "repeated failures",
            "evidence": [],
            "proposed_change": "retry with backoff",
            "expected_benefit": "fewer failures",
            "risks": [],
            "required_permissions": [],
            "validation_plan": [],
            "rollback_plan": [],
            "status": "proposed",
            "approval_required": true,
            "measurements": {},
            "schema_version": 1,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "reviewed_at": null
        })
        .to_string();
        repository
            .store
            .with(|conn| {
                conn.execute(
                    "INSERT INTO harness_improvement_proposals
                     (id,category,status,approval_required,schema_version,payload_json,
                      created_at,updated_at,reviewed_at)
                     VALUES ('imp_legacy','tool','proposed',1,1,?1,
                             '2026-01-01T00:00:00Z','2026-01-01T00:00:00Z',NULL)",
                    params![legacy],
                )?;
                Ok(())
            })
            .expect("insert legacy row");

        let loaded = repository.improvement("imp_legacy").expect("load legacy");
        assert_eq!(loaded.target, ImprovementTarget::HarnessComponent);
        assert_eq!(loaded.target.plane(), ImprovementPlane::Code);
        assert_eq!(loaded.risk_tier, RiskTier::High);
        assert!(loaded.success_metrics.is_empty());
        assert!(loaded.created_by.is_empty());
        assert!(loaded.affected_components.is_empty());
    }

    /// Same direction for events: an old row has no severity, and absent
    /// severity means `info` rather than a missing field or a panic.
    #[test]
    fn a_pre_v2_event_row_loads_with_default_severity() {
        let legacy = serde_json::json!({
            "id": "evt_legacy",
            "event_type": "turn.completed",
            "timestamp": "2026-01-01T00:00:00Z",
            "session_id": "sess_1",
            "profile_id": null,
            "goal_id": null,
            "plan_id": null,
            "task_id": null,
            "agent_id": null,
            "subagent_id": null,
            "run_id": null,
            "summary": "turn finished",
            "metadata": {},
            "sensitivity": "normal",
            "schema_version": 1
        })
        .to_string();
        let event: HarnessEvent = serde_json::from_str(&legacy).expect("decode legacy event");
        assert_eq!(event.severity, "info");
        assert!(event.provider.is_none());
        assert!(event.candidate_id.is_none());
    }
    /// The downgrade direction: a payload written by 2.11.0 must still decode
    /// under an older binary's narrower struct. `ImprovementProposal` therefore
    /// must not be `deny_unknown_fields` — this test is what keeps that true.
    #[test]
    fn a_v2_payload_still_decodes_under_a_pre_v2_struct() {
        #[derive(serde::Deserialize)]
        struct LegacyProposal {
            id: String,
            status: ImprovementStatus,
            problem: String,
        }

        let mut proposal = ImprovementProposal::new(ImprovementCategory::Tool, "problem", "change")
            .expect("proposal");
        proposal.target = ImprovementTarget::ToolRouter;
        proposal.risk_tier = RiskTier::Moderate;
        proposal.created_by = "improvement_planner".into();
        let payload = serde_json::to_string(&proposal).expect("encode");

        let legacy: LegacyProposal = serde_json::from_str(&payload).expect("old binary decode");
        assert_eq!(legacy.id, proposal.id);
        assert_eq!(legacy.status, ImprovementStatus::Draft);
        assert_eq!(legacy.problem, "problem");
    }
}
