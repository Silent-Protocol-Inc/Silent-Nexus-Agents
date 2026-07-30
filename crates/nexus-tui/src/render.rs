//! Rendering. Responsive NEXUS layout: identity header, transcript,
//! ACTIVE CONTEXT rail, input line, segmented status footer, plus the overlay
//! stack (palette, menus, forms, pagers, progress, approvals, toasts).
//!
//! Breakpoints: wide (≥100 cols) shows the context rail; medium (≥64) shows
//! a compact activity strip; narrow stacks a single column. Panels never
//! overlap and text never renders outside the terminal.

use crate::state::{Focus, Mode, State, WrapLayoutCacheEntry};
use crate::theme::Theme;
use crate::views::{
    ActivityDetail, ActivityTab, AsideChat, Form, Menu, Overlay, Pager, Palette, PlanChoice,
    PlanReview, Progress, SecretInput, SummaryPreview,
};
use nexus_app::{Item, Report, Sev};
use nexus_core::brand::{self, BrandConstraints, BrandLockup, BrandRole, BrandVariant};
use nexus_core::orchestration::StageStatus;
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

/// Map a semantic status color to the active theme.
fn seg_style(color: crate::layout::SegColor, t: &Theme) -> Style {
    use crate::layout::SegColor;
    match color {
        SegColor::Primary => t.primary(),
        SegColor::Secondary => t.secondary(),
        SegColor::Warning => t.warning(),
        SegColor::Success => t.success(),
        SegColor::Text => t.text(),
        SegColor::Muted => t.muted(),
    }
}

/// Wrap editor text to `wrapw` display columns, breaking hard on `\n`, and
/// locate the cursor's visual (row, col). Never splits inside a character.
fn wrap_editor(text: &str, cursor: usize, wrapw: usize) -> (Vec<String>, usize, usize) {
    let wrapw = wrapw.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    let mut cursor_row = 0usize;
    let mut cursor_col = 0usize;
    let mut found = false;
    let mut byte = 0usize;
    for ch in text.chars() {
        if byte == cursor {
            cursor_row = lines.len();
            cursor_col = cur_w;
            found = true;
        }
        byte += ch.len_utf8();
        if ch == '\n' {
            lines.push(std::mem::take(&mut cur));
            cur_w = 0;
            continue;
        }
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_w + w > wrapw && !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(ch);
        cur_w += w;
    }
    if !found && byte == cursor {
        cursor_row = lines.len();
        cursor_col = cur_w;
    }
    lines.push(cur);
    (lines, cursor_row, cursor_col)
}

/// Content width available for input text (inside borders, after the prompt).
fn input_wrap_width(area_width: u16) -> usize {
    (area_width as usize)
        .saturating_sub(2)
        .saturating_sub(2)
        .max(1)
}

/// Number of rows the input box needs, including its border (min 3).
fn input_box_rows(st: &State, area_width: u16, max_rows: u16) -> u16 {
    let text = st.search_edit.as_deref().unwrap_or_else(|| st.input.text());
    let (lines, _, _) = wrap_editor(text, 0, input_wrap_width(area_width));
    let content = (lines.len() as u16).clamp(1, max_rows.max(1));
    content.saturating_add(2)
}

pub fn draw(f: &mut Frame, st: &mut State) {
    let t = st.theme;
    let area = f.area();
    let rl = crate::layout::classify(area);

    if rl.too_small {
        draw_too_small(f, area, &t);
        return;
    }

    let input_rows = input_box_rows(st, area.width, rl.input_max_rows);
    // The tracker sits between the conversation and the composer, outside the
    // transcript widget, so scrolling the timeline never moves it and it never
    // becomes scrollback.
    let plan_rows = plan_panel_rows(st, &rl);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(rl.header_rows),
            Constraint::Min(3),
            Constraint::Length(plan_rows),
            Constraint::Length(input_rows),
            Constraint::Length(rl.status_rows),
        ])
        .split(area);

    draw_header(f, rows[0], st, &t, &rl);

    if rl.show_sidebar {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(40), Constraint::Length(rl.sidebar_width)])
            .split(rows[1]);
        draw_transcript(f, body[0], st, &t, &rl);
        draw_context_rail(f, body[1], st, &t);
    } else {
        draw_transcript(f, rows[1], st, &t, &rl);
    }

    if plan_rows > 0 {
        draw_plan_panel(f, rows[2], st, &t);
    }
    draw_input(f, rows[3], st, &t, &rl);
    draw_footer(f, rows[4], st, &t, &rl);

    // Overlay stack: render every overlay, topmost last.
    for overlay in &st.overlays {
        draw_overlay(f, area, overlay, st, &t);
    }

    if st.pending.is_some() {
        draw_approval(f, area, st, &t);
    }

    if !rl.show_sidebar && st.context_drawer {
        draw_context_drawer(f, area, st, &t);
    }
    if st.agent_drawer {
        draw_agent_drawer(f, area, st, &t);
    }

    draw_toasts(f, area, st, &t);
}

/// Controlled message when the terminal is below the usable floor.
fn draw_too_small(f: &mut Frame, area: Rect, t: &Theme) {
    let lines = vec![
        Line::from(Span::styled("Terminal too small", t.warning())),
        Line::from(Span::styled("Minimum recommended: 36×12", t.text())),
        Line::from(Span::styled(
            format!("Current: {}×{}", area.width, area.height),
            t.muted(),
        )),
        Line::from(Span::styled("Press ? for help", t.muted())),
    ];
    let inner = overlay_rect(area, 30, 4);
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

// -------------------------------------------------------------------- header

fn draw_header(
    f: &mut Frame,
    area: Rect,
    st: &State,
    t: &Theme,
    rl: &crate::layout::ResponsiveLayout,
) {
    use crate::layout::{compact_path, sandbox_short, WidthClass};
    let san = |s: &str| nexus_core::sanitize::sanitize_terminal(s);
    let model = san(&st.bar.model_label);
    let agent = san(&st.bar.agent);
    let sandbox_full = san(&st.bar.sandbox_level);
    let sbx = sandbox_short(&sandbox_full).to_string();
    let net = san(&st.bar.network);
    let model_style = if st.bar.model_ok {
        t.primary()
    } else {
        t.warning()
    };

    let narrow = matches!(rl.width_class, WidthClass::Narrow | WidthClass::Mobile);
    let ws_wide = compact_path(
        &st.bar.workspace,
        if rl.compact_labels { 24 } else { 40 },
        false,
    );
    let ws_proj = compact_path(&st.bar.workspace, 18, true);
    let ws = san(if narrow { &ws_proj } else { &ws_wide });

    // NEXUS identity mark for the first row.
    let mark_width = if narrow { 9 } else { 19 };
    let lockup = brand::lockup(
        BrandVariant::Compact,
        BrandConstraints {
            width: mark_width,
            height: 1,
            unicode: brand::unicode_supported(),
        },
    );
    let mark = |t: &Theme| -> Vec<Span<'static>> {
        let mut spans = vec![Span::styled(" ", t.text())];
        if let Some(line) = lockup.lines.first() {
            spans.extend(line.spans.iter().map(|span| {
                Span::styled(
                    span.text.clone(),
                    brand_role_style(span.role, t, lockup.monochrome),
                )
            }));
        }
        spans
    };
    let lbl = |s: &'static str, t: &Theme| Span::styled(s, t.muted());

    let budget = rl.header_rows as usize;
    let mut lines: Vec<Line<'static>> = match rl.width_class {
        WidthClass::Wide => {
            let mut r = mark(t);
            r.push(Span::styled(format!("  {ws}"), t.muted()));
            r.push(lbl("  MODEL ", t));
            r.push(Span::styled(model.clone(), model_style));
            r.push(lbl("  AGENT ", t));
            r.push(Span::styled(agent.clone(), t.secondary()));
            r.push(lbl("  SANDBOX ", t));
            r.push(Span::styled(sandbox_full.clone(), t.sandbox(&sandbox_full)));
            vec![Line::from(r)]
        }
        WidthClass::Desktop => {
            let mut r = mark(t);
            r.push(Span::styled(format!("  {ws}"), t.muted()));
            r.push(lbl("  M ", t));
            r.push(Span::styled(model.clone(), model_style));
            r.push(lbl("  A ", t));
            r.push(Span::styled(agent.clone(), t.secondary()));
            r.push(lbl("  SBX ", t));
            r.push(Span::styled(sbx.clone(), t.sandbox(&sandbox_full)));
            vec![Line::from(r)]
        }
        WidthClass::Compact if budget >= 2 => {
            let mut r0 = mark(t);
            r0.push(Span::styled(format!("  {ws}"), t.muted()));
            let r1 = vec![
                lbl("MODEL ", t),
                Span::styled(model.clone(), model_style),
                lbl("  AGENT ", t),
                Span::styled(agent.clone(), t.secondary()),
                lbl("  SANDBOX ", t),
                Span::styled(sbx.clone(), t.sandbox(&sandbox_full)),
            ];
            vec![Line::from(r0), Line::from(r1)]
        }
        WidthClass::Narrow if budget >= 3 => {
            let mut r0 = mark(t);
            r0.push(Span::styled(format!("  {ws}"), t.muted()));
            let r1 = vec![
                lbl("MODEL ", t),
                Span::styled(model.clone(), model_style),
                lbl("  AGENT ", t),
                Span::styled(agent.clone(), t.secondary()),
            ];
            let r2 = vec![
                lbl("SANDBOX ", t),
                Span::styled(sbx.clone(), t.sandbox(&sandbox_full)),
            ];
            vec![Line::from(r0), Line::from(r1), Line::from(r2)]
        }
        WidthClass::Mobile if budget >= 3 => {
            let mut r0 = mark(t);
            r0.push(Span::styled(format!(" {ws}"), t.muted()));
            let r1 = vec![
                lbl("M ", t),
                Span::styled(model.clone(), model_style),
                lbl("  A ", t),
                Span::styled(agent.clone(), t.secondary()),
            ];
            let r2 = vec![
                lbl("SBX ", t),
                Span::styled(sbx.clone(), t.sandbox(&sandbox_full)),
                lbl("  NET ", t),
                Span::styled(net.clone(), t.text()),
            ];
            vec![Line::from(r0), Line::from(r1), Line::from(r2)]
        }
        // Height-reduced fallbacks: keep identity + model (priority 1) visible.
        _ if budget >= 2 => {
            let mut r0 = mark(t);
            r0.push(Span::styled("  ", t.text()));
            r0.push(lbl("M ", t));
            r0.push(Span::styled(model.clone(), model_style));
            let r1 = vec![
                lbl("A ", t),
                Span::styled(agent.clone(), t.secondary()),
                lbl("  SBX ", t),
                Span::styled(sbx.clone(), t.sandbox(&sandbox_full)),
            ];
            vec![Line::from(r0), Line::from(r1)]
        }
        _ => {
            let mut r0 = mark(t);
            r0.push(Span::styled("  ", t.text()));
            r0.push(lbl("M ", t));
            r0.push(Span::styled(model.clone(), model_style));
            vec![Line::from(r0)]
        }
    };

    // Working spinner lives on the first header row.
    if (st.busy > 0 || st.mode == Mode::Running) && !lines.is_empty() {
        let frame = if st.reduced_motion {
            "▪"
        } else {
            SPINNER[st.spinner % SPINNER.len()]
        };
        if let Some(first) = lines.first_mut() {
            first
                .spans
                .push(Span::styled(format!("  {frame} "), t.primary()));
        }
    }

    f.render_widget(Paragraph::new(lines), area);
}

// ------------------------------------------------------------ processing fx

/// Cells in the activity sweep track.
const ACTIVITY_TRACK: usize = 12;

