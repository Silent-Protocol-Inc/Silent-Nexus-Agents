//! nexus-memory: persistent memory with retrieval and user control.
//!
//! Memory is durable but never authoritative on its own: records carry a
//! source, confidence, scope, sensitivity, and an approval flag. Anything
//! marked `requires_approval` is not surfaced to the model until the user
//! approves it. Secrets are refused at the API boundary — a record whose
//! content trips secret detection is rejected, never silently stored.

use nexus_core::ids::MemoryId;
use nexus_core::redact::Redactor;
use nexus_core::store::Store;
use nexus_core::{NexusError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Session,
    ProjectFact,
    Preference,
    Procedure,
    Correction,
    SkillRef,
    ArtifactRef,
    GoalHistory,
}

impl MemoryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryKind::Session => "session",
            MemoryKind::ProjectFact => "project_fact",
            MemoryKind::Preference => "preference",
            MemoryKind::Procedure => "procedure",
            MemoryKind::Correction => "correction",
            MemoryKind::SkillRef => "skill_ref",
            MemoryKind::ArtifactRef => "artifact_ref",
            MemoryKind::GoalHistory => "goal_history",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "session" => MemoryKind::Session,
            "project_fact" => MemoryKind::ProjectFact,
            "preference" => MemoryKind::Preference,
            "procedure" => MemoryKind::Procedure,
            "correction" => MemoryKind::Correction,
            "skill_ref" => MemoryKind::SkillRef,
            "artifact_ref" => MemoryKind::ArtifactRef,
            "goal_history" => MemoryKind::GoalHistory,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: MemoryId,
    pub kind: MemoryKind,
    pub content: String,
    pub source: String,
    pub confidence: f64,
    pub scope: String,
    pub sensitivity: String,
    pub requires_approval: bool,
    pub approved: bool,
    pub created_at: String,
    pub verified_at: Option<String>,
    pub expires_at: Option<String>,
}

/// Input for creating a memory.
#[derive(Debug, Clone)]
pub struct NewMemory {
    pub kind: MemoryKind,
    pub content: String,
    pub source: String,
    pub confidence: f64,
    pub scope: String, // "project" | "global"
    pub sensitivity: String,
    pub requires_approval: bool,
    pub ttl_days: Option<u32>,
}

pub struct MemoryStore {
    store: Store,
    workspace: String,
    redactor: Arc<Redactor>,
    global_enabled: bool,
}

impl MemoryStore {
    pub fn new(
        store: Store,
        workspace: &str,
        redactor: Arc<Redactor>,
        global_enabled: bool,
    ) -> Self {
        Self {
            store,
            workspace: workspace.to_string(),
            redactor,
            global_enabled,
        }
    }

