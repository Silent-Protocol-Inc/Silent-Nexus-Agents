//! `snx` command implementations. Every command routes through the shared
//! `nexus-app` service layer — the same functions the TUI slash commands
//! call — so the two surfaces cannot drift. This file only renders.

use crate::approval::{AutoApproveApprover, AutoDenyApprover, InteractiveApprover};
use crate::cli::*;
use crate::ui::Ui;
use anyhow::{anyhow, Result};
use nexus_agent::{AgentLoop, ApprovalHandler, LoopEvent};
use nexus_app::{services, App};
use nexus_core::brand;
use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

fn ui(app: &App) -> Ui {
    Ui::new(!app.no_color)
}

/// Render a shared report to the terminal (used by thin main.rs dispatches).
pub fn render(app: &App, report: &nexus_app::Report) {
    ui(app).render_report(report);
}

pub fn about(args: AboutArgs, no_color: bool) -> Result<()> {
    let ui = Ui::new(!no_color);
    if args.brand_only {
        ui.render_brand(if args.compact {
            brand::BrandVariant::Compact
        } else {
            brand::BrandVariant::Full
        });
        return Ok(());
    }
    let mut report = services::about_report();
    if args.compact {
        for item in &mut report.items {
            if let nexus_app::Item::Brand { variant } = item {
                *variant = brand::BrandVariant::Compact;
            }
        }
    }
    ui.render_report(&report);
    Ok(())
}

/// Confirm a destructive action on a TTY; non-interactive runs refuse unless
/// pre-authorized by a `--yes`-style flag.
fn confirm(ui: &Ui, prompt: &str, pre_authorized: bool) -> Result<bool> {
    if pre_authorized {
        return Ok(true);
    }
    if !io::stdin().is_terminal() {
        ui.warn("refusing without confirmation (no TTY); pass --yes to authorize");
        return Ok(false);
    }
    print!("{} {} [y/N]: ", ui.yellow("?"), ui.safe(prompt));
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(matches!(input.trim(), "y" | "Y" | "yes"))
}

// ----------------------------------------------------------------------- run

pub async fn run(app: &App, args: RunArgs, json: bool) -> Result<()> {
    let ui = ui(app);
    let objective = args.objective.join(" ");
    if objective.trim().is_empty() {
        return Err(anyhow!(
            "provide an objective, e.g. `snx run \"summarize the repo\"`"
        ));
    }
    let sessions = app.sessions();
    let (session_id, role_name) = match &args.session {
        Some(id) => {
            let meta = sessions
                .get(id)
                .map_err(|_| anyhow!("no such session `{id}`"))?;
            (
                nexus_core::SessionId::from(id.clone()),
                args.agent.clone().unwrap_or(meta.agent),
            )
        }
        None => {
            let role_name = args.agent.clone().unwrap_or_else(|| app.active_agent());
            app.resolve_agent(&role_name)
                .map_err(|_| anyhow!("unknown agent role `{role_name}`"))?;
            let model = app.any_model_name();
            let id = sessions.create(&app.workspace_key, &role_name, &model)?;
            services::attach_active_goal_to_session(app, &id)?;
            (id, role_name)
        }
    };
    let (role, custom_agent) = app
        .resolve_agent(&role_name)
        .map_err(|_| anyhow!("unknown agent role `{role_name}`"))?;

    let runtime = app.runtime(Some(session_id.clone()))?;

    // Stream loop events to the terminal unless emitting JSON.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<LoopEvent>();
    let printer = if json {
        None
    } else {
        let ui2 = ui;
        Some(tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                print_event(&ui2, &ev);
            }
        }))
    };

    // Choose an approver honestly: explicit --yes auto-approves; an attended
    // terminal prompts; an unattended (piped) run denies every escalation.
    let approver: Arc<dyn ApprovalHandler> = if args.yes {
        Arc::new(AutoApproveApprover)
    } else if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        Arc::new(InteractiveApprover::new(ui))
    } else {
        Arc::new(AutoDenyApprover)
    };

    let mut agent_loop = AgentLoop::new(runtime, role);
    if let Some(definition) = custom_agent {
        agent_loop = agent_loop.with_custom_agent(definition);
    }
    if !json {
        agent_loop = agent_loop.with_events(tx);
    } else {
        drop(tx);
    }

    let outcome = agent_loop.run(&session_id, &objective, approver).await;
    // Drop the loop (and with it the event sender) so the printer task sees the
    // channel close and finishes instead of blocking forever.
    drop(agent_loop);
    if let Some(p) = printer {
        let _ = p.await;
    }

    match outcome {
        Ok(o) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&o)?);
            } else {
                ui.header("result");
                println!("{}", ui.safe(&o.final_message));
                println!();
                ui.field("session", session_id.as_str());
                ui.field("steps", &o.steps.to_string());
                ui.field("tool calls", &o.tool_calls.to_string());
                ui.field("stopped", &o.stopped_reason);
                ui.field(
                    "tokens",
                    &format!("{} in / {} out", o.input_tokens, o.output_tokens),
                );
                // Only when the provider actually reported a cache. A line
                // reading `0 read` on Ollama would describe a miss rather than
                // an API that has no cache to report on.
                if o.cache.read > 0 || o.cache.write > 0 {
                    ui.field(
                        "cache",
                        &format!(
                            "{} read / {} written ({:.0}% of input)",
                            o.cache.read,
                            o.cache.write,
                            o.cache.hit_ratio(o.input_tokens) * 100.0
                        ),
                    );
                }
            }
            Ok(())
        }
        Err(e) => Err(anyhow!("{e}")),
    }
}

