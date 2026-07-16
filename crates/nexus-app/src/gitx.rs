//! Read-only git queries (plus the explicitly confirmed `/revert`), run
//! directly against the workspace. Output is sanitized before display.

use nexus_core::{NexusError, Result};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    pub name: String,
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileState {
    pub path: String,
    pub index_status: char,
    pub worktree_status: char,
}

fn run_git(workspace: &Path, args: &[&str]) -> Option<(bool, String)> {
    let out = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .ok()?;
    let text = if out.status.success() {
        String::from_utf8_lossy(&out.stdout).to_string()
    } else {
        String::from_utf8_lossy(&out.stderr).to_string()
    };
    Some((
        out.status.success(),
        nexus_core::sanitize::sanitize_terminal(text.trim_end()),
    ))
}

/// Whether the workspace is inside a git repository.
pub fn is_repo(workspace: &Path) -> bool {
    matches!(
        run_git(workspace, &["rev-parse", "--is-inside-work-tree"]),
        Some((true, ref s)) if s.trim() == "true"
    )
}

/// Top-level checkout containing `workspace`, resolved with Git rather than
/// `.git` path assumptions (works for worktrees and nested invocation dirs).
pub fn repo_root(workspace: &Path) -> Option<PathBuf> {
    match run_git(workspace, &["rev-parse", "--show-toplevel"]) {
        Some((true, root)) if !root.trim().is_empty() => {
            let path = PathBuf::from(root.trim());
            Some(path.canonicalize().unwrap_or(path))
        }
        _ => None,
    }
}

/// Current branch name (`HEAD` when detached), `None` outside a repo.
pub fn branch(workspace: &Path) -> Option<String> {
    match run_git(workspace, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Some((true, s)) if !s.is_empty() => Some(s),
        _ => None,
    }
}

/// Short HEAD commit, or `None` outside a repository/unborn branch.
pub fn head_commit(workspace: &Path) -> Option<String> {
    match run_git(workspace, &["rev-parse", "--short=12", "HEAD"]) {
        Some((true, value)) if !value.is_empty() => Some(value),
        _ => None,
    }
}

/// Modified/untracked paths from `git status --porcelain`.
pub fn modified_files(workspace: &Path) -> Vec<String> {
    file_states(workspace)
        .into_iter()
        .map(|state| state.path)
        .collect()
}

/// Parsed index/worktree state from `git status --porcelain=v1 -z`.
pub fn file_states(workspace: &Path) -> Vec<FileState> {
    let Ok(output) = Command::new("git")
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(workspace)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let fields: Vec<&[u8]> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect();
    let mut states = Vec::new();
    let mut index = 0usize;
    while index < fields.len() {
        let field = fields[index];
        if field.len() < 4 {
            index += 1;
            continue;
        }
        let status = &field[..2];
        let path = String::from_utf8_lossy(&field[3..]).to_string();
        if !path.is_empty() {
            states.push(FileState {
                path,
                index_status: status[0] as char,
                worktree_status: status[1] as char,
            });
        }
        let renamed_or_copied = status.contains(&b'R') || status.contains(&b'C');
        index += if renamed_or_copied { 2 } else { 1 };
    }
    states
}

pub fn diff_statistics(workspace: &Path) -> nexus_core::orchestration::DiffStatistics {
    let mut stats = nexus_core::orchestration::DiffStatistics::default();
    if !is_repo(workspace) {
        return stats;
    }
    let mut paths = std::collections::BTreeSet::new();
    for args in [
        &["diff", "--numstat"][..],
        &["diff", "--cached", "--numstat"][..],
    ] {
        let Some((true, output)) = run_git(workspace, args) else {
            continue;
        };
        for line in output.lines() {
            let mut fields = line.splitn(3, '\t');
            let added = fields.next().unwrap_or("0");
            let deleted = fields.next().unwrap_or("0");
            let path = fields.next().unwrap_or("");
            stats.insertions += added.parse::<usize>().unwrap_or(0);
            stats.deletions += deleted.parse::<usize>().unwrap_or(0);
            if !path.is_empty() {
                paths.insert(path.to_string());
            }
        }
    }
    stats.files = paths.len();
    stats
}

/// Working-tree diff (optionally limited to one path), size-capped.
pub fn diff(workspace: &Path, path: Option<&str>, max_bytes: usize) -> Result<String> {
    if !is_repo(workspace) {
        return Err(NexusError::Other(
            "this workspace is not a git repository — /diff needs git history to compare against"
                .into(),
        ));
    }
    let mut args = vec!["diff"];
    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    let (ok, mut text) = run_git(workspace, &args)
        .ok_or_else(|| NexusError::Other("git is not installed or not runnable".into()))?;
    if !ok {
        return Err(NexusError::Other(format!("git diff failed: {text}")));
    }
    if text.is_empty() {
        text = "no unstaged changes".into();
    }
    if text.len() > max_bytes {
        let mut cut = max_bytes;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n… (truncated)");
    }
    Ok(text)
}