    /// Store a memory. Rejects secret-bearing content and unauthorized global
    /// scope. Deduplicates on exact content within the same scope.
    pub fn add(&self, mem: NewMemory) -> Result<MemoryId> {
        if mem.content.trim().is_empty() {
            return Err(NexusError::Other("memory content is empty".into()));
        }
        // Refuse to store secrets: if redaction changes the content, it holds
        // something sensitive.
        if self.redactor.redact(&mem.content) != mem.content {
            return Err(NexusError::Other(
                "refusing to store memory: content appears to contain a secret".into(),
            ));
        }
        if mem.scope == "global" && !self.global_enabled {
            return Err(NexusError::PolicyDenied(
                "global memory is disabled; enable memory.global_enabled to store cross-project facts".into(),
            ));
        }
        let scope_ws = if mem.scope == "global" {
            String::new()
        } else {
            self.workspace.clone()
        };
        // Dedup: same content, kind and scope already present?
        let existing: Option<String> = self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id FROM memories WHERE content = ?1 AND kind = ?2 AND scope = ?3 AND workspace = ?4",
            )?;
            let mut rows = stmt.query(rusqlite::params![
                mem.content,
                mem.kind.as_str(),
                mem.scope,
                scope_ws
            ])?;
            Ok(rows.next()?.map(|r| r.get::<_, String>(0)).transpose()?)
        })?;
        if let Some(id) = existing {
            // Re-verify timestamp instead of duplicating.
            self.store.with(|conn| {
                conn.execute(
                    "UPDATE memories SET verified_at = ?1 WHERE id = ?2",
                    rusqlite::params![nexus_core::now_rfc3339(), id],
                )?;
                Ok(())
            })?;
            return Ok(MemoryId::from(id));
        }
        let id = MemoryId::generate();
        let expires_at = mem.ttl_days.filter(|d| *d > 0).map(|d| {
            (chrono::Utc::now() + chrono::Duration::days(d as i64))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        });
        // Auto-approve records that don't require it.
        let approved = !mem.requires_approval;
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO memories
                 (id, kind, content, source, confidence, scope, workspace, sensitivity, requires_approval, approved, created_at, expires_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                rusqlite::params![
                    id.as_str(),
                    mem.kind.as_str(),
                    mem.content,
                    mem.source,
                    mem.confidence,
                    mem.scope,
                    scope_ws,
                    mem.sensitivity,
                    mem.requires_approval as i32,
                    approved as i32,
                    nexus_core::now_rfc3339(),
                    expires_at,
                ],
            )?;
            Ok(())
        })?;
        Ok(id)
    }

    /// Full-text search over approved, non-expired memories in scope.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryRecord>> {
        let now = nexus_core::now_rfc3339();
        let fts_query = sanitize_fts_query(query);
        let mut hits = self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT m.id, m.kind, m.content, m.source, m.confidence, m.scope, m.sensitivity,
                        m.requires_approval, m.approved, m.created_at, m.verified_at, m.expires_at
                 FROM memories_fts f
                 JOIN memories m ON m.rowid = f.rowid
                 WHERE memories_fts MATCH ?1
                   AND m.approved = 1
                   AND (m.workspace = ?2 OR m.scope = 'global')
                   AND (m.expires_at IS NULL OR m.expires_at > ?3)
                 ORDER BY rank
                 LIMIT ?4",
            )?;
            let rows = stmt.query_map(
                rusqlite::params![
                    fts_query,
                    self.workspace,
                    now,
                    (limit.saturating_mul(4).max(limit)) as i64
                ],
                row_to_record,
            )?;
            collect(rows)
        })?;
        hits.sort_by(|a, b| {
            memory_score(b)
                .partial_cmp(&memory_score(a))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.created_at.cmp(&a.created_at))
        });
        let mut seen = HashSet::new();
        hits.retain(|record| seen.insert(normalize_memory(&record.content)));
        hits.truncate(limit);
        Ok(hits)
    }

    /// List memories (optionally including unapproved) for inspection.
    pub fn list(&self, include_unapproved: bool, limit: usize) -> Result<Vec<MemoryRecord>> {
        self.store.with(|conn| {
            let sql = format!(
                "SELECT id, kind, content, source, confidence, scope, sensitivity,
                        requires_approval, approved, created_at, verified_at, expires_at
                 FROM memories
                 WHERE (workspace = ?1 OR scope = 'global') {}
                 ORDER BY created_at DESC LIMIT ?2",
                if include_unapproved {
                    ""
                } else {
                    "AND approved = 1"
                }
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(
                rusqlite::params![self.workspace, limit as i64],
                row_to_record,
            )?;
            collect(rows)
        })
    }

    pub fn get(&self, id: &str) -> Result<MemoryRecord> {
        self.store.with(|conn| {
            conn.query_row(
                "SELECT id, kind, content, source, confidence, scope, sensitivity,
                        requires_approval, approved, created_at, verified_at, expires_at
                 FROM memories WHERE id = ?1",
                [id],
                row_to_record,
            )
            .map_err(|_| NexusError::NotFound(format!("memory `{id}`")))
        })
    }

    /// Approve a pending memory so it becomes retrievable.
    pub fn approve(&self, id: &str) -> Result<()> {
        let changed = self.store.with(|conn| {
            Ok(conn.execute(
                "UPDATE memories SET approved = 1, verified_at = ?1 WHERE id = ?2",
                rusqlite::params![nexus_core::now_rfc3339(), id],
            )?)
        })?;
        if changed == 0 {
            return Err(NexusError::NotFound(format!("memory `{id}`")));
        }
        Ok(())
    }

    /// Delete a memory permanently.
    pub fn forget(&self, id: &str) -> Result<()> {
        let changed = self
            .store
            .with(|conn| Ok(conn.execute("DELETE FROM memories WHERE id = ?1", [id])?))?;
        if changed == 0 {
            return Err(NexusError::NotFound(format!("memory `{id}`")));
        }
        Ok(())
    }

    /// Remove expired memories; returns the count pruned.
    pub fn prune(&self) -> Result<usize> {
        let now = nexus_core::now_rfc3339();
        let n = self.store.with(|conn| {
            Ok(conn.execute(
                "DELETE FROM memories WHERE expires_at IS NOT NULL AND expires_at <= ?1",
                [now],
            )?)
        })?;
        Ok(n)
    }

    /// Export all in-scope memories as JSON.
    pub fn export(&self) -> Result<String> {
        let all = self.list(true, 10_000)?;
        Ok(serde_json::to_string_pretty(&all)?)
    }
}

