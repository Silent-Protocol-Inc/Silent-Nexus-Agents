//! Canonical application control plane for adaptive harness state.
//!
//! TUI menu controllers and slash-command compatibility handlers call this
//! facade rather than mutating UI files or domain tables directly. Global and
//! workspace records share the same repository implementation; routing is
//! based on the record's explicit scope.

use crate::app::App;
use nexus_core::harness::{
    ActiveHarnessContext, EvidenceReference, FactOutcome, Goal, GoalStatus as HarnessGoalStatus,
    HarnessRepository, IdentityConflictDecision, IdentityConflictResolution, IdentityResolution,
    MemoryRecord, MemoryScope, MemorySourceType, MemoryStatus, MemoryType, PersonaAssignment,
    PersonaSource, PersonaStatus, PersonaVersion, Plan, PlanAssumption, PlanPhase, PlanRisk,
    PlanStatus, ProfileFact, ProfileFactSource, ProfileFactStatus, Task,
    TaskStatus as HarnessTaskStatus, UserProfile, ValidationGate,
};
use nexus_core::{NexusError, Result};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub enum HarnessAction {
    EnsureContext {
        session_id: Option<String>,
    },
    ObserveUserMessage {
        session_id: String,
        text: String,
    },
    SelectProfile {
        session_id: Option<String>,
        profile_id: String,
    },
    SelectProfileName {
        session_id: Option<String>,
        display_name: String,
    },
    SelectPersona {
        session_id: Option<String>,
        persona_id: Option<String>,
        version: Option<u32>,
    },
    SelectAgent {
        session_id: Option<String>,
        agent_id: String,
    },
    SelectModel {
        session_id: Option<String>,
        provider_id: Option<String>,
        model_id: String,
    },
    ActivateWork {
        session_id: Option<String>,
        goal_id: Option<String>,
        plan_id: Option<String>,
        plan_version: Option<u32>,
        task_id: Option<String>,
    },
    SaveMemory(Box<MemoryRecord>),
    ReviewMemory {
        memory_id: String,
        scope: MemoryScope,
        approved: bool,
    },
}

#[derive(Debug, Clone)]
pub enum HarnessActionResult {
    Context(Box<ActiveHarnessContext>),
    Learning(Box<LearningOutcome>),
    Memory(String),
}

#[derive(Debug, Clone, Default)]
pub struct LearningOutcome {
    pub identity: Option<IdentityResolution>,
    pub memory_ids: Vec<String>,
    pub notices: Vec<String>,
}

pub struct HarnessControlPlane<'a> {
    app: &'a App,
    workspace: HarnessRepository,
    global: HarnessRepository,
}

impl<'a> HarnessControlPlane<'a> {
    pub fn new(app: &'a App) -> Self {
        Self {
            workspace: HarnessRepository::new(app.store.clone()),
            global: HarnessRepository::new(app.global_store.clone()),
            app,
        }
    }

    pub fn workspace_repository(&self) -> &HarnessRepository {
        &self.workspace
    }

    pub fn global_repository(&self) -> &HarnessRepository {
        &self.global
    }

    pub fn execute(&self, action: HarnessAction) -> Result<HarnessActionResult> {
        match action {
            HarnessAction::EnsureContext { session_id } => self
                .ensure_context(session_id.as_deref())
                .map(Box::new)
                .map(HarnessActionResult::Context),
            HarnessAction::ObserveUserMessage { session_id, text } => self
                .observe_user_message(&session_id, &text)
                .map(Box::new)
                .map(HarnessActionResult::Learning),
            HarnessAction::SelectProfile {
                session_id,
                profile_id,
            } => self
                .select_profile(session_id.as_deref(), &profile_id)
                .map(Box::new)
                .map(HarnessActionResult::Context),
            HarnessAction::SelectProfileName {
                session_id,
                display_name,
            } => {
                let profile = match self.global.profiles_named(&display_name)?.first().cloned() {
                    Some(profile) => profile,
                    None => {
                        let profile = UserProfile::new(display_name)?;
                        self.global.create_profile(&profile)?;
                        profile
                    }
                };
                self.select_profile(session_id.as_deref(), &profile.id)
                    .map(Box::new)
                    .map(HarnessActionResult::Context)
            }
            HarnessAction::SelectPersona {
                session_id,
                persona_id,
                version,
            } => {
                let mut context = self.ensure_context(session_id.as_deref())?;
                let selected = persona_id
                    .as_deref()
                    .map(|id| self.sync_persona(id))
                    .transpose()?;
                if let Some(persona) = selected.as_ref() {
                    if let Some(requested) = version {
                        if requested != persona.version {
                            return Err(NexusError::Config(format!(
                                "persona `{}` current version is {}, not {requested}",
                                persona.persona_id, persona.version
                            )));
                        }
                    }
                    self.assign_persona_for_context(session_id.as_deref(), persona)?;
                }
                context.persona_id = selected.as_ref().map(|persona| persona.persona_id.clone());
                context.persona_version = selected.as_ref().map(|persona| persona.version);
                self.persist_and_sync_context(context)
                    .map(Box::new)
                    .map(HarnessActionResult::Context)
            }
            HarnessAction::SelectAgent {
                session_id,
                agent_id,
            } => {
                let mut context = self.ensure_context(session_id.as_deref())?;
                context.agent_id = Some(agent_id);
                self.persist_and_sync_context(context)
                    .map(Box::new)
                    .map(HarnessActionResult::Context)
            }
            HarnessAction::SelectModel {
                session_id,
                provider_id,
                model_id,
            } => {
                let mut context = self.ensure_context(session_id.as_deref())?;
                context.provider_id = provider_id;
                context.model_id = Some(model_id);
                self.persist_and_sync_context(context)
                    .map(Box::new)
                    .map(HarnessActionResult::Context)
            }
            HarnessAction::ActivateWork {
                session_id,
                goal_id,
                plan_id,
                plan_version,
                task_id,
            } => {
                let mut context = self.ensure_context(session_id.as_deref())?;
                context.goal_id = goal_id;
                context.plan_id = plan_id;
                context.plan_version = plan_version;
                context.task_id = task_id;
                self.persist_and_sync_context(context)
                    .map(Box::new)
                    .map(HarnessActionResult::Context)
            }
            HarnessAction::SaveMemory(memory) => self
                .repository_for_scope(&memory.scope)
                .save_memory(memory.as_ref())
                .map(HarnessActionResult::Memory),
            HarnessAction::ReviewMemory {
                memory_id,
                scope,
                approved,
            } => {
                let repository = self.repository_for_scope(&scope);
                repository.memory_in_scopes(&memory_id, std::slice::from_ref(&scope))?;
                repository.set_memory_status(
                    &memory_id,
                    if approved {
                        MemoryStatus::Active
                    } else {
                        MemoryStatus::Rejected
                    },
                )?;
                Ok(HarnessActionResult::Memory(memory_id))
            }
        }
    }

