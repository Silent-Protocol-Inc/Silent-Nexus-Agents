//! Workspace boundary enforcement.
//!
//! Every filesystem path a tool receives — from a model, a config file, or a
//! user — must be resolved through [`WorkspaceGuard`] before use. The guard
//! canonicalizes paths (resolving `..` and symlinks) and rejects anything that
//! lands outside the workspace root or inside a denied subtree.

use crate::error::{NexusError, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Guards a single workspace root.
#[derive(Debug, Clone)]
pub struct WorkspaceGuard {
    /// Set once the operator has answered full access's question about the
    /// safety class, unlocking the *denied-path* refusal for this session.
    ///
    /// It never touches workspace confinement or the symlink-escape checks:
    /// those answer "is this file inside the workspace at all", which is not a
    /// preference and has no mode. This only lifts "this path is one you told
    /// me to keep away from", which is exactly what full access is asking
    /// about. Shared with the owning application, so it is live and dies with
    /// the process.
    unlocked: Option<Arc<AtomicBool>>,
    root: PathBuf,
    denied: GlobSet,
    denied_patterns: Vec<String>,
}

/// Subtrees that are never readable or writable through tools, regardless of
/// policy, because they routinely contain credentials.
const BUILTIN_DENIED: &[&str] = &[
    "**/.nexus",
    "**/.nexus/**",
    "**/.git",
    "**/.git/**",
    "**/.env",
    "**/.netrc",
    "**/.npmrc",
    "**/.pypirc",
    "**/.git-credentials",
    "**/credentials.json",
    "**/credentials.toml",
    "**/credentials.yaml",
    "**/credentials.yml",
    "**/credentials.ini",
    "**/credentials.env",
    "**/credentials.txt",
    "**/auth.json",
    "**/*.pem",
    "**/*.key",
    "**/*.p12",
    "**/*.pfx",
    "**/id_rsa",
    "**/id_ed25519",
    "**/.ssh/**",
    "**/.gnupg/**",
    "**/.aws/**",
    "**/.azure/**",
    "**/.kube/**",
    "**/.docker/config.json",
    "**/.config/gcloud/**",
    "**/.config/gh/**",
];
const SENSITIVE_SCAN_LIMIT: usize = 50_000;

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
            unlocked: None,
            root,
            denied,
            denied_patterns,
        })
    }

    /// Share the session's safety answer, so an approved session can reach the
    /// paths this guard otherwise keeps away from. Confinement is unaffected.
    pub fn with_unlock(mut self, unlocked: Arc<AtomicBool>) -> Self {
        self.unlocked = Some(unlocked);
        self
    }

    /// Whether this session has answered full access's question about the
    /// safety class. Callers with their own path refusal consult it so the
    /// answer is not contradicted one layer further in.
    pub fn denied_paths_unlocked(&self) -> bool {
        self.unlocked
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn denied_patterns(&self) -> &[String] {
        &self.denied_patterns
    }

    /// Existing sensitive paths that a model-run container must mask. The
    /// result is workspace-relative and never follows symlinks.
    pub fn sensitive_paths_for_masking(&self) -> Result<Vec<PathBuf>> {
        self.sensitive_paths_for_masking_with_limit(SENSITIVE_SCAN_LIMIT)
    }

    fn sensitive_paths_for_masking_with_limit(&self, limit: usize) -> Result<Vec<PathBuf>> {
        let mut paths = Vec::new();
        let mut visited = 0usize;
        collect_sensitive_paths(&self.root, &self.root, &mut paths, &mut visited, limit)?;
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    /// Resolve a path (absolute or workspace-relative) for **reading**.
    /// The target must exist; symlinks are fully resolved and the final
    /// real path must stay inside the workspace.
    pub fn resolve_existing(&self, input: impl AsRef<Path>) -> Result<PathBuf> {
        let joined = self.join(input.as_ref());
        self.check_denied(&joined)?;
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
        self.check_denied(&joined)?;
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
        // Existing symlink leaves are always rejected. Even an in-workspace
        // target can be swapped between validation and write.
        if full
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(NexusError::PathDenied(format!(
                "refusing to write through symlink {}",
                full.display()
            )));
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
        if self.denied_paths_unlocked() {
            return Ok(());
        }
        if is_private_env_variant(path) {
            return Err(NexusError::PathDenied(path.display().to_string()));
        }
        if self.denied.is_match(path) {
            return Err(NexusError::PathDenied(path.display().to_string()));
        }
        Ok(())
    }
}

fn is_public_example(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some(".env.example")
}

fn is_private_env_variant(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".env.") && name != ".env.example")
}

fn collect_sensitive_paths(
    root: &Path,
    directory: &Path,
    output: &mut Vec<PathBuf>,
    visited: &mut usize,
    limit: usize,
) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        if *visited >= limit {
            return Err(NexusError::PathDenied(format!(
                "sensitive-path scan exceeded {limit} entries; refusing unmasked container execution"
            )));
        }
        *visited += 1;
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let metadata = std::fs::symlink_metadata(&path)?;
        if is_sensitive_relative(relative) {
            output.push(relative.to_path_buf());
            continue;
        }
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            collect_sensitive_paths(root, &path, output, visited, limit)?;
        }
    }
    Ok(())
}

