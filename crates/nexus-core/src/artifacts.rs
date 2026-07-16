//! Artifact store.
//!
//! Large tool outputs, downloads, diffs, and logs are written to
//! `.nexus/state/artifacts/<sha256-prefix>/<artifact-id>` and referenced by id
//! from the database and from model context, so full outputs never bloat the
//! conversation window and are still auditable.

use crate::error::Result;
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
    root: PathBuf,
    store: Store,
}

impl ArtifactStore {
    pub fn new(state_dir: &Path, store: Store) -> Result<Self> {
        let root = state_dir.join("artifacts");
        std::fs::create_dir_all(&root)?;
        Ok(Self { root, store })
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
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(id.as_str());
        std::fs::write(&path, content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        let record = ArtifactRecord {
            id: id.clone(),
            kind: kind.to_string(),
            path: path.clone(),
            sha256: sha.clone(),
            bytes: content.len(),
            content_type: content_type.to_string(),
        };
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO artifacts (id, session_id, kind, path, sha256, bytes, content_type, source_url, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    id.as_str(),
                    session.map(|s| s.as_str()),
                    kind,
                    path.display().to_string(),
                    sha,
                    content.len() as i64,
                    content_type,
                    source_url,
                    crate::now_rfc3339(),
                ],
            )?;
            Ok(())
        })?;
        Ok(record)
    }

    /// Read an artifact's content by id.
    pub fn get(&self, id: &ArtifactId) -> Result<Vec<u8>> {
        let path: String = self.store.with(|conn| {
            Ok(conn.query_row(
                "SELECT path FROM artifacts WHERE id = ?1",
                [id.as_str()],
                |r| r.get(0),
            )?)
        })?;
        Ok(std::fs::read(path)?)
    }

    pub fn root(&self) -> &Path {
        &self.root
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
    }
}
