//! On-demand durable background task worker.
//!
//! One detached worker is started per workspace. SQLite task leases provide
//! stale-run recovery and enforce three readers plus one writer. Writer tasks
//! run in persistent `snx/task/<id>` worktrees outside the source checkout;
//! the worker never commits, merges, stashes, removes a worktree, or deletes a
//! branch.

use crate::app::App;
use nexus_agent::{AgentLoop, AgentRole, ApprovalDecision, ApprovalHandler};
use nexus_core::orchestration::{BackgroundTask, TaskStatus, ValueEnvelope};
use nexus_core::timeline::{LifecyclePhase, TimelineEvent, TimelineKind, TimelineStatus};
use nexus_core::{NexusError, Result, RiskLevel, SpanId, TraceId, TurnId};
use nexus_policy::ActionRequest;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;

const MAX_READERS: usize = 3;
const IDLE_SHUTDOWN: Duration = Duration::from_secs(5 * 60);
const LEASE_SECS: i64 = 90;
const HEARTBEAT_SECS: u64 = 20;

pub fn ensure_started(app: &App) -> Result<bool> {
    if cfg!(test) || std::env::var_os("SNX_DISABLE_WORKER").is_some() {
        return Ok(false);
    }
    if worker_lock_live(&app.paths.state_dir.join("worker.lock")) {
        return Ok(false);
    }
    let binary = std::env::current_exe()
        .map_err(|error| NexusError::Other(format!("locating snx binary: {error}")))?;
    std::process::Command::new(binary)
        .arg("worker")
        .arg("--idle-secs")
        .arg(IDLE_SHUTDOWN.as_secs().to_string())
        .current_dir(&app.workspace)
        .env("SNX_WORKER_CHILD", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| NexusError::Other(format!("starting background worker: {error}")))?;
    Ok(true)
}

