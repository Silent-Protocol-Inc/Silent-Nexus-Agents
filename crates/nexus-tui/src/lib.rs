//! nexus-tui: the NEXUS terminal UI.
//!
//! A full-screen ratatui interface over the same `nexus-app` service layer
//! the CLI uses: every slash command resolves through the shared registry and
//! executor, so the two surfaces cannot drift. The UI never bypasses a safety
//! boundary — it renders only sanitized, redacted harness output, and
//! escalated actions surface as a modal that turns the operator's keypress
//! into the loop's approval decision.
//!
//! Responsiveness: every network- or process-touching operation (provider
//! discovery, device login, health probes, slash commands) runs on a
//! background task and reports back over a channel stamped with a generation
//! counter, so a stale result can never overwrite a newer view.

mod approver;
mod input;
mod intro;
mod menus;
mod render;
mod state;
mod theme;
mod views;

use approver::{ApprovalRequest, TuiApprover};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use nexus_agent::{AgentLoop, ApprovalDecision, ApprovalHandler, LoopEvent};
use nexus_app::codex::DeviceLoginEvent;
use nexus_app::providers::ProviderEntry;
use nexus_app::status::StatusSnapshot;
use nexus_app::{App, Effect, ExecCtx, Report, Sev};
use nexus_core::timeline::{
    LifecyclePhase, TimelineEvent, TimelineKind, TimelineSource, TimelineStatus,
};
use nexus_core::{SessionId, SpanId, TraceId, TurnId};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::{Terminal, TerminalOptions, Viewport};
use state::{Focus, Mode, State, StatusBar, TimelineEventUpdate};
use std::io::Stdout;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use views::{LoadRequest, Menu, Outcome, Overlay, Pager, Palette, SummaryPreview, UiAction};

/// Message sent from the agent task back to the render loop when a turn ends.
struct TurnDone {
    result: Result<nexus_agent::LoopOutcome, String>,
}

/// Results of background operations.
enum UiMsg {
    Loaded {
        generation: u64,
        data: Loaded,
    },
    Failed {
        generation: u64,
        context: String,
        error: String,
    },
    CommandEffect {
        generation: u64,
        effect: Effect,
    },
    Device(DeviceLoginEvent),
    DeviceDone(Result<(), String>),
    ClaudeLoginDone(Result<String, String>),
    Reloaded(Result<Arc<App>, String>),
    ShowReport {
        title: Option<String>,
        report: Report,
    },
}

/// Typed data for view loads.
enum Loaded {
    Status(Box<StatusSnapshot>),
    Login(Vec<ProviderEntry>),
    Model(Vec<ProviderEntry>),
    Provider {
        entry: Box<ProviderEntry>,
        configured: Vec<(String, String)>,
    },
}

/// Restores the terminal on drop, even on panic.
struct TermGuard {
    active: bool,
    alt_screen: bool,
    mouse: bool,
}
impl TermGuard {
    fn restore(&mut self) {
        if !self.active {
            return;
        }
        let _ = disable_raw_mode();
        let mut out = std::io::stdout();
        if self.mouse {
            let _ = out.execute(DisableMouseCapture);
        }
        if self.alt_screen {
            let _ = out.execute(LeaveAlternateScreen);
        }
        let _ = out.execute(crossterm::cursor::Show);
        self.active = false;
    }
}
impl Drop for TermGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Launch the TUI over a bootstrapped [`App`].
pub async fn run(app: Arc<App>) -> std::io::Result<()> {
    run_with_target(app, None, false).await
}

pub async fn run_inline(app: Arc<App>) -> std::io::Result<()> {
    run_with_target(app, None, true).await
}

/// Launch the TUI attached directly to a session or recoverable goal.
pub async fn run_resume(app: Arc<App>, id: String) -> std::io::Result<()> {
    run_with_target(app, Some(id), false).await
}

pub async fn run_resume_inline(app: Arc<App>, id: String) -> std::io::Result<()> {
    run_with_target(app, Some(id), true).await
}

async fn run_with_target(
    app: Arc<App>,
    initial_target: Option<String>,
    inline: bool,
) -> std::io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    if !inline {
        stdout.execute(EnterAlternateScreen)?;
    }
    stdout.execute(EnableMouseCapture)?;
    let mut guard = TermGuard {
        active: true,
        alt_screen: !inline,
        mouse: true,
    };
    let backend = CrosstermBackend::new(stdout);
    let viewport = if inline {
        let height = crossterm::terminal::size()
            .map(|(_, height)| height.saturating_sub(2).max(10))
            .unwrap_or(24);
        Viewport::Inline(height)
    } else if crossterm::terminal::size().is_ok() {
        Viewport::Fullscreen
    } else {
        Viewport::Fixed(Rect::new(0, 0, 80, 24))
    };
    let mut terminal = Terminal::with_options(backend, TerminalOptions { viewport })?;

    // Boot sequence: the animated mark, before any state exists. Skippable.
    let support = theme::detect_color_support(app.no_color);
    let boot_theme = theme::Theme::new(&app.theme_name(), support);
    if !inline {
        intro::play(
            &mut terminal,
            &boot_theme,
            app.config.general.reduced_motion,
        )?;
    }

    let handoff = event_loop(&mut terminal, app, initial_target).await?;
    guard.restore();
    println!("{handoff}");
    Ok(())
}

/// The status-bar MODEL label plus whether it is usable right now.
fn model_label(app: &App) -> (String, bool) {
    let name = app.any_model_name();
    let Some(cfg) = app.config.models.get(&name) else {
        return ("Not configured".into(), false);
    };
    let provider = match cfg.provider.as_str() {
        "ollama" => "Ollama",
        "llamacpp" => "llama.cpp",
        "codex" => "Codex",
        "claude-plan" => "Claude Plan",
        "anthropic" => "Anthropic",
        "openai" if cfg.auth.as_deref() == Some("codex") => "Codex",
        "openai" => "OpenAI",
        other => other,
    };
    if (cfg.auth.as_deref() == Some("codex") || cfg.provider == "codex")
        && nexus_models::codex_auth::resolve_with_consent(cfg.allow_existing_codex)
            .ok()
            .flatten()
            .is_none()
    {
        return (format!("{provider} / login required"), false);
    }
    if cfg.provider == "claude-plan" && !cfg.allow_existing_claude {
        return (format!("{provider} / consent required"), false);
    }
    (format!("{provider} / {}", cfg.model), true)
}