/// Wrap prose on word boundaries at display width, falling back to a hard
/// break for any single word too long to fit.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for word in text.split_whitespace() {
        let word_width = brand::visible_width(word);
        if word_width > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            lines.extend(wrap_terminal_line(word, width));
            if let Some(last) = lines.pop() {
                current_width = brand::visible_width(&last);
                current = last;
            }
            continue;
        }
        let needed = if current.is_empty() {
            word_width
        } else {
            word_width + 1
        };
        if current_width + needed > width {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        if !current.is_empty() {
            current.push(' ');
            current_width += 1;
        }
        current.push_str(word);
        current_width += word_width;
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

/// The animated sweep: a bright block group travelling a dim track with a
/// scan marker at its head. Empty under reduced motion or `animation = "off"`,
/// where the caller falls back to a static marker.
fn activity_track(st: &State) -> Vec<Span<'static>> {
    let t = &st.theme;
    if st.reduced_motion || st.animation == "off" {
        return Vec::new();
    }
    if st.animation == "minimal" {
        let glyph = ['·', '∙', '•', '∙'][(st.spinner * st.animation_rate / 2) % 4];
        return vec![Span::styled(glyph.to_string(), t.primary())];
    }
    const GROUP: [char; 3] = ['▰', '▰', '▰'];
    let frame = st.spinner.wrapping_mul(st.animation_rate);
    let head = frame % (ACTIVITY_TRACK + GROUP.len());
    let mut cells = ['▱'; ACTIVITY_TRACK];
    let mut marker = None;
    for (i, glyph) in GROUP.iter().enumerate() {
        let pos = head as isize - i as isize;
        if (0..ACTIVITY_TRACK as isize).contains(&pos) {
            cells[pos as usize] = *glyph;
            if i == 0 {
                marker = Some(pos as usize);
            }
        }
    }
    // Three spans so the scan head reads in a different hue from the trail.
    let mut spans = Vec::new();
    let mut flush = |text: String, style| {
        if !text.is_empty() {
            spans.push(Span::styled(text, style));
        }
    };
    match marker {
        Some(at) => {
            flush(cells[..at].iter().collect(), t.muted());
            flush(cells[at].to_string(), t.primary());
            flush(cells[at + 1..].iter().collect(), t.muted());
        }
        None => flush(cells.iter().collect(), t.muted()),
    }
    spans
}

/// 1-based position of the active stage in the turn's plan, when there is one.
///
/// `None` rather than a guess: a counter that is not bound to a real stage is
/// exactly the invented progress the status line must never show.
fn plan_position(st: &State) -> Option<usize> {
    let work = st.active_work.work.as_ref()?;
    let current = work.current_stage.as_deref()?;
    work.stages
        .iter()
        .position(|stage| stage.id == current)
        .map(|index| index + 1)
}

/// The live activity component: one status row, up to
/// `reasoning_preview_lines` of preview drawn from structured runtime state,
/// and a hint pointing at the full detail. Collapses to a single row on
/// narrow terminals.
fn processing_lines(st: &State, t: &Theme, width: usize) -> Vec<Line<'static>> {
    // The activity indicator — heading, animated track, elapsed — always
    // renders while a turn is running. It is the operator's only signal that
    // work is happening, so it must never depend on `/thinking`; even `off`
    // shows activity, tool execution, and progress. Only the reasoning preview
    // below it is gated.
    if st.mode != Mode::Running {
        return Vec::new();
    }

    let skin = nexus_core::brand::Skin::nexus().for_terminal(
        crate::glyphs::tier() != crate::glyphs::GlyphTier::Ascii,
        st.reduced_motion || st.animation == "off",
    );
    let phase = st.thinking_state();
    let action = st.status_action(&skin);
    let icon = skin.icon(action);
    let verb = action.verb();

    // Elapsed is withheld below the dwell floor so a sub-second turn does not
    // flash a counter, and it is the long form only where the row can carry it.
    let elapsed_secs = st.turn_started.map(|start| start.elapsed());
    let elapsed = elapsed_secs
        .filter(|e| e.as_millis() as u64 >= skin.motion.dwell_ms)
        .map(|e| {
            let style = if width >= 72 {
                nexus_core::brand::ElapsedStyle::Long
            } else {
                nexus_core::brand::ElapsedStyle::Short
            };
            style.format(e.as_secs())
        });

    // Mobile: verb only. Portrait stays as quiet as possible.
    if width < 40 {
        let mut text = format!("{icon} {verb}");
        if let (Some(elapsed), true) = (&elapsed, width >= 28) {
            text.push_str(skin.separators.field);
            text.push_str(elapsed);
        }
        return vec![Line::from(Span::styled(text, t.primary()))];
    }

    // Effort is shown only when the provider actually reported one. An absent
    // value is omitted rather than defaulted — "medium effort" that nobody
    // reported would be an invention.
    let effort = st
        .provider_effort
        .as_deref()
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
        .map(|effort| format!("{effort} effort"));
    // The step counter is bound to a real intent plan, and only `verbose` asks
    // for it. Without a plan there is no counter, rather than a made-up one.
    let step = (st.narration_mode == nexus_core::timeline::NarrationMode::Verbose
        && st.intent_steps > 0)
        .then(|| plan_position(st))
        .flatten()
        .map(|index| format!("step {index}/{}", st.intent_steps));

    // Waiting on the operator is the one state that asks for attention, so it
    // is the one state that colors its verb differently.
    let verb_style = if action.is_blocked_on_operator() {
        t.warning()
    } else {
        t.primary()
    };
    let track = activity_track(st);
    let mut head = vec![
        Span::styled(format!("  {icon} "), t.secondary()),
        Span::styled(
            if track.is_empty() {
                verb.to_string()
            } else {
                format!("{verb} ")
            },
            verb_style,
        ),
    ];
    head.extend(track);
    for field in [elapsed, effort, step].into_iter().flatten() {
        head.push(Span::styled(
            format!("{}{field}", skin.separators.field),
            t.muted(),
        ));
    }
    let mut lines = vec![Line::from(head)];

    // The deliberation gate applies to the reasoning preview only: `off` never
    // previews, `auto` defers to the resolved per-turn decision, and the
    // anti-flicker floor keeps a sub-second turn from flashing preview rows
    // under an indicator that was already on screen.
    if !st.thinking_preview_visible() {
        return lines;
    }
    let wrap_width = width.saturating_sub(6).max(8);
    let preview = crate::thinking::summarize(st, phase, wrap_width);
    if preview.is_empty() || width < 56 {
        return lines;
    }
    // The hard cap is three rendered rows; configuration may lower it.
    let cap = st
        .preview_lines
        .clamp(1, crate::thinking::MAX_PREVIEW_LINES);
    let mut rows = 0usize;
    let mut truncated = false;
    for entry in &preview {
        if rows >= cap {
            truncated = true;
            break;
        }
        let clean = nexus_core::sanitize::sanitize_terminal(entry);
        for chunk in wrap_words(&clean, wrap_width) {
            if rows >= cap {
                truncated = true;
                break;
            }
            lines.push(Line::from(Span::styled(
                format!("    {chunk}"),
                t.secondary(),
            )));
            rows += 1;
        }
    }
    lines.push(Line::from(Span::styled(
        if truncated {
            "    … Ctrl+E details".to_string()
        } else {
            "    Ctrl+E details".to_string()
        },
        t.muted(),
    )));
    lines
}

// ---------------------------------------------------------------- transcript

