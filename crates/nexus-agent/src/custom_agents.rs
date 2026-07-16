//! Declarative global and project agent definitions.
//!
//! Every custom agent inherits one audited built-in role. Definitions can
//! only remove tool categories, lower the maximum risk, disable writes or
//! delegation, and add tighter budgets. They cannot grant capabilities the
//! base role does not have or override harness safety.

use crate::AgentRole;
use nexus_core::{NexusError, Result, RiskLevel};
use nexus_tools::ToolCategory;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomAgentDefinition {
    pub name: String,
    pub base: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub tool_categories: Option<Vec<ToolCategory>>,
    #[serde(default)]
    pub allow_write: Option<bool>,
    #[serde(default)]
    pub max_risk: Option<RiskLevel>,
    #[serde(default)]
    pub max_steps: Option<u32>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub max_runtime_ms: Option<u64>,
    #[serde(default)]
    pub allow_delegation: Option<bool>,
    #[serde(skip)]
    pub scope: String,
    #[serde(skip)]
    pub source: PathBuf,
}

impl CustomAgentDefinition {
    pub fn base_role(&self) -> Result<AgentRole> {
        AgentRole::parse(&self.base).ok_or_else(|| {
            NexusError::Config(format!(
                "custom agent `{}` has unknown base role `{}`",
                self.name, self.base
            ))
        })
    }

    pub fn effective_tool_categories(&self) -> Result<Vec<ToolCategory>> {
        let base = self.base_role()?;
        Ok(self
            .tool_categories
            .clone()
            .unwrap_or_else(|| base.tool_categories()))
    }

    pub fn can_write(&self) -> Result<bool> {
        let base = self.base_role()?;
        Ok(base.can_write() && self.allow_write.unwrap_or(base.can_write()))
    }

    pub fn effective_max_risk(&self) -> Result<RiskLevel> {
        let base = self.base_role()?;
        Ok(self.max_risk.unwrap_or_else(|| base_max_risk(base)))
    }

