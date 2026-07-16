//! Explicit, non-destructive maintenance and backup operations.

use crate::artifacts::ArtifactStore;
use crate::store::Store;
use crate::{NexusError, Result};
use rusqlite::backup::Backup;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceReport {
    pub ok: bool,
    pub database_integrity: String,
    pub journal_mode: String,
    pub wal_log_frames: i64,
    pub wal_checkpointed_frames: i64,
    pub database_bytes: u64,
    pub state_bytes: u64,
    pub artifact_count: u64,
    pub artifact_bytes: u64,
    pub artifact_issues: Vec<String>,
    pub permission_issues: Vec<crate::permissions::PermissionIssue>,
    pub migration_checksums: u64,
    pub active_work: u64,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupFile {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupManifest {
    pub version: u32,
    pub created_at: String,
    pub source_state: String,
    pub database: String,
    pub files: Vec<BackupFile>,
}

pub fn check(
    store: &Store,
    state_dir: &Path,
    artifacts: &ArtifactStore,
    private_roots: &[PathBuf],
) -> Result<MaintenanceReport> {
    let (database_integrity, journal_mode, artifact_count, artifact_bytes, checksum_count, active) =
        store.with(|connection| {
            let mut integrity = connection.prepare("PRAGMA quick_check")?;
            let rows = integrity.query_map([], |row| row.get::<_, String>(0))?;
            let mut results = Vec::new();
            for row in rows {
                results.push(row?);
            }
            let journal_mode =
                connection.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?;
            let (artifact_count, artifact_bytes): (i64, i64) = connection.query_row(
                "SELECT COUNT(*), COALESCE(SUM(bytes),0) FROM artifacts",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let checksum_count: i64 =
                connection.query_row("SELECT COUNT(*) FROM migration_checksums", [], |row| {
                    row.get(0)
                })?;
            let active: i64 = connection.query_row(
                "SELECT
                    (SELECT COUNT(*) FROM background_tasks WHERE status='running') +
                    (SELECT COUNT(*) FROM agent_runs WHERE status='running') +
                    (SELECT COUNT(*) FROM timeline_events WHERE status='running')",
                [],
                |row| row.get(0),
            )?;
            Ok((
                results.join("; "),
                journal_mode,
                artifact_count,
                artifact_bytes,
                checksum_count,
                active,
            ))
        })?;
    let (wal_log_frames, wal_checkpointed_frames) = store.checkpoint_wal(false)?;
    let artifact_issues = artifacts.verify_all()?;
    let mut permission_issues = Vec::new();
    for root in private_roots {
        permission_issues.extend(crate::permissions::inspect_private_tree(root)?);
    }
    let database_bytes = std::fs::metadata(store.path())
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let state_bytes = tree_size(state_dir)?;
    let ok = database_integrity.eq_ignore_ascii_case("ok")
        && journal_mode.eq_ignore_ascii_case("wal")
        && artifact_issues.is_empty()
        && permission_issues.is_empty()
        && checksum_count == crate::store::MIGRATION_COUNT as i64;
    Ok(MaintenanceReport {
        ok,
        database_integrity,
        journal_mode,
        wal_log_frames,
        wal_checkpointed_frames,
        database_bytes,
        state_bytes,
        artifact_count: artifact_count.max(0) as u64,
        artifact_bytes: artifact_bytes.max(0) as u64,
        artifact_issues,
        permission_issues,
        migration_checksums: checksum_count.max(0) as u64,
        active_work: active.max(0) as u64,
        notes: vec!["No transcript, task, plan, goal, memory, or artifact was deleted.".into()],
    })
}

pub fn backup(store: &Store, state_dir: &Path, destination: &Path) -> Result<BackupManifest> {
    let destination = if destination.is_absolute() {
        destination.to_path_buf()
    } else {
        std::env::current_dir()?.join(destination)
    };
    let destination = destination.as_path();
    if destination.exists() {
        return Err(NexusError::Config(format!(
            "backup destination already exists: {}",
            destination.display()
        )));
    }
    let parent = destination.parent().ok_or_else(|| {
        NexusError::Config(format!(
            "backup destination has no parent: {}",
            destination.display()
        ))
    })?;
    crate::atomic::ensure_directory_tree(parent, 0o700)?;
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("silent-nexus-backup");
    let temporary = parent.join(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    crate::permissions::repair_private_tree(&temporary)?;

    let result = (|| -> Result<BackupManifest> {
        let database_path = temporary.join("nexus.db");
        store.with(|source| {
            let mut destination = rusqlite::Connection::open(&database_path)?;
            let backup = Backup::new(source, &mut destination)?;
            backup.run_to_completion(64, Duration::from_millis(10), None)?;
            Ok(())
        })?;
        crate::permissions::repair_private_tree(&temporary)?;

        let source_artifacts = state_dir.join("artifacts");
        let destination_artifacts = temporary.join("artifacts");
        if source_artifacts.exists() {
            copy_tree(&source_artifacts, &destination_artifacts)?;
        }

        let mut files = manifest_files(&temporary)?;
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let manifest = BackupManifest {
            version: 1,
            created_at: crate::now_rfc3339(),
            source_state: state_dir.display().to_string(),
            database: "nexus.db".into(),
            files,
        };
        let body = serde_json::to_vec_pretty(&manifest)?;
        crate::atomic::atomic_write_private(&temporary.join("manifest.json"), &body)?;
        crate::permissions::repair_private_tree(&temporary)?;
        std::fs::rename(&temporary, destination)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(manifest)
    })();

    if result.is_err() {
        let _ = std::fs::remove_dir_all(&temporary);
    }
    result
}

pub fn optimize(store: &Store, vacuum: bool) -> Result<MaintenanceReport> {
    let active = active_work(store)?;
    if active > 0 {
        return Err(NexusError::Other(format!(
            "maintenance optimize refused while {active} foreground/background item(s) are running"
        )));
    }
    store.with_retry(|connection| {
        connection.execute_batch("PRAGMA optimize;")?;
        Ok(())
    })?;
    store.checkpoint_wal(true)?;
    if vacuum {
        store.with_retry(|connection| {
            connection.execute_batch("VACUUM;")?;
            Ok(())
        })?;
    }
    let integrity = store.with(|connection| {
        Ok(connection.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))?)
    })?;
    Ok(MaintenanceReport {
        ok: integrity.eq_ignore_ascii_case("ok"),
        database_integrity: integrity,
        journal_mode: "wal".into(),
        wal_log_frames: 0,
        wal_checkpointed_frames: 0,
        database_bytes: std::fs::metadata(store.path())
            .map(|metadata| metadata.len())
            .unwrap_or(0),
        state_bytes: 0,
        artifact_count: 0,
        artifact_bytes: 0,
        artifact_issues: Vec::new(),
        permission_issues: Vec::new(),
        migration_checksums: 0,
        active_work: 0,
        notes: vec![if vacuum {
            "PRAGMA optimize, WAL checkpoint, and VACUUM completed.".into()
        } else {
            "PRAGMA optimize and WAL checkpoint completed; VACUUM was not requested.".into()
        }],
    })
}

fn active_work(store: &Store) -> Result<u64> {
    store.with(|connection| {
        let count: i64 = connection.query_row(
            "SELECT
                (SELECT COUNT(*) FROM background_tasks WHERE status='running') +
                (SELECT COUNT(*) FROM agent_runs WHERE status='running') +
                (SELECT COUNT(*) FROM timeline_events WHERE status='running')",
            [],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as u64)
    })
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(NexusError::PathDenied(format!(
            "backup refuses symlink {}",
            source.display()
        )));
    }
    if metadata.is_dir() {
        crate::permissions::repair_private_tree(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else if metadata.is_file() {
        let bytes = std::fs::read(source)?;
        crate::atomic::atomic_write_private(destination, &bytes)?;
    }
    Ok(())
}

fn manifest_files(root: &Path) -> Result<Vec<BackupFile>> {
    let mut files = Vec::new();
    collect_manifest_files(root, root, &mut files)?;
    Ok(files)
}

fn collect_manifest_files(root: &Path, path: &Path, output: &mut Vec<BackupFile>) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(NexusError::PathDenied(format!(
            "backup contains symlink {}",
            path.display()
        )));
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            collect_manifest_files(root, &entry?.path(), output)?;
        }
    } else if metadata.is_file()
        && path.file_name().and_then(|name| name.to_str()) != Some("manifest.json")
    {
        let bytes = std::fs::read(path)?;
        output.push(BackupFile {
            path: path
                .strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string(),
            sha256: hex::encode(Sha256::digest(&bytes)),
            bytes: bytes.len() as u64,
        });
    }
    Ok(())
}

