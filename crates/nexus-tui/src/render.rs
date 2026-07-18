//! Rendering. Responsive NEXUS layout: identity header, transcript,
//! ACTIVE CONTEXT rail, input line, segmented status footer, plus the overlay
//! stack (palette, menus, forms, pagers, progress, approvals, toasts).
//!
//! Breakpoints: wide (≥100 cols) shows the context rail; medium (≥64) shows
//! a compact activity strip; narrow stacks a single column. Panels never
//! overlap and text never renders outside the terminal.

use crate::state::{Focus, Mode, State, WrapLayoutCacheEntry};
use crate::theme::Theme;
use crate::views::{Form, Menu, Overlay, Pager, Palette, Progress, SecretInput, SummaryPreview};
use nexus_app::{Item, Report, Sev};
use nexus_core::brand::{self, BrandConstraints, BrandLockup, BrandRole, BrandVariant};
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Frame;
use std::hash::{Hash, Hasher};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const SPINNER: [&str; 4] = ["▖", "▘", "▝", "▗"];

fn brand_role_style(role: BrandRole, t: &Theme, monochrome: bool) -> Style {
    if monochrome {
        return match role {
            BrandRole::Icon | BrandRole::Wordmark => t.text().add_modifier(Modifier::BOLD),
            BrandRole::Attribution | BrandRole::Tagline | BrandRole::Spacer => t.muted(),
        };
    }
    match role {
        BrandRole::Icon => t.secondary().add_modifier(Modifier::BOLD),
        BrandRole::Wordmark => t.brand(),
        BrandRole::Attribution | BrandRole::Tagline => t.muted(),
        BrandRole::Spacer => t.text(),
    }
}

fn styled_brand_lines(lockup: &BrandLockup, t: &Theme, available_width: u16) -> Vec<Line<'static>> {
    let outer_pad = available_width.saturating_sub(lockup.width) / 2;
    lockup
        .lines
        .iter()
        .map(|line| {
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            if outer_pad > 0 {
                spans.push(Span::raw(" ".repeat(outer_pad as usize)));
            }
            spans.extend(line.spans.iter().map(|span| {
                Span::styled(
                    span.text.clone(),
                    brand_role_style(span.role, t, lockup.monochrome),
                )
            }));
            Line::from(spans)
        })
        .collect()
}

pub fn draw(f: &mut Frame, st: &mut State) {
    let t = st.theme;
    let area = f.area();
    let wide = area.width >= 100;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(3),    // body
            Constraint::Length(3), // input
            Constraint::Length(1), // footer
        ])
        .split(area);

    draw_header(f, rows[0], st, &t);

    if wide {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(40), Constraint::Length(36)])
            .split(rows[1]);
        draw_transcript(f, body[0], st, &t);
        draw_context_rail(f, body[1], st, &t);
    } else {
        draw_transcript(f, rows[1], st, &t);
    }

    draw_input(f, rows[2], st, &t);
    draw_footer(f, rows[3], st, &t);

    // Overlay stack: render every overlay, topmost last.
    for overlay in &st.overlays {
        draw_overlay(f, area, overlay, st, &t);
    }

    if st.pending.is_some() {
        draw_approval(f, area, st, &t);
    }

    if !wide && st.context_drawer {
        draw_context_drawer(f, area, st, &t);
    }
    if st.agent_drawer {
        draw_agent_drawer(f, area, st, &t);
    }

    draw_toasts(f, area, st, &t);
}

// -------------------------------------------------------------------- header

/// Abbreviate a filesystem path for the chrome: `$HOME` → `~`, and
/// middle-truncated from the left when it exceeds `max` (the tail — the part
/// that identifies where you are — always survives).
pub fn abbreviate_path(path: &str, max: usize) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut p = if !home.is_empty() && path.starts_with(&home) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_string()
    };
    let count = p.chars().count();
    if count > max && max > 4 {
        let tail: String = p.chars().skip(count - (max - 1)).collect();
        p = format!("…{tail}");
    }
    p
}