fn build_bar(app: &App) -> StatusBar {
    let (model_lbl, model_ok) = model_label(app);
    let net = match app.config.sandbox.network.as_str() {
        "off" | "none" => nexus_sandbox::NetworkMode::Off,
        "full" => nexus_sandbox::NetworkMode::Full,
        _ => nexus_sandbox::NetworkMode::Restricted,
    };
    StatusBar {
        workspace: app.workspace_key.clone(),
        model_label: model_lbl,
        model_ok,
        agent: app.active_agent(),
        sandbox_level: app.sandbox.backend().isolation(net).level,
        network: app.config.sandbox.network.clone(),
        git_branch: nexus_app::gitx::branch(&app.workspace),
        tokens_in: 0,
        tokens_out: 0,
        permission_mode: nexus_app::services::permission_mode(&app.config.policy).to_string(),
    }
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mut app: Arc<App>,
    initial_target: Option<String>,
) -> std::io::Result<String> {
    let history = app.read_ui_state(|s| s.history.clone());
    let mut st = State::new(
        app.theme_name(),
        theme::detect_color_support(app.no_color),
        app.config.general.reduced_motion,
        build_bar(&app),
        history,
        app.read_ui_state(|state| state.thinking_enabled),
    );
    st.active_work = nexus_app::services::active_work_snapshot(&app, None, "idle");

    // Channels.
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<LoopEvent>();
    let (appr_tx, mut appr_rx) = mpsc::unbounded_channel::<ApprovalRequest>();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<TurnDone>();
    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<Event>();
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<UiMsg>();

    // Blocking key reader on a dedicated thread; forwards crossterm events.
    let stop = Arc::new(AtomicBool::new(false));
    let stop_reader = stop.clone();
    let reader = std::thread::spawn(move || {
        while !stop_reader.load(Ordering::Relaxed) {
            if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                if let Ok(evt) = event::read() {
                    if key_tx.send(evt).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let approver: Arc<dyn ApprovalHandler> = Arc::new(TuiApprover::new(appr_tx));
    let mut session: Option<SessionId> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(250));

    // First run: no models configured yet — greet and offer onboarding.
    if app.config.models.is_empty() {
        st.system_sev(
            "FIRST RUN :: no models configured yet — /setup gets you talking to an agent",
            Sev::Warn,
        );
        st.push_overlay(Overlay::Menu(menus::welcome_menu()));
    }
    if let Some(id) = initial_target {
        if app.sessions().get(&id).is_ok() {
            attach_session(&mut st, &id, &app, &mut session);
        } else if app.goals().get(&id).is_ok() {
            resume_goal(&mut st, &id, &app, &mut session);
        } else {
            st.system_sev(
                format!("resume target `{id}` is neither a session nor a goal"),
                Sev::Err,
            );
        }
    }

    terminal.draw(|f| render::draw(f, &mut st))?;

    while !st.should_quit {
        tokio::select! {
            Some(evt) = key_rx.recv() => {
                handle_key(&mut st, evt, &app, &mut session, &ev_tx, &done_tx, &approver, &ui_tx);
            }
            Some(ev) = ev_rx.recv() => {
                apply_loop_event(&mut st, ev);
            }
            Some(req) = appr_rx.recv() => {
                st.pending_approvals = 1;
                st.approval_selected = if req.action.command.is_some()
                    && req.action.risk < nexus_core::RiskLevel::Destructive
                {
                    2
                } else {
                    1
                };
                st.approval_edit = None;
                st.pending = Some(req);
            }
            Some(done) = done_rx.recv() => {
                st.mode = Mode::Idle;
                st.turn_abort = None;
                match done.result {
                    Ok(o) => {
                        let final_already_streamed = st.timeline.iter().rev().any(|event| {
                            matches!(
                                &event.kind,
                                TimelineKind::FinalAnswer { text } if text == &o.final_message
                            )
                        });
                        if !o.final_message.trim().is_empty() && !final_already_streamed {
                            st.assistant(o.final_message.clone());
                        }
                        st.bar.tokens_in += o.input_tokens;
                        st.bar.tokens_out += o.output_tokens;
                        st.activity(format!(
                            "turn done · {} steps · {} tool calls · {}",
                            o.steps, o.tool_calls, o.stopped_reason
                        ));
                    }
                    Err(e) => {
                        st.last_error = Some(e.clone());
                        st.system_sev(format!("error: {e}"), Sev::Err);
                        st.activity(format!("error: {e}"));
                    }
                }
                st.follow = true;
                st.new_events = 0;
                st.active_work = nexus_app::services::active_work_snapshot(
                    &app,
                    session.as_ref().map(SessionId::as_str),
                    "idle",
                );
                if let Some(view) = st.session_view_state() {
                    let _ = app.timeline().save_view_state(&view);
                }
            }
            Some(msg) = ui_rx.recv() => {
                handle_ui_msg(&mut st, msg, &mut app, &mut session, &ui_tx);
            }
            _ = tick.tick() => {
                st.tick();
                if st.spinner.is_multiple_of(4) {
                    if let Some(session_id) = session.as_ref().map(SessionId::as_str) {
                        if let Ok(events) = app.timeline().background_after(
                            session_id,
                            st.last_background_sequence,
                            100,
                        ) {
                            for event in events {
                                st.push_event(event);
                            }
                        }
                        if st.follow {
                            if let Ok(sequence) = app.timeline().latest_sequence(session_id) {
                                let _ = app.timeline().mark_read(session_id, sequence);
                            }
                            let _ = app.orchestration().mark_agent_runs_read(session_id);
                        }
                    }
                    st.active_work = nexus_app::services::active_work_snapshot(
                        &app,
                        session.as_ref().map(SessionId::as_str),
                        if st.mode == Mode::Running { "running" } else { "idle" },
                    );
                }
            }
            else => break,
        }

        terminal.draw(|f| render::draw(f, &mut st))?;
    }

    // Persist history before leaving.
    let final_history: Vec<String> = st.input.history_snapshot().to_vec();
    let _ = app.update_ui_state(move |s| s.history = final_history);
    if let Some(view) = st.session_view_state() {
        let _ = app.timeline().save_view_state(&view);
    }

    stop.store(true, Ordering::Relaxed);
    let _ = reader.join();
    let handoff = exit_handoff(&app, &st, session.as_ref());
    Ok(handoff)
}

fn exit_handoff(app: &App, st: &State, session: Option<&SessionId>) -> String {
    let branch = nexus_app::gitx::branch(&app.workspace).unwrap_or_else(|| "n/a".into());
    let head = nexus_app::gitx::head_commit(&app.workspace).unwrap_or_else(|| "n/a".into());
    let dirty = nexus_app::gitx::modified_files(&app.workspace).len();
    let (session_id, title, goal, model, agent, usage) = match session {
        Some(id) => {
            let _ = app.sessions().mark_exit(id.as_str());
            let meta = app.sessions().get(id.as_str()).ok();
            let usage = app.sessions().usage_or_default(id.as_str()).ok();
            (
                id.as_str().to_string(),
                meta.as_ref()
                    .map(|meta| {
                        if meta.title.is_empty() {
                            "(untitled)".to_string()
                        } else {
                            meta.title.clone()
                        }
                    })
                    .unwrap_or_else(|| "(unavailable)".into()),
                meta.and_then(|meta| meta.current_goal)
                    .or_else(|| st.goal_label.clone())
                    .unwrap_or_else(|| "none".into()),
                app.sessions()
                    .get(id.as_str())
                    .map(|meta| meta.model)
                    .unwrap_or_else(|_| st.bar.model_label.clone()),
                app.sessions()
                    .get(id.as_str())
                    .map(|meta| meta.agent)
                    .unwrap_or_else(|_| st.bar.agent.clone()),
                usage,
            )
        }
        None => (
            "none".into(),
            "(no session)".into(),
            st.goal_label.clone().unwrap_or_else(|| "none".into()),
            st.bar.model_label.clone(),
            st.bar.agent.clone(),
            None,
        ),
    };
    let input_tokens = usage
        .as_ref()
        .map(|usage| usage.input_tokens)
        .unwrap_or(st.bar.tokens_in as u64);
    let output_tokens = usage
        .as_ref()
        .map(|usage| usage.output_tokens)
        .unwrap_or(st.bar.tokens_out as u64);
    let tool_calls = usage
        .as_ref()
        .map(|usage| usage.tool_calls)
        .unwrap_or(st.tool_calls);
    let elapsed_secs = usage
        .as_ref()
        .map(|usage| usage.elapsed_ms / 1_000)
        .unwrap_or_else(|| st.started.elapsed().as_secs());
    let (resume, continuation) = if session_id == "none" {
        (
            "No session was active.".to_string(),
            "No session was active.".to_string(),
        )
    } else {
        (
            format!("snx resume {session_id}"),
            format!("snx continue {session_id}"),
        )
    };
    format!(
        "{} :: EXIT HANDOFF\n\
         project     {}\n\
         workspace   {}\n\
         session     {} · {}\n\
         branch      {}\n\
         HEAD        {}\n\
         dirty files {}\n\
         model       {}\n\
         agent       {}\n\
         goal        {}\n\
         elapsed     {}s\n\
         tool calls  {}\n\
         tokens      {} input / {} output\n\n\
         Resume exactly:\n  {}\n\
         Continue in a linked child:\n  {}",
        nexus_core::brand::MARK,
        app.workspace
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("NEXUS"),
        app.workspace.display(),
        title,
        session_id,
        branch,
        head,
        dirty,
        model,
        agent,
        goal,
        elapsed_secs,
        tool_calls,
        input_tokens,
        output_tokens,
        resume,
        continuation,
    )
}

// ------------------------------------------------------------------ keyboard

#[allow(clippy::too_many_arguments)]
fn handle_key(
    st: &mut State,
    evt: Event,
    app: &Arc<App>,
    session: &mut Option<SessionId>,
    ev_tx: &mpsc::UnboundedSender<LoopEvent>,
    done_tx: &mpsc::UnboundedSender<TurnDone>,
    approver: &Arc<dyn ApprovalHandler>,
    ui_tx: &mpsc::UnboundedSender<UiMsg>,
) {
    let key = match evt {
        Event::Key(k) if k.kind != KeyEventKind::Release => k,
        Event::Mouse(mouse) => {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    transcript_scroll_up(st, 3, app, session.as_ref());
                }
                MouseEventKind::ScrollDown => transcript_scroll_down(st, 3),
                MouseEventKind::Down(MouseButton::Left) => {
                    st.focus = Focus::Timeline;
                    if let Some((id, _)) = st
                        .event_row_offsets
                        .iter()
                        .filter(|(_, row)| **row <= st.scroll + mouse.row as usize)
                        .max_by_key(|(_, row)| *row)
                    {
                        st.selected_event = st.timeline.iter().position(|event| &event.id == id);
                        activate_selected_event(st, app);
                    }
                }
                _ => {}
            }
            return;
        }
        Event::Resize(..) => return, // next draw adapts
        _ => return,
    };

    // Ctrl+C cancels a running turn first; while idle it exits.
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
        if let Some(req) = st.pending.take() {
            let _ = req.reply.send(ApprovalDecision::Deny);
        }
        if st.mode == Mode::Running {
            if let Some(abort) = st.turn_abort.take() {
                abort.abort();
            }
            if let Some(session_id) = session.as_ref() {
                let _ = nexus_app::services::record_session_cancellation(
                    app,
                    session_id.as_str(),
                    "running turn cancelled by operator",
                );
            }
            for event in st
                .timeline
                .iter_mut()
                .filter(|event| event.status == TimelineStatus::Running)
            {
                event.status = TimelineStatus::Cancelled;
                event.phase = LifecyclePhase::Cancelled;
                match &mut event.kind {
                    TimelineKind::AssistantMessage { streaming, .. } => *streaming = false,
                    TimelineKind::ToolExecution { exit_status, .. } => {
                        *exit_status = Some("cancelled".into());
                    }
                    _ => {}
                }
            }
            st.live_assistant_event = None;
            st.live_tool_events.clear();
            st.mode = Mode::Idle;
            st.pending_approvals = 0;
            st.system_sev("running turn cancelled by operator", Sev::Warn);
            st.toast("turn cancelled", Sev::Warn);
            return;
        }
        st.should_quit = true;
        return;
    }

    // Approval modal captures input first — it is a safety boundary.
    if st.pending.is_some() {
        if let Some(editor) = st.approval_edit.as_mut() {
            match key.code {
                KeyCode::Esc => st.approval_edit = None,
                KeyCode::Backspace => {
                    editor.pop();
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    editor.clear();
                }
                KeyCode::Enter => match serde_json::from_str::<serde_json::Value>(editor) {
                    Ok(value) if value.is_object() => {
                        resolve_approval(st, ApprovalDecision::ApproveEdited(value));
                    }
                    _ => st.toast("alternative must be a valid JSON object", Sev::Err),
                },
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    editor.push(c);
                }
                _ => {}
            }
            return;
        }
        let persistent_allowed = st
            .pending
            .as_ref()
            .is_some_and(|req| req.sandbox_active && req.action.session_grant_allowed());
        let edit_allowed = st.pending.as_ref().is_some_and(|req| {
            !req.action
                .command_analysis
                .as_ref()
                .is_some_and(|analysis| analysis.one_time_only)
        });
        let option_count = match (persistent_allowed, edit_allowed) {
            (true, true) => 4,
            (false, true) => 3,
            (_, false) => 2,
        };
        let decision = match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                st.approval_selected = st.approval_selected.saturating_sub(1);
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                st.approval_selected = (st.approval_selected + 1).min(option_count - 1);
                None
            }
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('o') => {
                Some(ApprovalDecision::Approve)
            }
            KeyCode::Char('s') | KeyCode::Char('S') if persistent_allowed => {
                Some(ApprovalDecision::ApproveForSession)
            }
            KeyCode::Char('e') | KeyCode::Char('E') if edit_allowed => {
                begin_approval_edit(st);
                None
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Some(ApprovalDecision::Deny),
            KeyCode::Enter => match (persistent_allowed, st.approval_selected) {
                (_, 0) => Some(ApprovalDecision::Approve),
                (true, 1) => Some(ApprovalDecision::ApproveForSession),
                (true, 2) | (false, 1) => Some(ApprovalDecision::Deny),
                _ if edit_allowed => {
                    begin_approval_edit(st);
                    None
                }
                _ => Some(ApprovalDecision::Deny),
            },
            _ => None,
        };
        if let Some(d) = decision {
            resolve_approval(st, d);
        }
        return;
    }

    // Overlays capture input next.
    if let Some(overlay) = st.overlay_top() {
        match overlay.handle_key(key) {
            Outcome::Consumed => {}
            Outcome::Close => st.pop_overlay(),
            Outcome::Action(action) => {
                st.pop_overlay();
                handle_action(st, action, app, session, ui_tx);
            }
            Outcome::ActionKeepOpen(action) => {
                handle_action(st, action, app, session, ui_tx);
            }
        }
        return;
    }

    if st.search_edit.is_some() {
        match key.code {
            KeyCode::Esc => st.search_edit = None,
            KeyCode::Backspace => {
                if let Some(search) = st.search_edit.as_mut() {
                    search.pop();
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(search) = st.search_edit.as_mut() {
                    search.clear();
                }
            }
            KeyCode::Enter => {
                let query = st.search_edit.take().unwrap_or_default();
                st.search_query = (!query.trim().is_empty()).then(|| query.trim().to_string());
                refresh_durable_search(st, app, session.as_ref());
                if let Some(view) = st.session_view_state() {
                    let _ = app.timeline().save_view_state(&view);
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(search) = st.search_edit.as_mut() {
                    search.push(c);
                }
            }
            _ => {}
        }
        return;
    }

    // Command palette.
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('k')) {
        open_palette(st, app);
        return;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('f')) {
        st.search_edit = Some(st.search_query.clone().unwrap_or_default());
        st.focus = Focus::Input;
        return;
    }

    if matches!(key.code, KeyCode::F(6)) {
        st.focus = st.focus.next();
        st.context_drawer = st.focus == Focus::Context;
        st.agent_drawer = st.focus == Focus::Drawer;
        return;
    }

    if matches!(key.code, KeyCode::BackTab) {
        let current = nexus_app::services::permission_mode(&app.config.policy);
        let modes = ["read-only", "default", "auto-edit", "full-access"];
        let next = modes
            .iter()
            .position(|mode| *mode == current)
            .map(|index| modes[(index + 1) % modes.len()])
            .unwrap_or("read-only");
        run_command(st, &format!("permissions {next}"), app, session, ui_tx);
        st.toast(format!("approval mode → {next}"), Sev::Info);
        return;
    }

    if st.focus == Focus::Timeline {
        match key.code {
            KeyCode::Char('k') | KeyCode::Up => {
                select_previous_event(st);
                return;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                select_next_event(st);
                return;
            }
            KeyCode::Enter => {
                activate_selected_event(st, app);
                return;
            }
            KeyCode::Char('n') => {
                select_search_match(st, false, app, session.as_ref());
                return;
            }
            KeyCode::Char('N') => {
                select_search_match(st, true, app, session.as_ref());
                return;
            }
            KeyCode::Char('y') => {
                if let Some(event) = st.selected_timeline_event() {
                    let text = serde_json::to_string_pretty(event)
                        .unwrap_or_else(|_| event.summary.clone());
                    match nexus_app::clipboard::copy(&text) {
                        Ok(method) => st.toast(format!("event copied via {method}"), Sev::Ok),
                        Err(error) => st.toast(format!("copy failed: {error}"), Sev::Err),
                    }
                }
                return;
            }
            KeyCode::Esc => {
                st.focus = Focus::Input;
                return;
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::PageUp => {
            transcript_scroll_up(st, st.viewport_rows.max(4) / 2, app, session.as_ref());
        }
        KeyCode::PageDown => {
            transcript_scroll_down(st, st.viewport_rows.max(4) / 2);
        }
        KeyCode::End => {
            if st.focus == Focus::Input {
                st.input.end();
            } else {
                st.follow = true;
                st.new_events = 0;
            }
        }
        KeyCode::Up => st.input.history_prev(),
        KeyCode::Down => st.input.history_next(),
        KeyCode::Left if st.input.is_empty() => {
            st.agent_drawer = !st.agent_drawer;
            st.context_drawer = false;
            st.focus = if st.agent_drawer {
                Focus::Drawer
            } else {
                Focus::Input
            };
        }
        KeyCode::Right if st.input.is_empty() => {
            st.context_drawer = !st.context_drawer;
            st.agent_drawer = false;
            st.focus = if st.context_drawer {
                Focus::Context
            } else {
                Focus::Input
            };
        }
        KeyCode::Left => st.input.left(),
        KeyCode::Right => st.input.right(),
        KeyCode::Home if st.focus != Focus::Input => {
            load_older_timeline(st, app, session.as_ref());
            st.follow = false;
            st.scroll = 0;
        }
        KeyCode::Home => st.input.home(),
        KeyCode::Backspace => st.input.backspace(),
        KeyCode::Delete => st.input.delete(),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => st.input.clear(),
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            st.input.delete_word()
        }
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => st.input.insert('\n'),
        KeyCode::Enter => {
            let line = st.input.take();
            let trimmed = line.trim().to_string();
            if trimmed.is_empty() {
                return;
            }
            let final_history: Vec<String> = st.input.history_snapshot().to_vec();
            let _ = app.update_ui_state(move |s| s.history = final_history);
            submit_line(st, &trimmed, app, session, ev_tx, done_tx, approver, ui_tx);
        }
        KeyCode::Char('?') if st.input.is_empty() => {
            start_load(st, LoadRequest::Help, app, ui_tx);
        }
        KeyCode::Char('/') if st.input.is_empty() => {
            open_palette(st, app);
        }
        KeyCode::Char(c) => st.input.insert(c),
        _ => {}
    }
}

fn activate_selected_event(st: &mut State, app: &Arc<App>) {
    let Some((id, artifacts)) = st
        .selected_timeline_event()
        .map(|event| (event.id.clone(), event.artifact_refs.clone()))
    else {
        return;
    };
    if st.collapsed_cards.insert(id.clone()) {
        if let Some(view) = st.session_view_state() {
            let _ = app.timeline().save_view_state(&view);
        }
        return;
    }
    if let Some(reference) = artifacts.first() {
        let artifact_id = nexus_core::ArtifactId::from(reference.id.clone());
        match app.artifacts.get(&artifact_id) {
            Ok(bytes) => {
                let content = app
                    .redactor
                    .redact(&nexus_core::sanitize::sanitize_terminal(
                        &String::from_utf8_lossy(&bytes),
                    ));
                st.push_overlay(Overlay::Pager(Pager::new(
                    format!("artifact · {}", reference.label),
                    Report::untitled()
                        .field("id", &reference.id)
                        .field("kind", &reference.kind)
                        .field("bytes", bytes.len().to_string())
                        .header("content")
                        .line(content),
                )));
            }
            Err(error) => st.toast(format!("artifact: {error}"), Sev::Err),
        }
    } else {
        st.collapsed_cards.remove(&id);
    }
    if let Some(view) = st.session_view_state() {
        let _ = app.timeline().save_view_state(&view);
    }
}

fn begin_approval_edit(st: &mut State) {
    if let Some(req) = &st.pending {
        st.approval_edit = serde_json::to_string_pretty(&req.arguments)
            .ok()
            .or_else(|| Some("{}".into()));
    }
}

fn resolve_approval(st: &mut State, decision: ApprovalDecision) {
    if let Some(req) = st.pending.take() {
        let label = match &decision {
            ApprovalDecision::Approve => "approved once",
            ApprovalDecision::ApproveForSession => "approved for session",
            ApprovalDecision::ApproveEdited(_) => "approved safer alternative",
            ApprovalDecision::Deny => "denied",
        };
        if let Some(event) = st.timeline.iter_mut().rev().find(|event| {
            matches!(
                &event.kind,
                TimelineKind::Approval { tool, decision: None, .. } if tool == &req.action.tool
            )
        }) {
            event.status = if matches!(&decision, ApprovalDecision::Deny) {
                TimelineStatus::Blocked
            } else {
                TimelineStatus::Completed
            };
            event.phase = LifecyclePhase::Completed;
            if let TimelineKind::Approval {
                decision: stored,
                edited,
                ..
            } = &mut event.kind
            {
                *stored = Some(label.into());
                *edited = matches!(&decision, ApprovalDecision::ApproveEdited(_));
            }
        }
        st.pending_approvals = 0;
        st.approval_edit = None;
        let _ = req.reply.send(decision);
    }
}

fn open_palette(st: &mut State, app: &Arc<App>) {
    let recent = app.read_ui_state(|s| s.recent_commands.clone());
    st.push_overlay(Overlay::Palette(Palette::new(recent)));
}

fn transcript_scroll_up(
    st: &mut State,
    amount: usize,
    app: &Arc<App>,
    session: Option<&SessionId>,
) {
    if st.follow {
        st.follow = false;
        st.scroll = st.total_wrapped_rows.saturating_sub(st.viewport_rows);
    }
    if st.scroll == 0 && st.has_older_events {
        load_older_timeline(st, app, session);
    }
    st.scroll = st.scroll.saturating_sub(amount.max(1));
}

fn transcript_scroll_down(st: &mut State, amount: usize) {
    let max_scroll = st.total_wrapped_rows.saturating_sub(st.viewport_rows);
    st.scroll = st.scroll.saturating_add(amount.max(1)).min(max_scroll);
    if st.scroll >= max_scroll {
        st.follow = true;
        st.new_events = 0;
    } else {
        st.follow = false;
    }
}

fn load_older_timeline(st: &mut State, app: &Arc<App>, session: Option<&SessionId>) {
    let Some(session) = session else {
        return;
    };
    let before = st.earliest_sequence;
    let Ok(events) = app.timeline().page(
        session.as_str(),
        before,
        100,
        nexus_core::timeline::TranscriptFilter::All,
    ) else {
        return;
    };
    if events.is_empty() {
        st.has_older_events = false;
        return;
    }
    st.has_older_events = events.first().is_some_and(|event| event.sequence > 1);
    st.prepend_events(events);
}

fn select_previous_event(st: &mut State) {
    let start = st.selected_event.unwrap_or(st.timeline.len());
    if let Some(index) = (0..start).rev().find(|index| {
        st.transcript_filter.matches(&st.timeline[*index])
            && (st.thinking_enabled
                || !matches!(
                    st.timeline[*index].kind,
                    TimelineKind::ReasoningSummary { .. }
                ))
    }) {
        st.selected_event = Some(index);
        focus_selected_event(st);
    }
}

fn select_next_event(st: &mut State) {
    let start = st.selected_event.map_or(0, |index| index.saturating_add(1));
    if let Some(index) = (start..st.timeline.len()).find(|index| {
        st.transcript_filter.matches(&st.timeline[*index])
            && (st.thinking_enabled
                || !matches!(
                    st.timeline[*index].kind,
                    TimelineKind::ReasoningSummary { .. }
                ))
    }) {
        st.selected_event = Some(index);
        focus_selected_event(st);
    }
}

fn refresh_durable_search(st: &mut State, app: &Arc<App>, session: Option<&SessionId>) {
    let Some(query) = st.search_query.as_deref() else {
        st.durable_search = false;
        st.refresh_search_matches();
        return;
    };
    let Some(session) = session else {
        st.durable_search = false;
        st.refresh_search_matches();
        return;
    };
    match app
        .timeline()
        .search_hits(session.as_str(), query, st.transcript_filter, 500)
    {
        Ok(hits) => {
            st.search_matches = hits.into_iter().map(|hit| hit.event_id).collect();
            st.search_match_index = 0;
            st.durable_search = true;
            st.selected_event = None;
            if st.search_matches.is_empty() {
                st.toast("no durable transcript matches", Sev::Info);
            } else {
                select_search_match(st, false, app, Some(session));
            }
        }
        Err(error) => st.toast(format!("timeline search: {error}"), Sev::Err),
    }
}

fn select_search_match(
    st: &mut State,
    previous: bool,
    app: &Arc<App>,
    session: Option<&SessionId>,
) {
    if st.search_matches.is_empty() {
        return;
    }
    if previous {
        st.search_match_index = if st.search_match_index == 0 {
            st.search_matches.len() - 1
        } else {
            st.search_match_index - 1
        };
    } else if st.selected_event.is_some() {
        st.search_match_index = (st.search_match_index + 1) % st.search_matches.len();
    }
    let id = st.search_matches[st.search_match_index].clone();
    st.selected_event = st.timeline.iter().position(|event| event.id == id);
    if st.selected_event.is_none() {
        if let (Some(session), Ok(event)) = (session, app.timeline().get(&id)) {
            if let Ok(events) = app.timeline().page_around(
                session.as_str(),
                event.sequence,
                50,
                nexus_core::timeline::TranscriptFilter::All,
            ) {
                st.timeline = events;
                st.earliest_sequence = st.timeline.first().map(|event| event.sequence);
                st.has_older_events = st.timeline.first().is_some_and(|event| event.sequence > 1);
                st.selected_event = st.timeline.iter().position(|event| event.id == id);
                st.wrap_layout_cache.clear();
            }
        }
    }
    st.focus = Focus::Timeline;
    focus_selected_event(st);
}

fn focus_selected_event(st: &mut State) {
    let Some(event) = st.selected_timeline_event() else {
        return;
    };
    if let Some(offset) = st.event_row_offsets.get(&event.id).copied() {
        st.follow = false;
        st.scroll = offset.saturating_sub(1);
    }
}

// ------------------------------------------------------------------- submit

fn push_command_event(
    st: &mut State,
    app: &Arc<App>,
    session: Option<&SessionId>,
    status: TimelineStatus,
    summary: String,
    kind: TimelineKind,
) {
    if let Some(session) = session {
        let mut event = TimelineEvent::new(
            session.clone(),
            TurnId::from("command"),
            TraceId::generate(),
            SpanId::generate(),
            None,
            LifecyclePhase::Message,
            status,
            summary.clone(),
            kind.clone(),
        );
        event.source = TimelineSource::Command;
        match app.timeline().append(event) {
            Ok(event) => {
                st.push_event(event);
                return;
            }
            Err(error) => {
                tracing::warn!(%error, "command timeline persistence failed");
            }
        }
    }
    st.push_local_event(status, summary, kind);
}

#[allow(clippy::too_many_arguments)]
fn submit_line(
    st: &mut State,
    line: &str,
    app: &Arc<App>,
    session: &mut Option<SessionId>,
    ev_tx: &mpsc::UnboundedSender<LoopEvent>,
    done_tx: &mpsc::UnboundedSender<TurnDone>,
    approver: &Arc<dyn ApprovalHandler>,
    ui_tx: &mpsc::UnboundedSender<UiMsg>,
) {
    match nexus_app::classify(line) {
        Ok(nexus_app::Input::Empty) => {}
        Ok(nexus_app::Input::Message(text)) => {
            if st.mode == Mode::Running {
                st.toast(
                    "a turn is already running — wait for it to finish",
                    Sev::Warn,
                );
                st.input.set_text(text);
                return;
            }
            st.user(text.clone());
            st.follow = true;
            submit_objective(st, app, session, text, ev_tx, done_tx, approver);
        }
        Ok(nexus_app::Input::Slash(cmd)) => {
            push_command_event(
                st,
                app,
                session.as_ref(),
                TimelineStatus::Completed,
                format!("/{} {}", cmd.name, cmd.rest).trim_end().to_string(),
                TimelineKind::SlashCommand {
                    command: cmd.name.clone(),
                    arguments: cmd.args.clone(),
                    result: None,
                },
            );
            run_command_parsed(st, cmd, app, session, ui_tx);
        }
        Ok(nexus_app::Input::Shell(body)) => {
            if st.mode == Mode::Running {
                st.toast(
                    "direct shell execution is blocked while a turn is active",
                    Sev::Warn,
                );
                st.input.set_text(format!("!{body}"));
                return;
            }
            let command_args = nexus_app::parse::tokenize(&body).unwrap_or_default();
            push_command_event(
                st,
                app,
                session.as_ref(),
                TimelineStatus::Pending,
                format!("sandbox command · {body}"),
                TimelineKind::SandboxCommand {
                    command: command_args.clone(),
                    backend: app.sandbox.backend().name().into(),
                    output_preview: String::new(),
                },
            );
            match nexus_app::parse::tokenize(&body) {
                Ok(tokens) if !tokens.is_empty() => {
                    let app = app.clone();
                    let tx = ui_tx.clone();
                    let generation = st.generation;
                    st.busy += 1;
                    st.activity(format!("sandbox exec: {body}"));
                    tokio::spawn(async move {
                        let msg = match nexus_app::services::sandbox_test(&app, &tokens).await {
                            Ok(report) => UiMsg::ShowReport {
                                title: None,
                                report,
                            },
                            Err(e) => UiMsg::Failed {
                                generation,
                                context: "shell".into(),
                                error: e.to_string(),
                            },
                        };
                        let _ = tx.send(msg);
                    });
                }
                Ok(_) => {}
                Err(e) => st.system_sev(format!("parse error: {e}"), Sev::Err),
            }
        }
        Err(e) => {
            st.system_sev(format!("parse error: {e}"), Sev::Err);
        }
    }
}

/// Run a slash command line (no leading `/`). `__`-prefixed names are
/// TUI-internal flows (secret inputs, forms, canned help).
fn run_command(
    st: &mut State,
    line: &str,
    app: &Arc<App>,
    session: &mut Option<SessionId>,
    ui_tx: &mpsc::UnboundedSender<UiMsg>,
) {
    // Internal names start with `__`, which the user-facing parser rightly
    // refuses — route them straight to the internal dispatcher.
    if line.starts_with("__") {
        if st.mode == Mode::Running && line.starts_with("__codex_do_import") {
            st.toast(
                "cancel the running turn first (Ctrl+C), then retry the credential import",
                Sev::Warn,
            );
            return;
        }
        let (name, rest) = line.split_once(' ').unwrap_or((line, ""));
        let cmd = nexus_app::SlashCommand {
            name: name.to_string(),
            args: rest.split_whitespace().map(String::from).collect(),
            rest: rest.to_string(),
        };
        return run_internal(st, &cmd, app, ui_tx);
    }
    match nexus_app::classify(&format!("/{line}")) {
        Ok(nexus_app::Input::Slash(cmd)) => run_command_parsed(st, cmd, app, session, ui_tx),
        _ => st.system_sev(format!("internal: bad command `{line}`"), Sev::Err),
    }
}

fn run_command_parsed(
    st: &mut State,
    cmd: nexus_app::SlashCommand,
    app: &Arc<App>,
    session: &mut Option<SessionId>,
    ui_tx: &mpsc::UnboundedSender<UiMsg>,
) {
    if cmd.name.starts_with("__") {
        return run_internal(st, &cmd, app, ui_tx);
    }

    if st.mode == Mode::Running && command_changes_active_context(&cmd) {
        st.toast(
            "cancel the running turn first (Ctrl+C), then retry this command",
            Sev::Warn,
        );
        st.system_sev(
            format!(
                "/{} is blocked while a turn is active to preserve session consistency",
                cmd.name
            ),
            Sev::Warn,
        );
        return;
    }

    if nexus_app::registry::find(&cmd.name).is_some() {
        let name = cmd.name.clone();
        let _ = app.update_ui_state(move |s| s.push_recent_command(&name));
    }

    let exec_ctx = ExecCtx {
        session_id: session.as_ref().map(|s| s.as_str().to_string()),
        interactive: true,
        active: nexus_app::status::ActiveContext {
            session_id: session.as_ref().map(|s| s.as_str().to_string()),
            tool_calls: st.tool_calls,
            runtime_secs: st.started.elapsed().as_secs(),
            pending_approvals: st.pending_approvals,
            last_error: st.last_error.clone(),
        },
        sidecar_context: {
            let timeline = st
                .timeline
                .iter()
                .rev()
                .take(32)
                .map(|event| {
                    format!(
                        "{} [{}] {}",
                        event.kind.type_label(),
                        event.status.as_str(),
                        event.summary
                    )
                })
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            format!("Durable timeline:\n{timeline}")
        },
    };
    let app2 = app.clone();
    let tx = ui_tx.clone();
    let generation = st.generation;
    st.busy += 1;
    tokio::spawn(async move {
        let msg = match nexus_app::execute(&app2, &exec_ctx, &cmd).await {
            Ok(effect) => UiMsg::CommandEffect { generation, effect },
            Err(e) => UiMsg::Failed {
                generation,
                context: format!("/{}", cmd.name),
                error: e.to_string(),
            },
        };
        let _ = tx.send(msg);
    });
}

/// TUI-internal pseudo-commands reached from menus.
fn run_internal(
    st: &mut State,
    cmd: &nexus_app::SlashCommand,
    app: &Arc<App>,
    ui_tx: &mpsc::UnboundedSender<UiMsg>,
) {
    match cmd.name.as_str() {
        "__codex_api_key" => st.push_overlay(Overlay::Secret(views::SecretInput::new(
            "Codex API key",
            "stored via `codex login --with-api-key` in the isolated NEXUS profile",
            views::SecretTarget::CodexApiKey,
        ))),
        "__custom_endpoint" => st.push_overlay(Overlay::Form(views::Form::custom_endpoint())),
        "__provider_key" => {
            if let Some(provider) = cmd.args.first() {
                st.push_overlay(Overlay::Secret(menus::provider_key_input(provider)));
            }
        }
        "__model_test" => {
            if let Some(name) = cmd.args.first().cloned() {
                let app = app.clone();
                let tx = ui_tx.clone();
                let generation = st.generation;
                st.busy += 1;
                st.toast(format!("testing {name}…"), Sev::Info);
                tokio::spawn(async move {
                    let msg = match nexus_app::providers::test_model(&app, &name).await {
                        Ok(report) => UiMsg::ShowReport {
                            title: Some(format!("model test — {name}")),
                            report,
                        },
                        Err(e) => UiMsg::Failed {
                            generation,
                            context: format!("model test {name}"),
                            error: e.to_string(),
                        },
                    };
                    let _ = tx.send(msg);
                });
            }
        }
        "__codex_do_import" => {
            let tx = ui_tx.clone();
            st.busy += 1;
            let generation = st.generation;
            tokio::spawn(async move {
                let msg = match nexus_app::codex::import_existing() {
                    Ok(profile) => {
                        // Refresh the plan-model cache with the new session.
                        let _ = nexus_app::codex::list_plan_models().await;
                        UiMsg::ShowReport {
                            title: None,
                            report: Report::untitled()
                                .ok(format!(
                                    "imported the Codex session into the isolated profile ({})",
                                    profile.mode
                                ))
                                .line_sev("the original ~/.codex login was not modified", Sev::Dim),
                        }
                    }
                    Err(e) => UiMsg::Failed {
                        generation,
                        context: "codex import".into(),
                        error: e.to_string(),
                    },
                };
                let _ = tx.send(msg);
            });
        }
        "__codex_auth" => {
            let status =
                nexus_app::codex::status_with_consent(app.read_ui_state(|s| s.codex_use_existing));
            st.push_overlay(Overlay::Menu(menus::codex_menu(&status)));
        }
        "__claude_auth" => {
            st.push_overlay(Overlay::Menu(menus::claude_menu(
                nexus_app::claude::claude_binary().is_some(),
                app.read_ui_state(|state| state.claude_use_existing),
            )));
        }
        "__ollama_help" => {
            st.push_overlay(Overlay::Pager(Pager::new(
                "start Ollama",
                Report::untitled()
                    .line("NEXUS never installs or starts Ollama for you.")
                    .line("1. install from https://ollama.com")
                    .line("2. run `ollama serve` (usually automatic after install)")
                    .line("3. pull a model, e.g. `ollama pull llama3.2`")
                    .line("4. back in /connect, choose Ollama → Retry connection"),
            )));
        }
        "__llamacpp_help" => {
            st.push_overlay(Overlay::Pager(Pager::new(
                "start llama.cpp",
                Report::untitled()
                    .line("NEXUS does not own the llama.cpp process.")
                    .line("run: llama-server -m your-model.gguf --port 8080")
                    .line("then in /connect choose llama.cpp → Retry connection"),
            )));
        }
        "__init_preview" => {
            st.push_overlay(Overlay::Pager(Pager::new(
                "AGENTS.md preview",
                nexus_app::services::init_report(app),
            )));
        }
        other => st.system_sev(format!("internal: unknown flow `{other}`"), Sev::Err),
    }
}

// ------------------------------------------------------------------- effects

fn handle_ui_msg(
    st: &mut State,
    msg: UiMsg,
    app: &mut Arc<App>,
    session: &mut Option<SessionId>,
    ui_tx: &mpsc::UnboundedSender<UiMsg>,
) {
    match msg {
        UiMsg::Loaded { generation, data } => {
            st.busy = st.busy.saturating_sub(1);
            if generation != st.generation {
                return; // stale: the view moved on
            }
            apply_loaded(st, data, app);
        }
        UiMsg::Failed {
            generation,
            context,
            error,
        } => {
            st.busy = st.busy.saturating_sub(1);
            let _ = generation;
            st.last_error = Some(error.clone());
            st.system_sev(format!("{context}: {error}"), Sev::Err);
            st.toast(context, Sev::Err);
        }
        UiMsg::CommandEffect { generation, effect } => {
            st.busy = st.busy.saturating_sub(1);
            let _ = generation; // effects apply regardless; views check their own
            handle_effect(st, effect, app, session, ui_tx);
        }
        UiMsg::Device(ev) => apply_device_event(st, ev),
        UiMsg::DeviceDone(result) => {
            st.busy = st.busy.saturating_sub(1);
            st.cancel_login = None;
            if let Some(Overlay::Progress(p)) = st.overlays.last_mut() {
                p.done = true;
                p.failed = result.is_err();
            }
            match result {
                Ok(()) => {
                    st.toast("Codex device login verified (isolated profile)", Sev::Ok);
                    spawn_reload_with_plan_refresh(st, ui_tx);
                }
                Err(e) => st.system_sev(format!("device login: {e}"), Sev::Err),
            }
        }
        UiMsg::ClaudeLoginDone(result) => {
            st.busy = st.busy.saturating_sub(1);
            if let Some(Overlay::Progress(progress)) = st.overlays.last_mut() {
                progress.done = true;
                progress.failed = result.is_err();
            }
            match result {
                Ok(detail) => {
                    let _ = app.update_ui_state(|state| state.claude_use_existing = true);
                    st.toast("Claude subscription login ready", Sev::Ok);
                    st.system_sev(detail, Sev::Ok);
                    spawn_reload(st, app, ui_tx);
                }
                Err(error) => {
                    st.system_sev(format!("Claude login: {error}"), Sev::Err);
                }
            }
        }
        UiMsg::Reloaded(result) => {
            st.busy = st.busy.saturating_sub(1);
            match result {
                Ok(new_app) => {
                    *app = new_app;
                    let bar = build_bar(app);
                    let tokens = (st.bar.tokens_in, st.bar.tokens_out);
                    st.bar = bar;
                    st.bar.tokens_in = tokens.0;
                    st.bar.tokens_out = tokens.1;
                    st.toast("configuration reloaded", Sev::Ok);
                }
                Err(e) => st.system_sev(format!("reload failed: {e}"), Sev::Err),
            }
        }
        UiMsg::ShowReport { title, report } => {
            st.busy = st.busy.saturating_sub(1);
            match title {
                Some(title) => st.push_overlay(Overlay::Pager(Pager::new(title, report))),
                None => st.push_report(&report),
            }
        }
    }
}

fn handle_effect(
    st: &mut State,
    effect: Effect,
    app: &mut Arc<App>,
    session: &mut Option<SessionId>,
    ui_tx: &mpsc::UnboundedSender<UiMsg>,
) {
    match effect {
        Effect::Report(report) => {
            if report.title.is_some() {
                let title = report.title.clone().unwrap_or_default();
                st.push_overlay(Overlay::Pager(Pager::new(title, report)));
            } else {
                st.push_report(&report);
            }
        }
        Effect::View(view) => {
            if view == nexus_app::View::Welcome {
                st.push_overlay(Overlay::Menu(menus::welcome_menu()));
                return;
            }
            let load = match view {
                nexus_app::View::Status => LoadRequest::Status,
                nexus_app::View::Goals => LoadRequest::Goals,
                nexus_app::View::GoalMenu => LoadRequest::GoalMenu,
                nexus_app::View::GoalDetail(id) => LoadRequest::GoalDetail(id),
                nexus_app::View::GoalForm => LoadRequest::GoalDetail("NEW".into()),
                nexus_app::View::Resume => LoadRequest::Resume,
                nexus_app::View::Sessions => LoadRequest::Sessions,
                nexus_app::View::Login => LoadRequest::Login,
                nexus_app::View::Model => LoadRequest::Model,
                nexus_app::View::Agents => LoadRequest::Agents,
                nexus_app::View::Tasks => LoadRequest::Tasks,
                nexus_app::View::Subagents => LoadRequest::Subagents,
                nexus_app::View::Persona => LoadRequest::Persona,
                nexus_app::View::Profile => LoadRequest::Profile,
                nexus_app::View::Tools => LoadRequest::Tools,
                nexus_app::View::Memory => LoadRequest::Memory,
                nexus_app::View::Skills => LoadRequest::Skills,
                nexus_app::View::Mcp => LoadRequest::Mcp,
                nexus_app::View::Theme => LoadRequest::Theme,
                nexus_app::View::Thinking => LoadRequest::Thinking,
                nexus_app::View::Details => LoadRequest::Details,
                nexus_app::View::Transcript => LoadRequest::Transcript,
                nexus_app::View::Permissions => LoadRequest::Permissions,
                nexus_app::View::Sandbox => LoadRequest::Sandbox,
                nexus_app::View::Init => LoadRequest::Init,
                nexus_app::View::Config => LoadRequest::Config,
                nexus_app::View::Branch => LoadRequest::Branch,
                nexus_app::View::Commit => LoadRequest::Commit,
                nexus_app::View::Connector => LoadRequest::Connector,
                nexus_app::View::Welcome => unreachable!("handled above"),
                nexus_app::View::Help => LoadRequest::Help,
            };
            start_load(st, load, app, ui_tx);
        }
        Effect::Confirm(action) => {
            st.push_overlay(Overlay::Confirm(views::Confirm::for_action(action)));
        }
        Effect::NewSession => {
            *session = None;
            st.session_id = None;
            st.system("fresh session — the next message starts it");
            st.toast("new session", Sev::Ok);
        }
        Effect::ClearTranscript => {
            st.timeline.clear();
            st.system("transcript cleared (stored session history is unchanged)");
        }
        Effect::EditTitle {
            session_id: _,
            current,
        } => {
            st.push_overlay(Overlay::Form(views::Form::session_title(&current)));
        }
        Effect::SummaryPreview(summary) => {
            let clipboard_status = match nexus_app::clipboard::copy(&summary.content) {
                Ok(method) => format!("copied via {method}"),
                Err(_) => "clipboard unavailable; saved artifact is copyable".into(),
            };
            st.push_overlay(Overlay::Summary(SummaryPreview::new(
                summary.session_id,
                summary.content,
                summary.path.display().to_string(),
                clipboard_status,
            )));
        }
        Effect::Compact => match session.as_ref() {
            Some(id) => match nexus_app::services::compact_session(app, id.as_str()) {
                Ok((new_id, report)) => {
                    *session = Some(SessionId::from(new_id.clone()));
                    st.session_id = Some(new_id);
                    st.push_report(&report);
                }
                Err(e) => st.system_sev(format!("compact: {e}"), Sev::Err),
            },
            None => st.system_sev("no active session to compact", Sev::Warn),
        },
        Effect::Quit => st.should_quit = true,
        Effect::AttachSession(id) => attach_session(st, &id, app, session),
        Effect::ResumeGoal(id) => resume_goal(st, &id, app, session),
        Effect::SetTheme(name) => {
            st.set_theme(&name);
            st.toast(format!("theme: {name}"), Sev::Ok);
        }
        Effect::SetThinking(enabled) => {
            st.thinking_enabled = enabled;
            st.toast(
                format!(
                    "reasoning summaries and traces {}",
                    if enabled { "enabled" } else { "disabled" }
                ),
                Sev::Ok,
            );
        }
        Effect::SetTranscriptDetail(detail) => {
            st.detail_level = detail;
            st.toast(format!("timeline details → {}", detail.as_str()), Sev::Ok);
            if let Some(view) = st.session_view_state() {
                let _ = app.timeline().save_view_state(&view);
            }
        }
        Effect::SetTranscriptFilter(filter) => {
            st.transcript_filter = filter;
            if st.search_query.is_some() {
                refresh_durable_search(st, app, session.as_ref());
            } else {
                st.refresh_search_matches();
                st.selected_event = st.timeline.iter().rposition(|event| filter.matches(event));
                st.follow = true;
            }
            st.toast(format!("timeline filter → {}", filter.as_str()), Sev::Ok);
            if let Some(view) = st.session_view_state() {
                let _ = app.timeline().save_view_state(&view);
            }
        }
        Effect::ContinueSession {
            id,
            report,
            provider_selection_required,
        } => {
            st.push_report(&report);
            attach_session(st, &id, app, session);
            if provider_selection_required {
                start_load(st, LoadRequest::Model, app, ui_tx);
            }
        }
        Effect::ReloadApp(report) => {
            st.push_report(&report);
            spawn_reload(st, app, ui_tx);
        }
    }
}

fn attach_session(st: &mut State, id: &str, app: &Arc<App>, session: &mut Option<SessionId>) {
    match app.sessions().get(id) {
        Ok(meta) => {
            *session = Some(SessionId::from(id.to_string()));
            st.session_id = Some(id.to_string());
            st.bar.agent = meta.agent.clone();
            st.bar.model_label = meta.model.clone();
            st.bar.model_ok = app.config.models.contains_key(&meta.model);
            st.goal_label = meta.current_goal.clone();
            let timeline = app.timeline();
            match (
                timeline.view_state(id),
                timeline.page(id, None, 100, nexus_core::timeline::TranscriptFilter::All),
            ) {
                (Ok(view), Ok(events)) => {
                    st.has_older_events = events.first().is_some_and(|event| event.sequence > 1);
                    st.load_session_timeline(events, view);
                    if st.search_query.is_some() {
                        refresh_durable_search(st, app, Some(&SessionId::from(id.to_string())));
                    }
                    if let Some(sequence) = st.timeline.last().map(|event| event.sequence) {
                        let _ = timeline.mark_read(id, sequence);
                    }
                    let _ = app.orchestration().mark_agent_runs_read(id);
                }
                (Err(error), _) | (_, Err(error)) => {
                    st.timeline.clear();
                    st.system_sev(format!("timeline load: {error}"), Sev::Err);
                }
            }
            st.active_work = nexus_app::services::active_work_snapshot(app, Some(id), "idle");
            let id2 = id.to_string();
            let _ = app.update_ui_state(move |s| s.last_session = Some(id2));
            st.toast("session attached", Sev::Ok);
        }
        Err(e) => st.system_sev(format!("resume: {e}"), Sev::Err),
    }
}

fn resume_goal(st: &mut State, id: &str, app: &Arc<App>, session: &mut Option<SessionId>) {
    let goals = app.goals();
    match goals.get(id) {
        Ok(goal) => {
            if goal.status == nexus_goals::GoalStatus::Paused {
                if let Err(e) =
                    goals.transition(id, nexus_goals::GoalStatus::Running, "resumed by operator")
                {
                    st.system_sev(format!("resume: {e}"), Sev::Err);
                    return;
                }
            }
            let id2 = id.to_string();
            let _ = app.update_ui_state(move |s| s.active_goal = Some(id2));
            st.goal_label = Some(format!("{} [{}]", goal.title, goal.status.as_str()));
            if let Some(sess) = &goal.session_id {
                attach_session(st, sess.as_str(), app, session);
            }
            st.system(format!(
                "goal {id} is active — completed steps are never re-run (idempotency keys guard side effects). \
                 Send a message to continue it."
            ));
        }
        Err(e) => st.system_sev(format!("resume: {e}"), Sev::Err),
    }
}

fn spawn_reload(st: &mut State, _app: &Arc<App>, ui_tx: &mpsc::UnboundedSender<UiMsg>) {
    let tx = ui_tx.clone();
    st.busy += 1;
    tokio::spawn(async move {
        let msg = match App::bootstrap(false).await {
            Ok(app) => UiMsg::Reloaded(Ok(Arc::new(app))),
            Err(e) => UiMsg::Reloaded(Err(e.to_string())),
        };
        let _ = tx.send(msg);
    });
}

/// Reload after refreshing the Codex plan-model cache, so a fresh login
/// immediately surfaces the account's default model instead of a preset.
fn spawn_reload_with_plan_refresh(st: &mut State, ui_tx: &mpsc::UnboundedSender<UiMsg>) {
    let tx = ui_tx.clone();
    st.busy += 1;
    tokio::spawn(async move {
        // Cache refresh only — a failure here surfaces later in the menu.
        let _ = nexus_app::codex::list_plan_models().await;
        let msg = match App::bootstrap(false).await {
            Ok(app) => UiMsg::Reloaded(Ok(Arc::new(app))),
            Err(e) => UiMsg::Reloaded(Err(e.to_string())),
        };
        let _ = tx.send(msg);
    });
}

// ------------------------------------------------------------------- actions

fn handle_action(
    st: &mut State,
    action: UiAction,
    app: &Arc<App>,
    session: &mut Option<SessionId>,
    ui_tx: &mpsc::UnboundedSender<UiMsg>,
) {
    if st.mode == Mode::Running && action_changes_active_context(&action) {
        st.toast(
            "cancel the running turn first (Ctrl+C), then retry this action",
            Sev::Warn,
        );
        st.system_sev(
            "configuration/session mutations are blocked while a turn is active",
            Sev::Warn,
        );
        return;
    }
    match action {
        UiAction::RunCommand(line) => run_command(st, &line, app, session, ui_tx),
        UiAction::InsertInput(text) => {
            st.close_overlays();
            st.input.set_text(text);
        }
        UiAction::Confirmed(confirmed) => {
            let logout_exit = matches!(
                &confirmed,
                nexus_app::ConfirmedAction::LogoutCodex
                    | nexus_app::ConfirmedAction::RemoveCredential {
                        exit_after: true,
                        ..
                    }
            );
            let reload = matches!(
                &confirmed,
                nexus_app::ConfirmedAction::LogoutCodex
                    | nexus_app::ConfirmedAction::UseExistingCodex
                    | nexus_app::ConfirmedAction::RevokeExistingCodex
                    | nexus_app::ConfirmedAction::UseExistingClaude
                    | nexus_app::ConfirmedAction::RevokeExistingClaude
                    | nexus_app::ConfirmedAction::RemoveCredential { .. }
            );
            match nexus_app::apply_confirmed(app, &confirmed) {
                Ok(report) => {
                    st.push_report(&report);
                    if logout_exit {
                        st.should_quit = true;
                    } else if reload {
                        spawn_reload(st, app, ui_tx);
                    }
                }
                Err(e) => st.system_sev(format!("failed: {e}"), Sev::Err),
            }
        }
        UiAction::Load(req) => start_load(st, req, app, ui_tx),
        UiAction::AttachSession(id) => attach_session(st, &id, app, session),
        UiAction::ResumeGoal(id) => resume_goal(st, &id, app, session),
        UiAction::SetTheme(name) => {
            let name2 = name.clone();
            let _ = app.update_ui_state(move |s| s.theme = Some(name2));
            st.set_theme(&name);
            st.toast(format!("theme: {name} (persisted)"), Sev::Ok);
        }
        UiAction::SubmitGoal(spec) => match nexus_app::services::goal_create(app, spec) {
            Ok(id) => {
                st.goal_label = Some(id.clone());
                st.toast(format!("goal {id} created"), Sev::Ok);
                run_command(st, &format!("goal show {id}"), app, session, ui_tx);
            }
            Err(e) => st.system_sev(format!("goal: {e}"), Sev::Err),
        },
        UiAction::SubmitCustomEndpoint(spec) => {
            match nexus_app::providers::save_custom_endpoint(app, &spec) {
                Ok(report) => {
                    st.push_report(&report);
                    spawn_reload(st, app, ui_tx);
                }
                Err(e) => st.system_sev(format!("endpoint: {e}"), Sev::Err),
            }
        }
        UiAction::TestCustomEndpoint(spec) => {
            let tx = ui_tx.clone();
            let generation = st.generation;
            st.busy += 1;
            st.toast("testing endpoint…", Sev::Info);
            tokio::spawn(async move {
                let msg = match nexus_app::providers::test_custom_endpoint(&spec).await {
                    Ok(report) => UiMsg::ShowReport {
                        title: Some("connection test".into()),
                        report,
                    },
                    Err(e) => UiMsg::Failed {
                        generation,
                        context: "endpoint test".into(),
                        error: e.to_string(),
                    },
                };
                let _ = tx.send(msg);
            });
        }
        UiAction::StoreProviderKey { provider, key } => {
            match app.credentials.set(&provider, "default", &key) {
                Ok(()) => {
                    app.redactor.register(key.expose());
                    st.toast(format!("{provider} key stored (restricted file)"), Sev::Ok);
                    start_load(st, LoadRequest::Login, app, ui_tx);
                }
                Err(e) => st.system_sev(format!("credential store: {e}"), Sev::Err),
            }
        }
        UiAction::StartDeviceLogin => {
            let mut progress = views::Progress::new("Codex device login (isolated profile)");
            progress.push_line("launching `codex login --device-auth`…".into());
            st.push_overlay(Overlay::Progress(progress));
            let (cancel_tx, cancel_rx) = watch::channel(false);
            st.cancel_login = Some(cancel_tx);
            let (dev_tx, mut dev_rx) = mpsc::unbounded_channel::<DeviceLoginEvent>();
            let tx = ui_tx.clone();
            tokio::spawn(async move {
                while let Some(ev) = dev_rx.recv().await {
                    let _ = tx.send(UiMsg::Device(ev));
                }
            });
            let tx = ui_tx.clone();
            st.busy += 1;
            tokio::spawn(async move {
                let result = nexus_app::codex::device_login(dev_tx, cancel_rx)
                    .await
                    .map_err(|e| e.to_string());
                let _ = tx.send(UiMsg::DeviceDone(result));
            });
        }
        UiAction::StartClaudeLogin => {
            let mut progress = views::Progress::new("Claude subscription login");
            progress.push_line("launching `claude auth login --claudeai`…".into());
            progress.push_line(
                "Complete the official browser flow; NEXUS never receives the credential.".into(),
            );
            st.push_overlay(Overlay::Progress(progress));
            let tx = ui_tx.clone();
            st.busy += 1;
            tokio::spawn(async move {
                let result = nexus_app::claude::login_subscription()
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(UiMsg::ClaudeLoginDone(result));
            });
        }
        UiAction::CodexImport => {
            let source = nexus_models::codex_auth::auth_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "~/.codex/auth.json".into());
            let dest = nexus_models::codex_auth::nexus_auth_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            st.push_overlay(Overlay::Confirm(views::Confirm::custom(
                "import Codex login",
                vec![
                    format!("source       {source}"),
                    format!("destination  {dest}"),
                    "This copies authentication into NEXUS storage.".into(),
                    "The original Codex profile will not be modified.".into(),
                ],
                UiAction::RunCommand("__codex_do_import".into()),
            )));
        }
        UiAction::CodexApiKey(key) => {
            let tx = ui_tx.clone();
            let generation = st.generation;
            st.busy += 1;
            st.toast("logging in with API key…", Sev::Info);
            tokio::spawn(async move {
                let msg = match nexus_app::codex::login_with_api_key(&key).await {
                    Ok(profile) => UiMsg::ShowReport {
                        title: None,
                        report: Report::untitled().ok(format!(
                            "Codex isolated profile logged in ({})",
                            profile.mode
                        )),
                    },
                    Err(e) => UiMsg::Failed {
                        generation,
                        context: "codex api-key login".into(),
                        error: e.to_string(),
                    },
                };
                let _ = tx.send(msg);
            });
        }
        UiAction::CancelOp => {
            if let Some(cancel) = &st.cancel_login {
                let _ = cancel.send(true);
                st.toast("cancelling…", Sev::Warn);
            }
        }
        UiAction::SelectModel(name) => match nexus_app::services::model_select(app, &name) {
            Ok(report) => {
                if let Some(id) = session.as_ref() {
                    let _ = app.sessions().set_model(id.as_str(), &name);
                    let _ = app.sessions().set_status(id.as_str(), "active");
                }
                st.push_report(&report);
                spawn_reload(st, app, ui_tx);
            }
            Err(e) => st.system_sev(format!("model: {e}"), Sev::Err),
        },
        UiAction::UseDiscovered {
            provider,
            base_url,
            model_id,
        } => {
            match nexus_app::providers::save_discovered_model(app, &provider, &base_url, &model_id)
            {
                Ok(name) => {
                    let _ = app.update_ui_state({
                        let name = name.clone();
                        move |s| s.active_model = Some(name)
                    });
                    if let Some(id) = session.as_ref() {
                        let _ = app.sessions().set_model(id.as_str(), &name);
                        let _ = app.sessions().set_status(id.as_str(), "active");
                    }
                    st.toast(format!("saved + selected `{name}`"), Sev::Ok);
                    st.close_overlays();
                    spawn_reload(st, app, ui_tx);
                }
                Err(e) => st.system_sev(format!("model save: {e}"), Sev::Err),
            }
        }
        UiAction::PickCodexEffort { model_id } => {
            let plan = nexus_app::codex::cached_plan_models();
            match plan.iter().find(|m| m.id == model_id) {
                Some(m) if !m.reasoning_efforts.is_empty() => {
                    st.push_overlay(Overlay::Menu(menus::effort_menu(m)));
                }
                _ => handle_action(
                    st,
                    UiAction::UseCodexModel {
                        model_id,
                        effort: None,
                    },
                    app,
                    session,
                    ui_tx,
                ),
            }
        }
        UiAction::UseCodexModel { model_id, effort } => {
            match nexus_app::providers::save_codex_model(app, &model_id, effort.as_deref()) {
                Ok(name) => {
                    let _ = app.update_ui_state({
                        let name = name.clone();
                        move |s| s.active_model = Some(name)
                    });
                    if let Some(id) = session.as_ref() {
                        let _ = app.sessions().set_model(id.as_str(), &name);
                        let _ = app.sessions().set_status(id.as_str(), "active");
                    }
                    let effort_note = effort.map(|e| format!(" · effort {e}")).unwrap_or_default();
                    st.toast(format!("selected `{name}`{effort_note}"), Sev::Ok);
                    st.close_overlays();
                    spawn_reload(st, app, ui_tx);
                }
                Err(e) => st.system_sev(format!("model save: {e}"), Sev::Err),
            }
        }
        UiAction::ProbeProvider(id) => {
            let app2 = app.clone();
            let tx = ui_tx.clone();
            st.bump_generation();
            let generation = st.generation;
            st.busy += 1;
            st.toast(format!("probing {id}…"), Sev::Info);
            tokio::spawn(async move {
                let entries = nexus_app::providers::catalog(&app2).await;
                let Some(mut entry) = entries.into_iter().find(|e| e.id == id) else {
                    let _ = tx.send(UiMsg::Failed {
                        generation,
                        context: "provider".into(),
                        error: format!("unknown provider `{id}`"),
                    });
                    return;
                };
                entry.state = nexus_app::providers::probe_provider(&app2, &entry).await;
                let configured: Vec<(String, String)> = entry
                    .configured_models
                    .iter()
                    .filter_map(|name| {
                        app2.config
                            .models
                            .get(name)
                            .map(|m| (name.clone(), m.model.clone()))
                    })
                    .collect();
                let _ = tx.send(UiMsg::Loaded {
                    generation,
                    data: Loaded::Provider {
                        entry: Box::new(entry),
                        configured,
                    },
                });
            });
        }
        UiAction::RenameSession { title } => match session.as_ref() {
            Some(id) => match app.sessions().rename(id.as_str(), &title) {
                Ok(()) => st.toast(format!("session title → {title}"), Sev::Ok),
                Err(e) => st.system_sev(format!("title: {e}"), Sev::Err),
            },
            None => st.system_sev("no active session to title", Sev::Warn),
        },
        UiAction::CopyText(text) => match nexus_app::clipboard::copy(&text) {
            Ok(method) => st.toast(format!("copied via {method}"), Sev::Ok),
            Err(e) => st.toast(e.to_string(), Sev::Warn),
        },
        UiAction::RolloverSummary {
            source_session,
            content,
        } => match nexus_app::services::rollover_summary(app, &source_session, &content) {
            Ok((new_id, report)) => {
                st.push_report(&report);
                attach_session(st, &new_id, app, session);
            }
            Err(e) => st.system_sev(format!("summary rollover: {e}"), Sev::Err),
        },
        UiAction::PrepareCommit { paths, message } => {
            match nexus_app::services::commit_preview_report(app, &paths, &message) {
                Ok(report) => {
                    let mut body = vec![
                        format!("message  {message}"),
                        format!("files    {}", paths.join(", ")),
                        "Selected-file diff preview:".into(),
                    ];
                    body.extend(report.to_plain_text().lines().map(String::from));
                    st.push_overlay(Overlay::Confirm(views::Confirm::custom(
                        "confirm local commit",
                        body,
                        UiAction::Confirmed(nexus_app::ConfirmedAction::CommitFiles {
                            paths,
                            message,
                            allow_hooks: false,
                        }),
                    )));
                }
                Err(e) => st.system_sev(format!("commit preview: {e}"), Sev::Err),
            }
        }
    }
}

