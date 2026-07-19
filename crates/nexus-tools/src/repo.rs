//! Repository and coding tools (`repo.*`).
//!
//! Git runs as a controlled subprocess inside the sandbox — never through a
//! shell, always with an argument vector. Read operations (status, diff, log)
//! are `Read` risk. `repo.git_restore` (rollback of working-tree changes) is
//! `Destructive`. Generic terminal commit/push/remote operations are hard
//! denied; local commits use the audited typed workflow, and remote mutation
//! is outside this tool surface.

use crate::{
    finalize_output, object_schema, Tool, ToolCategory, ToolContext, ToolMeta, ToolOutput,
    ToolRegistry,
};
use nexus_core::{NexusError, Result, RiskLevel};
use nexus_policy::ActionRequest;
use nexus_sandbox::{ExecSpec, FilesystemAccess, NetworkMode};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
enum RepoOp {
    GitStatus,
    GitDiff,
    GitLog,
    GitBranches,
    GitRestore,
    Structure,
    Dependencies,
    Check,
}

struct RepoTool {
    meta: ToolMeta,
    op: RepoOp,
}

fn meta(
    name: &str,
    description: &str,
    risk: RiskLevel,
    input_schema: Value,
    side_effects: &str,
    timeout: u64,
) -> ToolMeta {
    ToolMeta {
        name: format!("repo.{name}"),
        namespace: "repo".into(),
        description: description.into(),
        category: ToolCategory::Repo,
        input_schema,
        output_schema: json!({"type": "string"}),
        risk,
        required_capabilities: vec!["repo".into()],
        timeout_secs: timeout,
        max_output_bytes: 48_000,
        deterministic: false,
        needs_network: false,
        needs_sandbox: true,
        side_effects: side_effects.into(),
    }
}

pub fn register(registry: &mut ToolRegistry) {
    let tools: Vec<(RepoOp, ToolMeta)> = vec![
        (
            RepoOp::GitStatus,
            meta(
                "git_status",
                "git status --porcelain=v1 plus current branch.",
                RiskLevel::Read,
                object_schema(&[], &[]),
                "none",
                30,
            ),
        ),
        (
            RepoOp::GitDiff,
            meta(
                "git_diff",
                "Unified diff of working tree (or staged changes with staged=true), optionally limited to a path.",
                RiskLevel::Read,
                object_schema(
                    &[],
                    &[
                        ("staged", "boolean", "Show staged (index) diff instead"),
                        ("path", "string", "Limit diff to this path"),
                    ],
                ),
                "none",
                30,
            ),
        ),
        (
            RepoOp::GitLog,
            meta(
                "git_log",
                "Recent commit history (oneline with author and date).",
                RiskLevel::Read,
                object_schema(&[], &[("count", "integer", "Commits to show (default 15)")]),
                "none",
                30,
            ),
        ),
        (
            RepoOp::GitBranches,
            meta(
                "git_branches",
                "Local branches with current marked.",
                RiskLevel::Read,
                object_schema(&[], &[]),
                "none",
                30,
            ),
        ),
        (
            RepoOp::GitRestore,
            meta(
                "git_restore",
                "Discard uncommitted changes to a path (git restore). Destructive: lost edits cannot be recovered.",
                RiskLevel::Destructive,
                object_schema(
                    &[("path", "string", "Path to restore from HEAD")],
                    &[("staged", "boolean", "Also unstage the path")],
                ),
                "irreversibly discards uncommitted edits to the path",
                30,
            ),
        ),
        (
            RepoOp::Structure,
            meta(
                "structure",
                "Project structure overview: top-level layout, detected languages, entry points, and config files.",
                RiskLevel::Read,
                object_schema(&[], &[]),
                "none",
                30,
            ),
        ),
        (
            RepoOp::Dependencies,
            meta(
                "dependencies",
                "List declared dependencies from Cargo.toml / package.json / pyproject.toml / go.mod.",
                RiskLevel::Read,
                object_schema(&[], &[]),
                "none",
                30,
            ),
        ),
        (
            RepoOp::Check,
            meta(
                "check",
                "Run a project check: kind = format | lint | typecheck | test | build | bench. Auto-detects the toolchain (cargo, npm, go, python). Pass `target` to scope (e.g. a test name).",
                RiskLevel::Write,
                object_schema(
                    &[("kind", "string", "format|lint|typecheck|test|build|bench")],
                    &[("target", "string", "Optional scope, e.g. a test filter or package")],
                ),
                "runs the project toolchain; may write build artifacts inside the workspace",
                600,
            ),
        ),
    ];
    for (op, m) in tools {
        registry.register(Arc::new(RepoTool { meta: m, op }));
    }
}

