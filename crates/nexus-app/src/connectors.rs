//! Connector discovery/import for Codex MCP configuration and Agent Skills.
//!
//! Imports are intentionally inert: MCP servers are disabled/untrusted and
//! skills are disabled. Credential values are never copied from source
//! configuration; they require a separate credential-store action.

use crate::App;
use nexus_core::config::McpServerConfig;
use nexus_core::{NexusError, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct ConnectorCandidate {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub source: PathBuf,
    pub preview: String,
    pub credential_note: Option<String>,
    pub tools: Vec<String>,
    pub permissions: Vec<String>,
    pub commands: Vec<String>,
    pub trust: String,
    #[serde(skip)]
    payload: ConnectorPayload,
}

#[derive(Debug, Clone)]
enum ConnectorPayload {
    Mcp(McpServerConfig),
    Skill(nexus_skills::SkillManifest),
}

pub fn discover() -> Result<Vec<ConnectorCandidate>> {
    let mut out = Vec::new();
    if let Some(home) = nexus_models::codex_auth::codex_home() {
        discover_mcp(&home.join("config.toml"), &mut out)?;
        discover_skills(&home.join("skills"), &mut out)?;
    }
    if let Some(base) = directories::BaseDirs::new() {
        discover_skills(&base.home_dir().join(".agents").join("skills"), &mut out)?;
    }
    out.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
    Ok(out)
}

fn discover_mcp(path: &Path, out: &mut Vec<ConnectorCandidate>) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let text = std::fs::read_to_string(path)?;
    let value: toml::Value = toml::from_str(&text).map_err(|e| {
        NexusError::Config(format!(
            "cannot parse {} for MCP discovery: {e}",
            path.display()
        ))
    })?;
    let Some(servers) = value.get("mcp_servers").and_then(toml::Value::as_table) else {
        return Ok(());
    };
    for (name, entry) in servers {
        let name = sanitize_name(name);
        let table = entry.as_table().cloned().unwrap_or_default();
        let command = nexus_core::sanitize::sanitize_terminal(
            table
                .get("command")
                .and_then(toml::Value::as_str)
                .unwrap_or(""),
        );
        let args: Vec<String> = table
            .get("args")
            .and_then(toml::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let raw_url = table
            .get("url")
            .and_then(toml::Value::as_str)
            .map(String::from);
        let (args, args_had_credentials) = sanitize_mcp_args(&args);
        let (url, url_had_credentials) = raw_url
            .as_deref()
            .map(sanitize_connector_url)
            .unwrap_or((None, false));
        let transport = if url.is_some() { "http" } else { "stdio" };
        if transport == "stdio" && command.is_empty() {
            continue;
        }
        let credential_note = (args_had_credentials
            || url_had_credentials
            || table.keys().any(|key| {
                key.contains("env") || key.contains("token") || key.contains("credential")
            }))
        .then(|| "source references environment/credentials; values are not imported".to_string());
        let config = McpServerConfig {
            transport: transport.into(),
            command: command.clone(),
            args: args.clone(),
            url: url.clone(),
            enabled: false,
            trust: "untrusted".into(),
            ..Default::default()
        };
        out.push(ConnectorCandidate {
            id: format!("mcp:{name}"),
            kind: "mcp".into(),
            name: name.clone(),
            source: path.to_path_buf(),
            preview: if let Some(url) = url {
                format!("HTTP MCP {url} · imported disabled/untrusted")
            } else {
                format!(
                    "stdio MCP: {} {} · imported disabled/untrusted",
                    command,
                    args.join(" ")
                )
            },
            credential_note,
            tools: vec!["queried only after an explicitly enabled connection".into()],
            permissions: vec!["normal NEXUS policy and per-call approval".into()],
            commands: if command.is_empty() {
                Vec::new()
            } else {
                vec![format!("{} {}", command, args.join(" ")).trim().to_string()]
            },
            trust: "disabled / untrusted".into(),
            payload: ConnectorPayload::Mcp(config),
        });
    }
    Ok(())
}

fn discover_skills(root: &Path, out: &mut Vec<ConnectorCandidate>) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = if entry.path().is_dir() {
            entry.path().join("SKILL.md")
        } else {
            entry.path()
        };
        if path.file_name().and_then(|name| name.to_str()) != Some("SKILL.md") || !path.is_file() {
            continue;
        }
        let body = match std::fs::read_to_string(&path) {
            Ok(body) if !body.trim().is_empty() => body,
            _ => continue,
        };
        let raw_name = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("imported-skill");
        let name = sanitize_name(raw_name);
        let purpose = body
            .lines()
            .find(|line| {
                let line = line.trim();
                !line.is_empty() && !line.starts_with("---") && !line.starts_with('#')
            })
            .unwrap_or("Imported Agent Skill")
            .chars()
            .take(240)
            .collect::<String>();
        let inspected = body.chars().take(12_000).collect::<String>();
        let manifest = nexus_skills::SkillManifest {
            name: name.clone(),
            purpose,
            triggers: vec![format!("operator selects imported skill {name}")],
            required_tools: vec![],
            permissions: vec![],
            inputs: vec![],
            workflow: vec![format!(
                "Follow these inspected declarative instructions from {}:\n{}",
                path.display(),
                inspected
            )],
            expected_outputs: vec!["the skill-described result".into()],
            verification: "Use the skill's declared checks and normal NEXUS evidence rules".into(),
            examples: vec![],
            version: "1.0.0-imported".into(),
            provenance: "imported".into(),
        };
        out.push(ConnectorCandidate {
            id: format!("skill:{}", path.display()),
            kind: "skill".into(),
            name,
            source: path.clone(),
            preview: nexus_core::sanitize::sanitize_terminal(
                &body.chars().take(800).collect::<String>(),
            ),
            credential_note: None,
            tools: manifest.required_tools.clone(),
            permissions: manifest.permissions.clone(),
            commands: Vec::new(),
            trust: "disabled / untrusted".into(),
            payload: ConnectorPayload::Skill(manifest),
        });
    }
    Ok(())
}

