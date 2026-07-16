//! Shared slash-command execution. The TUI feeds parsed [`SlashCommand`]s
//! here; the CLI routes the equivalent `snx` subcommands through the same
//! service functions. Commands that need richer interaction return
//! [`Effect::View`] and the TUI opens the corresponding real-data view; the
//! CLI renders the same data as a report.

use crate::app::App;
use crate::parse::SlashCommand;
use crate::registry::{self, CommandId};
use crate::report::{Report, Sev};
use crate::services;
use crate::status::{self, ActiveContext};
use nexus_core::{NexusError, Result};
use nexus_goals::GoalStatus;

/// Caller-side facts the executor needs.
#[derive(Debug, Clone, Default)]
pub struct ExecCtx {
    /// Active session id, when the surface has one.
    pub session_id: Option<String>,
    /// True in the TUI: view-opening commands return `Effect::View`.
    pub interactive: bool,
    /// Live counters for `/status` (TUI); default zeros elsewhere.
    pub active: ActiveContext,
    /// Read-only live UI context supplied to `/btw` (activity/transcript
    /// snippets). It is never persisted unless the operator passes `--add`.
    pub sidecar_context: String,
}

/// Interactive views the TUI implements. Each renders real data loaded
/// through the same services the CLI uses.
#[derive(Debug, Clone, PartialEq)]
pub enum View {
    Status,
    Goals,
    GoalMenu,
    GoalDetail(String),
    GoalForm,
    Tasks,
    Subagents,
    Resume,
    Sessions,
    Login,
    Model,
    Agents,
    Persona,
    Profile,
    Tools,
    Memory,
    Skills,
    Mcp,
    Connector,
    Theme,
    Thinking,
    Details,
    Transcript,
    Welcome,
    Help,
    Permissions,
    Sandbox,
    Init,
    Config,
    Branch,
    Commit,
}

/// Destructive actions that need a confirmation step first. The TUI shows a
/// dialog and then calls [`apply_confirmed`]; the CLI prompts on a TTY.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmedAction {
    RevertFile(String),
    CancelGoal(String),
    LogoutCodex,
    UseExistingCodex,
    RevokeExistingCodex,
    UseExistingClaude,
    RevokeExistingClaude,
    RemoveCredential {
        provider: String,
        profile: String,
        exit_after: bool,
    },
    ForgetMemory(String),
    DeletePersona(String),
    DeleteProfileTrait(String),
    WriteStarterInstructions,
    InitializeGit,
    SwitchBranch(String),
    DeleteBranch(String),
    CommitFiles {
        paths: Vec<String>,
        message: String,
    },
    ImportConnector {
        id: String,
        preview: String,
    },
    ApprovePlan {
        session_id: String,
        work: Box<nexus_core::orchestration::WorkBreakdown>,
        diff: nexus_core::orchestration::PlanScopeDiff,
    },
    CancelTask(String),
    CancelAgentRun(String),
    CreateTask {
        session_id: String,
        title: String,
        objective: String,
        writer: bool,
    },
}

impl ConfirmedAction {
    pub fn prompt(&self) -> String {
        match self {
            ConfirmedAction::RevertFile(p) => {
                format!("Discard working-tree changes to `{p}`? This cannot be undone.")
            }
            ConfirmedAction::CancelGoal(id) => {
                format!("Cancel goal {id}? Cancelled goals cannot be restarted.")
            }
            ConfirmedAction::LogoutCodex => {
                "Remove the isolated NEXUS Codex session? Your own `codex` CLI login is not touched.".into()
            }
            ConfirmedAction::UseExistingCodex => {
                "Allow NEXUS to read your existing Codex CLI login for this workspace? \
                 The source profile remains read-only and is never modified."
                    .into()
            }
            ConfirmedAction::RevokeExistingCodex => {
                "Stop NEXUS from reading your existing Codex CLI login in this workspace?".into()
            }
            ConfirmedAction::UseExistingClaude => {
                "Allow NEXUS to use your existing official Claude Code subscription login \
                 for the `claude-plan` provider in this workspace? Claude tools remain disabled."
                    .into()
            }
            ConfirmedAction::RevokeExistingClaude => {
                "Stop NEXUS from using the Claude Code subscription login in this workspace? \
                 Your Claude CLI login itself will remain signed in."
                    .into()
            }
            ConfirmedAction::RemoveCredential {
                provider, profile, ..
            } => {
                format!("Delete stored credential {provider}/{profile}?")
            }
            ConfirmedAction::ForgetMemory(id) => {
                format!("Permanently delete memory {id}? This cannot be undone.")
            }
            ConfirmedAction::DeletePersona(id) => {
                format!("Delete persona {id}? Sessions already using it keep their stored id.")
            }
            ConfirmedAction::DeleteProfileTrait(id) => {
                format!("Permanently delete profile trait {id}?")
            }
            ConfirmedAction::WriteStarterInstructions => {
                "Write the previewed canonical AGENTS.md starter? If an empty or unreadable \
                 AGENTS.md exists, it will be replaced."
                    .into()
            }
            ConfirmedAction::InitializeGit => {
                "Run `git init --initial-branch=main` in the exact invocation directory? \
                 NEXUS will not create a nested repository or remove existing Git metadata."
                    .into()
            }
            ConfirmedAction::SwitchBranch(name) => {
                format!("Switch the clean working tree to branch `{name}`?")
            }
            ConfirmedAction::DeleteBranch(name) => {
                format!("Delete merged local branch `{name}`? Unmerged branches are refused.")
            }
            ConfirmedAction::CommitFiles { paths, message } => {
                format!(
                    "Stage {} selected file(s) and commit with message `{message}`?",
                    paths.len()
                )
            }
            ConfirmedAction::ImportConnector { id, preview } => {
                format!(
                    "Import connector `{id}` disabled and untrusted?\n\
                     Credential values are not copied.\n\n{preview}"
                )
            }
            ConfirmedAction::ApprovePlan { work, diff, .. } => format!(
                "Approve plan {} v{}?\n{}",
                work.id,
                work.version,
                if diff.summary.is_empty() {
                    "Initial planned scope.".to_string()
                } else {
                    diff.summary.clone()
                }
            ),
            ConfirmedAction::CancelTask(id) => {
                format!("Cancel background task {id}? Completed side effects are not undone.")
            }
            ConfirmedAction::CancelAgentRun(id) => {
                format!("Cancel agent run {id}? Completed side effects are not undone.")
            }
            ConfirmedAction::CreateTask {
                title,
                objective,
                writer,
                ..
            } => format!(
                "Queue {} background task `{title}`?\nObjective: {objective}\n\
                 Writer tasks use a persistent isolated Git worktree and never auto-commit or merge.",
                if *writer { "a writer" } else { "a reader" }
            ),
        }
    }
}