async fn run_git(ctx: &ToolContext, args: &[&str]) -> Result<String> {
    let root = ctx.workspace.root().to_path_buf();
    let owned = args
        .iter()
        .map(|argument| argument.to_string())
        .collect::<Vec<_>>();
    let outcome = tokio::task::spawn_blocking(move || {
        nexus_core::git::GitRunner::new(&root)
            .with_output_cap(512_000)
            .run_owned(&owned)
    })
    .await
    .map_err(|error| NexusError::Other(format!("Git task join: {error}")))??;
    if outcome.success {
        Ok(outcome.stdout)
    } else if outcome.stderr.contains("not a git repository") {
        Err(NexusError::ToolFailed {
            tool: "repo".into(),
            message: "workspace is not a git repository".into(),
        })
    } else {
        Err(NexusError::ToolFailed {
            tool: "repo".into(),
            message: format!(
                "git {} failed (exit {:?}): {}",
                args.first().unwrap_or(&""),
                outcome.code,
                outcome.stderr.trim()
            ),
        })
    }
}

fn model_safe_status(
    guard: &nexus_core::workspace::WorkspaceGuard,
    status: &str,
) -> (Vec<String>, Vec<String>) {
    let mut lines = Vec::new();
    let mut paths = std::collections::BTreeSet::new();
    for line in status.lines() {
        if line.len() < 4 {
            continue;
        }
        let raw = &line[3..];
        let candidates = raw
            .split_once(" -> ")
            .map(|(source, destination)| vec![source, destination])
            .unwrap_or_else(|| vec![raw]);
        if candidates.iter().any(|path| {
            path.starts_with('"') || path.ends_with('"') || guard.resolve_for_write(path).is_err()
        }) {
            continue;
        }
        lines.push(line.to_string());
        paths.extend(candidates.into_iter().map(str::to_string));
    }
    (lines, paths.into_iter().collect())
}

/// Detect the primary toolchain by manifest files.
fn detect_toolchain(root: &std::path::Path) -> Option<&'static str> {
    if root.join("Cargo.toml").exists() {
        Some("cargo")
    } else if root.join("package.json").exists() {
        Some("npm")
    } else if root.join("go.mod").exists() {
        Some("go")
    } else if root.join("pyproject.toml").exists() || root.join("setup.py").exists() {
        Some("python")
    } else {
        None
    }
}