fn draw_header(f: &mut Frame, area: Rect, st: &State, t: &Theme) {
    let compact = area.width < 64;
    // The operator always sees where snx was launched from.
    let ws = abbreviate_path(&st.bar.workspace, if compact { 24 } else { 40 });
    let lockup = brand::lockup(
        BrandVariant::Compact,
        BrandConstraints {
            width: if compact { 9 } else { 19 },
            height: 1,
            unicode: brand::unicode_supported(),
        },
    );
    let mut spans = vec![Span::styled(" ", t.text())];
    if let Some(line) = lockup.lines.first() {
        spans.extend(line.spans.iter().map(|span| {
            Span::styled(
                span.text.clone(),
                brand_role_style(span.role, t, lockup.monochrome),
            )
        }));
    }
    spans.push(Span::styled("  ", t.text()));
    spans.push(Span::styled(ws, t.muted()));
    if !compact {
        spans.push(Span::styled("  MODEL ", t.muted()));
        spans.push(Span::styled(
            st.bar.model_label.clone(),
            if st.bar.model_ok {
                t.primary()
            } else {
                t.warning()
            },
        ));
        spans.push(Span::styled("  AGENT ", t.muted()));
        spans.push(Span::styled(st.bar.agent.clone(), t.secondary()));
        spans.push(Span::styled("  SANDBOX ", t.muted()));
        spans.push(Span::styled(
            st.bar.sandbox_level.clone(),
            t.sandbox(&st.bar.sandbox_level),
        ));
    }
    if st.busy > 0 || st.mode == Mode::Running {
        let frame = if st.reduced_motion {
            "▪"
        } else {
            SPINNER[st.spinner % SPINNER.len()]
        };
        spans.push(Span::styled(format!("  {frame} "), t.primary()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ------------------------------------------------------------ processing fx

/// Neon "data stream" shown while the agent works: a pulse of shaded blocks
/// sweeping a fixed track, glitch mark up front. Honest and quiet under
/// reduced motion.
fn processing_line(st: &State, t: &Theme) -> Line<'static> {
    if st.reduced_motion {
        return Line::from(Span::styled("  ▪ NEXUS PROCESSING", t.secondary()));
    }
    const TRACK: usize = 18;
    const PULSE: [char; 7] = ['░', '▒', '▓', '█', '▓', '▒', '░'];
    let tick = st.spinner;
    let head = tick % (TRACK + PULSE.len());
    let mut cells = vec![' '; TRACK];
    for (i, c) in PULSE.iter().enumerate() {
        let pos = head as isize - i as isize;
        if (0..TRACK as isize).contains(&pos) {
            cells[pos as usize] = *c;
        }
    }
    let track: String = cells.into_iter().collect();
    let glyph = ['◢', '◣', '◤', '◥'][tick % 4];
    Line::from(vec![
        Span::styled(format!("  {glyph} "), t.secondary()),
        Span::styled("NEXUS PROCESSING ", t.primary()),
        Span::styled(track, t.secondary()),
        Span::styled(["⟨", "⟩", "⟨", "⟩"][(tick / 2) % 4].to_string(), t.muted()),
    ])
}

// ---------------------------------------------------------------- transcript

fn draw_transcript(f: &mut Frame, area: Rect, st: &mut State, t: &Theme) {
    let inner_width = area.width.saturating_sub(3).max(1) as usize;
    let mut next_row = usize::from(st.has_older_events);
    let mut layouts = Vec::new();
    st.event_row_offsets.clear();
    for (index, event) in st.timeline.iter().enumerate() {
        if !st.transcript_filter.matches(event) {
            continue;
        }
        if matches!(
            event.kind,
            nexus_core::timeline::TimelineKind::ReasoningSummary { .. }
        ) && !st.thinking_enabled
        {
            continue;
        }
        let selected = st.selected_event == Some(index) && st.focus == Focus::Timeline;
        st.event_row_offsets.insert(event.id.clone(), next_row);
        let expanded = st.detail_level != nexus_core::timeline::TranscriptDetail::Compact
            || st.collapsed_cards.contains(&event.id);
        let signature = event_layout_signature(event, st.detail_level, expanded, inner_width);
        let rows = match st.wrap_layout_cache.get(&event.id) {
            Some(cached)
                if cached.signature == signature
                    && cached.width == inner_width
                    && cached.detail == st.detail_level
                    && cached.expanded == expanded =>
            {
                cached.rows
            }
            _ => {
                let rows =
                    event_lines(event, st.detail_level, expanded, selected, inner_width, t).len();
                st.wrap_layout_cache.insert(
                    event.id.clone(),
                    WrapLayoutCacheEntry {
                        signature,
                        width: inner_width,
                        detail: st.detail_level,
                        expanded,
                        rows,
                    },
                );
                rows
            }
        };
        layouts.push((index, next_row, rows, expanded));
        next_row = next_row.saturating_add(rows);
    }
    let processing_row = (st.mode == Mode::Running).then_some(next_row);
    next_row = next_row.saturating_add(usize::from(processing_row.is_some()));
    let new_events_row = (st.new_events > 0).then_some(next_row);
    next_row = next_row.saturating_add(usize::from(new_events_row.is_some()));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(if st.focus == Focus::Timeline {
            t.primary()
        } else {
            t.muted()
        })
        .title(Span::styled(
            format!(
                " TIMELINE · {} · {} ",
                st.transcript_filter.as_str(),
                st.detail_level.as_str()
            ),
            t.primary(),
        ));
    let inner_h = area.height.saturating_sub(2) as usize;
    let total = next_row;
    if let Some(before) = st.prepend_anchor_rows.take() {
        st.scroll = st.scroll.saturating_add(total.saturating_sub(before));
    }
    st.total_wrapped_rows = total;
    st.viewport_rows = inner_h;
    let max_scroll = total.saturating_sub(inner_h);
    let scroll = if st.follow {
        max_scroll
    } else {
        st.scroll.min(max_scroll)
    };
    st.scroll = scroll;
    let visible_end = scroll.saturating_add(inner_h).min(total);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(inner_h);
    if st.has_older_events && scroll == 0 {
        lines.push(Line::from(Span::styled(
            "↑ older events available · PgUp/Home loads history",
            t.muted(),
        )));
    }
    for (index, offset, rows, expanded) in layouts {
        let event_end = offset.saturating_add(rows);
        if event_end <= scroll || offset >= visible_end {
            continue;
        }
        let event = &st.timeline[index];
        let selected = st.selected_event == Some(index) && st.focus == Focus::Timeline;
        let rendered = event_lines(event, st.detail_level, expanded, selected, inner_width, t);
        let from = scroll.saturating_sub(offset).min(rendered.len());
        let to = visible_end
            .saturating_sub(offset)
            .min(rendered.len())
            .max(from);
        lines.extend(rendered[from..to].iter().cloned());
    }
    if processing_row.is_some_and(|row| row >= scroll && row < visible_end) {
        lines.push(processing_line(st, t));
    }
    if new_events_row.is_some_and(|row| row >= scroll && row < visible_end) {
        lines.push(Line::from(Span::styled(
            format!("↓ {} NEW EVENTS · End to follow", st.new_events),
            t.warning().add_modifier(Modifier::BOLD),
        )));
    }
    let p = Paragraph::new(Text::from(lines)).block(block);
    f.render_widget(p, area);
    if total > inner_h {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"))
            .thumb_symbol("█")
            .track_symbol(Some("│"));
        let mut scrollbar_state = ScrollbarState::new(total)
            .position(scroll)
            .viewport_content_length(inner_h);
        f.render_stateful_widget(
            scrollbar,
            area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

fn event_layout_signature(
    event: &nexus_core::timeline::TimelineEvent,
    detail: nexus_core::timeline::TranscriptDetail,
    expanded: bool,
    width: usize,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(event)
        .unwrap_or_else(|_| event.summary.clone())
        .hash(&mut hasher);
    detail.as_str().hash(&mut hasher);
    expanded.hash(&mut hasher);
    width.hash(&mut hasher);
    hasher.finish()
}

fn event_lines(
    event: &nexus_core::timeline::TimelineEvent,
    detail: nexus_core::timeline::TranscriptDetail,
    expanded: bool,
    selected: bool,
    width: usize,
    t: &Theme,
) -> Vec<Line<'static>> {
    use nexus_core::timeline::{TimelineKind, TimelineStatus};

    let (glyph, status_label, status_style) = match event.status {
        TimelineStatus::Pending => ("◇", "PENDING", t.muted()),
        TimelineStatus::Running => ("◆", "RUNNING", t.primary()),
        TimelineStatus::Completed => ("✓", "DONE", t.success()),
        TimelineStatus::Failed => ("✗", "FAILED", t.failure()),
        TimelineStatus::Blocked => ("■", "BLOCKED", t.failure()),
        TimelineStatus::Cancelled => ("×", "CANCELLED", t.warning()),
        TimelineStatus::Skipped => ("–", "SKIPPED", t.muted()),
        TimelineStatus::Waiting => ("◫", "WAITING", t.warning()),
    };
    let label = event
        .kind
        .type_label()
        .replace('_', " ")
        .to_ascii_uppercase();
    let mut meta = String::new();
    if let Some(risk) = &event.risk {
        meta.push_str(&format!(" · {risk}"));
    }
    if let Some(duration) = event.duration_ms {
        meta.push_str(&format!(" · {}", human_duration(duration)));
    }
    let header_style = if selected {
        t.selection()
    } else {
        status_style
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{glyph} {status_label:<9}"), header_style),
        Span::styled(format!(" {label}{meta}"), t.muted()),
    ])];
    // Message-like events use `summary` as searchable/indexed metadata (often
    // the first body line). Rendering both fields duplicated one-line messages
    // and repeated the first line of multiline messages. The body is the sole
    // display source; summary is only a fallback when that body is empty.
    let has_primary_body = match &event.kind {
        TimelineKind::UserMessage { text }
        | TimelineKind::AssistantMessage { text, .. }
        | TimelineKind::FinalAnswer { text }
        | TimelineKind::ReasoningSummary { text }
        | TimelineKind::Notice { text, .. } => !text.trim().is_empty(),
        TimelineKind::Error { message, .. }
        | TimelineKind::Retry {
            reason: message, ..
        }
        | TimelineKind::ProviderLimit { message, .. } => !message.trim().is_empty(),
        _ => false,
    };
    if !has_primary_body {
        push_wrapped(&mut lines, &event.summary, width, t.text());
    }

    match &event.kind {
        TimelineKind::UserMessage { text } => {
            push_wrapped(&mut lines, text, width, t.user());
        }
        TimelineKind::AssistantMessage { text, .. } | TimelineKind::FinalAnswer { text } => {
            push_wrapped(&mut lines, text, width, t.text());
        }
        TimelineKind::ReasoningSummary { text } => {
            push_wrapped(&mut lines, text, width, t.muted());
        }
        TimelineKind::Notice { text, severity } => {
            let style = match severity.as_str() {
                "ok" => t.success(),
                "warning" => t.warning(),
                "error" => t.failure(),
                "dim" => t.muted(),
                _ => t.text(),
            };
            if !text.trim().is_empty() {
                push_wrapped(&mut lines, text, width, style);
            }
        }
        TimelineKind::ToolExecution {
            output_preview,
            affected_paths,
            ..
        } if !output_preview.is_empty() => {
            if !affected_paths.is_empty() {
                push_wrapped(
                    &mut lines,
                    &format!("paths: {}", affected_paths.join(", ")),
                    width,
                    t.warning(),
                );
            }
            push_wrapped(
                &mut lines,
                &truncate(
                    output_preview,
                    if expanded {
                        2_000
                    } else {
                        width.saturating_mul(2)
                    },
                ),
                width,
                t.muted(),
            );
        }
        TimelineKind::Error { message, .. } => {
            push_wrapped(&mut lines, message, width, t.failure());
        }
        TimelineKind::Retry { reason, .. }
        | TimelineKind::ProviderLimit {
            message: reason, ..
        } => {
            push_wrapped(&mut lines, reason, width, t.warning());
        }
        TimelineKind::WorkBreakdown { breakdown } => {
            let (done, total) = breakdown.progress();
            push_wrapped(
                &mut lines,
                &format!(
                    "{} · plan v{} · {done}/{total} stages{}",
                    breakdown.kind.as_str(),
                    breakdown.version,
                    if breakdown.approved {
                        ""
                    } else {
                        " · approval required"
                    }
                ),
                width,
                if breakdown.approved {
                    t.success()
                } else {
                    t.warning()
                },
            );
            if expanded {
                for stage in &breakdown.stages {
                    push_wrapped(
                        &mut lines,
                        &format!(
                            "{}. [{}] {} — {}",
                            stage.sequence,
                            stage.status.as_str(),
                            stage.title,
                            stage.description
                        ),
                        width,
                        t.muted(),
                    );
                }
            }
        }
        TimelineKind::Diff {
            path,
            insertions,
            deletions,
            preview,
        } => {
            let header = match path {
                Some(path) => format!("▸ {path}  (+{insertions} −{deletions})"),
                None => format!("(+{insertions} −{deletions})"),
            };
            lines.push(Line::from(Span::styled(header, t.secondary())));
            let cap = if expanded { 400 } else { 40 };
            for source_line in preview.lines().take(cap) {
                let style = match source_line.as_bytes().first() {
                    Some(b'+') => t.success(),
                    Some(b'-') => t.failure(),
                    Some(b'@') if source_line.starts_with("@@") => t.primary(),
                    _ => t.muted(),
                };
                push_wrapped(&mut lines, source_line, width, style);
            }
        }
        _ => {}
    }

    if expanded && detail != nexus_core::timeline::TranscriptDetail::Raw {
        let payload = serde_json::to_string_pretty(&event.kind).unwrap_or_default();
        push_wrapped(&mut lines, &payload, width, t.muted());
    }
    if detail == nexus_core::timeline::TranscriptDetail::Raw {
        let raw = serde_json::to_string_pretty(event).unwrap_or_default();
        push_wrapped(&mut lines, &raw, width, t.muted());
    }
    for artifact in &event.artifact_refs {
        push_wrapped(
            &mut lines,
            &format!(
                "artifact {} · {}{}",
                artifact.id,
                artifact.label,
                artifact
                    .bytes
                    .map(|bytes| format!(" · {bytes} bytes"))
                    .unwrap_or_default()
            ),
            width,
            t.secondary(),
        );
    }
    lines.push(Line::from(""));
    lines
}

fn push_wrapped(lines: &mut Vec<Line<'static>>, text: &str, width: usize, style: Style) {
    let width = width.max(1);
    if text.is_empty() {
        return;
    }
    for source_line in text.lines() {
        if source_line.is_empty() {
            lines.push(Line::from(""));
            continue;
        }
        for wrapped in wrap_terminal_line(source_line, width) {
            lines.push(Line::from(Span::styled(wrapped, style)));
        }
    }
}

fn wrap_terminal_line(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_width > 0 && current_width.saturating_add(ch_width) > width {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push(ch);
        current_width = current_width.saturating_add(ch_width);
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut value: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        value.push('…');
    }
    value
}

fn human_duration(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        format!("{duration_ms}ms")
    } else if duration_ms < 60_000 {
        format!("{:.1}s", duration_ms as f64 / 1_000.0)
    } else {
        format!("{}m{}s", duration_ms / 60_000, duration_ms / 1_000 % 60)
    }
}

// -------------------------------------------------------------- context rail

fn draw_context_rail(f: &mut Frame, area: Rect, st: &State, t: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(t.muted())
        .title(Span::styled(" ACTIVE CONTEXT ", t.secondary()));
    let mut lines: Vec<Line> = Vec::new();
    let kv = |k: &str, v: String, style: Style| -> Line<'static> {
        Line::from(vec![
            Span::styled(format!("{k:>9} "), t.muted()),
            Span::styled(v, style),
        ])
    };
    let active = &st.active_work;
    lines.push(kv(
        "session",
        active
            .session_id
            .as_ref()
            .map(|id| id.as_str().to_string())
            .unwrap_or_else(|| "none".into()),
        if active.session_id.is_some() {
            t.text()
        } else {
            t.muted()
        },
    ));
    if !active.session_title.is_empty() {
        lines.push(kv("title", truncate(&active.session_title, 24), t.text()));
    }
    lines.push(kv(
        "git",
        match (&active.branch, &active.head) {
            (Some(branch), Some(head)) => format!("{branch} @ {head}"),
            (Some(branch), None) => branch.clone(),
            _ => "not a repository".into(),
        },
        if active.branch.is_some() {
            t.secondary()
        } else {
            t.muted()
        },
    ));
    lines.push(kv(
        "model",
        if active.provider.is_empty() {
            st.bar.model_label.clone()
        } else {
            format!("{} / {}", active.provider, active.model)
        },
        t.primary(),
    ));
    lines.push(kv(
        "agent",
        if active.agent.is_empty() {
            st.bar.agent.clone()
        } else {
            active.agent.clone()
        },
        t.secondary(),
    ));
    lines.push(kv("mode", active.permission_mode.clone(), t.text()));
    if let Some(objective) = &active.objective {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("OBJECTIVE", t.muted())));
        push_wrapped(
            &mut lines,
            &truncate(objective, 180),
            area.width.saturating_sub(4) as usize,
            t.text(),
        );
    }
    if let Some(work) = &active.work {
        let (done, total) = work.progress();
        lines.push(Line::from(""));
        lines.push(kv(
            "plan",
            format!("{} v{} · {done}/{total}", work.kind.as_str(), work.version),
            if work.approved {
                t.success()
            } else {
                t.warning()
            },
        ));
        if let Some(current) = work
            .current_stage
            .as_ref()
            .and_then(|id| work.stages.iter().find(|stage| &stage.id == id))
        {
            lines.push(kv("current", truncate(&current.title, 24), t.warning()));
        }
        if let Some(next) = work
            .next_stage
            .as_ref()
            .and_then(|id| work.stages.iter().find(|stage| &stage.id == id))
        {
            lines.push(kv("next", truncate(&next.title, 24), t.muted()));
        }
    }
    if let Some(tool) = &active.active_foreground_tool {
        lines.push(kv("tool", truncate(tool, 24), t.warning()));
    }
    lines.push(kv(
        "tasks",
        format!(
            "{} bg · {} agents",
            active.background_tasks.len(),
            active.subagents.len()
        ),
        if active.background_tasks.is_empty() && active.subagents.is_empty() {
            t.muted()
        } else {
            t.secondary()
        },
    ));
    lines.push(kv(
        "files",
        format!(
            "{} mod · {} staged · {} new",
            active.modified_files.len(),
            active.staged_files.len(),
            active.untracked_files.len()
        ),
        if active.modified_files.is_empty() {
            t.muted()
        } else {
            t.warning()
        },
    ));
    lines.push(kv(
        "diff",
        format!(
            "{} files · +{} -{}",
            active.diff.files, active.diff.insertions, active.diff.deletions
        ),
        t.text(),
    ));
    lines.push(kv(
        "checks",
        format!(
            "{} ok · {} pending · {} failed",
            active.validation_completed.len(),
            active.validation_pending.len(),
            active.validation_failed.len()
        ),
        if active.validation_failed.is_empty() {
            t.success()
        } else {
            t.failure()
        },
    ));
    lines.push(kv(
        "approval",
        active.waiting_approvals.len().to_string(),
        if active.waiting_approvals.is_empty() {
            t.muted()
        } else {
            t.warning()
        },
    ));
    lines.push(kv(
        "context",
        format!(
            "{}{} / {} · c{}",
            if active.context.estimated { "≈" } else { "" },
            active.context.input_tokens,
            active.context.context_window,
            active.context.compaction_count
        ),
        t.text(),
    ));
    if let Some(retry) = &active.retry_state {
        lines.push(kv("retry", truncate(retry, 24), t.warning()));
    }
    for blocker in active.blockers.iter().take(3) {
        lines.push(kv("blocker", truncate(blocker, 24), t.failure()));
    }
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_context_drawer(f: &mut Frame, area: Rect, st: &State, t: &Theme) {
    let width = area.width.saturating_mul(3) / 4;
    let drawer = Rect::new(
        area.right().saturating_sub(width),
        area.y + 1,
        width,
        area.height.saturating_sub(2),
    );
    f.render_widget(Clear, drawer);
    draw_context_rail(f, drawer, st, t);
}