fn print_event(ui: &Ui, ev: &LoopEvent) {
    match ev {
        // Presentation-only, and the non-interactive surface has no live
        // component to gate — nothing to print.
        LoopEvent::ThinkingResolved { .. } => {}
        // The pinned tracker's feed. A non-interactive run prints stage
        // transitions as they happen instead, so echoing the whole list here
        // would say the same thing twice.
        LoopEvent::WorkPlanned { .. } => {}
        LoopEvent::PlanReviewRequested { request } => {
            ui.warn(&format!(
                "{} submitted plan v{} ({} step(s)) for approval",
                request.agent,
                request.version,
                request.stages.len()
            ));
            for stage in &request.stages {
                println!("  {}. {}", stage.sequence, ui.safe(&stage.title));
            }
        }
        LoopEvent::PlanReviewResolved { decision, .. } => {
            let label = match decision {
                nexus_agent::PlanDecision::Approve => "plan approved",
                nexus_agent::PlanDecision::ApproveWithNote(_) => "plan approved with a note",
                nexus_agent::PlanDecision::RequestChanges(_) => "changes requested",
                nexus_agent::PlanDecision::Decline => "plan declined",
            };
            ui.ok(label);
        }
        LoopEvent::ContextCompacted {
            before_tokens,
            after_tokens,
            summarized_messages,
            model_written,
        } => {
            let line = format!(
                "context compacted · {summarized_messages} messages · \
                 {before_tokens} → {after_tokens} tokens"
            );
            if *model_written {
                println!("{}", ui.dim(&line));
            } else {
                println!("{} · no model summary available", ui.yellow(&line));
            }
        }
        LoopEvent::PlanModeEnded { approved } => {
            if *approved {
                println!("{}", ui.green("plan approved — continuing into execution"));
            } else {
                println!("{}", ui.yellow("plan declined — staying in plan mode"));
            }
        }
        LoopEvent::Classified {
            class,
            model,
            agent,
        } => {
            println!(
                "{} {} · model {} · agent {}",
                ui.dim("classified"),
                ui.cyan(class),
                ui.violet(model),
                ui.violet(agent)
            );
        }
        LoopEvent::ModelFallback {
            from_model,
            to_model,
            provider,
            reason,
        } => {
            println!(
                "{} {} → {} · provider {} · {}",
                ui.yellow("pre-stream fallback"),
                ui.safe(from_model),
                ui.safe(to_model),
                ui.safe(provider),
                ui.safe(reason)
            );
        }
        LoopEvent::ProviderActivity {
            effort,
            reasoning_enabled,
            running,
            failed,
            ..
        } => {
            let phase = if *running {
                "started"
            } else if *failed {
                "failed"
            } else {
                "completed"
            };
            let label = if *reasoning_enabled {
                format!("Thinking… · {effort}")
            } else {
                "Generating… · reasoning off/unsupported".into()
            };
            println!("{} {}", ui.dim(phase), ui.safe(&label));
        }
        LoopEvent::ReasoningSummary(t) => {
            println!("{} {}", ui.dim("reasoning"), ui.safe(t))
        }
        // The full-screen TUI updates a stable card for every delta. The
        // line-oriented CLI prints the completed answer below, avoiding one
        // terminal line per provider chunk.
        LoopEvent::AssistantTextDelta(_) => {}
        LoopEvent::AssistantStreamFailed(reason) => {
            println!("  {} {}", ui.yellow("stream interrupted"), ui.safe(reason))
        }
        LoopEvent::FinalAnswer(_) => {}
        LoopEvent::PlanPromoted {
            work,
            from,
            to,
            reason,
        } => {
            println!(
                "{} {} → {} · plan v{} — {}",
                ui.yellow("scope promoted"),
                from,
                to,
                work.version,
                ui.safe(reason)
            );
        }
        LoopEvent::PlanResolved { work, approved, .. } => {
            println!(
                "{} plan {} v{}",
                if *approved {
                    ui.green("approved")
                } else {
                    ui.red("denied")
                },
                work.id,
                work.version
            );
        }
        LoopEvent::StageChanged { title, status, .. } => {
            println!(
                "{} {} [{}]",
                ui.dim("stage"),
                ui.safe(title),
                status.as_str()
            );
        }
        LoopEvent::ToolPlan {
            tool,
            summary,
            risk,
            ..
        } => {
            println!(
                "{} {} [{}] {}",
                ui.dim("tool"),
                ui.cyan(tool),
                ui.risk(risk),
                ui.safe(summary)
            );
        }
        LoopEvent::PolicyDecision {
            tool,
            decision,
            layer,
            reason,
        } => {
            println!(
                "  {} {} · {} · {} — {}",
                ui.dim("policy"),
                tool,
                layer,
                decision,
                ui.safe(reason)
            );
        }
        LoopEvent::ApprovalRequested { tool, .. } => {
            println!("  {} {}", ui.yellow("awaiting approval:"), tool);
        }
        LoopEvent::ToolExecutionStarted { tool } => println!("  {} {}", ui.dim("▶"), tool),
        LoopEvent::ToolExecutionFinished {
            tool,
            ok,
            preview,
            duration_ms,
            ..
        } => {
            let mark = if *ok { ui.green("✓") } else { ui.red("✗") };
            println!(
                "  {} {} {} {}ms",
                mark,
                tool,
                ui.dim(&ui.safe(preview)),
                duration_ms
            );
        }
        LoopEvent::DiffProduced {
            tool,
            path,
            insertions,
            deletions,
            preview,
        } => {
            let target = path.as_deref().unwrap_or(tool.as_str());
            println!(
                "  {} {} {}",
                ui.cyan("diff"),
                target,
                ui.dim(&format!("(+{insertions} -{deletions})"))
            );
            for line in preview.lines() {
                let rendered = ui.safe(line);
                let styled = match line.as_bytes().first() {
                    Some(b'+') => ui.green(&rendered),
                    Some(b'-') => ui.red(&rendered),
                    _ => ui.dim(&rendered),
                };
                println!("    {styled}");
            }
        }
        LoopEvent::Retry {
            attempt,
            max,
            reason,
        } => {
            println!(
                "  {} {attempt}/{max}: {}",
                ui.yellow("retry"),
                ui.safe(reason)
            );
        }
        LoopEvent::Error(e) => println!("  {} {}", ui.red("error"), ui.safe(e)),
    }
}

// -------------------------------------------------------------------- status

pub async fn status(app: &App, json: bool) -> Result<()> {
    let active = nexus_app::status::ActiveContext::default();
    let snap = nexus_app::status::snapshot(app, &active, true).await;
    if json {
        // Serialize the human-facing report lines (stable enough for scripts).
        println!(
            "{}",
            serde_json::json!({
                "workspace": snap.workspace,
                "agent": snap.agent,
                "model": snap.model.as_ref().map(|m| &m.name),
                "sandbox": snap.sandbox_level,
                "git_branch": snap.git_branch,
                "git_modified": snap.git_modified.len(),
                "mcp_total": snap.mcp_total,
            })
        );
        return Ok(());
    }
    ui(app).render_report(&nexus_app::status::to_report(&snap));
    Ok(())
}

// --------------------------------------------------------------------- model

pub async fn model(app: &App, cmd: ModelCmd, json: bool) -> Result<()> {
    let ui = ui(app);
    match cmd {
        ModelCmd::Show => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "active": app.any_model_name(),
                        "pinned": app.pinned_model,
                        "configured": app.config.models.keys().collect::<Vec<_>>(),
                    })
                );
                return Ok(());
            }
            ui.field("active", &app.any_model_name());
            ui.field(
                "pinned",
                app.pinned_model.as_deref().unwrap_or("no (config routing)"),
            );
            ui.render_report(&nexus_app::providers::models_report(app));
        }
        ModelCmd::Use { name } => ui.render_report(&services::model_select(app, &name)?),
        ModelCmd::Clear => ui.render_report(&services::model_clear(app)?),
        ModelCmd::Health => {
            ui.render_report(&nexus_app::providers::models_health_report(app).await?)
        }
        ModelCmd::Test { name } => {
            ui.render_report(&nexus_app::providers::test_model(app, &name).await?)
        }
    }
    Ok(())
}

// -------------------------------------------------------------------- resume

pub async fn resume(app: &App, id: Option<String>, json: bool) -> Result<()> {
    let ui = ui(app);
    match id {
        None => {
            if json {
                let list = services::resume_candidates(app)?;
                let v: Vec<_> = list
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "kind": c.kind, "id": c.id, "title": c.title,
                            "status": c.status, "last_activity": c.last_activity,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&v)?);
                return Ok(());
            }
            ui.render_report(&services::resume_report(app)?);
            println!(
                "  {}",
                ui.dim("resume a session with `snx run --session <id> \"<objective>\"` or inside the TUI via /resume")
            );
        }
        Some(id) => {
            if app.sessions().get(&id).is_ok() {
                if let Some(report) = services::resume_recovery_report(app, &id)? {
                    ui.render_report(&report);
                }
                ui.ok(&format!("session `{id}` exists — continue it with:"));
                println!("  snx run --session {id} \"<your next objective>\"");
            } else if app.goals().get(&id).is_ok() {
                ui.render_report(&services::goal_show_report(app, &id)?);
                println!(
                    "  {}",
                    ui.dim("resume this goal inside the TUI (/resume) or with `snx run`")
                );
            } else {
                return Err(anyhow!("`{id}` is neither a session nor a goal id"));
            }
        }
    }
    Ok(())
}

pub async fn continue_session(app: &App, id: Option<String>, json: bool) -> Result<()> {
    let session_id = id
        .or_else(|| app.read_ui_state(|state| state.last_session.clone()))
        .or_else(|| {
            app.sessions()
                .list(Some(&app.workspace_key), 1)
                .ok()?
                .first()
                .map(|session| session.id.as_str().to_string())
        })
        .ok_or_else(|| anyhow!("no session exists in this workspace"))?;
    let checkpoint = services::continuation_checkpoint(app, &session_id)?;
    let child = checkpoint.child_session_id;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "parent_session": session_id,
                "child_session": child,
                "resume": format!("snx resume {child}"),
                "provider_selection_required": checkpoint.provider_selection_required,
            })
        );
    } else {
        ui(app).render_report(&checkpoint.report);
    }
    Ok(())
}

pub async fn summary(app: &App, session: Option<String>, json: bool) -> Result<()> {
    let ui = ui(app);
    let session_id = session
        .or_else(|| app.read_ui_state(|state| state.last_session.clone()))
        .or_else(|| {
            app.sessions()
                .list(Some(&app.workspace_key), 1)
                .ok()?
                .first()
                .map(|session| session.id.as_str().to_string())
        })
        .ok_or_else(|| anyhow!("no session exists in this workspace"))?;
    let artifact = services::build_session_summary(app, &session_id)?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "session_id": artifact.session_id,
                "path": artifact.path,
                "content": artifact.content,
            })
        );
        return Ok(());
    }
    println!("{}", artifact.content);
    ui.field("saved", &artifact.path.display().to_string());
    match nexus_app::clipboard::copy(&artifact.content) {
        Ok(method) => ui.ok(&format!("copied via {method}")),
        Err(_) => ui.warn("clipboard unavailable; the saved artifact remains copyable"),
    }
    if confirm(
        &ui,
        "Create a linked fresh session containing only this approved handoff?",
        false,
    )? {
        let (new_id, report) = services::rollover_summary(app, &session_id, &artifact.content)?;
        ui.render_report(&report);
        ui.field("resume", &format!("snx resume {new_id}"));
    }
    Ok(())
}