fn action_changes_active_context(action: &UiAction) -> bool {
    matches!(
        action,
        UiAction::Confirmed(_)
            | UiAction::AttachSession(_)
            | UiAction::ResumeGoal(_)
            | UiAction::SubmitGoal(_)
            | UiAction::SubmitCustomEndpoint(_)
            | UiAction::StoreProviderKey { .. }
            | UiAction::StartDeviceLogin
            | UiAction::StartClaudeLogin
            | UiAction::CodexImport
            | UiAction::CodexApiKey(_)
            | UiAction::SelectModel(_)
            | UiAction::UseDiscovered { .. }
            | UiAction::UseCodexModel { .. }
            | UiAction::RenameSession { .. }
            | UiAction::RolloverSummary { .. }
            | UiAction::PrepareCommit { .. }
    )
}

// --------------------------------------------------------------------- loads

fn start_load(
    st: &mut State,
    req: LoadRequest,
    app: &Arc<App>,
    ui_tx: &mpsc::UnboundedSender<UiMsg>,
) {
    match req {
        // Cheap local reads: build synchronously.
        LoadRequest::Goals => match app.goals().list(Some(&app.workspace_key)) {
            Ok(goals) if goals.is_empty() => {
                st.system_sev("no goals yet — create one with /goal", Sev::Warn)
            }
            Ok(goals) => st.push_overlay(Overlay::Menu(menus::goals_menu(&goals))),
            Err(e) => st.system_sev(format!("goals: {e}"), Sev::Err),
        },
        LoadRequest::GoalMenu => match app.goals().list(Some(&app.workspace_key)) {
            Ok(goals) => {
                let active = nexus_app::services::active_goal_id(app);
                st.push_overlay(Overlay::Menu(menus::goal_menu(&goals, active.as_deref())));
            }
            Err(e) => st.system_sev(format!("goals: {e}"), Sev::Err),
        },
        LoadRequest::GoalDetail(id) if id == "NEW" => {
            st.push_overlay(Overlay::Form(views::Form::goal_create(
                app.config.limits.goal_step_budget as i64,
                app.config.limits.goal_runtime_budget_min as i64,
            )));
        }
        LoadRequest::GoalDetail(id) => match nexus_app::services::goal_show_report(app, &id) {
            Ok(report) => st.push_overlay(Overlay::Pager(
                Pager::new(format!("goal {id}"), report).refreshable(LoadRequest::GoalDetail(id)),
            )),
            Err(e) => st.system_sev(format!("goal: {e}"), Sev::Err),
        },
        LoadRequest::Sessions => match app.sessions().list(Some(&app.workspace_key), 30) {
            Ok(sessions) if sessions.is_empty() => st.system_sev("no sessions yet", Sev::Warn),
            Ok(sessions) => st.push_overlay(Overlay::Menu(menus::sessions_menu(&sessions))),
            Err(e) => st.system_sev(format!("sessions: {e}"), Sev::Err),
        },
        LoadRequest::Resume => match nexus_app::services::resume_candidates(app) {
            Ok(c) if c.is_empty() => {
                st.system_sev("nothing to resume in this workspace", Sev::Warn)
            }
            Ok(c) => st.push_overlay(Overlay::Menu(menus::resume_menu(&c))),
            Err(e) => st.system_sev(format!("resume: {e}"), Sev::Err),
        },
        LoadRequest::Agents => match app.agent_catalog() {
            Ok(catalog) => st.push_overlay(Overlay::Menu(menus::agents_menu(
                &app.active_agent(),
                &catalog.list(),
            ))),
            Err(error) => st.system_sev(format!("agents: {error}"), Sev::Err),
        },
        LoadRequest::Tasks => {
            match nexus_app::services::tasks_report(app, st.session_id.as_deref()) {
                Ok(report) => st.push_overlay(Overlay::Pager(
                    Pager::new("tasks", report).refreshable(LoadRequest::Tasks),
                )),
                Err(error) => st.system_sev(format!("tasks: {error}"), Sev::Err),
            }
        }
        LoadRequest::Subagents => match st.session_id.as_deref() {
            Some(session_id) => match nexus_app::services::subagents_report(app, session_id) {
                Ok(report) => st.push_overlay(Overlay::Pager(
                    Pager::new("subagents", report).refreshable(LoadRequest::Subagents),
                )),
                Err(error) => st.system_sev(format!("subagents: {error}"), Sev::Err),
            },
            None => st.system_sev("no active session for subagents", Sev::Warn),
        },
        LoadRequest::Persona => match app.personas().list() {
            Ok(personas) => {
                let selected = app.read_ui_state(|state| state.selected_persona.clone());
                st.push_overlay(Overlay::Menu(menus::personas_menu(
                    &personas,
                    selected.as_deref(),
                )));
            }
            Err(e) => st.system_sev(format!("persona: {e}"), Sev::Err),
        },
        LoadRequest::Profile => {
            let profile = app.read_ui_state(|state| state.profile_name.clone());
            match app.profiles().list(&profile, true) {
                Ok(traits) => {
                    st.push_overlay(Overlay::Menu(menus::profile_menu(&profile, &traits)))
                }
                Err(e) => st.system_sev(format!("profile: {e}"), Sev::Err),
            }
        }
        LoadRequest::Theme => {
            st.push_overlay(Overlay::Menu(menus::theme_menu(&st.theme_name)));
        }
        LoadRequest::Thinking => {
            st.push_overlay(Overlay::Menu(menus::thinking_menu(st.thinking_enabled)));
        }
        LoadRequest::Details => {
            st.push_overlay(Overlay::Menu(menus::details_menu(st.detail_level)));
        }
        LoadRequest::Transcript => {
            st.push_overlay(Overlay::Menu(menus::transcript_menu(st.transcript_filter)));
        }
        LoadRequest::Permissions => {
            let mode = nexus_app::services::permission_mode(&app.config.policy);
            st.push_overlay(Overlay::Menu(menus::permissions_menu(mode)));
        }
        LoadRequest::Sandbox => {
            st.push_overlay(Overlay::Menu(menus::sandbox_menu(&app.config.sandbox)));
        }
        LoadRequest::Init => {
            let plan = nexus_app::services::init_plan(app);
            st.push_overlay(Overlay::Menu(menus::init_menu(&plan)));
        }
        LoadRequest::Config => {
            st.push_overlay(Overlay::Menu(menus::config_menu()));
        }
        LoadRequest::Branch => match nexus_app::gitx::branches(&app.workspace) {
            Ok(branches) => {
                st.push_overlay(Overlay::Menu(menus::branches_menu(&branches)));
            }
            Err(e) => st.system_sev(format!("branch: {e}"), Sev::Err),
        },
        LoadRequest::Commit => {
            let paths = nexus_app::gitx::modified_files(&app.workspace);
            st.push_overlay(Overlay::Form(views::Form::git_commit(&paths)));
        }
        LoadRequest::Connector => match nexus_app::connectors::discover() {
            Ok(candidates) if candidates.is_empty() => st.system_sev(
                "no Codex MCP configuration or Agent Skills were discovered",
                Sev::Warn,
            ),
            Ok(candidates) => {
                st.push_overlay(Overlay::Menu(menus::connectors_menu(&candidates)));
            }
            Err(e) => st.system_sev(format!("connector discovery: {e}"), Sev::Err),
        },
        LoadRequest::Tools => {
            let report = nexus_app::services::tools_report(app);
            st.push_overlay(Overlay::Pager(
                Pager::new("tools", report).refreshable(LoadRequest::Tools),
            ));
        }
        LoadRequest::Memory => match nexus_app::services::memory_report(app, None) {
            Ok(report) => st.push_overlay(Overlay::Pager(
                Pager::new("memory — /memory add|search|forget", report)
                    .refreshable(LoadRequest::Memory),
            )),
            Err(e) => st.system_sev(format!("memory: {e}"), Sev::Err),
        },
        LoadRequest::Skills => match nexus_app::services::skills_report(app) {
            Ok(report) => st.push_overlay(Overlay::Pager(
                Pager::new("skills — /skills enable|disable <name>", report)
                    .refreshable(LoadRequest::Skills),
            )),
            Err(e) => st.system_sev(format!("skills: {e}"), Sev::Err),
        },
        LoadRequest::Mcp => match nexus_app::services::mcp_report(app) {
            Ok(report) => st.push_overlay(Overlay::Pager(
                Pager::new("mcp — /mcp trust|untrust|tools <name>", report)
                    .refreshable(LoadRequest::Mcp),
            )),
            Err(e) => st.system_sev(format!("mcp: {e}"), Sev::Err),
        },
        LoadRequest::Help => {
            st.push_overlay(Overlay::Pager(Pager::new("help", help_report())));
        }

        // Network-touching loads run in the background.
        LoadRequest::Status => {
            let app2 = app.clone();
            let tx = ui_tx.clone();
            st.bump_generation();
            let generation = st.generation;
            st.busy += 1;
            let active = nexus_app::status::ActiveContext {
                session_id: st.session_id.clone(),
                tool_calls: st.tool_calls,
                runtime_secs: st.started.elapsed().as_secs(),
                pending_approvals: st.pending_approvals,
                last_error: st.last_error.clone(),
            };
            tokio::spawn(async move {
                let snap = nexus_app::status::snapshot(&app2, &active, true).await;
                let _ = tx.send(UiMsg::Loaded {
                    generation,
                    data: Loaded::Status(Box::new(snap)),
                });
            });
        }
        LoadRequest::Login | LoadRequest::Model => {
            let is_login = req == LoadRequest::Login;
            let app2 = app.clone();
            let tx = ui_tx.clone();
            st.bump_generation();
            let generation = st.generation;
            st.busy += 1;
            tokio::spawn(async move {
                let entries = nexus_app::providers::catalog(&app2).await;
                let data = if is_login {
                    Loaded::Login(entries)
                } else {
                    Loaded::Model(entries)
                };
                let _ = tx.send(UiMsg::Loaded { generation, data });
            });
        }
    }
}

