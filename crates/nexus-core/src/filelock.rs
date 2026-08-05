//! Advisory whole-file locking for state a second process can also write.
//!
//! Any state file that more than one Silent Nexus process edits needs this.
//! Without it, a read-modify-write is not one operation: the CLI can persist a
//! change between another process's read and its write, and that change is then
//! overwritten by a value the writer never saw. Nothing errors, nothing logs,
//! and the operator's choice is simply gone.
//!
//! The lock is advisory and tied to the open file, so it is released when the
//! handle closes — including on crash or kill — and a dead process cannot wedge
//! the harness.

use crate::{NexusError, Result};
use std::fs::TryLockError;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// An exclusive lock, released when dropped.
#[derive(Debug)]
pub struct FileLock {
    // Held for its file descriptor. Dropping closes it, which drops the lock.
    _file: std::fs::File,
}

/// The lock path for a state file: the file's own name plus `.lock`.
///
/// Deliberately a sibling rather than the file itself, because the state file
/// is replaced by `rename(2)` on every write. Locking it directly would leave
/// each writer holding a descriptor to a different, already-unlinked inode —
/// mutual exclusion between processes that never see each other.
pub fn lock_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    path.with_file_name(name)
}

impl FileLock {
    /// Take the exclusive lock for `path`, blocking until it is available.
    ///
    /// The wait is bounded: a stuck holder must not hang the CLI forever, so
    /// this gives up rather than blocking indefinitely. Timing out is reported,
    /// never silently downgraded to an unlocked write.
    pub fn acquire(path: &Path) -> Result<Self> {
        Self::acquire_with_timeout(path, std::time::Duration::from_secs(5))
    }

    pub fn acquire_with_timeout(path: &Path, timeout: std::time::Duration) -> Result<Self> {
        let lock = lock_path(path);
        if let Some(parent) = lock.parent() {
            crate::permissions::repair_private_tree(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&lock)?;

        let deadline = std::time::Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                // Someone else holds it. Retry until the deadline.
                Err(TryLockError::WouldBlock) => {}
                // A real failure — a filesystem that cannot lock, for example.
                // Reported, never downgraded to an unlocked write.
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
            if std::time::Instant::now() >= deadline {
                return Err(NexusError::Other(format!(
                    "timed out waiting for the lock on {} — another Silent Nexus process is \
                     holding it",
                    lock.display()
                )));
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_is_a_sibling_of_the_file_it_guards() {
        // Locking the state file itself would be useless: every write replaces
        // it via rename, so two writers would hold descriptors to different
        // inodes and never contend.
        let path = Path::new("/tmp/nexus/state.json");
        assert_eq!(lock_path(path), Path::new("/tmp/nexus/state.json.lock"));
    }

    #[test]
    fn a_second_acquire_waits_and_then_reports_rather_than_writing_anyway() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("state.json");
        let held = FileLock::acquire(&path).expect("first lock");

        let error = FileLock::acquire_with_timeout(&path, std::time::Duration::from_millis(60))
            .expect_err("the second acquire must not succeed while the first is held");
        assert!(
            error.to_string().contains("another Silent Nexus process"),
            "a contended lock must say so, not fall back to an unlocked write: {error}"
        );

        drop(held);
        FileLock::acquire(&path).expect("the lock is released on drop");
    }

    #[test]
    fn dropping_the_lock_releases_it_for_the_next_writer() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("state.json");
        for _ in 0..3 {
            let lock = FileLock::acquire(&path).expect("acquire");
            drop(lock);
        }
    }
}