// ---------------------------------------------------------------------- test

pub async fn test(app: &App, command: Vec<String>) -> Result<()> {
    ui(app).render_report(&services::run_test(app, &command).await?);
    Ok(())
}

pub async fn branch(app: &App, command: BranchCmd) -> Result<()> {
    let ui = ui(app);
    match command {
        BranchCmd::List => ui.render_report(&services::git_branches_report(app)?),
        BranchCmd::Status => ui.render_report(&services::git_status_report(app)?),
        BranchCmd::Diff { staged, path } => {
            let text = match (staged, path.as_deref()) {
                (true, Some(path)) => {
                    nexus_app::gitx::staged_diff_path(&app.workspace, path, 128 * 1024)?
                }
                (true, None) => nexus_app::gitx::staged_diff(&app.workspace, 128 * 1024)?,
                (false, path) => nexus_app::gitx::diff(&app.workspace, path, 128 * 1024)?,
            };
            ui.render_report(
                &nexus_app::Report::new(if staged {
                    "staged diff"
                } else {
                    "working-tree diff"
                })
                .line(text),
            );
        }
        BranchCmd::Stage { paths } => ui.ok(&nexus_app::gitx::stage(&app.workspace, &paths)?),
        BranchCmd::Unstage { paths } => ui.ok(&nexus_app::gitx::unstage(&app.workspace, &paths)?),
        BranchCmd::Restore { path, yes } => {
            let action = nexus_app::ConfirmedAction::RevertFile(path);
            if confirm(&ui, &action.prompt(), yes)? {
                ui.render_report(&nexus_app::apply_confirmed(app, &action)?);
            }
        }
        BranchCmd::Log { limit } => ui.render_report(&services::git_log_report(app, limit)?),
        BranchCmd::Create { name } => {
            ui.ok(&nexus_app::gitx::create_branch(&app.workspace, &name)?)
        }
        BranchCmd::Switch { name, yes } => {
            let action = nexus_app::ConfirmedAction::SwitchBranch(name);
            if confirm(&ui, &action.prompt(), yes)? {
                ui.render_report(&nexus_app::apply_confirmed(app, &action)?);
            }
        }
        BranchCmd::Delete { name, yes } => {
            let action = nexus_app::ConfirmedAction::DeleteBranch(name);
            if confirm(&ui, &action.prompt(), yes)? {
                ui.render_report(&nexus_app::apply_confirmed(app, &action)?);
            }
        }
    }
    Ok(())
}

pub async fn commit(app: &App, args: CommitArgs) -> Result<()> {
    let ui = ui(app);
    ui.render_report(&services::commit_preview_report(
        app,
        &args.files,
        &args.message,
    )?);
    let action = nexus_app::ConfirmedAction::CommitFiles {
        paths: args.files,
        message: args.message,
        allow_hooks: args.allow_hooks,
    };
    if confirm(&ui, &action.prompt(), args.yes)? {
        ui.render_report(&nexus_app::apply_confirmed(app, &action)?);
    }
    Ok(())
}

// --------------------------------------------------------------------- theme

pub async fn theme(app: &App, name: Option<String>) -> Result<()> {
    let ui = ui(app);
    match name {
        None => {
            ui.field("active", &app.theme_name());
            ui.field("available", &nexus_app::theme_names().join(", "));
        }
        Some(name) => {
            if !nexus_app::theme_names().contains(&name.as_str()) {
                return Err(anyhow!(
                    "unknown theme `{name}` — one of: {}",
                    nexus_app::theme_names().join(", ")
                ));
            }
            app.update_ui_state(|s| s.theme = Some(name.clone()))?;
            ui.ok(&format!("theme set to `{name}`"));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ thinking

pub async fn thinking(app: &App, mode: Option<String>) -> Result<()> {
    let ui = ui(app);
    match mode.as_deref() {
        None | Some("status") => {
            let status = nexus_app::services::thinking_status(app);
            ui.field("mode", status.mode.as_str());
            ui.field("deep planning", yes_no(status.deep_planning));
            ui.field("summaries", yes_no(status.summarize_provider_reasoning));
            ui.field("min duration", &format!("{}ms", status.minimum_duration_ms));
            println!("{}", ui.dim(status.description()));
        }
        Some(word) => {
            let mode: nexus_core::ThinkingMode = word
                .parse()
                .map_err(|_| anyhow!("unknown mode `{word}` — one of: off, on, auto"))?;
            nexus_app::services::set_thinking(app, mode)?;
            ui.ok(&format!("thinking set to `{mode}`"));
        }
    }
    Ok(())
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

// ---------------------------------------------------------------------- goal

pub async fn goal(app: &App, cmd: GoalCmd, json: bool) -> Result<()> {
    let ui = ui(app);
    let goals = app.goals();
    let target = |id: Option<String>| -> Result<String> {
        id.or_else(|| services::active_goal_id(app))
            .ok_or_else(|| anyhow!("no active goal — create one with `snx goal new`"))
    };
    match cmd {
        GoalCmd::List => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&goals.list(Some(&app.workspace_key))?)?
                );
                return Ok(());
            }
            ui.render_report(&services::goals_report(app)?);
        }
        GoalCmd::New {
            title,
            criteria,
            objective,
            token_budget,
        } => {
            let title = title.join(" ");
            if criteria.is_empty() {
                ui.warn("no acceptance criteria given; the goal can never verify as complete");
            }
            let id = services::goal_create(
                app,
                services::GoalSpec {
                    objective: objective.unwrap_or_default(),
                    title,
                    acceptance_criteria: criteria,
                    token_budget: Some(token_budget),
                    ..Default::default()
                },
            )?;
            ui.ok(&format!("created goal {id}"));
        }
        GoalCmd::Show { id } => {
            if json {
                let g = goals.get(&id)?;
                let steps = goals.steps(&id)?;
                println!("{}", serde_json::json!({ "goal": g, "steps": steps }));
                return Ok(());
            }
            ui.render_report(&services::goal_show_report(app, &id)?);
        }
        GoalCmd::Verify { id } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&goals.verify(&id)?)?);
                return Ok(());
            }
            ui.render_report(&services::goal_verify_report(app, &id)?);
        }
        GoalCmd::Pause { id } => {
            let id = target(id)?;
            services::goal_transition(
                app,
                &id,
                nexus_goals::GoalStatus::Paused,
                "paused by operator",
            )?;
            ui.ok(&format!("paused goal {id}"));
        }
        GoalCmd::Resume { id } => {
            let id = target(id)?;
            services::goal_transition(
                app,
                &id,
                nexus_goals::GoalStatus::Running,
                "resumed by operator",
            )?;
            ui.ok(&format!("goal {id} is running again"));
        }
        GoalCmd::Cancel { id, yes } => {
            let id = target(id)?;
            let action = nexus_app::ConfirmedAction::CancelGoal(id);
            if confirm(&ui, &action.prompt(), yes)? {
                ui.render_report(&nexus_app::apply_confirmed(app, &action)?);
            }
        }
        GoalCmd::Plan { id } => {
            let id = target(id)?;
            ui.render_report(&services::goal_show_report(app, &id)?);
        }
        GoalCmd::Recover => {
            let list = goals.recoverable(&app.workspace_key)?;
            if list.is_empty() {
                ui.ok("no interrupted goals to recover");
                return Ok(());
            }
            ui.header("recoverable goals");
            for g in list {
                println!(
                    "  {} [{}] {}",
                    g.id.as_str(),
                    g.status.as_str(),
                    ui.safe(&g.title)
                );
            }
        }
        GoalCmd::Export { id } => {
            println!("{}", goals.export(&id)?);
        }
    }
    Ok(())
}

// ------------------------------------------------------------------- session