fn draw_agent_drawer(f: &mut Frame, area: Rect, st: &State, t: &Theme) {
    let width = area.width.saturating_mul(2) / 3;
    let drawer = Rect::new(area.x, area.y + 1, width, area.height.saturating_sub(2));
    f.render_widget(Clear, drawer);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(t.secondary())
        .title(Span::styled(" AGENTS / SESSIONS ", t.secondary()));
    let mut lines = Vec::new();
    if st.active_work.subagents.is_empty() && st.active_work.background_tasks.is_empty() {
        lines.push(Line::from(Span::styled(
            "No active agents or background tasks.",
            t.muted(),
        )));
    }
    for agent in &st.active_work.subagents {
        lines.push(Line::from(vec![
            Span::styled(
                if agent.status == "running" {
                    "◆ "
                } else {
                    "◇ "
                },
                if agent.status == "running" {
                    t.secondary()
                } else {
                    t.muted()
                },
            ),
            Span::styled(agent.role.clone(), t.text()),
            Span::styled(
                format!(
                    " · {} · {} · {}s · {} unread{}",
                    agent.status,
                    agent.model,
                    agent.duration_ms / 1_000,
                    agent.unread_events,
                    if agent.waiting_approval {
                        " · APPROVAL"
                    } else {
                        ""
                    }
                ),
                t.muted(),
            ),
        ]));
    }
    for task in &st.active_work.background_tasks {
        lines.push(Line::from(vec![
            Span::styled(if task.writer { "W " } else { "R " }, t.warning()),
            Span::styled(task.title.clone(), t.text()),
            Span::styled(
                format!(" · {} · {}s", task.status, task.duration_ms / 1_000),
                t.muted(),
            ),
        ]));
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        drawer,
    );
}