    pub fn can_delegate(&self) -> Result<bool> {
        let base = self.base_role()?;
        Ok(base.can_delegate() && self.allow_delegation.unwrap_or_else(|| base.can_delegate()))
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty()
            || self.name.len() > 64
            || !self.name.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            return Err(NexusError::Config(format!(
                "custom agent name `{}` must use 1-64 letters, digits, `-`, or `_`",
                self.name
            )));
        }
        let base = self.base_role()?;
        let base_categories = base.tool_categories();
        if let Some(categories) = &self.tool_categories {
            for category in categories {
                if !base_categories.contains(category) {
                    return Err(NexusError::PolicyDenied(format!(
                        "custom agent `{}` cannot add `{}` beyond base role `{}`",
                        self.name,
                        category.as_str(),
                        base.as_str()
                    )));
                }
            }
        }
        if self.allow_write == Some(true) && !base.can_write() {
            return Err(NexusError::PolicyDenied(format!(
                "custom agent `{}` cannot add write permission to read-only base `{}`",
                self.name,
                base.as_str()
            )));
        }
        if let Some(max_risk) = self.max_risk {
            let base_max = base_max_risk(base);
            if max_risk > base_max {
                return Err(NexusError::PolicyDenied(format!(
                    "custom agent `{}` max_risk `{max_risk}` expands base `{}` (`{base_max}`)",
                    self.name,
                    base.as_str()
                )));
            }
        }
        if self.allow_delegation == Some(true) && base != AgentRole::Orchestrator {
            return Err(NexusError::PolicyDenied(format!(
                "custom agent `{}` cannot add delegation to base `{}`",
                self.name,
                base.as_str()
            )));
        }
        for (label, value) in [
            ("max_steps", self.max_steps.map(u64::from)),
            ("max_tokens", self.max_tokens),
            ("max_runtime_ms", self.max_runtime_ms),
        ] {
            if value == Some(0) {
                return Err(NexusError::Config(format!(
                    "custom agent `{}` {label} must be greater than zero",
                    self.name
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct AgentCatalog {
    definitions: BTreeMap<String, CustomAgentDefinition>,
}

impl AgentCatalog {
    pub fn load(global_dir: &Path, project_dir: &Path) -> Result<Self> {
        let mut definitions = BTreeMap::new();
        load_directory(global_dir, "global", &mut definitions)?;
        load_directory(project_dir, "project", &mut definitions)?;
        Ok(Self { definitions })
    }

    pub fn get(&self, name: &str) -> Option<&CustomAgentDefinition> {
        self.definitions.get(name)
    }

    pub fn list(&self) -> Vec<CustomAgentDefinition> {
        self.definitions.values().cloned().collect()
    }
}

fn load_directory(
    directory: &Path,
    scope: &str,
    definitions: &mut BTreeMap<String, CustomAgentDefinition>,
) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Ok(());
    };
    let mut paths = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("toml"))
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let text = std::fs::read_to_string(&path).map_err(|error| NexusError::ConfigFile {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
        if text.trim().is_empty() {
            continue;
        }
        let mut definition: CustomAgentDefinition =
            toml::from_str(&text).map_err(|error| NexusError::ConfigFile {
                path: path.display().to_string(),
                message: error.to_string(),
            })?;
        definition.scope = scope.into();
        definition.source = path.clone();
        definition.validate()?;
        definitions.insert(definition.name.clone(), definition);
    }
    Ok(())
}

fn base_max_risk(role: AgentRole) -> RiskLevel {
    role.max_risk()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_definition_overrides_global_and_may_narrow() {
        let root = tempfile::tempdir().expect("root");
        let global = root.path().join("global");
        let project = root.path().join("project");
        std::fs::create_dir_all(&global).expect("global");
        std::fs::create_dir_all(&project).expect("project");
        std::fs::write(
            global.join("audit.toml"),
            "name='audit'\nbase='reviewer'\ndescription='global'\n",
        )
        .expect("write");
        std::fs::write(
            project.join("audit.toml"),
            "name='audit'\nbase='reviewer'\ndescription='project'\ntool_categories=['filesystem']\nmax_steps=4\n",
        )
        .expect("write");
        let catalog = AgentCatalog::load(&global, &project).expect("catalog");
        let definition = catalog.get("audit").expect("definition");
        assert_eq!(definition.description, "project");
        assert_eq!(
            definition.effective_tool_categories().expect("tools"),
            vec![ToolCategory::Filesystem]
        );
    }

    #[test]
    fn definition_cannot_expand_read_only_base() {
        let definition = CustomAgentDefinition {
            name: "bad".into(),
            base: "reviewer".into(),
            description: String::new(),
            instructions: String::new(),
            tool_categories: Some(vec![ToolCategory::Terminal]),
            allow_write: Some(true),
            max_risk: Some(RiskLevel::Write),
            max_steps: None,
            max_tokens: None,
            max_runtime_ms: None,
            allow_delegation: None,
            scope: "test".into(),
            source: PathBuf::new(),
        };
        assert!(definition.validate().is_err());
    }

    #[test]
    fn custom_orchestrator_may_only_narrow_delegation() {
        let mut definition = CustomAgentDefinition {
            name: "bounded_orchestrator".into(),
            base: "orchestrator".into(),
            description: String::new(),
            instructions: String::new(),
            tool_categories: None,
            allow_write: None,
            max_risk: None,
            max_steps: None,
            max_tokens: None,
            max_runtime_ms: None,
            allow_delegation: Some(false),
            scope: "project".into(),
            source: PathBuf::from("agent.toml"),
        };
        definition.validate().expect("valid narrowing");
        assert!(!definition.can_delegate().expect("delegation"));

        definition.base = "reviewer".into();
        definition.allow_delegation = Some(true);
        assert!(definition.validate().is_err());
    }
}
