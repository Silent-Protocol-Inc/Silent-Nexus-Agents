//! Interactive UI state. All model/tool text stored here has already been
//! sanitized/redacted by the harness before it reaches the transcript.

use crate::approver::ApprovalRequest;
use crate::input::InputEditor;
use crate::theme::{ColorSupport, Theme};
use crate::thinking::ThinkingState;
use crate::views::Overlay;
use nexus_app::Sev;
use nexus_core::brand;
use nexus_core::orchestration::ActiveWorkSnapshot;
use nexus_core::orchestration::StageStatus;
use nexus_core::timeline::{
    ActivityMode, ActivityVisibility, LifecyclePhase, SessionViewState, TimelineEvent,
    TimelineKind, TimelineSource, TimelineStatus, TranscriptDetail, TranscriptFilter,
};
use nexus_core::{SessionId, SpanId, TraceId, TurnId};
use std::collections::VecDeque;
use std::time::Instant;

/// Whether the agent loop is currently running a turn.
#[derive(PartialEq)]
pub enum Mode {
    Idle,
    Running,
}

/// A plan waiting on the operator, held across a dismissal of the pop-up.
///
/// The reply channel lives here rather than in the overlay so closing the
/// pop-up cannot answer for the operator: the turn stays parked, the pinned
/// panel keeps saying a decision is owed, and reopening resumes the same
/// review. The engine is still waiting on this exact sender.
pub struct PendingPlanReview {
    pub request: nexus_agent::PlanReviewRequest,
    pub reply: Option<tokio::sync::oneshot::Sender<nexus_agent::PlanReviewResponse>>,
}

/// One step in the pinned tracker.
pub struct PinnedStep {
    pub sequence: u32,
    pub title: String,
    pub status: StageStatus,
}

/// The turn's step list as the operator watches it happen.
///
/// Held apart from the timeline: it is a live view of one plan updated in
/// place, not a record of what was said, so it never joins the scrollback.
pub struct PinnedPlan {
    pub plan_id: String,
    pub agent: String,
    pub objective: String,
    pub version: u32,
    pub steps: Vec<PinnedStep>,
    /// A decision is owed before any of this runs.
    pub awaiting_approval: bool,
}

impl PinnedPlan {
    /// Completed steps and total, for the `2/5` counter.
    pub fn progress(&self) -> (usize, usize) {
        (
            self.steps
                .iter()
                .filter(|step| matches!(step.status, StageStatus::Completed | StageStatus::Skipped))
                .count(),
            self.steps.len(),
        )
    }

    /// Index of the step being worked on, else the first unfinished one.
    pub fn active_index(&self) -> Option<usize> {
        self.steps
            .iter()
            .position(|step| step.status == StageStatus::Running)
            .or_else(|| {
                self.steps.iter().position(|step| {
                    matches!(step.status, StageStatus::Pending | StageStatus::Blocked)
                })
            })
    }

    /// Apply one stage transition in place.
    pub fn update_step(&mut self, title: &str, status: StageStatus) {
        if let Some(step) = self.steps.iter_mut().find(|step| step.title == title) {
            step.status = status;
        }
    }
}

/// A transient notification.
pub struct Toast {
    pub text: String,
    pub sev: Sev,
    pub at: Instant,
}

/// Status-bar facts, refreshed from real state.
pub struct StatusBar {
    pub workspace: String,
    pub model_label: String,
    pub model_ok: bool,
    pub agent: String,
    pub sandbox_level: String,
    pub network: String,
    pub git_branch: Option<String>,
    pub tokens_in: usize,
    pub tokens_out: usize,
    /// Input the provider served from cache — a subset of `tokens_in`, not an
    /// addition to it. Zero on providers that report nothing.
    pub tokens_cached: usize,
    pub permission_mode: String,
    /// Plan mode is on. Shown next to the permission mode because that is what
    /// it overrides: while it is set, the turn refuses to change anything
    /// regardless of what the permission preset would otherwise allow.
    pub plan_mode: bool,
}

pub struct TimelineEventUpdate {
    pub status: TimelineStatus,
    pub phase: LifecyclePhase,
    pub summary: Option<String>,
    pub kind: TimelineKind,
    pub duration_ms: Option<u64>,
    pub artifacts: Vec<nexus_core::timeline::ArtifactReference>,
}

