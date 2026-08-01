//! The welcome panel: one card, drawn once, that is not a timeline event.
//!
//! Startup facts used to be pushed through `State::system(…)`, which files them
//! as `TimelineKind::Notice` with `TimelineStatus::Completed`. The card renderer
//! then drew a `✓ DONE  NOTICE` header over each one, so opening a session
//! looked like four tasks had just succeeded — session restoration, memory
//! linking, a changelog headline, and "Ready" — none of which is an outcome of
//! anything the agent did.
//!
//! This module renders the same facts as a single panel that lives *above* the
//! timeline, in its own region. It writes nothing, stores nothing, and scrolls
//! with nothing. After the first turn it collapses to one line, so the identity
//! stays visible without spending the session's vertical space on it.
//!
//! Everything here is a projection of [`nexus_app::boot::BootSnapshot`]. There
//! is no state to get out of sync, and re-rendering at a new width simply
//! produces a different projection of the same facts.

use nexus_app::boot::{BootSnapshot, NoticeLevel};
use nexus_core::brand;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// How wide a terminal must be before metadata sits in aligned columns.
///
/// The same threshold as the border: a panel wide enough to draw a frame is
/// wide enough for a ten-column label, and a bare `path-validation-only` with
/// nothing naming it is a worse trade than a narrower value column.
const COLUMNS_MIN_WIDTH: u16 = 44;
/// Below this the panel drops its border and becomes a bare stack.
const BORDER_MIN_WIDTH: u16 = 44;
/// Label column, wide enough for `WORKSPACE`.
const LABEL_WIDTH: usize = 10;

/// The collapsed one-line form, shown from the first turn onward.
pub fn collapsed_line(
    snapshot: &BootSnapshot,
    agent: &str,
    model: &str,
    t: &Theme,
    unicode: bool,
) -> Line<'static> {
    let mark = if unicode { "◢ " } else { "> " };
    Line::from(vec![
        Span::styled(format!(" {mark}"), t.brand()),
        Span::styled(snapshot.compact_with(agent, model), t.secondary()),
    ])
}

/// How many rows the panel needs at this width, given how many it may have.
///
/// The caller reserves exactly this much, so the panel never overruns its
/// region and never has to be clipped mid-border.
pub fn panel_rows(snapshot: &BootSnapshot, width: u16, budget: u16, unicode: bool) -> u16 {
    u16::try_from(panel_lines(snapshot, width, budget, &Theme::plain(), unicode).len())
        .unwrap_or(u16::MAX)
}

/// What a line is worth when the panel does not fit.
///
/// An 80×24 terminal is ordinary, not an edge case, so the panel sheds content
/// in a defined order rather than disappearing: breathing room first, then the
/// third tip, then the news, then the greeting. Identity, the four metadata
/// rows, and anything asking the operator to act are never dropped — a panel
/// that hides "no models configured" to save a row has failed at its job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Weight {
    /// Blank spacers.
    Spacer,
    /// The third tip, then the second.
    ExtraTip,
    /// One changelog headline.
    News,
    /// "Welcome back."
    Greeting,
    /// Session and memory state. Shed before the metadata: it is the part of
    /// the panel that is nice to know, where the four metadata rows are what
    /// the panel is *for*.
    State,
    /// Workspace, model, agent, access.
    Metadata,
    /// Identity, the first tip, and every notice — a notice is the one thing
    /// the operator may have to act on, so it outranks everything.
    Essential,
}