fn normalize_memory(content: &str) -> String {
    content
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn memory_score(record: &MemoryRecord) -> f64 {
    let mut score = record.confidence.clamp(0.0, 1.0) * 4.0;
    if record.scope == "project" {
        score += 1.0;
    }
    if record.kind == MemoryKind::Correction {
        score += 2.0;
    }
    if record.verified_at.is_some() {
        score += 0.5;
    }
    if let Ok(created) = chrono::DateTime::parse_from_rfc3339(&record.created_at) {
        let days = (chrono::Utc::now() - created.with_timezone(&chrono::Utc))
            .num_days()
            .max(0) as f64;
        score += (1.0 - days / 365.0).max(0.0);
    }
    score
}

// ------------------------------------------------------------- personas

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersonaRecord {
    pub id: String,
    pub name: String,
    pub scope: String,
    pub workspace: String,
    pub parent_id: Option<String>,
    pub description: String,
    pub instructions: String,
    pub created_at: String,
    pub updated_at: String,
}

pub struct PersonaStore {
    store: Store,
    workspace: String,
}

impl PersonaStore {
    pub fn new(store: Store, workspace: &str) -> Self {
        Self {
            store,
            workspace: workspace.to_string(),
        }
    }

    pub fn create(
        &self,
        name: &str,
        scope: &str,
        parent_id: Option<&str>,
        description: &str,
        instructions: &str,
    ) -> Result<String> {
        validate_persona(name, scope, instructions)?;
        if let Some(parent) = parent_id {
            let parent = self.get(parent)?;
            if scope == "global" && parent.scope != "global" {
                return Err(NexusError::Config(
                    "a global persona may only inherit from another global persona".into(),
                ));
            }
        }
        let id = prefixed_id("persona");
        let workspace = if scope == "global" {
            String::new()
        } else {
            self.workspace.clone()
        };
        let now = nexus_core::now_rfc3339();
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO personas
                 (id, name, scope, workspace, parent_id, description, instructions,
                  created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)",
                rusqlite::params![
                    id,
                    name.trim(),
                    scope,
                    workspace,
                    parent_id,
                    description.trim(),
                    instructions.trim(),
                    now
                ],
            )
            .map_err(|e| {
                if e.to_string().contains("UNIQUE") {
                    NexusError::Other(format!(
                        "persona `{}` already exists in {scope} scope",
                        name.trim()
                    ))
                } else {
                    NexusError::from(e)
                }
            })?;
            Ok(())
        })?;
        Ok(id)
    }

    pub fn clone_persona(&self, source_id: &str, new_name: &str, scope: &str) -> Result<String> {
        let source = self.get(source_id)?;
        self.create(
            new_name,
            scope,
            source.parent_id.as_deref(),
            &source.description,
            &source.instructions,
        )
    }

    pub fn update(
        &self,
        id: &str,
        description: &str,
        instructions: &str,
        parent_id: Option<&str>,
    ) -> Result<()> {
        let current = self.get(id)?;
        validate_persona(&current.name, &current.scope, instructions)?;
        if parent_id == Some(id) {
            return Err(NexusError::Other(
                "a persona cannot inherit from itself".into(),
            ));
        }
        if let Some(parent) = parent_id {
            let parent_record = self.get(parent)?;
            if current.scope == "global" && parent_record.scope != "global" {
                return Err(NexusError::Config(
                    "a global persona may only inherit from another global persona".into(),
                ));
            }
            let mut next = Some(parent_record);
            let mut seen = HashSet::from([id.to_string()]);
            let mut depth = 0usize;
            while let Some(record) = next {
                if !seen.insert(record.id.clone()) {
                    return Err(NexusError::Other(
                        "persona inheritance cycle detected".into(),
                    ));
                }
                depth += 1;
                if depth >= 8 {
                    return Err(NexusError::Other(
                        "persona inheritance exceeds the maximum depth of 8".into(),
                    ));
                }
                next = match record.parent_id {
                    Some(parent) => Some(self.get(&parent)?),
                    None => None,
                };
            }
        }
        let changed = self.store.with(|conn| {
            Ok(conn.execute(
                "UPDATE personas SET description = ?1, instructions = ?2, parent_id = ?3,
                                     updated_at = ?4 WHERE id = ?5",
                rusqlite::params![
                    description.trim(),
                    instructions.trim(),
                    parent_id,
                    nexus_core::now_rfc3339(),
                    id
                ],
            )?)
        })?;
        if changed == 0 {
            return Err(NexusError::NotFound(format!("persona `{id}`")));
        }
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let changed = self
            .store
            .with(|conn| Ok(conn.execute("DELETE FROM personas WHERE id = ?1", [id])?))?;
        if changed == 0 {
            return Err(NexusError::NotFound(format!("persona `{id}`")));
        }
        Ok(())
    }

    pub fn get(&self, id_or_name: &str) -> Result<PersonaRecord> {
        self.store.with(|conn| {
            conn.query_row(
                "SELECT id, name, scope, workspace, parent_id, description, instructions,
                        created_at, updated_at
                 FROM personas
                 WHERE id = ?1 OR (name = ?1 AND (workspace = ?2 OR scope = 'global'))
                 ORDER BY CASE WHEN workspace = ?2 THEN 0 ELSE 1 END
                 LIMIT 1",
                rusqlite::params![id_or_name, self.workspace],
                row_to_persona,
            )
            .map_err(|_| NexusError::NotFound(format!("persona `{id_or_name}`")))
        })
    }

    pub fn list(&self) -> Result<Vec<PersonaRecord>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, scope, workspace, parent_id, description, instructions,
                        created_at, updated_at
                 FROM personas WHERE workspace = ?1 OR scope = 'global'
                 ORDER BY name, CASE WHEN workspace = ?1 THEN 0 ELSE 1 END",
            )?;
            let rows = stmt.query_map([&self.workspace], row_to_persona)?;
            collect_personas(rows)
        })
    }

    pub fn resolved_instructions(&self, id_or_name: &str) -> Result<String> {
        let mut current = Some(self.get(id_or_name)?);
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        while let Some(persona) = current {
            if !seen.insert(persona.id.clone()) {
                return Err(NexusError::Other(
                    "persona inheritance cycle detected".into(),
                ));
            }
            chain.push(persona.instructions.clone());
            if chain.len() >= 8 {
                return Err(NexusError::Other(
                    "persona inheritance exceeds the maximum depth of 8".into(),
                ));
            }
            current = match persona.parent_id {
                Some(parent) => Some(self.get(&parent)?),
                None => None,
            };
        }
        chain.reverse();
        Ok(chain.join("\n\n"))
    }
}