pub async fn session(app: &App, cmd: SessionCmd, json: bool) -> Result<()> {
    let ui = ui(app);
    let sessions = app.sessions();
    match cmd {
        SessionCmd::List => {
            let list = sessions.list(Some(&app.workspace_key), 50)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
                return Ok(());
            }
            if list.is_empty() {
                ui.warn("no sessions yet");
                return Ok(());
            }
            let rows: Vec<Vec<String>> = list
                .iter()
                .map(|s| {
                    vec![
                        s.id.as_str().to_string(),
                        s.agent.clone(),
                        s.model.clone(),
                        s.created_at.clone(),
                    ]
                })
                .collect();
            ui.table(&["id", "agent", "model", "created"], &rows);
        }
        SessionCmd::Show { id } => {
            let messages = sessions.messages(&id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&messages)?);
                return Ok(());
            }
            for m in messages {
                let role = format!("{:?}", m.role).to_lowercase();
                println!("{}", ui.cyan(&format!("── {role} ──")));
                if !m.content.is_empty() {
                    println!("{}", ui.safe(&m.content));
                }
                for tc in &m.tool_calls {
                    println!(
                        "  {} {}({})",
                        ui.dim("call"),
                        tc.name,
                        ui.safe(&tc.arguments)
                    );
                }
            }
        }
        SessionCmd::Title { id, title } => {
            let title = title.join(" ");
            if title.trim().is_empty() {
                return Err(anyhow!("session title cannot be empty"));
            }
            app.sessions().rename(&id, &title)?;
            ui.ok(&format!("session {id} title → {title}"));
        }
    }
    Ok(())
}

pub async fn persona(app: &App, cmd: PersonaCmd, json: bool) -> Result<()> {
    let ui = ui(app);
    match cmd {
        PersonaCmd::List => {
            if json {
                println!("{}", serde_json::to_string_pretty(&app.personas().list()?)?);
            } else {
                ui.render_report(&services::personas_report(app)?);
            }
        }
        PersonaCmd::Create {
            name,
            instructions,
            scope,
            parent,
            description,
        } => ui.render_report(&services::persona_create(
            app,
            &name,
            &scope,
            parent.as_deref(),
            &description,
            &instructions.join(" "),
        )?),
        PersonaCmd::Clone {
            source,
            new_name,
            scope,
        } => ui.render_report(&services::persona_clone(app, &source, &new_name, &scope)?),
        PersonaCmd::Edit { id, instructions } => {
            ui.render_report(&services::persona_edit(app, &id, &instructions.join(" "))?)
        }
        PersonaCmd::Delete { id, yes } => {
            let action = nexus_app::ConfirmedAction::DeletePersona(id);
            if confirm(&ui, &action.prompt(), yes)? {
                ui.render_report(&nexus_app::apply_confirmed(app, &action)?);
            }
        }
        PersonaCmd::Select { id } => {
            ui.render_report(&services::persona_select(app, id.as_deref())?)
        }
    }
    Ok(())
}

pub async fn profile(app: &App, cmd: ProfileCmd, json: bool) -> Result<()> {
    let ui = ui(app);
    match cmd {
        ProfileCmd::List { all } => {
            if json {
                let profile = app.read_ui_state(|state| state.profile_name.clone());
                println!(
                    "{}",
                    serde_json::to_string_pretty(&app.profiles().list(&profile, all)?)?
                );
            } else {
                ui.render_report(&services::profile_report(app, all)?);
            }
        }
        ProfileCmd::Add { key, value } => ui.render_report(&services::profile_add(
            app,
            &key,
            &value.join(" "),
            true,
            None,
        )?),
        ProfileCmd::Select { name } => ui.render_report(&services::profile_select(app, &name)?),
        ProfileCmd::Approve { id } => ui.render_report(&services::profile_review(app, &id, true)?),
        ProfileCmd::Reject { id } => ui.render_report(&services::profile_review(app, &id, false)?),
        ProfileCmd::Delete { id, yes } => {
            let action = nexus_app::ConfirmedAction::DeleteProfileTrait(id);
            if confirm(&ui, &action.prompt(), yes)? {
                ui.render_report(&nexus_app::apply_confirmed(app, &action)?);
            }
        }
        ProfileCmd::Proposals { all } => ui.render_report(&services::rsi_report(app, all)?),
        ProfileCmd::ApproveProposal { id } => {
            ui.render_report(&services::rsi_review(app, &id, true)?)
        }
        ProfileCmd::RejectProposal { id } => {
            ui.render_report(&services::rsi_review(app, &id, false)?)
        }
    }
    Ok(())
}

// -------------------------------------------------------------------- memory

pub async fn memory(app: &App, cmd: MemoryCmd, json: bool) -> Result<()> {
    let ui = ui(app);
    let mem = app.memory();
    match cmd {
        MemoryCmd::List { all } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&mem.list(all, 100)?)?);
                return Ok(());
            }
            ui.render_report(&services::memory_report(app, None)?);
        }
        MemoryCmd::Add {
            content,
            kind,
            scope,
        } => {
            let content = content.join(" ");
            let kind = nexus_memory::MemoryKind::parse(&kind)
                .ok_or_else(|| anyhow!("unknown memory kind `{kind}`"))?;
            let id = mem.add(nexus_memory::NewMemory {
                kind,
                content,
                source: "cli".into(),
                confidence: 1.0,
                scope,
                sensitivity: "normal".into(),
                requires_approval: false,
                ttl_days: None,
            })?;
            ui.ok(&format!("stored memory {}", id.as_str()));
        }
        MemoryCmd::Search { query } => {
            let query = query.join(" ");
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&mem.search(&query, 20)?)?
                );
                return Ok(());
            }
            ui.render_report(&services::memory_report(app, Some(&query))?);
        }
        MemoryCmd::Approve { id } => {
            // Promote in both stores. Flipping only the legacy row leaves the
            // canonical record a candidate, so `/memory` and `snx memory list`
            // keep reporting the memory as unapproved however often it is
            // approved. The harness path updates the pair.
            ui.render_report(&services::memory_approve(app, &id)?);
        }
        MemoryCmd::Forget { id, yes } => {
            let action = nexus_app::ConfirmedAction::ForgetMemory(id);
            if confirm(&ui, &action.prompt(), yes)? {
                ui.render_report(&nexus_app::apply_confirmed(app, &action)?);
            }
        }
        MemoryCmd::Prune => {
            let n = mem.prune()?;
            ui.ok(&format!("pruned {n} expired memories"));
        }
        MemoryCmd::Export => {
            println!("{}", mem.export()?);
        }
    }
    Ok(())
}

// --------------------------------------------------------------------- skill

pub async fn skill(app: &App, cmd: SkillCmd, json: bool) -> Result<()> {
    let ui = ui(app);
    let skills = app.skills();
    match cmd {
        SkillCmd::List => {
            if json {
                println!("{}", serde_json::to_string_pretty(&skills.list()?)?);
                return Ok(());
            }
            ui.render_report(&services::skills_report(app)?);
        }
        SkillCmd::Show { name } => {
            println!("{}", skills.export(&name)?);
        }
        SkillCmd::Enable { name } => {
            ui.render_report(&services::skill_set_enabled(app, &name, true)?);
        }
        SkillCmd::Disable { name } => {
            ui.render_report(&services::skill_set_enabled(app, &name, false)?);
        }
        SkillCmd::Import { file } => {
            let path = app.guard.resolve_existing(&file)?;
            let text = std::fs::read_to_string(path)?;
            let id = skills.import(&text)?;
            ui.ok(&format!(
                "imported skill {} (disabled; enable explicitly)",
                id.as_str()
            ));
        }
        SkillCmd::Export { name } => {
            println!("{}", skills.export(&name)?);
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------- mcp

pub async fn mcp(app: &App, cmd: McpCmd, json: bool) -> Result<()> {
    let ui = ui(app);
    let registry = app.mcp_registry();
    match cmd {
        McpCmd::List => {
            if json {
                println!("{}", serde_json::to_string_pretty(&registry.list()?)?);
                return Ok(());
            }
            ui.render_report(&services::mcp_report(app)?);
        }
        McpCmd::Add {
            name,
            command,
            args,
        } => {
            let cfg = nexus_core::config::McpServerConfig {
                transport: "stdio".into(),
                command,
                args,
                enabled: true,
                ..Default::default()
            };
            let id = registry.add(&name, cfg)?;
            ui.ok(&format!(
                "registered MCP server `{name}` ({}) — untrusted by default",
                id.as_str()
            ));
        }
        McpCmd::Remove { name } => {
            registry.remove(&name)?;
            ui.ok(&format!("removed `{name}`"));
        }
        McpCmd::Trust { name } => {
            ui.render_report(&services::mcp_set_trust(app, &name, true)?);
        }
        McpCmd::Untrust { name } => {
            ui.render_report(&services::mcp_set_trust(app, &name, false)?);
        }
        McpCmd::Tools { name } => {
            let rec = registry.get(&name)?;
            let client = nexus_mcp::McpClient::connect_stdio(
                &rec.name,
                &rec.config.command,
                &rec.config.args,
                &rec.config.env_allowlist,
                rec.config.timeout_secs,
            )
            .await
            .map_err(|e| anyhow!("{e}"))?;
            let tools = client.list_tools().await.map_err(|e| anyhow!("{e}"))?;
            client.shutdown().await;
            registry.record_health(&name, &format!("ok: {} tools", tools.len()))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&tools)?);
                return Ok(());
            }
            ui.header(&format!("{name} tools ({})", tools.len()));
            for t in tools {
                println!(
                    "  {} {}",
                    ui.cyan(&t.name),
                    ui.dim(&ui.safe(&t.description))
                );
            }
        }
        McpCmd::Serve => {
            serve_mcp(app).await?;
        }
    }
    Ok(())
}