pub fn find(candidate_id: &str) -> Result<ConnectorCandidate> {
    discover()?
        .into_iter()
        .find(|candidate| candidate.id == candidate_id)
        .ok_or_else(|| NexusError::NotFound(format!("connector candidate `{candidate_id}`")))
}

pub fn confirmation_preview(candidate_id: &str) -> Result<String> {
    let candidate = find(candidate_id)?;
    let list = |values: &[String], empty: &str| {
        if values.is_empty() {
            empty.to_string()
        } else {
            values.join(", ")
        }
    };
    Ok(nexus_core::sanitize::sanitize_terminal(&format!(
        "kind: {}\nname: {}\nsource: {}\ntrust: {}\ntools: {}\npermissions: {}\ncommands: {}\ncredentials: {}\npreview: {}",
        candidate.kind,
        candidate.name,
        candidate.source.display(),
        candidate.trust,
        list(&candidate.tools, "none declared"),
        list(&candidate.permissions, "none declared"),
        list(&candidate.commands, "none"),
        candidate
            .credential_note
            .as_deref()
            .unwrap_or("no credential values are imported"),
        candidate.preview.lines().next().unwrap_or(""),
    )))
}

pub fn import(app: &App, candidate_id: &str) -> Result<String> {
    let candidate = find(candidate_id)?;
    match candidate.payload {
        ConnectorPayload::Mcp(mut config) => {
            config.enabled = false;
            config.trust = "untrusted".into();
            app.mcp_registry().add(&candidate.name, config)?;
        }
        ConnectorPayload::Skill(mut manifest) => {
            manifest.provenance = "imported".into();
            app.skills().create(manifest, false)?;
        }
    }
    app.store.with(|conn| {
        conn.execute(
            "INSERT OR IGNORE INTO connector_imports
             (id, kind, name, source, preview, enabled, trust, created_at)
             VALUES (?1,?2,?3,?4,?5,0,'untrusted',?6)",
            rusqlite::params![
                format!("connector_{}", uuid::Uuid::new_v4().simple()),
                candidate.kind,
                candidate.name,
                candidate.source.display().to_string(),
                candidate.preview,
                nexus_core::now_rfc3339()
            ],
        )?;
        Ok(())
    })?;
    Ok(format!(
        "imported {} `{}` disabled and untrusted",
        candidate.kind, candidate.name
    ))
}