/// The full panel.
///
/// Sections appear only when the snapshot has something real in them, so a
/// fresh workspace renders identity, metadata, and a tip — and nothing that
/// says "none".
pub fn panel_lines(
    snapshot: &BootSnapshot,
    width: u16,
    budget: u16,
    t: &Theme,
    unicode: bool,
) -> Vec<Line<'static>> {
    let bordered = width >= BORDER_MIN_WIDTH;
    let inner = if bordered {
        width.saturating_sub(4) as usize
    } else {
        width.saturating_sub(1) as usize
    };
    // Each line carries what it is worth, so a short terminal can shed the
    // right rows instead of the last ones.
    let mut body: Vec<(Weight, Line<'static>)> = Vec::new();
    let columns = width >= COLUMNS_MIN_WIDTH;

    // Identity. The mark and the spaced wordmark are the canonical lockup; the
    // panel never draws its own ASCII art, and never spends five rows on a logo
    // the header already carries. When there is a border, the title holds it.
    if !bordered {
        body.push((
            Weight::Essential,
            Line::from(vec![
                Span::styled(if unicode { "\u{25e2} " } else { "> " }, t.brand()),
                Span::styled(brand::WORDMARK.to_string(), t.brand()),
            ]),
        ));
    }
    // Version and greeting share a row where they fit: two half-empty lines at
    // the top of a panel is the kind of spacing that pushes the transcript off
    // an 80×24. Where they do not, the greeting takes its own row and is the
    // first thing the height budget gives back.
    let version = format!("{} {}", brand::PRODUCT_FULL, snapshot.version);
    let greeting = snapshot.greeting.trim();
    let inline =
        !greeting.is_empty() && version.chars().count() + 2 + greeting.chars().count() <= inner;
    if inline {
        body.push((
            Weight::Essential,
            Line::from(vec![
                Span::styled(truncate_end(&version, inner), t.muted()),
                Span::styled("  ".to_string(), t.muted()),
                Span::styled(greeting.to_string(), t.text()),
            ]),
        ));
    } else {
        body.push((
            Weight::Essential,
            Line::from(Span::styled(truncate_end(&version, inner), t.muted())),
        ));
        if !greeting.is_empty() {
            body.push((
                Weight::Greeting,
                Line::from(Span::styled(truncate_end(greeting, inner), t.text())),
            ));
        }
    }

    body.push((Weight::Spacer, Line::from("")));
    for (label, value) in [
        ("WORKSPACE", &snapshot.workspace),
        ("MODEL", &snapshot.model),
        ("AGENT", &snapshot.agent),
        ("ACCESS", &snapshot.access),
    ] {
        body.push((
            Weight::Metadata,
            metadata_row(label, value, columns, inner, t),
        ));
    }

    let session = snapshot.session.as_ref();
    let memory = snapshot
        .memory
        .as_ref()
        .and_then(|memory| memory.summary().map(|summary| (memory, summary)));
    let has_state = session.is_some()
        || memory.is_some()
        || snapshot.update.is_some()
        || !snapshot.notices.is_empty();
    if has_state {
        body.push((Weight::Spacer, Line::from("")));
    }
    // One label column for every state row, so SESSION, MEMORY, and UPDATE line
    // their details up instead of stepping raggedly across the panel.
    let state_label_width = state_label_width(snapshot);
    if let Some(session) = session {
        body.push((
            Weight::State,
            section_row(
                "SESSION",
                "RESTORED",
                &session.summary(),
                t.success(),
                columns,
                state_label_width,
                inner,
                t,
            ),
        ));
    }
    if let Some((memory, summary)) = memory {
        // Amber only when something is actually waiting on a human; a memory
        // store that simply exists is not a pending action.
        let style = if memory.pending > 0 || memory.improvements > 0 {
            t.warning()
        } else {
            t.secondary()
        };
        body.push((
            Weight::State,
            section_row(
                "MEMORY",
                "LINKED",
                &summary,
                style,
                columns,
                state_label_width,
                inner,
                t,
            ),
        ));
    }
    if let Some(update) = &snapshot.update {
        body.push((
            Weight::News,
            section_row(
                "UPDATE",
                "NEW",
                update,
                t.primary(),
                columns,
                state_label_width,
                inner,
                t,
            ),
        ));
    }
    for notice in &snapshot.notices {
        let (state, style) = match notice.level {
            NoticeLevel::Degraded => ("DEGRADED", t.warning()),
            NoticeLevel::Blocked => ("ACTION", t.failure()),
        };
        body.push((
            Weight::Essential,
            section_row(
                &notice.subsystem,
                state,
                &notice.detail,
                style,
                columns,
                state_label_width,
                inner,
                t,
            ),
        ));
    }

    if !snapshot.tips.is_empty() {
        body.push((Weight::Spacer, Line::from("")));
        for (index, line) in tip_lines(snapshot, inner, t).into_iter().enumerate() {
            let weight = if index == 0 {
                Weight::Essential
            } else {
                Weight::ExtraTip
            };
            body.push((weight, line));
        }
    }

    let body = fit(body, budget, bordered);
    if !bordered {
        return body.into_iter().map(|line| indent(line, " ")).collect();
    }
    frame(body, width, t, unicode)
}