fn draw_transcript(
    f: &mut Frame,
    area: Rect,
    st: &mut State,
    t: &Theme,
    rl: &crate::layout::ResponsiveLayout,
) {
    use crate::layout::WidthClass;
    let inner_width = area.width.saturating_sub(3).max(1) as usize;
    let mut next_row = usize::from(st.has_older_events);
    let mut layouts = Vec::new();
    st.event_row_offsets.clear();
    for (index, event) in st.timeline.iter().enumerate() {
        if !st.event_visible(event) {
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
                let rows = event_lines(
                    event,
                    st.detail_level,
                    expanded,
                    selected,
                    inner_width,
                    t,
                    st.reveals_machine_detail(),
                )
                .len();
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
    let processing = (st.mode == Mode::Running)
        .then(|| processing_lines(st, t, inner_width))
        .filter(|lines| !lines.is_empty());
    let processing_row = processing.as_ref().map(|_| next_row);
    next_row = next_row.saturating_add(processing.as_ref().map_or(0, Vec::len));
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
            match rl.width_class {
                WidthClass::Wide | WidthClass::Desktop => format!(
                    " TIMELINE · {} · {} ",
                    st.transcript_filter.as_str(),
                    st.detail_level.as_str()
                ),
                WidthClass::Compact => format!(" TIMELINE · {} ", st.detail_level.as_str()),
                WidthClass::Narrow => " TIMELINE ".to_string(),
                WidthClass::Mobile => " LOG ".to_string(),
            },
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
        let rendered = event_lines(
            event,
            st.detail_level,
            expanded,
            selected,
            inner_width,
            t,
            st.reveals_machine_detail(),
        );
        let from = scroll.saturating_sub(offset).min(rendered.len());
        let to = visible_end
            .saturating_sub(offset)
            .min(rendered.len())
            .max(from);
        lines.extend(rendered[from..to].iter().cloned());
    }
    if let (Some(row), Some(rendered)) = (processing_row, processing.as_ref()) {
        // Clip the component to the viewport the same way event cards are, so
        // a 4-row activity block never overruns a short terminal.
        let from = scroll.saturating_sub(row).min(rendered.len());
        let to = visible_end
            .saturating_sub(row)
            .min(rendered.len())
            .max(from);
        lines.extend(rendered[from..to].iter().cloned());
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

/// A concise, purpose-built header for the kinds the operator sees most.
/// Returns `None` for kinds that keep the generic status/type row.
fn component_header(
    event: &nexus_core::timeline::TimelineEvent,
    t: &Theme,
    header_style: ratatui::style::Style,
) -> Option<Line<'static>> {
    use nexus_core::timeline::{TimelineKind, TimelineStatus};

    let running = event.status == TimelineStatus::Running;
    let failed = matches!(
        event.status,
        TimelineStatus::Failed | TimelineStatus::Blocked
    );
    // `●` reads as live, `✓` settled, `✕` failed, `◆` handed to the background.
    let mark = match event.status {
        TimelineStatus::Running => "●",
        TimelineStatus::Completed => "✓",
        TimelineStatus::Failed => "✕",
        TimelineStatus::Blocked => "■",
        TimelineStatus::Cancelled => "×",
        TimelineStatus::Waiting => "◫",
        TimelineStatus::Pending => "◇",
        TimelineStatus::Skipped => "–",
    };
    let duration = event.duration_ms.map(human_duration);
    let mark = match &event.kind {
        TimelineKind::BackgroundTask { .. } if !failed => "◆",
        TimelineKind::ProviderLimit { .. } => "△",
        // `◢` while a segment is live, `◆` once it is part of the record. The
        // pair reads as one thing settling rather than two different events.
        TimelineKind::AgentActivity { .. } if running => "◢",
        TimelineKind::AgentActivity { .. } => "◆",
        TimelineKind::Intent { .. } => {
            nexus_core::brand::Skin::nexus().icon(nexus_core::brand::ActionState::ShapingApproach)
        }
        _ => mark,
    };

    let (body, trailing) = match &event.kind {
        TimelineKind::ToolExecution {
            tool, exit_status, ..
        } => {
            let detail = match (failed, exit_status.as_deref()) {
                (true, Some(status)) if status != "error" => format!(" · exit {status}"),
                (true, _) => " · failed".to_string(),
                _ => String::new(),
            };
            let summary = if event.summary.trim().is_empty() {
                String::new()
            } else {
                format!(" · {}", truncate(event.summary.trim(), 60))
            };
            (format!("{tool}{detail}{summary}"), duration)
        }
        TimelineKind::SandboxCommand { command, .. } => {
            // Show enough of the command line to identify it, not just argv[0].
            let rendered = truncate(&command.join(" "), 52);
            let rendered = if rendered.is_empty() {
                "shell".to_string()
            } else {
                rendered
            };
            let verb = if running {
                format!("Running {rendered}")
            } else if failed {
                format!("Failed · {rendered}")
            } else {
                format!("Ran {rendered}")
            };
            (verb, duration)
        }
        TimelineKind::FileMutation {
            path, operation, ..
        } => {
            let verb = match operation.as_str() {
                "create" | "created" => "Created",
                "delete" | "deleted" => "Deleted",
                _ => "Updated",
            };
            (
                format!("{verb} {}", crate::layout::compact_path(path, 48, true)),
                None,
            )
        }
        TimelineKind::Diff {
            path,
            insertions,
            deletions,
            ..
        } => {
            let target = path
                .as_deref()
                .map(|path| crate::layout::compact_path(path, 44, true))
                .unwrap_or_else(|| "working tree".into());
            (
                format!("Updated {target}"),
                Some(format!("+{insertions} −{deletions}")),
            )
        }
        // The message body follows on its own line, so the header stays a
        // label — repeating a short error in both places reads as a stutter.
        TimelineKind::Error {
            class, retryable, ..
        } => (
            class.clone(),
            retryable.then(|| "retryable".to_string()).or(duration),
        ),
        TimelineKind::ProviderLimit { .. } => ("Provider limit".to_string(), None),
        // Whoever is actually running names itself, and the phase says what
        // kind of moment this is. Neither is ever a hardcoded product name.
        TimelineKind::AgentActivity {
            role, step, phase, ..
        } => {
            let role = role.trim();
            let role = if role.is_empty() { "AGENT" } else { role };
            let step = step
                .map(|(index, total)| format!(" · STEP {index}/{total}"))
                .unwrap_or_default();
            (
                format!("{} {}{step}", role.to_uppercase(), phase.label()),
                duration,
            )
        }
        // The answer is the point of the turn: no status word, no type label,
        // just a quiet marker and any evidence worth citing.
        TimelineKind::FinalAnswer { .. } => ("Answer".to_string(), {
            let sources = event.artifact_refs.len();
            (sources > 0).then(|| format!("Sources: {sources}"))
        }),
        TimelineKind::BackgroundTask { .. } => (
            format!("Background · {}", truncate(event.summary.trim(), 60)),
            duration,
        ),
        _ => return None,
    };

    // Errors and limits are warnings first; the status colour would read as
    // just another card otherwise.
    let body_style = match &event.kind {
        TimelineKind::Error { .. } => t.failure(),
        TimelineKind::ProviderLimit { .. } => t.warning(),
        // Identity, in the accent that means identity everywhere else.
        TimelineKind::AgentActivity { .. } => t.secondary(),
        _ => t.text(),
    };
    let mut spans = vec![
        Span::styled(format!("{mark} "), header_style),
        Span::styled(body, body_style),
    ];
    if let Some(trailing) = trailing {
        spans.push(Span::styled(format!("  {trailing}"), t.muted()));
    }
    Some(Line::from(spans))
}

fn event_lines(
    event: &nexus_core::timeline::TimelineEvent,
    detail: nexus_core::timeline::TranscriptDetail,
    expanded: bool,
    selected: bool,
    width: usize,
    t: &Theme,
    // `/view detailed|debug`. Tool names are machine detail and belong to the
    // debug layer; the product layers describe what happened instead.
    reveal_machine_detail: bool,
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
    // Common kinds get a purpose-built header that reads as a sentence; the
    // generic status/type/meta row remains the fallback for the rest.
    let component = component_header(event, t, header_style);
    // A component header already states the summary; repeating it below would
    // be the verbosity this redesign is removing.
    let header_carries_summary = component.is_some();
    let mut lines = match component {
        Some(line) => vec![line],
        None => vec![Line::from(vec![
            Span::styled(format!("{glyph} {status_label:<9}"), header_style),
            Span::styled(format!(" {label}{meta}"), t.muted()),
        ])],
    };
    // Message-like events use `summary` as searchable/indexed metadata (often
    // the first body line). Rendering both fields duplicated one-line messages
    // and repeated the first line of multiline messages. The body is the sole
    // display source; summary is only a fallback when that body is empty.
    let has_primary_body = match &event.kind {
        TimelineKind::UserMessage { text }
        | TimelineKind::AssistantMessage { text, .. }
        | TimelineKind::FinalAnswer { text }
        | TimelineKind::ReasoningSummary { text }
        | TimelineKind::AgentActivity { text, .. }
        | TimelineKind::Notice { text, .. } => !text.trim().is_empty(),
        // The step list below is the body; the summary would repeat the count.
        TimelineKind::Intent { steps, .. } => !steps.is_empty(),
        TimelineKind::Error { message, .. }
        | TimelineKind::Retry {
            reason: message, ..
        }
        | TimelineKind::ProviderLimit { message, .. } => !message.trim().is_empty(),
        _ => false,
    };
    if !has_primary_body && !header_carries_summary {
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
        TimelineKind::AgentActivity { text, tools, .. } => {
            push_wrapped(&mut lines, text, width, t.text());
            // The tools that ran under this segment — machine detail, so only
            // under `/view detailed|debug`. In the product view the segment
            // text already says what happened, and a list of function names
            // beside it is the noise the narration layer exists to remove.
            // Bounded even when revealed: forty reads should not push the rest
            // of the turn off the screen.
            const MAX_TOOL_ROWS: usize = 6;
            let tier = crate::glyphs::tier();
            let tools: &[String] = if reveal_machine_detail { tools } else { &[] };
            for tool in tools.iter().take(MAX_TOOL_ROWS) {
                push_wrapped(
                    &mut lines,
                    &format!("  {} {tool}", crate::glyphs::tool_glyph(tool, tier)),
                    width,
                    t.muted(),
                );
            }
            if let Some(extra) = tools.len().checked_sub(MAX_TOOL_ROWS).filter(|n| *n > 0) {
                push_wrapped(&mut lines, &format!("  +{extra} more"), width, t.muted());
            }
        }
        TimelineKind::Intent { steps, refined, .. } => {
            // Numbered, and never ticked off: this is what the agent said it
            // would do, not a record of what it did. Progress lives in the
            // milestones below it, each tied to something that happened.
            for (index, step) in steps.iter().enumerate() {
                push_wrapped(
                    &mut lines,
                    &format!("  {}. {step}", index + 1),
                    width,
                    t.text(),
                );
            }
            // Only under `/view detailed|debug`: whether a model was allowed to
            // reword the plan is provenance, not product copy. It is recorded
            // rather than implied so a degraded turn can be told apart from an
            // authored one.
            if reveal_machine_detail {
                let provenance = if *refined {
                    "wording refined by the model; steps are the harness's"
                } else {
                    "harness wording (no refinement)"
                };
                push_wrapped(&mut lines, &format!("  {provenance}"), width, t.muted());
            }
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
            // The component header already names the file and the counts; the
            // full path is still worth showing when it was compacted away.
            if !header_carries_summary {
                let header = match path {
                    Some(path) => format!("▸ {path}  (+{insertions} −{deletions})"),
                    None => format!("(+{insertions} −{deletions})"),
                };
                lines.push(Line::from(Span::styled(header, t.secondary())));
            } else if let Some(path) = path.as_deref().filter(|path| path.len() > 44) {
                lines.push(Line::from(Span::styled(path.to_string(), t.muted())));
            }
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

fn draw_input(
    f: &mut Frame,
    area: Rect,
    st: &State,
    t: &Theme,
    rl: &crate::layout::ResponsiveLayout,
) {
    use crate::layout::WidthClass;
    let searching = st.search_edit.is_some();
    let (title, border) = if searching {
        (
            " search timeline · Enter apply · Esc cancel ".to_string(),
            t.secondary(),
        )
    } else if st.mode == Mode::Running {
        (
            " running — input queued after turn ".to_string(),
            t.warning(),
        )
    } else if st.focus != Focus::Input {
        (" F6 focus · input inactive ".to_string(), t.muted())
    } else if st.bar.plan_mode {
        // The title is where the operator looks before typing, so plan mode
        // says what typing will do here rather than only in the status bar.
        (
            match rl.width_class {
                WidthClass::Wide => {
                    " plan mode · nothing is written until you approve · /plan exit "
                }
                WidthClass::Desktop | WidthClass::Compact => " plan mode · /plan exit ",
                WidthClass::Narrow | WidthClass::Mobile => " PLAN ",
            }
            .to_string(),
            t.warning(),
        )
    } else {
        (
            match rl.width_class {
                WidthClass::Wide => " message · / commands · Ctrl+K palette ",
                WidthClass::Desktop | WidthClass::Compact => " message · / commands ",
                WidthClass::Narrow => " message · / ",
                WidthClass::Mobile => " INPUT  / commands ",
            }
            .to_string(),
            t.primary(),
        )
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(Span::styled(title, border));

    let text = st.search_edit.as_deref().unwrap_or_else(|| st.input.text());
    let cursor = if searching {
        text.len()
    } else {
        st.input.cursor()
    };
    let wrapw = input_wrap_width(area.width);
    let (vlines, crow, ccol) = wrap_editor(text, cursor, wrapw);

    // Inner height available for text (box height minus the two borders).
    let inner_rows = area.height.saturating_sub(2).max(1) as usize;
    // Scroll so the cursor row stays visible when content exceeds the box.
    let scroll_top = crow.saturating_sub(inner_rows.saturating_sub(1));
    let prompt = if searching { "⌕ " } else { "❯ " };

    let mut rendered: Vec<Line> = Vec::with_capacity(inner_rows);
    for (i, line) in vlines.iter().enumerate().skip(scroll_top).take(inner_rows) {
        let lead = if i == 0 { prompt } else { "  " };
        rendered.push(Line::from(vec![
            Span::styled(lead, t.secondary()),
            Span::styled(line.clone(), t.text()),
        ]));
    }
    f.render_widget(Paragraph::new(rendered).block(block), area);

    // Place the terminal cursor at the edit position within the visible window.
    if st.overlays.is_empty() && st.pending.is_none() && st.focus == Focus::Input {
        let vis_row = crow.saturating_sub(scroll_top) as u16;
        let x = area.x + 1 + 2 + (ccol as u16).min(area.width.saturating_sub(4));
        let y = area.y + 1 + vis_row.min(inner_rows.saturating_sub(1) as u16);
        f.set_cursor_position((x, y));
    }
}

// -------------------------------------------------------------------- footer

fn draw_footer(
    f: &mut Frame,
    area: Rect,
    st: &State,
    t: &Theme,
    rl: &crate::layout::ResponsiveLayout,
) {
    use crate::layout::{pack_status, sandbox_short, SegColor, StatusSegment, WidthClass};
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
    let san = |s: &str| nexus_core::sanitize::sanitize_terminal(s);

    let (state_label, state_color) = if st.mode == Mode::Running {
        let label = if st.reduced_motion {
            "RUNNING".to_string()
        } else {
            const RAMP: [char; 4] = ['░', '▒', '▓', '█'];
            format!("RUNNING {}", RAMP[st.spinner % RAMP.len()])
        };
        (label, SegColor::Warning)
    } else if st.busy > 0 {
        ("LOADING".to_string(), SegColor::Secondary)
    } else {
        ("READY".to_string(), SegColor::Success)
    };

    let mk = |full: &'static str,
              compact: &'static str,
              minimal: &'static str,
              value: String,
              color: SegColor,
              priority: u8,
              allow_hide: bool| StatusSegment {
        full_label: full,
        compact_label: compact,
        minimal_label: minimal,
        value,
        color,
        priority,
        allow_hide,
    };

    // Priority 0/1 lead so they always land on the first row(s).
    let mut segs = vec![
        mk("STATUS", "", "", state_label, state_color, 0, false),
        mk(
            "MODEL",
            "M",
            "M",
            san(&st.bar.model_label),
            if st.bar.model_ok {
                SegColor::Primary
            } else {
                SegColor::Warning
            },
            0,
            false,
        ),
    ];
    if st.new_events > 0 {
        segs.push(mk(
            "NEW",
            "NEW",
            "NEW",
            st.new_events.to_string(),
            SegColor::Warning,
            1,
            true,
        ));
    }
    segs.push(mk(
        "NET",
        "NET",
        "NET",
        san(&st.bar.network),
        SegColor::Text,
        2,
        true,
    ));
    segs.push(mk(
        "SANDBOX",
        "SBX",
        "SBX",
        sandbox_short(&san(&st.bar.sandbox_level)).to_string(),
        SegColor::Secondary,
        2,
        true,
    ));
    segs.push(mk(
        "AGENT",
        "A",
        "A",
        san(&st.bar.agent),
        SegColor::Secondary,
        2,
        true,
    ));
    segs.push(mk(
        "MODE",
        "MODE",
        "MODE",
        san(&st.bar.permission_mode),
        SegColor::Secondary,
        2,
        true,
    ));
    // Priority 0 and never hidden: plan mode changes what a message will do,
    // so an operator must not be able to lose the indicator to a narrow
    // terminal and then wonder why their instruction was refused.
    if st.bar.plan_mode {
        segs.push(mk(
            "PLAN",
            "PLAN",
            "PLN",
            "on".to_string(),
            SegColor::Warning,
            0,
            false,
        ));
    }
    if let Some(branch) = &st.bar.git_branch {
        segs.push(mk(
            "BRANCH",
            "BR",
            "BR",
            san(branch),
            SegColor::Secondary,
            2,
            true,
        ));
    }
    if !st.search_matches.is_empty() {
        segs.push(mk(
            "MATCH",
            "M",
            "M",
            format!(
                "{}/{}",
                st.search_match_index.saturating_add(1),
                st.search_matches.len()
            ),
            SegColor::Secondary,
            1,
            true,
        ));
    }
    // The cache figure is appended only once there is one: on providers that
    // report nothing a permanent `0≡` would read as a broken cache rather than
    // as an absent one, and it would cost width the segment cannot spare.
    let tokens = if st.bar.tokens_cached > 0 {
        format!(
            "{}↑ {}↓ {}≡",
            st.bar.tokens_in, st.bar.tokens_out, st.bar.tokens_cached
        )
    } else {
        format!("{}↑ {}↓", st.bar.tokens_in, st.bar.tokens_out)
    };
    segs.push(mk("TOKENS", "TOK", "TOK", tokens, SegColor::Text, 3, true));
    segs.push(mk(
        "VIEW",
        "VIEW",
        "V",
        format!(
            "{}·{}",
            st.transcript_filter.as_str(),
            st.detail_level.as_str()
        ),
        SegColor::Muted,
        3,
        true,
    ));
    // Informational, so it drops before anything the operator must see.
    segs.push(mk(
        "THINK",
        "THK",
        "T",
        st.thinking_mode.bar_value().to_string(),
        SegColor::Muted,
        3,
        true,
    ));
    let help = match rl.width_class {
        WidthClass::Wide => "Enter send · PgUp/PgDn scroll · ? help",
        WidthClass::Desktop | WidthClass::Compact => "Enter send · ? help",
        WidthClass::Narrow | WidthClass::Mobile => "? help",
    };
    segs.push(mk("", "", "", help.to_string(), SegColor::Muted, 1, false));

    let (rows, hidden) = pack_status(
        &segs,
        area.width as usize,
        rl.status_rows as usize,
        rl.label_form(),
        3,
    );
    let sep = Span::styled(" │ ", t.muted());
    let mut lines: Vec<Line> = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut spans: Vec<Span> = Vec::new();
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                spans.push(sep.clone());
            }
            if cell.label.is_empty() {
                spans.push(Span::styled(" ", t.muted()));
            } else {
                spans.push(Span::styled(format!("{} ", cell.label), t.muted()));
            }
            spans.push(Span::styled(cell.value.clone(), seg_style(cell.color, t)));
        }
        lines.push(Line::from(spans));
    }
    if hidden > 0 {
        if let Some(last) = lines.last_mut() {
            last.spans
                .push(Span::styled(format!("  +{hidden} ⋯ Ctrl+S"), t.muted()));
        }
    }
    f.render_widget(Paragraph::new(lines), area);
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
        Overlay::ActivityDetail(d) => draw_activity_detail(f, area, d, t),
        Overlay::Aside(a) => draw_aside(f, area, a, t),
        Overlay::PlanReview(p) => draw_plan_review(f, area, p, t),
    }
}

fn draw_activity_detail(f: &mut Frame, area: Rect, d: &ActivityDetail, t: &Theme) {
    // Mobile gets the whole screen; there is no room for a floating panel.
    let mobile = area.width < 60;
    let rect = if mobile {
        area
    } else {
        overlay_rect(
            area,
            area.width.saturating_sub(6).min(110),
            area.height.saturating_sub(4),
        )
    };
    f.render_widget(Clear, rect);

    let hint = if d.editing_search {
        " type to filter · Enter apply · Esc clear "
    } else if mobile {
        " Tab tabs · Esc close "
    } else {
        " Tab/1-9 tabs · ↑↓ PgUp/PgDn scroll · / search · c copy · Esc close "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(t.primary())
        .title(Span::styled(" nexus activity ", t.brand()))
        .title_bottom(Span::styled(hint, t.muted()));

    let mut lines: Vec<Line> = Vec::new();
    let mut tab_row: Vec<Span> = Vec::new();
    for (index, tab) in d.tabs.iter().enumerate() {
        if index > 0 {
            tab_row.push(Span::styled(" · ", t.muted()));
        }
        let label = if mobile {
            tab.title.clone()
        } else {
            format!("{} {}", index + 1, tab.title)
        };
        tab_row.push(Span::styled(
            label,
            if index == d.selected {
                t.primary().add_modifier(Modifier::BOLD)
            } else {
                t.muted()
            },
        ));
    }
    lines.push(Line::from(tab_row));
    if let Some(query) = &d.search {
        lines.push(Line::from(Span::styled(
            format!("/{query}{}", if d.editing_search { "▏" } else { "" }),
            t.warning(),
        )));
    }
    lines.push(Line::from(""));

    let body = d.visible_lines();
    if body.is_empty() {
        lines.push(Line::from(Span::styled(
            if d.search.is_some() {
                "no lines match this filter"
            } else {
                "nothing recorded for this tab yet"
            },
            t.muted(),
        )));
    } else {
        // Copy mode drops styling so a terminal selection yields clean text.
        let style = if d.copy_mode { t.text() } else { t.secondary() };
        lines.extend(
            body.into_iter()
                .map(|line| Line::from(Span::styled(line, style))),
        );
    }

    let total = lines.len() as u16;
    let inner = rect.height.saturating_sub(2);
    let scroll = d.scroll.min(total.saturating_sub(inner.min(total)));
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        rect,
    );
}

/// Group the current turn's events into detail tabs. Only tabs with content
/// are returned, and the Raw tab is built only in Debug mode.
pub(crate) fn activity_detail_tabs(st: &State) -> Vec<ActivityTab> {
    use nexus_core::timeline::{ActivityMode, TimelineKind};
    let turn = st
        .active_turn_id
        .clone()
        .or_else(|| st.timeline.last().map(|event| event.turn_id.clone()));
    let events: Vec<&nexus_core::timeline::TimelineEvent> = st
        .timeline
        .iter()
        .filter(|event| turn.as_ref().is_none_or(|id| &event.turn_id == id))
        .collect();

    let describe = |event: &nexus_core::timeline::TimelineEvent| {
        let summary = nexus_core::sanitize::sanitize_terminal(&event.summary);
        match event
            .kind
            .text()
            .map(str::trim)
            .filter(|text| !text.is_empty() && *text != event.summary.trim())
        {
            Some(text) => format!(
                "{} · {summary}\n    {}",
                event.status.as_str(),
                nexus_core::sanitize::sanitize_terminal(text)
            ),
            None => format!("{} · {summary}", event.status.as_str()),
        }
    };

    let collect = |title: &str, matches: &dyn Fn(&TimelineKind) -> bool| {
        let lines: Vec<String> = events
            .iter()
            .filter(|event| matches(&event.kind))
            .map(|event| describe(event))
            .collect();
        (!lines.is_empty()).then(|| ActivityTab {
            title: title.to_string(),
            lines,
        })
    };

    let mut tabs: Vec<ActivityTab> = Vec::new();
    // Activity: the whole turn in order, so the tab is never empty when
    // anything happened at all.
    if !events.is_empty() {
        // Lead with the deliberation decision so the choice is inspectable
        // here rather than being invisible or, worse, spamming the timeline.
        let mut lines = vec![format!(
            "thinking · mode {} · {}{}",
            st.thinking_mode.as_str(),
            match st.thinking_mode {
                nexus_core::ThinkingMode::Auto => match st.thinking_show {
                    Some(true) => "shown",
                    Some(false) => "hidden",
                    None => "undecided",
                },
                nexus_core::ThinkingMode::On => "shown",
                nexus_core::ThinkingMode::Off => "hidden",
            },
            st.thinking_reason
                .map(|reason| format!(" · {reason}"))
                .unwrap_or_default()
        )];
        lines.extend(events.iter().map(|event| describe(event)));
        tabs.push(ActivityTab {
            title: "Activity".into(),
            lines,
        });
    }
    tabs.extend(collect("Reasoning", &|kind| {
        matches!(kind, TimelineKind::ReasoningSummary { .. })
    }));
    tabs.extend(collect("Tools", &|kind| {
        matches!(
            kind,
            TimelineKind::ToolExecution { .. }
                | TimelineKind::ToolProposal { .. }
                | TimelineKind::ToolProgress { .. }
                | TimelineKind::SandboxCommand { .. }
        )
    }));
    tabs.extend(collect("Policy", &|kind| {
        matches!(
            kind,
            TimelineKind::PolicyDecision { .. } | TimelineKind::Approval { .. }
        )
    }));
    tabs.extend(collect("Provider", &|kind| {
        matches!(
            kind,
            TimelineKind::ProviderActivity { .. }
                | TimelineKind::ModelRouting { .. }
                | TimelineKind::ProviderLimit { .. }
                | TimelineKind::Retry { .. }
        )
    }));
    // Raw payloads are a debugging tool, not a default surface.
    if st.activity_mode == ActivityMode::Debug {
        let lines: Vec<String> = events
            .iter()
            .filter_map(|event| {
                serde_json::to_string(&event.kind)
                    .ok()
                    .map(|json| nexus_core::sanitize::sanitize_terminal(&json))
            })
            .collect();
        if !lines.is_empty() {
            tabs.push(ActivityTab {
                title: "Raw".into(),
                lines,
            });
        }
    }
    tabs
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

/// Status mark for one step. Paired with a word wherever there is room, so the
/// state is readable without color and without knowing the symbols.
fn step_mark(status: StageStatus, awaiting: bool) -> (&'static str, &'static str) {
    if awaiting {
        return ("?", "awaiting approval");
    }
    match status {
        StageStatus::Pending => ("◇", "pending"),
        StageStatus::Running => ("◆", "active"),
        StageStatus::Completed => ("✓", "complete"),
        StageStatus::Failed => ("×", "failed"),
        StageStatus::Blocked => ("!", "blocked"),
        StageStatus::Skipped => ("–", "skipped"),
    }
}

/// How many rows the pinned tracker gets this frame: none when there is no
/// live plan, otherwise the plan's own size clamped to the height budget.
fn plan_panel_rows(st: &State, rl: &crate::layout::ResponsiveLayout) -> u16 {
    let Some(plan) = st.pinned_plan.as_ref() else {
        return 0;
    };
    if rl.plan_panel_max_rows == 0 {
        return 0;
    }
    // One header row, one row per step, one row for the elision note.
    let steps = plan.steps.len().min(u16::MAX as usize) as u16;
    (1 + steps + 1).min(rl.plan_panel_max_rows)
}

/// The pinned execution tracker.
///
/// One live view of the current plan, updated in place. It shows what is
/// happening now rather than repeating the plan into the conversation, and it
/// stays put while the operator scrolls the timeline.
fn draw_plan_panel(f: &mut Frame, area: Rect, st: &State, t: &Theme) {
    let Some(plan) = st.pinned_plan.as_ref() else {
        return;
    };
    if area.height == 0 {
        return;
    }
    let (done, total) = plan.progress();
    let agent = if plan.agent.trim().is_empty() {
        "Agent".to_string()
    } else {
        plan.agent.trim().to_string()
    };
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(if plan.awaiting_approval {
        vec![
            Span::styled("PLAN ", t.warning().add_modifier(Modifier::BOLD)),
            Span::styled(format!("{done}/{total}"), t.warning()),
            Span::styled(" · AWAITING APPROVAL · ", t.warning()),
            Span::styled(agent.clone(), t.primary()),
            Span::styled("  (Enter to review)", t.muted()),
        ]
    } else {
        let mut header = vec![
            Span::styled("AGENT ", t.muted()),
            Span::styled(agent.clone(), t.primary()),
            Span::styled(" · EXECUTION ", t.muted()),
            Span::styled(format!("{done}/{total}"), t.secondary()),
        ];
        // Name what is being worked on, but only with room to spare: the
        // counter and the step list are what the panel is for.
        let used = agent.len() + 24;
        if area.width as usize > used + 12 && !plan.objective.trim().is_empty() {
            header.push(Span::styled(
                format!(
                    " · {}",
                    crate::layout::truncate_display(
                        plan.objective.trim(),
                        area.width as usize - used
                    )
                ),
                t.muted(),
            ));
        }
        header
    }));

    // A long plan shows a window around the step being worked on, so the
    // interesting part stays on screen instead of the beginning.
    let body_rows = area.height.saturating_sub(1) as usize;
    let active = plan.active_index().unwrap_or(0);
    let visible = body_rows.saturating_sub(1).max(1);
    let start = active.saturating_sub(visible / 2).min(
        plan.steps
            .len()
            .saturating_sub(visible.min(plan.steps.len())),
    );
    let end = (start + visible).min(plan.steps.len());
    for (index, step) in plan.steps[start..end].iter().enumerate() {
        let index = start + index;
        let (mark, label) = step_mark(step.status, plan.awaiting_approval);
        let style = match step.status {
            _ if plan.awaiting_approval => t.warning(),
            StageStatus::Running => t.secondary().add_modifier(Modifier::BOLD),
            StageStatus::Completed => t.success(),
            StageStatus::Failed => t.failure(),
            StageStatus::Blocked => t.warning(),
            StageStatus::Skipped => t.muted(),
            StageStatus::Pending => t.muted(),
        };
        let width = area.width as usize;
        let title = if width > 24 {
            format!("{} {}", mark, step.title)
        } else {
            format!("{mark} {}", step.title)
        };
        let mut spans = vec![Span::styled(
            crate::layout::truncate_display(&title, width.saturating_sub(label.len() + 3)),
            style,
        )];
        // The word only appears when it fits; the symbol always does.
        if width >= 40 {
            spans.push(Span::styled(format!("  {label}"), t.muted()));
        }
        let _ = index;
        lines.push(Line::from(spans));
    }
    let hidden = plan.steps.len().saturating_sub(end - start);
    if hidden > 0 && lines.len() < area.height as usize {
        lines.push(Line::from(Span::styled(
            format!("  +{hidden} more"),
            t.muted(),
        )));
    }
    lines.truncate(area.height as usize);
    f.render_widget(Paragraph::new(Text::from(lines)), area);
}

/// The plan-authorization pop-up.
///
/// Sized from the terminal like every other overlay, so a narrow or mobile
/// window clamps rather than clipping the options: the decision list is the one
/// thing that must always be reachable, so it is laid out last and given its
/// rows first.
fn draw_plan_review(f: &mut Frame, area: Rect, p: &PlanReview, t: &Theme) {
    let rect = overlay_rect(area, 76, 24);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        // Amber: a decision is owed. Not an error, not a success.
        .border_style(t.warning())
        .title(Span::styled(" PLAN AUTHORIZATION ", t.brand()));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Options and hint are fixed; the plan gets whatever is left.
    let editing = p.editor.is_some();
    let decision_rows = if editing {
        4
    } else {
        PlanChoice::ALL.len() as u16 + 1
    };
    let chrome = 1 + decision_rows + 1;
    let steps_rows = inner.height.saturating_sub(chrome).max(1);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(steps_rows),
            Constraint::Length(decision_rows.min(inner.height.saturating_sub(1))),
            Constraint::Length(1),
        ])
        .split(inner);

    // Header: who proposed this, which revision, how big.
    let agent = if p.request.agent.trim().is_empty() {
        "Agent"
    } else {
        p.request.agent.trim()
    };
    // Header, narrowest-first. Who proposed it and whether the execution is
    // contained are the facts the decision turns on, so they survive a narrow
    // terminal; the step count is already visible in the list below.
    let (containment, containment_style) = if p.request.sandbox_active {
        ("sandboxed", t.success())
    } else {
        ("NOT sandboxed", t.failure())
    };
    let mut header = vec![
        Span::styled(agent.to_string(), t.primary()),
        Span::styled(" · ", t.muted()),
        Span::styled(containment, containment_style),
    ];
    let width = rows[0].width as usize;
    let used = |spans: &[Span]| {
        spans
            .iter()
            .map(|span| brand::visible_width(&span.content))
            .sum::<usize>()
    };
    let revision = format!(" · rev {}", p.request.version);
    if used(&header) + brand::visible_width(&revision) <= width {
        header.push(Span::styled(revision, t.muted()));
    }
    let step_count = p.request.stages.len();
    let steps = format!(
        " · {} step{}",
        step_count,
        if step_count == 1 { "" } else { "s" }
    );
    if used(&header) + brand::visible_width(&steps) <= width {
        header.push(Span::styled(steps, t.muted()));
    }
    f.render_widget(Paragraph::new(Line::from(header)), rows[0]);

    // The plan itself. Without this the operator is approving a title.
    let width = rows[1].width as usize;
    let mut lines: Vec<Line> = Vec::new();
    for step in &p.request.stages {
        let mut head = wrap_words(
            &format!("{}. {}", step.sequence, step.title),
            width.saturating_sub(1),
        )
        .into_iter();
        lines.push(Line::from(Span::styled(
            head.next().unwrap_or_default(),
            t.text(),
        )));
        for rest in head {
            lines.push(Line::from(Span::styled(format!("   {rest}"), t.text())));
        }
        for detail in wrap_words(&step.detail, width.saturating_sub(3)) {
            lines.push(Line::from(Span::styled(format!("   {detail}"), t.muted())));
        }
        if !step.files.is_empty() {
            for file in wrap_words(&step.files.join(", "), width.saturating_sub(3)) {
                lines.push(Line::from(Span::styled(
                    format!("   {file}"),
                    t.secondary(),
                )));
            }
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("the plan has no steps", t.muted())));
    }
    let max_scroll = lines.len().saturating_sub(rows[1].height as usize);
    f.render_widget(
        Paragraph::new(Text::from(lines)).scroll((p.scroll.min(max_scroll) as u16, 0)),
        rows[1],
    );

    // Decision. While writing a note the same rows hold the editor, so the
    // pop-up never grows past the space it was given.
    if let Some((choice, text)) = &p.editor {
        let mut editor_lines = vec![Line::from(Span::styled(choice.prompt(), t.secondary()))];
        let typed = wrap_words(text, rows[2].width.saturating_sub(2) as usize);
        if typed.is_empty() {
            editor_lines.push(Line::from(vec![Span::styled("▏", t.primary())]));
        } else {
            for (index, line) in typed.iter().enumerate() {
                let mut spans = vec![Span::styled(line.clone(), t.text())];
                if index + 1 == typed.len() {
                    spans.push(Span::styled("▏", t.primary()));
                }
                editor_lines.push(Line::from(spans));
            }
        }
        f.render_widget(
            Paragraph::new(Text::from(editor_lines)).wrap(Wrap { trim: false }),
            rows[2],
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Enter submit · Ctrl+U clear · Esc back to the options",
                t.muted(),
            )))
            .wrap(Wrap { trim: false }),
            rows[3],
        );
        return;
    }

    let mut option_lines = Vec::new();
    for (index, choice) in PlanChoice::ALL.iter().enumerate() {
        let selected = index == p.selected;
        option_lines.push(Line::from(vec![
            Span::styled(if selected { " › " } else { "   " }, t.primary()),
            Span::styled(
                choice.label(),
                if selected {
                    t.primary().add_modifier(Modifier::BOLD)
                } else {
                    t.text()
                },
            ),
        ]));
    }
    f.render_widget(Paragraph::new(Text::from(option_lines)), rows[2]);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "↑↓ select · Enter confirm · A approve · N note · R changes · D decline · Esc later",
            t.muted(),
        )))
        .wrap(Wrap { trim: false }),
        rows[3],
    );
}