/// What a command execution asks the surface to do.
#[derive(Debug)]
pub enum Effect {
    /// Render this output.
    Report(Report),
    /// Open an interactive view (TUI only; exec never returns this when
    /// `interactive` is false).
    View(View),
    /// Ask the operator to confirm, then call [`apply_confirmed`].
    Confirm(ConfirmedAction),
    /// Start a fresh session.
    NewSession,
    /// Clear the visible transcript.
    ClearTranscript,
    /// Open a title editor initialized from the active session title.
    EditTitle {
        session_id: String,
        current: String,
    },
    SummaryPreview(services::SummaryArtifact),
    /// Compact the active session's context.
    Compact,
    /// Leave the TUI.
    Quit,
    /// Attach to an existing session (resume).
    AttachSession(String),
    /// Resume an interrupted goal (attaches its session where present).
    ResumeGoal(String),
    /// Apply a theme by name (already persisted).
    SetTheme(String),
    /// Toggle provider reasoning summaries and operational traces.
    SetThinking(bool),
    SetTranscriptDetail(nexus_core::timeline::TranscriptDetail),
    SetTranscriptFilter(nexus_core::timeline::TranscriptFilter),
    ContinueSession {
        id: String,
        report: Report,
        provider_selection_required: bool,
    },
    /// Configuration changed on disk (managed models / credentials): the
    /// surface should rebuild its `App` to pick it up.
    ReloadApp(Report),
}