fn check_command(
    toolchain: &str,
    kind: &str,
    target: Option<&str>,
) -> Result<(String, Vec<String>)> {
    let (program, args): (&str, Vec<String>) = match (toolchain, kind) {
        ("cargo", "format") => ("cargo", vec!["fmt".into(), "--check".into()]),
        ("cargo", "lint") => (
            "cargo",
            vec![
                "clippy".into(),
                "--all-targets".into(),
                "--".into(),
                "-D".into(),
                "warnings".into(),
            ],
        ),
        ("cargo", "typecheck") => ("cargo", vec!["check".into(), "--all-targets".into()]),
        ("cargo", "test") => {
            let mut a = vec!["test".into()];
            if let Some(t) = target {
                a.push(t.into());
            }
            ("cargo", a)
        }
        ("cargo", "build") => ("cargo", vec!["build".into()]),
        ("cargo", "bench") => ("cargo", vec!["bench".into()]),
        ("npm", "format") => ("npx", vec!["prettier".into(), "--check".into(), ".".into()]),
        ("npm", "lint") => ("npm", vec!["run".into(), "lint".into()]),
        ("npm", "typecheck") => ("npx", vec!["tsc".into(), "--noEmit".into()]),
        ("npm", "test") => ("npm", vec!["test".into()]),
        ("npm", "build") => ("npm", vec!["run".into(), "build".into()]),
        ("go", "format") => ("gofmt", vec!["-l".into(), ".".into()]),
        ("go", "lint") => ("go", vec!["vet".into(), "./...".into()]),
        ("go", "typecheck") => (
            "go",
            vec![
                "build".into(),
                "-o".into(),
                "/dev/null".into(),
                "./...".into(),
            ],
        ),
        ("go", "test") => {
            let mut a = vec!["test".into()];
            if let Some(t) = target {
                a.push("-run".into());
                a.push(t.into());
            }
            a.push("./...".into());
            ("go", a)
        }
        ("go", "build") => ("go", vec!["build".into(), "./...".into()]),
        ("python", "test") => {
            let mut a = vec!["-m".into(), "pytest".into(), "-x".into(), "-q".into()];
            if let Some(t) = target {
                a.push("-k".into());
                a.push(t.into());
            }
            ("python3", a)
        }
        ("python", "lint") => (
            "python3",
            vec!["-m".into(), "ruff".into(), "check".into(), ".".into()],
        ),
        ("python", "typecheck") => ("python3", vec!["-m".into(), "mypy".into(), ".".into()]),
        _ => {
            return Err(NexusError::ToolInput {
                tool: "repo.check".into(),
                message: format!("no `{kind}` command known for toolchain `{toolchain}`"),
            })
        }
    };
    Ok((program.to_string(), args))
}