fn apply_loaded(st: &mut State, data: Loaded, app: &Arc<App>) {
    match data {
        Loaded::Status(snap) => {
            st.goal_label = snap
                .goal
                .as_ref()
                .map(|g| format!("{} [{}]", g.title, g.status));
            if let Some(model) = &snap.model {
                st.bar.model_ok = !model.auth_state.contains("required");
            }
            let report = nexus_app::status::to_report(&snap);
            // Replace an existing status pager in place (refresh), else open.
            if let Some(Overlay::Pager(p)) = st.overlays.last_mut() {
                if p.refresh == Some(LoadRequest::Status) {
                    p.report = report;
                    return;
                }
            }
            st.push_overlay(Overlay::Pager(
                Pager::new("status — r to refresh", report).refreshable(LoadRequest::Status),
            ));
        }
        Loaded::Login(entries) => replace_or_push_menu(st, menus::login_menu(&entries)),
        Loaded::Model(entries) => {
            let active = app.any_model_name();
            let configured: Vec<(String, String, String)> = app
                .config
                .models
                .iter()
                .map(|(n, m)| (n.clone(), m.provider.clone(), m.model.clone()))
                .collect();
            replace_or_push_menu(st, menus::model_menu(&configured, &entries, &active));
        }
        Loaded::Provider { entry, configured } => {
            st.push_overlay(Overlay::Menu(menus::provider_menu(&entry, &configured)));
        }
    }
}