#[derive(Debug, Clone)]
pub(crate) struct WrapLayoutCacheEntry {
    pub signature: u64,
    pub width: usize,
    pub detail: TranscriptDetail,
    pub expanded: bool,
    pub rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Input,
    Timeline,
    Context,
    Drawer,
}

impl Focus {
    pub fn next(self) -> Self {
        match self {
            Self::Input => Self::Timeline,
            Self::Timeline => Self::Context,
            Self::Context => Self::Drawer,
            Self::Drawer => Self::Input,
        }
    }
}

pub struct State {
    pub theme: Theme,
    pub theme_name: String,
    pub color_support: ColorSupport,
    pub reduced_motion: bool,

    pub timeline: Vec<TimelineEvent>,
    pub transcript_filter: TranscriptFilter,
    pub detail_level: TranscriptDetail,
    pub collapsed_cards: std::collections::BTreeSet<String>,
    pub selected_event: Option<usize>,
    /// Streaming assistant cards, keyed by logical turn. A turn can update
    /// only its own card, even if an older task drains after a new turn starts.
    pub live_assistant_events: std::collections::BTreeMap<String, String>,
    /// Terminal assistant cards, keyed by logical turn. This makes terminal
    /// delivery idempotent without comparing answer text across turns.
    pub terminal_events: std::collections::BTreeMap<String, String>,
    /// Last accepted UI envelope sequence for each turn.
    pub turn_sequences: std::collections::BTreeMap<String, u64>,
    pub active_turn_id: Option<TurnId>,
    pub live_tool_events: std::collections::BTreeMap<String, String>,
    pub live_provider_events: std::collections::BTreeMap<String, String>,
    pub earliest_sequence: Option<u64>,
    pub last_background_sequence: u64,
    pub has_older_events: bool,
    pub new_events: u64,
    pub search_query: Option<String>,
    pub search_edit: Option<String>,
    pub search_matches: Vec<String>,
    pub search_match_index: usize,
    pub durable_search: bool,
    pub input: InputEditor,
    pub overlays: Vec<Overlay>,
    pub mode: Mode,
    pub pending: Option<ApprovalRequest>,
    /// Approvals that arrived while another was on screen. Held rather than
    /// dropped: dropping one denies an action the operator never saw.
    pub approval_queue: VecDeque<ApprovalRequest>,
    pub approval_selected: usize,
    pub approval_edit: Option<String>,
    /// The plan awaiting a decision, kept while the pop-up is dismissed so the
    /// pinned panel can say so and reopen it.
    pub plan_review: Option<PendingPlanReview>,
    /// The current turn's step list, shown pinned above the composer.
    pub pinned_plan: Option<PinnedPlan>,
    pub should_quit: bool,
    pub scroll: usize,
    pub follow: bool,
    pub total_wrapped_rows: usize,
    pub viewport_rows: usize,
    pub prepend_anchor_rows: Option<usize>,
    pub event_row_offsets: std::collections::BTreeMap<String, usize>,
    pub(crate) wrap_layout_cache: std::collections::HashMap<String, WrapLayoutCacheEntry>,
    pub focus: Focus,
    pub context_drawer: bool,
    pub agent_drawer: bool,
    pub toasts: VecDeque<Toast>,
    pub spinner: usize,

    pub bar: StatusBar,
    pub session_id: Option<String>,
    pub goal_label: Option<String>,
    pub tool_calls: u64,
    pub pending_approvals: u32,
    pub last_error: Option<String>,
    pub started: Instant,

    /// Number of background loads in flight (drives the busy indicator).
    pub busy: usize,
    /// Generation stamp: results from an older generation are dropped so a
    /// slow older request can never overwrite a newer view.
    pub generation: u64,
    /// Cancel signal for the running device login, when one is active.
    pub cancel_login: Option<tokio::sync::watch::Sender<bool>>,
    /// Abort handle for the active agent turn. Context-changing commands are
    /// blocked until this turn finishes or is explicitly cancelled.
    pub turn_abort: Option<tokio::task::AbortHandle>,
    /// Deliberation mode (`/thinking`). Independent of [`Self::activity_mode`]:
    /// neither control may change the other.
    pub thinking_mode: nexus_core::ThinkingMode,
    /// Anti-flicker floor before the live component may render.
    pub thinking_min_duration: std::time::Duration,
    /// Resolved AUTO decision for the active turn, from `ThinkingResolved`.
    pub thinking_show: Option<bool>,
    /// Why the decision went the way it did; shown in the detail panel only.
    pub thinking_reason: Option<&'static str>,
    /// Whether provider-supplied summaries may be previewed
    /// (`[thinking].summarize_provider_reasoning`).
    pub summarize_provider_reasoning: bool,
    /// Timeline verbosity: Default (concise) hides reasoning/diagnostics.
    pub activity_mode: ActivityMode,
    /// When the active turn started, for the live elapsed counter.
    pub turn_started: Option<Instant>,
    /// Cap on the NEXUS activity preview (`[tui.activity].reasoning_preview_lines`).
    pub preview_lines: usize,
    /// Activity animation style: `nexus`, `minimal`, or `off`.
    pub animation: String,
    /// Frames the activity animation advances per tick: slow 1, normal 2, fast 3.
    pub animation_rate: usize,
    pub active_work: ActiveWorkSnapshot,
}

