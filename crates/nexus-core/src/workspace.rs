//! Workspace boundary enforcement.
//!
//! Every filesystem path a tool receives — from a model, a config file, or a
//! user — must be resolved through [`WorkspaceGuard`] before use. The guard
//! canonicalizes paths (resolving `..` and symlinks) and rejects anything that
//! lands outside the workspace root or inside a denied subtree.

use crate::error::{NexusError, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::{Component, Path, PathBuf};

/// Guards a single workspace root.
#[derive(Debug, Clone)]
pub struct WorkspaceGuard {
    root: PathBuf,
    denied: GlobSet,
    denied_patterns: Vec<String>,
}

/// Subtrees that are never readable or writable through tools, regardless of
/// policy, because they routinely contain credentials.
const BUILTIN_DENIED: &[&str] = &[
    "**/.nexus/secrets*",
    "**/.ssh/**",
    "**/.gnupg/**",
    "**/.aws/credentials",
    "**/.config/gcloud/**",
];

impl WorkspaceGuard {
    /// Create a guard rooted at `root`. `root` must exist; it is canonicalized
    /// so later comparisons are symlink-stable.
    pub fn new(root: impl AsRef<Path>, extra_denied: &[String]) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|e| NexusError::Config(format!("workspace root: {e}")))?;
        let mut builder = GlobSetBuilder::new();
        let mut denied_patterns = Vec::new();
        for pat in BUILTIN_DENIED
            .iter()
            .copied()
            .map(str::to_string)
            .chain(extra_denied.iter().cloned())
        {
            let glob = Glob::new(&pat)
                .map_err(|e| NexusError::Config(format!("denied path pattern `{pat}`: {e}")))?;
            builder.add(glob);
            denied_patterns.push(pat);
        }
        let denied = builder
            .build()
            .map_err(|e| NexusError::Config(format!("denied path set: {e}")))?;
        Ok(Self {
            root,
            denied,
            denied_patterns,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn denied_patterns(&self) -> &[String] {
        &self.denied_patterns
    }

    /// Resolve a path (absolute or workspace-relative) for **reading**.
    /// The target must exist; symlinks are fully resolved and the final
    /// real path must stay inside the workspace.
    pub fn resolve_existing(&self, input: impl AsRef<Path>) -> Result<PathBuf> {
        let joined = self.join(input.as_ref());
        let real = joined
            .canonicalize()
            .map_err(|_| NexusError::NotFound(joined.display().to_string()))?;
        self.check_inside(&real)?;
        self.check_denied(&real)?;
        Ok(real)
    }

    /// Resolve a path for **creation**: the parent must exist inside the
    /// workspace; the leaf may be new. Rejects a leaf that already exists as a
    /// symlink (symlink-swap escape protection).
    pub fn resolve_for_write(&self, input: impl AsRef<Path>) -> Result<PathBuf> {
        let joined = self.join(input.as_ref());
        let file_name = joined
            .file_name()
            .ok_or_else(|| NexusError::PathDenied("path has no file name".into()))?
            .to_os_string();
        let parent = joined
            .parent()
            .ok_or_else(|| NexusError::PathDenied("path has no parent".into()))?;
        // Walk up to the nearest existing ancestor and canonicalize from there
        // so `a/b/c/new.txt` works when `b/c` do not exist yet.
        let mut existing = parent.to_path_buf();
        let mut suffix: Vec<std::ffi::OsString> = Vec::new();
        while !existing.exists() {
            let Some(name) = existing.file_name().map(|n| n.to_os_string()) else {
                return Err(NexusError::PathEscape(joined.display().to_string()));
            };
            suffix.push(name);
            let Some(p) = existing.parent() else {
                return Err(NexusError::PathEscape(joined.display().to_string()));
            };
            existing = p.to_path_buf();
        }
        let real_parent = existing
            .canonicalize()
            .map_err(|e| NexusError::PathDenied(format!("{}: {e}", existing.display())))?;
        self.check_inside(&real_parent)?;
        let mut full = real_parent;
        for part in suffix.iter().rev() {
            full.push(part);
        }
        full.push(&file_name);
        // If the leaf exists and is a symlink, resolve and re-check.
        if full
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            let target = full
                .canonicalize()
                .map_err(|e| NexusError::PathDenied(format!("symlink target: {e}")))?;
            self.check_inside(&target)?;
        }
        self.check_denied(&full)?;
        Ok(full)
    }

    /// Display a guarded path relative to the workspace root.
    pub fn display_relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| path.display().to_string())
    }

    fn join(&self, input: &Path) -> PathBuf {
        let mut out = if input.is_absolute() {
            input.to_path_buf()
        } else {
            self.root.join(input)
        };
        // Normalize lexical `..`/`.` before touching the filesystem so we do
        // not depend on intermediate components existing.
        let mut normalized = PathBuf::new();
        for comp in out.components() {
            match comp {
                Component::ParentDir => {
                    normalized.pop();
                }
                Component::CurDir => {}
                other => normalized.push(other.as_os_str()),
            }
        }
        out = normalized;
        out
    }

    fn check_inside(&self, real: &Path) -> Result<()> {
        if real.starts_with(&self.root) {
            Ok(())
        } else {
            Err(NexusError::PathEscape(real.display().to_string()))
        }
    }

    fn check_denied(&self, path: &Path) -> Result<()> {
        if self.denied.is_match(path) {
            return Err(NexusError::PathDenied(path.display().to_string()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> (tempfile::TempDir, WorkspaceGuard) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("inside.txt"), "ok").expect("write");
        std::fs::create_dir_all(dir.path().join("sub/deep")).expect("mkdir");
        let g = WorkspaceGuard::new(dir.path(), &[]).expect("guard");
        (dir, g)
    }

    #[test]
    fn allows_inside_paths() {
        let (_d, g) = guard();
        assert!(g.resolve_existing("inside.txt").is_ok());
        assert!(g.resolve_for_write("sub/newfile.rs").is_ok());
        assert!(g.resolve_for_write("sub/deep/a/b/new.rs").is_ok());
    }

    #[test]
    fn rejects_dotdot_traversal() {
        let (_d, g) = guard();
        assert!(matches!(
            g.resolve_existing("../../../etc/passwd"),
            Err(NexusError::PathEscape(_)) | Err(NexusError::NotFound(_))
        ));
        assert!(g.resolve_for_write("sub/../../outside.txt").is_err());
    }

    #[test]
    fn rejects_absolute_outside() {
        let (_d, g) = guard();
        assert!(g.resolve_existing("/etc/passwd").is_err());
        assert!(g.resolve_for_write("/tmp/evil.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        let (d, g) = guard();
        std::os::unix::fs::symlink("/etc", d.path().join("link")).expect("symlink");
        assert!(g.resolve_existing("link/passwd").is_err());
        // Writing through a symlinked leaf that points outside must fail too.
        std::os::unix::fs::symlink("/tmp/target.txt", d.path().join("out.txt")).expect("symlink");
        assert!(g.resolve_for_write("out.txt").is_err());
    }

    #[test]
    fn denies_builtin_sensitive_paths() {
        let (d, g) = guard();
        std::fs::create_dir_all(d.path().join(".ssh")).expect("mkdir");
        std::fs::write(d.path().join(".ssh/id_rsa"), "key").expect("write");
        assert!(matches!(
            g.resolve_existing(".ssh/id_rsa"),
            Err(NexusError::PathDenied(_))
        ));
    }
}
