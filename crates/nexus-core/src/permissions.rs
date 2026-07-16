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

/// Create and repair a private tree: directories `0700`, regular files `0600`.
/// Symlinks are rejected rather than followed.
pub fn repair_private_tree(root: &Path) -> Result<Vec<PermissionIssue>> {
    crate::atomic::ensure_directory_tree(root, 0o700)?;
    let mut repaired = Vec::new();
    repair_entry(root, &mut repaired)?;
    Ok(repaired)
}

pub fn inspect_private_tree(root: &Path) -> Result<Vec<PermissionIssue>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut issues = Vec::new();
    inspect_entry(root, &mut issues)?;
    Ok(issues)
}

fn repair_entry(path: &Path, repaired: &mut Vec<PermissionIssue>) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
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
            repair_entry(&entry?.path(), repaired)?;
        }
    }
    Ok(())
}

fn inspect_entry(path: &Path, issues: &mut Vec<PermissionIssue>) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
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
            inspect_entry(&entry?.path(), issues)?;
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