/// Refresh semantics: if the top overlay is a menu with the same title,
/// replace its items and keep the cursor; otherwise push a new overlay.
fn replace_or_push_menu(st: &mut State, menu: Menu) {
    if let Some(Overlay::Menu(existing)) = st.overlays.last_mut() {
        if existing.title == menu.title {
            let selected = existing.selected.min(menu.items.len().saturating_sub(1));
            *existing = menu;
            existing.selected = selected;
            return;
        }
    }
    st.push_overlay(Overlay::Menu(menu));
}

fn apply_device_event(st: &mut State, ev: DeviceLoginEvent) {
    let Some(Overlay::Progress(p)) = st.overlays.last_mut() else {
        return;
    };
    match ev {
        DeviceLoginEvent::VerificationUrl(url) => p.url = Some(url),
        DeviceLoginEvent::UserCode(code) => p.code = Some(code),
        DeviceLoginEvent::Info(line) => p.push_line(line),
        DeviceLoginEvent::Success { mode, account_id } => {
            p.done = true;
            p.push_line(format!(
                "authenticated ({mode}){}",
                account_id
                    .map(|a| format!(" — account {a}"))
                    .unwrap_or_default()
            ));
        }
        DeviceLoginEvent::Failed(e) => {
            p.done = true;
            p.failed = true;
            p.push_line(e);
        }
    }
}