fn is_sensitive_relative(path: &Path) -> bool {
    let components = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    if components.iter().any(|component| {
        matches!(
            *component,
            ".git" | ".nexus" | ".ssh" | ".gnupg" | ".aws" | ".azure" | ".kube" | ".docker"
        )
    }) {
        return true;
    }
    if components
        .windows(2)
        .any(|pair| pair == [".config", "gcloud"] || pair == [".config", "gh"])
    {
        return true;
    }
    if is_public_example(path) {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    lower == ".env"
        || lower.starts_with(".env.")
        || matches!(
            lower.as_str(),
            ".netrc"
                | ".npmrc"
                | ".pypirc"
                | ".git-credentials"
                | "credentials.json"
                | "auth.json"
                | "id_rsa"
                | "id_ed25519"
        )
        || matches!(
            lower.as_str(),
            "credentials.toml"
                | "credentials.yaml"
                | "credentials.yml"
                | "credentials.ini"
                | "credentials.env"
                | "credentials.txt"
        )
        || [".pem", ".key", ".p12", ".pfx"]
            .iter()
            .any(|extension| lower.ends_with(extension))
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

    /// A session that has answered full access's one question can read the
    /// paths the guard keeps away from — that question is precisely "may this
    /// session touch the safety class".
    ///
    /// What the answer cannot do is move the workspace boundary. Confinement
    /// and symlink-escape answer "is this file inside the workspace at all",
    /// which is not a preference and has no mode, so they are unaffected.
    #[test]
    fn the_session_answer_unlocks_denied_paths_and_nothing_else() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(directory.path().join(".env"), "SECRET=1").expect("write");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("elsewhere.txt"), "x").expect("write");

        let unlocked = Arc::new(AtomicBool::new(false));
        let guard = WorkspaceGuard::new(directory.path(), &[])
            .expect("guard")
            .with_unlock(unlocked.clone());

        // Unanswered: refused, as always.
        assert!(guard.resolve_existing(".env").is_err());

        unlocked.store(true, Ordering::Release);
        assert!(
            guard.resolve_existing(".env").is_ok(),
            "the approved session still cannot read the file it approved"
        );

        // …and the boundary has not moved.
        assert!(
            guard
                .resolve_existing(outside.path().join("elsewhere.txt"))
                .is_err(),
            "the unlock escaped the workspace"
        );
        assert!(
            guard.resolve_existing("../elsewhere.txt").is_err(),
            "the unlock allowed traversal out of the workspace"
        );
        let linked = directory.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &linked).expect("link");
        assert!(
            guard.resolve_existing("escape/elsewhere.txt").is_err(),
            "the unlock allowed a symlink out of the workspace"
        );
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

    #[test]
    fn denies_git_state_and_credentials_but_allows_public_env_example() {
        let (d, g) = guard();
        std::fs::create_dir_all(d.path().join(".git")).expect("git");
        std::fs::write(d.path().join(".git/config"), "secret").expect("config");
        std::fs::write(d.path().join(".env"), "TOKEN=secret").expect("env");
        std::fs::write(d.path().join(".env.example"), "TOKEN=").expect("example");
        std::fs::write(d.path().join(".env.production"), "TOKEN=secret").expect("variant");
        std::fs::write(d.path().join("credentials.rs"), "pub struct Credentials;").expect("source");
        assert!(matches!(
            g.resolve_existing(".git/config"),
            Err(NexusError::PathDenied(_))
        ));
        assert!(matches!(
            g.resolve_existing(".env"),
            Err(NexusError::PathDenied(_))
        ));
        assert!(g.resolve_existing(".env.example").is_ok());
        assert!(matches!(
            g.resolve_existing(".env.production"),
            Err(NexusError::PathDenied(_))
        ));
        assert!(g.resolve_existing("credentials.rs").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn denied_lexical_paths_cannot_be_hidden_by_in_workspace_symlinks() {
        let (d, g) = guard();
        std::fs::create_dir_all(d.path().join("safe")).expect("safe");
        std::fs::write(d.path().join("safe/config"), "secret").expect("config");
        std::fs::write(d.path().join("safe/token"), "secret").expect("token");
        std::os::unix::fs::symlink("safe", d.path().join(".git")).expect("git link");
        std::os::unix::fs::symlink("safe/token", d.path().join(".env")).expect("env link");
        assert!(matches!(
            g.resolve_existing(".git/config"),
            Err(NexusError::PathDenied(_))
        ));
        assert!(matches!(
            g.resolve_existing(".env"),
            Err(NexusError::PathDenied(_))
        ));
        assert!(matches!(
            g.resolve_for_write(".git/new"),
            Err(NexusError::PathDenied(_))
        ));
    }

    #[test]
    fn public_env_example_does_not_override_a_sensitive_parent() {
        let (d, g) = guard();
        std::fs::create_dir_all(d.path().join(".git")).expect("git");
        std::fs::write(d.path().join(".git/.env.example"), "TOKEN=").expect("example");
        assert!(matches!(
            g.resolve_existing(".git/.env.example"),
            Err(NexusError::PathDenied(_))
        ));
    }

    #[test]
    fn sensitive_mask_list_is_relative_and_skips_public_examples() {
        let (d, g) = guard();
        std::fs::create_dir_all(d.path().join(".git")).expect("git");
        std::fs::write(d.path().join(".env"), "TOKEN=secret").expect("env");
        std::fs::write(d.path().join(".env.example"), "TOKEN=").expect("example");
        let masks = g.sensitive_paths_for_masking().expect("masks");
        assert!(masks.contains(&PathBuf::from(".git")));
        assert!(masks.contains(&PathBuf::from(".env")));
        assert!(!masks.contains(&PathBuf::from(".env.example")));
    }

    #[test]
    fn sensitive_mask_scan_fails_closed_when_the_limit_is_exceeded() {
        let directory = tempfile::tempdir().expect("directory");
        std::fs::write(directory.path().join("one"), "one").expect("one");
        std::fs::write(directory.path().join("two"), "two").expect("two");
        let guard = WorkspaceGuard::new(directory.path(), &[]).expect("guard");
        let error = guard
            .sensitive_paths_for_masking_with_limit(1)
            .expect_err("scan must fail closed");
        assert!(error.to_string().contains("refusing unmasked"));
    }
}
