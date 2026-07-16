//! SQLite storage.
//!
//! One database per workspace at `.nexus/state/nexus.db` (WAL mode).
//! Migrations are embedded at compile time from `/migrations` and applied in
//! order inside a transaction; applied versions are recorded in
//! `schema_migrations` so upgrades are idempotent.
//!
//! rusqlite is synchronous; [`Store`] hands connections out behind a mutex and
//! async callers wrap calls in `spawn_blocking` via [`Store::with`].

use crate::error::{NexusError, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Embedded migrations, applied in array order. Never reorder or edit an
/// entry that has shipped — append a new file instead.
const MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_initial",
        include_str!("../../../migrations/0001_initial.sql"),
    ),
    (
        "0002_interactive_agent",
        include_str!("../../../migrations/0002_interactive_agent.sql"),
    ),
    (
        "0003_session_approval_grants",
        include_str!("../../../migrations/0003_session_approval_grants.sql"),
    ),
    (
        "0004_orchestration_timeline",
        include_str!("../../../migrations/0004_orchestration_timeline.sql"),
    ),
];

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("path", &self.path).finish()
    }
}

impl Store {
    /// Open (creating if needed) the workspace database and apply pending
    /// migrations.
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            path: db_path.to_path_buf(),
        };
        store.migrate()?;
        // Restrict database file permissions: it may hold conversation data.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(db_path) {
                let mut perms = meta.permissions();
                perms.set_mode(0o600);
                let _ = std::fs::set_permissions(db_path, perms);
            }
        }
        Ok(store)
    }

    /// In-memory store for tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            path: PathBuf::from(":memory:"),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run `f` with the locked connection. Synchronous; for async contexts
    /// use [`Store::with_blocking`].
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| NexusError::other("storage lock poisoned"))?;
        f(&conn)
    }

    /// Run `f` on the blocking thread pool (for use inside async tasks).
    pub async fn with_blocking<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.with(f))
            .await
            .map_err(|e| NexusError::other(format!("storage task join: {e}")))?
    }

    fn migrate(&self) -> Result<()> {
        self.with(|conn| {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations (
                    name TEXT PRIMARY KEY,
                    applied_at TEXT NOT NULL
                );",
            )?;
            for (name, sql) in MIGRATIONS {
                let applied: bool = conn
                    .prepare("SELECT 1 FROM schema_migrations WHERE name = ?1")?
                    .exists([name])?;
                if applied {
                    continue;
                }
                let tx_result: rusqlite::Result<()> = (|| {
                    conn.execute_batch("BEGIN")?;
                    conn.execute_batch(sql)?;
                    conn.execute(
                        "INSERT INTO schema_migrations (name, applied_at) VALUES (?1, ?2)",
                        rusqlite::params![name, crate::now_rfc3339()],
                    )?;
                    conn.execute_batch("COMMIT")?;
                    Ok(())
                })();
                if let Err(e) = tx_result {
                    let _ = conn.execute_batch("ROLLBACK");
                    return Err(NexusError::Migration(format!("{name}: {e}")));
                }
                tracing::info!(migration = name, "applied schema migration");
            }
            Ok(())
        })
    }

    /// Get a value from the kv_meta table.
    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        self.with(|conn| {
            let mut stmt = conn.prepare("SELECT value FROM kv_meta WHERE key = ?1")?;
            let mut rows = stmt.query([key])?;
            match rows.next()? {
                Some(row) => Ok(Some(row.get(0)?)),
                None => Ok(None),
            }
        })
    }

    /// Set a value in the kv_meta table.
    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO kv_meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, value],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_and_are_idempotent() {
        let store = Store::open_in_memory().expect("open");
        // Re-running migrate must be a no-op, not an error.
        store.migrate().expect("idempotent");
        let count: i64 = store
            .with(|c| Ok(c.query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))?))
            .expect("query");
        assert_eq!(count as usize, MIGRATIONS.len());
    }

    #[test]
    fn existing_0001_database_upgrades_without_rewrite() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("old.db");
        {
            let conn = Connection::open(&path).expect("open old");
            conn.execute_batch(
                "CREATE TABLE schema_migrations (
                    name TEXT PRIMARY KEY,
                    applied_at TEXT NOT NULL
                 );",
            )
            .expect("migration table");
            conn.execute_batch(include_str!("../../../migrations/0001_initial.sql"))
                .expect("0001");
            conn.execute(
                "INSERT INTO schema_migrations (name, applied_at) VALUES ('0001_initial', ?1)",
                [crate::now_rfc3339()],
            )
            .expect("record 0001");
        }

        let store = Store::open(&path).expect("upgrade");
        let columns: Vec<String> = store
            .with(|conn| {
                let mut stmt = conn.prepare("PRAGMA table_info(sessions)")?;
                let rows = stmt.query_map([], |row| row.get(1))?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row?);
                }
                Ok(out)
            })
            .expect("columns");
        assert!(columns.contains(&"persona_id".to_string()));
        assert!(columns.contains(&"profile_name".to_string()));
        let count: i64 = store
            .with(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                        row.get(0)
                    })?,
                )
            })
            .expect("migration count");
        assert_eq!(count as usize, MIGRATIONS.len());
    }

    #[test]
    fn databases_at_every_previous_migration_level_upgrade_to_timeline() {
        for applied_count in 1..MIGRATIONS.len() {
            let dir = tempfile::tempdir().expect("dir");
            let path = dir.path().join(format!("level-{applied_count}.db"));
            {
                let conn = Connection::open(&path).expect("open old");
                conn.execute_batch(
                    "CREATE TABLE schema_migrations (
                        name TEXT PRIMARY KEY,
                        applied_at TEXT NOT NULL
                     );",
                )
                .expect("migration table");
                for (name, sql) in MIGRATIONS.iter().take(applied_count) {
                    conn.execute_batch(sql).expect("apply old migration");
                    conn.execute(
                        "INSERT INTO schema_migrations (name, applied_at) VALUES (?1, ?2)",
                        rusqlite::params![name, crate::now_rfc3339()],
                    )
                    .expect("record old migration");
                }
            }
            let store = Store::open(&path).expect("upgrade");
            let (migration_count, timeline_exists): (i64, bool) = store
                .with(|conn| {
                    let count =
                        conn.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                            row.get(0)
                        })?;
                    let exists = conn
                        .prepare(
                            "SELECT 1 FROM sqlite_master
                             WHERE type='table' AND name='timeline_events'",
                        )?
                        .exists([])?;
                    Ok((count, exists))
                })
                .expect("inspect");
            assert_eq!(migration_count as usize, MIGRATIONS.len());
            assert!(timeline_exists, "level {applied_count} did not upgrade");
        }
    }

    #[test]
    fn kv_meta_roundtrip() {
        let store = Store::open_in_memory().expect("open");
        assert_eq!(store.meta_get("k").expect("get"), None);
        store.meta_set("k", "v1").expect("set");
        store.meta_set("k", "v2").expect("set");
        assert_eq!(store.meta_get("k").expect("get"), Some("v2".into()));
    }

    #[test]
    fn core_tables_exist() {
        let store = Store::open_in_memory().expect("open");
        for table in [
            "sessions",
            "messages",
            "tool_calls",
            "approvals",
            "goals",
            "goal_steps",
            "memories",
            "skills",
            "mcp_servers",
            "audit_events",
            "artifacts",
            "index_files",
            "index_symbols",
            "web_sources",
            "personas",
            "profile_traits",
            "memory_candidates",
            "session_usage",
            "session_links",
            "rsi_proposals",
            "connector_imports",
            "session_approval_grants",
            "timeline_events",
            "context_manifests",
            "session_view_state",
            "background_tasks",
            "agent_runs",
            "plan_versions",
            "plan_steps",
            "plan_edges",
            "plan_approvals",
            "session_interruptions",
        ] {
            let ok: bool = store
                .with(|c| {
                    Ok(
                        c.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name=?1")?
                            .exists([table])?,
                    )
                })
                .expect("query");
            assert!(ok, "table {table} missing");
        }
    }
}