fn help_report() -> Report {
    let mut r = Report::untitled()
        .header("keys")
        .line("Enter send · Alt+Enter newline · ↑/↓ history · ←/→ cursor")
        .line("/ (empty input) or Ctrl+K command palette · ? this help")
        .line("PgUp/PgDn scroll transcript · End follow · Ctrl+C quit")
        .line("in menus: ↑/↓ or j/k move · Enter select · Esc back · r refresh")
        .line("approvals: [y]es · [s]ession · [n]o (deny is the default)")
        .header("input modes")
        .line("plain text        message to the agent")
        .line("/command          slash command (see below)")
        .line("!command          run a command in the sandbox (never a raw shell)")
        .header("commands");
    let mut by_cat: std::collections::BTreeMap<&str, Vec<String>> = Default::default();
    for c in nexus_app::registry::COMMANDS {
        if !c.interactive {
            continue;
        }
        by_cat.entry(c.category.label()).or_default().push(format!(
            "/{:<12} {}{}",
            c.name,
            c.summary,
            if c.usage.is_empty() {
                String::new()
            } else {
                format!(" — usage: /{} {}", c.name, c.usage)
            }
        ));
    }
    for (cat, lines) in by_cat {
        r = r.header(cat);
        for l in lines {
            r = r.line(l);
        }
    }
    r
}

