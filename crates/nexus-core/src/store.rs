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
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    (
        "0005_production_hardening",
        include_str!("../../../migrations/0005_production_hardening.sql"),
    ),
    (
        "0006_adaptive_harness",
        include_str!("../../../migrations/0006_adaptive_harness.sql"),
    ),
    (
        "0007_task_dependencies",
        include_str!("../../../migrations/0007_task_dependencies.sql"),
    ),
    (
        "0008_workspace_approval_grants",
        include_str!("../../../migrations/0008_workspace_approval_grants.sql"),
    ),
    (
        "0009_provider_catalog_cache",
        include_str!("../../../migrations/0009_provider_catalog_cache.sql"),
    ),
];
pub const MIGRATION_COUNT: usize = MIGRATIONS.len();

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
            crate::permissions::repair_private_tree(parent)?;
        }
        let conn = Connection::open(db_path)?;
        configure_connection(&conn, true)?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            path: db_path.to_path_buf(),
        };
        store.migrate()?;
        if let Some(parent) = db_path.parent() {
            crate::permissions::repair_private_tree(parent)?;
        }
        Ok(store)
    }

    /// In-memory store for tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        configure_connection(&conn, false)?;
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

    /// Retry a short SQLite operation when another NEXUS process owns the
    /// write lock. The connection busy timeout remains the primary guard;
    /// this bounded retry handles foreground/worker races around transactions.
    pub fn with_retry<T>(&self, mut operation: impl FnMut(&Connection) -> Result<T>) -> Result<T> {
        let mut attempts = 0u32;
        loop {
            let result = self.with(&mut operation);
            match result {
                Err(error) if is_busy_error(&error) && attempts < 4 => {
                    attempts += 1;
                    std::thread::sleep(Duration::from_millis(25 * (1 << attempts)));
                }
                other => return other,
            }
        }
    }

    fn migrate(&self) -> Result<()> {
        self.with(|conn| {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations (
                    name TEXT PRIMARY KEY,
                    applied_at TEXT NOT NULL
                );",
            )?;
            verify_known_checksums(conn, false)?;
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
                    verify_known_checksums(conn, true).map_err(|error| {
                        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                    })?;
                    conn.execute_batch("COMMIT")?;
                    Ok(())
                })();
                if let Err(e) = tx_result {
                    let _ = conn.execute_batch("ROLLBACK");
                    return Err(NexusError::Migration(format!("{name}: {e}")));
                }
                tracing::info!(migration = name, "applied schema migration");
            }
            verify_known_checksums(conn, false)?;
            Ok(())
        })
    }

    pub fn checkpoint_wal(&self, truncate: bool) -> Result<(i64, i64)> {
        self.with_retry(|connection| {
            let mode = if truncate { "TRUNCATE" } else { "PASSIVE" };
            let sql = format!("PRAGMA wal_checkpoint({mode})");
            let (_, log_frames, checkpointed): (i64, i64, i64) =
                connection
                    .query_row(&sql, [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            Ok((log_frames, checkpointed))
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

fn configure_connection(connection: &Connection, disk: bool) -> Result<()> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "wal_autocheckpoint", 1_000)?;
    connection.pragma_update(None, "journal_size_limit", 64 * 1024 * 1024)?;
    if disk {
        connection.pragma_update(None, "journal_mode", "WAL")?;
    }
    Ok(())
}

fn verify_known_checksums(connection: &Connection, backfill: bool) -> Result<()> {
    let checksum_table: bool = connection
        .prepare(
            "SELECT 1 FROM sqlite_master
             WHERE type='table' AND name='migration_checksums'",
        )?
        .exists([])?;
    if !checksum_table {
        return Ok(());
    }
    for (name, sql) in MIGRATIONS {
        let applied = connection
            .prepare("SELECT 1 FROM schema_migrations WHERE name=?1")?
            .exists([name])?;
        if !applied {
            continue;
        }
        let expected = hex::encode(Sha256::digest(sql.as_bytes()));
        let stored = connection
            .query_row(
                "SELECT sha256 FROM migration_checksums WHERE name=?1",
                [name],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match stored {
            Some(stored) if stored != expected => {
                return Err(NexusError::Migration(format!(
                    "{name}: checksum mismatch; database history may have been tampered with"
                )));
            }
            Some(_) => {}
            None if backfill => {
                connection.execute(
                    "INSERT INTO migration_checksums(name,sha256,verified_at)
                     VALUES (?1,?2,?3)",
                    rusqlite::params![name, expected, crate::now_rfc3339()],
                )?;
            }
            None => {
                return Err(NexusError::Migration(format!(
                    "{name}: checksum record is missing; database history may have been tampered with"
                )));
            }
        }
    }
    Ok(())
}

fn is_busy_error(error: &NexusError) -> bool {
    matches!(
        error,
        NexusError::Storage(rusqlite::Error::SqliteFailure(inner, _))
            if matches!(
                inner.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            )
    )
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
        let checksum_count: i64 =
            store
                .with(|connection| {
                    Ok(connection.query_row(
                        "SELECT COUNT(*) FROM migration_checksums",
                        [],
                        |row| row.get(0),
                    )?)
                })
                .expect("checksums");
        assert_eq!(checksum_count as usize, MIGRATIONS.len());
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
                // Migration 0005 introduced checksum enforcement. A database
                // genuinely produced by Store::migrate has checksums for every
                // applied migration; preserve that invariant when synthesizing
                // old migration levels for this upgrade test.
                if applied_count >= 5 {
                    for (name, sql) in MIGRATIONS.iter().take(applied_count) {
                        conn.execute(
                            "INSERT INTO migration_checksums(name,sha256,verified_at)
                             VALUES (?1,?2,?3)",
                            rusqlite::params![
                                name,
                                hex::encode(Sha256::digest(sql.as_bytes())),
                                crate::now_rfc3339()
                            ],
                        )
                        .expect("record old migration checksum");
                    }
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
    fn production_migration_backfills_existing_timeline_rows_into_fts() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("level-0004.db");
        {
            let connection = Connection::open(&path).expect("old database");
            connection
                .execute_batch(
                    "CREATE TABLE schema_migrations (
                        name TEXT PRIMARY KEY,
                        applied_at TEXT NOT NULL
                     );",
                )
                .expect("migration table");
            for (name, sql) in MIGRATIONS.iter().take(4) {
                connection.execute_batch(sql).expect("apply migration");
                connection
                    .execute(
                        "INSERT INTO schema_migrations(name,applied_at) VALUES (?1,?2)",
                        rusqlite::params![name, crate::now_rfc3339()],
                    )
                    .expect("record migration");
            }
            connection
                .execute(
                    "INSERT INTO sessions(id,workspace,created_at,updated_at)
                     VALUES ('session_old','/tmp',?1,?1)",
                    [crate::now_rfc3339()],
                )
                .expect("session");
            connection
                .execute(
                    "INSERT INTO timeline_events
                     (id,session_id,turn_id,trace_id,span_id,sequence,at,event_type,
                      phase,status,summary,payload)
                     VALUES ('event_old','session_old','turn','trace','span',1,?1,
                             'notice','completed','completed','legacy searchable phrase',
                             '{\"kind\":\"notice\",\"data\":{\"text\":\"legacy searchable phrase\",\"severity\":\"info\"}}')",
                    [crate::now_rfc3339()],
                )
                .expect("timeline");
        }

        let store = Store::open(&path).expect("upgrade");
        let count: i64 = store
            .with(|connection| {
                Ok(connection.query_row(
                    "SELECT COUNT(*) FROM timeline_fts
                     WHERE timeline_fts MATCH 'legacy AND searchable'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .expect("fts");
        assert_eq!(count, 1);
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
            "workspace_approval_grants",
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
            "migration_checksums",
            "timeline_fts",
            "harness_active_contexts",
            "harness_profiles",
            "harness_profile_facts",
            "harness_identity_conflicts",
            "harness_memories",
            "provider_catalog_cache",
            "harness_persona_versions",
            "harness_persona_assignments",
            "harness_agent_definitions",
            "harness_goals",
            "harness_plans",
            "harness_tasks",
            "harness_task_edges",
            "harness_subagent_specs",
            "harness_loop_states",
            "harness_checkpoints",
            "harness_improvement_proposals",
            "harness_events",
            "harness_provider_privacy_grants",
            "harness_model_assignments",
            "harness_task_attempts",
            "harness_resource_claims",
            "harness_approval_requests",
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

    #[test]
    fn migration_checksum_mismatch_blocks_open() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("state.db");
        let store = Store::open(&path).expect("open");
        store
            .with(|connection| {
                connection.execute(
                    "UPDATE migration_checksums SET sha256='tampered'
                     WHERE name='0001_initial'",
                    [],
                )?;
                Ok(())
            })
            .expect("tamper");
        drop(store);
        let error = Store::open(&path).expect_err("must reject");
        assert!(error.to_string().contains("checksum mismatch"));
    }

    #[test]
    fn missing_migration_checksum_blocks_open() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("state.db");
        let store = Store::open(&path).expect("open");
        store
            .with(|connection| {
                connection.execute(
                    "DELETE FROM migration_checksums WHERE name='0002_interactive_agent'",
                    [],
                )?;
                Ok(())
            })
            .expect("delete checksum");
        drop(store);
        let error = Store::open(&path).expect_err("must reject");
        assert!(error.to_string().contains("checksum record is missing"));
    }

    #[test]
    fn concurrent_connections_retry_short_writes() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("state.db");
        let first = Store::open(&path).expect("first");
        let second = Store::open(&path).expect("second");
        let thread = std::thread::spawn(move || {
            for index in 0..100 {
                second
                    .with_retry(|connection| {
                        connection.execute(
                            "INSERT INTO kv_meta(key,value) VALUES (?1,?2)
                             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                            rusqlite::params![format!("worker-{index}"), "ok"],
                        )?;
                        Ok(())
                    })
                    .expect("worker write");
            }
        });
        for index in 0..100 {
            first
                .with_retry(|connection| {
                    connection.execute(
                        "INSERT INTO kv_meta(key,value) VALUES (?1,?2)
                         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                        rusqlite::params![format!("foreground-{index}"), "ok"],
                    )?;
                    Ok(())
                })
                .expect("foreground write");
        }
        thread.join().expect("thread");
        let count: i64 = first
            .with(|connection| {
                Ok(connection.query_row("SELECT COUNT(*) FROM kv_meta", [], |row| row.get(0))?)
            })
            .expect("count");
        assert_eq!(count, 200);
    }
}