impl State {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        theme_name: String,
        color_support: ColorSupport,
        reduced_motion: bool,
        bar: StatusBar,
        history: Vec<String>,
        thinking_mode: nexus_core::ThinkingMode,
    ) -> Self {
        let active_work = ActiveWorkSnapshot::empty(bar.workspace.clone());
        let mut s = Self {
            theme: Theme::new(&theme_name, color_support),
            theme_name,
            color_support,
            reduced_motion,
            timeline: Vec::new(),
            transcript_filter: TranscriptFilter::All,
            detail_level: TranscriptDetail::Compact,
            collapsed_cards: std::collections::BTreeSet::new(),
            selected_event: None,
            live_assistant_events: std::collections::BTreeMap::new(),
            terminal_events: std::collections::BTreeMap::new(),
            turn_sequences: std::collections::BTreeMap::new(),
            active_turn_id: None,
            live_tool_events: std::collections::BTreeMap::new(),
            live_provider_events: std::collections::BTreeMap::new(),
            earliest_sequence: None,
            last_background_sequence: 0,
            has_older_events: false,
            new_events: 0,
            search_query: None,
            search_edit: None,
            search_matches: Vec::new(),
            search_match_index: 0,
            durable_search: false,
            input: InputEditor::with_history(history),
            overlays: Vec::new(),
            mode: Mode::Idle,
            pending: None,
            approval_queue: VecDeque::new(),
            approval_selected: 0,
            plan_review: None,
            pinned_plan: None,
            approval_edit: None,
            should_quit: false,
            scroll: 0,
            follow: true,
            total_wrapped_rows: 0,
            viewport_rows: 0,
            prepend_anchor_rows: None,
            event_row_offsets: std::collections::BTreeMap::new(),
            wrap_layout_cache: std::collections::HashMap::new(),
            focus: Focus::Input,
            context_drawer: false,
            agent_drawer: false,
            toasts: VecDeque::new(),
            spinner: 0,
            bar,
            session_id: None,
            goal_label: None,
            tool_calls: 0,
            pending_approvals: 0,
            last_error: None,
            started: Instant::now(),
            busy: 0,
            generation: 0,
            cancel_login: None,
            turn_abort: None,
            thinking_mode,
            thinking_min_duration: std::time::Duration::from_millis(500),
            thinking_show: None,
            thinking_reason: None,
            summarize_provider_reasoning: true,
            activity_mode: ActivityMode::default(),
            turn_started: None,
            preview_lines: 3,
            animation: "nexus".into(),
            animation_rate: 2,
            active_work,
        };
        s.system(format!("{} ONLINE :: {}", brand::MARK, brand::TAGLINE));
        s.system("Type a message, `/` for commands, Ctrl+K for the palette, /help for keys.");
        s
    }

    /// Whether an event appears in the main timeline: it must pass the
    /// content-type filter and the verbosity mode. Default mode shows only
    /// Essential events; reasoning/diagnostics live in the Ctrl+E detail.
    pub fn event_visible(&self, event: &TimelineEvent) -> bool {
        self.transcript_filter.matches(event) && self.activity_mode.shows(event.kind.visibility())
    }

    /// The current thinking phase, derived from structured runtime state
    /// rather than guessed from prose.
    ///
    /// This is a **pure read**: it mutates nothing and records nothing. That is
    /// why a phase transition updates one widget instead of appending a
    /// timeline entry — the widget is recomputed from this every frame, so
    /// there is no transition event that could spam the transcript.
    pub fn thinking_state(&self) -> ThinkingState {
        let work = &self.active_work;
        if !work.waiting_approvals.is_empty() || work.turn_state.contains("waiting") {
            return ThinkingState::Waiting;
        }
        if let Some(tool) = &work.active_foreground_tool {
            return if tool.starts_with("web.") || tool.contains("search") {
                ThinkingState::Searching
            } else {
                ThinkingState::Executing
            };
        }
        if !work.validation_pending.is_empty() {
            return ThinkingState::Verifying;
        }
        if let Some(plan) = &work.work {
            if !plan.approved {
                return ThinkingState::Planning;
            }
        }
        // Assistant text is streaming with no tool in flight: the answer is
        // being composed.
        if !self.live_assistant_events.is_empty() {
            return ThinkingState::Finalizing;
        }
        ThinkingState::Understanding
    }

    /// Whether the reasoning *preview* may render right now.
    ///
    /// This gates the preview rows only. The activity indicator itself always
    /// renders while a turn runs — `/thinking off` still shows activity, tool
    /// execution, and progress, and hiding the indicator would leave the
    /// operator with no sign that anything is happening.
    ///
    /// The minimum-duration floor does double duty: it stops sub-second turns
    /// from flashing preview rows, and it covers the few milliseconds AUTO
    /// needs to classify, so `thinking_show` is always resolved first.
    pub fn thinking_preview_visible(&self) -> bool {
        if self.mode != Mode::Running {
            return false;
        }
        match self.turn_started {
            Some(started) if started.elapsed() >= self.thinking_min_duration => {}
            _ => return false,
        }
        match self.thinking_mode {
            nexus_core::ThinkingMode::Off => false,
            nexus_core::ThinkingMode::On => true,
            nexus_core::ThinkingMode::Auto => self.thinking_show.unwrap_or(false),
        }
    }

    /// Clear the per-turn deliberation resolution. Called when a turn starts so
    /// a previous turn's decision can never leak into the next one.
    pub fn reset_thinking_resolution(&mut self) {
        self.thinking_show = None;
        self.thinking_reason = None;
    }

    /// Set the deliberation mode.
    ///
    /// Deliberately touches nothing else. `/thinking` and `/view` are separate
    /// controls, and an earlier version of this code had each silently
    /// overwrite the other — see `view_and_thinking_stay_independent`.
    pub fn set_thinking_mode(&mut self, mode: nexus_core::ThinkingMode) {
        self.thinking_mode = mode;
    }

    /// Set the timeline verbosity. Touches nothing else, for the same reason.
    pub fn set_activity_mode(&mut self, mode: ActivityMode) {
        self.activity_mode = mode;
    }

    /// True when the active turn carries a real provider reasoning channel.
    /// Only then may the component claim to show reasoning — a summary the
    /// harness derived from its own state is labelled ACTIVITY instead.
    pub fn has_provider_reasoning(&self) -> bool {
        let turn = self.active_turn_id.as_ref();
        self.timeline.iter().rev().take(64).any(|event| {
            matches!(event.kind, TimelineKind::ReasoningSummary { .. })
                && turn.is_none_or(|id| &event.turn_id == id)
        })
    }

    /// Text for the activity preview, most informative first. Returns the
    /// harness's own structured knowledge — never fabricated reasoning.
    pub fn activity_preview(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let work = &self.active_work;
        if let Some(objective) = work.objective.as_ref().filter(|o| !o.trim().is_empty()) {
            out.push(objective.trim().to_string());
        }
        if let Some(plan) = &work.work {
            if let Some(stage) = plan
                .stages
                .iter()
                .find(|stage| stage.status == StageStatus::Running)
            {
                out.push(stage.title.clone());
                if let Some(next) = stage.next_action.as_ref().filter(|n| !n.trim().is_empty()) {
                    out.push(format!("Next · {}", next.trim()));
                }
            }
        }
        if let Some(pending) = work.validation_pending.first() {
            out.push(format!("Pending check · {pending}"));
        }
        // Fall back to the newest non-essential card, which is exactly the
        // detail the concise timeline is holding back.
        if out.is_empty() {
            if let Some(event) = self.timeline.iter().rev().take(64).find(|event| {
                event.kind.visibility() != ActivityVisibility::Essential
                    && !event.summary.trim().is_empty()
            }) {
                out.push(event.summary.trim().to_string());
            }
        }
        out.truncate(self.preview_lines.max(1));
        out
    }

    pub fn set_theme(&mut self, name: &str) {
        self.theme_name = name.to_string();
        self.theme = Theme::new(name, self.color_support);
    }

    pub fn system(&mut self, text: impl Into<String>) {
        self.system_sev(text, Sev::Dim);
    }

    pub fn system_sev(&mut self, text: impl Into<String>, sev: Sev) {
        let text = text.into();
        let status = match sev {
            Sev::Err => TimelineStatus::Failed,
            Sev::Warn => TimelineStatus::Waiting,
            _ => TimelineStatus::Completed,
        };
        self.push_local_event(
            status,
            text.lines().next().unwrap_or("").to_string(),
            TimelineKind::Notice {
                text,
                severity: match sev {
                    Sev::Ok => "ok",
                    Sev::Warn => "warning",
                    Sev::Err => "error",
                    Sev::Dim => "dim",
                    Sev::Info => "info",
                }
                .into(),
            },
        );
    }

    #[cfg(test)]
    pub fn user(&mut self, text: impl Into<String>) {
        self.user_for_turn(TurnId::generate(), text);
    }

    pub fn user_for_turn(&mut self, turn_id: TurnId, text: impl Into<String>) {
        let text = text.into();
        self.push_local_event_for_turn(
            turn_id,
            TimelineStatus::Completed,
            text.lines().next().unwrap_or("").to_string(),
            TimelineKind::UserMessage { text },
        );
    }

    pub fn activity(&mut self, line: impl Into<String>) {
        let line = line.into();
        let severity = if line.to_ascii_lowercase().contains("error") {
            "error"
        } else if line.to_ascii_lowercase().contains("await")
            || line.to_ascii_lowercase().contains("retry")
        {
            "warning"
        } else {
            "info"
        };
        self.push_local_event(
            if severity == "error" {
                TimelineStatus::Failed
            } else {
                TimelineStatus::Completed
            },
            line.clone(),
            TimelineKind::Notice {
                text: line,
                severity: severity.into(),
            },
        );
    }

    pub fn push_event(&mut self, event: TimelineEvent) {
        if self.timeline.iter().any(|existing| existing.id == event.id) {
            if let Some(existing) = self
                .timeline
                .iter_mut()
                .find(|existing| existing.id == event.id)
            {
                *existing = event;
            }
            return;
        }
        self.earliest_sequence = Some(
            self.earliest_sequence
                .map_or(event.sequence, |earliest| earliest.min(event.sequence)),
        );
        if event.source == TimelineSource::Background {
            self.last_background_sequence = self.last_background_sequence.max(event.sequence);
        }
        self.timeline.push(event);
        self.timeline.sort_by_key(|event| event.sequence);
        if self.follow {
            self.new_events = 0;
        } else {
            self.new_events = self.new_events.saturating_add(1);
        }
        self.refresh_search_matches();
    }

    pub fn prepend_events(&mut self, mut events: Vec<TimelineEvent>) {
        self.prepend_anchor_rows = Some(self.total_wrapped_rows);
        events.retain(|event| !self.timeline.iter().any(|existing| existing.id == event.id));
        events.append(&mut self.timeline);
        events.sort_by_key(|event| event.sequence);
        self.timeline = events;
        self.earliest_sequence = self.timeline.first().map(|event| event.sequence);
        self.last_background_sequence = self
            .timeline
            .iter()
            .filter(|event| event.source == TimelineSource::Background)
            .map(|event| event.sequence)
            .max()
            .unwrap_or(0);
        self.refresh_search_matches();
    }

    pub fn load_session_timeline(&mut self, events: Vec<TimelineEvent>, view: SessionViewState) {
        self.timeline = events;
        self.transcript_filter = view.selected_filter;
        self.detail_level = view.detail_level;
        self.collapsed_cards = view.collapsed_cards;
        self.search_query = view.search_query;
        self.durable_search = false;
        self.earliest_sequence = self.timeline.first().map(|event| event.sequence);
        self.last_background_sequence = self
            .timeline
            .iter()
            .filter(|event| event.source == TimelineSource::Background)
            .map(|event| event.sequence)
            .max()
            .unwrap_or(0);
        self.selected_event = self.timeline.len().checked_sub(1);
        self.live_assistant_events.clear();
        self.terminal_events.clear();
        self.turn_sequences.clear();
        self.active_turn_id = None;
        self.live_tool_events.clear();
        self.scroll = 0;
        self.follow = true;
        self.new_events = 0;
        self.wrap_layout_cache.clear();
        self.refresh_search_matches();
    }

    pub fn session_view_state(&self) -> Option<SessionViewState> {
        let session_id = self.session_id.as_ref()?;
        Some(SessionViewState {
            session_id: SessionId::from(session_id.as_str()),
            last_read_sequence: self
                .timeline
                .last()
                .map(|event| event.sequence)
                .unwrap_or(0),
            selected_filter: self.transcript_filter,
            detail_level: self.detail_level,
            collapsed_cards: self.collapsed_cards.clone(),
            search_query: self.search_query.clone(),
            updated_at: nexus_core::now_rfc3339(),
        })
    }

    pub fn selected_timeline_event(&self) -> Option<&TimelineEvent> {
        self.selected_event
            .and_then(|index| self.timeline.get(index))
    }

    pub fn refresh_search_matches(&mut self) {
        let Some(query) = self
            .search_query
            .as_ref()
            .map(|query| query.to_ascii_lowercase())
        else {
            self.search_matches.clear();
            self.search_match_index = 0;
            self.durable_search = false;
            return;
        };
        if self.durable_search {
            return;
        }
        self.search_matches = self
            .timeline
            .iter()
            .filter(|event| {
                self.transcript_filter.matches(event) && event.searchable_text().contains(&query)
            })
            .map(|event| event.id.clone())
            .collect();
        self.search_match_index = self
            .search_match_index
            .min(self.search_matches.len().saturating_sub(1));
    }

    pub fn push_local_event(
        &mut self,
        status: TimelineStatus,
        summary: String,
        kind: TimelineKind,
    ) -> String {
        self.push_local_event_for_turn(TurnId::generate(), status, summary, kind)
    }

    pub fn push_local_event_for_turn(
        &mut self,
        turn_id: TurnId,
        status: TimelineStatus,
        summary: String,
        kind: TimelineKind,
    ) -> String {
        let sequence = self
            .timeline
            .last()
            .map(|event| event.sequence.saturating_add(1))
            .unwrap_or(1);
        let mut event = TimelineEvent::new(
            SessionId::from(self.session_id.as_deref().unwrap_or("local")),
            turn_id,
            TraceId::from("ui"),
            SpanId::generate(),
            None,
            LifecyclePhase::Message,
            status,
            summary,
            kind,
        );
        event.sequence = sequence;
        event.source = TimelineSource::Command;
        let id = event.id.clone();
        self.push_event(event);
        id
    }

    pub fn update_event(&mut self, id: &str, update: TimelineEventUpdate) {
        if let Some(event) = self.timeline.iter_mut().find(|event| event.id == id) {
            event.status = update.status;
            event.phase = update.phase;
            if let Some(summary) = update.summary {
                event.summary = summary;
            }
            event.kind = update.kind;
            event.duration_ms = update.duration_ms;
            event.artifact_refs = update.artifacts;
        }
        self.refresh_search_matches();
    }

    pub fn toast(&mut self, text: impl Into<String>, sev: Sev) {
        self.toasts.push_back(Toast {
            text: text.into(),
            sev,
            at: Instant::now(),
        });
        while self.toasts.len() > 3 {
            self.toasts.pop_front();
        }
    }

    /// Drop expired toasts (4s lifetime).
    pub fn tick(&mut self) {
        self.spinner = self.spinner.wrapping_add(1);
        while self
            .toasts
            .front()
            .is_some_and(|t| t.at.elapsed().as_secs() >= 4)
        {
            self.toasts.pop_front();
        }
    }

    /// Render a shared [`nexus_app::Report`] into the transcript.
    pub fn push_report(&mut self, report: &nexus_app::Report) {
        if let Some(title) = &report.title {
            self.system_sev(format!("── {title} ──"), Sev::Info);
        }
        for item in &report.items {
            match item {
                nexus_app::Item::Brand { variant } => {
                    let lockup = brand::lockup(
                        *variant,
                        brand::BrandConstraints {
                            width: 60,
                            height: 20,
                            unicode: brand::unicode_supported(),
                        },
                    );
                    for line in lockup.lines {
                        self.system_sev(line.text(), Sev::Info);
                    }
                }
                nexus_app::Item::Header(h) => self.system_sev(format!("· {h}"), Sev::Info),
                nexus_app::Item::Field { key, value, sev } => {
                    let sev = *sev;
                    self.system_sev(format!("{key:>16}  {value}"), sev);
                }
                nexus_app::Item::Line { text, sev } => {
                    let sev = *sev;
                    let mark = match sev {
                        Sev::Ok => "✓ ",
                        Sev::Warn => "! ",
                        Sev::Err => "✗ ",
                        _ => "",
                    };
                    self.system_sev(format!("{mark}{text}"), sev);
                }
                nexus_app::Item::Table { headers, rows } => {
                    self.system_sev(headers.join("  ·  "), Sev::Info);
                    for row in rows {
                        self.system_sev(format!("  {}", row.join("  ·  ")), Sev::Dim);
                    }
                }
            }
        }
        self.follow = true;
        self.new_events = 0;
    }

    /// Bump the generation, invalidating in-flight results for closed views.
    pub fn bump_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    pub fn overlay_top(&mut self) -> Option<&mut Overlay> {
        self.overlays.last_mut()
    }

    pub fn push_overlay(&mut self, overlay: Overlay) {
        self.overlays.push(overlay);
    }

    pub fn pop_overlay(&mut self) {
        self.overlays.pop();
        self.bump_generation();
    }

    pub fn close_overlays(&mut self) {
        self.overlays.clear();
        self.bump_generation();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorSupport;

    fn state() -> State {
        State::new(
            "matrix".into(),
            ColorSupport::None,
            true,
            StatusBar {
                workspace: "/workspace".into(),
                model_label: "mock".into(),
                model_ok: true,
                agent: "orchestrator".into(),
                sandbox_level: "process".into(),
                network: "approval".into(),
                git_branch: Some("main".into()),
                tokens_in: 0,
                tokens_out: 0,
                tokens_cached: 0,
                permission_mode: "default".into(),
                plan_mode: false,
            },
            Vec::new(),
            nexus_core::ThinkingMode::Off,
        )
    }

    #[test]
    fn view_and_thinking_stay_independent() {
        // Regression guard. Before 2.4.0 each control silently overwrote the
        // other: `/view` changed reasoning visibility and `/thinking` forced a
        // timeline verbosity. Both directions must stay severed.
        let mut state = state();
        state.set_thinking_mode(nexus_core::ThinkingMode::On);
        state.set_activity_mode(ActivityMode::Default);

        state.set_activity_mode(ActivityMode::Debug);
        assert_eq!(
            state.thinking_mode,
            nexus_core::ThinkingMode::On,
            "/view must not change the deliberation mode"
        );

        state.set_thinking_mode(nexus_core::ThinkingMode::Off);
        assert_eq!(
            state.activity_mode,
            ActivityMode::Debug,
            "/thinking must not change the timeline verbosity"
        );
    }

    #[test]
    fn thinking_phases_follow_structured_state_in_priority_order() {
        use crate::thinking::ThinkingState;
        let mut state = state();

        // Nothing happening yet.
        assert_eq!(state.thinking_state(), ThinkingState::Understanding);

        // Streaming an answer with no tool in flight.
        state
            .live_assistant_events
            .insert("turn".into(), "event".into());
        assert_eq!(state.thinking_state(), ThinkingState::Finalizing);
        state.live_assistant_events.clear();

        state.active_work.validation_pending = vec!["cargo test".into()];
        assert_eq!(state.thinking_state(), ThinkingState::Verifying);

        state.active_work.active_foreground_tool = Some("fs.read_file".into());
        assert_eq!(
            state.thinking_state(),
            ThinkingState::Executing,
            "an active tool outranks a pending validation"
        );

        state.active_work.active_foreground_tool = Some("web.search".into());
        assert_eq!(state.thinking_state(), ThinkingState::Searching);

        state.active_work.waiting_approvals = vec!["terminal.run".into()];
        assert_eq!(
            state.thinking_state(),
            ThinkingState::Waiting,
            "waiting on the operator outranks everything"
        );
    }

    #[test]
    fn reading_the_phase_never_mutates_state_or_the_timeline() {
        let mut state = state();
        state.active_work.active_foreground_tool = Some("terminal.run".into());
        let before = state.timeline.len();
        for _ in 0..25 {
            let _ = state.thinking_state();
            let _ = state.thinking_state().title();
        }
        assert_eq!(
            state.timeline.len(),
            before,
            "phase transitions must update one widget, never append events"
        );
    }

    #[test]
    fn a_new_turn_clears_the_previous_resolution() {
        let mut state = state();
        state.thinking_show = Some(true);
        state.thinking_reason = Some("class=coding");
        state.reset_thinking_resolution();
        assert_eq!(state.thinking_show, None);
        assert_eq!(state.thinking_reason, None);
    }

    #[test]
    fn activity_mode_gates_the_visible_timeline() {
        let mut state = state();
        state.timeline.clear();
        state.user("ship the release");
        // A notice is operator-facing and stays visible in every mode; the
        // agent's internal reasoning is what folds away.
        state.activity("packed 12 files into context");
        state.push_local_event(
            TimelineStatus::Completed,
            "reasoning".into(),
            TimelineKind::ReasoningSummary {
                text: "Considering the release checklist.".into(),
            },
        );
        state.push_local_event(
            TimelineStatus::Completed,
            "routing".into(),
            TimelineKind::ModelRouting {
                provider: "mock".into(),
                model: "mock".into(),
                reason: "default".into(),
            },
        );
        let visible = |s: &State| s.timeline.iter().filter(|e| s.event_visible(e)).count();

        assert_eq!(state.activity_mode, ActivityMode::Default);
        assert_eq!(
            visible(&state),
            2,
            "default shows the message and the notice, not the reasoning",
        );
        state.activity_mode = ActivityMode::Detailed;
        assert_eq!(visible(&state), 3, "detailed adds the reasoning summary");
        state.activity_mode = ActivityMode::Debug;
        assert_eq!(visible(&state), 4, "debug adds routing diagnostics");
    }

    #[test]
    fn transcript_filter_stays_orthogonal_to_activity_mode() {
        let mut state = state();
        state.timeline.clear();
        state.user("ship the release");
        state.activity("packed 12 files into context");
        state.activity_mode = ActivityMode::Debug;
        let visible = |s: &State| s.timeline.iter().filter(|e| s.event_visible(e)).count();
        assert_eq!(visible(&state), 2, "debug shows both events");
        state.transcript_filter = TranscriptFilter::Diffs;
        assert_eq!(
            visible(&state),
            0,
            "the content-type filter still applies in debug"
        );
    }

    #[test]
    fn scrolled_transcript_counts_new_events_without_stealing_viewport() {
        let mut state = state();
        state.follow = false;
        state.scroll = 7;
        state.new_events = 0;
        state.user("background result");
        assert_eq!(state.scroll, 7);
        assert_eq!(state.new_events, 1);

        state.follow = true;
        state.user("followed result");
        assert_eq!(state.new_events, 0);
    }

    #[test]
    fn prepending_history_records_wrap_aware_anchor() {
        let mut state = state();
        state.total_wrapped_rows = 42;
        let mut older = TimelineEvent::new(
            SessionId::from("local"),
            TurnId::from("older"),
            TraceId::from("older"),
            SpanId::generate(),
            None,
            LifecyclePhase::Message,
            TimelineStatus::Completed,
            "older event",
            TimelineKind::Notice {
                text: "older event".into(),
                severity: "info".into(),
            },
        );
        older.sequence = 0;
        state.prepend_events(vec![older]);
        assert_eq!(state.prepend_anchor_rows, Some(42));
        assert_eq!(state.timeline.first().map(|event| event.sequence), Some(0));
    }

    #[test]
    fn search_matches_respect_the_selected_filter() {
        let mut state = state();
        state.user("needle in a user message");
        state.push_local_event(
            TimelineStatus::Completed,
            "needle tool".into(),
            TimelineKind::ToolProgress {
                tool: "repo.search".into(),
                message: "needle in tool progress".into(),
                completed_units: Some(1),
                total_units: Some(1),
            },
        );
        state.search_query = Some("needle".into());
        state.transcript_filter = TranscriptFilter::Messages;
        state.refresh_search_matches();
        assert_eq!(state.search_matches.len(), 1);
        state.transcript_filter = TranscriptFilter::Tools;
        state.refresh_search_matches();
        assert_eq!(state.search_matches.len(), 1);
    }

    #[test]
    fn focus_cycle_keeps_permission_key_binding_separate() {
        assert_eq!(Focus::Input.next(), Focus::Timeline);
        assert_eq!(Focus::Timeline.next(), Focus::Context);
        assert_eq!(Focus::Context.next(), Focus::Drawer);
        assert_eq!(Focus::Drawer.next(), Focus::Input);
    }
}