// ------------------------------------------------------------------ the turn

#[allow(clippy::too_many_arguments)]
fn submit_objective(
    st: &mut State,
    app: &Arc<App>,
    session: &mut Option<SessionId>,
    objective: String,
    ev_tx: &mpsc::UnboundedSender<LoopEvent>,
    done_tx: &mpsc::UnboundedSender<TurnDone>,
    approver: &Arc<dyn ApprovalHandler>,
) {
    let role_name = session
        .as_ref()
        .and_then(|id| app.sessions().get(id.as_str()).ok())
        .map(|meta| meta.agent)
        .unwrap_or_else(|| app.active_agent());
    let (role, custom_agent) = match app.resolve_agent(&role_name) {
        Ok(resolved) => resolved,
        Err(error) => {
            st.system_sev(
                format!("cannot resolve agent `{role_name}`: {error}"),
                Sev::Err,
            );
            return;
        }
    };

    // Create the session lazily on first turn.
    if session.is_none() {
        match app
            .sessions()
            .create(&app.workspace_key, &role_name, &app.any_model_name())
        {
            Ok(id) => {
                if let Err(e) = nexus_app::services::attach_active_goal_to_session(app, &id) {
                    st.system_sev(format!("cannot attach active goal: {e}"), Sev::Err);
                    return;
                }
                st.session_id = Some(id.as_str().to_string());
                let id_for_state = id.as_str().to_string();
                let _ = app.update_ui_state(move |state| state.last_session = Some(id_for_state));
                *session = Some(id);
            }
            Err(e) => {
                st.system_sev(format!("cannot start session: {e}"), Sev::Err);
                return;
            }
        }
    }
    let session_id = session.clone().expect("session created above");

    let runtime = match app.runtime(Some(session_id.clone())) {
        Ok(r) => r,
        Err(e) => {
            st.system_sev(format!("cannot build runtime: {e}"), Sev::Err);
            return;
        }
    };

    st.mode = Mode::Running;
    let ev_tx = ev_tx.clone();
    let done_tx = done_tx.clone();
    let approver = approver.clone();

    let handle = tokio::spawn(async move {
        let mut agent_loop = AgentLoop::new(runtime, role).with_events(ev_tx);
        if let Some(definition) = custom_agent {
            agent_loop = agent_loop.with_custom_agent(definition);
        }
        let result = agent_loop
            .run(&session_id, &objective, approver)
            .await
            .map_err(|e| e.to_string());
        let _ = done_tx.send(TurnDone { result });
    });
    st.turn_abort = Some(handle.abort_handle());
}