fn draw_aside(f: &mut Frame, area: Rect, a: &AsideChat, t: &Theme) {
    let rect = overlay_rect(area, 78, 22);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(t.secondary())
        .title(Span::styled(" by the way — aside ", t.brand()));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);

    // Body: the exchange log. Empty state explains what this surface is for.
    let mut lines: Vec<Line> = Vec::new();
    if a.exchanges.is_empty() {
        lines.push(Line::from(Span::styled(
            "Ask a question or hand the agent context.",
            t.muted(),
        )));
        lines.push(Line::from(Span::styled(
            "This runs separately — the main turn keeps going, and nothing here",
            t.muted(),
        )));
        lines.push(Line::from(Span::styled(
            "joins the transcript. It is kept as side context for this session.",
            t.muted(),
        )));
    }
    // Wrap here rather than leaving it to the paragraph: the scroll offset is
    // in rendered lines, so counting logical ones would land short of the
    // bottom and hide the newest part of a long answer.
    let width = rows[0].width as usize;
    for exchange in &a.exchanges {
        let mut question = wrap_words(&exchange.question, width.saturating_sub(2)).into_iter();
        lines.push(Line::from(vec![
            Span::styled("❯ ", t.primary()),
            Span::styled(question.next().unwrap_or_default(), t.text()),
        ]));
        for rest in question {
            lines.push(Line::from(Span::styled(format!("  {rest}"), t.text())));
        }
        match &exchange.answer {
            Some(answer) => {
                for line in answer.lines() {
                    if line.trim().is_empty() {
                        lines.push(Line::from(""));
                        continue;
                    }
                    for wrapped in wrap_words(line, width) {
                        lines.push(Line::from(Span::styled(wrapped, t.muted())));
                    }
                }
            }
            None => lines.push(Line::from(Span::styled(
                "  …thinking (separate from the main turn)",
                t.secondary(),
            ))),
        }
        lines.push(Line::from(""));
    }
    // Follow the newest content, unless the operator scrolled back.
    let body_height = rows[0].height as usize;
    let max_back = lines.len().saturating_sub(body_height);
    let scroll = max_back.saturating_sub(a.scroll_back().min(max_back)) as u16;
    f.render_widget(
        Paragraph::new(Text::from(lines)).scroll((scroll, 0)),
        rows[0],
    );

    // Footer: the input line and the key hint.
    let footer = Text::from(vec![
        Line::from(vec![
            Span::styled("btw ❯ ", t.secondary()),
            Span::styled(a.input().to_string(), t.text()),
            Span::styled("▏", t.primary()),
        ]),
        Line::from(Span::styled(
            "Enter ask · ↑↓ scroll · Esc close — kept as side context, never added to the transcript",
            t.muted(),
        )),
    ]);
    f.render_widget(Paragraph::new(footer).wrap(Wrap { trim: false }), rows[1]);
}