// --------------------------------------------------------------------- input

fn draw_input(f: &mut Frame, area: Rect, st: &State, t: &Theme) {
    let (title, border) = if st.search_edit.is_some() {
        (
            " search timeline · Enter apply · Esc cancel ",
            t.secondary(),
        )
    } else if st.mode == Mode::Running {
        (" running — input queued after turn ", t.warning())
    } else if st.focus != Focus::Input {
        (" F6 focus · input inactive ", t.muted())
    } else {
        (" message · / commands · Ctrl+K palette ", t.primary())
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(Span::styled(title, border));
    let text = st.search_edit.as_deref().unwrap_or_else(|| st.input.text());
    let display: String = text.replace('\n', " ⏎ ");
    let content = Line::from(vec![
        Span::styled(
            if st.search_edit.is_some() {
                "⌕ "
            } else {
                "❯ "
            },
            t.secondary(),
        ),
        Span::styled(display, t.text()),
    ]);
    f.render_widget(Paragraph::new(content).block(block), area);

    // Place the terminal cursor at the edit position (single-row rendering).
    if st.overlays.is_empty() && st.pending.is_none() && st.focus == Focus::Input {
        let cursor = if st.search_edit.is_some() {
            text.len()
        } else {
            st.input.cursor()
        };
        let before = &text[..cursor];
        let width = UnicodeWidthStr::width(before.replace('\n', " ⏎ ").as_str()) as u16;
        let x = area.x + 3 + width.min(area.width.saturating_sub(4));
        let y = area.y + 1;
        f.set_cursor_position((x, y));
    }
}

// -------------------------------------------------------------------- footer

fn draw_footer(f: &mut Frame, area: Rect, st: &State, t: &Theme) {
    if st.pending.is_some() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " approval: ↑/↓ choose · Enter · [n]o · [e]dit ",
                t.warning(),
            ))),
            area,
        );
        return;
    }
    let sep = Span::styled(" │ ", t.muted());
    let mut spans: Vec<Span> = Vec::new();
    let seg = |label: &str, value: String, style: Style| -> Vec<Span<'static>> {
        vec![
            Span::styled(format!(" {label} "), t.muted()),
            Span::styled(value, style),
        ]
    };
    if let Some(branch) = &st.bar.git_branch {
        spans.extend(seg("BRANCH", branch.clone(), t.secondary()));
        spans.push(sep.clone());
    }
    spans.extend(seg(
        "MODEL",
        st.bar.model_label.clone(),
        if st.bar.model_ok {
            t.primary()
        } else {
            t.warning()
        },
    ));
    spans.push(sep.clone());
    spans.extend(seg(
        "TOKENS",
        format!("{}↑ {}↓", st.bar.tokens_in, st.bar.tokens_out),
        t.text(),
    ));
    spans.push(sep.clone());
    spans.extend(seg("MODE", st.bar.permission_mode.clone(), t.secondary()));
    spans.push(sep.clone());
    spans.extend(seg("NET", st.bar.network.clone(), t.text()));
    spans.push(sep.clone());
    spans.extend(seg(
        "VIEW",
        format!(
            "{}·{}",
            st.transcript_filter.as_str(),
            st.detail_level.as_str()
        ),
        t.muted(),
    ));
    spans.push(sep.clone());
    if !st.search_matches.is_empty() {
        spans.extend(seg(
            "MATCH",
            format!(
                "{}/{}",
                st.search_match_index.saturating_add(1),
                st.search_matches.len()
            ),
            t.secondary(),
        ));
        spans.push(sep.clone());
    }
    let (state_label, state_style) = if st.mode == Mode::Running {
        let label = if st.reduced_motion {
            "RUNNING".to_string()
        } else {
            // Neon shimmer: the block after RUNNING cycles the pulse ramp.
            const RAMP: [char; 4] = ['░', '▒', '▓', '█'];
            format!("RUNNING {}", RAMP[st.spinner % RAMP.len()])
        };
        (label, t.warning())
    } else if st.busy > 0 {
        ("LOADING".to_string(), t.secondary())
    } else {
        ("READY".to_string(), t.success())
    };
    spans.extend(seg("STATUS", state_label, state_style));
    if st.new_events > 0 {
        spans.push(sep.clone());
        spans.extend(seg(
            "NEW",
            st.new_events.to_string(),
            t.warning().add_modifier(Modifier::BOLD),
        ));
    }
    if area.width >= 100 {
        spans.push(sep);
        spans.push(Span::styled(
            " Enter send · PgUp/PgDn scroll · ? help ",
            t.muted(),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ------------------------------------------------------------------ overlays

fn overlay_rect(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2));
    let h = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

fn draw_overlay(f: &mut Frame, area: Rect, overlay: &Overlay, st: &State, t: &Theme) {
    match overlay {
        Overlay::Palette(p) => draw_palette(f, area, p, t),
        Overlay::Menu(m) => draw_menu(f, area, m, t),
        Overlay::Confirm(c) => draw_confirm(f, area, c, t),
        Overlay::Secret(s) => draw_secret(f, area, s, t),
        Overlay::Form(form) => draw_form(f, area, form, t),
        Overlay::Pager(p) => draw_pager(f, area, p, t),
        Overlay::Progress(p) => draw_progress(f, area, p, st, t),
        Overlay::Summary(s) => draw_summary(f, area, s, t),
    }
}

fn draw_palette(f: &mut Frame, area: Rect, p: &Palette, t: &Theme) {
    let rect = overlay_rect(area, 74, 18);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(t.primary())
        .title(Span::styled(" command palette ", t.brand()));
    let mut lines = vec![
        Line::from(vec![
            Span::styled("❯ ", t.secondary()),
            Span::styled(p.query.clone(), t.text()),
            Span::styled("▏", t.primary()),
        ]),
        Line::from(""),
    ];
    let hits = p.matches();
    let visible_rows = rect.height.saturating_sub(5) as usize;
    let first = p.selected.saturating_sub(visible_rows.saturating_sub(1));
    for (i, def) in hits.iter().enumerate().skip(first).take(visible_rows) {
        let selected = i == p.selected;
        let style = if selected { t.selection() } else { t.text() };
        let usage = if def.usage.is_empty() {
            String::new()
        } else {
            format!(" {}", def.usage)
        };
        lines.push(Line::from(vec![
            Span::styled(if selected { "▸ " } else { "  " }, t.primary()),
            Span::styled(format!("/{:<12}", def.name), style),
            Span::styled(format!("{:<10}", def.category.label()), t.secondary()),
            Span::styled(format!("{}{usage}", def.summary), t.muted()),
        ]));
    }
    if hits.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matching commands",
            t.muted(),
        )));
    }
    lines.push(Line::from(Span::styled(
        " Enter run · Tab insert · Esc close",
        t.muted(),
    )));
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), rect);
}