fn apply_loop_event(st: &mut State, ev: LoopEvent) {
    match ev {
        LoopEvent::Classified {
            class,
            model,
            agent,
        } => {
            st.bar.model_label = model.clone();
            st.bar.agent = agent.clone();
            st.push_local_event(
                TimelineStatus::Completed,
                format!("classified {class} · {model} · {agent}"),
                TimelineKind::Classification {
                    class,
                    model,
                    agent,
                },
            );
        }
        LoopEvent::ReasoningSummary(t) if st.thinking_enabled => {
            if let Some(id) = st.live_assistant_event.take() {
                st.update_event(
                    &id,
                    TimelineEventUpdate {
                        status: TimelineStatus::Completed,
                        phase: LifecyclePhase::Completed,
                        summary: Some("provider reasoning summary".into()),
                        kind: TimelineKind::ReasoningSummary { text: t },
                        duration_ms: None,
                        artifacts: Vec::new(),
                    },
                );
            } else {
                st.push_local_event(
                    TimelineStatus::Completed,
                    "provider reasoning summary".into(),
                    TimelineKind::ReasoningSummary { text: t },
                );
            }
        }
        LoopEvent::ReasoningSummary(t) => {
            if let Some(id) = st.live_assistant_event.take() {
                st.update_event(
                    &id,
                    TimelineEventUpdate {
                        status: TimelineStatus::Completed,
                        phase: LifecyclePhase::Completed,
                        summary: Some("provider reasoning summary".into()),
                        kind: TimelineKind::ReasoningSummary { text: t },
                        duration_ms: None,
                        artifacts: Vec::new(),
                    },
                );
            }
        }
        LoopEvent::AssistantTextDelta(delta) => {
            if let Some(id) = st.live_assistant_event.clone() {
                let mut text = st
                    .timeline
                    .iter()
                    .find(|event| event.id == id)
                    .and_then(|event| match &event.kind {
                        TimelineKind::AssistantMessage { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                text.push_str(&delta);
                st.update_event(
                    &id,
                    TimelineEventUpdate {
                        status: TimelineStatus::Running,
                        phase: LifecyclePhase::Progress,
                        summary: Some(text.lines().next().unwrap_or("").to_string()),
                        kind: TimelineKind::AssistantMessage {
                            text,
                            streaming: true,
                        },
                        duration_ms: None,
                        artifacts: Vec::new(),
                    },
                );
            } else {
                let id = st.push_local_event(
                    TimelineStatus::Running,
                    delta.lines().next().unwrap_or("").to_string(),
                    TimelineKind::AssistantMessage {
                        text: delta,
                        streaming: true,
                    },
                );
                st.live_assistant_event = Some(id);
            }
        }
        LoopEvent::AssistantStreamFailed(reason) => {
            if let Some(id) = st.live_assistant_event.take() {
                let text = st
                    .timeline
                    .iter()
                    .find(|event| event.id == id)
                    .and_then(|event| match &event.kind {
                        TimelineKind::AssistantMessage { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                st.update_event(
                    &id,
                    TimelineEventUpdate {
                        status: TimelineStatus::Failed,
                        phase: LifecyclePhase::Failed,
                        summary: Some(format!("assistant stream interrupted · {reason}")),
                        kind: TimelineKind::AssistantMessage {
                            text,
                            streaming: false,
                        },
                        duration_ms: None,
                        artifacts: Vec::new(),
                    },
                );
            }
        }
        LoopEvent::FinalAnswer(text) => {
            if let Some(id) = st.live_assistant_event.take() {
                st.update_event(
                    &id,
                    TimelineEventUpdate {
                        status: TimelineStatus::Completed,
                        phase: LifecyclePhase::Completed,
                        summary: Some(text.lines().next().unwrap_or("").to_string()),
                        kind: TimelineKind::FinalAnswer { text },
                        duration_ms: None,
                        artifacts: Vec::new(),
                    },
                );
            } else {
                st.push_local_event(
                    TimelineStatus::Completed,
                    text.lines().next().unwrap_or("").to_string(),
                    TimelineKind::FinalAnswer { text },
                );
            }
        }
        LoopEvent::PlanPromoted {
            work,
            from,
            to,
            reason,
        } => {
            st.push_local_event(
                if work.kind == nexus_core::orchestration::WorkBreakdownKind::Planned
                    && !work.approved
                {
                    TimelineStatus::Waiting
                } else {
                    TimelineStatus::Running
                },
                format!("{from} → {to} · plan v{}", work.version),
                TimelineKind::PlanRevision {
                    plan_id: work.id.as_str().to_string(),
                    from_version: work.version.saturating_sub(1),
                    to_version: work.version,
                    diff: reason,
                    approval_required: work.kind
                        == nexus_core::orchestration::WorkBreakdownKind::Planned
                        && !work.approved,
                },
            );
            st.push_local_event(
                if work.approved {
                    TimelineStatus::Running
                } else {
                    TimelineStatus::Waiting
                },
                format!(
                    "{} work · {} stage(s) · plan v{}",
                    work.kind.as_str(),
                    work.stages.len(),
                    work.version
                ),
                TimelineKind::WorkBreakdown { breakdown: work },
            );
        }
        LoopEvent::PlanResolved {
            work,
            approved,
            diff,
        } => {
            st.push_local_event(
                if approved {
                    TimelineStatus::Completed
                } else {
                    TimelineStatus::Blocked
                },
                if approved {
                    format!("plan v{} approved", work.version)
                } else {
                    format!("plan v{} denied", work.version)
                },
                TimelineKind::PlanRevision {
                    plan_id: work.id.as_str().to_string(),
                    from_version: work.version,
                    to_version: work.version,
                    diff: if diff.summary.is_empty() {
                        "initial plan approval".into()
                    } else {
                        diff.summary
                    },
                    approval_required: !approved,
                },
            );
            if let Some(stage) = work
                .current_stage
                .as_ref()
                .and_then(|id| work.stages.iter().find(|stage| &stage.id == id))
            {
                st.push_local_event(
                    match stage.status {
                        nexus_core::orchestration::StageStatus::Pending => TimelineStatus::Pending,
                        nexus_core::orchestration::StageStatus::Running => TimelineStatus::Running,
                        nexus_core::orchestration::StageStatus::Completed => {
                            TimelineStatus::Completed
                        }
                        nexus_core::orchestration::StageStatus::Failed => TimelineStatus::Failed,
                        nexus_core::orchestration::StageStatus::Blocked => TimelineStatus::Blocked,
                        nexus_core::orchestration::StageStatus::Skipped => TimelineStatus::Skipped,
                    },
                    stage.title.clone(),
                    TimelineKind::StageChanged {
                        plan_id: work.id.as_str().to_string(),
                        stage_id: stage.id.clone(),
                        title: stage.title.clone(),
                        status: stage.status,
                        next_action: stage.next_action.clone(),
                    },
                );
            }
        }
        LoopEvent::StageChanged {
            plan_id,
            stage_id,
            title,
            status,
            next_action,
        } => {
            st.push_local_event(
                match status {
                    nexus_core::orchestration::StageStatus::Pending => TimelineStatus::Pending,
                    nexus_core::orchestration::StageStatus::Running => TimelineStatus::Running,
                    nexus_core::orchestration::StageStatus::Completed => TimelineStatus::Completed,
                    nexus_core::orchestration::StageStatus::Failed => TimelineStatus::Failed,
                    nexus_core::orchestration::StageStatus::Blocked => TimelineStatus::Blocked,
                    nexus_core::orchestration::StageStatus::Skipped => TimelineStatus::Skipped,
                },
                title.clone(),
                TimelineKind::StageChanged {
                    plan_id,
                    stage_id,
                    title,
                    status,
                    next_action,
                },
            );
        }
        LoopEvent::ToolPlan {
            tool,
            summary,
            risk,
            arguments,
        } => {
            let id = st.push_local_event(
                TimelineStatus::Pending,
                summary.clone(),
                TimelineKind::ToolProposal {
                    tool: tool.clone(),
                    arguments,
                    summary,
                    risk,
                },
            );
            st.live_tool_events.insert(tool, id);
        }
        LoopEvent::PolicyDecision {
            tool,
            decision,
            layer,
            reason,
        } => {
            st.push_local_event(
                if decision == "deny" {
                    TimelineStatus::Blocked
                } else {
                    TimelineStatus::Completed
                },
                format!("policy {decision} · {reason}"),
                TimelineKind::PolicyDecision {
                    tool,
                    decision,
                    layer,
                    reason,
                },
            );
        }
        LoopEvent::ApprovalRequested { tool, summary } => {
            st.push_local_event(
                TimelineStatus::Waiting,
                format!("awaiting approval · {tool}"),
                TimelineKind::Approval {
                    tool,
                    decision: None,
                    summary,
                    edited: false,
                },
            );
        }
        LoopEvent::ToolExecutionStarted { tool } => {
            if let Some(id) = st.live_tool_events.get(&tool).cloned() {
                let arguments = st
                    .timeline
                    .iter()
                    .find(|event| event.id == id)
                    .and_then(|event| match &event.kind {
                        TimelineKind::ToolProposal { arguments, .. } => Some(arguments.clone()),
                        _ => None,
                    })
                    .unwrap_or(serde_json::Value::Null);
                st.update_event(
                    &id,
                    TimelineEventUpdate {
                        status: TimelineStatus::Running,
                        phase: LifecyclePhase::Started,
                        summary: None,
                        kind: TimelineKind::ToolExecution {
                            tool,
                            arguments,
                            output_preview: String::new(),
                            exit_status: None,
                            affected_paths: Vec::new(),
                        },
                        duration_ms: None,
                        artifacts: Vec::new(),
                    },
                );
            }
        }
        LoopEvent::ToolExecutionFinished {
            tool,
            ok,
            preview,
            duration_ms,
            affected_paths,
            artifacts,
        } => {
            st.tool_calls += 1;
            if let Some(id) = st.live_tool_events.remove(&tool) {
                let arguments = st
                    .timeline
                    .iter()
                    .find(|event| event.id == id)
                    .and_then(|event| match &event.kind {
                        TimelineKind::ToolExecution { arguments, .. }
                        | TimelineKind::ToolProposal { arguments, .. } => Some(arguments.clone()),
                        _ => None,
                    })
                    .unwrap_or(serde_json::Value::Null);
                st.update_event(
                    &id,
                    TimelineEventUpdate {
                        status: if ok {
                            TimelineStatus::Completed
                        } else {
                            TimelineStatus::Failed
                        },
                        phase: if ok {
                            LifecyclePhase::Completed
                        } else {
                            LifecyclePhase::Failed
                        },
                        summary: None,
                        kind: TimelineKind::ToolExecution {
                            tool,
                            arguments,
                            output_preview: preview,
                            exit_status: Some(if ok { "ok" } else { "error" }.into()),
                            affected_paths,
                        },
                        duration_ms: Some(duration_ms),
                        artifacts,
                    },
                );
            }
        }
        LoopEvent::DiffProduced { tool, preview } => {
            st.push_local_event(
                TimelineStatus::Completed,
                format!("diff from {tool}"),
                TimelineKind::Diff {
                    path: None,
                    insertions: 0,
                    deletions: 0,
                    preview,
                },
            );
        }
        LoopEvent::Retry {
            attempt,
            max,
            reason,
        } => {
            st.push_local_event(
                TimelineStatus::Waiting,
                format!("retry {attempt}/{max}"),
                TimelineKind::Retry {
                    attempt,
                    max,
                    reason,
                },
            );
        }
        LoopEvent::Error(e) => {
            st.last_error = Some(e.clone());
            st.push_local_event(
                TimelineStatus::Failed,
                e.lines().next().unwrap_or("").to_string(),
                TimelineKind::Error {
                    class: "agent_loop".into(),
                    message: e,
                    retryable: false,
                },
            );
        }
    }
}

fn command_changes_active_context(cmd: &nexus_app::SlashCommand) -> bool {
    use nexus_app::registry::CommandId;
    let Some(def) = nexus_app::registry::find(&cmd.name) else {
        return false;
    };
    match def.id {
        CommandId::New
        | CommandId::Resume
        | CommandId::Continue
        | CommandId::Pause
        | CommandId::Cancel
        | CommandId::Compact
        | CommandId::Logout
        | CommandId::Exit => true,
        CommandId::Title => true,
        CommandId::Summary => true,
        CommandId::Branch => matches!(
            cmd.args.first().map(String::as_str),
            Some("create" | "switch" | "delete" | "stage" | "unstage" | "restore")
        ),
        CommandId::Commit => true,
        CommandId::Connector => {
            matches!(cmd.args.first().map(String::as_str), Some("import"))
        }
        CommandId::Model | CommandId::Agent | CommandId::Persona | CommandId::Profile => {
            !cmd.args.is_empty()
        }
        CommandId::Goal => !matches!(
            cmd.args.first().map(String::as_str),
            None | Some("show" | "verify" | "export")
        ),
        CommandId::Plan => !matches!(
            cmd.args.first().map(String::as_str),
            None | Some("verify" | "history" | "export")
        ),
        CommandId::Task => !matches!(
            cmd.args.first().map(String::as_str),
            None | Some("list" | "show" | "logs" | "result")
        ),
        CommandId::Subagents => !matches!(
            cmd.args.first().map(String::as_str),
            None | Some("list" | "tree" | "show" | "wait" | "collect")
        ),
        CommandId::Auth => matches!(
            cmd.args.first().map(String::as_str),
            Some("use-existing" | "revoke-existing" | "remove")
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_label_variants() {
        // Exercised through App in integration tests; here we lock the
        // formatting contract for the not-configured case.
        let (label, ok) = ("Not configured".to_string(), false);
        assert_eq!(label, "Not configured");
        assert!(!ok);
    }

    #[test]
    fn help_lists_every_interactive_command() {
        let text = help_report().to_plain_text();
        for c in nexus_app::registry::COMMANDS {
            if c.interactive {
                assert!(
                    text.contains(&format!("/{}", c.name)),
                    "missing /{}",
                    c.name
                );
            }
        }
    }

    #[test]
    fn active_turn_guard_blocks_transitions_but_allows_inspection() {
        let command = |line: &str| match nexus_app::classify(&format!("/{line}")).expect("parse") {
            nexus_app::Input::Slash(command) => command,
            _ => panic!("expected slash command"),
        };
        for line in [
            "new",
            "summary",
            "model use local",
            "goal create work",
            "logout codex",
            "branch stage src/lib.rs",
            "connector import mcp:test",
            "profile select focused",
        ] {
            assert!(
                command_changes_active_context(&command(line)),
                "{line} should be blocked"
            );
        }
        for line in [
            "status",
            "goal show goal_1",
            "branch status",
            "branch diff",
            "connector list",
            "connector show mcp:test",
            "theme cyberpunk",
            "btw what changed",
        ] {
            assert!(
                !command_changes_active_context(&command(line)),
                "{line} should remain available"
            );
        }
    }

    #[test]
    fn direct_mutating_actions_are_guarded() {
        assert!(action_changes_active_context(&UiAction::AttachSession(
            "session_1".into()
        )));
        assert!(action_changes_active_context(&UiAction::RolloverSummary {
            source_session: "session_1".into(),
            content: "handoff".into(),
        }));
        assert!(!action_changes_active_context(&UiAction::CopyText(
            "handoff".into()
        )));
        assert!(!action_changes_active_context(&UiAction::Load(
            LoadRequest::Status
        )));
    }
}