/// Drop the least valuable rows until the panel fits its budget.
///
/// Trailing spacers go with the section they introduced, so shedding never
/// leaves a blank line hanging at the bottom of the panel.
fn fit(mut body: Vec<(Weight, Line<'static>)>, budget: u16, bordered: bool) -> Vec<Line<'static>> {
    let chrome = if bordered { 2 } else { 0 };
    let budget = budget.saturating_sub(chrome) as usize;
    for weight in [
        Weight::Spacer,
        Weight::ExtraTip,
        Weight::News,
        Weight::Greeting,
        Weight::State,
        Weight::Metadata,
    ] {
        if body.len() <= budget.max(1) {
            break;
        }
        // Drop from the end within a weight class: the third tip goes before
        // the second, and the last spacer before the first.
        while body.len() > budget.max(1) {
            let Some(index) = body.iter().rposition(|(w, _)| *w == weight) else {
                break;
            };
            body.remove(index);
        }
    }
    while body.last().is_some_and(|(w, _)| *w == Weight::Spacer) {
        body.pop();
    }
    body.truncate(budget.max(1));
    body.into_iter().map(|(_, line)| line).collect()
}

/// Width of the widest `SUBSYSTEM // STATE` heading in this snapshot, so the
/// details after them line up in one column.
fn state_label_width(snapshot: &BootSnapshot) -> usize {
    let mut headings: Vec<usize> = Vec::new();
    if snapshot.session.is_some() {
        headings.push("SESSION // RESTORED".len());
    }
    if snapshot.memory.is_some() {
        headings.push("MEMORY // LINKED".len());
    }
    if snapshot.update.is_some() {
        headings.push("UPDATE // NEW".len());
    }
    for notice in &snapshot.notices {
        let state = match notice.level {
            NoticeLevel::Degraded => "DEGRADED",
            NoticeLevel::Blocked => "ACTION",
        };
        headings.push(notice.subsystem.len() + 4 + state.len());
    }
    headings.into_iter().max().unwrap_or(0)
}

/// `LABEL  value`, or `LABEL` over `value` when the row is too narrow to hold
/// both. Truncation keeps the tail of a path, because the last segment is the
/// one that says which project this is.
fn metadata_row(label: &str, value: &str, columns: bool, inner: usize, t: &Theme) -> Line<'static> {
    if !columns {
        return Line::from(vec![Span::styled(
            truncate_start(value, inner),
            t.secondary(),
        )]);
    }
    let room = inner.saturating_sub(LABEL_WIDTH);
    Line::from(vec![
        Span::styled(format!("{label:<LABEL_WIDTH$}"), t.muted()),
        Span::styled(truncate_start(value, room), t.secondary()),
    ])
}

/// `SESSION // RESTORED  detail` — a presentation label, not a run outcome.
///
/// The `//` form is deliberate: `DONE`, `FAILED`, and `NOTICE` are statuses the
/// timeline gives to work that ran, and startup did not run any.
#[allow(clippy::too_many_arguments)]
fn section_row(
    subsystem: &str,
    state: &str,
    detail: &str,
    style: Style,
    columns: bool,
    label_width: usize,
    inner: usize,
    t: &Theme,
) -> Line<'static> {
    let heading = format!("{subsystem} // {state}");
    if !columns {
        return Line::from(vec![Span::styled(
            truncate_end(&format!("{heading} {detail}"), inner),
            style,
        )]);
    }
    let pad = label_width.saturating_sub(heading.chars().count()) + 2;
    let room = inner.saturating_sub(heading.chars().count() + pad);
    Line::from(vec![
        Span::styled(heading, style),
        Span::styled(" ".repeat(pad), t.muted()),
        Span::styled(truncate_end(detail, room), t.secondary()),
    ])
}