fn sanitize_name(name: &str) -> String {
    let value = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let sanitized: String = value.trim_matches('-').chars().take(64).collect();
    if sanitized.is_empty() {
        "imported-skill".into()
    } else {
        sanitized
    }
}

fn sanitize_mcp_args(args: &[String]) -> (Vec<String>, bool) {
    let mut out = Vec::with_capacity(args.len());
    let mut redact_next = false;
    let mut found = false;
    for arg in args {
        if redact_next {
            out.push("[credential-required]".into());
            redact_next = false;
            found = true;
            continue;
        }
        let lower = arg.to_ascii_lowercase();
        let sensitive = [
            "token",
            "api-key",
            "apikey",
            "secret",
            "password",
            "authorization",
        ]
        .iter()
        .any(|needle| lower.contains(needle));
        if sensitive {
            found = true;
            if let Some((flag, _)) = arg.split_once('=') {
                out.push(format!("{flag}=[credential-required]"));
            } else if arg.starts_with('-') {
                out.push(arg.clone());
                redact_next = true;
            } else {
                out.push("[credential-required]".into());
            }
        } else if lower.starts_with("bearer ") {
            found = true;
            out.push("Bearer [credential-required]".into());
        } else {
            out.push(arg.clone());
        }
    }
    (
        out.into_iter()
            .map(|arg| nexus_core::sanitize::sanitize_terminal(&arg))
            .collect(),
        found,
    )
}

fn sanitize_connector_url(raw: &str) -> (Option<String>, bool) {
    let Ok(mut parsed) = url::Url::parse(raw) else {
        let lower = raw.to_ascii_lowercase();
        let looks_sensitive = raw.contains('@')
            || ["token=", "key=", "secret=", "password=", "auth="]
                .iter()
                .any(|needle| lower.contains(needle));
        return if looks_sensitive {
            (Some("[invalid URL with credentials removed]".into()), true)
        } else {
            (Some(raw.to_string()), false)
        };
    };
    let mut found = false;
    if !parsed.username().is_empty() {
        let _ = parsed.set_username("");
        found = true;
    }
    if parsed.password().is_some() {
        let _ = parsed.set_password(None);
        found = true;
    }
    let pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .map(|(key, value)| {
            let lower = key.to_ascii_lowercase();
            if ["token", "key", "secret", "password", "auth"]
                .iter()
                .any(|needle| lower.contains(needle))
            {
                found = true;
                (key.into_owned(), "[credential-required]".into())
            } else {
                (key.into_owned(), value.into_owned())
            }
        })
        .collect();
    if !pairs.is_empty() {
        parsed.query_pairs_mut().clear().extend_pairs(pairs);
    }
    (Some(parsed.to_string()), found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_name_is_manifest_safe() {
        assert_eq!(sanitize_name("My Skill!"), "my-skill");
        assert_eq!(sanitize_name("!!!"), "imported-skill");
    }

    #[test]
    fn connector_credentials_are_replaced_not_imported() {
        let (args, found) = sanitize_mcp_args(&[
            "--api-key".into(),
            "super-secret".into(),
            "--mode=read".into(),
        ]);
        assert!(found);
        assert_eq!(args[1], "[credential-required]");
        assert!(!args.join(" ").contains("super-secret"));
        let (args, found) =
            sanitize_mcp_args(&["--header".into(), "Authorization: Bearer abc".into()]);
        assert!(found);
        assert!(!args.join(" ").contains("abc"));

        let (url, found) =
            sanitize_connector_url("https://user:pass@example.test/mcp?token=abc&mode=read");
        let url = url.expect("url");
        assert!(found);
        assert!(!url.contains("user"));
        assert!(!url.contains("pass"));
        assert!(!url.contains("abc"));
        assert!(url.contains("credential-required"));
    }
}
