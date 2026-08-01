//! Private state-directory creation, repair, and inspection.

use crate::{NexusError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionIssue {
    pub path: PathBuf,
    pub expected: u32,
    pub actual: u32,
}

/// Subtrees another program also writes into, where a symlink is that
/// program's business rather than evidence of tampering.
///
/// Only one so far: the isolated Codex profile under the auth root. The Codex
/// CLI keeps its scratch there while it runs — including `arg0` shim symlinks —
/// and NEXUS walked it as though it owned every entry. A concurrent `snx usage`
/// was therefore enough to stop `snx` from starting at all, since
/// `App::bootstrap` repairs the auth root and the global root above it, with
/// `private state contains symlink …` as the only explanation offered.
///
/// This is a carve-out in the *walk*, not in the guarantee: neither mode ever
/// follows a link, and modes are still repaired throughout. Refusing to
/// continue here protects nothing — it only lets a foreign process's ordinary
/// working files brick the CLI.
///
/// Matched on the trailing components so it holds for the real config root, a
/// `SILENT_NEXUS_CODEX_HOME` override, and a test temp dir alike.
const SHARED_SUBTREES: &[&[&str]] = &[&["auth", "codex"]];

fn is_shared(path: &Path) -> bool {
    SHARED_SUBTREES.iter().any(|subtree| {
        let mut want = subtree.iter().rev();
        let mut have = path.components().rev();
        want.all(|component| {
            have.next()
                .is_some_and(|actual| actual.as_os_str() == *component)
        })
    })
}

/// Create and repair a private tree: directories `0700`, regular files `0600`.
/// Symlinks are rejected rather than followed, except under [`SHARED_SUBTREES`].
pub fn repair_private_tree(root: &Path) -> Result<Vec<PermissionIssue>> {
    crate::atomic::ensure_directory_tree(root, 0o700)?;
    let mut repaired = Vec::new();
    repair_entry(root, is_shared(root), &mut repaired)?;
    Ok(repaired)
}

pub fn inspect_private_tree(root: &Path) -> Result<Vec<PermissionIssue>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut issues = Vec::new();
    inspect_entry(root, is_shared(root), &mut issues)?;
    Ok(issues)
}

fn repair_entry(path: &Path, shared: bool, repaired: &mut Vec<PermissionIssue>) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        if shared {
            return Ok(());
        }
        return Err(NexusError::PathDenied(format!(
            "private state contains symlink {}",
            path.display()
        )));
    }
    let expected = if metadata.is_dir() { 0o700 } else { 0o600 };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let actual = metadata.permissions().mode() & 0o777;
        if actual != expected {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(expected))?;
            repaired.push(PermissionIssue {
                path: path.to_path_buf(),
                expected,
                actual,
            });
        }
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let child = entry?.path();
            let shared = shared || is_shared(&child);
            repair_entry(&child, shared, repaired)?;
        }
    }
    Ok(())
}

fn inspect_entry(path: &Path, shared: bool, issues: &mut Vec<PermissionIssue>) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        if shared {
            return Ok(());
        }
        return Err(NexusError::PathDenied(format!(
            "private state contains symlink {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let expected = if metadata.is_dir() { 0o700 } else { 0o600 };
        let actual = metadata.permissions().mode() & 0o777;
        if actual != expected {
            issues.push(PermissionIssue {
                path: path.to_path_buf(),
                expected,
                actual,
            });
        }
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let child = entry?.path();
            let shared = shared || is_shared(&child);
            inspect_entry(&child, shared, issues)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn repairs_existing_private_tree_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().expect("directory");
        let root = directory.path().join("state");
        std::fs::create_dir_all(root.join("logs")).expect("create");
        std::fs::write(root.join("logs/log.jsonl"), "x").expect("write");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).expect("mode");
        std::fs::set_permissions(
            root.join("logs/log.jsonl"),
            std::fs::Permissions::from_mode(0o644),
        )
        .expect("file mode");

        let repaired = repair_private_tree(&root).expect("repair");
        assert!(!repaired.is_empty());
        assert!(inspect_private_tree(&root).expect("inspect").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_in_private_tree() {
        let directory = tempfile::tempdir().expect("directory");
        let root = directory.path().join("state");
        std::fs::create_dir_all(&root).expect("create");
        std::os::unix::fs::symlink("/tmp", root.join("escape")).expect("link");
        assert!(repair_private_tree(&root).is_err());
    }

    /// Reproduces a startup failure found by running the CLI: with a Codex
    /// subprocess mid-flight, `snx` refused to start because that process had
    /// an `arg0` shim symlink under its own scratch directory — which lives
    /// inside the NEXUS auth root, which `App::bootstrap` repairs.
    ///
    /// The scratch belongs to Codex. Refusing to continue there protected
    /// nothing and stopped everything.
    #[cfg(unix)]
    #[test]
    fn a_codex_subprocess_scratch_symlink_does_not_stop_startup() {
        let directory = tempfile::tempdir().expect("directory");
        let global = directory.path().join("silent-nexus");
        let scratch = global.join("auth/codex/tmp/arg0/codex-abc123");
        std::fs::create_dir_all(&scratch).expect("create");
        std::os::unix::fs::symlink("/usr/bin/true", scratch.join("applypatch")).expect("link");

        // Both the auth root and the global root above it are repaired at
        // startup, and neither may fail.
        for root in [global.join("auth"), global.clone()] {
            repair_private_tree(&root).unwrap_or_else(|e| panic!("{}: {e}", root.display()));
        }
        assert!(inspect_private_tree(&global).is_ok());

        // The carve-out is the Codex tree only — NEXUS still owns its own.
        std::os::unix::fs::symlink("/tmp", global.join("escape")).expect("link");
        assert!(
            repair_private_tree(&global).is_err(),
            "a symlink NEXUS did not put there must still stop the walk"
        );
    }

    #[cfg(unix)]
    #[test]
    fn nested_symlink_parent_is_rejected_without_creating_outside() {
        let directory = tempfile::tempdir().expect("directory");
        let outside = tempfile::tempdir().expect("outside");
        let linked = directory.path().join("linked");
        std::os::unix::fs::symlink(outside.path(), &linked).expect("link");
        assert!(repair_private_tree(&linked.join("new/state")).is_err());
        assert!(!outside.path().join("new").exists());
    }
}