pub fn staged_diff(workspace: &Path, max_bytes: usize) -> Result<String> {
    diff_with_args(workspace, &["diff", "--cached"], max_bytes)
}

pub fn staged_diff_path(workspace: &Path, path: &str, max_bytes: usize) -> Result<String> {
    validate_paths(&[path.to_string()])?;
    diff_with_args(workspace, &["diff", "--cached", "--", path], max_bytes)
}

pub fn status_text(workspace: &Path) -> Result<String> {
    if !is_repo(workspace) {
        return Err(NexusError::Other("not a git repository".into()));
    }
    let (ok, text) = run_git(workspace, &["status", "--short", "--branch"])
        .ok_or_else(|| NexusError::Other("git is not installed or not runnable".into()))?;
    if ok {
        Ok(if text.is_empty() {
            "working tree clean".into()
        } else {
            text
        })
    } else {
        Err(NexusError::Other(format!("git status failed: {text}")))
    }
}

pub fn stage(workspace: &Path, paths: &[String]) -> Result<String> {
    run_git_paths(workspace, &["add"], paths, "git add")?;
    Ok(format!("staged {} path(s)", paths.len()))
}

pub fn unstage(workspace: &Path, paths: &[String]) -> Result<String> {
    run_git_paths(
        workspace,
        &["restore", "--staged"],
        paths,
        "git restore --staged",
    )?;
    Ok(format!("unstaged {} path(s)", paths.len()))
}

pub fn branches(workspace: &Path) -> Result<Vec<BranchInfo>> {
    if !is_repo(workspace) {
        return Err(NexusError::Other("not a git repository".into()));
    }
    let (ok, text) = run_git(
        workspace,
        &["branch", "--format=%(HEAD)%09%(refname:short)"],
    )
    .ok_or_else(|| NexusError::Other("git is not installed or not runnable".into()))?;
    if !ok {
        return Err(NexusError::Other(format!("git branch failed: {text}")));
    }
    Ok(text
        .lines()
        .filter_map(|line| {
            let (head, name) = line.split_once('\t')?;
            Some(BranchInfo {
                name: name.to_string(),
                current: head.trim() == "*",
            })
        })
        .collect())
}

pub fn create_branch(workspace: &Path, name: &str) -> Result<String> {
    validate_branch_name(workspace, name)?;
    let (ok, text) = run_git(workspace, &["branch", name])
        .ok_or_else(|| NexusError::Other("git is not installed or not runnable".into()))?;
    if !ok {
        return Err(NexusError::Other(format!("git branch failed: {text}")));
    }
    Ok(format!("created branch `{name}`"))
}

pub fn switch_branch(workspace: &Path, name: &str) -> Result<String> {
    validate_branch_name(workspace, name)?;
    if !modified_files(workspace).is_empty() {
        return Err(NexusError::Other(
            "working tree is dirty; commit, stash externally, or restore changes before switching"
                .into(),
        ));
    }
    let (ok, text) = run_git(workspace, &["switch", name])
        .ok_or_else(|| NexusError::Other("git is not installed or not runnable".into()))?;
    if !ok {
        return Err(NexusError::Other(format!("git switch failed: {text}")));
    }
    Ok(format!("switched to branch `{name}`"))
}

pub fn delete_branch(workspace: &Path, name: &str) -> Result<String> {
    validate_branch_name(workspace, name)?;
    if branch(workspace).as_deref() == Some(name) {
        return Err(NexusError::Other(
            "cannot delete the currently checked-out branch".into(),
        ));
    }
    let (ok, text) = run_git(workspace, &["branch", "-d", name])
        .ok_or_else(|| NexusError::Other("git is not installed or not runnable".into()))?;
    if !ok {
        return Err(NexusError::Other(format!(
            "git branch -d failed (unmerged branches are preserved): {text}"
        )));
    }
    Ok(format!("deleted merged branch `{name}`"))
}

pub fn log(workspace: &Path, limit: usize) -> Result<String> {
    let count = limit.clamp(1, 200).to_string();
    let (ok, text) = run_git(
        workspace,
        &[
            "log",
            "--oneline",
            "--decorate",
            "--date=short",
            "-n",
            &count,
        ],
    )
    .ok_or_else(|| NexusError::Other("git is not installed or not runnable".into()))?;
    if ok {
        Ok(text)
    } else {
        Err(NexusError::Other(format!("git log failed: {text}")))
    }
}

