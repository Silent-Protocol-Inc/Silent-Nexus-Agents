//! Atomic, no-follow file replacement for private NEXUS state.

use crate::{NexusError, Result};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// Replace `path` with `contents` using a same-directory temporary file.
///
/// The target and every existing parent component must not be symlinks. The
/// temporary file is opened with `O_NOFOLLOW`, synced, renamed over the target,
/// and followed by a directory sync. On Unix, `mode` is applied before data is
/// written so a crash cannot expose a permissive private file.
pub fn atomic_write(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    atomic_write_inner(path, contents, mode, |_| Ok(()))
}

fn atomic_write_inner(
    path: &Path,
    contents: &[u8],
    mode: u32,
    before_rename: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let absolute = absolute_path(path)?;
    let parent = absolute
        .parent()
        .ok_or_else(|| NexusError::PathDenied(format!("{} has no parent", absolute.display())))?;
    let directory_mode = if mode & 0o077 == 0 { 0o700 } else { 0o755 };
    ensure_directory_tree(parent, directory_mode)?;
    reject_symlink_components(parent)?;
    reject_symlink_target(&absolute)?;

    let name = absolute
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("state");
    let temporary = parent.join(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));

    let result = (|| -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(mode)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let mut file = options.open(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        file.write_all(contents)?;
        file.sync_all()?;
        before_rename(&temporary)?;
        std::fs::rename(&temporary, &absolute)?;
        let directory = std::fs::File::open(parent)?;
        directory.sync_all()?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub fn atomic_write_private(path: &Path, contents: &[u8]) -> Result<()> {
    atomic_write(path, contents, 0o600)
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn reject_symlink_target(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(NexusError::PathDenied(format!(
            "refusing to replace symlink {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Create a directory tree without following existing symlink components.
///
/// Callers choose the final directory mode (`0700` for private state, `0755`
/// for ordinary workspace directories).
pub fn ensure_directory_tree(path: &Path, mode: u32) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                current.push(component.as_os_str());
                continue;
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err(NexusError::PathDenied(format!(
                    "parent traversal in {}",
                    path.display()
                )));
            }
            Component::Normal(_) => current.push(component.as_os_str()),
        }
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(NexusError::PathDenied(format!(
                    "symlink component {}",
                    current.display()
                )));
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(NexusError::PathDenied(format!(
                    "non-directory parent component {}",
                    current.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut builder = std::fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(mode);
                }
                builder.create(&current)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&current, std::fs::Permissions::from_mode(mode))?;
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn reject_symlink_components(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                current.push(component.as_os_str());
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err(NexusError::PathDenied(format!(
                    "parent traversal in {}",
                    path.display()
                )));
            }
        }
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(NexusError::PathDenied(format!(
                    "symlink component {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_leaves_no_temporary_file() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("state.json");
        atomic_write_private(&path, b"one").expect("first");
        atomic_write_private(&path, b"two").expect("second");
        assert_eq!(std::fs::read(&path).expect("read"), b"two");
        assert_eq!(
            std::fs::read_dir(directory.path())
                .expect("read directory")
                .count(),
            1
        );
    }

    #[test]
    fn interrupted_replacement_preserves_the_previous_file() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("state.json");
        atomic_write_private(&path, b"stable").expect("seed");
        let result = atomic_write_inner(&path, b"partial", 0o600, |_| {
            Err(NexusError::Other("simulated interruption".into()))
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).expect("read"), b"stable");
        assert_eq!(
            std::fs::read_dir(directory.path())
                .expect("read directory")
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_refuses_symlink_targets_and_parents() {
        let directory = tempfile::tempdir().expect("directory");
        let outside = tempfile::tempdir().expect("outside");
        let target = outside.path().join("secret");
        std::fs::write(&target, "unchanged").expect("seed");

        let link = directory.path().join("state");
        std::os::unix::fs::symlink(&target, &link).expect("target link");
        assert!(atomic_write_private(&link, b"attack").is_err());
        assert_eq!(std::fs::read_to_string(&target).expect("read"), "unchanged");

        let linked_parent = directory.path().join("linked");
        std::os::unix::fs::symlink(outside.path(), &linked_parent).expect("parent link");
        assert!(atomic_write_private(&linked_parent.join("new"), b"attack").is_err());
        assert!(!outside.path().join("new").exists());

        assert!(atomic_write_private(&linked_parent.join("created/new"), b"attack").is_err());
        assert!(!outside.path().join("created").exists());
    }
}