fn validate_persona(name: &str, scope: &str, instructions: &str) -> Result<()> {
    if name.trim().is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' '))
    {
        return Err(NexusError::Config(
            "persona name must use letters, digits, spaces, `-`, or `_`".into(),
        ));
    }
    if !matches!(scope, "global" | "project") {
        return Err(NexusError::Config(
            "persona scope must be `global` or `project`".into(),
        ));
    }
    if instructions.trim().is_empty() {
        return Err(NexusError::Config(
            "persona instructions cannot be empty".into(),
        ));
    }
    Ok(())
}

fn row_to_persona(row: &rusqlite::Row) -> rusqlite::Result<PersonaRecord> {
    Ok(PersonaRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        scope: row.get(2)?,
        workspace: row.get(3)?,
        parent_id: row.get(4)?,
        description: row.get(5)?,
        instructions: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn collect_personas(
    rows: rusqlite::MappedRows<impl FnMut(&rusqlite::Row) -> rusqlite::Result<PersonaRecord>>,
) -> Result<Vec<PersonaRecord>> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

// --------------------------------------------------------- profile traits

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileTrait {
    pub id: String,
    pub profile_name: String,
    pub trait_key: String,
    pub trait_value: String,
    pub category: String,
    pub explicit: bool,
    pub confidence: f64,
    pub evidence: String,
    pub source_session: Option<String>,
    pub sensitivity: String,
    pub status: String,
    pub scope: String,
    pub workspace: String,
    pub created_at: String,
    pub updated_at: String,
}

pub struct ProfileStore {
    store: Store,
    workspace: String,
}

impl ProfileStore {
    pub fn new(store: Store, workspace: &str) -> Self {
        Self {
            store,
            workspace: workspace.to_string(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_trait(
        &self,
        profile_name: &str,
        key: &str,
        value: &str,
        category: &str,
        explicit: bool,
        confidence: f64,
        evidence: &str,
        source_session: Option<&str>,
        scope: &str,
    ) -> Result<String> {
        if profile_name.trim().is_empty() || key.trim().is_empty() || value.trim().is_empty() {
            return Err(NexusError::Config(
                "profile name, trait key, and trait value are required".into(),
            ));
        }
        if !matches!(scope, "project" | "global") {
            return Err(NexusError::Config(
                "profile trait scope must be project or global".into(),
            ));
        }
        let sensitivity = classify_trait_sensitivity(key, value);
        let workspace = if scope == "global" {
            String::new()
        } else {
            self.workspace.clone()
        };
        if let Some(existing) = self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id FROM profile_traits
                 WHERE profile_name = ?1 AND trait_key = ?2 AND trait_value = ?3
                   AND scope = ?4 AND workspace = ?5 AND status != 'rejected'
                 LIMIT 1",
            )?;
            let mut rows = stmt.query(rusqlite::params![
                profile_name.trim(),
                key.trim(),
                value.trim(),
                scope,
                workspace
            ])?;
            Ok(rows
                .next()?
                .map(|row| row.get::<_, String>(0))
                .transpose()?)
        })? {
            return Ok(existing);
        }
        let conflicts_with_approved = self.store.with(|conn| {
            Ok(conn
                .prepare(
                    "SELECT 1 FROM profile_traits
                     WHERE profile_name = ?1 AND trait_key = ?2 AND trait_value != ?3
                       AND status = 'approved'
                       AND (workspace = ?4 OR scope = 'global')
                     LIMIT 1",
                )?
                .exists(rusqlite::params![
                    profile_name.trim(),
                    key.trim(),
                    value.trim(),
                    self.workspace
                ])?)
        })?;
        let low_risk_explicit = explicit
            && sensitivity == "normal"
            && category == "workflow"
            && !conflicts_with_approved;
        let status = if low_risk_explicit {
            "approved"
        } else {
            "pending"
        };
        let stored_evidence = if conflicts_with_approved {
            format!(
                "{}; conflicts with an approved value for this trait",
                evidence.trim()
            )
        } else {
            evidence.trim().to_string()
        };
        let id = prefixed_id("trait");
        let now = nexus_core::now_rfc3339();
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO profile_traits
                 (id, profile_name, trait_key, trait_value, category, explicit, confidence,
                  evidence, source_session, sensitivity, status, scope, workspace,
                  created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14)",
                rusqlite::params![
                    id,
                    profile_name.trim(),
                    key.trim(),
                    value.trim(),
                    category,
                    explicit as i32,
                    confidence.clamp(0.0, 1.0),
                    stored_evidence,
                    source_session,
                    sensitivity,
                    status,
                    scope,
                    workspace,
                    now
                ],
            )?;
            Ok(())
        })?;
        Ok(id)
    }

    pub fn list(&self, profile_name: &str, include_pending: bool) -> Result<Vec<ProfileTrait>> {
        self.store.with(|conn| {
            let sql = format!(
                "SELECT id, profile_name, trait_key, trait_value, category, explicit,
                        confidence, evidence, source_session, sensitivity, status, scope,
                        workspace, created_at, updated_at
                 FROM profile_traits
                 WHERE profile_name = ?1 AND (workspace = ?2 OR scope = 'global') {}
                 ORDER BY CASE status WHEN 'pending' THEN 0 ELSE 1 END, updated_at DESC",
                if include_pending {
                    ""
                } else {
                    "AND status = 'approved'"
                }
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(
                rusqlite::params![profile_name, self.workspace],
                row_to_profile_trait,
            )?;
            collect_traits(rows)
        })
    }

    pub fn review(&self, id: &str, approve: bool) -> Result<()> {
        let status = if approve { "approved" } else { "rejected" };
        let changed = self.store.with(|conn| {
            Ok(conn.execute(
                "UPDATE profile_traits SET status = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![status, nexus_core::now_rfc3339(), id],
            )?)
        })?;
        if changed == 0 {
            return Err(NexusError::NotFound(format!("profile trait `{id}`")));
        }
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let changed = self
            .store
            .with(|conn| Ok(conn.execute("DELETE FROM profile_traits WHERE id = ?1", [id])?))?;
        if changed == 0 {
            return Err(NexusError::NotFound(format!("profile trait `{id}`")));
        }
        Ok(())
    }

    pub fn approved_prompt(&self, profile_name: &str) -> Result<String> {
        Ok(self
            .list(profile_name, false)?
            .into_iter()
            .map(|record| format!("- {}: {}", record.trait_key, record.trait_value))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn classify_trait_sensitivity(key: &str, value: &str) -> &'static str {
    let text = format!("{key} {value}").to_lowercase();
    if [
        "password", "token", "secret", "health", "medical", "religion", "politic", "sexual",
        "identity", "address", "phone", "email",
    ]
    .iter()
    .any(|needle| text.contains(needle))
    {
        "sensitive"
    } else {
        "normal"
    }
}

fn row_to_profile_trait(row: &rusqlite::Row) -> rusqlite::Result<ProfileTrait> {
    Ok(ProfileTrait {
        id: row.get(0)?,
        profile_name: row.get(1)?,
        trait_key: row.get(2)?,
        trait_value: row.get(3)?,
        category: row.get(4)?,
        explicit: row.get::<_, i32>(5)? != 0,
        confidence: row.get(6)?,
        evidence: row.get(7)?,
        source_session: row.get(8)?,
        sensitivity: row.get(9)?,
        status: row.get(10)?,
        scope: row.get(11)?,
        workspace: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn collect_traits(
    rows: rusqlite::MappedRows<impl FnMut(&rusqlite::Row) -> rusqlite::Result<ProfileTrait>>,
) -> Result<Vec<ProfileTrait>> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn prefixed_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

// --------------------------------------------------------------- RSI

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RsiProposal {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub risk: String,
    pub source_session: Option<String>,
    pub status: String,
    pub created_at: String,
    pub reviewed_at: Option<String>,
}

pub struct RsiStore {
    store: Store,
    workspace: String,
}

impl RsiStore {
    pub fn new(store: Store, workspace: &str) -> Self {
        Self {
            store,
            workspace: workspace.to_string(),
        }
    }

    /// Deterministic post-turn analysis. Explicit low-risk workflow
    /// preferences are approved immediately; inferred traits and reusable
    /// capability changes remain inspectable proposals.
    pub fn after_completed_turn(&self, session_id: &str, objective: &str) -> Result<()> {
        let trimmed = objective.trim();
        let lower = trimmed.to_lowercase();
        let profile_name = self.store.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT profile_name FROM sessions WHERE id = ?1",
                    [session_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap_or_else(|_| "default".into()))
        })?;
        if ["always ", "please always ", "prefer ", "by default "]
            .iter()
            .any(|prefix| lower.starts_with(prefix))
        {
            ProfileStore::new(self.store.clone(), &self.workspace).add_trait(
                &profile_name,
                "explicit_workflow_preference",
                trimmed,
                "workflow",
                true,
                1.0,
                "explicit wording in a completed user turn",
                Some(session_id),
                "project",
            )?;
        } else if lower.starts_with("i usually ") || lower.starts_with("you seem to prefer ") {
            ProfileStore::new(self.store.clone(), &self.workspace).add_trait(
                &profile_name,
                "inferred_workflow_preference",
                trimmed,
                "workflow",
                false,
                0.6,
                "inferred wording in a completed user turn",
                Some(session_id),
                "project",
            )?;
        }

        if let Some((tool, failures)) = self.repeated_tool_failure()? {
            self.propose(
                "tool",
                &format!("Reduce repeated `{tool}` failures"),
                &serde_json::json!({
                    "observation": format!("{failures} failed calls were recorded"),
                    "proposal": "Inspect the failure pattern and propose a safer reusable workflow or tool improvement.",
                    "executable_payload": false,
                })
                .to_string(),
                "review",
                Some(session_id),
            )?;
        }

        let repeats = self.objective_repeat_count(trimmed)?;
        if repeats >= 3 {
            let name = format!(
                "workflow-{}",
                trimmed
                    .split_whitespace()
                    .take(4)
                    .collect::<Vec<_>>()
                    .join("-")
                    .chars()
                    .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
                    .collect::<String>()
                    .to_lowercase()
            );
            let manifest = serde_json::json!({
                "name": name,
                "purpose": trimmed,
                "triggers": [trimmed],
                "required_tools": [],
                "permissions": [],
                "inputs": [],
                "workflow": ["Review the repeated workflow and fill in explicit declarative steps."],
                "expected_outputs": ["A verified result"],
                "verification": "Normal NEXUS evidence rules",
                "examples": [],
                "version": "0.1.0-proposed",
                "provenance": "agent_proposed",
                "executable_payload": false,
                "enabled": false
            });
            self.propose(
                "skill",
                "Reusable workflow detected",
                &manifest.to_string(),
                "review",
                Some(session_id),
            )?;
        }
        Ok(())
    }

    pub fn list(&self, include_reviewed: bool) -> Result<Vec<RsiProposal>> {
        self.store.with(|conn| {
            let sql = format!(
                "SELECT id, kind, title, body, risk, source_session, status, created_at, reviewed_at
                 FROM rsi_proposals {}
                 ORDER BY created_at DESC",
                if include_reviewed {
                    ""
                } else {
                    "WHERE status = 'pending'"
                }
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], row_to_rsi)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    pub fn get(&self, id: &str) -> Result<RsiProposal> {
        self.store.with(|conn| {
            conn.query_row(
                "SELECT id, kind, title, body, risk, source_session, status, created_at, reviewed_at
                 FROM rsi_proposals WHERE id = ?1",
                [id],
                row_to_rsi,
            )
            .map_err(|_| NexusError::NotFound(format!("RSI proposal `{id}`")))
        })
    }

    pub fn review(&self, id: &str, approve: bool) -> Result<()> {
        let changed = self.store.with(|conn| {
            Ok(conn.execute(
                "UPDATE rsi_proposals SET status = ?1, reviewed_at = ?2 WHERE id = ?3",
                rusqlite::params![
                    if approve { "approved" } else { "rejected" },
                    nexus_core::now_rfc3339(),
                    id
                ],
            )?)
        })?;
        if changed == 0 {
            return Err(NexusError::NotFound(format!("RSI proposal `{id}`")));
        }
        Ok(())
    }

    fn propose(
        &self,
        kind: &str,
        title: &str,
        body: &str,
        risk: &str,
        source_session: Option<&str>,
    ) -> Result<String> {
        if body.contains("#!/") || body.contains("base64,") || body.contains('\0') {
            return Err(NexusError::Other(
                "RSI proposal rejected: executable/hidden payload marker".into(),
            ));
        }
        if let Some(existing) = self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id FROM rsi_proposals
                 WHERE kind = ?1 AND title = ?2 AND status = 'pending' LIMIT 1",
            )?;
            let mut rows = stmt.query(rusqlite::params![kind, title])?;
            Ok(rows
                .next()?
                .map(|row| row.get::<_, String>(0))
                .transpose()?)
        })? {
            return Ok(existing);
        }
        let id = prefixed_id("rsi");
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO rsi_proposals
                 (id, kind, title, body, risk, source_session, status, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,'pending',?7)",
                rusqlite::params![
                    id,
                    kind,
                    title,
                    body,
                    risk,
                    source_session,
                    nexus_core::now_rfc3339()
                ],
            )?;
            Ok(())
        })?;
        Ok(id)
    }

    fn repeated_tool_failure(&self) -> Result<Option<(String, i64)>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT tool, COUNT(*) AS failures FROM tool_calls
                 WHERE exit_status IS NOT NULL AND exit_status != 'ok'
                 GROUP BY tool HAVING failures >= 3
                 ORDER BY failures DESC LIMIT 1",
            )?;
            let mut rows = stmt.query([])?;
            match rows.next()? {
                Some(row) => Ok(Some((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))),
                None => Ok(None),
            }
        })
    }

    fn objective_repeat_count(&self, objective: &str) -> Result<i64> {
        self.store.with(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM messages m
                 JOIN sessions s ON s.id = m.session_id
                 WHERE m.role = 'user' AND m.content = ?1 AND s.workspace = ?2",
                rusqlite::params![objective, self.workspace],
                |row| row.get(0),
            )?)
        })
    }
}