    /// Resolve the profile id a `/profile` operation should act on. Prefers the
    /// active context's profile, but when a turn established the context before
    /// any profile was resolved (leaving `profile_id` null), it falls back to
    /// the canonical profile named by the operator's current selection —
    /// creating it if absent — instead of failing with "no active profile".
    /// It deliberately does not rewrite the turn context, so prompt composition
    /// (which treats a null `profile_id` as "use the legacy profile prompt") is
    /// left untouched.
    pub fn active_profile_id(&self, session_id: Option<&str>) -> Result<String> {
        if let Some(profile_id) = self.ensure_context(session_id)?.profile_id {
            return Ok(profile_id);
        }
        let legacy = self.app.read_ui_state(|state| state.profile_name.clone());
        let profile = match self.global.profiles_named(&legacy)?.first().cloned() {
            Some(profile) => profile,
            None => {
                let mut profile = UserProfile::new(&legacy)?;
                if legacy.eq_ignore_ascii_case("default") {
                    profile
                        .metadata
                        .insert("is_default".into(), serde_json::Value::Bool(true));
                }
                self.global.create_profile(&profile)?;
                profile
            }
        };
        Ok(profile.id)
    }

    /// The most recently updated real profile card, if the operator has one.
    fn most_recently_used_profile(&self) -> Result<Option<UserProfile>> {
        Ok(most_recent_real_card(self.global.profiles(false)?))
    }

    pub fn ensure_context(&self, session_id: Option<&str>) -> Result<ActiveHarnessContext> {
        if let Some(context) = self
            .workspace
            .active_context(&self.app.workspace_key, session_id)?
        {
            return Ok(context);
        }

        let legacy_profile = self.app.read_ui_state(|state| state.profile_name.clone());
        // `"default"` is the *absence* of a choice, not a choice, so it is
        // handled before any name lookup. Matching it by name first — which is
        // what this did — meant the inheritance below could only ever run on an
        // installation that had no default card at all, i.e. never after the
        // first launch. A second checkout therefore still started as nobody, and
        // wrote the operator's facts onto the anonymous card.
        //
        // An explicit choice is any other name, and is matched below.
        let unchosen = legacy_profile.eq_ignore_ascii_case("default");
        let inherited = if unchosen {
            self.most_recently_used_profile()?
        } else {
            None
        };
        let profile = match inherited {
            // Whoever the operator most recently was. Cards are global, and
            // being greeted by name in one directory and not the next reads as
            // the harness having forgotten.
            Some(profile) => profile,
            None => match self
                .global
                .profiles_named(&legacy_profile)?
                .first()
                .cloned()
            {
                Some(profile) => profile,
                None => {
                    let mut profile = UserProfile::new(&legacy_profile)?;
                    if unchosen {
                        profile
                            .metadata
                            .insert("is_default".into(), serde_json::Value::Bool(true));
                    }
                    self.global.create_profile(&profile)?;
                    profile
                }
            },
        };

        let mut context = ActiveHarnessContext::new(
            self.app.workspace_key.clone(),
            session_id.map(str::to_owned),
        );
        context.profile_id = Some(profile.id);
        context.persona_id = self
            .app
            .read_ui_state(|state| state.selected_persona.clone());
        context.agent_id = Some(self.app.active_agent());
        context.goal_id = self.app.read_ui_state(|state| state.active_goal.clone());
        context.model_id = Some(self.app.any_model_name());
        self.workspace.set_active_context(context)
    }

    pub fn observe_user_message(&self, session_id: &str, text: &str) -> Result<LearningOutcome> {
        let mut outcome = LearningOutcome::default();
        let mut context = self.ensure_context(Some(session_id))?;
        let source_ref = format!("session:{session_id}");

        if let Some(name) = explicit_name(text) {
            if self.app.redactor.redact(name) == name {
                let resolution = self.global.resolve_explicit_identity(
                    context.profile_id.as_deref(),
                    name,
                    Some(&source_ref),
                )?;
                match &resolution {
                    IdentityResolution::Created(profile)
                    | IdentityResolution::Activated(profile) => {
                        context.profile_id = Some(profile.id.clone());
                        self.persist_and_sync_context(context.clone())?;
                        let mut memory = MemoryRecord::new(
                            MemoryType::Episodic,
                            MemoryScope::profile(profile.id.clone()),
                            format!("The user explicitly identified as {}.", profile.display_name),
                            MemorySourceType::UserExplicit,
                        )?;
                        memory.status = MemoryStatus::Active;
                        memory.sensitivity = "normal".into();
                        memory.confidence = 1.0;
                        memory.importance = 0.9;
                        memory.source_refs.push(source_ref.clone());
                        let memory_id = self.global.save_memory(&memory)?;
                        outcome.memory_ids.push(memory_id);
                        outcome.notices.push(match &resolution {
                            IdentityResolution::Created(_) => format!(
                                "PROFILE CREATED · {} · selected as active",
                                profile.display_name
                            ),
                            _ => format!("PROFILE SELECTED · {}", profile.display_name),
                        });
                    }
                    IdentityResolution::Conflict(conflict) => outcome.notices.push(format!(
                        "IDENTITY CONFLICT · kept current profile unchanged · resolution {} is pending",
                        conflict.id
                    )),
                }
                outcome.identity = Some(resolution);
            }
        }

        // Durable attributes beyond the name: occupation, timezone, language,
        // stated preferences, tooling. These used to have no capture path at
        // all — the only automatic extractor wrote them to the legacy trait
        // table, which the prompt stopped reading once a canonical card
        // existed, so everything SNX "learned" was invisible to the model.
        if self.app.config.profile.auto_capture {
            for candidate in crate::profile_capture::detect(text, &self.app.redactor) {
                if !self.app.config.profile.capture_preferences
                    && candidate.key.starts_with("preferences.")
                {
                    continue;
                }
                let sensitive = candidate.sensitivity != "normal";
                if sensitive && !self.app.config.profile.require_review_for_sensitive {
                    continue;
                }
                match self.record_profile_fact(
                    Some(session_id),
                    context.profile_id.as_deref(),
                    candidate.key,
                    &candidate.value,
                    candidate.explicit,
                    candidate.sensitivity,
                ) {
                    Ok((_, FactOutcome::Unchanged { .. })) => {}
                    Ok((_, FactOutcome::Created { .. })) if sensitive => {
                        outcome.notices.push(format!(
                            "PROFILE REVIEW · {} · awaiting your approval",
                            candidate.key
                        ));
                    }
                    Ok((_, FactOutcome::Created { .. })) => outcome
                        .notices
                        .push(format!("PROFILE UPDATED · {} recorded", candidate.key)),
                    Ok((_, FactOutcome::Updated { .. })) => outcome.notices.push(format!(
                        "PROFILE UPDATED · {} replaces the previous value",
                        candidate.key
                    )),
                    // A fact that cannot be stored is not worth failing the
                    // turn over — the operator asked a question, not for a
                    // profile write. It is reported, never swallowed.
                    Err(error) => outcome
                        .notices
                        .push(format!("PROFILE NOT STORED · {} · {error}", candidate.key)),
                }
            }
        }

        if let Some(content) = explicit_memory(text) {
            if self.app.redactor.redact(content) == content {
                let scope = MemoryScope {
                    profile_id: context.profile_id.clone(),
                    workspace_id: Some(self.app.workspace_key.clone()),
                    ..MemoryScope::default()
                };
                let mut memory = MemoryRecord::new(
                    MemoryType::Semantic,
                    scope.clone(),
                    content,
                    MemorySourceType::UserExplicit,
                )?;
                let legacy_id = self.app.memory().add(nexus_memory::NewMemory {
                    kind: nexus_memory::MemoryKind::ProjectFact,
                    content: content.to_string(),
                    source: source_ref.clone(),
                    confidence: 1.0,
                    scope: "project".into(),
                    sensitivity: "normal".into(),
                    requires_approval: false,
                    ttl_days: None,
                })?;
                memory.id = legacy_id.as_str().to_string();
                memory.status = MemoryStatus::Active;
                memory.sensitivity = "normal".into();
                memory.confidence = 1.0;
                memory.importance = 0.8;
                memory.source_refs.push(source_ref);
                memory
                    .source_refs
                    .push(format!("legacy-memory:{}", legacy_id.as_str()));
                let id = self.repository_for_scope(&scope).save_memory(&memory)?;
                outcome.memory_ids.push(id);
                outcome
                    .notices
                    .push("MEMORY SAVED · explicit durable request".into());
            }
        }

        Ok(outcome)
    }