fn tree_size(root: &Path) -> Result<u64> {
    if !root.exists() {
        return Ok(0);
    }
    let metadata = std::fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() {
        return Err(NexusError::PathDenied(format!(
            "state tree contains symlink {}",
            root.display()
        )));
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    let mut total = 0u64;
    for entry in std::fs::read_dir(root)? {
        total = total.saturating_add(tree_size(&entry?.path())?);
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifacts::ArtifactStore;

    #[test]
    fn backup_restores_database_and_artifact_hashes() {
        let directory = tempfile::tempdir().expect("directory");
        let state = directory.path().join("state");
        let store = Store::open(&state.join("nexus.db")).expect("store");
        store.meta_set("backup-test", "present").expect("meta");
        let artifacts = ArtifactStore::new(&state, store.clone()).expect("artifacts");
        artifacts
            .put(None, "test", "text/plain", b"artifact", None)
            .expect("artifact");

        let destination = directory.path().join("backup");
        let manifest = backup(&store, &state, &destination).expect("backup");
        assert!(manifest.files.iter().any(|file| file.path == "nexus.db"));
        let restored = Store::open(&destination.join("nexus.db")).expect("restored");
        assert_eq!(
            restored.meta_get("backup-test").expect("get").as_deref(),
            Some("present")
        );
        for file in &manifest.files {
            let bytes = std::fs::read(destination.join(&file.path)).expect("backup file");
            assert_eq!(hex::encode(Sha256::digest(&bytes)), file.sha256);
            assert_eq!(bytes.len() as u64, file.bytes);
        }
    }

    #[test]
    fn optimize_refuses_running_work() {
        let store = Store::open_in_memory().expect("store");
        let session = crate::SessionId::generate();
        store
            .with(|connection| {
                connection.execute(
                    "INSERT INTO sessions
                     (id,workspace,created_at,updated_at)
                     VALUES (?1,'/tmp',?2,?2)",
                    rusqlite::params![session.as_str(), crate::now_rfc3339()],
                )?;
                connection.execute(
                    "INSERT INTO timeline_events
                     (id,session_id,turn_id,trace_id,span_id,sequence,at,event_type,phase,status,summary,payload)
                     VALUES ('event',?1,'turn','trace','span',1,?2,'notice','started','running','running',
                             '{\"kind\":\"notice\",\"data\":{\"text\":\"running\",\"severity\":\"info\"}}')",
                    rusqlite::params![session.as_str(), crate::now_rfc3339()],
                )?;
                Ok(())
            })
            .expect("running");
        assert!(optimize(&store, false).is_err());
    }
}