#[async_trait::async_trait]
impl Tool for RepoTool {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn action_request(&self, args: &Value) -> Result<ActionRequest> {
        let path = args.get("path").and_then(Value::as_str);
        let summary = match self.op {
            RepoOp::GitRestore => format!("DISCARD uncommitted changes to {}", path.unwrap_or("?")),
            RepoOp::Check => format!(
                "run {} check{}",
                args.get("kind").and_then(Value::as_str).unwrap_or("?"),
                args.get("target")
                    .and_then(Value::as_str)
                    .map(|t| format!(" ({t})"))
                    .unwrap_or_default()
            ),
            _ => self.meta.name.clone(),
        };
        Ok(ActionRequest {
            tool: self.meta.name.clone(),
            risk: self.meta.risk,
            paths: path.map(|p| vec![p.to_string()]).unwrap_or_default(),
            formats: Vec::new(),
            command: None,
            command_analysis: None,
            destination: None,
            summary,
        })
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutput> {
        let (raw, metadata) = match self.op {
            RepoOp::GitStatus => {
                let branch = run_git(ctx, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
                let status = run_git(ctx, &["status", "--porcelain=v1"]).await?;
                let (status_lines, _) = model_safe_status(&ctx.workspace, &status);
                let changed = status_lines.len();
                (
                    format!(
                        "branch: {}\n{}",
                        branch.trim(),
                        if status_lines.is_empty() {
                            "no model-accessible changes".to_string()
                        } else {
                            status_lines.join("\n")
                        }
                    ),
                    json!({"changed_files": changed, "branch": branch.trim()}),
                )
            }
            RepoOp::GitDiff => {
                let path = args.get("path").and_then(Value::as_str);
                let paths = if let Some(path) = path {
                    ctx.workspace.resolve_for_write(path)?;
                    vec![path.to_string()]
                } else {
                    let status = run_git(ctx, &["status", "--porcelain=v1"]).await?;
                    let (_, mut paths) = model_safe_status(&ctx.workspace, &status);
                    paths.truncate(200);
                    paths
                };
                if paths.is_empty() {
                    return finalize_output(
                        ctx,
                        &self.meta,
                        "no model-accessible differences".into(),
                        json!({"diff_lines": 0}),
                    )
                    .await;
                }
                let mut a: Vec<&str> = vec!["diff"];
                if args.get("staged").and_then(Value::as_bool).unwrap_or(false) {
                    a.push("--cached");
                }
                a.push("--");
                a.extend(paths.iter().map(String::as_str));
                let diff = run_git(ctx, &a).await?;
                let lines = diff.lines().count();
                (
                    if diff.is_empty() {
                        "no differences".to_string()
                    } else {
                        diff
                    },
                    json!({"diff_lines": lines}),
                )
            }
            RepoOp::GitLog => {
                let count = args
                    .get("count")
                    .and_then(Value::as_u64)
                    .unwrap_or(15)
                    .clamp(1, 100)
                    .to_string();
                let log = run_git(
                    ctx,
                    &[
                        "log",
                        &format!("-{count}"),
                        "--pretty=format:%h %ad %an: %s",
                        "--date=short",
                    ],
                )
                .await?;
                (log, Value::Null)
            }
            RepoOp::GitBranches => {
                let out = run_git(ctx, &["branch", "--list", "-v"]).await?;
                (out, Value::Null)
            }
            RepoOp::GitRestore => {
                let path = args.get("path").and_then(Value::as_str).ok_or_else(|| {
                    NexusError::ToolInput {
                        tool: self.meta.name.clone(),
                        message: "missing path".into(),
                    }
                })?;
                // Validate the path is inside the workspace before git sees it.
                ctx.workspace.resolve_existing(path)?;
                if args.get("staged").and_then(Value::as_bool).unwrap_or(false) {
                    run_git(ctx, &["restore", "--staged", "--", path]).await?;
                }
                run_git(ctx, &["restore", "--", path]).await?;
                (format!("restored {path} from HEAD"), Value::Null)
            }
            RepoOp::Structure => {
                let root = ctx.workspace.root().to_path_buf();
                let guard = ctx.workspace.clone();
                tokio::task::spawn_blocking(move || {
                    let mut langs: BTreeMap<&str, usize> = BTreeMap::new();
                    let mut configs = Vec::new();
                    let mut tops = Vec::new();
                    for entry in ignore::WalkBuilder::new(&root)
                        .max_depth(Some(3))
                        .hidden(false)
                        .git_ignore(true)
                        .build()
                        .flatten()
                    {
                        let p = entry.path();
                        if p == root {
                            continue;
                        }
                        let Ok(real) = guard.resolve_existing(p) else {
                            continue;
                        };
                        let rel = guard.display_relative(&real);
                        if real.parent() == Some(root.as_path()) {
                            tops.push(rel.clone());
                        }
                        if let Some(ext) = real.extension().and_then(|e| e.to_str()) {
                            let lang = match ext {
                                "rs" => "Rust",
                                "ts" | "tsx" => "TypeScript",
                                "js" | "jsx" => "JavaScript",
                                "py" => "Python",
                                "go" => "Go",
                                "java" => "Java",
                                "c" | "h" => "C",
                                "cpp" | "cc" | "hpp" => "C++",
                                "rb" => "Ruby",
                                "sh" => "Shell",
                                _ => continue,
                            };
                            *langs.entry(lang).or_insert(0) += 1;
                        }
                        if matches!(
                            real.file_name().and_then(|n| n.to_str()),
                            Some(
                                "Cargo.toml"
                                    | "package.json"
                                    | "go.mod"
                                    | "pyproject.toml"
                                    | "Makefile"
                                    | "CMakeLists.txt"
                                    | "Dockerfile"
                            )
                        ) {
                            configs.push(rel);
                        }
                    }
                    tops.sort();
                    let lang_summary: Vec<String> = langs
                        .iter()
                        .map(|(l, c)| format!("{l} ({c} files)"))
                        .collect();
                    Ok::<(String, Value), NexusError>((
                        format!(
                            "top-level:\n{}\n\nlanguages: {}\nbuild/config files: {}",
                            tops.iter()
                                .map(|t| format!("  {t}"))
                                .collect::<Vec<_>>()
                                .join("\n"),
                            lang_summary.join(", "),
                            configs.join(", ")
                        ),
                        json!({"languages": langs, "configs": configs}),
                    ))
                })
                .await
                .map_err(|e| NexusError::other(format!("join: {e}")))??
            }
            RepoOp::Dependencies => {
                let root = ctx.workspace.root().to_path_buf();
                tokio::task::spawn_blocking(move || {
                    let mut out = String::new();
                    let cargo = root.join("Cargo.toml");
                    if cargo.exists() {
                        if let Ok(text) = std::fs::read_to_string(&cargo) {
                            if let Ok(v) = text.parse::<toml::Value>() {
                                out.push_str("Cargo.toml dependencies:\n");
                                for section in ["dependencies", "workspace.dependencies"] {
                                    if let Some(deps) = lookup(&v, section) {
                                        for (name, spec) in deps {
                                            out.push_str(&format!("  {name} = {spec}\n"));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    let pkg = root.join("package.json");
                    if pkg.exists() {
                        if let Ok(text) = std::fs::read_to_string(&pkg) {
                            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                                for key in ["dependencies", "devDependencies"] {
                                    if let Some(deps) = v.get(key).and_then(Value::as_object) {
                                        out.push_str(&format!("package.json {key}:\n"));
                                        for (name, ver) in deps {
                                            out.push_str(&format!("  {name} = {ver}\n"));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    let gomod = root.join("go.mod");
                    if gomod.exists() {
                        if let Ok(text) = std::fs::read_to_string(&gomod) {
                            out.push_str("go.mod:\n");
                            out.push_str(&text);
                        }
                    }
                    if out.is_empty() {
                        out = "no recognized dependency manifests found".into();
                    }
                    Ok::<(String, Value), NexusError>((out, Value::Null))
                })
                .await
                .map_err(|e| NexusError::other(format!("join: {e}")))??
            }
            RepoOp::Check => {
                let kind = args.get("kind").and_then(Value::as_str).ok_or_else(|| {
                    NexusError::ToolInput {
                        tool: self.meta.name.clone(),
                        message: "missing kind".into(),
                    }
                })?;
                let toolchain = detect_toolchain(ctx.workspace.root()).ok_or_else(|| {
                    NexusError::ToolFailed {
                        tool: self.meta.name.clone(),
                        message: "no recognized toolchain manifest in workspace root".into(),
                    }
                })?;
                let (program, cmd_args) =
                    check_command(toolchain, kind, args.get("target").and_then(Value::as_str))?;
                let sensitive_path_masks = ctx.workspace.sensitive_paths_for_masking()?;
                let spec = ExecSpec {
                    program,
                    args: cmd_args,
                    shell: false,
                    cwd: ctx.workspace.root().to_path_buf(),
                    env: BTreeMap::new(),
                    env_allowlist: ctx.config.sandbox.env_allowlist.clone(),
                    network: NetworkMode::Off,
                    approved_network: NetworkMode::Off,
                    filesystem_access: FilesystemAccess::WorkspaceWrite,
                    sensitive_path_masks,
                    unsafe_host_approved: ctx.sandbox.strong_isolation()
                        || ctx.authorization.consume_unsafe_host_once(),
                    timeout_secs: self.meta.timeout_secs,
                    cpu_limit_secs: self.meta.timeout_secs,
                    memory_limit_mb: ctx.config.sandbox.memory_limit_mb.max(2048),
                    output_hard_cap: ctx.config.sandbox.max_output_bytes.max(1),
                    stdin: None,
                };
                let outcome = ctx.sandbox.execute(spec, None).await?;
                let passed = outcome.exit_code == Some(0);
                let mut body = format!(
                    "{} check: {}\n",
                    kind,
                    if passed { "PASSED" } else { "FAILED" }
                );
                body.push_str(&outcome.stdout);
                if !outcome.stderr.is_empty() {
                    body.push_str("\n--- stderr ---\n");
                    body.push_str(&outcome.stderr);
                }
                (
                    body,
                    json!({
                        "passed": passed,
                        "exit_code": outcome.exit_code,
                        "timed_out": outcome.timed_out,
                        "toolchain": toolchain,
                    }),
                )
            }
        };
        finalize_output(ctx, &self.meta, raw, metadata).await
    }
}

fn lookup(v: &toml::Value, dotted: &str) -> Option<Vec<(String, String)>> {
    let mut cursor = v;
    for part in dotted.split('.') {
        cursor = cursor.get(part)?;
    }
    cursor.as_table().map(|t| {
        t.iter()
            .map(|(k, val)| {
                let spec = match val {
                    toml::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (k.clone(), spec)
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::context;

    async fn git(dir: &std::path::Path, args: &[&str]) {
        let status = tokio::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    #[tokio::test]
    async fn git_status_and_diff_work() {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "-q", "-b", "main"]).await;
        std::fs::write(dir.path().join("f.txt"), "one\n").expect("write");
        git(dir.path(), &["add", "."]).await;
        git(dir.path(), &["commit", "-qm", "init"]).await;
        std::fs::write(dir.path().join("f.txt"), "two\n").expect("write");

        let ctx = context(dir.path());
        let mut r = ToolRegistry::new();
        register(&mut r);
        let out = r
            .get("repo.git_status")
            .expect("tool")
            .execute(&ctx, json!({}))
            .await
            .expect("status");
        assert!(out.content.contains("branch: main"));
        assert!(out.content.contains("f.txt"));
        let out = r
            .get("repo.git_diff")
            .expect("tool")
            .execute(&ctx, json!({}))
            .await
            .expect("diff");
        assert!(out.content.contains("-one"));
        assert!(out.content.contains("+two"));
    }

    #[tokio::test]
    async fn git_status_and_diff_hide_denied_files() {
        let directory = tempfile::tempdir().expect("directory");
        git(directory.path(), &["init", "-q", "-b", "main"]).await;
        std::fs::write(directory.path().join("visible.txt"), "one\n").expect("visible");
        std::fs::write(directory.path().join(".env"), "TOKEN=old-secret\n").expect("env");
        git(directory.path(), &["add", "."]).await;
        git(directory.path(), &["commit", "-qm", "init"]).await;
        std::fs::write(directory.path().join("visible.txt"), "two\n").expect("visible update");
        std::fs::write(directory.path().join(".env"), "TOKEN=new-secret\n").expect("env update");

        let ctx = context(directory.path());
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        let status = registry
            .get("repo.git_status")
            .expect("status tool")
            .execute(&ctx, json!({}))
            .await
            .expect("status");
        assert!(status.content.contains("visible.txt"));
        assert!(!status.content.contains(".env"));

        let diff = registry
            .get("repo.git_diff")
            .expect("diff tool")
            .execute(&ctx, json!({}))
            .await
            .expect("diff");
        assert!(diff.content.contains("-one"));
        assert!(diff.content.contains("+two"));
        assert!(!diff.content.contains("old-secret"));
        assert!(!diff.content.contains("new-secret"));
        assert!(!diff.content.contains(".env"));

        assert!(registry
            .get("repo.git_diff")
            .expect("diff tool")
            .execute(&ctx, json!({"path": ".env"}))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn non_repo_reports_cleanly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = context(dir.path());
        let mut r = ToolRegistry::new();
        register(&mut r);
        let err = r
            .get("repo.git_status")
            .expect("tool")
            .execute(&ctx, json!({}))
            .await
            .expect_err("must fail");
        assert!(err.to_string().contains("not a git repository"));
    }

    #[test]
    fn check_commands_are_argument_vectors() {
        let (prog, args) = check_command("cargo", "test", Some("policy")).expect("cmd");
        assert_eq!(prog, "cargo");
        assert_eq!(args, vec!["test", "policy"]);
        assert!(check_command("cargo", "yolo", None).is_err());
    }
}
