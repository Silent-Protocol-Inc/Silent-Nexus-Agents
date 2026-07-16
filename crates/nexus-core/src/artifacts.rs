//! Artifact store.
//!
//! Large tool outputs, downloads, diffs, and logs are written to
//! `.nexus/state/artifacts/<sha256-prefix>/<artifact-id>` and referenced by id
//! from the database and from model context, so full outputs never bloat the
//! conversation window and are still auditable.

use crate::error::{NexusError, Result};
use crate::ids::{ArtifactId, SessionId};
use crate::store::Store;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ArtifactRecord {
    pub id: ArtifactId,
    pub kind: String,
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: usize,
    pub content_type: String,
}

#[derive(Debug, Clone)]
pub struct ArtifactStore {
    state_root: PathBuf,
    root: PathBuf,
    store: Store,
}

impl ArtifactStore {
    pub fn new(state_dir: &Path, store: Store) -> Result<Self> {
        let root = state_dir.join("artifacts");
        crate::permissions::repair_private_tree(&root)?;
        Ok(Self {
            state_root: state_dir.to_path_buf(),
            root,
            store,
        })
    }

    /// Persist `content` as an artifact and record it. Content is stored
    /// verbatim; callers redact secrets *before* storing when the artifact
    /// will be surfaced to models or logs.
    pub fn put(
        &self,
        session: Option<&SessionId>,
        kind: &str,
        content_type: &str,
        content: &[u8],
        source_url: Option<&str>,
    ) -> Result<ArtifactRecord> {
        let id = ArtifactId::generate();
        let sha = hex::encode(Sha256::digest(content));
        let dir = self.root.join(&sha[..2]);
        crate::permissions::repair_private_tree(&dir)?;
        let path = dir.join(id.as_str());
        crate::atomic::atomic_write_private(&path, content)?;
        let relative = path.strip_prefix(&self.state_root).map_err(|_| {
            NexusError::PathDenied(format!(
                "artifact path escaped state root: {}",
                path.display()
            ))
        })?;
        let record = ArtifactRecord {
            id: id.clone(),
            kind: kind.to_string(),
            path: path.clone(),
            sha256: sha.clone(),
            bytes: content.len(),
            content_type: content_type.to_string(),
        };
        let inserted = self.store.with(|conn| {
            conn.execute(
                "INSERT INTO artifacts (id, session_id, kind, path, sha256, bytes, content_type, source_url, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    id.as_str(),
                    session.map(|s| s.as_str()),
                    kind,
                    relative.display().to_string(),
                    sha,
                    content.len() as i64,
                    content_type,
                    source_url,
                    crate::now_rfc3339(),
                ],
            )?;
            Ok(())
        });
        if inserted.is_err() {
            let _ = std::fs::remove_file(&path);
        }
        inserted?;
        Ok(record)
    }

    /// Read an artifact's content by id.
    pub fn get(&self, id: &ArtifactId) -> Result<Vec<u8>> {
        let (stored_path, expected_sha, expected_bytes): (String, String, i64) =
            self.store.with(|conn| {
                Ok(conn.query_row(
                    "SELECT path, sha256, bytes FROM artifacts WHERE id = ?1",
                    [id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?)
            })?;
        let path = self.resolve_stored_path(&stored_path)?;
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(NexusError::PathDenied(format!(
                "artifact {} is not a regular no-follow file",
                id.as_str()
            )));
        }
        let bytes = std::fs::read(&path)?;
        if expected_bytes < 0 || bytes.len() as i64 != expected_bytes {
            return Err(NexusError::Other(format!(
                "artifact {} size mismatch: expected {}, found {}",
                id.as_str(),
                expected_bytes,
                bytes.len()
            )));
        }
        let actual_sha = hex::encode(Sha256::digest(&bytes));
        if actual_sha != expected_sha {
            return Err(NexusError::Other(format!(
                "artifact {} SHA-256 mismatch",
                id.as_str()
            )));
        }
        Ok(bytes)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn verify_all(&self) -> Result<Vec<String>> {
        let ids = self.store.with(|conn| {
            let mut statement = conn.prepare("SELECT id FROM artifacts ORDER BY id")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row?);
            }
            Ok(ids)
        })?;
        let mut issues = Vec::new();
        for id in ids {
            if let Err(error) = self.get(&ArtifactId::from(id.clone())) {
                issues.push(format!("{id}: {error}"));
            }
        }
        Ok(issues)
    }

    fn resolve_stored_path(&self, stored: &str) -> Result<PathBuf> {
        let stored_path = PathBuf::from(stored);
        let candidate = if stored_path.is_absolute() {
            stored_path
        } else {
            self.state_root.join(&stored_path)
        };
        let relative = candidate.strip_prefix(&self.state_root).map_err(|_| {
            NexusError::PathDenied(format!(
                "artifact path is not rooted in state: {}",
                candidate.display()
            ))
        })?;
        if relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        }) {
            return Err(NexusError::PathDenied(format!(
                "artifact path contains unsafe components: {}",
                candidate.display()
            )));
        }
        let mut current = self.state_root.clone();
        for component in relative.components() {
            current.push(component.as_os_str());
            let metadata = std::fs::symlink_metadata(&current).map_err(|error| {
                NexusError::PathDenied(format!("artifact path {}: {error}", current.display()))
            })?;
            if metadata.file_type().is_symlink() {
                return Err(NexusError::PathDenied(format!(
                    "artifact path contains symlink {}",
                    current.display()
                )));
            }
        }
        let root = self
            .state_root
            .canonicalize()
            .map_err(|error| NexusError::PathDenied(format!("state root: {error}")))?;
        let real = candidate.canonicalize().map_err(|error| {
            NexusError::PathDenied(format!("artifact path {}: {error}", candidate.display()))
        })?;
        if !real.starts_with(&root) {
            return Err(NexusError::PathDenied(format!(
                "artifact path escaped state root: {}",
                real.display()
            )));
        }
        Ok(real)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open_in_memory().expect("store");
        let artifacts = ArtifactStore::new(dir.path(), store).expect("artifacts");
        let rec = artifacts
            .put(None, "tool_output", "text/plain", b"hello world", None)
            .expect("put");
        assert_eq!(rec.bytes, 11);
        let back = artifacts.get(&rec.id).expect("get");
        assert_eq!(back, b"hello world");
        let stored: String = artifacts
            .store
            .with(|connection| {
                Ok(connection.query_row(
                    "SELECT path FROM artifacts WHERE id=?1",
                    [rec.id.as_str()],
                    |row| row.get(0),
                )?)
            })
            .expect("stored path");
        assert!(!Path::new(&stored).is_absolute());
    }

    #[test]
    fn tampered_artifact_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open_in_memory().expect("store");
        let artifacts = ArtifactStore::new(dir.path(), store).expect("artifacts");
        let record = artifacts
            .put(None, "tool_output", "text/plain", b"original", None)
            .expect("put");
        std::fs::write(&record.path, "tampered").expect("tamper");
        assert!(artifacts.get(&record.id).is_err());
        assert_eq!(artifacts.verify_all().expect("verify").len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn artifact_symlink_substitution_is_rejected_even_inside_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open_in_memory().expect("store");
        let artifacts = ArtifactStore::new(dir.path(), store).expect("artifacts");
        let record = artifacts
            .put(None, "tool_output", "text/plain", b"original", None)
            .expect("put");
        let replacement = artifacts.root.join("replacement");
        std::fs::write(&replacement, "original").expect("replacement");
        std::fs::remove_file(&record.path).expect("remove");
        std::os::unix::fs::symlink(&replacement, &record.path).expect("symlink");
        assert!(matches!(
            artifacts.get(&record.id),
            Err(NexusError::PathDenied(_))
        ));
    }

    #[test]
    fn verified_legacy_absolute_paths_remain_readable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open_in_memory().expect("store");
        let artifacts = ArtifactStore::new(dir.path(), store.clone()).expect("artifacts");
        let path = artifacts.root.join("legacy");
        std::fs::write(&path, "legacy").expect("write");
        let sha = hex::encode(Sha256::digest(b"legacy"));
        store
            .with(|connection| {
                connection.execute(
                    "INSERT INTO artifacts
                     (id,kind,path,sha256,bytes,content_type,created_at)
                     VALUES ('artifact_legacy','tool_output',?1,?2,6,'text/plain',?3)",
                    rusqlite::params![path.display().to_string(), sha, crate::now_rfc3339()],
                )?;
                Ok(())
            })
            .expect("insert");
        assert_eq!(
            artifacts
                .get(&ArtifactId::from("artifact_legacy"))
                .expect("read"),
            b"legacy"
        );
    }
}
