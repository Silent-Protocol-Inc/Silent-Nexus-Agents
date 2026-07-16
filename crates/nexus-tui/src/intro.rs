//! Fast, responsive NEXUS-first startup sequence.
//!
//! The canonical lockup comes from `nexus-core`; this module only stages its
//! semantic roles through the active TUI theme. The sequence layers a short
//! CRT-style power-on (rows expand from the center scanline), a sweeping
//! scanline, and per-glyph glitch noise during each element's soft phase.
//! Animation is cosmetic, skippable, and never represents loading progress.

use crate::theme::Theme;
use crossterm::event::{self, Event, KeyEventKind};
use nexus_core::brand::{self, BrandConstraints, BrandLockup, BrandRole, BrandVariant};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Terminal;
use std::io::{IsTerminal, Stdout};
use std::time::Duration;

const STAGES: usize = 8;
const FRAME_DELAY_MS: u64 = 50;
const FINAL_HOLD_MS: u64 = 80;
const FRAME_DELAY: Duration = Duration::from_millis(FRAME_DELAY_MS);
const FINAL_HOLD: Duration = Duration::from_millis(FINAL_HOLD_MS);

/// Stages 0..CRT_OPEN_STAGES ramp the visible band open like a CRT warming up.
const CRT_OPEN_STAGES: usize = 3;

const GLITCH_UNICODE: [char; 6] = ['▓', '▒', '░', '╳', '▚', '·'];
const GLITCH_ASCII: [char; 6] = ['#', '%', '*', '+', ':', '.'];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visibility {
    Hidden,
    Soft,
    Full,
}

fn visibility(role: BrandRole, stage: usize) -> Visibility {
    let (soft, full) = match role {
        BrandRole::Icon => (1, 3),
        BrandRole::Wordmark => (3, 5),
        BrandRole::Attribution => (5, 6),
        BrandRole::Tagline => (6, 7),
        BrandRole::Spacer => return Visibility::Full,
    };
    if stage >= full {
        Visibility::Full
    } else if stage >= soft {
        Visibility::Soft
    } else {
        Visibility::Hidden
    }
}

/// Rows within this distance of the vertical center are visible — the CRT
/// power-on band. Fully open (returns `usize::MAX`) once the open stages pass.
fn crt_band(stage: usize, height: usize) -> usize {
    if stage >= CRT_OPEN_STAGES {
        return usize::MAX;
    }
    // stage 0 → thin center line, then widen towards full height.
    let half = height.div_ceil(2);
    (half * (stage + 1)).div_ceil(CRT_OPEN_STAGES).max(1)
}

/// The bright scanline row for this stage, sweeping top→bottom while the
/// composition resolves. The last stage is clean (no scanline).
fn scan_row(stage: usize, height: usize) -> Option<usize> {
    if height == 0 || stage == 0 || stage >= STAGES - 1 {
        return None;
    }
    Some((height - 1) * (stage - 1) / (STAGES - 3).max(1))
}

/// Deterministic per-cell glitch decision — stable within a stage, different
/// across stages, no RNG state.
fn glitched(row: usize, col: usize, stage: usize) -> bool {
    (row.wrapping_mul(31))
        .wrapping_add(col.wrapping_mul(17))
        .wrapping_add(stage.wrapping_mul(13))
        .is_multiple_of(5)
}

/// Replace a soft-phase glyph with glitch noise while preserving cell width.
fn glitch_text(text: &str, row: usize, col_offset: usize, stage: usize, unicode: bool) -> String {
    let set = if unicode {
        &GLITCH_UNICODE
    } else {
        &GLITCH_ASCII
    };
    text.chars()
        .enumerate()
        .map(|(i, ch)| {
            if ch != ' ' && glitched(row, col_offset + i, stage) {
                set[(row + col_offset + i + stage) % set.len()]
            } else {
                ch
            }
        })
        .collect()
}