/// Tips as `/command detail`, one per line, with the commands in one column.
///
/// One per line rather than run together: a tip is something to act on, and a
/// row of three separated by dots reads as decoration. The height budget drops
/// the extras when there is no room, which is a better trade than cramming.
fn tip_lines(snapshot: &BootSnapshot, inner: usize, t: &Theme) -> Vec<Line<'static>> {
    let command_width = snapshot
        .tips
        .iter()
        .map(|tip| tip.command.chars().count())
        .max()
        .unwrap_or(0);
    snapshot
        .tips
        .iter()
        .map(|tip| {
            let pad = command_width.saturating_sub(tip.command.chars().count()) + 1;
            let room = inner.saturating_sub(command_width + 1);
            Line::from(vec![
                Span::styled(tip.command.clone(), t.primary()),
                Span::styled(" ".repeat(pad), t.muted()),
                Span::styled(truncate_end(&tip.detail, room), t.muted()),
            ])
        })
        .collect()
}

/// Wrap the body in the angular NEXUS frame.
fn frame(body: Vec<Line<'static>>, width: u16, t: &Theme, unicode: bool) -> Vec<Line<'static>> {
    let (tl, tr, bl, br, h, v) = if unicode {
        ('╭', '╮', '╰', '╯', '─', '│')
    } else {
        ('+', '+', '+', '+', '-', '|')
    };
    let width = width as usize;
    let title = if unicode {
        format!("{h} ◢ {} // ONLINE ", brand::WORDMARK)
    } else {
        format!("{h} {} // ONLINE ", brand::MARK)
    };
    let title_width = brand::visible_width(&title);
    let mut lines = Vec::with_capacity(body.len() + 2);
    lines.push(Line::from(vec![
        Span::styled(tl.to_string(), t.primary()),
        Span::styled(title, t.brand()),
        Span::styled(
            h.to_string()
                .repeat(width.saturating_sub(title_width + 2))
                .to_string(),
            t.primary(),
        ),
        Span::styled(tr.to_string(), t.primary()),
    ]));
    let inner = width.saturating_sub(4);
    for line in body {
        // A row that outgrew the panel is clipped here rather than allowed to
        // push the right border off the end: a frame that does not close reads
        // as a rendering bug, whatever caused the overflow.
        let line = clamp(line, inner);
        let used = line
            .spans
            .iter()
            .map(|span| brand::visible_width(&span.content))
            .sum::<usize>();
        let mut spans = vec![Span::styled(format!("{v} "), t.primary())];
        spans.extend(line.spans);
        spans.push(Span::styled(
            format!("{}{v}", " ".repeat(inner.saturating_sub(used) + 1)),
            t.primary(),
        ));
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(Span::styled(
        format!("{bl}{}{br}", h.to_string().repeat(width.saturating_sub(2))),
        t.primary(),
    )));
    lines
}

/// Trim a line's spans to a visible width, dropping and cutting from the end.
fn clamp(line: Line<'static>, max: usize) -> Line<'static> {
    let total: usize = line
        .spans
        .iter()
        .map(|span| brand::visible_width(&span.content))
        .sum();
    if total <= max {
        return line;
    }
    let mut kept: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in line.spans {
        let width = brand::visible_width(&span.content);
        if used + width <= max {
            used += width;
            kept.push(span);
            continue;
        }
        let room = max.saturating_sub(used);
        if room > 0 {
            kept.push(Span::styled(truncate_end(&span.content, room), span.style));
        }
        break;
    }
    Line::from(kept)
}

fn indent(line: Line<'static>, prefix: &str) -> Line<'static> {
    let mut spans = vec![Span::raw(prefix.to_string())];
    spans.extend(line.spans);
    Line::from(spans)
}

/// Truncate keeping the *end* — for paths, where the project name is the point.
fn truncate_start(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max || max == 0 {
        return text.to_string();
    }
    let kept: String = text.chars().skip(count - max.saturating_sub(1)).collect();
    format!("…{kept}")
}

/// Truncate keeping the beginning — for prose.
fn truncate_end(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max || max == 0 {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_app::boot::{BootNotice, MemoryState, SessionState, Tip};

    fn plain(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn snapshot() -> BootSnapshot {
        BootSnapshot {
            version: "2.11.0".into(),
            workspace: "~/Airsec_Inc/SP_Product/Silent-Nexus".into(),
            model: "Ollama / sans-ai-v:latest".into(),
            agent: "implementer".into(),
            access: "path-validation-only".into(),
            greeting: "Welcome back.".into(),
            session: Some(SessionState {
                title: Some("fix the tier check".into()),
                branch: Some("feat/nexus-rsi-flagship".into()),
                when: Some("23 Jul 2026".into()),
                resumable: true,
            }),
            memory: Some(MemoryState {
                linked: 1,
                pending: 2,
                improvements: 0,
            }),
            update: Some("Governed self-improvement".into()),
            tips: vec![
                Tip {
                    command: "/resume".into(),
                    detail: "continue the restored session".into(),
                },
                Tip {
                    command: "/memory".into(),
                    detail: "review what the agent recorded".into(),
                },
            ],
            notices: Vec::new(),
        }
    }

    /// **The bug this whole module exists for.** Startup state is not a run
    /// outcome, so none of the timeline's status vocabulary may appear in it.
    #[test]
    fn the_panel_never_uses_run_outcome_vocabulary() {
        for width in [36, 44, 60, 80, 120] {
            let text = plain(&panel_lines(&snapshot(), width, 40, &Theme::plain(), true));
            for forbidden in ["DONE", "NOTICE", "FAILED", "✓ ", "COMPLETED"] {
                assert!(
                    !text.contains(forbidden),
                    "`{forbidden}` at width {width}:\n{text}"
                );
            }
        }
    }

    /// One panel, and every fact in it exactly once — the duplication that made
    /// startup read as four separate cards.
    #[test]
    fn every_fact_appears_exactly_once() {
        let text = plain(&panel_lines(&snapshot(), 100, 40, &Theme::plain(), true));
        for fact in [
            "~/Airsec_Inc/SP_Product/Silent-Nexus",
            "Ollama / sans-ai-v:latest",
            "implementer",
            "path-validation-only",
            "Governed self-improvement",
            "/resume",
        ] {
            assert_eq!(text.matches(fact).count(), 1, "`{fact}` in:\n{text}");
        }
    }

    #[test]
    fn identity_is_present_at_every_width() {
        for width in [36, 44, 60, 80, 120] {
            let text = plain(&panel_lines(&snapshot(), width, 40, &Theme::plain(), true));
            assert!(text.contains(brand::WORDMARK), "width {width}:\n{text}");
            assert!(text.contains('◢'), "no mark at width {width}:\n{text}");
        }
    }

    /// The ASCII tier has to stay aligned: a terminal that cannot draw the box
    /// characters still gets a panel, not a broken one.
    #[test]
    fn the_ascii_fallback_contains_no_box_drawing() {
        let text = plain(&panel_lines(&snapshot(), 80, 40, &Theme::plain(), false));
        for glyph in ['╭', '╮', '╰', '╯', '─', '│', '◢'] {
            assert!(!text.contains(glyph), "{glyph} in ascii panel:\n{text}");
        }
        assert!(text.contains(brand::MARK), "{text}");
    }

    /// A border that does not close is worse than no border. Every row of a
    /// bordered panel is exactly the requested width.
    #[test]
    fn every_bordered_row_is_exactly_the_panel_width() {
        for width in [44u16, 60, 72, 100, 160] {
            for lines in [
                panel_lines(&snapshot(), width, 40, &Theme::plain(), true),
                panel_lines(&BootSnapshot::default(), width, 40, &Theme::plain(), true),
            ] {
                for line in &lines {
                    let rendered = line
                        .spans
                        .iter()
                        .map(|span| span.content.as_ref())
                        .collect::<String>();
                    assert_eq!(
                        brand::visible_width(&rendered),
                        width as usize,
                        "width {width}: {rendered:?}"
                    );
                }
            }
        }
    }

    /// A fresh workspace has no session, no memory, and no news. It must not
    /// render empty section headers to say so.
    #[test]
    fn a_fresh_workspace_omits_the_sections_it_has_nothing_for() {
        let fresh = BootSnapshot {
            version: "2.11.0".into(),
            workspace: "~/new".into(),
            model: "Ollama / qwen".into(),
            agent: "orchestrator".into(),
            access: "path-validation-only".into(),
            greeting: "Workspace ready.".into(),
            tips: vec![Tip {
                command: "/help".into(),
                detail: "keys and commands".into(),
            }],
            ..Default::default()
        };
        let text = plain(&panel_lines(&fresh, 80, 40, &Theme::plain(), true));
        assert!(!text.contains("SESSION"), "{text}");
        assert!(!text.contains("MEMORY"), "{text}");
        assert!(!text.contains("UPDATE"), "{text}");
        assert!(text.contains("Workspace ready."), "{text}");
    }

    /// A blocked subsystem is stated in words, not only in red.
    #[test]
    fn a_blocked_subsystem_reads_without_color() {
        let mut snapshot = snapshot();
        snapshot.notices.push(BootNotice {
            subsystem: "MODELS".into(),
            level: NoticeLevel::Blocked,
            detail: "none configured — /setup gets you talking to an agent".into(),
        });
        let text = plain(&panel_lines(&snapshot, 100, 40, &Theme::plain(), true));
        assert!(text.contains("MODELS // ACTION"), "{text}");
        assert!(text.contains("/setup"), "{text}");
    }

    /// Mobile portrait: no border to corrupt, one tip, and the project name
    /// survives the path truncation.
    #[test]
    fn a_narrow_terminal_drops_the_border_and_keeps_the_project_name() {
        let lines = panel_lines(&snapshot(), 36, 40, &Theme::plain(), true);
        let text = plain(&lines);
        assert!(!text.contains('╭'), "{text}");
        assert!(text.contains("Silent-Nexus"), "{text}");
        assert_eq!(text.matches("/resume").count(), 1, "{text}");
        // A phone gets at most two tips, each on its own line — never the
        // joined row, which would wrap into a second line anyway.
        let tips = lines
            .iter()
            .filter(|line| line.spans.iter().any(|span| span.content.starts_with('/')))
            .count();
        assert!(tips <= 2, "{tips} tips on a phone:\n{text}");
        for line in &lines {
            let rendered = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            assert!(brand::visible_width(&rendered) <= 36, "{rendered:?}");
        }
    }

    /// The reserved height and the rendered height are the same number, or the
    /// panel gets clipped mid-border.
    #[test]
    fn the_reserved_row_count_matches_what_is_drawn() {
        for width in [36u16, 44, 60, 80, 120] {
            let rows = panel_rows(&snapshot(), width, 40, true);
            let drawn = panel_lines(&snapshot(), width, 40, &Theme::plain(), true).len();
            assert_eq!(rows as usize, drawn, "width {width}");
        }
    }

    #[test]
    fn the_collapsed_line_keeps_identity_and_fits_one_row() {
        let snapshot = snapshot();
        let text = collapsed_text(&snapshot, &snapshot.agent, &snapshot.model);
        assert!(text.contains("NEXUS"), "{text}");
        assert!(text.contains("implementer"), "{text}");
        assert!(!text.contains('\n'), "{text}");
    }

    /// The collapsed panel sits directly under the header, which shows the
    /// live agent and model. Rendering the boot values there put two
    /// contradictory lines next to each other as soon as anyone ran `/model`.
    #[test]
    fn the_collapsed_line_follows_the_session_not_the_startup_snapshot() {
        let snapshot = snapshot();
        let text = collapsed_text(&snapshot, "reviewer", "Codex / gpt-5.6-luna");
        assert!(text.contains("reviewer"), "{text}");
        assert!(text.contains("Codex / gpt-5.6-luna"), "{text}");
        assert!(
            !text.contains(&snapshot.model),
            "the startup model is still being claimed: {text}"
        );
        // Facts about startup are still facts about startup.
        assert!(text.contains("NEXUS"), "{text}");
    }

    fn collapsed_text(snapshot: &BootSnapshot, agent: &str, model: &str) -> String {
        collapsed_line(snapshot, agent, model, &Theme::plain(), true)
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    /// Terminals that report no color still have to be readable: every state is
    /// named, never signalled by color alone.
    #[test]
    fn no_color_still_names_every_state() {
        let text = plain(&panel_lines(&snapshot(), 100, 40, &Theme::plain(), true));
        assert!(text.contains("SESSION // RESTORED"), "{text}");
        assert!(text.contains("MEMORY // LINKED"), "{text}");
        assert!(text.contains("2 awaiting review"), "{text}");
    }

    /// A short terminal gets a shorter panel, not a missing one, and never one
    /// that overruns the rows it was given.
    #[test]
    fn a_tight_height_sheds_content_instead_of_overflowing() {
        for budget in 6u16..=20 {
            let lines = panel_lines(&snapshot(), 80, budget, &Theme::plain(), true);
            assert!(
                lines.len() <= budget as usize,
                "budget {budget} produced {} rows",
                lines.len()
            );
            let text = plain(&lines);
            // Identity survives every budget; it is what makes the panel a
            // panel rather than a stray block of text.
            assert!(text.contains(brand::WORDMARK), "budget {budget}:\n{text}");
            // Metadata survives anything roomier than the floor.
            if budget >= 12 {
                assert!(
                    text.contains("Ollama / sans-ai-v:latest"),
                    "budget {budget}"
                );
                assert!(text.contains("implementer"), "budget {budget}");
            }
        }
    }

    /// Shedding never leaves a blank row dangling above the border.
    #[test]
    fn a_shed_panel_has_no_trailing_blank_row() {
        for budget in 6u16..=20 {
            let lines = panel_lines(&snapshot(), 80, budget, &Theme::plain(), true);
            let last_body = lines
                .get(lines.len().saturating_sub(2))
                .map(|line| plain(std::slice::from_ref(line)))
                .unwrap_or_default();
            assert!(
                !last_body.trim_matches(['│', ' ']).is_empty(),
                "budget {budget} ended on a blank row"
            );
        }
    }

    /// The one thing that must survive every budget: something the operator has
    /// to act on. A panel that hides "no models configured" to save a row has
    /// failed at the only job it has on a fresh install.
    #[test]
    fn an_action_notice_survives_the_tightest_budget() {
        let mut snapshot = snapshot();
        snapshot.notices.push(BootNotice {
            subsystem: "MODELS".into(),
            level: NoticeLevel::Blocked,
            detail: "none configured — /setup gets you talking to an agent".into(),
        });
        for budget in 6u16..=14 {
            let text = plain(&panel_lines(&snapshot, 80, budget, &Theme::plain(), true));
            assert!(
                text.contains("MODELS // ACTION"),
                "budget {budget}:\n{text}"
            );
        }
    }

    /// Whatever the width, drawing must not panic — including sizes below the
    /// panel's own minimums.
    #[test]
    fn no_width_panics() {
        for width in 0u16..=200 {
            let _ = panel_lines(&snapshot(), width, 40, &Theme::plain(), true);
            let _ = panel_lines(&BootSnapshot::default(), width, 40, &Theme::plain(), false);
        }
    }
}