/// Execute a parsed slash command against the shared services.
pub async fn execute(app: &App, ctx: &ExecCtx, cmd: &SlashCommand) -> Result<Effect> {
    let Some(def) = registry::find(&cmd.name) else {
        return Ok(Effect::Report(unknown_command_report(&cmd.name)));
    };
    if ctx.interactive && !def.interactive {
        return Ok(Effect::Report(Report::untitled().warn(format!(
            "/{} is only available from the non-interactive CLI",
            def.name
        ))));
    }

    let args: Vec<&str> = cmd.args.iter().map(String::as_str).collect();
    let view_or = |view: View, report: Report| -> Effect {
        if ctx.interactive {
            Effect::View(view)
        } else {
            Effect::Report(report)
        }
    };

    Ok(match def.id {
        CommandId::Help => match args.first() {
            Some(name) => Effect::Report(help_for(name)),
            None if ctx.interactive => Effect::View(View::Help),
            None => Effect::Report(help_overview()),
        },
        CommandId::New => Effect::NewSession,
        CommandId::Clear => Effect::ClearTranscript,
        CommandId::Title => {
            let session_id = ctx.session_id.as_deref().ok_or_else(|| {
                NexusError::NotFound("no active session — send a message or /resume first".into())
            })?;
            if cmd.rest.trim().is_empty() {
                let current = app.sessions().get(session_id)?.title;
                Effect::EditTitle {
                    session_id: session_id.to_string(),
                    current,
                }
            } else {
                app.sessions().rename(session_id, cmd.rest.trim())?;
                Effect::Report(
                    Report::untitled().ok(format!("session title → {}", cmd.rest.trim())),
                )
            }
        }
        CommandId::Summary => {
            let session_id = ctx.session_id.as_deref().ok_or_else(|| {
                NexusError::NotFound("no active session — send a message or /resume first".into())
            })?;
            Effect::SummaryPreview(services::build_session_summary(app, session_id)?)
        }
        CommandId::Continue => {
            let session_id = args
                .first()
                .copied()
                .or(ctx.session_id.as_deref())
                .ok_or_else(|| {
                    NexusError::NotFound(
                        "no active session — send a message or /resume first".into(),
                    )
                })?;
            let checkpoint = services::continuation_checkpoint(app, session_id)?;
            Effect::ContinueSession {
                id: checkpoint.child_session_id,
                report: checkpoint.report,
                provider_selection_required: checkpoint.provider_selection_required,
            }
        }
        CommandId::Exit => Effect::Quit,
        CommandId::Compact => Effect::Compact,
        CommandId::Welcome => view_or(View::Welcome, services::welcome_report()),
        CommandId::About => Effect::Report(services::about_report()),

        CommandId::Status => {
            if ctx.interactive {
                Effect::View(View::Status)
            } else {
                let snap = status::snapshot(app, &ctx.active, true).await;
                Effect::Report(status::to_report(&snap))
            }
        }
        CommandId::Usage => Effect::Report(services::usage_report(app).await?),
        CommandId::Setup => Effect::ReloadApp(services::run_setup(app).await?),
        CommandId::Init => match args.first().copied() {
            None if ctx.interactive => Effect::View(View::Init),
            None | Some("preview") => Effect::Report(services::init_report(app)),
            Some("write") => {
                let plan = services::init_plan(app);
                if plan.usable_source.is_some() {
                    Effect::Report(services::init_report(app))
                } else {
                    Effect::Confirm(ConfirmedAction::WriteStarterInstructions)
                }
            }
            Some("git") => {
                let plan = services::init_plan(app);
                if plan.git_repo || plan.malformed_git_metadata {
                    Effect::Report(services::init_report(app))
                } else {
                    Effect::Confirm(ConfirmedAction::InitializeGit)
                }
            }
            _ => usage(def),
        },

        CommandId::Model => match args.as_slice() {
            [] => view_or(View::Model, crate::providers::models_report(app)),
            ["clear"] => {
                let report = services::model_clear(app)?;
                if let Some(session_id) = ctx.session_id.as_deref() {
                    app.sessions().set_model(session_id, "")?;
                }
                Effect::ReloadApp(report)
            }
            ["use", name] | [name] => {
                let report = services::model_select(app, name)?;
                if let Some(session_id) = ctx.session_id.as_deref() {
                    app.sessions().set_model(session_id, name)?;
                    app.sessions().set_status(session_id, "active")?;
                }
                Effect::ReloadApp(report)
            }
            _ => usage(def),
        },
        CommandId::Models => match args.first() {
            Some(&"health") => Effect::Report(crate::providers::models_health_report(app).await?),
            _ => Effect::Report(crate::providers::models_report(app)),
        },

        CommandId::Login => match args.as_slice() {
            [] => view_or(View::Login, login_report(app).await),
            ["claude-plan"] | ["claude"] => Effect::Confirm(ConfirmedAction::UseExistingClaude),
            _ => usage(def),
        },
        CommandId::Logout => match args.as_slice() {
            [] if ctx.interactive => Effect::View(View::Login),
            [] => Effect::Report(Report::untitled().warn(
                "specify what to log out of: `logout codex` or `logout <provider> <profile>`",
            )),
            ["codex"] => Effect::Confirm(ConfirmedAction::LogoutCodex),
            ["claude"] | ["claude-plan"] => Effect::Confirm(ConfirmedAction::RevokeExistingClaude),
            [provider] => Effect::Confirm(ConfirmedAction::RemoveCredential {
                provider: provider.to_string(),
                profile: "default".into(),
                exit_after: true,
            }),
            [provider, profile] => Effect::Confirm(ConfirmedAction::RemoveCredential {
                provider: provider.to_string(),
                profile: profile.to_string(),
                exit_after: true,
            }),
            _ => usage(def),
        },
        CommandId::Auth => match args.as_slice() {
            [] | ["status"] => Effect::Report(services::auth_status_report(app)),
            ["profiles"] => Effect::Report(services::auth_profiles_report(app)?),
            ["use-existing"] => Effect::Confirm(ConfirmedAction::UseExistingCodex),
            ["revoke-existing"] => Effect::Confirm(ConfirmedAction::RevokeExistingCodex),
            ["use-existing-claude"] => Effect::Confirm(ConfirmedAction::UseExistingClaude),
            ["revoke-existing-claude"] => Effect::Confirm(ConfirmedAction::RevokeExistingClaude),
            ["remove", provider, profile] => Effect::Confirm(ConfirmedAction::RemoveCredential {
                provider: provider.to_string(),
                profile: profile.to_string(),
                exit_after: false,
            }),
            _ => usage(def),
        },

        CommandId::Agent => match args.first() {
            Some(role) => {
                let report = services::agent_set(app, role)?;
                if let Some(session_id) = ctx.session_id.as_deref() {
                    app.sessions().set_agent(session_id, role)?;
                }
                Effect::Report(report)
            }
            None => view_or(View::Agents, services::agents_report(app)?),
        },
        CommandId::Agents => view_or(View::Agents, services::agents_report(app)?),
        CommandId::Persona => match args.as_slice() {
            [] => view_or(View::Persona, services::personas_report(app)?),
            ["list"] => Effect::Report(services::personas_report(app)?),
            ["select"] | ["select", "none"] => {
                let report = services::persona_select(app, None)?;
                sync_session_persona_profile(app, ctx.session_id.as_deref())?;
                Effect::Report(report)
            }
            ["select", id] => {
                let report = services::persona_select(app, Some(id))?;
                sync_session_persona_profile(app, ctx.session_id.as_deref())?;
                Effect::Report(report)
            }
            ["create", name, rest @ ..] if !rest.is_empty() => Effect::Report(
                services::persona_create(app, name, "project", None, "", &rest.join(" "))?,
            ),
            ["clone", source, new_name] => {
                Effect::Report(services::persona_clone(app, source, new_name, "project")?)
            }
            ["clone", source, new_name, scope] => {
                Effect::Report(services::persona_clone(app, source, new_name, scope)?)
            }
            ["edit", id, rest @ ..] if !rest.is_empty() => {
                Effect::Report(services::persona_edit(app, id, &rest.join(" "))?)
            }
            ["delete", id] => Effect::Confirm(ConfirmedAction::DeletePersona(id.to_string())),
            _ => usage(def),
        },
        CommandId::Profile => match args.as_slice() {
            [] => view_or(View::Profile, services::profile_report(app, true)?),
            ["list"] => Effect::Report(services::profile_report(app, false)?),
            ["review"] => Effect::Report(services::profile_report(app, true)?),
            ["add", key, rest @ ..] if !rest.is_empty() => Effect::Report(services::profile_add(
                app,
                key,
                &rest.join(" "),
                true,
                ctx.session_id.as_deref(),
            )?),
            ["select", name] => {
                let report = services::profile_select(app, name)?;
                sync_session_persona_profile(app, ctx.session_id.as_deref())?;
                Effect::Report(report)
            }
            ["approve", id] => Effect::Report(services::profile_review(app, id, true)?),
            ["reject", id] => Effect::Report(services::profile_review(app, id, false)?),
            ["delete", id] => Effect::Confirm(ConfirmedAction::DeleteProfileTrait(id.to_string())),
            ["proposals"] => Effect::Report(services::rsi_report(app, false)?),
            ["approve-proposal", id] => Effect::Report(services::rsi_review(app, id, true)?),
            ["reject-proposal", id] => Effect::Report(services::rsi_review(app, id, false)?),
            _ => usage(def),
        },

        CommandId::Goal => match args.as_slice() {
            [] => view_or(View::GoalMenu, services::goals_report(app)?),
            ["show", id] => Effect::Report(services::goal_show_report(app, id)?),
            ["verify", id] => Effect::Report(services::goal_verify_report(app, id)?),
            ["export", id] => {
                Effect::Report(Report::new("goal export").line(app.goals().export(id)?))
            }
            _ => {
                // Fast path: `/goal <objective…>` creates a draft goal.
                let id = services::goal_fast_create(app, &cmd.rest)?;
                if let Some(session_id) = ctx.session_id.as_deref() {
                    services::attach_goal_to_session(app, &id, session_id)?;
                }
                if ctx.interactive {
                    // Show the fast-created goal for editing.
                    return Ok(Effect::View(View::GoalDetail(id)));
                }
                Effect::Report(Report::untitled().ok(format!(
                    "created draft goal {id} — add acceptance criteria before running it"
                )))
            }
        },
        CommandId::Goals => view_or(View::Goals, services::goals_report(app)?),
        CommandId::Plan => {
            let session_id = ctx.session_id.as_deref().ok_or_else(|| {
                NexusError::NotFound("no active session — send a message first".into())
            })?;
            match args.as_slice() {
                [] => match services::latest_plan_for_session(app, session_id) {
                    Ok(work) => Effect::Report(services::plan_report(
                        app,
                        work.id.as_str(),
                        Some(work.version),
                    )?),
                    Err(_) => Effect::Report(
                        Report::new("plan")
                            .warn("no durable plan — create one with /plan create <objective>"),
                    ),
                },
                ["create", rest @ ..] if !rest.is_empty() => {
                    Effect::Report(services::plan_create(app, session_id, &rest.join(" "))?)
                }
                ["edit", rest @ ..] | ["replan", rest @ ..] if !rest.is_empty() => {
                    let (work, diff) = services::plan_revise(app, session_id, &rest.join(" "))?;
                    if diff.requires_approval() {
                        Effect::Confirm(ConfirmedAction::ApprovePlan {
                            session_id: session_id.to_string(),
                            work: Box::new(work),
                            diff,
                        })
                    } else {
                        Effect::Report(services::plan_report(
                            app,
                            work.id.as_str(),
                            Some(work.version),
                        )?)
                    }
                }
                ["approve"] | ["run"] => {
                    let work = services::latest_plan_for_session(app, session_id)?;
                    if work.approved {
                        Effect::Report(services::plan_report(
                            app,
                            work.id.as_str(),
                            Some(work.version),
                        )?)
                    } else {
                        Effect::Confirm(ConfirmedAction::ApprovePlan {
                            session_id: session_id.to_string(),
                            work: Box::new(work),
                            diff: nexus_core::orchestration::PlanScopeDiff {
                                summary: "initial plan approval".into(),
                                ..Default::default()
                            },
                        })
                    }
                }
                ["pause"] => Effect::Report(services::plan_set_paused(app, session_id, true)?),
                ["resume"] => Effect::Report(services::plan_set_paused(app, session_id, false)?),
                ["verify"] => {
                    let work = services::latest_plan_for_session(app, session_id)?;
                    Effect::Report(services::plan_report(
                        app,
                        work.id.as_str(),
                        Some(work.version),
                    )?)
                }
                ["history"] => {
                    let work = services::latest_plan_for_session(app, session_id)?;
                    Effect::Report(services::plan_history_report(app, work.id.as_str())?)
                }
                ["export"] => {
                    let work = services::latest_plan_for_session(app, session_id)?;
                    Effect::Report(
                        Report::new("plan export").line(serde_json::to_string_pretty(&work)?),
                    )
                }
                _ => usage(def),
            }
        }
        CommandId::Task => match args.as_slice() {
            [] if ctx.interactive => Effect::View(View::Tasks),
            [] | ["list"] => {
                Effect::Report(services::tasks_report(app, ctx.session_id.as_deref())?)
            }
            ["create", mode, title, rest @ ..] if !rest.is_empty() => {
                let session_id = ctx.session_id.as_deref().ok_or_else(|| {
                    NexusError::NotFound("no active session for this task".into())
                })?;
                let writer = match *mode {
                    "reader" | "read" => false,
                    "writer" | "write" => true,
                    _ => return Ok(usage(def)),
                };
                let objective = rest.join(" ");
                if writer {
                    Effect::Confirm(ConfirmedAction::CreateTask {
                        session_id: session_id.to_string(),
                        title: title.to_string(),
                        objective,
                        writer,
                    })
                } else {
                    Effect::Report(services::task_create(
                        app, session_id, title, &objective, writer,
                    )?)
                }
            }
            ["show", id] | ["result", id] => Effect::Report(services::task_show_report(app, id)?),
            ["logs", id] => Effect::Report(services::task_logs_report(app, id)?),
            ["pause", id] => Effect::Report(services::task_set_status(
                app,
                id,
                nexus_core::orchestration::TaskStatus::Paused,
            )?),
            ["resume", id] => Effect::Report(services::task_set_status(
                app,
                id,
                nexus_core::orchestration::TaskStatus::Queued,
            )?),
            ["cancel", id] => Effect::Confirm(ConfirmedAction::CancelTask(id.to_string())),
            ["retry", id] => Effect::Report(services::task_retry(app, id)?),
            ["attach", id, session_id] => {
                Effect::Report(services::task_attach(app, id, session_id)?)
            }
            ["cleanup"] => {
                let session_id = ctx.session_id.as_deref().ok_or_else(|| {
                    NexusError::NotFound("no active session for task cleanup".into())
                })?;
                let removed = app.orchestration().cleanup_tasks(session_id)?;
                Effect::Report(
                    Report::untitled().ok(format!("removed {removed} terminal task record(s)")),
                )
            }
            _ => usage(def),
        },
        CommandId::Subagents => {
            let session_id = ctx
                .session_id
                .as_deref()
                .ok_or_else(|| NexusError::NotFound("no active session for subagents".into()))?;
            match args.as_slice() {
                [] if ctx.interactive => Effect::View(View::Subagents),
                [] | ["list"] | ["tree"] => {
                    Effect::Report(services::subagents_report(app, session_id)?)
                }
                ["spawn", role, rest @ ..] if !rest.is_empty() => Effect::Report(
                    services::subagent_spawn(app, session_id, role, &rest.join(" "), None)?,
                ),
                ["fanout", roles, rest @ ..] if !rest.is_empty() => {
                    let mut report = Report::new("subagent fanout");
                    for role in roles.split(',').filter(|role| !role.is_empty()).take(8) {
                        let spawned =
                            services::subagent_spawn(app, session_id, role, &rest.join(" "), None)?;
                        report = report.line(spawned.to_plain_text());
                    }
                    Effect::Report(report)
                }
                ["show", id] | ["collect", id] => {
                    Effect::Report(services::subagent_show_report(app, id)?)
                }
                ["wait", id] => Effect::Report(services::subagent_wait_report(app, id, 30).await?),
                ["wait", id, timeout] => {
                    let timeout = timeout.parse::<u64>().map_err(|_| {
                        NexusError::Config("subagent wait timeout must be seconds".into())
                    })?;
                    Effect::Report(services::subagent_wait_report(app, id, timeout).await?)
                }
                ["cancel", id] => Effect::Confirm(ConfirmedAction::CancelAgentRun(id.to_string())),
                ["retry", id] => Effect::Report(services::subagent_retry(app, id)?),
                ["steer", id, rest @ ..] if !rest.is_empty() => {
                    let run = app.orchestration().agent_run(id)?;
                    Effect::Report(services::subagent_spawn(
                        app,
                        session_id,
                        &run.role,
                        &rest.join(" "),
                        Some(id),
                    )?)
                }
                _ => usage(def),
            }
        }
        CommandId::Pause => {
            let id = target_goal(app, args.first())?;
            services::goal_transition(app, &id, GoalStatus::Paused, "paused by operator")?;
            Effect::Report(Report::untitled().ok(format!("paused goal {id}")))
        }
        CommandId::Cancel => {
            Effect::Confirm(ConfirmedAction::CancelGoal(target_goal(app, args.first())?))
        }

        CommandId::Resume => match args.first() {
            None => view_or(View::Resume, services::resume_report(app)?),
            Some(id) => resolve_resume_target(app, id)?,
        },
        CommandId::Sessions => {
            let report = {
                let list = app.sessions().list(Some(&app.workspace_key), 30)?;
                if list.is_empty() {
                    Report::new("sessions").warn("no sessions yet")
                } else {
                    let rows = list
                        .iter()
                        .map(|s| {
                            vec![
                                s.id.as_str().to_string(),
                                s.agent.clone(),
                                s.model.clone(),
                                s.status.clone(),
                                s.updated_at.clone(),
                            ]
                        })
                        .collect();
                    Report::new("sessions")
                        .table(&["id", "agent", "model", "status", "updated"], rows)
                }
            };
            view_or(View::Sessions, report)
        }

        CommandId::Context => {
            Effect::Report(services::context_report(app, ctx.session_id.as_deref())?)
        }
        CommandId::Details => match args.first().copied() {
            None if ctx.interactive => Effect::View(View::Details),
            None => Effect::Report(Report::new("details").line("compact, expanded, raw")),
            Some(value) => Effect::SetTranscriptDetail(value.parse()?),
        },
        CommandId::Transcript => match args.first().copied() {
            None if ctx.interactive => Effect::View(View::Transcript),
            None => Effect::Report(
                Report::new("transcript filters")
                    .line("all, messages, plans, tools, diffs, agents, warnings, errors"),
            ),
            Some(value) => Effect::SetTranscriptFilter(value.parse()?),
        },
        CommandId::Export => {
            match args.as_slice() {
                [format] => {
                    let session_id = ctx.session_id.as_deref().ok_or_else(|| {
                        NexusError::NotFound("no active session to export".into())
                    })?;
                    Effect::Report(services::export_timeline(app, session_id, format, None)?)
                }
                [format, path] => {
                    let session_id = ctx.session_id.as_deref().ok_or_else(|| {
                        NexusError::NotFound("no active session to export".into())
                    })?;
                    Effect::Report(services::export_timeline(
                        app,
                        session_id,
                        format,
                        Some(path),
                    )?)
                }
                _ => usage(def),
            }
        }

        CommandId::Memory => match args.as_slice() {
            [] => view_or(View::Memory, services::memory_report(app, None)?),
            ["search", rest @ ..] if !rest.is_empty() => {
                Effect::Report(services::memory_report(app, Some(&rest.join(" ")))?)
            }
            ["add", rest @ ..] if !rest.is_empty() => {
                Effect::Report(services::memory_add(app, &rest.join(" "), "operator")?)
            }
            ["forget", id] => Effect::Confirm(ConfirmedAction::ForgetMemory(id.to_string())),
            _ => usage(def),
        },
        CommandId::Skills => match args.as_slice() {
            [] => view_or(View::Skills, services::skills_report(app)?),
            ["enable", name] => Effect::Report(services::skill_set_enabled(app, name, true)?),
            ["disable", name] => Effect::Report(services::skill_set_enabled(app, name, false)?),
            _ => usage(def),
        },
        CommandId::Mcp => match args.as_slice() {
            [] => view_or(View::Mcp, services::mcp_report(app)?),
            ["trust", name] => Effect::Report(services::mcp_set_trust(app, name, true)?),
            ["untrust", name] => Effect::Report(services::mcp_set_trust(app, name, false)?),
            ["tools", name] => Effect::Report(mcp_tools_report(app, name).await?),
            _ => usage(def),
        },
        CommandId::Connector => match args.as_slice() {
            [] if ctx.interactive => Effect::View(View::Connector),
            [] | ["list"] | ["discover"] => Effect::Report(services::connectors_report()?),
            ["show", id] => Effect::Report(services::connector_show_report(id)?),
            ["import", id] => Effect::Confirm(ConfirmedAction::ImportConnector {
                id: id.to_string(),
                preview: crate::connectors::confirmation_preview(id)?,
            }),
            _ => usage(def),
        },
        CommandId::Tools => match args.first() {
            Some(name) => Effect::Report(services::tool_show_report(app, name)?),
            None => view_or(View::Tools, services::tools_report(app)),
        },
        CommandId::Permissions => match args.as_slice() {
            [] => view_or(View::Permissions, services::permissions_report(app)),
            ["show"] => Effect::Report(services::permissions_report(app)),
            [mode] => Effect::ReloadApp(services::set_permission_mode(app, mode)?),
            _ => usage(def),
        },
        CommandId::Sandbox => match args.split_first() {
            Some((&"test", rest)) => {
                let cmd: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
                Effect::Report(services::sandbox_test(app, &cmd).await?)
            }
            Some((&"show", [])) => Effect::Report(services::sandbox_report(app).await),
            Some((&"backend", [backend])) => {
                Effect::ReloadApp(services::set_sandbox_backend(app, backend)?)
            }
            Some((&"network", [mode])) => {
                Effect::ReloadApp(services::set_sandbox_network(app, mode)?)
            }
            Some((&"on", [])) | Some((&"enable", [])) => {
                Effect::ReloadApp(services::set_sandbox_backend(app, "auto")?)
            }
            Some((&"off", [])) | Some((&"disable", [])) => {
                Effect::ReloadApp(services::set_sandbox_backend(app, "none")?)
            }
            None => view_or(View::Sandbox, services::sandbox_report(app).await),
            _ => usage(def),
        },

        CommandId::Diff => {
            let text = crate::gitx::diff(&app.workspace, args.first().copied(), 128 * 1024)?;
            Effect::Report(Report::new("diff").line(text))
        }
        CommandId::Changes => {
            Effect::Report(services::changes_report(app, ctx.session_id.as_deref()))
        }
        CommandId::Revert => match args.first() {
            Some(path) => Effect::Confirm(ConfirmedAction::RevertFile(path.to_string())),
            None => usage(def),
        },
        CommandId::Branch => match args.as_slice() {
            [] if ctx.interactive => Effect::View(View::Branch),
            [] | ["list"] => Effect::Report(services::git_branches_report(app)?),
            ["status"] => Effect::Report(services::git_status_report(app)?),
            ["log"] => Effect::Report(services::git_log_report(app, 30)?),
            ["diff"] => Effect::Report(Report::new("working-tree diff").line(crate::gitx::diff(
                &app.workspace,
                None,
                128 * 1024,
            )?)),
            ["diff", "--staged"] => Effect::Report(
                Report::new("staged diff")
                    .line(crate::gitx::staged_diff(&app.workspace, 128 * 1024)?),
            ),
            ["diff", "--staged", path] => {
                Effect::Report(Report::new(format!("staged diff · {path}")).line(
                    crate::gitx::staged_diff_path(&app.workspace, path, 128 * 1024)?,
                ))
            }
            ["diff", path] => Effect::Report(
                Report::new(format!("working-tree diff · {path}")).line(crate::gitx::diff(
                    &app.workspace,
                    Some(path),
                    128 * 1024,
                )?),
            ),
            ["stage", paths @ ..] if !paths.is_empty() => {
                let paths = paths
                    .iter()
                    .map(|path| (*path).to_string())
                    .collect::<Vec<_>>();
                Effect::Report(Report::untitled().ok(crate::gitx::stage(&app.workspace, &paths)?))
            }
            ["unstage", paths @ ..] if !paths.is_empty() => {
                let paths = paths
                    .iter()
                    .map(|path| (*path).to_string())
                    .collect::<Vec<_>>();
                Effect::Report(Report::untitled().ok(crate::gitx::unstage(&app.workspace, &paths)?))
            }
            ["restore", path] => Effect::Confirm(ConfirmedAction::RevertFile(path.to_string())),
            ["create", name] => Effect::Report(
                Report::untitled().ok(crate::gitx::create_branch(&app.workspace, name)?),
            ),
            ["switch", name] => Effect::Confirm(ConfirmedAction::SwitchBranch(name.to_string())),
            ["delete", name] => Effect::Confirm(ConfirmedAction::DeleteBranch(name.to_string())),
            _ => usage(def),
        },
        CommandId::Commit => {
            if args.is_empty() {
                if ctx.interactive {
                    Effect::View(View::Commit)
                } else {
                    usage(def)
                }
            } else if let Some(split) = args.iter().position(|arg| *arg == "--") {
                let message = args[..split].join(" ");
                let paths = args[split + 1..]
                    .iter()
                    .map(|path| (*path).to_string())
                    .collect::<Vec<_>>();
                crate::gitx::commit_preview(&app.workspace, &paths, 128 * 1024)?;
                Effect::Confirm(ConfirmedAction::CommitFiles { paths, message })
            } else {
                usage(def)
            }
        }
        CommandId::Test => {
            let cmd_args: Vec<String> = cmd.args.clone();
            Effect::Report(services::run_test(app, &cmd_args).await?)
        }
        CommandId::Logs => Effect::Report(services::logs_report(app)),
        CommandId::Audit => Effect::Report(services::audit_report(app, args.first().copied(), 30)?),
        CommandId::Config => match args.first().copied() {
            None if ctx.interactive => Effect::View(View::Config),
            None | Some("show") | Some("advanced") => Effect::Report(services::config_report(app)),
            Some("path") => Effect::Report(
                Report::new("configuration paths")
                    .field("global", app.paths.global_file.display().to_string())
                    .field("project", app.paths.project_file.display().to_string())
                    .field(
                        "managed models",
                        app.paths.managed_models_file.display().to_string(),
                    )
                    .field(
                        "managed overrides",
                        app.paths.managed_overrides_file.display().to_string(),
                    ),
            ),
            _ => usage(def),
        },

        CommandId::Theme => match args.first() {
            Some(name) => {
                let name = crate::theme_names()
                    .iter()
                    .find(|t| **t == *name)
                    .ok_or_else(|| {
                        NexusError::Config(format!(
                            "unknown theme `{name}` — one of: {}",
                            crate::theme_names().join(", ")
                        ))
                    })?;
                app.update_ui_state(|s| s.theme = Some(name.to_string()))?;
                Effect::SetTheme(name.to_string())
            }
            None => view_or(
                View::Theme,
                Report::new("themes").line(crate::theme_names().join(", ")),
            ),
        },
        CommandId::Thinking => match args.first().copied() {
            Some("on") | Some("show") => {
                app.update_ui_state(|state| state.thinking_enabled = true)?;
                Effect::SetThinking(true)
            }
            Some("off") | Some("hide") => {
                app.update_ui_state(|state| state.thinking_enabled = false)?;
                Effect::SetThinking(false)
            }
            Some("toggle") => {
                let enabled = !app.read_ui_state(|state| state.thinking_enabled);
                app.update_ui_state(|state| state.thinking_enabled = enabled)?;
                Effect::SetThinking(enabled)
            }
            None => view_or(
                View::Thinking,
                Report::new("thinking").field(
                    "reasoning summaries & traces",
                    if app.read_ui_state(|state| state.thinking_enabled) {
                        "enabled"
                    } else {
                        "disabled"
                    },
                ),
            ),
            _ => usage(def),
        },

        CommandId::Btw => {
            if cmd.args.iter().any(|arg| arg == "--remember") {
                return Ok(Effect::Report(Report::untitled().warn(
                    "`--remember` is no longer implicit; use `--add` to attach the sidecar response, \
                     or /memory add for durable memory",
                )));
            }
            let add = cmd.args.iter().any(|a| a == "--add");
            let note: Vec<&str> = cmd
                .args
                .iter()
                .filter(|a| *a != "--add")
                .map(String::as_str)
                .collect();
            Effect::Report(
                services::btw(
                    app,
                    ctx.session_id.as_deref(),
                    &note.join(" "),
                    add,
                    &ctx.sidecar_context,
                )
                .await?,
            )
        }
    })
}