fn draw_menu(f: &mut Frame, area: Rect, m: &Menu, t: &Theme) {
    let visible = m.visible();
    let rect_width = 78.min(area.width.saturating_sub(2)).max(1);
    let content_width = rect_width.saturating_sub(4);
    let brand_lockup = m.brand.map(|variant| {
        brand::lockup(
            variant,
            BrandConstraints {
                width: content_width,
                height: area.height.saturating_sub(8),
                unicode: brand::unicode_supported(),
            },
        )
    });
    let brand_rows = brand_lockup
        .as_ref()
        .map(|lockup| lockup.height.saturating_add(1))
        .unwrap_or(0);
    let body_rows: u16 = if m.loading || m.error.is_some() || visible.is_empty() {
        1
    } else {
        visible.iter().map(|&i| menu_item_height(&m.items[i])).sum()
    };
    let search_rows = u16::from(m.searchable);
    let state_rows = u16::from(!m.filters.is_empty() || m.sort.is_some())
        .saturating_add(u16::from(m.detail_preview.is_some()))
        .saturating_add(if m.help_visible { 3 } else { 0 });
    let wanted_height = body_rows
        .saturating_add(brand_rows)
        .saturating_add(search_rows)
        .saturating_add(state_rows)
        .saturating_add(4);
    let max_height = area.height.saturating_sub(2).max(1);
    let height = wanted_height.min(max_height).max(1);
    let rect = overlay_rect(area, 78, height);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(t.primary())
        .title(Span::styled(format!(" {} ", m.title), t.brand()));
    let mut lines: Vec<Line> = Vec::new();
    if let Some(lockup) = &brand_lockup {
        lines.extend(styled_brand_lines(lockup, t, rect.width.saturating_sub(2)));
        lines.push(Line::from(""));
    }
    if m.searchable {
        lines.push(Line::from(vec![
            Span::styled(
                if m.focused_region == crate::views::MenuFocusRegion::Search {
                    "search▸ "
                } else {
                    "search  "
                },
                t.muted(),
            ),
            Span::styled(m.filter.clone(), t.text()),
            Span::styled(if m.search_mode { "▏" } else { "" }, t.primary()),
        ]));
    }
    if !m.filters.is_empty() || m.sort.is_some() {
        let filters = m
            .filters
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sort = m.sort.as_ref().map_or_else(String::new, |sort| {
            format!("sort={} {:?}", sort.field, sort.direction)
        });
        lines.push(Line::from(Span::styled(
            [filters, sort]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" · "),
            t.secondary(),
        )));
    }
    let width = rect.width.saturating_sub(4) as usize;
    let fixed_rows = brand_rows
        .saturating_add(search_rows)
        .saturating_add(state_rows)
        .saturating_add(2);
    let item_budget = rect.height.saturating_sub(2).saturating_sub(fixed_rows) as usize;
    let (first, last) = menu_viewport(m, &visible, item_budget);
    let mut remaining_rows = item_budget;
    if m.loading {
        lines.push(Line::from(Span::styled("  loading…", t.primary())));
        remaining_rows = remaining_rows.saturating_sub(1);
    } else if let Some(error) = &m.error {
        lines.push(Line::from(Span::styled(
            format!("  error: {error}"),
            t.failure(),
        )));
        remaining_rows = remaining_rows.saturating_sub(1);
    }
    for (position, &idx) in visible
        .iter()
        .enumerate()
        .take(last)
        .skip(first)
        .filter(|_| !m.loading && m.error.is_none())
    {
        if remaining_rows == 0 {
            break;
        }
        let item = &m.items[idx];
        let selected = position == m.selected;
        let base = if item.disabled.is_some() {
            t.muted()
        } else if selected {
            t.selection()
        } else {
            t.text()
        };
        let toggled = m.toggled_item_ids.contains(&item.id);
        let marker = match (selected, toggled) {
            (true, true) => "▸✓",
            (true, false) => "▸ ",
            (false, true) => " ✓",
            (false, false) => "  ",
        };
        let badge = if let Some(reason) = &item.disabled {
            format!("[{reason}]")
        } else {
            item.badge.clone()
        };
        let label_width = width.saturating_sub(UnicodeWidthStr::width(badge.as_str()) + 3);
        let mut label = item.label.clone();
        if UnicodeWidthStr::width(label.as_str()) > label_width {
            label = label
                .chars()
                .take(label_width.saturating_sub(1))
                .collect::<String>()
                + "…";
        }
        let pad = label_width.saturating_sub(UnicodeWidthStr::width(label.as_str()));
        lines.push(Line::from(vec![
            Span::styled(marker, t.primary()),
            Span::styled(label, base),
            Span::raw(" ".repeat(pad)),
            Span::styled(
                badge,
                if item.disabled.is_some() {
                    t.warning()
                } else {
                    t.secondary()
                },
            ),
        ]));
        remaining_rows = remaining_rows.saturating_sub(1);
        if !item.detail.is_empty() && remaining_rows > 0 {
            let detail = truncate_width(&item.detail, width.saturating_sub(4));
            lines.push(Line::from(Span::styled(format!("    {detail}"), t.muted())));
            remaining_rows -= 1;
        }
    }
    if visible.is_empty() && !m.loading && m.error.is_none() {
        lines.push(Line::from(Span::styled(
            format!("  {}", m.empty_message),
            t.muted(),
        )));
    }
    if let Some(preview) = &m.detail_preview {
        lines.push(Line::from(Span::styled(
            format!(
                " detail: {}",
                truncate_width(preview, width.saturating_sub(9))
            ),
            if m.focused_region == crate::views::MenuFocusRegion::Detail {
                t.selection()
            } else {
                t.muted()
            },
        )));
    }
    if m.help_visible {
        lines.push(Line::from(Span::styled(
            " ↑/↓ j/k navigate · Enter select · Space toggle",
            t.muted(),
        )));
        lines.push(Line::from(Span::styled(
            " / search · Tab/Shift+Tab focus · Ctrl+R refresh",
            t.muted(),
        )));
        lines.push(Line::from(Span::styled(
            " Esc clear/back · ? close controls",
            t.muted(),
        )));
    }
    let hint = if m.hint.is_empty() {
        format!(
            "↑/↓ j/k · Enter · / search · Tab focus:{} · ? · Esc",
            m.focused_region.label()
        )
    } else {
        m.hint.clone()
    };
    let hint = m
        .parent_route
        .as_ref()
        .map_or(hint.clone(), |parent| format!("{hint} · back:{parent}"));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(format!(" {hint}"), t.muted())));
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false }),
        rect,
    );
}

fn menu_item_height(item: &crate::views::MenuItem) -> u16 {
    if item.detail.is_empty() {
        1
    } else {
        2
    }
}

fn menu_viewport(m: &Menu, visible: &[usize], row_budget: usize) -> (usize, usize) {
    if visible.is_empty() || row_budget == 0 {
        return (0, 0);
    }
    let selected = m.selected.min(visible.len() - 1);
    let mut first = selected;
    let mut last = selected + 1;
    let mut used = menu_item_height(&m.items[visible[selected]]) as usize;
    if used > row_budget {
        return (selected, selected + 1);
    }

    let mut before_next = true;
    loop {
        let before = first.checked_sub(1).and_then(|position| {
            let height = menu_item_height(&m.items[visible[position]]) as usize;
            (used + height <= row_budget).then_some((position, height))
        });
        let after = (last < visible.len()).then(|| {
            let height = menu_item_height(&m.items[visible[last]]) as usize;
            (last, height)
        });
        let after = after.filter(|(_, height)| used + height <= row_budget);
        let choice = match (before, after, before_next) {
            (Some(before), Some(_), true) => Some((true, before)),
            (Some(_), Some(after), false) => Some((false, after)),
            (Some(before), None, _) => Some((true, before)),
            (None, Some(after), _) => Some((false, after)),
            (None, None, _) => None,
        };
        let Some((is_before, (position, height))) = choice else {
            break;
        };
        used += height;
        if is_before {
            first = position;
        } else {
            last = position + 1;
        }
        before_next = !before_next;
    }
    (first, last)
}

fn truncate_width(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    let mut out = String::new();
    for ch in text.chars() {
        let next = format!("{out}{ch}");
        if UnicodeWidthStr::width(next.as_str()) >= max_width {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

fn draw_confirm(f: &mut Frame, area: Rect, c: &crate::views::Confirm, t: &Theme) {
    let wanted = (c.body.len() as u16 + 7).min(22);
    let rect = overlay_rect(area, 76, wanted);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(t.warning())
        .title(Span::styled(format!(" ⚠ {} ", c.title), t.warning()));
    let note_rows = if c.note.is_some() { 2 } else { 0 };
    let available_body_rows = rect
        .height
        .saturating_sub(2) // borders
        .saturating_sub(note_rows)
        .saturating_sub(3) as usize; // blank + controls
    let start = c.scroll.min(c.body.len().saturating_sub(1));
    let body_budget = if start + available_body_rows < c.body.len() {
        available_body_rows.saturating_sub(1)
    } else {
        available_body_rows
    };
    let end = (start + body_budget).min(c.body.len());
    let width = rect.width.saturating_sub(4) as usize;
    let mut lines: Vec<Line> = c
        .body
        .iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .map(|line| Line::from(Span::styled(truncate_width(line, width), t.text())))
        .collect();
    if end < c.body.len() {
        lines.push(Line::from(Span::styled(
            format!("↓ {} more line(s)", c.body.len() - end),
            t.muted(),
        )));
    }
    if let Some(note) = &c.note {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(note.clone(), t.success())));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑/↓ scroll · [y] confirm · [n]/Esc/Enter cancel (default: cancel)",
        t.primary(),
    )));
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), rect);
}

