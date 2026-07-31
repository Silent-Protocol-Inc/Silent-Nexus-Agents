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
    Plan,
    Tasks,
    Subagents,
    Resume,
    Sessions,
    Login,
    Connect,
    Model,
    Agents,
    Persona,
    Profile,
    Tools,
    Memory,
    Rsi,
    Skills,
    Mcp,
    Connector,
    Theme,
    Thinking,
    Narrate,
    Activity,
    Details,
    Transcript,
    Welcome,
    Help,
    Permissions,
    Sandbox,
    Init,
    Config,
    Budgets,
    Branch,
    Commit,
    CommandMenu(String),
}

/// Resolve every bare interactive slash command to a menu/view without
/// executing its default behavior. Commands without a dedicated workspace use
/// a generic action menu whose default action re-enters execution with
/// `interactive=false`, preventing recursive menu reopening.
fn bare_interactive_view(def: &registry::CommandDef) -> Option<View> {
    if !def.interactive {
        return None;
    }
    Some(match def.id {
        CommandId::Setup | CommandId::Welcome => View::Welcome,
        CommandId::Init => View::Init,
        CommandId::Model => View::Model,
        CommandId::Catalog => View::CommandMenu("catalog".into()),
        CommandId::Login => View::Login,
        CommandId::Connect => View::Connect,
        CommandId::Agent | CommandId::Agents => View::Agents,
        CommandId::Persona => View::Persona,
        CommandId::Profile => View::Profile,
        CommandId::Goal => View::GoalMenu,
        CommandId::Goals => View::Goals,
        // Kept for the registry invariant below, but `execute` never takes
        // this route: bare `/plan` toggles plan mode instead.
        CommandId::Plan => View::Plan,
        CommandId::Task => View::Tasks,
        CommandId::Subagents => View::Subagents,
        CommandId::Resume => View::Resume,
        CommandId::Sessions => View::Sessions,
        CommandId::Details => View::Details,
        CommandId::Transcript => View::Transcript,
        CommandId::Memory => View::Memory,
        CommandId::Rsi => View::Rsi,
        CommandId::Connector => View::Connector,
        CommandId::Permissions => View::Permissions,
        CommandId::Sandbox => View::Sandbox,
        CommandId::Branch => View::Branch,
        CommandId::Commit => View::Commit,
        CommandId::Config => View::Config,
        CommandId::Theme => View::Theme,
        CommandId::Thinking => View::Thinking,
        CommandId::Narrate => View::Narrate,
        CommandId::View => View::Activity,
        _ => View::CommandMenu(def.name.to_string()),
    })
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
    SetProfileStatus {
        profile_id: String,
        status: nexus_core::harness::ProfileStatus,
    },
    WriteStarterInstructions,
    InitializeGit,
    SwitchBranch(String),
    DeleteBranch(String),
    CommitFiles {
        paths: Vec<String>,
        message: String,
        allow_hooks: bool,
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
    SetConfig {
        workspace: bool,
        path: String,
        value: String,
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
            ConfirmedAction::SetProfileStatus { profile_id, status } => format!(
                "Set profile {profile_id} to {}? Active-profile and default-profile safety checks remain enforced.",
                format!("{status:?}").to_ascii_lowercase()
            ),
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
            ConfirmedAction::CommitFiles {
                paths,
                message,
                allow_hooks,
            } => {
                format!(
                    "Stage {} selected file(s) and commit with message `{message}`?{}",
                    paths.len(),
                    if *allow_hooks {
                        " Repository hooks are explicitly enabled and may execute local code."
                    } else {
                        " Repository hooks remain disabled."
                    }
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
            ConfirmedAction::SetConfig { path, value, .. } => format!(
                "Apply security-weakening configuration `{path} = {value}`? Hard denials, policy evaluation, approval, sandbox metadata, redaction, and audit remain enforced."
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
    /// Attach to a session (resume, or one just created on demand). The
    /// optional report is rendered afterwards, so a command that had to open a
    /// session in order to do its job can still show its own result.
    AttachSession {
        id: String,
        report: Option<Report>,
    },
    /// Resume an interrupted goal (attaches its session where present).
    ResumeGoal(String),
    /// Apply a theme by name (already persisted).
    SetTheme(String),
    /// Set the deliberation mode. Independent of [`Effect::SetActivityMode`].
    SetThinking(nexus_core::ThinkingMode),
    /// Set how much the agent narrates. A third axis: independent of both
    /// [`Effect::SetThinking`] (how much it deliberates) and
    /// [`Effect::SetActivityMode`] (which stored events render).
    SetNarration(nexus_core::timeline::NarrationMode),
    /// Enter or leave plan mode. The flag is already persisted to UI state by
    /// the time this is emitted; the TUI reflects it and the next turn reads
    /// it back when the runtime is built.
    SetPlanMode(bool),
    SetActivityMode(nexus_core::timeline::ActivityMode),
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
    // `/plan` on its own is the mode switch, not a menu. It is the one
    // interactive command whose bare form does something rather than opening
    // a picker, because entering plan mode is the entire reason to type it;
    // the stored-plan view stays reachable through `/plan history` and the
    // other subcommands.
    if ctx.interactive && cmd.args.is_empty() && def.id != CommandId::Plan {
        return Ok(Effect::View(
            bare_interactive_view(def).expect("interactive command has a menu route"),
        ));
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
                let provider_id = app
                    .config
                    .models
                    .get(*name)
                    .map(|model| model.provider.clone());
                app.harness()
                    .execute(crate::control_plane::HarnessAction::SelectModel {
                        session_id: ctx.session_id.clone(),
                        provider_id,
                        model_id: name.to_string(),
                    })?;
                if let Some(session_id) = ctx.session_id.as_deref() {
                    app.sessions().set_model(session_id, name)?;
                    app.sessions().set_status(session_id, "active")?;
                }
                Effect::ReloadApp(report)
            }
            _ => usage(def),
        },
        CommandId::Catalog => match args.first() {
            Some(&"health") => {
                let _ = crate::providers::refresh_catalog(app).await;
                Effect::Report(crate::providers::catalog_report(app).await)
            }
            _ => Effect::Report(crate::providers::catalog_report(app).await),
        },

        CommandId::Login => match args.as_slice() {
            [] => view_or(View::Login, login_report(app).await),
            ["claude-plan"] | ["claude"] => Effect::Confirm(ConfirmedAction::UseExistingClaude),
            _ => usage(def),
        },
        CommandId::Connect => match args.as_slice() {
            [] => view_or(View::Connect, connect_report(app).await),
            // Preserve the 1.0 advanced compatibility form. Bare /connect is
            // now the endpoint/runtime manager; hosted auth belongs to /login.
            ["codex"] | ["openai"] | ["anthropic"] | ["claude-plan"] | ["claude"] => {
                Effect::Report(Report::new("provider authentication moved").line(format!(
                    "use `/login {}` for hosted authentication",
                    args[0]
                )))
            }
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
            Some(sub) if *sub == "show" => match args.get(1) {
                Some(name) => Effect::Report(services::agent_show_report(app, name)?),
                None => usage(registry::find("agent").expect("agent registered")),
            },
            Some(sub) if *sub == "recommend" => {
                let objective = args[1..].join(" ");
                if objective.trim().is_empty() {
                    usage(registry::find("agent").expect("agent registered"))
                } else {
                    Effect::Report(services::agent_recommend_report(app, &objective)?)
                }
            }
            Some(role) => {
                let report = services::agent_set(app, role)?;
                app.harness()
                    .execute(crate::control_plane::HarnessAction::SelectAgent {
                        session_id: ctx.session_id.clone(),
                        agent_id: role.to_string(),
                    })?;
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
            ["show", id] => Effect::Report(services::persona_show_report(app, id)?),
            ["select"] | ["select", "none"] | ["reset"] => {
                let report = services::persona_select(app, None)?;
                app.harness()
                    .execute(crate::control_plane::HarnessAction::SelectPersona {
                        session_id: ctx.session_id.clone(),
                        persona_id: None,
                        version: None,
                    })?;
                sync_session_persona_profile(app, ctx.session_id.as_deref())?;
                Effect::Report(report)
            }
            ["select", id] => {
                let report = services::persona_select(app, Some(id))?;
                app.harness()
                    .execute(crate::control_plane::HarnessAction::SelectPersona {
                        session_id: ctx.session_id.clone(),
                        persona_id: Some(id.to_string()),
                        version: None,
                    })?;
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
            ["list"] | ["profiles"] => Effect::Report(services::profiles_report(app)?),
            ["facts"] => Effect::Report(services::profile_report(app, false)?),
            ["review"] => Effect::Report(services::profile_report(app, true)?),
            ["conflicts"] => Effect::Report(services::profile_conflicts_report(app)?),
            ["resolve", conflict_id, "switch", profile_id] => {
                Effect::Report(services::profile_resolve_conflict(
                    app,
                    ctx.session_id.as_deref(),
                    conflict_id,
                    nexus_core::harness::IdentityConflictDecision::SwitchExisting(
                        profile_id.to_string(),
                    ),
                )?)
            }
            ["resolve", conflict_id, "create"] => {
                Effect::Report(services::profile_resolve_conflict(
                    app,
                    ctx.session_id.as_deref(),
                    conflict_id,
                    nexus_core::harness::IdentityConflictDecision::CreateSeparate,
                )?)
            }
            ["resolve", conflict_id, "keep"] => Effect::Report(services::profile_resolve_conflict(
                app,
                ctx.session_id.as_deref(),
                conflict_id,
                nexus_core::harness::IdentityConflictDecision::KeepActive,
            )?),
            ["resolve", conflict_id, "temporary"] => {
                Effect::Report(services::profile_resolve_conflict(
                    app,
                    ctx.session_id.as_deref(),
                    conflict_id,
                    nexus_core::harness::IdentityConflictDecision::TemporaryContext,
                )?)
            }
            ["resolve", conflict_id, "dismiss"] => {
                Effect::Report(services::profile_resolve_conflict(
                    app,
                    ctx.session_id.as_deref(),
                    conflict_id,
                    nexus_core::harness::IdentityConflictDecision::Dismiss,
                )?)
            }
            ["add", key, rest @ ..] if !rest.is_empty() => Effect::Report(services::profile_add(
                app,
                key,
                &rest.join(" "),
                true,
                ctx.session_id.as_deref(),
            )?),
            ["select", name] => {
                let report = services::profile_select(app, name)?;
                app.harness()
                    .execute(crate::control_plane::HarnessAction::SelectProfileName {
                        session_id: ctx.session_id.clone(),
                        display_name: name.to_string(),
                    })?;
                sync_session_persona_profile(app, ctx.session_id.as_deref())?;
                Effect::Report(report)
            }
            ["approve", id] => Effect::Report(services::profile_review(app, id, true)?),
            ["reject", id] => Effect::Report(services::profile_review(app, id, false)?),
            ["delete", id] => Effect::Confirm(ConfirmedAction::DeleteProfileTrait(id.to_string())),
            ["archive", profile_id] => Effect::Confirm(ConfirmedAction::SetProfileStatus {
                profile_id: profile_id.to_string(),
                status: nexus_core::harness::ProfileStatus::Archived,
            }),
            ["restore", profile_id] => Effect::Report(services::profile_set_status(
                app,
                profile_id,
                nexus_core::harness::ProfileStatus::Active,
            )?),
            ["delete-profile", profile_id] => Effect::Confirm(ConfirmedAction::SetProfileStatus {
                profile_id: profile_id.to_string(),
                status: nexus_core::harness::ProfileStatus::Deleted,
            }),
            ["rename", rest @ ..] if !rest.is_empty() => {
                Effect::Report(services::profile_rename(app, &rest.join(" "))?)
            }
            ["export"] => Effect::Report(services::profile_export(app, None)?),
            ["export", path] => Effect::Report(services::profile_export(app, Some(path))?),
            ["proposals"] => Effect::Report(services::rsi_report(app, false)?),
            ["approve-proposal", id] => Effect::Report(services::rsi_review(app, id, true)?),
            ["reject-proposal", id] => Effect::Report(services::rsi_review(app, id, false)?),
            _ => usage(def),
        },

        CommandId::Goal => match args.as_slice() {
            [] => view_or(View::GoalMenu, services::goals_report(app)?),
            ["show", id] => Effect::Report(services::goal_show_report(app, id)?),
            ["verify", id] => Effect::Report(services::goal_verify_report(app, id)?),
            ["archive", id] => Effect::Report(services::goal_archive(app, id)?),
            ["risks", id] => Effect::Report(services::goal_risks_report(app, id)?),
            ["export", id] => {
                Effect::Report(Report::new("goal export").line(app.goals().export(id)?))
            }
            _ => {
                // Fast path: `/goal <objective…>` creates a draft goal.
                let id = services::goal_fast_create(app, &cmd.rest)?;
                if let Some(session_id) = ctx.session_id.as_deref() {
                    services::attach_goal_to_session(app, &id, session_id)?;
                }
                app.harness()
                    .execute(crate::control_plane::HarnessAction::ActivateWork {
                        session_id: ctx.session_id.clone(),
                        goal_id: Some(id.clone()),
                        plan_id: None,
                        plan_version: None,
                        task_id: None,
                    })?;
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
        // Entering plan mode deliberately precedes the session check below:
        // planning is what you do *before* the first instruction, so requiring
        // a message first would make the mode unreachable when it is most
        // wanted. Every other subcommand acts on a stored plan and still needs
        // a session.
        CommandId::Plan if matches!(args.as_slice(), [] if ctx.interactive) => {
            app.update_ui_state(|s| s.plan_mode = true)?;
            Effect::SetPlanMode(true)
        }
        CommandId::Plan if matches!(args.as_slice(), ["exit"] | ["cancel"]) => {
            app.update_ui_state(|s| s.plan_mode = false)?;
            Effect::SetPlanMode(false)
        }
        // `/plan <free-text>` is not one of the stored-plan subcommands — the
        // operator typed the objective they want planned. Enter plan mode (the
        // same state bare `/plan` produces; its toast tells them to describe the
        // change) rather than falling into the session check below, which
        // answered a typed objective with "no active session — send a message
        // first" — an error the operator had just given the message it asked for
        // and could not act on.
        CommandId::Plan
            if ctx.interactive
                && matches!(
                    args.as_slice(),
                    [first, ..]
                        if !matches!(
                            *first,
                            "create"
                                | "edit"
                                | "replan"
                                | "approve"
                                | "run"
                                | "pause"
                                | "resume"
                                | "verify"
                                | "history"
                                | "export"
                        )
                ) =>
        {
            app.update_ui_state(|s| s.plan_mode = true)?;
            Effect::SetPlanMode(true)
        }
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
            ["graph"] => {
                let session_id = ctx.session_id.as_deref().ok_or_else(|| {
                    NexusError::NotFound("no active session for the task graph".into())
                })?;
                Effect::Report(services::task_graph_report(app, session_id)?)
            }
            ["depend", id, depends_on] => {
                Effect::Report(services::task_depend(app, id, depends_on)?)
            }
            ["validate", id] => Effect::Report(services::task_validate_report(app, id)?),
            ["assign", id, owner] => Effect::Report(services::task_assign(app, id, owner)?),
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
                ["limits"] => Effect::Report(services::subagent_limits_report(app, session_id)?),
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
        CommandId::View => {
            use nexus_core::timeline::ActivityMode;
            let current = |app: &App| {
                ActivityMode::parse(&app.read_ui_state(|s| s.activity_mode.clone()))
                    .unwrap_or_default()
            };
            match args.first().copied() {
                Some("cycle") | Some("next") => {
                    let mode = current(app).cycle();
                    app.update_ui_state(|s| s.activity_mode = mode.as_str().into())?;
                    Effect::SetActivityMode(mode)
                }
                Some(value) => {
                    let mode = ActivityMode::parse(value).ok_or_else(|| {
                        NexusError::Config(format!(
                            "unknown activity view '{value}' (expected default, detailed, or debug)"
                        ))
                    })?;
                    app.update_ui_state(|s| s.activity_mode = mode.as_str().into())?;
                    Effect::SetActivityMode(mode)
                }
                None => view_or(
                    View::Activity,
                    Report::new("view")
                        .field("mode", current(app).as_str())
                        .line("default — essential activity only")
                        .line("detailed — adds reasoning summaries, plans, and stages")
                        .line("debug — adds routing, policy, and provider diagnostics"),
                ),
            }
        }
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
            [] => view_or(
                View::Memory,
                services::memory_report_for_context(app, ctx.session_id.as_deref(), None)?,
            ),
            ["show", id] => Effect::Report(services::memory_show_report_for_context(
                app,
                ctx.session_id.as_deref(),
                id,
            )?),
            ["approve", id] => Effect::Report(services::memory_approve_for_context(
                app,
                ctx.session_id.as_deref(),
                id,
            )?),
            ["reject", id] => Effect::Report(services::memory_reject_for_context(
                app,
                ctx.session_id.as_deref(),
                id,
            )?),
            ["search", rest @ ..] if !rest.is_empty() => {
                Effect::Report(services::memory_report_for_context(
                    app,
                    ctx.session_id.as_deref(),
                    Some(&rest.join(" ")),
                )?)
            }
            ["add", rest @ ..] if !rest.is_empty() => {
                Effect::Report(services::memory_add_for_context(
                    app,
                    ctx.session_id.as_deref(),
                    &rest.join(" "),
                    "operator",
                )?)
            }
            ["forget", id] => Effect::Confirm(ConfirmedAction::ForgetMemory(id.to_string())),
            ["scopes"] => Effect::Report(services::memory_scopes_report(
                app,
                ctx.session_id.as_deref(),
            )?),
            ["stats"] => Effect::Report(services::memory_stats_report(
                app,
                ctx.session_id.as_deref(),
            )?),
            ["candidates"] => Effect::Report(services::memory_candidates_report(
                app,
                ctx.session_id.as_deref(),
            )?),
            ["contradictions"] => Effect::Report(services::memory_contradictions_report(
                app,
                ctx.session_id.as_deref(),
            )?),
            ["export"] => Effect::Report(services::memory_export(app, None)?),
            ["export", path] => Effect::Report(services::memory_export(app, Some(path))?),
            _ => usage(def),
        },
        CommandId::Improve => match args.as_slice() {
            [] | ["list"] => Effect::Report(services::rsi_report(app, false)?),
            ["all"] => Effect::Report(services::rsi_report(app, true)?),
            ["show", id] => Effect::Report(services::improve_show_report(app, id)?),
            ["approve", id] => Effect::Report(services::rsi_review(app, id, true)?),
            ["reject", id] => Effect::Report(services::rsi_review(app, id, false)?),
            ["apply", id] => Effect::Report(services::improve_set_applied(app, id, true)?),
            ["rollback", id] => Effect::Report(services::improve_set_applied(app, id, false)?),
            _ => usage(def),
        },
        CommandId::Rsi => match args.as_slice() {
            [] => view_or(View::Rsi, crate::rsi::status_report(app)?),
            ["status"] => Effect::Report(crate::rsi::status_report(app)?),
            ["candidates"] | ["list"] => Effect::Report(crate::rsi::candidates_report(app)?),
            ["show", id] => Effect::Report(crate::rsi::candidate_show_report(app, id)?),
            ["observations"] => Effect::Report(crate::rsi::observations_report(app)?),
            ["outcomes"] => Effect::Report(crate::rsi::outcomes_report(app)?),
            ["promotions"] => Effect::Report(crate::rsi::promotions_report(app)?),
            ["rollbacks"] => Effect::Report(crate::rsi::rollbacks_report(app)?),
            ["governance"] => Effect::Report(crate::rsi::governance_report()),
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
            ["revoke", token] => Effect::Report(services::revoke_workspace_permission(app, token)?),
            ["format", format, decision] => {
                Effect::ReloadApp(services::set_read_format(app, format, decision, false)?)
            }
            ["format", format, decision, "global"] => {
                Effect::ReloadApp(services::set_read_format(app, format, decision, true)?)
            }
            [mode] if *mode == "full-access" => {
                Effect::Report(services::set_permission_mode(app, mode)?)
            }
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
                Effect::Confirm(ConfirmedAction::CommitFiles {
                    paths,
                    message,
                    allow_hooks: false,
                })
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
            Some("budgets") if ctx.interactive => Effect::View(View::Budgets),
            Some("budgets") => Effect::Report(services::limits_report(app)),
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
            Some("set") if args.len() >= 4 => {
                let workspace = match args[1] {
                    "workspace" => true,
                    "global" => false,
                    _ => return Err(NexusError::Config("scope must be workspace|global".into())),
                };
                let path = args[2].to_string();
                let value = args[3..].join(" ");
                let weakening = (path == "sandbox.backend" && value.contains("none"))
                    || (path == "sandbox.network" && value.contains("full"))
                    || (path.starts_with("policy.") && value.contains("allow"))
                    || (path == "web.enabled" && value == "true")
                    || (path.starts_with("mcp.")
                        && (path.ends_with(".enabled") || path.ends_with(".trust")));
                if weakening {
                    Effect::Confirm(ConfirmedAction::SetConfig {
                        workspace,
                        path,
                        value,
                    })
                } else {
                    Effect::ReloadApp(services::config_set(app, workspace, &path, &value)?)
                }
            }
            Some("reset") if args.len() == 3 => {
                let workspace = match args[1] {
                    "workspace" => true,
                    "global" => false,
                    _ => return Err(NexusError::Config("scope must be workspace|global".into())),
                };
                Effect::ReloadApp(services::config_reset(app, workspace, args[2])?)
            }
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
            Some("status") => Effect::Report(services::thinking_report(app)),
            // `toggle` shipped in 2.3.0 as a boolean flip; it stays working as
            // a three-way cycle rather than breaking existing muscle memory.
            Some("toggle") => {
                let next = app.read_ui_state(|state| state.thinking()).cycle();
                services::set_thinking(app, next)?;
                Effect::SetThinking(next)
            }
            Some(word) => match word.parse::<nexus_core::ThinkingMode>() {
                Ok(mode) => {
                    services::set_thinking(app, mode)?;
                    Effect::SetThinking(mode)
                }
                Err(_) => usage(def),
            },
            None => view_or(View::Thinking, services::thinking_report(app)),
        },

        CommandId::Narrate => match args.first().copied() {
            Some("status") => Effect::Report(services::narration_report(app)),
            Some("cycle") | Some("next") => {
                let next = app.narration_mode().cycle();
                services::set_narration(app, next)?;
                Effect::SetNarration(next)
            }
            Some(word) => match nexus_core::timeline::NarrationMode::parse(word) {
                Some(mode) => {
                    services::set_narration(app, mode)?;
                    Effect::SetNarration(mode)
                }
                None => usage(def),
            },
            None => view_or(View::Narrate, services::narration_report(app)),
        },

        CommandId::Btw => {
            if cmd.args.iter().any(|arg| arg == "--remember") {
                return Ok(Effect::Report(Report::untitled().warn(
                    "`--remember` is no longer implicit; /btw already keeps what you say as \
                     side context for this session, or use /memory add for durable memory",
                )));
            }
            // `--add` spliced the answer into the transcript, which is exactly
            // the per-turn cost /btw exists to avoid. Retaining side context is
            // now the default, so the flag has nothing left to mean.
            if cmd.args.iter().any(|arg| arg == "--add") {
                return Ok(Effect::Report(Report::untitled().warn(
                    "`--add` is gone; /btw now keeps what you say as side context for this \
                     session, which informs later turns without joining the transcript",
                )));
            }
            match cmd.args.first().map(String::as_str) {
                Some("--list") => {
                    Effect::Report(services::btw_list(app, ctx.session_id.as_deref())?)
                }
                Some("--clear") => {
                    Effect::Report(services::btw_clear(app, ctx.session_id.as_deref())?)
                }
                _ => {
                    // Supplying context before asking for anything is the
                    // ordinary use, so open a session rather than refusing.
                    let (id, created) = services::btw_session(app, ctx.session_id.as_deref())?;
                    let report =
                        services::btw(app, &id, &cmd.args.join(" "), &ctx.sidecar_context).await?;
                    if created {
                        Effect::AttachSession {
                            id,
                            report: Some(report),
                        }
                    } else {
                        Effect::Report(report)
                    }
                }
            }
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
        ConfirmedAction::DeleteProfileTrait(id) => services::profile_delete_fact(app, id),
        ConfirmedAction::SetProfileStatus { profile_id, status } => {
            services::profile_set_status(app, profile_id, *status)
        }
        ConfirmedAction::WriteStarterInstructions => services::init_write(app, true),
        ConfirmedAction::InitializeGit => services::init_git(app),
        ConfirmedAction::SwitchBranch(name) => {
            Ok(Report::untitled().ok(crate::gitx::switch_branch(&app.workspace, name)?))
        }
        ConfirmedAction::DeleteBranch(name) => {
            Ok(Report::untitled().ok(crate::gitx::delete_branch(&app.workspace, name)?))
        }
        ConfirmedAction::SetConfig {
            workspace,
            path,
            value,
        } => services::config_set(app, *workspace, path, value),
        ConfirmedAction::CommitFiles {
            paths,
            message,
            allow_hooks,
        } => Ok(Report::untitled().ok(crate::gitx::commit(
            &app.workspace,
            paths,
            message,
            *allow_hooks,
        )?)),
        ConfirmedAction::ImportConnector { id, preview: _ } => {
            Ok(Report::untitled().ok(crate::connectors::import(app, id)?))
        }
        ConfirmedAction::ApprovePlan {
            session_id,
            work,
            diff,
        } => {
            services::approve_plan_work(app, session_id, work, diff)?;
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
        return Ok(Effect::AttachSession {
            id: id.to_string(),
            report: None,
        });
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

/// `/connect` as a non-interactive report: the endpoint/runtime manager, so it
/// lists only local runtimes and configured endpoints (the same filter as the
/// interactive `connect_menu`), not the hosted-auth catalog that `/login` owns.
async fn connect_report(app: &App) -> Report {
    let entries = crate::providers::catalog(app).await;
    let mut r = Report::new("connections");
    let mut shown = 0;
    for e in &entries {
        if !(e.local || e.id.starts_with("custom:") || e.endpoint.is_some()) {
            continue;
        }
        shown += 1;
        let sev = if !e.implemented {
            Sev::Dim
        } else if e.authenticated {
            Sev::Ok
        } else {
            Sev::Warn
        };
        let endpoint = e.endpoint.as_deref().unwrap_or("not configured");
        r = r.line_sev(
            format!(
                "{} {:<28} {} · {}",
                e.marker(),
                e.label,
                e.auth_state,
                endpoint
            ),
            sev,
        );
    }
    if shown == 0 {
        r = r.line_sev("no local runtimes or endpoints configured yet", Sev::Dim);
    }
    r.line_sev(
        "add an endpoint or local runtime with `/connect` in the TUI; hosted sign-in is `/login`",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bare_interactive_command_resolves_to_a_view() {
        for def in registry::COMMANDS.iter().filter(|def| def.interactive) {
            let view = bare_interactive_view(def);
            assert!(view.is_some(), "/{} has no bare menu route", def.name);
        }
    }

    #[test]
    fn generic_command_routes_carry_the_canonical_name() {
        let def = registry::find("status").expect("status command");
        assert_eq!(
            bare_interactive_view(def),
            Some(View::CommandMenu("status".into()))
        );
    }
}