pub async fn run(app: Arc<App>, idle_secs: u64) -> Result<()> {
    let lock_path = app.paths.state_dir.join("worker.lock");
    let Some(_lock) = WorkerLock::acquire(&lock_path)? else {
        return Ok(());
    };
    let orchestration = app.orchestration();
    orchestration.recover_stale_tasks(&nexus_core::now_rfc3339())?;
    let worker_id = format!(
        "worker-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    );
    let idle_limit = Duration::from_secs(idle_secs.max(1));
    let mut idle_since = Instant::now();
    let mut jobs = JoinSet::new();
    let mut active = BTreeMap::<String, bool>::new();

    loop {
        while jobs.len() < MAX_READERS + 1 {
            let writer_running = active.values().any(|writer| *writer);
            let lease_expires = lease_expiry();
            let Some(task) = orchestration.lease_next(
                &worker_id,
                &lease_expires,
                MAX_READERS,
                !writer_running,
            )?
            else {
                break;
            };
            idle_since = Instant::now();
            active.insert(task.id.to_string(), task.writer);
            let app = app.clone();
            let worker_id = worker_id.clone();
            jobs.spawn(async move {
                let id = task.id.to_string();
                let writer = task.writer;
                let result = execute_task(app, task, &worker_id).await;
                (id, writer, result)
            });
        }

        if jobs.is_empty() {
            if idle_since.elapsed() >= idle_limit {
                break;
            }
            tokio::time::sleep(Duration::from_millis(750)).await;
            continue;
        }

        tokio::select! {
            joined = jobs.join_next() => {
                if let Some(Ok((id, _, result))) = joined {
                    active.remove(&id);
                    if let Err(error) = result {
                        tracing::error!(task_id = %id, %error, "background task failed");
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
    }
    Ok(())
}

async fn execute_task(app: Arc<App>, task: BackgroundTask, worker_id: &str) -> Result<()> {
    let orchestration = app.orchestration();
    let run = orchestration.agent_run_for_task(task.id.as_str())?;
    if let Some(run) = &run {
        orchestration.set_agent_run_status(run.id.as_str(), TaskStatus::Running, None, None)?;
        append_agent_event(&app, run, TimelineStatus::Running, "running")?;
    }
    append_task_event(&app, &task, TimelineStatus::Running, "running")?;

    let workspace = if task.writer {
        match ensure_writer_worktree(&app, &task) {
            Ok(path) => path,
            Err(error) => {
                let message = app.redactor.redact(&error.to_string());
                orchestration.set_task_status(
                    task.id.as_str(),
                    TaskStatus::Blocked,
                    None,
                    Some(&message),
                )?;
                if let Some(run) = &run {
                    orchestration.set_agent_run_status(
                        run.id.as_str(),
                        TaskStatus::Blocked,
                        None,
                        Some(&message),
                    )?;
                    append_agent_event(&app, run, TimelineStatus::Blocked, &message)?;
                }
                append_task_event(&app, &task, TimelineStatus::Blocked, &message)?;
                return Ok(());
            }
        }
    } else {
        app.workspace.clone()
    };

    let (role, custom_agent) = if task.owner == "worker" {
        (
            if task.writer {
                AgentRole::Implementer
            } else {
                AgentRole::Reviewer
            },
            None,
        )
    } else {
        app.resolve_agent(&task.owner)?
    };
    let can_write = custom_agent
        .as_ref()
        .map(|definition| definition.can_write())
        .transpose()?
        .unwrap_or_else(|| role.can_write());
    if task.writer && !can_write {
        let message = format!("agent role `{}` cannot own a writer task", role.as_str());
        orchestration.set_task_status(
            task.id.as_str(),
            TaskStatus::Blocked,
            None,
            Some(&message),
        )?;
        return Ok(());
    }

    let runtime = app.runtime_in_workspace(Some(task.session_id.clone()), &workspace)?;
    let mut loop_ = AgentLoop::new(runtime, role);
    if let Some(definition) = custom_agent {
        loop_ = loop_.with_custom_agent(definition);
    }
    let approver: Arc<dyn ApprovalHandler> = Arc::new(BackgroundApprover {
        writer: task.writer,
    });
    let mut heartbeat = tokio::time::interval(Duration::from_secs(HEARTBEAT_SECS));
    let run_future = loop_.run(&task.session_id, &task.objective, approver);
    tokio::pin!(run_future);
    let outcome = loop {
        tokio::select! {
            result = &mut run_future => break result,
            _ = heartbeat.tick() => {
                if orchestration
                    .heartbeat_task(task.id.as_str(), worker_id, &lease_expiry())
                    .is_err()
                {
                    return Ok(());
                }
            }
        }
    };

    // Pause/cancel/re-lease can happen while the agent future is resolving.
    // Never overwrite the operator's newer state with a late completion.
    if orchestration.task(task.id.as_str())?.status != TaskStatus::Running {
        return Ok(());
    }

    match outcome {
        Ok(outcome) if outcome.stopped_reason == "finished" => {
            let result = ValueEnvelope {
                summary: app.redactor.redact(&outcome.final_message),
                artifact_ids: Vec::new(),
                evidence: vec![format!(
                    "{} step(s), {} tool call(s), {} input / {} output tokens",
                    outcome.steps, outcome.tool_calls, outcome.input_tokens, outcome.output_tokens
                )],
            };
            orchestration.set_task_status(
                task.id.as_str(),
                TaskStatus::Completed,
                Some(&result),
                None,
            )?;
            if let Some(run) = &run {
                orchestration.set_agent_run_status(
                    run.id.as_str(),
                    TaskStatus::Completed,
                    Some(&result),
                    None,
                )?;
                append_agent_event(&app, run, TimelineStatus::Completed, &result.summary)?;
            }
            append_task_event(&app, &task, TimelineStatus::Completed, &result.summary)?;
        }
        Ok(outcome) => {
            let message = app.redactor.redact(&outcome.final_message);
            let status = if outcome.stopped_reason == "policy_stop" {
                TaskStatus::Blocked
            } else {
                TaskStatus::Failed
            };
            orchestration.set_task_status(task.id.as_str(), status, None, Some(&message))?;
            if let Some(run) = &run {
                orchestration.set_agent_run_status(
                    run.id.as_str(),
                    status,
                    None,
                    Some(&message),
                )?;
                append_agent_event(
                    &app,
                    run,
                    if status == TaskStatus::Blocked {
                        TimelineStatus::Blocked
                    } else {
                        TimelineStatus::Failed
                    },
                    &message,
                )?;
            }
            append_task_event(
                &app,
                &task,
                if status == TaskStatus::Blocked {
                    TimelineStatus::Blocked
                } else {
                    TimelineStatus::Failed
                },
                &message,
            )?;
        }
        Err(error) => {
            let message = app.redactor.redact(&error.to_string());
            orchestration.set_task_status(
                task.id.as_str(),
                TaskStatus::Failed,
                None,
                Some(&message),
            )?;
            if let Some(run) = &run {
                orchestration.set_agent_run_status(
                    run.id.as_str(),
                    TaskStatus::Failed,
                    None,
                    Some(&message),
                )?;
                append_agent_event(&app, run, TimelineStatus::Failed, &message)?;
            }
            append_task_event(&app, &task, TimelineStatus::Failed, &message)?;
        }
    }
    Ok(())
}

fn ensure_writer_worktree(app: &App, task: &BackgroundTask) -> Result<PathBuf> {
    let (repo_root, worktree, work_area) = writer_worktree_paths(&app.workspace, task.id.as_str())?;
    let branch = task
        .branch
        .clone()
        .unwrap_or_else(|| format!("snx/task/{}", task.id.as_str()));
    let root = worktree
        .parent()
        .ok_or_else(|| NexusError::Other("writer worktree has no parent".into()))?;
    std::fs::create_dir_all(root)?;

    if worktree.exists() {
        let current = git_output(&worktree, &["branch", "--show-current"])?;
        if current.trim() != branch {
            return Err(NexusError::Other(format!(
                "existing worktree {} is on `{}`, expected `{branch}`; left untouched",
                worktree.display(),
                current.trim()
            )));
        }
    } else {
        let reference = format!("refs/heads/{branch}");
        let branch_exists = nexus_core::git::GitRunner::new(&repo_root)
            .run(&["show-ref", "--verify", "--quiet", &reference])
            .map(|output| output.success)
            .unwrap_or(false);
        let mut arguments = vec!["worktree".to_string(), "add".to_string()];
        if !branch_exists {
            arguments.push("-b".into());
            arguments.push(branch.clone());
        }
        arguments.push(worktree.display().to_string());
        if branch_exists {
            arguments.push(branch.clone());
        } else {
            arguments.push("HEAD".into());
        }
        let output = nexus_core::git::GitRunner::new(&repo_root).run_owned(&arguments)?;
        if !output.success {
            return Err(NexusError::Other(format!(
                "git worktree add failed: {}",
                output.stderr
            )));
        }
    }
    if !work_area.is_dir() {
        return Err(NexusError::Other(format!(
            "workspace subdirectory {} does not exist in the writer worktree at HEAD",
            work_area.display()
        )));
    }
    app.orchestration().set_task_workspace(
        task.id.as_str(),
        Some(&branch),
        Some(&worktree.display().to_string()),
    )?;
    Ok(work_area)
}

fn writer_worktree_paths(workspace: &Path, task_id: &str) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let repo_root = crate::gitx::repo_root(workspace).ok_or_else(|| {
        NexusError::Other(
            "writer tasks require a Git repository so isolation can use a dedicated worktree"
                .into(),
        )
    })?;
    let parent = repo_root.parent().ok_or_else(|| {
        NexusError::Other("repository has no parent for an external worktree".into())
    })?;
    let repo_name = repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace");
    let worktree = parent.join(".snx-worktrees").join(repo_name).join(task_id);
    if worktree.starts_with(&repo_root) {
        return Err(NexusError::PathDenied(
            "writer worktree must be outside the source checkout".into(),
        ));
    }
    let relative_workspace = workspace.strip_prefix(&repo_root).map_err(|_| {
        NexusError::PathDenied("workspace is outside its reported Git repository".into())
    })?;
    let work_area = worktree.join(relative_workspace);
    Ok((repo_root, worktree, work_area))
}

fn git_output(workspace: &Path, args: &[&str]) -> Result<String> {
    let output = nexus_core::git::GitRunner::new(workspace).run(args)?;
    if !output.success {
        return Err(NexusError::Other(format!(
            "git {} failed: {}",
            args.join(" "),
            output.stderr
        )));
    }
    Ok(output.stdout.trim().to_string())
}

fn append_task_event(
    app: &App,
    task: &BackgroundTask,
    status: TimelineStatus,
    detail: &str,
) -> Result<()> {
    let summary = format!(
        "task {} · {} · {}",
        task.id.as_str(),
        status.as_str(),
        nexus_core::sanitize::sanitize_terminal(detail)
    );
    let phase = match status {
        TimelineStatus::Running | TimelineStatus::Waiting => LifecyclePhase::Started,
        TimelineStatus::Failed | TimelineStatus::Blocked => LifecyclePhase::Failed,
        TimelineStatus::Cancelled => LifecyclePhase::Cancelled,
        _ => LifecyclePhase::Completed,
    };
    let mut event = TimelineEvent::new(
        task.session_id.clone(),
        TurnId::from(format!("task:{}", task.id.as_str())),
        TraceId::generate(),
        SpanId::generate(),
        None,
        phase,
        status,
        app.redactor.redact(&summary),
        TimelineKind::BackgroundTask {
            task_id: task.id.to_string(),
            title: task.title.clone(),
            status: status.as_str().into(),
            owner: task.owner.clone(),
        },
    );
    event.source = nexus_core::timeline::TimelineSource::Background;
    app.timeline().append(event)?;
    Ok(())
}

fn append_agent_event(
    app: &App,
    run: &nexus_core::orchestration::AgentRun,
    status: TimelineStatus,
    detail: &str,
) -> Result<()> {
    let summary = format!(
        "agent {} · {} · {}",
        run.id.as_str(),
        status.as_str(),
        nexus_core::sanitize::sanitize_terminal(detail)
    );
    let phase = match status {
        TimelineStatus::Running | TimelineStatus::Waiting => LifecyclePhase::Started,
        TimelineStatus::Failed | TimelineStatus::Blocked => LifecyclePhase::Failed,
        TimelineStatus::Cancelled => LifecyclePhase::Cancelled,
        _ => LifecyclePhase::Completed,
    };
    let mut event = TimelineEvent::new(
        run.session_id.clone(),
        TurnId::from(format!("agent:{}", run.id.as_str())),
        TraceId::generate(),
        SpanId::generate(),
        None,
        phase,
        status,
        app.redactor.redact(&summary),
        TimelineKind::AgentRun {
            agent_id: run.id.to_string(),
            parent_agent_id: run.parent_run_id.as_ref().map(ToString::to_string),
            role: run.role.clone(),
            status: status.as_str().into(),
            objective: run.objective.clone(),
        },
    );
    event.source = nexus_core::timeline::TimelineSource::Background;
    app.timeline().append(event)?;
    app.orchestration()
        .increment_agent_unread(run.id.as_str())?;
    Ok(())
}

fn lease_expiry() -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(LEASE_SECS))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

struct BackgroundApprover {
    writer: bool,
}

#[async_trait::async_trait]
impl ApprovalHandler for BackgroundApprover {
    async fn request_approval(
        &self,
        action: &ActionRequest,
        _arguments: &Value,
        _reason: &str,
        sandbox_active: bool,
    ) -> ApprovalDecision {
        if action
            .command_analysis
            .as_ref()
            .is_some_and(|analysis| analysis.one_time_only)
            || (action.tool.starts_with("terminal.") && !sandbox_active)
        {
            return ApprovalDecision::Deny;
        }
        let permitted = if self.writer {
            action.risk <= RiskLevel::Write
        } else {
            action.risk <= RiskLevel::Network
        };
        if permitted {
            ApprovalDecision::Approve
        } else {
            ApprovalDecision::Deny
        }
    }
}

struct WorkerLock {
    path: PathBuf,
}

impl WorkerLock {
    fn acquire(path: &Path) -> Result<Option<Self>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        for _ in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())?;
                    file.sync_all()?;
                    return Ok(Some(Self {
                        path: path.to_path_buf(),
                    }));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if worker_lock_live(path) {
                        return Ok(None);
                    }
                    let _ = std::fs::remove_file(path);
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(None)
    }
}

impl Drop for WorkerLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn worker_lock_live(path: &Path) -> bool {
    let Ok(pid) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(pid) = pid.trim().parse::<u32>() else {
        return false;
    };
    #[cfg(target_os = "linux")]
    {
        Path::new("/proc").join(pid.to_string()).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        path.metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age < Duration::from_secs(10 * 60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn background_approver_never_allows_destructive_work() {
        let approver = BackgroundApprover { writer: true };
        let action = ActionRequest {
            tool: "fs.delete".into(),
            risk: RiskLevel::Destructive,
            paths: vec!["a".into()],
            command: None,
            command_analysis: None,
            destination: None,
            summary: "delete".into(),
        };
        assert!(matches!(
            approver
                .request_approval(&action, &Value::Null, "", true)
                .await,
            ApprovalDecision::Deny
        ));
    }

    #[test]
    fn writer_worktree_for_nested_workspace_stays_outside_checkout() {
        let parent = tempfile::tempdir().expect("parent");
        let repo = parent.path().join("repo");
        let nested = repo.join("crates/app");
        std::fs::create_dir_all(&nested).expect("nested");
        let status = std::process::Command::new("git")
            .args(["init", "-q", "--initial-branch=main"])
            .current_dir(&repo)
            .status()
            .expect("git init");
        assert!(status.success());

        let (root, worktree, work_area) =
            writer_worktree_paths(&nested, "task_snapshot").expect("paths");
        assert_eq!(root, repo.canonicalize().expect("repo root"));
        assert!(!worktree.starts_with(&root));
        assert_eq!(
            work_area.strip_prefix(&worktree).expect("relative"),
            Path::new("crates/app")
        );
    }
}