/// Perform a previously confirmed destructive action.
pub fn apply_confirmed(app: &App, action: &ConfirmedAction) -> Result<Report> {
    match action {
        ConfirmedAction::RevertFile(path) => {
            let msg = crate::gitx::revert_file(&app.workspace, path)?;
            Ok(Report::untitled().ok(msg))
        }
        ConfirmedAction::CancelGoal(id) => {
            services::goal_transition(app, id, GoalStatus::Cancelled, "cancelled by operator")?;
            Ok(Report::untitled().ok(format!("cancelled goal {id}")))
        }
        ConfirmedAction::LogoutCodex => {
            let paused = services::pause_tasks_for_provider(app, "codex")?;
            let removed = crate::codex::logout_isolated()?;
            app.update_ui_state(|state| state.codex_use_existing = false)?;
            if removed {
                Ok(Report::untitled()
                    .ok("removed the isolated NEXUS Codex session")
                    .field("dependent tasks paused", paused.to_string())
                    .line_sev(
                        "existing-login consent was cleared; your own `codex` CLI login is untouched",
                        Sev::Dim,
                    ))
            } else {
                Ok(Report::untitled()
                    .warn("no isolated Codex session to remove")
                    .line_sev("existing-login consent was cleared", Sev::Dim))
            }
        }
        ConfirmedAction::UseExistingCodex => {
            if crate::codex::status().existing.is_none() {
                return Ok(Report::untitled().warn("no existing Codex CLI login was detected"));
            }
            app.update_ui_state(|state| state.codex_use_existing = true)?;
            Ok(Report::untitled()
                .ok("existing Codex CLI login enabled for this workspace")
                .line_sev("the source profile is consumed read-only", Sev::Dim))
        }
        ConfirmedAction::RevokeExistingCodex => {
            app.update_ui_state(|state| state.codex_use_existing = false)?;
            Ok(Report::untitled().ok("existing Codex CLI login consent revoked"))
        }
        ConfirmedAction::UseExistingClaude => {
            if crate::claude::claude_binary().is_none() {
                return Ok(Report::untitled().warn("the official `claude` CLI is not installed"));
            }
            app.update_ui_state(|state| state.claude_use_existing = true)?;
            Ok(Report::untitled()
                .ok("Claude subscription login enabled for this workspace")
                .line_sev(
                    "NEXUS invokes one non-persistent plan turn with all Claude tools disabled",
                    Sev::Dim,
                ))
        }
        ConfirmedAction::RevokeExistingClaude => {
            let paused = services::pause_tasks_for_provider(app, "claude-plan")?;
            app.update_ui_state(|state| state.claude_use_existing = false)?;
            Ok(Report::untitled()
                .ok("Claude subscription login consent revoked")
                .field("dependent tasks paused", paused.to_string())
                .line_sev("the Claude CLI login itself was not modified", Sev::Dim))
        }
        ConfirmedAction::RemoveCredential {
            provider, profile, ..
        } => {
            let paused = services::pause_tasks_for_provider(app, provider)?;
            if app.credentials.remove(provider, profile)? {
                Ok(Report::untitled()
                    .ok(format!("deleted credential {provider}/{profile}"))
                    .field("dependent tasks paused", paused.to_string()))
            } else {
                Ok(Report::untitled().warn(format!("no credential {provider}/{profile}")))
            }
        }
        ConfirmedAction::ForgetMemory(id) => services::memory_forget(app, id),
        ConfirmedAction::DeletePersona(id) => {
            app.personas().delete(id)?;
            let selected = app.read_ui_state(|state| state.selected_persona.clone());
            if selected.as_deref() == Some(id.as_str()) {
                app.update_ui_state(|state| state.selected_persona = None)?;
            }
            Ok(Report::untitled().ok(format!("deleted persona {id}")))
        }
        ConfirmedAction::DeleteProfileTrait(id) => {
            app.profiles().delete(id)?;
            Ok(Report::untitled().ok(format!("deleted profile trait {id}")))
        }
        ConfirmedAction::WriteStarterInstructions => services::init_write(app, true),
        ConfirmedAction::InitializeGit => services::init_git(app),
        ConfirmedAction::SwitchBranch(name) => {
            Ok(Report::untitled().ok(crate::gitx::switch_branch(&app.workspace, name)?))
        }
        ConfirmedAction::DeleteBranch(name) => {
            Ok(Report::untitled().ok(crate::gitx::delete_branch(&app.workspace, name)?))
        }
        ConfirmedAction::CommitFiles { paths, message } => {
            Ok(Report::untitled().ok(crate::gitx::commit(&app.workspace, paths, message)?))
        }
        ConfirmedAction::ImportConnector { id, preview: _ } => {
            Ok(Report::untitled().ok(crate::connectors::import(app, id)?))
        }
        ConfirmedAction::ApprovePlan {
            session_id,
            work,
            diff,
        } => {
            let mut work = work.clone();
            let approval = app.orchestration().request_plan_approval(&work, diff)?;
            app.orchestration()
                .resolve_plan_approval(&approval.id, true, "operator")?;
            work.approve();
            app.orchestration()
                .save_plan(session_id, &work, "approved", "operator")?;
            Ok(Report::untitled().ok(format!("approved plan {} v{}", work.id, work.version)))
        }
        ConfirmedAction::CancelTask(id) => {
            services::task_set_status(app, id, nexus_core::orchestration::TaskStatus::Cancelled)
        }
        ConfirmedAction::CancelAgentRun(id) => services::subagent_cancel(app, id),
        ConfirmedAction::CreateTask {
            session_id,
            title,
            objective,
            writer,
        } => services::task_create(app, session_id, title, objective, *writer),
    }
}