fn draw_secret(f: &mut Frame, area: Rect, s: &SecretInput, t: &Theme) {
    let rect = overlay_rect(area, 64, 8);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(t.secondary())
        .title(Span::styled(format!(" {} ", s.title), t.brand()));
    let lines = vec![
        Line::from(Span::styled(s.prompt.clone(), t.muted())),
        Line::from(""),
        Line::from(vec![
            Span::styled("❯ ", t.secondary()),
            Span::styled(s.masked(), t.text()),
            Span::styled("▏", t.primary()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Enter save · Esc cancel — the value is never echoed, logged, or kept in history",
            t.muted(),
        )),
    ];
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false }),
        rect,
    );
}

fn draw_form(f: &mut Frame, area: Rect, form: &Form, t: &Theme) {
    let rect = overlay_rect(
        area,
        76,
        (form.fields.len() as u16 * 2 + 6).min(area.height),
    );
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(t.primary())
        .title(Span::styled(format!(" {} ", form.title), t.brand()));
    let mut lines: Vec<Line> = Vec::new();
    for (i, field) in form.fields.iter().enumerate() {
        let focused = i == form.focus;
        let label_style = if focused { t.primary() } else { t.muted() };
        let value_style = if focused { t.selection() } else { t.text() };
        let cursor = if focused { "▏" } else { "" };
        lines.push(Line::from(vec![
            Span::styled(format!("{:>20}  ", field.label), label_style),
            Span::styled(field.shown_value(), value_style),
            Span::styled(cursor, t.primary()),
        ]));
        if focused {
            lines.push(Line::from(Span::styled(
                format!("{:>22}{}", "", field.hint),
                t.muted(),
            )));
        }
    }
    lines.push(Line::from(""));
    if let Some(err) = &form.error {
        lines.push(Line::from(Span::styled(format!(" ✗ {err}"), t.failure())));
    }
    let extra = if matches!(form.kind, crate::views::FormKind::CustomEndpoint) {
        " · Ctrl+T test connection"
    } else {
        ""
    };
    lines.push(Line::from(Span::styled(
        format!(" Tab/↑↓ move · Enter next/submit · Esc cancel{extra}"),
        t.muted(),
    )));
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false }),
        rect,
    );
}

/// Turn a shared [`Report`] into themed lines (used by the pager).
pub fn report_lines(
    report: &Report,
    t: &Theme,
    available_width: u16,
    available_height: u16,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    for item in &report.items {
        match item {
            Item::Brand { variant } => {
                let lockup = brand::lockup(
                    *variant,
                    BrandConstraints {
                        width: available_width,
                        height: available_height,
                        unicode: brand::unicode_supported(),
                    },
                );
                lines.extend(styled_brand_lines(&lockup, t, available_width));
                lines.push(Line::from(""));
            }
            Item::Header(h) => {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("─ {h} ─"),
                    t.secondary().add_modifier(Modifier::BOLD),
                )));
            }
            Item::Field { key, value, sev } => {
                lines.push(Line::from(vec![
                    Span::styled(format!("{key:>18}  "), t.muted()),
                    Span::styled(value.clone(), t.sev(*sev)),
                ]));
            }
            Item::Line { text, sev } => {
                let mark = match sev {
                    Sev::Ok => "✓ ",
                    Sev::Warn => "! ",
                    Sev::Err => "✗ ",
                    _ => "",
                };
                for l in text.lines() {
                    lines.push(Line::from(Span::styled(format!("{mark}{l}"), t.sev(*sev))));
                }
            }
            Item::Table { headers, rows } => {
                let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
                for row in rows {
                    for (i, cell) in row.iter().enumerate() {
                        if i < widths.len() {
                            widths[i] = widths[i].max(cell.chars().count()).min(40);
                        }
                    }
                }
                let head: Vec<String> = headers
                    .iter()
                    .enumerate()
                    .map(|(i, h)| format!("{:<w$}", h, w = widths[i]))
                    .collect();
                lines.push(Line::from(Span::styled(
                    format!("  {}", head.join("  ")),
                    t.primary().add_modifier(Modifier::BOLD),
                )));
                for row in rows {
                    let cells: Vec<String> = row
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            let w = widths.get(i).copied().unwrap_or(8);
                            let mut c = c.clone();
                            if c.chars().count() > w {
                                c = c.chars().take(w.saturating_sub(1)).collect::<String>() + "…";
                            }
                            format!("{c:<w$}")
                        })
                        .collect();
                    lines.push(Line::from(Span::styled(
                        format!("  {}", cells.join("  ")),
                        t.text(),
                    )));
                }
            }
        }
    }
    lines
}

fn draw_pager(f: &mut Frame, area: Rect, p: &Pager, t: &Theme) {
    let rect = overlay_rect(
        area,
        area.width.saturating_sub(6).min(100),
        area.height.saturating_sub(4),
    );
    f.render_widget(Clear, rect);
    let refresh = if p.refresh.is_some() {
        " · r refresh"
    } else {
        ""
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(t.primary())
        .title(Span::styled(format!(" {} ", p.title), t.brand()))
        .title_bottom(Span::styled(
            format!(" ↑↓ scroll · Esc close{refresh} "),
            t.muted(),
        ));
    let lines = report_lines(
        &p.report,
        t,
        rect.width.saturating_sub(4),
        rect.height.saturating_sub(2),
    );
    let total = lines.len() as u16;
    let inner = rect.height.saturating_sub(2);
    let scroll = p.scroll.min(total.saturating_sub(inner.min(total)));
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        rect,
    );
}

fn draw_progress(f: &mut Frame, area: Rect, p: &Progress, st: &State, t: &Theme) {
    let rect = overlay_rect(area, 70, 16);
    f.render_widget(Clear, rect);
    let border = if p.failed {
        t.failure()
    } else if p.done {
        t.success()
    } else {
        t.secondary()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(Span::styled(format!(" {} ", p.title), t.brand()));
    let mut lines: Vec<Line> = Vec::new();
    if let Some(url) = &p.url {
        lines.push(Line::from(vec![
            Span::styled("  open  ", t.muted()),
            Span::styled(url.clone(), t.primary().add_modifier(Modifier::BOLD)),
        ]));
    }
    if let Some(code) = &p.code {
        lines.push(Line::from(vec![
            Span::styled("  code  ", t.muted()),
            Span::styled(code.clone(), t.success().add_modifier(Modifier::BOLD)),
        ]));
    }
    if p.url.is_some() || p.code.is_some() {
        lines.push(Line::from(Span::styled(
            "  (select with your terminal's copy shortcut)",
            t.muted(),
        )));
        lines.push(Line::from(""));
    }
    for l in &p.lines {
        lines.push(Line::from(Span::styled(l.clone(), t.muted())));
    }
    if !p.done {
        let frame = if st.reduced_motion {
            "▪"
        } else {
            SPINNER[st.spinner % SPINNER.len()]
        };
        lines.push(Line::from(Span::styled(
            format!("  {frame} waiting… Esc cancels"),
            t.secondary(),
        )));
    } else {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            if p.failed {
                "  ✗ failed — Esc/Enter close"
            } else {
                "  ✓ done — Esc/Enter close"
            },
            if p.failed { t.failure() } else { t.success() },
        )));
    }
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false }),
        rect,
    );
}

fn draw_summary(f: &mut Frame, area: Rect, summary: &SummaryPreview, t: &Theme) {
    let rect = overlay_rect(
        area,
        area.width.saturating_sub(6).min(110),
        area.height.saturating_sub(4),
    );
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(t.secondary())
        .title(Span::styled(" session handoff preview ", t.brand()))
        .title_bottom(Span::styled(
            " c copy · Enter/r create linked rollover · Esc keep current session ",
            t.muted(),
        ));
    let mut lines = vec![
        Line::from(vec![
            Span::styled("saved  ", t.muted()),
            Span::styled(summary.path.clone(), t.text()),
        ]),
        Line::from(vec![
            Span::styled("copy   ", t.muted()),
            Span::styled(summary.clipboard_status.clone(), t.secondary()),
        ]),
        Line::from(""),
    ];
    lines.extend(
        summary
            .content
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), t.text()))),
    );
    let total = lines.len() as u16;
    let inner = rect.height.saturating_sub(2);
    let scroll = summary.scroll.min(total.saturating_sub(inner.min(total)));
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        rect,
    );
}

// ----------------------------------------------------------------- approvals