fn row_to_rsi(row: &rusqlite::Row) -> rusqlite::Result<RsiProposal> {
    Ok(RsiProposal {
        id: row.get(0)?,
        kind: row.get(1)?,
        title: row.get(2)?,
        body: row.get(3)?,
        risk: row.get(4)?,
        source_session: row.get(5)?,
        status: row.get(6)?,
        created_at: row.get(7)?,
        reviewed_at: row.get(8)?,
    })
}

fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<MemoryRecord> {
    Ok(MemoryRecord {
        id: MemoryId::from(row.get::<_, String>(0)?),
        kind: MemoryKind::parse(&row.get::<_, String>(1)?).unwrap_or(MemoryKind::Session),
        content: row.get(2)?,
        source: row.get(3)?,
        confidence: row.get(4)?,
        scope: row.get(5)?,
        sensitivity: row.get(6)?,
        requires_approval: row.get::<_, i32>(7)? != 0,
        approved: row.get::<_, i32>(8)? != 0,
        created_at: row.get(9)?,
        verified_at: row.get(10)?,
        expires_at: row.get(11)?,
    })
}

fn collect(
    rows: rusqlite::MappedRows<impl FnMut(&rusqlite::Row) -> rusqlite::Result<MemoryRecord>>,
) -> Result<Vec<MemoryRecord>> {
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Turn a free-text query into a safe FTS5 MATCH expression: OR the terms so
/// partial matches count, and quote each term to neutralize FTS operators.
fn sanitize_fts_query(query: &str) -> String {
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|t| t.chars().any(|c| c.is_alphanumeric()))
        .map(|t| {
            let cleaned: String = t
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            format!("\"{cleaned}\"")
        })
        .filter(|t| t.len() > 2)
        .collect();
    if terms.is_empty() {
        "\"\"".to_string()
    } else {
        terms.join(" OR ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> MemoryStore {
        MemoryStore::new(
            Store::open_in_memory().expect("store"),
            "/ws",
            Arc::new(Redactor::new()),
            false,
        )
    }

    fn new_fact(content: &str) -> NewMemory {
        NewMemory {
            kind: MemoryKind::ProjectFact,
            content: content.into(),
            source: "test".into(),
            confidence: 0.8,
            scope: "project".into(),
            sensitivity: "normal".into(),
            requires_approval: false,
            ttl_days: None,
        }
    }

    #[test]
    fn add_and_search() {
        let m = store();
        m.add(new_fact("The build uses cargo and requires Rust stable"))
            .expect("add");
        let hits = m.search("cargo build", 10).expect("search");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].content.contains("cargo"));
    }

    #[test]
    fn refuses_secrets() {
        let m = store();
        let err = m
            .add(new_fact("api key is sk-abcdef1234567890ABCDEF"))
            .expect_err("must refuse");
        assert!(err.to_string().contains("secret"));
    }

    #[test]
    fn dedups_identical_content() {
        let m = store();
        let id1 = m.add(new_fact("same content here")).expect("add");
        let id2 = m.add(new_fact("same content here")).expect("add");
        assert_eq!(id1, id2);
        assert_eq!(m.list(true, 100).expect("list").len(), 1);
    }

    #[test]
    fn unapproved_not_returned_by_search() {
        let m = store();
        let mut mem = new_fact("secret-ish plan detail needs review");
        mem.requires_approval = true;
        let id = m.add(mem).expect("add");
        assert!(m.search("plan detail", 10).expect("search").is_empty());
        m.approve(id.as_str()).expect("approve");
        assert_eq!(m.search("plan detail", 10).expect("search").len(), 1);
    }

    #[test]
    fn global_scope_requires_enablement() {
        let m = store();
        let mut mem = new_fact("global fact");
        mem.scope = "global".into();
        assert!(m.add(mem).is_err());
    }

    #[test]
    fn prune_removes_expired() {
        let m = store();
        let mut mem = new_fact("ephemeral");
        mem.ttl_days = Some(1);
        m.add(mem).expect("add");
        // Force expiry by rewriting expires_at into the past.
        m.store
            .with(|conn| {
                conn.execute(
                    "UPDATE memories SET expires_at = '2000-01-01T00:00:00Z'",
                    [],
                )?;
                Ok(())
            })
            .expect("update");
        assert_eq!(m.prune().expect("prune"), 1);
    }

    #[test]
    fn fts_query_is_injection_safe() {
        // FTS special chars must not blow up the query.
        let q = sanitize_fts_query("cargo* OR (build) NEAR");
        assert!(q.contains("\"cargo\""));
        assert!(q.contains("\"build\""));
    }

    #[test]
    fn correction_and_project_scope_rank_ahead() {
        let m = store();
        let mut ordinary = new_fact("build command uses cargo check");
        ordinary.confidence = 0.9;
        m.add(ordinary).expect("ordinary");
        let mut correction = new_fact("build command correction use cargo check workspace");
        correction.kind = MemoryKind::Correction;
        correction.confidence = 0.7;
        m.add(correction).expect("correction");
        let hits = m.search("build command cargo check", 10).expect("search");
        assert_eq!(hits[0].kind, MemoryKind::Correction);
    }

    #[test]
    fn persona_inheritance_and_project_override_work() {
        let db = Store::open_in_memory().expect("store");
        let personas = PersonaStore::new(db, "/ws");
        let base = personas
            .create("reviewer", "global", None, "base", "Be concise.")
            .expect("base");
        let child = personas
            .create(
                "project-reviewer",
                "project",
                Some(&base),
                "project",
                "Prioritize Rust safety.",
            )
            .expect("child");
        let resolved = personas.resolved_instructions(&child).expect("resolved");
        assert!(
            resolved
                .find("Be concise.")
                .expect("base persona instructions")
                < resolved
                    .find("Rust safety")
                    .expect("child persona instructions")
        );

        personas
            .create("shared", "global", None, "", "Global behavior.")
            .expect("global override target");
        let project = personas
            .create("shared", "project", None, "", "Project behavior.")
            .expect("project override");
        assert_eq!(personas.get("shared").expect("get").id, project);

        let first = personas
            .create("first", "project", None, "", "First.")
            .expect("first");
        let second = personas
            .create("second", "project", Some(&first), "", "Second.")
            .expect("second");
        assert!(personas
            .update(&first, "", "First.", Some(&second))
            .is_err());
        assert!(personas
            .get(&first)
            .expect("first unchanged")
            .parent_id
            .is_none());
    }

    #[test]
    fn profile_auto_approval_excludes_sensitive_and_conflicting_traits() {
        let db = Store::open_in_memory().expect("store");
        let profiles = ProfileStore::new(db, "/ws");
        let approved = profiles
            .add_trait(
                "default",
                "format",
                "run cargo fmt",
                "workflow",
                true,
                1.0,
                "operator said so",
                None,
                "project",
            )
            .expect("approved");
        let sensitive = profiles
            .add_trait(
                "default",
                "email",
                "user@example.test",
                "workflow",
                true,
                1.0,
                "operator said so",
                None,
                "project",
            )
            .expect("sensitive");
        let conflicting = profiles
            .add_trait(
                "default",
                "format",
                "never run cargo fmt",
                "workflow",
                true,
                1.0,
                "operator correction",
                None,
                "project",
            )
            .expect("conflicting");
        let records = profiles.list("default", true).expect("list");
        let status = |id: &str| {
            records
                .iter()
                .find(|record| record.id == id)
                .map(|record| record.status.as_str())
                .expect("profile trait record")
        };
        assert_eq!(status(&approved), "approved");
        assert_eq!(status(&sensitive), "pending");
        assert_eq!(status(&conflicting), "pending");
    }

    #[test]
    fn rsi_learning_uses_the_sessions_selected_profile() {
        let db = Store::open_in_memory().expect("store");
        db.with(|conn| {
            conn.execute(
                "INSERT INTO sessions
                 (id, title, workspace, created_at, updated_at, model, agent, status, profile_name)
                 VALUES ('session_1', '', '/ws', ?1, ?1, 'm', 'planner', 'active', 'focused')",
                [nexus_core::now_rfc3339()],
            )?;
            Ok(())
        })
        .expect("session");
        RsiStore::new(db.clone(), "/ws")
            .after_completed_turn("session_1", "Prefer concise validation summaries")
            .expect("RSI");
        let focused = ProfileStore::new(db, "/ws")
            .list("focused", true)
            .expect("traits");
        assert_eq!(focused.len(), 1);
        assert_eq!(focused[0].status, "approved");
        assert!(focused[0].trait_value.contains("concise"));
    }
}