fn target_goal(app: &App, arg: Option<&&str>) -> Result<String> {
    arg.map(|s| s.to_string())
        .or_else(|| services::active_goal_id(app))
        .ok_or_else(|| NexusError::NotFound("no active goal — create one with /goal".into()))
}

fn sync_session_persona_profile(app: &App, session_id: Option<&str>) -> Result<()> {
    let Some(session_id) = session_id else {
        return Ok(());
    };
    let (persona, profile) =
        app.read_ui_state(|state| (state.selected_persona.clone(), state.profile_name.clone()));
    app.sessions()
        .set_persona_profile(session_id, persona.as_deref(), &profile)
}

fn resolve_resume_target(app: &App, id: &str) -> Result<Effect> {
    if app.sessions().get(id).is_ok() {
        return Ok(Effect::AttachSession(id.to_string()));
    }
    if app.goals().get(id).is_ok() {
        return Ok(Effect::ResumeGoal(id.to_string()));
    }
    Err(NexusError::NotFound(format!(
        "`{id}` is neither a session nor a goal id — run /resume for the list"
    )))
}

fn usage(def: &registry::CommandDef) -> Effect {
    Effect::Report(
        Report::untitled()
            .warn(format!("usage: /{} {}", def.name, def.usage))
            .line_sev(def.summary, Sev::Dim),
    )
}