/// Run NEXUS as an MCP server over stdio, exposing only the curated,
/// read-only capability set.
async fn serve_mcp(app: &App) -> Result<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();
    let indexer = app.indexer();
    let guard = app.guard.clone();

    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = req.get("id").and_then(|i| i.as_u64());
        let params = req.get("params").cloned();

        let resp = nexus_mcp::server::handle_request(method, id, params, |name, args| {
            let indexer = &indexer;
            let guard = &guard;
            async move { dispatch_exposed(indexer, guard, &name, args) }
        })
        .await;

        // Notifications (no id) get no response.
        if id.is_some() || resp.error.is_some() {
            let mut out = serde_json::to_string(&resp)?;
            out.push('\n');
            stdout.write_all(out.as_bytes()).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

fn dispatch_exposed(
    indexer: &nexus_index::Indexer,
    guard: &nexus_core::workspace::WorkspaceGuard,
    name: &str,
    args: serde_json::Value,
) -> std::result::Result<String, String> {
    match name {
        "nexus.search_code" => {
            let q = args.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let hits = indexer.find_symbol(q, 25).map_err(|e| e.to_string())?;
            Ok(hits
                .iter()
                .map(|(file, s)| format!("{file}:{} {} {}", s.line, s.kind, s.name))
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "nexus.read_file" => {
            let p = args.get("path").and_then(|n| n.as_str()).unwrap_or("");
            let resolved = guard.resolve_existing(p).map_err(|e| e.to_string())?;
            let text = std::fs::read_to_string(resolved).map_err(|e| e.to_string())?;
            Ok(nexus_core::sanitize::sanitize_terminal(&text))
        }
        "nexus.project_structure" => {
            let status = indexer.status().map_err(|e| e.to_string())?;
            Ok(format!(
                "{} files, {} symbols indexed",
                status.files, status.symbols
            ))
        }
        other => Err(format!("unknown exposed tool `{other}`")),
    }
}

pub async fn connector(app: &App, cmd: ConnectorCmd, json: bool) -> Result<()> {
    let ui = ui(app);
    match cmd {
        ConnectorCmd::List => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&nexus_app::connectors::discover()?)?
                );
            } else {
                ui.render_report(&services::connectors_report()?);
            }
        }
        ConnectorCmd::Show { id } => {
            ui.render_report(&services::connector_show_report(&id)?);
        }
        ConnectorCmd::Import { id, yes } => {
            let preview = nexus_app::connectors::confirmation_preview(&id)?;
            let action = nexus_app::ConfirmedAction::ImportConnector { id, preview };
            if confirm(&ui, &action.prompt(), yes)? {
                ui.render_report(&nexus_app::apply_confirmed(app, &action)?);
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------- sandbox

pub async fn sandbox(app: &App, cmd: SandboxCmd, json: bool) -> Result<()> {
    let ui = ui(app);
    match cmd {
        SandboxCmd::Status => {
            if json {
                let net = match app.config.sandbox.network.as_str() {
                    "off" | "none" => nexus_sandbox::NetworkMode::Off,
                    "full" => nexus_sandbox::NetworkMode::Full,
                    _ => nexus_sandbox::NetworkMode::Restricted,
                };
                let report = app.sandbox.backend().isolation(net);
                println!("{}", serde_json::to_string_pretty(&report)?);
                return Ok(());
            }
            ui.render_report(&services::sandbox_report(app).await);
        }
        SandboxCmd::Test { command } => {
            ui.render_report(&services::sandbox_test(app, &command).await?);
        }
    }
    Ok(())
}

// --------------------------------------------------------------------- index

pub async fn index(app: &App, cmd: IndexCmd, json: bool) -> Result<()> {
    let ui = ui(app);
    let indexer = app.indexer();
    match cmd {
        IndexCmd::Build => {
            let status =
                indexer.build_with_policy(&app.workspace, &app.guard, &app.config.policy)?;
            ui.ok(&format!(
                "indexed {} files, {} symbols",
                status.files, status.symbols
            ));
        }
        IndexCmd::Status => {
            let status = indexer.status()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
                return Ok(());
            }
            ui.field("files", &status.files.to_string());
            ui.field("symbols", &status.symbols.to_string());
        }
        IndexCmd::Symbol { name } => {
            let hits = indexer.find_symbol(&name, 50)?;
            if hits.is_empty() {
                ui.warn("no matching symbols (build the index first with `snx index build`)");
                return Ok(());
            }
            for (file, s) in hits {
                println!(
                    "  {}:{} {} {}",
                    ui.cyan(&file),
                    ui.dim(&s.line.to_string()),
                    ui.violet(s.kind.as_str()),
                    ui.safe(&s.name)
                );
            }
        }
        IndexCmd::File { path } => {
            let symbols = indexer.file_symbols(&path)?;
            for s in symbols {
                println!(
                    "  {} {} {}",
                    ui.dim(&s.line.to_string()),
                    s.kind.as_str(),
                    ui.safe(&s.name)
                );
            }
        }
        IndexCmd::Clean => {
            indexer.clean()?;
            ui.ok("index cleared");
        }
    }
    Ok(())
}

// --------------------------------------------------------------------- tools

pub async fn tools(app: &App, cmd: ToolsCmd, json: bool) -> Result<()> {
    let ui = ui(app);
    match cmd {
        ToolsCmd::List => {
            if json {
                let registry = app.tools();
                let mut metas: Vec<_> = registry.all().map(|t| t.meta().clone()).collect();
                metas.sort_by(|a, b| a.name.cmp(&b.name));
                println!("{}", serde_json::to_string_pretty(&metas)?);
                return Ok(());
            }
            ui.render_report(&services::tools_report(app));
        }
        ToolsCmd::Show { name } => {
            if json {
                let registry = app.tools();
                let tool = registry.get(&name)?;
                println!("{}", serde_json::to_string_pretty(tool.meta())?);
                return Ok(());
            }
            ui.render_report(&services::tool_show_report(app, &name)?);
            let registry = app.tools();
            let tool = registry.get(&name)?;
            ui.header("input schema");
            println!(
                "{}",
                serde_json::to_string_pretty(&tool.meta().input_schema)?
            );
        }
    }
    Ok(())
}

// ------------------------------------------------------------------- catalog

pub async fn catalog(app: &App, cmd: CatalogCmd, json: bool) -> Result<()> {
    let ui = ui(app);
    match cmd {
        CatalogCmd::List => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&nexus_app::providers::catalog(app).await)?
                );
                return Ok(());
            }
            ui.render_report(&nexus_app::providers::catalog_report(app).await);
        }
        CatalogCmd::Health => {
            let refreshed = nexus_app::providers::refresh_catalog(app).await;
            if json {
                println!("{}", serde_json::to_string_pretty(&refreshed)?);
                return Ok(());
            }
            ui.render_report(&nexus_app::providers::catalog_report(app).await);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------- auth

pub async fn auth(app: &App, cmd: AuthCmd, json: bool) -> Result<()> {
    let ui = ui(app);
    match cmd {
        AuthCmd::Status => {
            if json {
                let s = nexus_app::codex::status();
                println!(
                    "{}",
                    serde_json::json!({
                        "cli_installed": s.cli_installed,
                        "isolated_logged_in": s.isolated.is_some(),
                        "isolated_mode": s.isolated.as_ref().map(|p| p.mode),
                        "existing_cli_logged_in": s.existing.is_some(),
                        "active_source": s.active_source.map(|x| x.label()),
                    })
                );
                return Ok(());
            }
            ui.render_report(&services::auth_status_report(app));
        }
        AuthCmd::Login(args) => auth_login(app, args).await?,
        AuthCmd::Logout => {
            let action = nexus_app::ConfirmedAction::LogoutCodex;
            if confirm(&ui, &action.prompt(), false)? {
                ui.render_report(&nexus_app::apply_confirmed(app, &action)?);
            }
        }
        AuthCmd::Profiles => {
            ui.render_report(&services::auth_profiles_report(app)?);
        }
        AuthCmd::Remove { provider, profile } => {
            let action = nexus_app::ConfirmedAction::RemoveCredential {
                provider,
                profile,
                exit_after: false,
            };
            if confirm(&ui, &action.prompt(), false)? {
                ui.render_report(&nexus_app::apply_confirmed(app, &action)?);
            }
        }
    }
    Ok(())
}

async fn auth_login(app: &App, args: AuthLoginArgs) -> Result<()> {
    use nexus_app::codex::{self, DeviceLoginEvent};
    let ui = ui(app);

    let status = codex::status();
    if !status.cli_installed {
        ui.warn("the `codex` CLI was not found on PATH.");
        println!(
            "  NEXUS delegates Codex login to the official CLI rather than\n  \
             reimplementing OpenAI OAuth. Install it, then re-run `snx auth login`."
        );
        return Err(anyhow!("codex CLI not installed"));
    }

    // Existing-login detection (never modified, only offered for import).
    if status.existing.is_some() && status.isolated.is_none() && !args.import && !args.api_key {
        ui.ok("existing Codex CLI login detected");
        println!(
            "  {}\n  {}\n",
            ui.dim("NEXUS will not use it without explicit consent;"),
            ui.dim("use `--use-existing`, copy it with `--import`, or continue for a fresh device login")
        );
    }

    if args.use_existing {
        let action = nexus_app::ConfirmedAction::UseExistingCodex;
        if confirm(&ui, &action.prompt(), false)? {
            ui.render_report(&nexus_app::apply_confirmed(app, &action)?);
        }
        return Ok(());
    }

    if args.import {
        let source = nexus_models::codex_auth::auth_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~/.codex/auth.json".into());
        let dest = nexus_models::codex_auth::nexus_auth_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        ui.header("import existing Codex login");
        ui.field("source", &format!("{source} (read once, never modified)"));
        ui.field("destination", &dest);
        if !confirm(&ui, "Copy the session into isolated NEXUS storage?", false)? {
            return Ok(());
        }
        let profile = codex::import_existing()?;
        ui.ok(&format!(
            "imported ({} mode) — original profile untouched",
            profile.mode
        ));
        report_plan_models(&ui).await;
        return Ok(());
    }

    if args.api_key {
        let key = read_secret_line("  OpenAI API key: ")?;
        let key = nexus_core::SecretString::new(key.trim().to_string());
        let profile = codex::login_with_api_key(&key).await?;
        ui.ok(&format!(
            "logged in to the isolated profile ({})",
            profile.mode
        ));
        report_plan_models(&ui).await;
        return Ok(());
    }

    // Device login (default, also with --device).
    ui.field("method", "device login via `codex login --device-auth`");
    ui.field(
        "isolation",
        &format!(
            "CODEX_HOME={} (child process only)",
            nexus_models::codex_auth::nexus_isolated_home()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        ),
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<DeviceLoginEvent>();
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let printer = {
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                match ev {
                    DeviceLoginEvent::VerificationUrl(u) => ui.field("open", &u),
                    DeviceLoginEvent::UserCode(c) => ui.field("code", &c),
                    DeviceLoginEvent::Info(line) => println!("  {}", ui.dim(&line)),
                    DeviceLoginEvent::Success { mode, account_id } => {
                        ui.ok(&format!(
                            "logged in ({mode}){}",
                            account_id
                                .map(|a| format!(" account {a}"))
                                .unwrap_or_default()
                        ));
                    }
                    DeviceLoginEvent::Failed(e) => ui.warn(&e),
                }
            }
        })
    };
    let result = codex::device_login(tx, cancel_rx).await;
    let _ = printer.await;
    result?;
    report_plan_models(&ui).await;
    Ok(())
}

