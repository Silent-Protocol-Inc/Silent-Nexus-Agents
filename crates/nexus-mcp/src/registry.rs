//! MCP server registry with trust states, persisted to SQLite.

use nexus_core::config::McpServerConfig;
use nexus_core::ids::McpServerId;
use nexus_core::store::Store;
use nexus_core::{NexusError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustState {
    /// Tools require per-call approval.
    Untrusted,
    /// Tools may run under normal policy (still audited).
    Trusted,
}

impl TrustState {
    pub fn as_str(&self) -> &'static str {
        match self {
            TrustState::Untrusted => "untrusted",
            TrustState::Trusted => "trusted",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "trusted" => TrustState::Trusted,
            _ => TrustState::Untrusted,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerRecord {
    pub id: McpServerId,
    pub name: String,
    pub config: McpServerConfig,
    pub trust: TrustState,
    pub enabled: bool,
    pub last_health: Option<String>,
    pub created_at: String,
}

pub struct McpRegistry {
    store: Store,
}

impl McpRegistry {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    /// Register a server. This is an explicit user action — the caller is the
    /// CLI/TUI after user confirmation, never the model directly.
    pub fn add(&self, name: &str, config: McpServerConfig) -> Result<McpServerId> {
        if name.trim().is_empty() {
            return Err(NexusError::Other("MCP server name is required".into()));
        }
        let id = McpServerId::generate();
        let trust = TrustState::parse(&config.trust);
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO mcp_servers (id, name, config, trust, enabled, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                rusqlite::params![
                    id.as_str(),
                    name,
                    serde_json::to_string(&config)?,
                    trust.as_str(),
                    config.enabled as i32,
                    nexus_core::now_rfc3339(),
                ],
            )
            .map_err(|e| {
                if e.to_string().contains("UNIQUE") {
                    NexusError::Other(format!("MCP server `{name}` already exists"))
                } else {
                    NexusError::from(e)
                }
            })?;
            Ok(())
        })?;
        Ok(id)
    }

    pub fn get(&self, name: &str) -> Result<McpServerRecord> {
        self.store.with(|conn| {
            conn.query_row(
                "SELECT id, name, config, trust, enabled, last_health, created_at
                 FROM mcp_servers WHERE name = ?1",
                [name],
                row_to_record,
            )
            .map_err(|_| NexusError::NotFound(format!("MCP server `{name}`")))
        })
    }

    pub fn list(&self) -> Result<Vec<McpServerRecord>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, config, trust, enabled, last_health, created_at
                 FROM mcp_servers ORDER BY name",
            )?;
            let rows = stmt.query_map([], row_to_record)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
    }

    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<()> {
        self.update_field(name, "enabled", &(enabled as i32).to_string())
    }

    pub fn set_trust(&self, name: &str, trust: TrustState) -> Result<()> {
        self.update_field(name, "trust", trust.as_str())
    }

    pub fn record_health(&self, name: &str, health: &str) -> Result<()> {
        self.update_field(name, "last_health", health)
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        let n = self
            .store
            .with(|conn| Ok(conn.execute("DELETE FROM mcp_servers WHERE name = ?1", [name])?))?;
        if n == 0 {
            return Err(NexusError::NotFound(format!("MCP server `{name}`")));
        }
        Ok(())
    }

    fn update_field(&self, name: &str, field: &str, value: &str) -> Result<()> {
        // `field` is from a fixed internal set, never user input.
        let sql = format!("UPDATE mcp_servers SET {field} = ?1 WHERE name = ?2");
        let n = self
            .store
            .with(|conn| Ok(conn.execute(&sql, rusqlite::params![value, name])?))?;
        if n == 0 {
            return Err(NexusError::NotFound(format!("MCP server `{name}`")));
        }
        Ok(())
    }
}

fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<McpServerRecord> {
    let config_json: String = row.get(2)?;
    Ok(McpServerRecord {
        id: McpServerId::from(row.get::<_, String>(0)?),
        name: row.get(1)?,
        config: serde_json::from_str(&config_json).unwrap_or_default(),
        trust: TrustState::parse(&row.get::<_, String>(3)?),
        enabled: row.get::<_, i32>(4)? != 0,
        last_health: row.get(5)?,
        created_at: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> McpRegistry {
        McpRegistry::new(Store::open_in_memory().expect("store"))
    }

    fn config() -> McpServerConfig {
        McpServerConfig {
            transport: "stdio".into(),
            command: "my-mcp-server".into(),
            args: vec!["--flag".into()],
            enabled: true,
            trust: "untrusted".into(),
            ..Default::default()
        }
    }

    #[test]
    fn add_defaults_to_untrusted() {
        let r = registry();
        r.add("srv", config()).expect("add");
        let rec = r.get("srv").expect("get");
        assert_eq!(rec.trust, TrustState::Untrusted);
        assert!(rec.enabled);
    }

    #[test]
    fn trust_and_enable_toggle() {
        let r = registry();
        r.add("srv", config()).expect("add");
        r.set_trust("srv", TrustState::Trusted).expect("trust");
        r.set_enabled("srv", false).expect("disable");
        let rec = r.get("srv").expect("get");
        assert_eq!(rec.trust, TrustState::Trusted);
        assert!(!rec.enabled);
    }

    #[test]
    fn duplicate_name_rejected() {
        let r = registry();
        r.add("srv", config()).expect("add");
        assert!(r.add("srv", config()).is_err());
    }
}