pub fn commit_preview(workspace: &Path, paths: &[String], max_bytes: usize) -> Result<String> {
    validate_paths(paths)?;
    if !is_repo(workspace) {
        return Err(NexusError::Other("not a git repository".into()));
    }
    let mut sections = Vec::new();
    let tracked: Vec<&str> = paths
        .iter()
        .map(String::as_str)
        .filter(|path| is_tracked(workspace, path))
        .collect();
    if !tracked.is_empty() {
        let mut args = vec!["diff", "HEAD", "--"];
        args.extend(tracked);
        let text = diff_with_args(workspace, &args, max_bytes)?;
        if text != "no changes" {
            sections.push(text);
        }
    }
    for path in paths.iter().filter(|path| !is_tracked(workspace, path)) {
        let full = workspace.join(path);
        if !full.is_file() {
            return Err(NexusError::Config(format!(
                "selected untracked path `{path}` is not a regular file"
            )));
        }
        let output = Command::new("git")
            .args(["diff", "--no-index", "--", "/dev/null", path])
            .current_dir(workspace)
            .output()
            .map_err(|error| NexusError::Other(format!("git diff failed: {error}")))?;
        // `git diff --no-index` returns 1 when differences are present.
        if !matches!(output.status.code(), Some(0 | 1)) {
            return Err(NexusError::Other(format!(
                "git diff failed: {}",
                nexus_core::sanitize::sanitize_terminal(&String::from_utf8_lossy(&output.stderr))
            )));
        }
        let text =
            nexus_core::sanitize::sanitize_terminal(&String::from_utf8_lossy(&output.stdout));
        if !text.trim().is_empty() {
            sections.push(text);
        }
    }
    if sections.is_empty() {
        return Err(NexusError::Config(
            "the selected files have no changes to commit".into(),
        ));
    }
    let mut preview = sections.join("\n");
    truncate(&mut preview, max_bytes);
    Ok(preview)
}

pub fn commit(workspace: &Path, paths: &[String], message: &str) -> Result<String> {
    if message.trim().is_empty() {
        return Err(NexusError::Config("commit message cannot be empty".into()));
    }
    stage(workspace, paths)?;
    let mut args = vec!["commit", "--only", "-m", message.trim(), "--"];
    args.extend(paths.iter().map(String::as_str));
    let (ok, text) = run_git(workspace, &args)
        .ok_or_else(|| NexusError::Other("git is not installed or not runnable".into()))?;
    if !ok {
        return Err(NexusError::Other(format!("git commit failed: {text}")));
    }
    Ok(text)
}

fn is_tracked(workspace: &Path, path: &str) -> bool {
    matches!(
        run_git(workspace, &["ls-files", "--error-unmatch", "--", path]),
        Some((true, _))
    )
}

fn diff_with_args(workspace: &Path, args: &[&str], max_bytes: usize) -> Result<String> {
    if !is_repo(workspace) {
        return Err(NexusError::Other("not a git repository".into()));
    }
    let (ok, mut text) = run_git(workspace, args)
        .ok_or_else(|| NexusError::Other("git is not installed or not runnable".into()))?;
    if !ok {
        return Err(NexusError::Other(format!("git diff failed: {text}")));
    }
    if text.is_empty() {
        text = "no changes".into();
    }
    truncate(&mut text, max_bytes);
    Ok(text)
}

fn truncate(text: &mut String, max_bytes: usize) {
    if text.len() <= max_bytes {
        return;
    }
    let mut cut = max_bytes;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    text.push_str("\n… (truncated)");
}

fn run_git_paths(workspace: &Path, prefix: &[&str], paths: &[String], label: &str) -> Result<()> {
    validate_paths(paths)?;
    let mut args = prefix.to_vec();
    args.push("--");
    args.extend(paths.iter().map(String::as_str));
    let (ok, text) = run_git(workspace, &args)
        .ok_or_else(|| NexusError::Other("git is not installed or not runnable".into()))?;
    if ok {
        Ok(())
    } else {
        Err(NexusError::Other(format!("{label} failed: {text}")))
    }
}

fn validate_paths(paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Err(NexusError::Config(
            "select at least one path; broad implicit commits are not allowed".into(),
        ));
    }
    if paths.iter().any(|path| !safe_relative_path(path)) {
        return Err(NexusError::Config(
            "refusing an empty, option-like, or traversal path".into(),
        ));
    }
    Ok(())
}

fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('-')
        && !path.chars().any(char::is_control)
        && Path::new(path).components().all(|component| {
            !matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn validate_branch_name(workspace: &Path, name: &str) -> Result<()> {
    if name.trim().is_empty() || name.starts_with('-') {
        return Err(NexusError::Config("invalid branch name".into()));
    }
    let (ok, text) = run_git(workspace, &["check-ref-format", "--branch", name])
        .ok_or_else(|| NexusError::Other("git is not installed or not runnable".into()))?;
    if ok {
        Ok(())
    } else {
        Err(NexusError::Config(format!(
            "invalid branch name `{name}`: {text}"
        )))
    }
}

/// Discard staged and working-tree changes to one file. The caller MUST have
/// shown a confirmation first (`/revert` is marked `requires_confirmation`).
pub fn revert_file(workspace: &Path, path: &str) -> Result<String> {
    if !is_repo(workspace) {
        return Err(NexusError::Other(
            "not a git repository — nothing to revert against".into(),
        ));
    }
    // Refuse paths outside the repo or with option-like shapes.
    if !safe_relative_path(path) {
        return Err(NexusError::Config(format!(
            "refusing suspicious path `{path}`"
        )));
    }
    let (ok, text) = run_git(
        workspace,
        &[
            "restore",
            "--source=HEAD",
            "--staged",
            "--worktree",
            "--",
            path,
        ],
    )
    .ok_or_else(|| NexusError::Other("git is not installed or not runnable".into()))?;
    if !ok {
        return Err(NexusError::Other(format!("git restore failed: {text}")));
    }
    Ok(format!(
        "restored staged and working-tree changes in `{path}` from HEAD"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("dir");
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .expect("git runs")
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["config", "user.name", "NEXUS Test"]);
        run(&["config", "user.email", "nexus@example.invalid"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").expect("write");
        run(&["add", "."]);
        run(&["commit", "-qm", "init"]);
        dir
    }

    #[test]
    fn detects_repo_branch_and_changes() {
        let dir = init_repo();
        assert!(is_repo(dir.path()));
        assert!(branch(dir.path()).is_some());
        assert!(modified_files(dir.path()).is_empty());
        std::fs::write(dir.path().join("a.txt"), "two\n").expect("write");
        let modified = modified_files(dir.path());
        assert_eq!(modified, vec!["a.txt".to_string()]);
        let d = diff(dir.path(), None, 64 * 1024).expect("diff");
        assert!(d.contains("-one") && d.contains("+two"), "got: {d}");
        std::fs::write(dir.path().join("space name.txt"), "new\n").expect("write");
        assert!(modified_files(dir.path()).contains(&"space name.txt".to_string()));
    }

    #[test]
    fn revert_restores_file() {
        let dir = init_repo();
        std::fs::write(dir.path().join("a.txt"), "two\n").expect("write");
        revert_file(dir.path(), "a.txt").expect("revert");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).expect("read"),
            "one\n"
        );
    }

    #[test]
    fn revert_clears_staged_changes_too() {
        let dir = init_repo();
        std::fs::write(dir.path().join("a.txt"), "two\n").expect("write");
        let status = Command::new("git")
            .args(["add", "a.txt"])
            .current_dir(dir.path())
            .status()
            .expect("git add");
        assert!(status.success());
        revert_file(dir.path(), "a.txt").expect("revert");
        assert!(modified_files(dir.path()).is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).expect("read"),
            "one\n"
        );
    }

    #[test]
    fn revert_rejects_suspicious_paths() {
        let dir = init_repo();
        assert!(revert_file(dir.path(), "--force").is_err());
        assert!(revert_file(dir.path(), "../outside").is_err());
        assert!(safe_relative_path("valid..name.txt"));
    }

    #[test]
    fn non_repo_is_honest() {
        let dir = tempfile::tempdir().expect("dir");
        assert!(!is_repo(dir.path()));
        assert!(diff(dir.path(), None, 1024).is_err());
    }

    #[test]
    fn commit_preview_includes_untracked_files() {
        let dir = init_repo();
        std::fs::write(dir.path().join("new.txt"), "new content\n").expect("write");
        let preview = commit_preview(dir.path(), &["new.txt".into()], 64 * 1024).expect("preview");
        assert!(preview.contains("new.txt"));
        assert!(preview.contains("+new content"));
    }

    #[test]
    fn commit_only_includes_selected_files() {
        let dir = init_repo();
        std::fs::write(dir.path().join("a.txt"), "selected\n").expect("write");
        std::fs::write(dir.path().join("other.txt"), "other\n").expect("write");
        stage(dir.path(), &["other.txt".into()]).expect("pre-stage unrelated");
        commit(dir.path(), &["a.txt".into()], "selected only").expect("commit");

        let names = run_git(
            dir.path(),
            &["show", "--pretty=format:", "--name-only", "HEAD"],
        )
        .expect("git show");
        assert!(names.0);
        assert_eq!(names.1.trim(), "a.txt");
        let staged = staged_diff(dir.path(), 64 * 1024).expect("staged diff");
        assert!(staged.contains("other.txt"));
    }
}