fn draw_approval(f: &mut Frame, area: Rect, st: &State, t: &Theme) {
    let Some(req) = &st.pending else { return };
    let w = area.width.saturating_sub(8).min(80);
    let h = 20u16.min(area.height.saturating_sub(4));
    let rect = overlay_rect(area, w, h);
    f.render_widget(Clear, rect);

    let mut lines = vec![
        Line::from(Span::styled(
            "Approval required",
            t.warning().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        kv(t, "tool", &req.action.tool),
        kv_styled(
            t,
            "risk",
            &req.action.risk.to_string(),
            t.risk(&req.action.risk.to_string()),
        ),
        kv(t, "summary", &req.action.summary),
    ];
    if let Some(cmd) = &req.action.command {
        lines.push(kv(t, "command", cmd));
    }
    if let Some(dest) = &req.action.destination {
        lines.push(kv(t, "destination", dest));
    }
    if !req.action.paths.is_empty() {
        lines.push(kv(t, "paths", &req.action.paths.join(", ")));
    }
    lines.push(kv(t, "reason", &req.reason));
    let iso = if req.sandbox_active {
        Line::from(vec![
            Span::styled(format!("{:>12}  ", "sandbox"), t.muted()),
            Span::styled("active", t.success()),
        ])
    } else {
        Line::from(vec![
            Span::styled(format!("{:>12}  ", "sandbox"), t.muted()),
            Span::styled(
                "NOT isolating this action",
                t.failure().add_modifier(Modifier::BOLD),
            ),
        ])
    };
    lines.push(iso);
    lines.push(Line::from(""));
    if let Some(edit) = &st.approval_edit {
        lines.push(Line::from(Span::styled(
            "propose safer tool arguments (JSON object):",
            t.secondary(),
        )));
        for line in edit.lines().take(5) {
            lines.push(Line::from(Span::styled(line.to_string(), t.text())));
        }
        lines.push(Line::from(Span::styled(
            "Ctrl+U replace · Enter validate & allow once · Esc back",
            t.muted(),
        )));
    } else {
        let persistent_allowed = req.sandbox_active && req.action.session_grant_allowed();
        let edit_allowed = !req
            .action
            .command_analysis
            .as_ref()
            .is_some_and(|analysis| analysis.one_time_only);
        let options: Vec<&str> = match (persistent_allowed, edit_allowed) {
            (true, true) => vec![
                "Allow once",
                "Allow this proved argv for this session",
                "Deny",
                "Edit / propose a safer action",
            ],
            (false, true) => vec!["Allow once", "Deny", "Propose a safer alternative"],
            (_, false) => vec!["Allow once", "Deny"],
        };
        for (index, option) in options.iter().enumerate() {
            let selected = index == st.approval_selected;
            lines.push(Line::from(vec![
                Span::styled(if selected { "▸ " } else { "  " }, t.primary()),
                Span::styled(
                    (*option).to_string(),
                    if selected { t.selection() } else { t.text() },
                ),
            ]));
        }
        lines.push(Line::from(Span::styled(
            "↑/↓ choose · Enter select · n/Esc deny · e edit",
            t.muted(),
        )));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(t.warning())
        .title(Span::styled(" ⚠ approve action ", t.warning()));
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false }),
        rect,
    );
}

// -------------------------------------------------------------------- toasts

fn draw_toasts(f: &mut Frame, area: Rect, st: &State, t: &Theme) {
    let mut y = area.y + 1;
    for toast in &st.toasts {
        let text = format!(
            " {} {} ",
            match toast.sev {
                Sev::Ok => "✓",
                Sev::Warn => "!",
                Sev::Err => "✗",
                _ => "·",
            },
            toast.text
        );
        let w =
            (UnicodeWidthStr::width(text.as_str()) as u16 + 2).min(area.width.saturating_sub(2));
        let rect = Rect {
            x: area.x + area.width.saturating_sub(w + 1),
            y,
            width: w,
            height: 3,
        };
        if rect.y + rect.height > area.y + area.height {
            break;
        }
        f.render_widget(Clear, rect);
        let style = t.sev(toast.sev);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(text, style)))
                .block(Block::default().borders(Borders::ALL).border_style(style)),
            rect,
        );
        y += 3;
    }
}

fn kv(t: &Theme, key: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:>12}  "), t.muted()),
        Span::raw(value.to_string()),
    ])
}