fn role_style(role: BrandRole, theme: &Theme, monochrome: bool) -> Style {
    if monochrome {
        return match role {
            BrandRole::Icon | BrandRole::Wordmark => theme.text().add_modifier(Modifier::BOLD),
            BrandRole::Attribution | BrandRole::Tagline | BrandRole::Spacer => theme.muted(),
        };
    }
    match role {
        BrandRole::Icon => theme.secondary().add_modifier(Modifier::BOLD),
        BrandRole::Wordmark => theme.brand(),
        BrandRole::Attribution => theme.muted(),
        BrandRole::Tagline => theme.muted().add_modifier(Modifier::BOLD),
        BrandRole::Spacer => theme.text(),
    }
}

fn frame_lines(
    lockup: &BrandLockup,
    stage: usize,
    theme: &Theme,
    unicode: bool,
) -> Vec<Line<'static>> {
    let height = lockup.lines.len();
    let center = height / 2;
    let band = crt_band(stage, height);
    let scan = scan_row(stage, height);
    let clean = stage >= STAGES - 1;

    lockup
        .lines
        .iter()
        .enumerate()
        .map(|(row, line)| {
            // CRT power-on: rows outside the band stay dark but keep height.
            if row.abs_diff(center) >= band {
                let width: usize = line
                    .spans
                    .iter()
                    .map(|s| brand::visible_width(&s.text))
                    .sum();
                return Line::from(Span::raw(" ".repeat(width)));
            }
            let on_scanline = !clean && scan == Some(row);
            let mut col = 0usize;
            let spans = line
                .spans
                .iter()
                .map(|span| {
                    let span_cols = brand::visible_width(&span.text);
                    let start_col = col;
                    col += span_cols;
                    let rendered = match visibility(span.role, stage) {
                        Visibility::Hidden => {
                            Span::raw(" ".repeat(brand::visible_width(&span.text)))
                        }
                        Visibility::Soft => Span::styled(
                            if clean {
                                span.text.clone()
                            } else {
                                glitch_text(&span.text, row, start_col, stage, unicode)
                            },
                            theme.muted(),
                        ),
                        Visibility::Full => Span::styled(
                            span.text.clone(),
                            role_style(span.role, theme, lockup.monochrome),
                        ),
                    };
                    if on_scanline {
                        rendered.patch_style(Style::default().add_modifier(Modifier::REVERSED))
                    } else {
                        rendered
                    }
                })
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect()
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}

fn animation_disabled(reduced_motion: bool) -> bool {
    reduced_motion
        || std::env::var_os("SNX_REDUCED_MOTION").is_some()
        || std::env::var_os("REDUCED_MOTION").is_some()
        || std::env::var_os("CI").is_some()
        || std::env::var("TERM").is_ok_and(|term| term == "dumb")
}

fn draw_stage(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    theme: &Theme,
    stage: usize,
) -> std::io::Result<()> {
    let unicode = brand::unicode_supported();
    terminal.draw(|frame| {
        let terminal_area = frame.area();
        let lockup = brand::lockup(
            BrandVariant::Full,
            BrandConstraints {
                width: terminal_area.width,
                height: terminal_area.height,
                unicode,
            },
        );
        let area = centered(terminal_area, lockup.width, lockup.height);
        frame.render_widget(
            Paragraph::new(frame_lines(&lockup, stage, theme, unicode)),
            area,
        );
    })?;
    Ok(())
}

/// Wait for the next frame. A key skips immediately; resize events are
/// consumed and the following draw recalculates the complete composition.
fn wait_or_skip(duration: Duration) -> std::io::Result<bool> {
    if !event::poll(duration)? {
        return Ok(false);
    }
    match event::read()? {
        Event::Key(key) if key.kind != KeyEventKind::Release => Ok(true),
        _ => Ok(false),
    }
}

/// Play the startup sequence. Non-interactive, CI, and reduced-motion runs
/// return without a timed animation.
pub fn play(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    theme: &Theme,
    reduced_motion: bool,
) -> std::io::Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Ok(());
    }
    if animation_disabled(reduced_motion) {
        draw_stage(terminal, theme, STAGES - 1)?;
        return Ok(());
    }

    for stage in 0..STAGES {
        draw_stage(terminal, theme, stage)?;
        if wait_or_skip(FRAME_DELAY)? {
            return Ok(());
        }
    }
    let _ = wait_or_skip(FINAL_HOLD)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorSupport;

    fn normal_duration_ms() -> u128 {
        FRAME_DELAY.as_millis() * STAGES as u128 + FINAL_HOLD.as_millis()
    }

    #[test]
    fn normal_animation_stays_inside_target_budget() {
        let duration = normal_duration_ms();
        assert_eq!(duration, 480);
        assert!(duration <= 500);
    }

    #[test]
    fn reduced_motion_always_disables_boot_animation() {
        assert!(animation_disabled(true));
    }

    #[test]
    fn reveal_order_keeps_nexus_ahead_of_supporting_copy() {
        assert_eq!(visibility(BrandRole::Icon, 1), Visibility::Soft);
        assert_eq!(visibility(BrandRole::Wordmark, 1), Visibility::Hidden);
        assert_eq!(visibility(BrandRole::Wordmark, 5), Visibility::Full);
        assert_eq!(visibility(BrandRole::Attribution, 5), Visibility::Soft);
        assert_eq!(visibility(BrandRole::Tagline, 6), Visibility::Soft);
        assert_eq!(visibility(BrandRole::Tagline, 7), Visibility::Full);
    }

    #[test]
    fn crt_band_opens_from_center_then_fully() {
        assert_eq!(crt_band(0, 17), 3);
        assert!(crt_band(1, 17) > crt_band(0, 17));
        assert_eq!(crt_band(CRT_OPEN_STAGES, 17), usize::MAX);
    }

    #[test]
    fn scanline_sweeps_down_and_final_frame_is_clean() {
        assert_eq!(scan_row(0, 17), None);
        assert_eq!(scan_row(1, 17), Some(0));
        let mid = scan_row(4, 17).expect("mid stage has a scanline");
        let late = scan_row(STAGES - 2, 17).expect("last sweep stage has a scanline");
        assert!(mid < late);
        assert_eq!(late, 16);
        assert_eq!(scan_row(STAGES - 1, 17), None);
    }

    #[test]
    fn glitch_preserves_cell_width_and_spaces() {
        let noisy = glitch_text("█▄  █  █████", 2, 0, 2, true);
        assert_eq!(brand::visible_width(&noisy), 12);
        for (orig, out) in "█▄  █  █████".chars().zip(noisy.chars()) {
            if orig == ' ' {
                assert_eq!(out, ' ');
            }
        }
        let ascii = glitch_text("##  #  #####", 2, 0, 2, false);
        assert!(ascii.is_ascii());
        assert_eq!(ascii.len(), 12);
    }

    #[test]
    fn rendered_frames_preserve_lockup_width_without_color() {
        let lockup = brand::lockup(BrandVariant::Full, BrandConstraints::default());
        let theme = Theme::new("mono", ColorSupport::None);
        for stage in 0..STAGES {
            let lines = frame_lines(&lockup, stage, &theme, true);
            assert_eq!(lines.len(), lockup.height as usize);
            for line in lines {
                let text = line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<Vec<_>>()
                    .concat();
                assert!(brand::visible_width(&text) <= lockup.width as usize);
            }
        }
    }

    #[test]
    fn final_stage_matches_pure_lockup_text() {
        let lockup = brand::lockup(BrandVariant::Full, BrandConstraints::default());
        let theme = Theme::new("nexus-dark", ColorSupport::TrueColor);
        let lines = frame_lines(&lockup, STAGES - 1, &theme, true);
        for (rendered, canonical) in lines.iter().zip(&lockup.lines) {
            let text = rendered
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<Vec<_>>()
                .concat();
            assert_eq!(text, canonical.text());
        }
    }

    #[test]
    fn centering_tracks_terminal_resizes() {
        let small = centered(Rect::new(0, 0, 60, 20), 37, 17);
        let large = centered(Rect::new(0, 0, 120, 40), 37, 17);
        assert_eq!(small.y, 1);
        assert_eq!(large.y, 11);
        assert!(large.x > small.x);
    }
}