fn unknown_command_report(name: &str) -> Report {
    let mut r = Report::untitled().error(format!("unknown command `/{name}`"));
    let suggestions = registry::suggest(name);
    if !suggestions.is_empty() {
        r = r.line(format!(
            "did you mean {}?",
            suggestions
                .iter()
                .map(|c| format!("/{}", c.name))
                .collect::<Vec<_>>()
                .join(" or ")
        ));
    }
    r = r.line_sev("see /help for every command", Sev::Dim);
    r
}

fn help_overview() -> Report {
    let mut r = Report::new("commands");
    let mut by_cat: std::collections::BTreeMap<&str, Vec<String>> = Default::default();
    for c in registry::COMMANDS {
        by_cat
            .entry(c.category.label())
            .or_default()
            .push(format!("/{:<12} {}", c.name, c.summary));
    }
    for (cat, lines) in by_cat {
        r = r.header(cat);
        for l in lines {
            r = r.line(l);
        }
    }
    r
}

fn help_for(name: &str) -> Report {
    match registry::find(name) {
        Some(def) => {
            let mut r = Report::new(format!("/{}", def.name))
                .field("summary", def.summary)
                .field(
                    "usage",
                    if def.usage.is_empty() {
                        format!("/{}", def.name)
                    } else {
                        format!("/{} {}", def.name, def.usage)
                    },
                )
                .field("category", def.category.label())
                .field(
                    "surfaces",
                    match (def.interactive, def.non_interactive) {
                        (true, true) => "TUI and CLI",
                        (true, false) => "TUI only",
                        (false, true) => "CLI only",
                        (false, false) => "disabled",
                    },
                );
            if !def.aliases.is_empty() {
                r = r.field(
                    "aliases",
                    def.aliases
                        .iter()
                        .map(|a| format!("/{a}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            if def.requires_confirmation {
                r = r.line_sev("asks for confirmation before executing", Sev::Warn);
            }
            r
        }
        None => unknown_command_report(name),
    }
}

/// `/login` as a non-interactive report (the TUI shows the interactive menu).
async fn login_report(app: &App) -> Report {
    let entries = crate::providers::catalog(app).await;
    let mut r = Report::new("providers");
    for e in &entries {
        let sev = if !e.implemented {
            Sev::Dim
        } else if e.authenticated {
            Sev::Ok
        } else {
            Sev::Warn
        };
        r = r.line_sev(
            format!(
                "{} {:<28} {} — {}",
                e.marker(),
                e.label,
                e.summary(),
                e.auth_state
            ),
            sev,
        );
    }
    r.line_sev(
        "authenticate with `snx auth login` (Codex) or store keys via `/login` in the TUI",
        Sev::Dim,
    )
}

async fn mcp_tools_report(app: &App, name: &str) -> Result<Report> {
    let registry = app.mcp_registry();
    let rec = registry.get(name)?;
    let client = nexus_mcp::McpClient::connect_stdio(
        &rec.name,
        &rec.config.command,
        &rec.config.args,
        &rec.config.env_allowlist,
        rec.config.timeout_secs,
    )
    .await
    .map_err(|e| NexusError::Other(e.to_string()))?;
    let tools = client
        .list_tools()
        .await
        .map_err(|e| NexusError::Other(e.to_string()))?;
    client.shutdown().await;
    registry.record_health(name, &format!("ok: {} tools", tools.len()))?;
    let rows = tools
        .into_iter()
        .map(|t| vec![t.name, t.description])
        .collect();
    Ok(Report::new(format!("{name} tools")).table(&["tool", "description"], rows))
}