fn kv_styled(t: &Theme, key: &str, value: &str, style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:>12}  "), t.muted()),
        Span::styled(value.to_string(), style),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StatusBar;
    use crate::theme::ColorSupport;
    use crate::views::{MenuItem, UiAction};
    use nexus_core::orchestration::{StageStatus, ValidationEvidence, WorkBreakdown, WorkEstimate};
    use nexus_core::timeline::{LifecyclePhase, TimelineKind, TimelineStatus, TranscriptDetail};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn message_event(summary: &str, kind: TimelineKind) -> nexus_core::timeline::TimelineEvent {
        nexus_core::timeline::TimelineEvent::new(
            nexus_core::SessionId::from("sess_render"),
            nexus_core::TurnId::from("turn_render"),
            nexus_core::TraceId::from("trace_render"),
            nexus_core::SpanId::from("span_render"),
            None,
            LifecyclePhase::Message,
            TimelineStatus::Completed,
            summary,
            kind,
        )
    }

    #[test]
    fn one_line_prompt_and_answer_render_once() {
        let theme = Theme::new("cyberpunk", ColorSupport::None);
        for event in [
            message_event(
                "prompt appears once",
                TimelineKind::UserMessage {
                    text: "prompt appears once".into(),
                },
            ),
            message_event(
                "answer appears once",
                TimelineKind::FinalAnswer {
                    text: "answer appears once".into(),
                },
            ),
        ] {
            let text = event_lines(&event, TranscriptDetail::Compact, false, false, 80, &theme)
                .iter()
                .map(line_text)
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(text.matches(&event.summary).count(), 1, "{text}");
        }
    }

    #[test]
    fn multiline_prompt_and_answer_do_not_repeat_first_line() {
        let theme = Theme::new("cyberpunk", ColorSupport::None);
        for event in [
            message_event(
                "prompt first line",
                TimelineKind::UserMessage {
                    text: "prompt first line\nprompt second line".into(),
                },
            ),
            message_event(
                "answer first line",
                TimelineKind::AssistantMessage {
                    text: "answer first line\nanswer second line".into(),
                    streaming: false,
                },
            ),
        ] {
            let text = event_lines(&event, TranscriptDetail::Compact, false, false, 80, &theme)
                .iter()
                .map(line_text)
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(text.matches(&event.summary).count(), 1, "{text}");
            assert!(text.contains("second line"), "{text}");
        }
    }

    #[test]
    fn diff_card_shows_path_header_and_colorized_lines() {
        let theme = Theme::new("nexus-dark", ColorSupport::TrueColor);
        let event = message_event(
            "diff · page.html",
            TimelineKind::Diff {
                path: Some("page.html".into()),
                insertions: 2,
                deletions: 1,
                preview: "-old line\n+new line\n+another".into(),
            },
        );
        let lines = event_lines(&event, TranscriptDetail::Compact, false, false, 80, &theme);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        // Path header with counts is present.
        assert!(text.contains("page.html"), "{text}");
        assert!(text.contains("+2"), "{text}");
        assert!(text.contains("−1"), "{text}");
        // Added/removed lines are rendered in the body.
        assert!(text.contains("+new line"), "{text}");
        assert!(text.contains("-old line"), "{text}");
        // Added lines use the success color; removed lines the failure color.
        let added = lines
            .iter()
            .find(|line| line_text(line).starts_with("+new line"))
            .expect("added line");
        assert_eq!(added.spans[0].style.fg, theme.success().fg);
        let removed = lines
            .iter()
            .find(|line| line_text(line).starts_with("-old line"))
            .expect("removed line");
        assert_eq!(removed.spans[0].style.fg, theme.failure().fg);
    }

    #[test]
    fn menu_viewport_keeps_selected_row_visible() {
        let items = (0..12)
            .map(|i| MenuItem::new(format!("item {i}"), UiAction::RunCommand(i.to_string())))
            .collect();
        let mut menu = Menu::new("test", items);
        menu.selected = 10;
        let visible = menu.visible();
        let (first, last) = menu_viewport(&menu, &visible, 4);
        assert!(first <= menu.selected);
        assert!(last > menu.selected);
        assert!(last - first <= 4);
    }

    #[test]
    fn menu_viewport_accounts_for_detail_rows() {
        let items = (0..8)
            .map(|i| {
                MenuItem::new(format!("item {i}"), UiAction::RunCommand(i.to_string()))
                    .detail("secondary detail")
            })
            .collect();
        let mut menu = Menu::new("test", items);
        menu.selected = 6;
        let visible = menu.visible();
        let (first, last) = menu_viewport(&menu, &visible, 5);
        assert!(first <= menu.selected);
        assert!(last > menu.selected);
        let used: u16 = visible[first..last]
            .iter()
            .map(|&index| menu_item_height(&menu.items[index]))
            .sum();
        assert!(used <= 5);
    }

    #[test]
    fn width_truncation_never_exceeds_container() {
        let text = truncate_width("provider with a long Unicode ▚ detail", 16);
        assert!(UnicodeWidthStr::width(text.as_str()) <= 16);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn required_terminal_sizes_render_without_panics() {
        for (width, height) in [(60, 20), (80, 24), (100, 30), (120, 40), (160, 50)] {
            let mut state = State::new(
                "cyberpunk".into(),
                ColorSupport::TrueColor,
                true,
                StatusBar {
                    workspace: "/workspace/project".into(),
                    model_label: "mock / test".into(),
                    model_ok: true,
                    agent: "orchestrator".into(),
                    sandbox_level: "approval-only-host".into(),
                    network: "restricted".into(),
                    git_branch: Some("main".into()),
                    tokens_in: 12,
                    tokens_out: 8,
                    permission_mode: "default".into(),
                },
                vec![],
                true,
            );
            state.push_overlay(Overlay::Menu(Box::new(
                Menu::new(
                    "responsive menu",
                    (0..24)
                        .map(|index| {
                            MenuItem::new(
                                format!("menu item {index}"),
                                UiAction::RunCommand("help".into()),
                            )
                            .detail("detail row that must remain inside the viewport")
                        })
                        .collect(),
                )
                .searchable(),
            )));
            if let Some(Overlay::Menu(menu)) = state.overlays.last_mut() {
                menu.selected = 20;
            }
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).expect("terminal");
            terminal
                .draw(|frame| draw(frame, &mut state))
                .expect("draw");
        }
    }

    fn representative_state() -> State {
        let mut state = State::new(
            "neon-noir".into(),
            ColorSupport::None,
            true,
            StatusBar {
                workspace: "/workspace/nexus".into(),
                model_label: "anthropic / claude-sonnet".into(),
                model_ok: true,
                agent: "orchestrator".into(),
                sandbox_level: "approval-only-host".into(),
                network: "approval".into(),
                git_branch: Some("feature/timeline".into()),
                tokens_in: 2048,
                tokens_out: 512,
                permission_mode: "default".into(),
            },
            vec![],
            true,
        );
        state.session_id = Some("session_snapshot".into());
        state.focus = Focus::Timeline;
        state.user("Redesign the transcript and verify responsive scrolling.");
        state.push_local_event(
            TimelineStatus::Completed,
            "coding · claude-sonnet · orchestrator".into(),
            TimelineKind::Classification {
                class: "coding".into(),
                model: "claude-sonnet".into(),
                agent: "orchestrator".into(),
            },
        );
        state.push_local_event(
            TimelineStatus::Completed,
            "repository grounded".into(),
            TimelineKind::ReasoningSummary {
                text: "The timeline needs stable lifecycle cards and a truthful context rail."
                    .into(),
            },
        );
        state.push_local_event(
            TimelineStatus::Running,
            "update transcript renderer".into(),
            TimelineKind::ToolExecution {
                tool: "fs.apply_patch".into(),
                arguments: serde_json::json!({"path":"crates/nexus-tui/src/render.rs"}),
                output_preview: "rendering wrapped event cards".into(),
                exit_status: None,
                affected_paths: vec!["crates/nexus-tui/src/render.rs".into()],
            },
        );
        state.push_local_event(
            TimelineStatus::Completed,
            "responsive snapshot verified".into(),
            TimelineKind::Validation {
                evidence: ValidationEvidence {
                    label: "responsive snapshots".into(),
                    status: StageStatus::Completed,
                    command: Some("cargo test -p nexus-tui".into()),
                    summary: "all required terminal sizes rendered".into(),
                    artifact_id: None,
                    at: "2026-07-17T00:00:00Z".into(),
                },
            },
        );
        state.push_local_event(
            TimelineStatus::Completed,
            "Transcript cards now stream in place and preserve the viewport.".into(),
            TimelineKind::FinalAnswer {
                text: "Transcript cards now stream in place and preserve the viewport.".into(),
            },
        );

        state.active_work.session_id = Some(nexus_core::SessionId::from("session_snapshot"));
        state.active_work.session_title = "NEXUS 0.2 timeline".into();
        state.active_work.branch = Some("feature/timeline".into());
        state.active_work.head = Some("abc1234".into());
        state.active_work.model = "claude-sonnet".into();
        state.active_work.provider = "anthropic".into();
        state.active_work.agent = "orchestrator".into();
        state.active_work.permission_mode = "default".into();
        state.active_work.objective =
            Some("Redesign the transcript and verify responsive scrolling.".into());
        state.active_work.turn_state = "running".into();
        state.active_work.work = Some(WorkBreakdown::generate(
            "Redesign the transcript and verify responsive scrolling.",
            WorkEstimate {
                predicted_actions: 4,
                writes: true,
                predictable: true,
                needs_grounding: true,
                ..Default::default()
            },
        ));
        state.active_work.active_foreground_tool = Some("fs.apply_patch".into());
        state.active_work.modified_files =
            vec!["crates/nexus-tui/src/render.rs".into(), "README.md".into()];
        state.active_work.staged_files = vec!["migrations/0004_orchestration_timeline.sql".into()];
        state.active_work.validation_pending = vec!["full workspace tests".into()];
        state.active_work.context.input_tokens = 4096;
        state.active_work.context.context_window = 200_000;
        state.active_work.context.reserved_output_tokens = 8192;
        state
    }

    fn render_state_text(state: &mut State, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| draw(frame, state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let mut output = String::new();
        for y in 0..height {
            let mut line = String::new();
            for x in 0..width {
                if let Some(cell) = buffer.cell((x, y)) {
                    line.push_str(cell.symbol());
                }
            }
            output.push_str(line.trim_end());
            output.push('\n');
        }
        output
    }

    fn rendered_text(width: u16, height: u16) -> String {
        let mut state = representative_state();
        if width < 100 {
            state.context_drawer = true;
        }
        render_state_text(&mut state, width, height)
    }

    fn fnv1a64(text: &str) -> u64 {
        text.as_bytes()
            .iter()
            .fold(0xcbf29ce484222325, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
            })
    }

    #[test]
    fn timeline_and_context_snapshots_match_required_terminal_sizes() {
        let actual: Vec<(u16, u16, u64)> = [(60, 20), (80, 24), (100, 30), (120, 40), (160, 50)]
            .into_iter()
            .map(|(width, height)| (width, height, fnv1a64(&rendered_text(width, height))))
            .collect();
        let expected = [
            (60, 20, 79_688_150_447_093_718),
            (80, 24, 15_835_664_293_227_740_923),
            (100, 30, 10_022_091_956_940_501_711),
            (120, 40, 9_210_262_804_388_401_044),
            (160, 50, 13_979_991_942_712_635_246),
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn wrap_layout_cache_preserves_the_rendered_frame() {
        let mut state = representative_state();
        let first = render_state_text(&mut state, 120, 40);
        assert!(!state.wrap_layout_cache.is_empty());
        let second = render_state_text(&mut state, 120, 40);
        assert_eq!(first, second);
    }

    #[test]
    fn thinking_toggle_hides_only_reasoning_not_operational_events() {
        let mut state = representative_state();
        state.timeline.clear();
        state.thinking_enabled = false;
        state.push_local_event(
            TimelineStatus::Completed,
            "reasoning".into(),
            TimelineKind::ReasoningSummary {
                text: "HIDDEN_PROVIDER_REASONING".into(),
            },
        );
        state.push_local_event(
            TimelineStatus::Completed,
            "VISIBLE_TOOL_OPERATION".into(),
            TimelineKind::ToolExecution {
                tool: "repo.status".into(),
                arguments: serde_json::json!({}),
                output_preview: "VISIBLE_TOOL_OPERATION".into(),
                exit_status: Some("ok".into()),
                affected_paths: Vec::new(),
            },
        );
        let output = render_state_text(&mut state, 100, 30);
        assert!(!output.contains("HIDDEN_PROVIDER_REASONING"));
        assert!(output.contains("VISIBLE_TOOL_OPERATION"));
    }
}