/// After a successful Codex login, list (and cache) the models on the
/// operator's plan so the default model reflects the account, not a preset.
async fn report_plan_models(ui: &Ui) {
    match nexus_app::codex::list_plan_models().await {
        Ok(models) => {
            ui.ok(&format!(
                "{} model(s) available on your plan:",
                models.len()
            ));
            for m in &models {
                let mark = if m.is_default { " (default)" } else { "" };
                println!("    {} — {}{}", m.id, m.display_name, ui.dim(mark));
            }
            println!(
                "  {}",
                ui.dim("pick one with `snx model use <name>` or /connect in the TUI")
            );
        }
        Err(e) => ui.warn(&format!("could not list your plan's models: {e}")),
    }
}

fn read_secret_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let echo_disabled = set_terminal_echo(false).unwrap_or(false);
    let mut input = String::new();
    let result = io::stdin().read_line(&mut input);
    if echo_disabled {
        let _ = set_terminal_echo(true);
        println!();
    }
    result?;
    Ok(input)
}

fn set_terminal_echo(enabled: bool) -> Result<bool> {
    #[cfg(unix)]
    {
        let arg = if enabled { "echo" } else { "-echo" };
        let status = std::process::Command::new("stty")
            .arg(arg)
            .status()
            .map_err(|e| anyhow!("failed to run stty: {e}"))?;
        Ok(status.success())
    }
    #[cfg(not(unix))]
    {
        let _ = enabled;
        Ok(false)
    }
}

// -------------------------------------------------------------------- config

pub async fn config(app: &App, cmd: ConfigCmd) -> Result<()> {
    let ui = ui(app);
    match cmd {
        ConfigCmd::Show => {
            println!("{}", serde_json::to_string_pretty(&*app.config)?);
        }
        ConfigCmd::Budgets => {
            ui.render_report(&nexus_app::services::limits_report(app));
        }
        ConfigCmd::Set {
            path,
            value,
            workspace,
        } => {
            ui.render_report(&nexus_app::services::config_set(
                app, workspace, &path, &value,
            )?);
        }
        ConfigCmd::Reset { path, workspace } => {
            ui.render_report(&nexus_app::services::config_reset(app, workspace, &path)?);
        }
        ConfigCmd::Path => {
            ui.field("global", &app.paths.global_file.display().to_string());
            ui.field(
                "managed models",
                &app.paths.managed_models_file.display().to_string(),
            );
            ui.field("project", &app.paths.project_file.display().to_string());
            ui.field("state", &app.paths.state_dir.display().to_string());
        }
        ConfigCmd::Schema => {
            println!(
                "{}",
                serde_json::to_string_pretty(&nexus_core::config::Config::json_schema())?
            );
        }
    }
    Ok(())
}

// --------------------------------------------------------------------- audit