    pub fn select_profile(
        &self,
        session_id: Option<&str>,
        profile_id: &str,
    ) -> Result<ActiveHarnessContext> {
        let profile = self.global.profile(profile_id)?;
        if matches!(
            profile.status,
            nexus_core::harness::ProfileStatus::Archived
                | nexus_core::harness::ProfileStatus::Deleted
        ) {
            return Err(NexusError::PolicyDenied(format!(
                "profile `{profile_id}` is not selectable"
            )));
        }
        let mut context = self.ensure_context(session_id)?;
        context.profile_id = Some(profile_id.to_string());
        self.persist_and_sync_context(context)
    }

    fn persist_and_sync_context(
        &self,
        context: ActiveHarnessContext,
    ) -> Result<ActiveHarnessContext> {
        let context = self.workspace.set_active_context(context)?;
        let profile_name = context
            .profile_id
            .as_deref()
            .and_then(|id| self.global.profile(id).ok())
            .map(|profile| profile.display_name)
            .unwrap_or_else(|| "default".into());
        let persona_id = context.persona_id.clone();
        let agent_id = context.agent_id.clone();
        let model_id = context.model_id.clone();
        let goal_id = context.goal_id.clone();
        let profile_name_for_state = profile_name.clone();
        self.app.update_ui_state(|state| {
            state.profile_name = profile_name_for_state;
            state.selected_persona = persona_id;
            state.active_agent = agent_id;
            state.active_model = model_id;
            state.active_goal = goal_id;
        })?;
        if let Some(session_id) = context.session_id.as_deref() {
            if self.app.sessions().get(session_id).is_ok() {
                self.app.sessions().set_persona_profile(
                    session_id,
                    context.persona_id.as_deref(),
                    &profile_name,
                )?;
                if let Some(agent) = context.agent_id.as_deref() {
                    self.app.sessions().set_agent(session_id, agent)?;
                }
                if let Some(model) = context.model_id.as_deref() {
                    self.app.sessions().set_model(session_id, model)?;
                }
            }
        }
        Ok(context)
    }

    fn repository_for_scope(&self, scope: &MemoryScope) -> &HarnessRepository {
        if scope.global
            || (scope.profile_id.is_some()
                && scope.workspace_id.is_none()
                && scope.project_id.is_none()
                && scope.session_id.is_none()
                && scope.goal_id.is_none()
                && scope.plan_id.is_none()
                && scope.task_id.is_none()
                && scope.agent_id.is_none())
        {
            &self.global
        } else {
            &self.workspace
        }
    }

    /// All exact memory scopes visible to the active identity and work
    /// context. A scope is never broadened by similarity: global/profile
    /// records are queried in the global store and workspace/work records in
    /// the workspace store.
    pub fn active_memory_scopes(
        &self,
        session_id: Option<&str>,
    ) -> Result<(Vec<MemoryScope>, Vec<MemoryScope>)> {
        let context = self.ensure_context(session_id)?;
        nexus_core::harness::authorized_memory_scopes(
            &context,
            self.app.config.memory.global_enabled,
        )
    }

