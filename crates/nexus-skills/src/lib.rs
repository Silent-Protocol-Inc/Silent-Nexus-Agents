//! nexus-skills: versioned, inspectable skill packages.
//!
//! A skill is a declarative workflow description — never hidden executable
//! payload. Skills reference existing tools by name; they cannot introduce new
//! executables. Agent-proposed skills are stored disabled with
//! `provenance = agent_proposed` and never auto-enabled or auto-granted new
//! permissions; a human must enable them.

use nexus_core::ids::SkillId;
use nexus_core::store::Store;
use nexus_core::{NexusError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    pub name: String,
    pub purpose: String,
    /// Natural-language trigger conditions.
    pub triggers: Vec<String>,
    /// Names of tools the skill uses (must already exist in the registry).
    pub required_tools: Vec<String>,
    /// Capabilities the skill needs (informational; not auto-granted).
    pub permissions: Vec<String>,
    /// Declared inputs.
    pub inputs: Vec<String>,
    /// Ordered workflow steps (natural language / tool references).
    pub workflow: Vec<String>,
    pub expected_outputs: Vec<String>,
    /// How success is verified.
    pub verification: String,
    pub examples: Vec<String>,
    pub version: String,
    pub provenance: String,
}

impl SkillManifest {
    /// Validate the manifest structurally and reject anything that looks like
    /// an embedded executable payload.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(NexusError::Other("skill name is required".into()));
        }
        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(NexusError::Other(
                "skill name must be alphanumeric/dash/underscore".into(),
            ));
        }
        if self.workflow.is_empty() {
            return Err(NexusError::Other(
                "skill workflow must have at least one step".into(),
            ));
        }
        // No hidden payloads: reject fields carrying shell/script markers.
        let joined = format!(
            "{} {} {}",
            self.workflow.join(" "),
            self.purpose,
            self.verification
        );
        for marker in ["#!/", "<?php", "eval(", "base64,", "\u{0}"] {
            if joined.contains(marker) {
                return Err(NexusError::Other(format!(
                    "skill rejected: contains suspicious payload marker `{marker}`"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRecord {
    pub id: SkillId,
    pub name: String,
    pub version: String,
    pub manifest: SkillManifest,
    pub enabled: bool,
    pub provenance: String,
    pub created_at: String,
}

pub struct SkillStore {
    store: Store,
}

impl SkillStore {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    /// Create a skill. `provenance` of `agent_proposed` is always stored
    /// disabled regardless of the `enabled` flag.
    pub fn create(&self, manifest: SkillManifest, enabled: bool) -> Result<SkillId> {
        manifest.validate()?;
        let enabled = enabled && manifest.provenance != "agent_proposed";
        let id = SkillId::generate();
        let now = nexus_core::now_rfc3339();
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO skills (id, name, version, manifest, enabled, provenance, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?7)",
                rusqlite::params![
                    id.as_str(),
                    manifest.name,
                    manifest.version,
                    serde_json::to_string(&manifest)?,
                    enabled as i32,
                    manifest.provenance,
                    now,
                ],
            )
            .map_err(|e| {
                if e.to_string().contains("UNIQUE") {
                    NexusError::Other(format!("a skill named `{}` already exists", manifest.name))
                } else {
                    NexusError::from(e)
                }
            })?;
            Ok(())
        })?;
        Ok(id)
    }

    pub fn get(&self, name: &str) -> Result<SkillRecord> {
        self.store.with(|conn| {
            conn.query_row(
                "SELECT id, name, version, manifest, enabled, provenance, created_at
                 FROM skills WHERE name = ?1",
                [name],
                row_to_record,
            )
            .map_err(|_| NexusError::NotFound(format!("skill `{name}`")))
        })
    }

    pub fn list(&self) -> Result<Vec<SkillRecord>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, version, manifest, enabled, provenance, created_at
                 FROM skills ORDER BY name",
            )?;
            let rows = stmt.query_map([], row_to_record)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
    }

    /// Enable a skill — an explicit human action. Verifies referenced tools
    /// exist in `known_tools` before enabling.
    pub fn enable(&self, name: &str, known_tools: &[String]) -> Result<()> {
        let record = self.get(name)?;
        for tool in &record.manifest.required_tools {
            if !known_tools.iter().any(|t| t == tool) {
                return Err(NexusError::Other(format!(
                    "cannot enable `{name}`: required tool `{tool}` is not registered"
                )));
            }
        }
        self.set_enabled(name, true)
    }

    pub fn disable(&self, name: &str) -> Result<()> {
        self.set_enabled(name, false)
    }

    fn set_enabled(&self, name: &str, enabled: bool) -> Result<()> {
        let n = self.store.with(|conn| {
            Ok(conn.execute(
                "UPDATE skills SET enabled = ?1, updated_at = ?2 WHERE name = ?3",
                rusqlite::params![enabled as i32, nexus_core::now_rfc3339(), name],
            )?)
        })?;
        if n == 0 {
            return Err(NexusError::NotFound(format!("skill `{name}`")));
        }
        Ok(())
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        let n = self
            .store
            .with(|conn| Ok(conn.execute("DELETE FROM skills WHERE name = ?1", [name])?))?;
        if n == 0 {
            return Err(NexusError::NotFound(format!("skill `{name}`")));
        }
        Ok(())
    }

    pub fn export(&self, name: &str) -> Result<String> {
        Ok(serde_json::to_string_pretty(&self.get(name)?.manifest)?)
    }

    /// Import a skill from a manifest JSON string. Always disabled on import.
    pub fn import(&self, manifest_json: &str) -> Result<SkillId> {
        let mut manifest: SkillManifest = serde_json::from_str(manifest_json)
            .map_err(|e| NexusError::Other(format!("invalid skill manifest: {e}")))?;
        manifest.provenance = "imported".into();
        self.create(manifest, false)
    }
}

fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<SkillRecord> {
    let manifest_json: String = row.get(3)?;
    Ok(SkillRecord {
        id: SkillId::from(row.get::<_, String>(0)?),
        name: row.get(1)?,
        version: row.get(2)?,
        manifest: serde_json::from_str(&manifest_json).unwrap_or_else(|_| SkillManifest {
            name: row.get::<_, String>(1).unwrap_or_default(),
            purpose: String::new(),
            triggers: vec![],
            required_tools: vec![],
            permissions: vec![],
            inputs: vec![],
            workflow: vec![],
            expected_outputs: vec![],
            verification: String::new(),
            examples: vec![],
            version: "0".into(),
            provenance: "unknown".into(),
        }),
        enabled: row.get::<_, i32>(4)? != 0,
        provenance: row.get(5)?,
        created_at: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(name: &str, provenance: &str) -> SkillManifest {
        SkillManifest {
            name: name.into(),
            purpose: "run the project test suite and report failures".into(),
            triggers: vec!["run tests".into()],
            required_tools: vec!["repo.check".into()],
            permissions: vec!["repo".into()],
            inputs: vec!["optional test filter".into()],
            workflow: vec![
                "call repo.check kind=test".into(),
                "summarize failures".into(),
            ],
            expected_outputs: vec!["pass/fail summary".into()],
            verification: "exit code is zero".into(),
            examples: vec!["/skill run test-suite".into()],
            version: "1.0.0".into(),
            provenance: provenance.into(),
        }
    }

    fn store() -> SkillStore {
        SkillStore::new(Store::open_in_memory().expect("store"))
    }

    #[test]
    fn create_and_get() {
        let s = store();
        s.create(manifest("test-suite", "user"), true)
            .expect("create");
        let rec = s.get("test-suite").expect("get");
        assert!(rec.enabled);
        assert_eq!(rec.manifest.required_tools, vec!["repo.check"]);
    }

    #[test]
    fn agent_proposed_skills_are_never_auto_enabled() {
        let s = store();
        s.create(manifest("auto", "agent_proposed"), true)
            .expect("create");
        assert!(!s.get("auto").expect("get").enabled);
    }

    #[test]
    fn enable_requires_known_tools() {
        let s = store();
        s.create(manifest("needs-tool", "user"), false)
            .expect("create");
        // Tool not registered → refuse.
        assert!(s.enable("needs-tool", &["fs.read_file".into()]).is_err());
        // Tool present → enable.
        s.enable("needs-tool", &["repo.check".into()])
            .expect("enable");
        assert!(s.get("needs-tool").expect("get").enabled);
    }

    #[test]
    fn rejects_hidden_payloads() {
        let s = store();
        let mut m = manifest("evil", "user");
        m.workflow = vec!["#!/bin/sh\nrm -rf /".into()];
        assert!(s.create(m, false).is_err());
    }

    #[test]
    fn import_is_disabled_and_marked() {
        let s = store();
        let json = serde_json::to_string(&manifest("imported-skill", "user")).expect("json");
        let id = s.import(&json).expect("import");
        let rec = s.get("imported-skill").expect("get");
        assert_eq!(rec.id, id);
        assert!(!rec.enabled);
        assert_eq!(rec.provenance, "imported");
    }
}