pub async fn audit(app: &App, args: AuditArgs, json: bool) -> Result<()> {
    if json {
        let rows = app.audit().query(args.kind.as_deref(), None, args.limit)?;
        let v: Vec<_> = rows
            .iter()
            .map(|(id, at, kind, payload)| {
                serde_json::json!({"id": id, "at": at, "kind": kind, "payload": payload})
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    ui(app).render_report(&services::audit_report(
        app,
        args.kind.as_deref(),
        args.limit,
    )?);
    Ok(())
}

// ---------------------------------------------------------------------- logs

pub async fn logs(app: &App) -> Result<()> {
    ui(app).render_report(&services::logs_report(app));
    Ok(())
}

pub async fn init(app: &App) -> Result<()> {
    let ui = ui(app);
    let plan = services::init_plan(app);
    ui.render_report(&services::init_report(app));
    if plan.usable_source.is_none() {
        let prompt = if plan.target.exists() {
            format!(
                "Replace {} with the previewed starter?",
                plan.target.display()
            )
        } else {
            format!("Write {}?", plan.target.display())
        };
        if confirm(&ui, &prompt, false)? {
            ui.render_report(&services::init_write(app, true)?);
        }
    }
    if plan.git_init_needed
        && confirm(
            &ui,
            "Initialize Git here with `git init --initial-branch=main`?",
            false,
        )?
    {
        ui.render_report(&services::init_git(app)?);
    } else if plan.malformed_git_metadata {
        ui.warn(
            "invalid or incomplete .git metadata was left untouched; repair it manually before git init",
        );
    }
    Ok(())
}

// --------------------------------------------------------------------- setup

/// First-run onboarding. Detects local runtimes/models and the host GPU, then
/// writes a ready-to-use starter config. Runs without an existing config.
pub async fn setup(args: SetupArgs, no_color: bool) -> Result<()> {
    let ui = Ui::new(!no_color);
    ui.banner();

    let workspace = std::env::current_dir()?;
    let paths =
        nexus_core::config::ConfigPaths::discover(&workspace).map_err(|e| anyhow!("{e}"))?;
    for private_root in [
        &paths.project_dir,
        &paths.state_dir,
        &paths.global_dir,
        &paths.auth_dir,
    ] {
        if private_root.exists() {
            nexus_core::permissions::repair_private_tree(private_root)?;
        }
    }
    let target = if args.project {
        paths.project_file.clone()
    } else {
        paths.global_file.clone()
    };
    let scope = if args.project {
        "project"
    } else {
        "global (applies in any folder)"
    };

    // Detect hardware + local runtimes.
    let gpu = nexus_core::gpu::detect();
    ui.field("gpu", &gpu.summary());
    println!(
        "{}",
        ui.dim("  scanning for local model runtimes (Ollama / llama.cpp / OpenAI-compatible)…")
    );
    let runtimes = nexus_models::detect_local_models().await;

    let mut models: Vec<(String, String, String, String)> = Vec::new(); // (cfg_name, provider, base_url, model)
    for rt in &runtimes {
        if rt.models.is_empty() {
            ui.warn(&format!(
                "{} is reachable but reports no models{}",
                rt.label,
                if rt.provider == "ollama" {
                    " — pull one with `ollama pull llama3.2`"
                } else {
                    ""
                }
            ));
            continue;
        }
        ui.ok(&format!(
            "{}: {} model(s) — {}",
            rt.label,
            rt.models.len(),
            rt.models.join(", ")
        ));
        for m in &rt.models {
            let name = sanitize_model_name(m);
            let name = dedup_name(&models, name);
            models.push((name, rt.provider.clone(), rt.base_url.clone(), m.clone()));
        }
    }

    // Note Codex/GPT availability (auth-based, not a local model).
    let allow_existing = nexus_app::uistate::UiStateFile::load(&paths.ui_state_file)
        .map(|state| state.state.codex_use_existing)
        .unwrap_or(false);
    let codex_mode = nexus_models::codex_auth::load_with_consent(allow_existing)
        .ok()
        .flatten()
        .map(|c| c.mode);
    let codex_available = codex_mode.is_some();
    match codex_mode {
        Some("api_key") => ui.ok("Codex API-key session found — GPT wired via `auth = \"codex\"`"),
        Some("oauth") => {
            ui.ok("Codex \"Sign in with ChatGPT\" session found — GPT wired via `auth = \"codex\"`")
        }
        _ => {}
    }

    let toml = build_config_toml(&models, &gpu, codex_available);
    let has_any_model = !models.is_empty() || codex_available;

    // Validate before writing so we never emit a broken config.
    let parsed: std::result::Result<nexus_core::config::Config, _> = toml::from_str(&toml);
    match parsed {
        Ok(cfg) => cfg
            .validate()
            .map_err(|e| anyhow!("generated config failed validation: {e}"))?,
        Err(e) => return Err(anyhow!("internal: generated config did not parse: {e}")),
    }

    if target.exists() {
        ui.warn(&format!(
            "{} config already exists at {}",
            scope,
            target.display()
        ));
        println!(
            "  {}",
            ui.dim("setup preserves existing configuration; it will not overwrite this file.")
        );
        if args.force {
            println!(
                "  {}",
                ui.dim("--force now means refresh discovery only; destructive replacement is disabled.")
            );
        }
        if !args.project && (!models.is_empty() || codex_available) {
            let mut managed = nexus_core::config::Config::load_managed_models(&paths)
                .map_err(|e| anyhow!("{e}"))?;
            let before = managed.len();
            for (name, provider, base_url, model_id) in &models {
                managed
                    .entry(name.clone())
                    .or_insert_with(|| nexus_core::config::ModelConfig {
                        provider: provider.clone(),
                        base_url: base_url.clone(),
                        model: model_id.clone(),
                        role: "executor".into(),
                        ..Default::default()
                    });
            }
            if codex_available {
                managed
                    .entry("codex".into())
                    .or_insert_with(|| nexus_core::config::ModelConfig {
                        provider: "codex".into(),
                        base_url: String::new(),
                        model: nexus_app::codex::cached_default_model()
                            .unwrap_or_else(|| "gpt-5.5".into()),
                        context_window: 128_000,
                        max_output_tokens: 8192,
                        role: "executor".into(),
                        ..Default::default()
                    });
            }
            if managed.len() != before {
                nexus_core::config::Config::save_managed_models(&paths, &managed)
                    .map_err(|e| anyhow!("{e}"))?;
                ui.ok(&format!(
                    "added {} discovered model(s) to {}",
                    managed.len() - before,
                    paths.managed_models_file.display()
                ));
            }
        }
        if !has_any_model {
            print_no_model_guidance(&ui, codex_available);
        }
        return Ok(());
    }

    if let Some(parent) = target.parent() {
        nexus_core::permissions::repair_private_tree(parent)?;
    }
    nexus_core::atomic::atomic_write_private(&target, toml.as_bytes())?;

    println!();
    ui.ok(&format!("wrote {scope} config → {}", target.display()));
    if !has_any_model {
        print_no_model_guidance(&ui, codex_available);
    } else {
        ui.header("next");
        println!(
            "  {}  {}",
            ui.cyan("snx doctor"),
            ui.dim("verify everything is wired")
        );
        println!(
            "  {}  {}",
            ui.cyan("snx catalog health"),
            ui.dim("probe your model server(s)")
        );
        println!(
            "  {}  {}",
            ui.cyan("snx"),
            ui.dim("launch the TUI, or `snx run \"...\"`")
        );
    }
    Ok(())
}

fn print_no_model_guidance(ui: &Ui, codex_available: bool) {
    ui.header("no local model runtime found");
    println!("  NEXUS never downloads models. To get one running locally, either:");
    println!(
        "    {}  {}",
        ui.cyan("Ollama"),
        ui.dim("install from ollama.com, then `ollama pull llama3.2`")
    );
    println!(
        "    {}  {}",
        ui.cyan("llama.cpp"),
        ui.dim("run `llama-server -m your-model.gguf --port 8080`")
    );
    println!("  Then re-run {}.", ui.cyan("snx setup --force"));
    if codex_available {
        println!(
            "\n  {} Your Codex session is wired as [models.codex] — GPT works now.",
            ui.green("✓")
        );
    } else {
        println!(
            "\n  Or use a hosted model: set an API key (see docs/providers.md) or run {} for GPT via Codex.",
            ui.cyan("snx auth login")
        );
    }
}

/// Turn a model id like `llama3.1:8b-instruct` into a TOML-key-safe name.
fn sanitize_model_name(model: &str) -> String {
    let mut s: String = model
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    let s = s.trim_matches('_').to_string();
    if s.is_empty() {
        "model".into()
    } else {
        s
    }
}

fn dedup_name(existing: &[(String, String, String, String)], mut name: String) -> String {
    let base = name.clone();
    let mut n = 2;
    while existing.iter().any(|(e, ..)| e == &name) {
        name = format!("{base}_{n}");
        n += 1;
    }
    name
}

use nexus_app::services::build_config_toml;

// -------------------------------------------------------------------- doctor

pub async fn doctor(app: &App, args: DoctorArgs, json: bool) -> Result<()> {
    let ui = ui(app);
    let mut checks: Vec<(String, bool, String)> = Vec::new();

    checks.push((
        "configuration".into(),
        true,
        format!("valid (version {})", app.config.version),
    ));
    checks.push((
        "workspace".into(),
        true,
        app.workspace.display().to_string(),
    ));
    checks.push((
        "database".into(),
        true,
        app.store.path().display().to_string(),
    ));

    let backend = app.sandbox.backend();
    let avail = backend.availability().await;
    let report = backend.isolation(nexus_sandbox::NetworkMode::Off);
    checks.push((
        "sandbox".into(),
        avail.is_ok(),
        match &avail {
            Ok(n) => format!("{} — {}", report.level, n),
            Err(e) => format!("{} — {e}", report.level),
        },
    ));

    // GPU / accelerator is informational: absence is fine (CPU-first), so this
    // check is always "ok" and simply reports what was detected.
    let gpu = nexus_core::gpu::detect();
    checks.push((
        "gpu / accelerator".into(),
        true,
        if gpu.has_gpu() {
            format!(
                "{} — local models can offload to {}",
                gpu.summary(),
                gpu.primary_backend().unwrap_or("GPU")
            )
        } else {
            "none detected — CPU-only (NEXUS is CPU-first; prefer smaller/quantized models)".into()
        },
    ));

    checks.push((
        "models configured".into(),
        !app.config.models.is_empty(),
        format!("{} configured", app.config.models.len()),
    ));

    let detected = nexus_models::detect_local_servers().await;
    checks.push((
        "local model servers".into(),
        !detected.is_empty(),
        if detected.is_empty() {
            "none detected on default ports".into()
        } else {
            detected
                .iter()
                .map(|(l, _)| l.clone())
                .collect::<Vec<_>>()
                .join(", ")
        },
    ));

    if args.deep {
        let maintenance = nexus_core::maintenance::check(
            &app.store,
            &app.paths.state_dir,
            &app.artifacts,
            &[app.paths.state_dir.clone(), app.paths.global_dir.clone()],
        )?;
        checks.push((
            "database integrity".into(),
            maintenance.database_integrity.eq_ignore_ascii_case("ok"),
            maintenance.database_integrity.clone(),
        ));
        checks.push((
            "state permissions".into(),
            maintenance.permission_issues.is_empty(),
            if maintenance.permission_issues.is_empty() {
                "private directories 0700 and files 0600".into()
            } else {
                format!(
                    "{} permission issue(s)",
                    maintenance.permission_issues.len()
                )
            },
        ));
        checks.push((
            "artifact integrity".into(),
            maintenance.artifact_issues.is_empty(),
            format!(
                "{} artifact(s), {} bytes, {} issue(s)",
                maintenance.artifact_count,
                maintenance.artifact_bytes,
                maintenance.artifact_issues.len()
            ),
        ));
        checks.push((
            "migration checksums".into(),
            maintenance.migration_checksums == nexus_core::store::MIGRATION_COUNT as u64,
            format!("{} verified", maintenance.migration_checksums),
        ));
        checks.push((
            "WAL".into(),
            maintenance.journal_mode.eq_ignore_ascii_case("wal"),
            format!(
                "{}; {} / {} frames checkpointed",
                maintenance.journal_mode,
                maintenance.wal_checkpointed_frames,
                maintenance.wal_log_frames
            ),
        ));
        checks.push((
            "release metadata".into(),
            brand::VERSION == env!("CARGO_PKG_VERSION")
                && brand::BUILD_TARGET == "x86_64-unknown-linux-gnu",
            format!(
                "{} · {} · {} · commit {} · epoch {}",
                brand::VERSION,
                brand::BUILD_TARGET,
                brand::BUILD_PROFILE,
                brand::BUILD_COMMIT,
                brand::BUILD_EPOCH
            ),
        ));
        checks.push((
            "automatic terminal isolation".into(),
            app.sandbox.strong_isolation(),
            if app.sandbox.strong_isolation() {
                "strong container isolation available".into()
            } else {
                "approval-only host fallback; unattended terminal execution is denied".into()
            },
        ));
        checks.push(binary_integrity_check()?);
    }

    if json {
        let v: Vec<_> = checks
            .iter()
            .map(
                |(name, ok, detail)| serde_json::json!({"check": name, "ok": ok, "detail": detail}),
            )
            .collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }

    ui.banner();
    for (name, ok, detail) in &checks {
        let mark = if *ok { ui.green("✓") } else { ui.yellow("!") };
        println!("  {} {:<22} {}", mark, ui.bold(name), ui.safe(detail));
    }
    println!("\n  {} {}", ui.dim("brand"), ui.cyan(brand::MARK));
    Ok(())
}

pub async fn maintenance(app: &App, command: MaintenanceCmd, json: bool) -> Result<()> {
    match command {
        MaintenanceCmd::Check => {
            let report = nexus_core::maintenance::check(
                &app.store,
                &app.paths.state_dir,
                &app.artifacts,
                &[app.paths.state_dir.clone(), app.paths.global_dir.clone()],
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                render_maintenance_report(&ui(app), &report);
            }
            if !report.ok {
                return Err(anyhow!("maintenance check found integrity issues"));
            }
        }
        MaintenanceCmd::Backup { directory } => {
            let manifest = nexus_core::maintenance::backup(
                &app.store,
                &app.paths.state_dir,
                std::path::Path::new(&directory),
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&manifest)?);
            } else {
                let ui = ui(app);
                ui.ok(&format!("backup created → {directory}"));
                ui.field("files", &manifest.files.len().to_string());
                ui.field("database", &manifest.database);
                ui.field("created", &manifest.created_at);
            }
        }
        MaintenanceCmd::Optimize { vacuum } => {
            nexus_core::maintenance::optimize(&app.store, vacuum)?;
            let report = nexus_core::maintenance::check(
                &app.store,
                &app.paths.state_dir,
                &app.artifacts,
                &[app.paths.state_dir.clone(), app.paths.global_dir.clone()],
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                render_maintenance_report(&ui(app), &report);
                ui(app).ok(if vacuum {
                    "optimize, checkpoint, and vacuum completed"
                } else {
                    "optimize and checkpoint completed"
                });
            }
        }
    }
    Ok(())
}

fn render_maintenance_report(ui: &Ui, report: &nexus_core::maintenance::MaintenanceReport) {
    ui.header("maintenance");
    ui.field("integrity", &report.database_integrity);
    ui.field("journal", &report.journal_mode);
    ui.field(
        "WAL",
        &format!(
            "{} / {} frames checkpointed",
            report.wal_checkpointed_frames, report.wal_log_frames
        ),
    );
    ui.field("state bytes", &report.state_bytes.to_string());
    ui.field(
        "artifacts",
        &format!(
            "{} files / {} bytes / {} issue(s)",
            report.artifact_count,
            report.artifact_bytes,
            report.artifact_issues.len()
        ),
    );
    ui.field(
        "permissions",
        &format!("{} issue(s)", report.permission_issues.len()),
    );
    if report.ok {
        ui.ok("state, database, permissions, WAL, and artifacts are consistent");
    } else {
        ui.warn("maintenance issues require attention");
    }
}

fn binary_integrity_check() -> Result<(String, bool, String)> {
    use sha2::Digest;
    let executable = std::env::current_exe()?;
    let canonical = executable.canonicalize()?;
    let metadata = std::fs::symlink_metadata(&canonical)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok((
            "binary integrity".into(),
            false,
            format!("{} is not a regular no-follow file", canonical.display()),
        ));
    }
    let bytes = std::fs::read(&canonical)?;
    let digest = hex::encode(sha2::Sha256::digest(&bytes));
    #[cfg(unix)]
    let ownership = {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        format!(
            "mode {:o}, uid {}, gid {}",
            metadata.permissions().mode() & 0o777,
            metadata.uid(),
            metadata.gid()
        )
    };
    #[cfg(not(unix))]
    let ownership = "platform ownership metadata unavailable".to_string();
    Ok((
        "binary integrity".into(),
        true,
        format!(
            "{} · sha256 {} · {}",
            canonical.display(),
            digest,
            ownership
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_config_enables_codex_when_session_is_available() {
        let toml = build_config_toml(&[], &nexus_core::gpu::GpuReport::default(), true);

        assert!(toml.contains("[models.codex]"));
        assert!(toml.contains("provider = \"codex\""));

        let cfg: nexus_core::config::Config = toml::from_str(&toml).expect("config parses");
        cfg.validate().expect("generated config is valid");
        assert_eq!(cfg.routing.fallback.as_deref(), Some("codex"));
        let model = cfg.models.get("codex").expect("codex model");
        assert_eq!(model.provider, "codex");
        assert!(
            model.base_url.is_empty(),
            "backend is implied by the provider, never api.openai.com"
        );
    }

    #[test]
    fn generated_config_keeps_codex_commented_when_session_is_missing() {
        let toml = build_config_toml(&[], &nexus_core::gpu::GpuReport::default(), false);

        assert!(!toml.contains("\n[models.codex]\n"));

        let cfg: nexus_core::config::Config = toml::from_str(&toml).expect("config parses");
        cfg.validate().expect("generated config is valid");
        assert!(cfg.models.is_empty());
    }
}