    /// Import 1.0 memory rows into the canonical schema without deleting or
    /// rewriting the legacy row used by the existing prompt composer.
    pub fn import_legacy_memories(&self) -> Result<()> {
        let (global_scopes, workspace_scopes) = self.active_memory_scopes(None)?;
        for legacy in self.app.memory().list(true, 10_000)? {
            match self
                .workspace
                .memory_in_scopes(legacy.id.as_str(), &workspace_scopes)
            {
                Ok(_) | Err(NexusError::PolicyDenied(_)) => continue,
                Err(NexusError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
            match self
                .global
                .memory_in_scopes(legacy.id.as_str(), &global_scopes)
            {
                Ok(_) | Err(NexusError::PolicyDenied(_)) => continue,
                Err(NexusError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
            let safe_global = matches!(
                legacy.sensitivity.to_ascii_lowercase().as_str(),
                "public" | "non_sensitive" | "system"
            );
            let downgraded_global = legacy.scope == "global" && !safe_global;
            let scope = if legacy.scope == "global" && safe_global {
                MemoryScope::global()
            } else {
                MemoryScope::workspace(self.app.workspace_key.clone())
            };
            let mut memory = MemoryRecord::new(
                legacy_memory_type(legacy.kind),
                scope.clone(),
                legacy.content,
                MemorySourceType::Imported,
            )?;
            memory.id = legacy.id.as_str().to_string();
            memory.source_refs = vec![
                format!("legacy-memory:{}", legacy.id.as_str()),
                legacy.source,
            ];
            memory.sensitivity = legacy.sensitivity;
            if downgraded_global {
                memory.tags.push("legacy-global-scope-downgraded".into());
            }
            memory.confidence = legacy.confidence.clamp(0.0, 1.0);
            memory.importance = 0.5;
            memory.status = if legacy.approved {
                MemoryStatus::Active
            } else {
                MemoryStatus::Candidate
            };
            memory.created_at = legacy.created_at;
            memory.updated_at = legacy
                .verified_at
                .clone()
                .unwrap_or_else(|| memory.created_at.clone());
            memory.last_accessed_at = legacy.verified_at;
            memory.expires_at = legacy.expires_at;
            self.repository_for_scope(&scope).save_memory(&memory)?;
        }
        Ok(())
    }

    pub fn memories(
        &self,
        session_id: Option<&str>,
        query: Option<&str>,
        include_inactive: bool,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>> {
        self.import_legacy_memories()?;
        let (global_scopes, workspace_scopes) = self.active_memory_scopes(session_id)?;
        let mut records = if let Some(query) = query {
            let mut records =
                self.workspace
                    .query_memories(&workspace_scopes, Some(query), limit)?;
            if records.len() < limit {
                records.extend(self.global.query_memories(
                    &global_scopes,
                    Some(query),
                    limit - records.len(),
                )?);
            }
            records
        } else {
            let mut records =
                self.workspace
                    .list_memories(&workspace_scopes, include_inactive, limit)?;
            if records.len() < limit {
                records.extend(self.global.list_memories(
                    &global_scopes,
                    include_inactive,
                    limit - records.len(),
                )?);
            }
            records
        };
        let mut seen = HashSet::new();
        records.retain(|record| seen.insert(record.id.clone()));
        records.sort_by(|left, right| {
            right
                .importance
                .partial_cmp(&left.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.updated_at.cmp(&left.updated_at))
        });
        records.truncate(limit);
        Ok(records)
    }

    pub fn memory(&self, session_id: Option<&str>, memory_id: &str) -> Result<MemoryRecord> {
        self.import_legacy_memories()?;
        let (global_scopes, workspace_scopes) = self.active_memory_scopes(session_id)?;
        let memory = match self
            .workspace
            .memory_in_scopes(memory_id, &workspace_scopes)
        {
            Ok(memory) => memory,
            Err(NexusError::NotFound(_)) => {
                self.global.memory_in_scopes(memory_id, &global_scopes)?
            }
            Err(error) => return Err(error),
        };
        // Forgotten rows stay on disk so the legacy-import dedup can see them,
        // but they must never be retrievable by id after the operator deletes.
        if memory.status == MemoryStatus::Deleted {
            return Err(NexusError::NotFound(format!("memory `{memory_id}`")));
        }
        Ok(memory)
    }

    /// Store an explicit operator memory in both representations. The
    /// canonical row reuses the 1.0 id so approval/deletion remains coherent
    /// while the old prompt composer continues to consume approved rows.
    pub fn save_operator_memory(
        &self,
        session_id: Option<&str>,
        content: &str,
        source: &str,
    ) -> Result<String> {
        if content.trim().is_empty() {
            return Err(NexusError::Config("memory content is required".into()));
        }
        if self.app.redactor.redact(content) != content {
            return Err(NexusError::PolicyDenied(
                "refusing to store memory: content appears to contain a secret".into(),
            ));
        }
        let context = self.ensure_context(session_id)?;
        let scope = MemoryScope {
            profile_id: context.profile_id,
            workspace_id: Some(self.app.workspace_key.clone()),
            ..MemoryScope::default()
        };
        let legacy_id = self.app.memory().add(nexus_memory::NewMemory {
            kind: nexus_memory::MemoryKind::ProjectFact,
            content: content.trim().to_string(),
            source: source.to_string(),
            confidence: 1.0,
            scope: "project".into(),
            sensitivity: "normal".into(),
            requires_approval: false,
            ttl_days: None,
        })?;
        let mut memory = MemoryRecord::new(
            MemoryType::Semantic,
            scope,
            content.trim(),
            MemorySourceType::UserConfirmed,
        )?;
        memory.id = legacy_id.as_str().to_string();
        memory.status = MemoryStatus::Active;
        memory.sensitivity = "normal".into();
        memory.confidence = 1.0;
        memory.importance = 0.8;
        memory.source_refs = vec![
            format!("legacy-memory:{}", legacy_id.as_str()),
            source.into(),
        ];
        let id = self.workspace.save_memory(&memory)?;
        Ok(id)
    }

    pub fn set_memory_status_for_context(
        &self,
        session_id: Option<&str>,
        memory_id: &str,
        status: MemoryStatus,
    ) -> Result<()> {
        let memory = self.memory(session_id, memory_id)?;
        self.repository_for_scope(&memory.scope)
            .set_memory_status(memory_id, status)?;
        // Preserve the existing prompt store when this row was imported or
        // created through the compatibility adapter. Candidate rejection and
        // deletion remove it from legacy retrieval; canonical provenance is
        // retained as a soft lifecycle state.
        if self.app.memory().get(memory_id).is_ok() {
            match status {
                MemoryStatus::Active => self.app.memory().approve(memory_id)?,
                MemoryStatus::Rejected | MemoryStatus::Deleted => {
                    self.app.memory().forget(memory_id)?
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Every canonical persona revision visible to this workspace.
    pub fn workspace_persona_versions(&self) -> Result<Vec<PersonaVersion>> {
        self.workspace.persona_versions(None, None)
    }

    pub fn sync_persona(&self, id_or_name: &str) -> Result<PersonaVersion> {
        self.sync_persona_with(id_or_name, None)
    }

    /// Mirror the editable persona record into an immutable canonical revision.
    ///
    /// `metadata` carries the fields the editable record has no column for
    /// (content profile, base, recommendations, …). When it is `None` the
    /// latest revision's metadata is carried forward — otherwise editing a
    /// persona's text would silently reset it to `General` with no base, which
    /// is exactly the kind of quiet downgrade the persona rules forbid.
    pub fn sync_persona_with(
        &self,
        id_or_name: &str,
        metadata: Option<PersonaMetadata>,
    ) -> Result<PersonaVersion> {
        let legacy = self.app.personas().get(id_or_name)?;
        let prompt = self.app.personas().resolved_instructions(&legacy.id)?;
        let versions = self.workspace.persona_versions(None, None)?;
        let latest = versions
            .iter()
            .filter(|version| version.persona_id == legacy.id)
            .max_by_key(|version| version.version);
        let metadata =
            metadata.unwrap_or_else(|| latest.map(PersonaMetadata::from).unwrap_or_default());
        if let Some(existing) = versions.iter().find(|version| {
            version.persona_id == legacy.id
                && version.name == legacy.name
                && version.description == legacy.description
                && version.system_prompt == prompt
                && version.scope_kind == persona_scope_kind(&legacy.scope)
                && metadata.matches(version)
        }) {
            return Ok(existing.clone());
        }
        let next_version = versions
            .iter()
            .filter(|version| version.persona_id == legacy.id)
            .map(|version| version.version)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let mut persona = PersonaVersion::first(&legacy.name, prompt)?;
        persona.persona_id = legacy.id;
        persona.version = next_version;
        persona.description = legacy.description;
        persona.source = PersonaSource::UserCreated;
        persona.scope_kind = persona_scope_kind(&legacy.scope).into();
        persona.scope_key = if legacy.scope == "global" {
            String::new()
        } else {
            self.app.workspace_key.clone()
        };
        persona.status = PersonaStatus::Active;
        persona.created_at = legacy.created_at;
        persona.updated_at = legacy.updated_at;
        metadata.apply(&mut persona);
        self.workspace.save_persona_version(&persona)?;
        Ok(persona)
    }

    pub fn assign_persona_for_context(
        &self,
        session_id: Option<&str>,
        persona: &PersonaVersion,
    ) -> Result<()> {
        let now = nexus_core::now_rfc3339();
        self.workspace.assign_persona(&PersonaAssignment {
            id: format!("persona_assignment_{}", uuid::Uuid::new_v4().simple()),
            persona_id: persona.persona_id.clone(),
            persona_version: persona.version,
            target_kind: if session_id.is_some() {
                "session".into()
            } else {
                "workspace".into()
            },
            target_id: session_id
                .map(str::to_string)
                .unwrap_or_else(|| self.app.workspace_key.clone()),
            status: "active".into(),
            precedence: 60,
            schema_version: nexus_core::harness::HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn sync_legacy_goal(&self, goal_id: &str) -> Result<Goal> {
        let legacy = self.app.goals().get(goal_id)?;
        let context = self.ensure_context(legacy.session_id.as_ref().map(|id| id.as_str()))?;
        let mut goal = Goal::new(&legacy.objective, &legacy.workspace)?;
        goal.id = legacy.id.as_str().to_string();
        goal.success_criteria = legacy.acceptance_criteria.clone();
        goal.constraints = legacy.constraints.clone();
        goal.scope = legacy
            .allowed_paths
            .iter()
            .map(|path| format!("allow:{path}"))
            .chain(
                legacy
                    .prohibited_paths
                    .iter()
                    .map(|path| format!("deny:{path}")),
            )
            .collect();
        goal.owner_profile_id = context.profile_id;
        goal.selected_agent_id = context.agent_id;
        goal.selected_persona_id = context.persona_id;
        goal.status = harness_goal_status(legacy.status);
        goal.risks = legacy.blockers.clone();
        goal.created_at = legacy.created_at;
        goal.updated_at = legacy.updated_at;
        for step in self.app.goals().steps(goal_id)? {
            for evidence in step.evidence {
                if let Some(criterion) = goal.success_criteria.get(evidence.criterion_index) {
                    goal.validation_evidence.push(EvidenceReference {
                        criterion: criterion.clone(),
                        summary: evidence.description,
                        source_ref: evidence.artifact_id.unwrap_or(evidence.source_tool),
                        passed: evidence.passed,
                        observed_at: evidence.recorded_at,
                    });
                }
            }
        }
        self.workspace.save_goal(&goal)?;
        Ok(goal)
    }

    /// Bridge a durable 1.0 work breakdown into a versioned canonical plan
    /// and phase task graph. Existing plan versions and completed task records
    /// remain immutable across revisions.
    pub fn sync_work_breakdown(
        &self,
        session_id: &str,
        work: &nexus_core::orchestration::WorkBreakdown,
    ) -> Result<Plan> {
        let goal_id = match self
            .app
            .sessions()
            .get(session_id)
            .ok()
            .and_then(|session| session.current_goal)
            .or_else(|| crate::services::active_goal_id(self.app))
        {
            Some(goal_id) => goal_id,
            None => crate::services::goal_fast_create(self.app, &work.objective)?,
        };
        crate::services::attach_goal_to_session(self.app, &goal_id, session_id)?;
        let goal = self.sync_legacy_goal(&goal_id)?;
        if let Ok(existing) = self.workspace.plan(work.id.as_str(), work.version) {
            if work.approved
                && matches!(
                    existing.status,
                    PlanStatus::Draft
                        | PlanStatus::Analyzing
                        | PlanStatus::Proposed
                        | PlanStatus::UnderReview
                        | PlanStatus::NeedsRevision
                )
            {
                self.workspace
                    .approve_plan(work.id.as_str(), work.version)?;
            }
            let mut context = self.ensure_context(Some(session_id))?;
            context.goal_id = Some(goal_id);
            context.plan_id = Some(work.id.as_str().to_string());
            context.plan_version = Some(work.version);
            self.persist_and_sync_context(context)?;
            return self.workspace.plan(work.id.as_str(), work.version);
        }

        let mut plan = Plan::new(&goal_id, plan_title(&work.objective))?;
        plan.id = work.id.as_str().to_string();
        plan.version = work.version;
        plan.summary = work.objective.clone();
        plan.status = PlanStatus::Proposed;
        plan.assumptions = work
            .rationale
            .iter()
            .enumerate()
            .map(|(index, rationale)| PlanAssumption {
                id: format!("assumption-{}-{index}", work.id.as_str()),
                statement: rationale.clone(),
                verified: false,
            })
            .collect();
        plan.constraints = goal.constraints.clone();
        plan.phases = work
            .stages
            .iter()
            .map(|stage| PlanPhase {
                id: stage.id.clone(),
                title: stage.title.clone(),
                summary: stage.description.clone(),
                milestones: stage.evidence.clone(),
                status: stage.status.as_str().into(),
            })
            .collect();
        plan.risks = work
            .rationale
            .iter()
            .filter(|item| {
                let item = item.to_ascii_lowercase();
                item.contains("risk") || item.contains("write") || item.contains("external")
            })
            .enumerate()
            .map(|(index, description)| PlanRisk {
                id: format!("risk-{}-{index}", work.id.as_str()),
                description: description.clone(),
                likelihood: "bounded".into(),
                impact: "review required".into(),
                mitigation: "Enforce the plan approval and tool permission gates.".into(),
            })
            .collect();
        plan.validation_gates = vec![ValidationGate {
            id: format!("validation-{}-{}", work.id.as_str(), work.version),
            description: "Validate task outputs and attach evidence before completion.".into(),
            required_evidence: goal.success_criteria.clone(),
            passed: false,
        }];
        plan.rollback_strategy = Some(
            "Pause execution, preserve completed tasks and artifacts, and resume the previous approved plan version."
                .into(),
        );
        if let Some(agent) = self.ensure_context(Some(session_id))?.agent_id {
            plan.assigned_agent_ids.push(agent);
        }
        plan.created_at = work.created_at.clone();
        plan.updated_at = work.updated_at.clone();
        self.workspace.save_plan(&plan)?;

        // A revised plan supersedes only unfinished phase tasks. Completed
        // work is retained verbatim and never silently discarded.
        if work.version > 1 {
            for previous in self
                .workspace
                .plan_tasks(work.id.as_str(), work.version - 1)?
            {
                if !matches!(
                    previous.status,
                    HarnessTaskStatus::Completed | HarnessTaskStatus::Cancelled
                ) {
                    let mut superseded = previous;
                    superseded.status = HarnessTaskStatus::Superseded;
                    superseded.updated_at = nexus_core::now_rfc3339();
                    self.workspace.save_task(&superseded)?;
                }
            }
        }

        let mut phase_task_ids = Vec::new();
        for stage in &work.stages {
            let mut task = Task::new(&stage.title, &stage.description)?;
            task.id = format!(
                "plan-task-{}-v{}-{}",
                work.id.as_str(),
                work.version,
                stage.id
            );
            task.goal_id = Some(goal_id.clone());
            task.plan_id = Some(work.id.as_str().to_string());
            task.plan_version = Some(work.version);
            task.phase_id = Some(stage.id.clone());
            task.status = harness_stage_status(stage.status);
            let passing_validation = stage
                .validation
                .iter()
                .filter(|validation| {
                    validation.status == nexus_core::orchestration::StageStatus::Completed
                })
                .collect::<Vec<_>>();
            if stage.status == nexus_core::orchestration::StageStatus::Completed {
                if passing_validation.is_empty() {
                    // A legacy stage completion is useful progress, but it is
                    // not canonical acceptance evidence by itself.
                    task.status = HarnessTaskStatus::Validating;
                    task.acceptance_criteria = vec![stage.description.clone()];
                } else {
                    task.acceptance_criteria = passing_validation
                        .iter()
                        .map(|validation| validation.label.clone())
                        .collect();
                    task.validation_evidence = passing_validation
                        .iter()
                        .map(|validation| EvidenceReference {
                            criterion: validation.label.clone(),
                            summary: validation.summary.clone(),
                            source_ref: validation
                                .artifact_id
                                .clone()
                                .or_else(|| validation.command.clone())
                                .unwrap_or_else(|| {
                                    format!("plan:{}:phase:{}", work.id.as_str(), stage.id)
                                }),
                            passed: true,
                            observed_at: validation.at.clone(),
                        })
                        .collect();
                }
            } else {
                task.acceptance_criteria = if stage.title.eq_ignore_ascii_case("validation") {
                    goal.success_criteria.clone()
                } else {
                    vec![stage.description.clone()]
                };
            }
            task.created_at = work.created_at.clone();
            task.updated_at = work.updated_at.clone();
            self.workspace.save_task(&task)?;
            phase_task_ids.push(task.id);
        }
        for pair in phase_task_ids.windows(2) {
            self.workspace.add_task_dependency(
                work.id.as_str(),
                work.version,
                &pair[0],
                &pair[1],
            )?;
        }
        if work.approved {
            self.workspace
                .approve_plan(work.id.as_str(), work.version)?;
        }
        let mut context = self.ensure_context(Some(session_id))?;
        context.goal_id = Some(goal_id);
        context.plan_id = Some(work.id.as_str().to_string());
        context.plan_version = Some(work.version);
        self.persist_and_sync_context(context)?;
        self.workspace.plan(work.id.as_str(), work.version)
    }

    pub fn sync_background_task(
        &self,
        task: &nexus_core::orchestration::BackgroundTask,
    ) -> Result<Task> {
        let context = self.ensure_context(Some(task.session_id.as_str()))?;
        let plan = if let Some(plan_id) = task.plan_id.as_deref() {
            let work = self.app.orchestration().plan(plan_id, None)?;
            self.sync_work_breakdown(task.session_id.as_str(), &work)?;
            Some((plan_id.to_string(), work.version))
        } else {
            None
        };
        let goal_id = context.goal_id.clone();
        if let Some(goal_id) = goal_id.as_deref() {
            self.sync_legacy_goal(goal_id)?;
        }
        let mut canonical = Task::new(&task.title, &task.objective)?;
        canonical.id = task.id.as_str().to_string();
        canonical.goal_id = goal_id;
        canonical.plan_id = plan.as_ref().map(|(id, _)| id.clone());
        canonical.plan_version = plan.map(|(_, version)| version);
        canonical.phase_id = task.stage_id.clone();
        canonical.status = harness_background_task_status(task.status);
        canonical.assigned_agent_id = Some(task.owner.clone());
        canonical.allowed_tools = if task.writer {
            vec!["workspace.read".into(), "workspace.write".into()]
        } else {
            vec!["workspace.read".into()]
        };
        if !task.writer {
            canonical.restricted_tools.push("workspace.write".into());
        }
        canonical.attempt_count = task.attempts;
        canonical.max_attempts = 3;
        if let Some(result) = &task.result {
            canonical.artifact_refs = result.artifact_ids.clone();
            canonical.acceptance_criteria = result.evidence.clone();
            canonical.validation_evidence = result
                .evidence
                .iter()
                .map(|evidence| EvidenceReference {
                    criterion: evidence.clone(),
                    summary: evidence.clone(),
                    source_ref: format!("task-result:{}", task.id.as_str()),
                    passed: true,
                    observed_at: task
                        .finished_at
                        .clone()
                        .unwrap_or_else(|| task.updated_at.clone()),
                })
                .collect();
        }
        if canonical.status == HarnessTaskStatus::Completed
            && canonical.validation_evidence.is_empty()
        {
            canonical.status = HarnessTaskStatus::Validating;
        }
        canonical.created_at = task.created_at.clone();
        canonical.updated_at = task.updated_at.clone();
        self.workspace.save_task(&canonical)?;
        Ok(canonical)
    }

    pub fn add_profile_fact(
        &self,
        session_id: Option<&str>,
        key: &str,
        value: &str,
        explicit: bool,
    ) -> Result<ProfileFact> {
        self.record_profile_fact(session_id, None, key, value, explicit, "normal")
            .map(|(fact, _)| fact)
    }

    /// Store one fact on the active card, reconciled against what it already
    /// says.
    ///
    /// Returns the fact together with what recording it actually did, because
    /// "stored", "replaced an older answer", and "the card already said that"
    /// are three different things to tell the operator, and claiming the first
    /// when the third happened is the kind of quiet inaccuracy that makes the
    /// whole feature untrustworthy.
    pub fn record_profile_fact(
        &self,
        session_id: Option<&str>,
        profile_id: Option<&str>,
        key: &str,
        value: &str,
        explicit: bool,
        sensitivity: &str,
    ) -> Result<(ProfileFact, nexus_core::harness::FactOutcome)> {
        let profile_id = match profile_id {
            Some(id) => id.to_string(),
            None => self
                .ensure_context(session_id)?
                .profile_id
                .ok_or_else(|| NexusError::NotFound("no active profile".into()))?,
        };
        if self.app.redactor.redact(value) != value {
            return Err(NexusError::PolicyDenied(
                "refusing to store profile fact: value appears to contain a secret".into(),
            ));
        }
        let now = nexus_core::now_rfc3339();
        // Anything the operator did not state outright, or that falls in a
        // sensitive category, lands as a candidate: visible in `/profile`
        // immediately, and not handed to the model until a human approves it.
        let reviewed = !explicit || sensitivity != "normal";
        let fact = ProfileFact {
            id: format!("pfact_{}", uuid::Uuid::new_v4().simple()),
            profile_id,
            key: key.trim().to_string(),
            value: serde_json::Value::String(value.trim().to_string()),
            source_type: if explicit {
                ProfileFactSource::UserExplicit
            } else {
                ProfileFactSource::Imported
            },
            source_ref: session_id.map(|id| format!("session:{id}")),
            confidence: if explicit { 1.0 } else { 0.6 },
            sensitivity: sensitivity.to_string(),
            status: if reviewed {
                ProfileFactStatus::Candidate
            } else {
                ProfileFactStatus::Active
            },
            schema_version: nexus_core::harness::HARNESS_SCHEMA_VERSION,
            created_at: now.clone(),
            updated_at: now,
            expires_at: None,
            superseded_by: None,
        };
        let outcome = self.global.record_profile_fact(&fact)?;
        Ok((fact, outcome))
    }

    pub fn resolve_identity_conflict(
        &self,
        session_id: Option<&str>,
        conflict_id: &str,
        decision: IdentityConflictDecision,
    ) -> Result<IdentityConflictResolution> {
        let resolution = self
            .global
            .resolve_identity_conflict(conflict_id, decision)?;
        if let Some(profile) = resolution.selected_profile.as_ref() {
            self.select_profile(session_id, &profile.id)?;
        }
        Ok(resolution)
    }
}

fn legacy_memory_type(kind: nexus_memory::MemoryKind) -> MemoryType {
    match kind {
        nexus_memory::MemoryKind::Session => MemoryType::Session,
        nexus_memory::MemoryKind::Procedure | nexus_memory::MemoryKind::SkillRef => {
            MemoryType::Procedural
        }
        nexus_memory::MemoryKind::GoalHistory | nexus_memory::MemoryKind::ArtifactRef => {
            MemoryType::Episodic
        }
        _ => MemoryType::Semantic,
    }
}

/// Persona fields the editable record has no column for.
///
/// Metadata only: none of it is consulted when the prompt is built, so no value
/// here can add, remove, or reword a single character of persona text.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PersonaMetadata {
    pub content_profile: nexus_core::persona::ContentProfile,
    pub category: String,
    pub base_persona_id: Option<String>,
    pub inheritance_mode: nexus_core::persona::InheritanceMode,
    pub persistence_policy: nexus_core::persona::PersistencePolicy,
    pub enabled: bool,
    pub compatibility_notes: String,
    pub recommended_providers: Vec<String>,
    pub recommended_models: Vec<String>,
    pub recommended_agents: Vec<String>,
    pub adult_acknowledgment: Option<String>,
}

impl PersonaMetadata {
    /// A brand-new persona's metadata. `Default` cannot serve here: it would
    /// leave `enabled` false and the persona would never reach a prompt.
    pub fn new() -> Self {
        Self {
            enabled: true,
            ..Self::default()
        }
    }

    fn matches(&self, version: &PersonaVersion) -> bool {
        self == &Self::from(version)
    }

    fn apply(&self, version: &mut PersonaVersion) {
        version.content_profile = self.content_profile;
        version.category.clone_from(&self.category);
        version.base_persona_id.clone_from(&self.base_persona_id);
        version.inheritance_mode = self.inheritance_mode;
        version.persistence_policy = self.persistence_policy;
        version.enabled = self.enabled;
        version
            .compatibility_notes
            .clone_from(&self.compatibility_notes);
        version
            .recommended_providers
            .clone_from(&self.recommended_providers);
        version
            .recommended_models
            .clone_from(&self.recommended_models);
        version
            .recommended_agents
            .clone_from(&self.recommended_agents);
        version
            .adult_acknowledgment
            .clone_from(&self.adult_acknowledgment);
    }
}

impl From<&PersonaVersion> for PersonaMetadata {
    fn from(version: &PersonaVersion) -> Self {
        Self {
            content_profile: version.content_profile,
            category: version.category.clone(),
            base_persona_id: version.base_persona_id.clone(),
            inheritance_mode: version.inheritance_mode,
            persistence_policy: version.persistence_policy,
            enabled: version.enabled,
            compatibility_notes: version.compatibility_notes.clone(),
            recommended_providers: version.recommended_providers.clone(),
            recommended_models: version.recommended_models.clone(),
            recommended_agents: version.recommended_agents.clone(),
            adult_acknowledgment: version.adult_acknowledgment.clone(),
        }
    }
}

fn persona_scope_kind(scope: &str) -> &'static str {
    if scope == "global" {
        "global"
    } else {
        "workspace"
    }
}

fn harness_goal_status(status: nexus_goals::GoalStatus) -> HarnessGoalStatus {
    match status {
        nexus_goals::GoalStatus::Draft => HarnessGoalStatus::Draft,
        nexus_goals::GoalStatus::Planned => HarnessGoalStatus::Defined,
        nexus_goals::GoalStatus::Running => HarnessGoalStatus::Active,
        nexus_goals::GoalStatus::WaitingApproval => HarnessGoalStatus::Planning,
        nexus_goals::GoalStatus::Blocked => HarnessGoalStatus::Blocked,
        nexus_goals::GoalStatus::Paused => HarnessGoalStatus::Paused,
        nexus_goals::GoalStatus::Verifying => HarnessGoalStatus::Validating,
        nexus_goals::GoalStatus::Completed => HarnessGoalStatus::Completed,
        nexus_goals::GoalStatus::Failed => HarnessGoalStatus::Failed,
        nexus_goals::GoalStatus::Cancelled => HarnessGoalStatus::Cancelled,
    }
}

fn harness_stage_status(status: nexus_core::orchestration::StageStatus) -> HarnessTaskStatus {
    match status {
        nexus_core::orchestration::StageStatus::Pending => HarnessTaskStatus::Pending,
        nexus_core::orchestration::StageStatus::Running => HarnessTaskStatus::Running,
        nexus_core::orchestration::StageStatus::Blocked => HarnessTaskStatus::Blocked,
        nexus_core::orchestration::StageStatus::Completed => HarnessTaskStatus::Completed,
        nexus_core::orchestration::StageStatus::Skipped => HarnessTaskStatus::Superseded,
        nexus_core::orchestration::StageStatus::Failed => HarnessTaskStatus::Failed,
    }
}

fn harness_background_task_status(
    status: nexus_core::orchestration::TaskStatus,
) -> HarnessTaskStatus {
    match status {
        nexus_core::orchestration::TaskStatus::Queued => HarnessTaskStatus::Ready,
        nexus_core::orchestration::TaskStatus::Running => HarnessTaskStatus::Running,
        nexus_core::orchestration::TaskStatus::Paused => HarnessTaskStatus::Paused,
        nexus_core::orchestration::TaskStatus::Completed => HarnessTaskStatus::Completed,
        nexus_core::orchestration::TaskStatus::Failed => HarnessTaskStatus::Failed,
        nexus_core::orchestration::TaskStatus::Cancelled => HarnessTaskStatus::Cancelled,
        nexus_core::orchestration::TaskStatus::Blocked => HarnessTaskStatus::Blocked,
    }
}

fn plan_title(objective: &str) -> String {
    let title = objective
        .split_whitespace()
        .take(12)
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        "Plan".into()
    } else {
        title
    }
}

/// Detect the operator asserting their own name, anywhere in the message.
///
/// Two tiers keep this from firing on ordinary sentences. The strong forms
/// (`my name is …`, `call me …`) are explicit enough to take whatever follows;
/// the weaker self-introductions (`I'm …`, `I am …`, `this is …`) are only
/// honored when what follows is name-shaped, so "I am tired" and "I'm working
/// on the parser" produce nothing.
fn explicit_name(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    // ASCII-lowercased haystack: same byte length, so indices map back onto the
    // original slice and preserve the name's original casing.
    let lower = trimmed.to_ascii_lowercase();

    for marker in ["you can call me ", "my name is ", "call me "] {
        if let Some(rest) = after_marker(trimmed, &lower, marker) {
            if let Some(name) = free_name(rest) {
                return Some(name);
            }
        }
    }
    for marker in ["i'm ", "i am ", "this is "] {
        if let Some(rest) = after_marker(trimmed, &lower, marker) {
            if let Some(name) = leading_name(rest) {
                return Some(name);
            }
        }
    }
    None
}

/// The card a workspace that has never chosen one should inherit.
///
/// The default card is excluded: it is a placeholder for nobody in particular,
/// and inheriting it is exactly the behaviour this replaces — a second
/// workspace would greet an operator SNX already knows as a stranger.
/// `last_seen_at` is the ordering key, falling back to `updated_at` for cards
/// old enough to predate it.
fn most_recent_real_card(profiles: Vec<UserProfile>) -> Option<UserProfile> {
    let mut real: Vec<UserProfile> = profiles
        .into_iter()
        .filter(|profile| !profile.is_default())
        .collect();
    real.sort_by(|a, b| {
        b.last_seen_at
            .as_deref()
            .unwrap_or(&b.updated_at)
            .cmp(a.last_seen_at.as_deref().unwrap_or(&a.updated_at))
    });
    real.into_iter().next()
}

/// The text following `marker`'s first occurrence at a word boundary (message
/// start, or preceded by a non-alphanumeric character), sliced from `orig`.
pub(crate) fn after_marker<'a>(orig: &'a str, lower: &str, marker: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(rel) = lower[from..].find(marker) {
        let idx = from + rel;
        let boundary = idx == 0
            || lower[..idx]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric());
        if boundary {
            return Some(&orig[idx + marker.len()..]);
        }
        from = idx + 1;
    }
    None
}

/// A strong-form name: everything up to the next sentence boundary, kept only
/// if it reads as a name (letters, spaces, and the usual name punctuation).
fn free_name(rest: &str) -> Option<&str> {
    let end = rest
        .find(['.', '!', '?', ',', ';', '\n'])
        .unwrap_or(rest.len());
    let candidate = rest[..end].trim();
    if !candidate.is_empty()
        && candidate.chars().count() <= 64
        && candidate.chars().all(|character| {
            character.is_alphanumeric()
                || character.is_whitespace()
                || matches!(character, '-' | '_' | '\'')
        })
    {
        Some(candidate)
    } else {
        None
    }
}

/// A weak-form name: the leading 1–3 capitalized tokens, stopping at the first
/// lowercase word or punctuation. Rejects "tired", "working on the parser".
fn leading_name(rest: &str) -> Option<&str> {
    let s = rest.trim_start();
    let mut pos = 0usize;
    let mut end = 0usize;
    let mut count = 0usize;
    while count < 3 && pos < s.len() {
        let ws = s[pos..]
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(s.len() - pos);
        pos += ws;
        if pos >= s.len() {
            break;
        }
        let tok_end = s[pos..]
            .find(char::is_whitespace)
            .map(|k| pos + k)
            .unwrap_or(s.len());
        let token = &s[pos..tok_end];
        let core = token.trim_end_matches([',', '.', '!', '?', ';', ':']);
        if core.is_empty() || !is_name_token(core) {
            break;
        }
        end = pos + core.len();
        count += 1;
        pos = tok_end;
        // Trailing punctuation ends the name phrase.
        if core.len() != token.len() {
            break;
        }
    }
    (count > 0).then(|| &s[..end])
}

/// A single name-shaped token: an uppercase letter followed by letters and the
/// interior punctuation names carry (`-`, `'`).
fn is_name_token(token: &str) -> bool {
    let mut chars = token.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() && c.is_uppercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphabetic() || matches!(c, '-' | '\''))
}

fn explicit_memory(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    for prefix in [
        "please remember that ",
        "remember that ",
        "please remember ",
    ] {
        if lower.starts_with(prefix) {
            let candidate = trimmed[prefix.len()..].trim();
            if !candidate.is_empty() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_identity_detection_is_narrow() {
        assert_eq!(explicit_name("My name is Sans."), Some("Sans"));
        assert_eq!(explicit_name("call me Alex"), Some("Alex"));
        assert_eq!(explicit_name("I am tired"), None);
        assert_eq!(explicit_name("my name might be Sans"), None);
    }

    #[test]
    fn explicit_identity_detection_reads_self_introductions_anywhere() {
        // Weak forms are accepted when what follows is name-shaped …
        assert_eq!(explicit_name("I'm Sans"), Some("Sans"));
        assert_eq!(explicit_name("hi, I am Sans"), Some("Sans"));
        assert_eq!(explicit_name("this is Sans"), Some("Sans"));
        // … anywhere in the message, and stopping at punctuation.
        assert_eq!(explicit_name("btw I'm Sans, can you help?"), Some("Sans"));
        assert_eq!(
            explicit_name("I'm Jean-Luc and I love rust"),
            Some("Jean-Luc")
        );
        // … but not when what follows is an ordinary lowercase clause.
        assert_eq!(explicit_name("I'm working on the parser"), None);
        assert_eq!(explicit_name("this is a test"), None);
        // Strong forms still work mid-sentence.
        assert_eq!(explicit_name("ok, my name is Sans"), Some("Sans"));
    }

    /// A workspace that has never chosen a card inherits the operator's most
    /// recent one instead of starting over as "default" — the reason a second
    /// checkout used to greet someone SNX already knew as a stranger.
    #[test]
    fn a_fresh_workspace_inherits_the_operators_latest_card_not_the_placeholder() {
        let card = |name: &str, seen: Option<&str>, default: bool| {
            let mut profile = UserProfile::new(name).expect("valid card");
            profile.updated_at = "2026-01-01T00:00:00Z".into();
            profile.last_seen_at = seen.map(str::to_owned);
            if default {
                profile
                    .metadata
                    .insert("is_default".into(), serde_json::Value::Bool(true));
            }
            profile
        };

        let picked = most_recent_real_card(vec![
            card("default", Some("2026-07-31T00:00:00Z"), true),
            card("Erpan", Some("2026-07-01T00:00:00Z"), false),
            card("Sans", Some("2026-07-30T00:00:00Z"), false),
        ])
        .expect("a real card exists");
        assert_eq!(picked.display_name, "Sans", "the placeholder must not win");

        // A card too old to carry `last_seen_at` still orders, by `updated_at`.
        let mut older = card("Erpan", None, false);
        older.updated_at = "2025-01-01T00:00:00Z".into();
        let picked = most_recent_real_card(vec![older, card("Sans", None, false)])
            .expect("a real card exists");
        assert_eq!(picked.display_name, "Sans");

        // With nothing but the placeholder there is nothing to inherit, and
        // the caller falls back to creating the default card as before.
        assert!(most_recent_real_card(vec![card("default", None, true)]).is_none());
        assert!(most_recent_real_card(Vec::new()).is_none());
    }

    #[test]
    fn explicit_memory_detection_ignores_casual_statements() {
        assert_eq!(
            explicit_memory("Please remember that this project uses Rust"),
            Some("this project uses Rust")
        );
        assert_eq!(explicit_memory("I may learn Rust someday"), None);
    }

    #[test]
    fn compatibility_mappings_preserve_domain_boundaries() {
        assert_eq!(
            legacy_memory_type(nexus_memory::MemoryKind::Procedure),
            MemoryType::Procedural
        );
        assert_eq!(
            legacy_memory_type(nexus_memory::MemoryKind::GoalHistory),
            MemoryType::Episodic
        );
        assert_eq!(persona_scope_kind("project"), "workspace");
        assert_eq!(persona_scope_kind("global"), "global");
        assert_eq!(
            harness_goal_status(nexus_goals::GoalStatus::Verifying),
            HarnessGoalStatus::Validating
        );
        assert_eq!(
            harness_stage_status(nexus_core::orchestration::StageStatus::Skipped),
            HarnessTaskStatus::Superseded
        );
    }

    #[test]
    fn generated_plan_titles_are_bounded_and_nonempty() {
        let objective = (0..20)
            .map(|index| format!("word{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(plan_title(&objective).split_whitespace().count(), 12);
        assert_eq!(plan_title("   "), "Plan");
    }
}
