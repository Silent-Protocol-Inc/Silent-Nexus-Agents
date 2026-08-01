//! Candidate isolation.
//!
//! A self-modifying candidate is **never** validated against production state.
//! Data-plane candidates get a disposable overlay workspace (a temp directory and
//! its own throwaway database); code-plane candidates get a real git worktree so
//! a patch can be compiled and tested against a checkout that is discarded
//! afterwards. This mirrors the harness's existing `snx/task/<id>` writer-worktree
//! pattern, kept off the source checkout.

use nexus_core::{NexusError, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A prepared isolated environment. Cleanup happens on drop: the temp directory
/// is removed, and a git worktree is deregistered first so no stale entry is left
/// in the source repository.
pub struct Isolate {
    path: PathBuf,
    /// Owns the temp directory lifetime; dropping it removes the files.
    _temp: tempfile::TempDir,
    /// When set, the source repo to deregister the worktree from on drop.
    worktree_repo: Option<PathBuf>,
}

impl Isolate {
    /// The working directory validators should run in.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_worktree(&self) -> bool {
        self.worktree_repo.is_some()
    }
}

impl Drop for Isolate {
    fn drop(&mut self) {
        if let Some(repo) = &self.worktree_repo {
            // Best effort: deregister before the temp dir removes the files.
            let _ = Command::new("git")
                .arg("-C")
                .arg(repo)
                .arg("worktree")
                .arg("remove")
                .arg("--force")
                .arg(&self.path)
                .output();
        }
    }
}

/// Prepares an isolated environment for a candidate.
pub trait IsolationProvider {
    fn prepare(&self, candidate_id: &str) -> Result<Isolate>;
}

/// A disposable overlay workspace: a fresh temp directory with nothing shared
/// from production. Suitable for data-plane candidates (config/prompt/memory).
pub struct OverlayIsolation;

impl IsolationProvider for OverlayIsolation {
    fn prepare(&self, _candidate_id: &str) -> Result<Isolate> {
        let temp = tempfile::tempdir()
            .map_err(|e| NexusError::Other(format!("overlay isolation tempdir: {e}")))?;
        let path = temp.path().to_path_buf();
        Ok(Isolate {
            path,
            _temp: temp,
            worktree_repo: None,
        })
    }
}

/// A git worktree isolation for code-plane candidates: a detached checkout of the
/// repository's HEAD at a throwaway path, so a patch can be built and tested
/// without touching the source checkout.
pub struct WorktreeIsolation {
    repo_root: PathBuf,
}

impl WorktreeIsolation {
    pub fn new(repo_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
        }
    }
}

impl IsolationProvider for WorktreeIsolation {
    fn prepare(&self, candidate_id: &str) -> Result<Isolate> {
        let temp = tempfile::tempdir()
            .map_err(|e| NexusError::Other(format!("worktree isolation tempdir: {e}")))?;
        // git worktree add wants a path that does not yet exist.
        let sanitized: String = candidate_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let worktree_path = temp.path().join(format!("wt_{sanitized}"));
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .arg("worktree")
            .arg("add")
            .arg("--detach")
            .arg(&worktree_path)
            .arg("HEAD")
            .output()
            .map_err(|e| NexusError::Other(format!("git worktree add: {e}")))?;
        if !output.status.success() {
            return Err(NexusError::Other(format!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(Isolate {
            path: worktree_path,
            _temp: temp,
            worktree_repo: Some(self.repo_root.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(args: &[&str], cwd: &Path) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git runs")
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    }

    #[test]
    fn overlay_prepares_a_disposable_directory() {
        let isolate = OverlayIsolation.prepare("cnd-1").expect("overlay");
        assert!(isolate.path().is_dir());
        assert!(!isolate.is_worktree());
        let path = isolate.path().to_path_buf();
        drop(isolate);
        assert!(!path.exists(), "overlay dir removed on drop");
    }

    #[test]
    fn worktree_prepares_a_detached_checkout_and_cleans_up() {
        // A throwaway source repo with one commit.
        let repo = tempfile::tempdir().expect("repo dir");
        git(&["init", "-q"], repo.path());
        git(&["config", "user.email", "t@t"], repo.path());
        git(&["config", "user.name", "t"], repo.path());
        std::fs::write(repo.path().join("README.md"), "hi").expect("write");
        git(&["add", "."], repo.path());
        git(&["commit", "-qm", "init"], repo.path());

        let iso = WorktreeIsolation::new(repo.path());
        let isolate = iso.prepare("cnd-42").expect("worktree");
        assert!(isolate.is_worktree());
        assert!(
            isolate.path().join("README.md").exists(),
            "checkout populated"
        );
        let worktree_path = isolate.path().to_path_buf();
        drop(isolate);
        assert!(!worktree_path.exists(), "worktree removed on drop");
    }
}