fn draw_form(f: &mut Frame, area: Rect, form: &Form, t: &Theme) {
    // Chrome: two border rows, a blank line, an optional error line, and the
    // key hint. Whatever is left belongs to the fields.
    let chrome = 4 + u16::from(form.error.is_some());
    let wanted = form.fields.len() as u16 * 2 + chrome;
    let rect = overlay_rect(area, 76, wanted.min(area.height));
    let body_height = rect.height.saturating_sub(chrome) as usize;

    // Each field renders as one row, plus a hint row when focused and a
    // heading row when it opens a section. Fit as many as the viewport holds,
    // scrolled so the focused field is always inside it — without this a form
    // longer than the terminal simply hides its tail.
    let row_height = |index: usize| {
        1 + usize::from(form.fields[index].section.is_some()) + usize::from(index == form.focus)
    };
    let mut first = form.focus;
    let mut used = row_height(form.focus);
    let mut last = form.focus;
    loop {
        let grew_up = first > 0 && used + row_height(first - 1) <= body_height;
        if grew_up {
            first -= 1;
            used += row_height(first);
        }
        let grew_down = last + 1 < form.fields.len() && used + row_height(last + 1) <= body_height;
        if grew_down {
            last += 1;
            used += row_height(last);
        }
        if !grew_up && !grew_down {
            break;
        }
    }

    f.render_widget(Clear, rect);
    let title = if form.fields.len() > last - first + 1 {
        format!(
            " {} · {}/{} ",
            form.title,
            form.focus + 1,
            form.fields.len()
        )
    } else {
        format!(" {} ", form.title)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(t.primary())
        .title(Span::styled(title, t.brand()));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line> = Vec::new();
    for (i, field) in form
        .fields
        .iter()
        .enumerate()
        .skip(first)
        .take(last + 1 - first)
    {
        if let Some(section) = field.section {
            lines.push(Line::from(Span::styled(
                format!("{:>20}  ", section.to_uppercase()),
                t.muted(),
            )));
        }
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
            // The hint is guidance, not content: truncate it rather than let
            // it wrap back to column zero and break the label alignment.
            lines.push(Line::from(Span::styled(
                format!(
                    "{:>22}{}",
                    "",
                    truncate_width(field.hint, inner.width.saturating_sub(22) as usize)
                ),
                t.muted(),
            )));
        }
    }
    // Chrome lives at the bottom of the frame rather than after the fields:
    // in a form long enough to scroll, trailing content is exactly what falls
    // off, and the key hints are the last thing that should disappear.
    let mut chrome_lines: Vec<Line> = vec![Line::from("")];
    if let Some(err) = &form.error {
        chrome_lines.push(Line::from(Span::styled(format!(" ✗ {err}"), t.failure())));
    }
    let extra = if matches!(form.kind, crate::views::FormKind::CustomEndpoint) {
        " · Ctrl+T test connection"
    } else {
        ""
    };
    chrome_lines.push(Line::from(Span::styled(
        format!(" Tab/↑↓ move · Enter next/submit · Esc cancel{extra}"),
        t.muted(),
    )));
    let chrome_height = (chrome_lines.len() as u16).min(inner.height);
    let body = Rect {
        height: inner.height.saturating_sub(chrome_height),
        ..inner
    };
    let footer = Rect {
        y: inner.y + body.height,
        height: chrome_height,
        ..inner
    };
    f.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        body,
    );
    f.render_widget(
        Paragraph::new(Text::from(chrome_lines)).wrap(Wrap { trim: false }),
        footer,
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
        let options = [
            "Approve once".to_string(),
            if persistent_allowed {
                "Approve only for this session".into()
            } else {
                "Approve only for this session (unavailable: one-time-only action)".into()
            },
            if persistent_allowed {
                "Don't ask again (this workspace)".into()
            } else {
                "Don't ask again (unavailable: one-time-only action)".into()
            },
            "Deny".to_string(),
        ];
        for (index, option) in options.iter().enumerate() {
            let selected = index == st.approval_selected;
            lines.push(Line::from(vec![
                Span::styled(if selected { "▸ " } else { "  " }, t.primary()),
                Span::styled(
                    option.clone(),
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
            let text = event_lines(
                &event,
                TranscriptDetail::Compact,
                false,
                false,
                80,
                &theme,
                false,
            )
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
            assert_eq!(text.matches(&event.summary).count(), 1, "{text}");
        }
    }

    fn activity_event(
        role: &str,
        step: Option<(u32, u32)>,
        tools: &[&str],
    ) -> nexus_core::timeline::TimelineEvent {
        message_event(
            "activity",
            TimelineKind::AgentActivity {
                role: role.into(),
                step,
                phase: nexus_core::timeline::ActivityPhase::Analysis,
                text: "Inspecting repository structure before choosing the review path.".into(),
                tools: tools.iter().map(|tool| tool.to_string()).collect(),
            },
        )
    }

    /// The product view: what an operator sees at `/view default`.
    fn rendered(event: &nexus_core::timeline::TimelineEvent, width: usize) -> String {
        rendered_with_detail(event, width, false)
    }

    /// The debug view: `/view detailed|debug`, where machine detail is revealed.
    fn rendered_debug(event: &nexus_core::timeline::TimelineEvent, width: usize) -> String {
        rendered_with_detail(event, width, true)
    }

    fn rendered_with_detail(
        event: &nexus_core::timeline::TimelineEvent,
        width: usize,
        reveal: bool,
    ) -> String {
        let theme = Theme::new("nexus-dark", ColorSupport::None);
        event_lines(
            event,
            TranscriptDetail::Compact,
            false,
            false,
            width,
            &theme,
            reveal,
        )
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n")
    }

    #[test]
    fn activity_header_names_the_running_role_and_its_step() {
        let text = rendered(
            &activity_event("reviewer", Some((2, 5)), &["repo.structure"]),
            80,
        );
        assert!(text.contains("REVIEWER ANALYSIS"), "{text}");
        assert!(text.contains("STEP 2/5"), "{text}");
        assert!(text.contains("choosing the review path"), "{text}");
    }

    #[test]
    fn an_unnamed_role_renders_as_agent_not_a_product_name() {
        let text = rendered(&activity_event("   ", None, &[]), 80);
        assert!(text.contains("AGENT ANALYSIS"), "{text}");
        // A plan-less turn simply has no step to show.
        assert!(!text.contains("STEP"), "{text}");
    }

    #[test]
    fn grouped_tools_are_listed_once_and_bounded_under_debug() {
        let many: Vec<String> = (0..12).map(|i| format!("fs.read_file_{i}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let text = rendered_debug(&activity_event("reviewer", None, &refs), 80);
        assert!(text.contains("fs.read_file_0"), "{text}");
        // A segment with forty reads must not push the turn off the screen.
        assert!(!text.contains("fs.read_file_11"), "{text}");
        assert!(text.contains("+6 more"), "{text}");
    }

    /// The layer boundary: a function name is machine detail. In the product
    /// view the segment says what happened; the names live one keystroke away
    /// under `/view detailed|debug`.
    #[test]
    fn the_product_view_never_shows_a_tool_name() {
        let event = activity_event("reviewer", None, &["fs.read_file", "terminal.exec"]);
        let product = rendered(&event, 80);
        assert!(!product.contains("fs.read_file"), "{product}");
        assert!(!product.contains("terminal.exec"), "{product}");
        // The narration itself still renders — folding a row is not silence.
        assert!(product.contains("choosing the review path"), "{product}");
        // And the same event under the debug view does show them.
        let debug = rendered_debug(&event, 80);
        assert!(debug.contains("fs.read_file"), "{debug}");
    }

    #[test]
    fn an_intent_renders_numbered_steps_and_never_ticks_one_off() {
        let event = message_event(
            "intent · 3 step(s)",
            TimelineKind::Intent {
                steps: vec![
                    "Read the failing test".into(),
                    "Apply the fix".into(),
                    "Run the suite".into(),
                ],
                class: "coding".into(),
                refined: false,
            },
        );
        let text = rendered(&event, 80);
        assert!(text.contains("1. Read the failing test"), "{text}");
        assert!(text.contains("3. Run the suite"), "{text}");
        // An intention, not a record: no *step* is marked done. (The card's own
        // status glyph is a different thing and is allowed to be there.)
        for line in text
            .lines()
            .filter(|line| line.trim_start().starts_with(|c: char| c.is_ascii_digit()))
        {
            assert!(!line.contains('✓'), "a step was ticked off: {line}");
            assert!(!line.contains('✕'), "a step was marked failed: {line}");
        }
        // Provenance is machine detail, not product copy.
        assert!(!text.contains("no refinement"), "{text}");
        assert!(
            rendered_debug(&event, 80).contains("no refinement"),
            "provenance must be available under debug"
        );
    }

    #[test]
    fn a_narrow_terminal_renders_activity_without_panicking() {
        for width in [8, 20, 40] {
            let text = rendered(
                &activity_event("implementer", Some((10, 10)), &["fs.write_file"]),
                width,
            );
            assert!(!text.is_empty(), "width {width}");
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
            let text = event_lines(
                &event,
                TranscriptDetail::Compact,
                false,
                false,
                80,
                &theme,
                false,
            )
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
        let lines = event_lines(
            &event,
            TranscriptDetail::Compact,
            false,
            false,
            80,
            &theme,
            false,
        );
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
                    tokens_cached: 0,
                    permission_mode: "default".into(),
                    plan_mode: false,
                },
                vec![],
                nexus_core::ThinkingMode::Auto,
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

    fn activity_state() -> State {
        let mut state = representative_state();
        state.mode = Mode::Running;
        state.turn_started = Some(std::time::Instant::now());
        state.reduced_motion = false;
        state.active_work.objective = Some("Ship the 2.3.0 release".into());
        // Open the deliberation gate so these tests exercise rendering rather
        // than the visibility decision, which has its own tests below.
        state.thinking_mode = nexus_core::ThinkingMode::On;
        state.thinking_min_duration = std::time::Duration::ZERO;
        state
    }

    #[test]
    fn the_activity_preview_never_exceeds_its_configured_line_budget() {
        let mut state = activity_state();
        state.active_work.objective = Some("word ".repeat(200));
        for budget in [1usize, 3, 5] {
            state.preview_lines = budget;
            let lines = processing_lines(&state, &state.theme.clone(), 80);
            // The configured budget may lower the preview but never raise it
            // past the three-line ceiling.
            let effective = budget.min(crate::thinking::MAX_PREVIEW_LINES);
            // One status row + preview rows + one hint row.
            assert_eq!(
                lines.len(),
                effective + 2,
                "budget {budget} produced {} rows",
                lines.len()
            );
            assert!(
                line_text(lines.last().expect("hint")).contains("Ctrl+E"),
                "the operator is always told where the detail lives",
            );
        }
    }

    #[test]
    fn thinking_off_keeps_the_activity_indicator_and_drops_only_the_preview() {
        // Regression: gating the whole component on /thinking removed the
        // operator's only sign that work was happening. `off` hides reasoning,
        // not activity.
        let mut state = activity_state();
        state.thinking_mode = nexus_core::ThinkingMode::Off;
        let lines = processing_lines(&state, &state.theme.clone(), 120);
        assert_eq!(lines.len(), 1, "the indicator row must survive `off`");
        let text = line_text(&lines[0]);
        let skin = nexus_core::brand::Skin::nexus();
        assert!(text.contains(state.status_action(&skin).verb()), "{text}");
        assert!(
            !text.contains("Ctrl+E"),
            "no preview means no detail pointer: {text}"
        );
    }

    #[test]
    fn the_animated_track_runs_in_every_thinking_mode() {
        // The sweep is the liveness signal; it cannot depend on /thinking.
        let mut state = activity_state();
        for mode in [
            nexus_core::ThinkingMode::Off,
            nexus_core::ThinkingMode::On,
            nexus_core::ThinkingMode::Auto,
        ] {
            state.thinking_mode = mode;
            state.thinking_show = Some(false);
            let lines = processing_lines(&state, &state.theme.clone(), 120);
            assert!(!lines.is_empty(), "{mode:?} lost the indicator");
            let head = line_text(&lines[0]);
            assert!(
                head.contains('▰') || head.contains('▱'),
                "{mode:?} lost the animated track: {head}"
            );
        }
    }

    #[test]
    fn thinking_on_always_renders_regardless_of_the_task() {
        let mut state = activity_state();
        state.thinking_mode = nexus_core::ThinkingMode::On;
        state.thinking_show = Some(false);
        let lines = processing_lines(&state, &state.theme.clone(), 120);
        assert!(
            lines.len() > 1,
            "on previews regardless of the per-turn decision"
        );
    }

    #[test]
    fn thinking_auto_follows_the_resolved_decision() {
        let mut state = activity_state();
        state.thinking_mode = nexus_core::ThinkingMode::Auto;

        let rows = |st: &State| processing_lines(st, &st.theme.clone(), 120).len();

        state.thinking_show = Some(true);
        assert!(rows(&state) > 1, "a shown turn previews");

        // Hidden and unresolved keep the indicator but drop the preview.
        state.thinking_show = Some(false);
        assert_eq!(rows(&state), 1);
        state.thinking_show = None;
        assert_eq!(
            rows(&state),
            1,
            "unresolved stays quiet rather than guessing"
        );
    }

    #[test]
    fn a_sub_second_turn_never_flashes_the_component() {
        let mut state = activity_state();
        state.thinking_mode = nexus_core::ThinkingMode::On;
        state.thinking_min_duration = std::time::Duration::from_millis(500);
        state.turn_started = Some(std::time::Instant::now());
        assert_eq!(
            processing_lines(&state, &state.theme.clone(), 120).len(),
            1,
            "the floor suppresses preview rows, not the indicator"
        );

        // Once the floor has passed, the preview appears under it.
        state.turn_started =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(900));
        assert!(processing_lines(&state, &state.theme.clone(), 120).len() > 1);
    }

    #[test]
    fn an_idle_harness_renders_no_activity_component() {
        let mut state = activity_state();
        state.mode = Mode::Idle;
        assert!(processing_lines(&state, &state.theme.clone(), 120).is_empty());
    }

    #[test]
    fn the_preview_never_exceeds_three_rendered_rows() {
        let mut state = activity_state();
        state.preview_lines = 10;
        state.active_work.objective = Some("word ".repeat(400));
        let lines = processing_lines(&state, &state.theme.clone(), 120);
        // status row + at most three preview rows + hint row
        assert!(
            lines.len() <= crate::thinking::MAX_PREVIEW_LINES + 2,
            "rendered {} rows",
            lines.len()
        );
    }

    #[test]
    fn mobile_portrait_stays_one_row_without_elapsed() {
        let mut state = activity_state();
        state.turn_started = Some(std::time::Instant::now() - std::time::Duration::from_secs(12));
        let lines = processing_lines(&state, &state.theme.clone(), 26);
        assert_eq!(lines.len(), 1);
        let text = line_text(&lines[0]);
        assert!(!text.contains('·'), "portrait must omit elapsed: {text}");
    }

    #[test]
    fn mobile_landscape_shows_elapsed_on_its_single_row() {
        let mut state = activity_state();
        state.turn_started = Some(std::time::Instant::now() - std::time::Duration::from_secs(12));
        let lines = processing_lines(&state, &state.theme.clone(), 36);
        assert_eq!(lines.len(), 1);
        assert!(line_text(&lines[0]).contains('·'));
    }

    #[test]
    fn the_status_bar_reports_the_active_thinking_mode() {
        let mut state = activity_state();
        for (mode, expected) in [
            (nexus_core::ThinkingMode::Off, "fast"),
            (nexus_core::ThinkingMode::On, "deep"),
            (nexus_core::ThinkingMode::Auto, "auto"),
        ] {
            state.thinking_mode = mode;
            let text = render_state_text(&mut state, 80, 24);
            assert!(
                text.contains(expected),
                "status bar should report `{expected}` for {mode:?}"
            );
        }
    }

    #[test]
    fn the_activity_detail_reports_the_deliberation_decision() {
        let mut state = activity_state();
        state.thinking_mode = nexus_core::ThinkingMode::Auto;
        state.thinking_show = Some(true);
        state.thinking_reason = Some("class=coding");
        let tabs = activity_detail_tabs(&state);
        let activity = tabs
            .iter()
            .find(|tab| tab.title == "Activity")
            .expect("activity tab");
        assert!(
            activity.lines[0].contains("mode auto"),
            "{:?}",
            activity.lines[0]
        );
        assert!(activity.lines[0].contains("shown"));
        assert!(activity.lines[0].contains("class=coding"));
    }

    #[test]
    fn a_narrow_terminal_collapses_activity_to_one_row() {
        let state = activity_state();
        let lines = processing_lines(&state, &state.theme.clone(), 36);
        assert_eq!(lines.len(), 1);
        let text = line_text(&lines[0]);
        let skin = nexus_core::brand::Skin::nexus();
        let action = state.status_action(&skin);
        assert!(text.starts_with(skin.icon(action)), "{text}");
        assert!(text.contains(action.verb()), "{text}");
        // A tool name would be machine detail on the most cramped surface there
        // is; the status line never carries one.
        assert!(!text.contains("fs."), "{text}");
    }

    #[test]
    fn activity_is_not_labelled_as_reasoning_without_a_provider_channel() {
        let mut state = activity_state();
        state
            .timeline
            .retain(|event| !matches!(event.kind, TimelineKind::ReasoningSummary { .. }));
        let text = line_text(&processing_lines(&state, &state.theme.clone(), 80)[0]);
        // The row no longer announces the product at all — it says what the
        // agent is doing, which is the only thing an operator needs from it.
        assert!(!text.contains("NEXUS"), "{text}");
        let skin = nexus_core::brand::Skin::nexus();
        assert!(text.contains(state.status_action(&skin).verb()), "{text}");

        state.active_turn_id = None;
        state.push_local_event(
            TimelineStatus::Completed,
            "provider reasoning summary".into(),
            TimelineKind::ReasoningSummary {
                text: "Considering the release checklist.".into(),
            },
        );
        let text = line_text(&processing_lines(&state, &state.theme.clone(), 80)[0]);
        assert!(!text.contains("NEXUS"), "{text}");
        let skin = nexus_core::brand::Skin::nexus();
        assert!(text.contains(state.status_action(&skin).verb()), "{text}");
    }

    #[test]
    fn reduced_motion_replaces_the_sweep_with_a_static_marker() {
        let mut state = activity_state();
        state.reduced_motion = true;
        assert!(activity_track(&state).is_empty());
        let text = line_text(&processing_lines(&state, &state.theme.clone(), 80)[0]);
        // Reduced motion stops the movement; it does not swap in a different
        // design, so the state's own icon is still what leads the row.
        let skin = nexus_core::brand::Skin::nexus();
        assert!(
            text.starts_with(&format!("  {}", skin.icon(state.status_action(&skin)))),
            "{text}"
        );
        assert!(!text.contains('▰'), "{text}");
    }

    #[test]
    fn the_status_line_is_absent_when_idle() {
        let mut state = activity_state();
        state.mode = Mode::Idle;
        assert!(processing_lines(&state, &state.theme.clone(), 120).is_empty());
    }

    /// Effort is the provider's claim, not the harness's. With nothing
    /// reported the field is omitted — a defaulted "medium effort" would be an
    /// invention on the most-read row in the product.
    #[test]
    fn effort_is_shown_only_when_the_provider_reported_one() {
        let mut state = activity_state();
        state.provider_effort = None;
        let text = line_text(&processing_lines(&state, &state.theme.clone(), 120)[0]);
        assert!(!text.contains("effort"), "{text}");

        state.provider_effort = Some("high".into());
        let text = line_text(&processing_lines(&state, &state.theme.clone(), 120)[0]);
        assert!(text.contains("high effort"), "{text}");
    }

    /// The step counter is bound to a real plan. Without one there is no
    /// counter rather than a made-up denominator.
    #[test]
    fn the_step_counter_needs_both_verbose_and_a_real_plan() {
        let mut state = activity_state();
        state.narration_mode = nexus_core::timeline::NarrationMode::Verbose;
        state.intent_steps = 0;
        let text = line_text(&processing_lines(&state, &state.theme.clone(), 120)[0]);
        assert!(!text.contains("step "), "{text}");

        state.intent_steps = 3;
        state.narration_mode = nexus_core::timeline::NarrationMode::Auto;
        let text = line_text(&processing_lines(&state, &state.theme.clone(), 120)[0]);
        assert!(
            !text.contains("step "),
            "auto must not show a counter: {text}"
        );
    }

    /// A sub-second turn must not flash a counter that is instantly stale.
    #[test]
    fn elapsed_is_withheld_below_the_dwell_floor() {
        let mut state = activity_state();
        state.turn_started = Some(std::time::Instant::now());
        let text = line_text(&processing_lines(&state, &state.theme.clone(), 120)[0]);
        assert!(!text.contains("second"), "{text}");

        state.turn_started = Some(std::time::Instant::now() - std::time::Duration::from_secs(24));
        let text = line_text(&processing_lines(&state, &state.theme.clone(), 120)[0]);
        assert!(text.contains("24 seconds"), "{text}");
    }

    /// Narrow rows keep the verb and drop the prose form of the elapsed time.
    #[test]
    fn elapsed_switches_to_the_short_form_on_a_narrow_row() {
        let mut state = activity_state();
        state.turn_started = Some(std::time::Instant::now() - std::time::Duration::from_secs(24));
        let text = line_text(&processing_lines(&state, &state.theme.clone(), 60)[0]);
        assert!(text.contains("24s"), "{text}");
        assert!(!text.contains("24 seconds"), "{text}");
    }

    /// Rendering the status line is a pure projection: it must never append to
    /// the record it sits above.
    #[test]
    fn the_status_line_never_writes_a_timeline_event() {
        let state = activity_state();
        let before = state.timeline.len();
        for _ in 0..50 {
            let _ = processing_lines(&state, &state.theme.clone(), 120);
        }
        assert_eq!(state.timeline.len(), before);
    }

    /// The 2.4.1 regression guard, extended to the narration axis: liveness
    /// feedback is not verbosity, so every mode still shows the row.
    #[test]
    fn the_status_line_renders_in_every_narration_mode() {
        use nexus_core::timeline::NarrationMode;
        for mode in [
            NarrationMode::Off,
            NarrationMode::Compact,
            NarrationMode::Auto,
            NarrationMode::Verbose,
        ] {
            let mut state = activity_state();
            state.narration_mode = mode;
            assert!(
                !processing_lines(&state, &state.theme.clone(), 120).is_empty(),
                "{mode:?} lost the status line"
            );
        }
    }

    /// A fast tool sequence must not strobe the verb.
    #[test]
    fn the_verb_holds_for_the_dwell_window() {
        let skin = nexus_core::brand::Skin::nexus();
        let mut state = activity_state();
        state.active_work.active_foreground_tool = None;
        let first = state.status_action(&skin);

        // The underlying phase changes immediately…
        state.active_work.active_foreground_tool = Some("fs.write_file".into());
        assert_eq!(
            state.status_action(&skin),
            first,
            "the displayed verb changed inside the dwell window"
        );

        // …and the display catches up once the window has passed.
        let past = nexus_core::brand::Skin {
            motion: nexus_core::brand::Motion {
                dwell_ms: 0,
                ..skin.motion
            },
            ..skin
        };
        assert_eq!(
            state.status_action(&past),
            nexus_core::brand::ActionState::Applying
        );
    }

    #[test]
    fn the_sweep_advances_deterministically_and_wraps() {
        let mut state = activity_state();
        state.animation_rate = 1;
        let frame = |state: &State| {
            activity_track(state)
                .iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        };
        let period = ACTIVITY_TRACK + 3;
        state.spinner = 0;
        let first = frame(&state);
        state.spinner = period;
        assert_eq!(frame(&state), first, "the sweep is periodic");
        state.spinner = 1;
        assert_ne!(frame(&state), first, "successive ticks differ");
    }

    #[test]
    fn control_characters_never_reach_the_activity_preview() {
        let mut state = activity_state();
        state.active_work.objective = Some("clean\u{1b}[31mred\u{7}text".into());
        let text = processing_lines(&state, &state.theme.clone(), 80)
            .iter()
            .map(line_text)
            .collect::<String>();
        assert!(!text.contains('\u{1b}'), "{text:?}");
        assert!(!text.contains('\u{7}'), "{text:?}");
    }

    #[test]
    fn activity_tabs_are_built_only_when_they_have_content() {
        let mut state = representative_state();
        state.activity_mode = nexus_core::timeline::ActivityMode::Detailed;
        let titles: Vec<String> = activity_detail_tabs(&state)
            .into_iter()
            .map(|tab| tab.title)
            .collect();
        assert!(titles.contains(&"Activity".to_string()));
        assert!(
            !titles.contains(&"Raw".to_string()),
            "raw payloads are a debug surface only: {titles:?}",
        );

        state.activity_mode = nexus_core::timeline::ActivityMode::Debug;
        let titles: Vec<String> = activity_detail_tabs(&state)
            .into_iter()
            .map(|tab| tab.title)
            .collect();
        assert!(titles.contains(&"Raw".to_string()), "{titles:?}");
    }

    #[test]
    fn activity_detail_lines_are_sanitized() {
        let mut state = representative_state();
        state.push_local_event(
            TimelineStatus::Completed,
            "clean\u{1b}[31m summary".into(),
            TimelineKind::Notice {
                text: "body\u{7}bell".into(),
                severity: "info".into(),
            },
        );
        let joined = activity_detail_tabs(&state)
            .into_iter()
            .flat_map(|tab| tab.lines)
            .collect::<String>();
        assert!(!joined.contains('\u{1b}'), "{joined:?}");
        assert!(!joined.contains('\u{7}'), "{joined:?}");
    }

    fn component_card(
        status: TimelineStatus,
        summary: &str,
        kind: TimelineKind,
        duration_ms: Option<u64>,
    ) -> String {
        let theme = Theme::new("cyberpunk", ColorSupport::None);
        let mut event = message_event(summary, kind);
        event.status = status;
        event.duration_ms = duration_ms;
        event_lines(
            &event,
            TranscriptDetail::Compact,
            false,
            false,
            80,
            &theme,
            false,
        )
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
    }

    fn tool(status: TimelineStatus, exit: Option<&str>) -> TimelineKind {
        let _ = status;
        TimelineKind::ToolExecution {
            tool: "fs.read_file".into(),
            arguments: serde_json::json!({}),
            output_preview: String::new(),
            exit_status: exit.map(str::to_string),
            affected_paths: vec![],
        }
    }

    #[test]
    fn a_tool_card_reads_as_one_line_through_its_lifecycle() {
        let running = component_card(
            TimelineStatus::Running,
            "reading render.rs",
            tool(TimelineStatus::Running, None),
            None,
        );
        assert_eq!(running, "● fs.read_file · reading render.rs");

        let done = component_card(
            TimelineStatus::Completed,
            "read 412 lines",
            tool(TimelineStatus::Completed, Some("ok")),
            Some(340),
        );
        assert_eq!(done, "✓ fs.read_file · read 412 lines  340ms");

        let failed = component_card(
            TimelineStatus::Failed,
            "command not found",
            tool(TimelineStatus::Failed, Some("127")),
            Some(90),
        );
        assert!(failed.starts_with("✕ fs.read_file · exit 127"), "{failed}");
    }

    #[test]
    fn a_shell_card_shows_the_command_not_just_the_program() {
        let card = component_card(
            TimelineStatus::Running,
            "cargo test",
            TimelineKind::SandboxCommand {
                command: vec!["cargo".into(), "test".into()],
                backend: "process".into(),
                output_preview: String::new(),
            },
            None,
        );
        assert_eq!(card, "● Running cargo test");
    }

    #[test]
    fn a_diff_card_states_the_file_and_counts_exactly_once() {
        let card = component_card(
            TimelineStatus::Completed,
            "diff",
            TimelineKind::Diff {
                path: Some("crates/nexus-tui/src/render.rs".into()),
                insertions: 42,
                deletions: 7,
                preview: "+added\n-removed".into(),
            },
            None,
        );
        assert!(card.starts_with("✓ Updated render.rs  +42 −7"), "{card}");
        assert_eq!(card.matches("+42").count(), 1, "counts appear once: {card}");
    }

    #[test]
    fn errors_and_limits_carry_their_own_marks_without_stuttering() {
        let error = component_card(
            TimelineStatus::Failed,
            "provider request timed out after 30s",
            TimelineKind::Error {
                class: "provider_timeout".into(),
                message: "provider request timed out after 30s".into(),
                retryable: true,
            },
            None,
        );
        assert!(error.starts_with("✕ provider_timeout"), "{error}");
        assert!(error.contains("retryable"), "{error}");
        assert_eq!(
            error.matches("timed out after 30s").count(),
            1,
            "the message is stated once: {error}",
        );

        let limit = component_card(
            TimelineStatus::Waiting,
            "resets at 14:02Z",
            TimelineKind::ProviderLimit {
                provider: "anthropic".into(),
                limit_kind: "rate".into(),
                message: "rate limited; resets at 14:02Z".into(),
                reset_at: None,
            },
            None,
        );
        assert!(limit.starts_with("△ Provider limit"), "{limit}");
    }

    #[test]
    fn the_final_answer_is_visually_distinct_from_diagnostics() {
        let card = component_card(
            TimelineStatus::Completed,
            "answer",
            TimelineKind::FinalAnswer {
                text: "The release is ready.".into(),
            },
            None,
        );
        assert!(card.starts_with("✓ Answer"), "{card}");
        assert!(
            !card.contains("FINAL ANSWER") && !card.contains("DONE  "),
            "no diagnostic labels on the answer: {card}",
        );
        assert!(card.contains("The release is ready."), "{card}");
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
                tokens_cached: 0,
                permission_mode: "default".into(),
                plan_mode: false,
            },
            vec![],
            nexus_core::ThinkingMode::Auto,
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

    /// Plan mode changes what pressing Enter will do, so it has to be visible
    /// at every width — including the narrow ones where lower-priority status
    /// segments are dropped.
    #[test]
    fn plan_mode_is_announced_at_every_terminal_width() {
        for (width, height) in [(36, 20), (60, 20), (80, 24), (120, 32)] {
            let mut off = representative_state();
            off.focus = Focus::Input;
            let plain = render_state_text(&mut off, width, height);

            let mut on = representative_state();
            on.focus = Focus::Input;
            on.bar.plan_mode = true;
            let planning = render_state_text(&mut on, width, height);

            assert!(
                planning.contains("PLAN") || planning.contains("PLN"),
                "plan mode is invisible at {width}x{height}:\n{planning}"
            );
            assert_ne!(
                plain, planning,
                "plan mode changed nothing on screen at {width}x{height}"
            );
        }
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
        let actual: Vec<(u16, u16, u64)> = [
            (36, 20),
            (45, 20),
            (60, 18),
            (60, 20),
            (80, 24),
            (100, 30),
            (120, 40),
            (160, 50),
        ]
        .into_iter()
        .map(|(width, height)| (width, height, fnv1a64(&rendered_text(width, height))))
        .collect();
        let expected = [
            (36, 20, 5_109_255_286_466_179_782),
            (45, 20, 9_711_206_854_701_928_601),
            (60, 18, 2_036_211_522_606_675_275),
            (60, 20, 12_044_102_317_102_766_665),
            (80, 24, 2_926_271_281_647_782_349),
            (100, 30, 3_979_066_461_458_076_351),
            (120, 40, 9_251_621_077_969_128_800),
            (160, 50, 2_050_824_127_170_740_716),
        ];
        assert_eq!(actual, expected);
    }

    /// The budget form has more fields than an 80x24 terminal has rows. Before
    /// the viewport existed the overlay was simply clipped, so the last fields
    /// could be focused but never seen.
    #[test]
    fn a_long_form_keeps_the_focused_field_on_screen() {
        let limits = nexus_core::config::LimitsConfig::default();
        for (width, height) in [(80, 24), (80, 16), (60, 20)] {
            for focus in 0..limits_form_len() {
                let mut state = representative_state();
                let mut form = crate::views::Form::budgets(&limits);
                let label = form.fields[focus].label;
                form.focus = focus;
                state.push_overlay(Overlay::Form(form));
                let out = render_state_text(&mut state, width, height);
                assert!(
                    out.contains(label),
                    "field `{label}` (index {focus}) is unreachable at {width}x{height}:\n{out}",
                );
            }
        }
    }

    #[test]
    #[ignore = "visual aid: cargo test -p nexus-tui budget_form_looks_right -- --ignored --nocapture"]
    fn budget_form_looks_right() {
        for focus in [0, 8, 16] {
            let mut state = representative_state();
            let mut form =
                crate::views::Form::budgets(&nexus_core::config::LimitsConfig::default());
            form.focus = focus;
            state.push_overlay(Overlay::Form(form));
            println!("{}", render_state_text(&mut state, 80, 24));
        }
    }

    fn limits_form_len() -> usize {
        crate::views::Form::budgets(&nexus_core::config::LimitsConfig::default())
            .fields
            .len()
    }

    #[test]
    fn responsive_sizes_render_key_regions_without_overflow() {
        let sizes = [
            (30, 10),
            (36, 12),
            (40, 16),
            (45, 20),
            (50, 22),
            (60, 18),
            (70, 24),
            (80, 24),
            (100, 30),
            (120, 35),
            (160, 40),
        ];
        for (w, h) in sizes {
            let mut state = representative_state();
            let out = render_state_text(&mut state, w, h);
            let low = out.to_lowercase();
            assert!(low.contains("nexus"), "no identity mark at {w}x{h}");
            assert!(
                low.contains("input") || low.contains("message"),
                "no input area at {w}x{h}"
            );
            assert!(
                out.contains("READY") || out.contains("RUNNING") || out.contains("LOADING"),
                "no execution status at {w}x{h}:\n{out}"
            );
            for line in out.lines() {
                assert!(
                    UnicodeWidthStr::width(line) <= w as usize,
                    "row exceeds width {w} at {w}x{h}: {line:?}"
                );
            }
        }
    }

    #[test]
    fn too_small_terminal_shows_controlled_message() {
        let mut state = representative_state();
        let out = render_state_text(&mut state, 20, 6);
        assert!(out.to_lowercase().contains("too small"));
    }

    #[test]
    fn wrap_editor_wraps_and_locates_cursor() {
        assert_eq!(wrap_editor("", 0, 10).0, vec!["".to_string()]);
        let (lines, row, col) = wrap_editor("hello", 5, 10);
        assert_eq!(lines, vec!["hello".to_string()]);
        assert_eq!((row, col), (0, 5));
        assert_eq!(wrap_editor(&"x".repeat(25), 0, 10).0.len(), 3);
        let (lines, row, col) = wrap_editor("a\nbc", 4, 10);
        assert_eq!(lines, vec!["a".to_string(), "bc".to_string()]);
        assert_eq!((row, col), (1, 2));
    }

    #[test]
    fn input_box_grows_then_caps() {
        let mut state = representative_state();
        state.input.set_text("x".repeat(400));
        assert_eq!(input_box_rows(&state, 80, 4), 6); // capped 4 content + 2 border
        assert_eq!(input_box_rows(&state, 40, 3), 5); // capped 3 content + 2 border
        state.input.set_text("hi");
        assert_eq!(input_box_rows(&state, 80, 4), 3); // 1 content + 2 border
    }

    #[test]
    fn resize_shrink_then_grow_keeps_input_intact() {
        let mut state = representative_state();
        state.input.set_text("draft message");
        for (w, h) in [(120, 40), (36, 12), (120, 40)] {
            let out = render_state_text(&mut state, w, h);
            assert!(!out.is_empty());
        }
        assert_eq!(state.input.text(), "draft message");
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
        state.thinking_mode = nexus_core::ThinkingMode::Off;
        // Isolate the axis under test: narration also folds raw tool rows, and
        // this test is about `/thinking` not hiding operational events.
        state.narration_mode = nexus_core::timeline::NarrationMode::Off;
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
    fn pinned(awaiting: bool) -> crate::state::PinnedPlan {
        use crate::state::{PinnedPlan, PinnedStep};
        PinnedPlan {
            plan_id: "plan_1".into(),
            agent: "planner".into(),
            objective: "repair the approval workflow".into(),
            version: 1,
            steps: vec![
                PinnedStep {
                    sequence: 1,
                    title: "Inspect existing approval state".into(),
                    status: StageStatus::Completed,
                },
                PinnedStep {
                    sequence: 2,
                    title: "Implement the decision popup".into(),
                    status: StageStatus::Running,
                },
                PinnedStep {
                    sequence: 3,
                    title: "Validate resize behavior".into(),
                    status: StageStatus::Pending,
                },
                PinnedStep {
                    sequence: 4,
                    title: "Run the gate".into(),
                    status: StageStatus::Pending,
                },
            ],
            awaiting_approval: awaiting,
        }
    }

    #[test]
    fn the_pinned_panel_shows_progress_and_the_active_step() {
        let mut state = representative_state();
        state.pinned_plan = Some(pinned(false));
        let text = render_state_text(&mut state, 100, 30);
        assert!(text.contains("planner"), "the panel names the active agent");
        assert!(text.contains("1/4"), "one of four steps is done:\n{text}");
        assert!(
            text.contains("◆ Implement the decision popup"),
            "the active step is marked:\n{text}"
        );
        assert!(
            text.contains("✓ Inspect existing approval state"),
            "completed steps are marked:\n{text}"
        );
        assert!(
            text.contains("active") && text.contains("complete"),
            "symbols are paired with words where there is room:\n{text}"
        );
    }

    #[test]
    fn the_pinned_panel_says_when_a_decision_is_owed() {
        let mut state = representative_state();
        state.pinned_plan = Some(pinned(true));
        let text = render_state_text(&mut state, 100, 30);
        assert!(
            text.contains("AWAITING APPROVAL"),
            "a parked plan says so:\n{text}"
        );
        assert!(
            !text.contains("EXECUTION 0/4"),
            "nothing is executing yet, so the panel must not claim it is:\n{text}"
        );
    }

    #[test]
    fn the_pinned_panel_updates_in_place_rather_than_repeating_itself() {
        let mut state = representative_state();
        state.pinned_plan = Some(pinned(false));
        let first = render_state_text(&mut state, 100, 30);
        if let Some(plan) = state.pinned_plan.as_mut() {
            plan.update_step("Implement the decision popup", StageStatus::Completed);
            plan.update_step("Validate resize behavior", StageStatus::Running);
        }
        let second = render_state_text(&mut state, 100, 30);
        assert_eq!(
            second.matches("Validate resize behavior").count(),
            1,
            "a step appears once, updated, not appended again:\n{second}"
        );
        assert!(first.contains("1/4") && second.contains("2/4"));
    }

    #[test]
    fn the_pinned_panel_survives_narrow_and_short_terminals() {
        for (width, height) in [(40u16, 20u16), (52, 24), (80, 24), (120, 40), (30, 16)] {
            let mut state = representative_state();
            state.pinned_plan = Some(pinned(false));
            let text = render_state_text(&mut state, width, height);
            for line in text.lines() {
                assert!(
                    brand::visible_width(line) <= width as usize,
                    "{width}x{height}: a line overflowed the terminal: {line:?}"
                );
            }
        }
    }

    #[test]
    fn a_finished_turn_leaves_no_tracker_above_the_composer() {
        let mut state = representative_state();
        state.pinned_plan = Some(pinned(false));
        assert!(render_state_text(&mut state, 100, 30).contains("EXECUTION 1/4"));
        // What `apply_turn_done` does when the turn ends.
        state.pinned_plan = None;
        let text = render_state_text(&mut state, 100, 30);
        assert!(
            !text.contains("EXECUTION 1/4") && !text.contains("AWAITING APPROVAL"),
            "stale task state must not sit above the input:\n{text}"
        );
    }

    #[test]
    fn the_plan_popup_keeps_its_options_reachable_at_every_size() {
        for (width, height) in [(40u16, 20u16), (60, 24), (100, 30), (120, 40)] {
            let mut state = representative_state();
            state.push_overlay(Overlay::PlanReview(Box::new(PlanReview::new(
                nexus_agent::PlanReviewRequest {
                    plan_id: "plan_1".into(),
                    version: 2,
                    run_id: "run_1".into(),
                    session_id: "sess_1".into(),
                    agent: "planner".into(),
                    objective: "repair the approval workflow".into(),
                    stages: (1..=12)
                        .map(|n| nexus_agent::PlanReviewStage {
                            sequence: n,
                            title: format!("Step number {n} with a fairly long title"),
                            detail: "detail that wraps across the popup width".repeat(2),
                            files: vec!["crates/nexus-tui/src/render.rs".into()],
                        })
                        .collect(),
                    sandbox_active: false,
                },
            ))));
            let text = render_state_text(&mut state, width, height);
            assert!(
                text.contains("PLAN AUTHORIZATION"),
                "{width}x{height}: the popup is missing:\n{text}"
            );
            for option in ["Approve", "Request changes", "Decline"] {
                assert!(
                    text.contains(option),
                    "{width}x{height}: `{option}` was clipped:\n{text}"
                );
            }
            assert!(
                text.contains("planner") && text.contains("NOT sandboxed"),
                "{width}x{height}: the operator must see who proposed it and what contains it:\n{text}"
            );
            for line in text.lines() {
                assert!(
                    brand::visible_width(line) <= width as usize,
                    "{width}x{height}: overflowed: {line:?}"
                );
            }
        }
    }
}
