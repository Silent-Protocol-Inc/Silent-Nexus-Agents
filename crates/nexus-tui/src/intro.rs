//! Fast, calm NEXUS startup sequence.
//!
//! The canonical lockup comes from `nexus-core`; this module only stages its
//! reveal through the active TUI theme. Elements fade in by role — identity
//! mark first, then the wordmark, then supporting copy — within a sub-second
//! budget. Animation is cosmetic, skippable, and never represents progress.

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

/// Reveal stages: 0 icon in, 1 wordmark in, 2 supporting copy in, 3 all solid.
const STAGES: usize = 4;
const FRAME_DELAY_MS: u64 = 90;
const FRAME_DELAY: Duration = Duration::from_millis(FRAME_DELAY_MS);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visibility {
    Hidden,
    Soft,
    Full,
}

/// Staged fade: each role appears dim one stage before it resolves to full
/// color, so the identity mark leads and the tagline trails.
fn visibility(role: BrandRole, stage: usize) -> Visibility {
    let (soft, full) = match role {
        BrandRole::Icon => (0, 1),
        BrandRole::Wordmark => (1, 2),
        BrandRole::Attribution | BrandRole::Tagline => (2, 3),
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

fn frame_lines(lockup: &BrandLockup, stage: usize, theme: &Theme) -> Vec<Line<'static>> {
    lockup
        .lines
        .iter()
        .map(|line| {
            let spans = line
                .spans
                .iter()
                .map(|span| match visibility(span.role, stage) {
                    // Preserve cell width so the composition never reflows.
                    Visibility::Hidden => Span::raw(" ".repeat(brand::visible_width(&span.text))),
                    // The "fade": real glyphs, dimmed, before they resolve.
                    Visibility::Soft => Span::styled(span.text.clone(), theme.muted()),
                    Visibility::Full => Span::styled(
                        span.text.clone(),
                        role_style(span.role, theme, lockup.monochrome),
                    ),
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
        frame.render_widget(Paragraph::new(frame_lines(&lockup, stage, theme)), area);
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
            draw_stage(terminal, theme, STAGES - 1)?;
            return Ok(());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorSupport;

    fn normal_duration_ms() -> u128 {
        FRAME_DELAY.as_millis() * STAGES as u128
    }

    #[test]
    fn normal_animation_stays_inside_target_budget() {
        let duration = normal_duration_ms();
        assert_eq!(duration, 360);
        assert!(duration <= 400);
    }

    #[test]
    fn reduced_motion_always_disables_boot_animation() {
        assert!(animation_disabled(true));
    }

    #[test]
    fn reveal_order_keeps_nexus_ahead_of_supporting_copy() {
        assert_eq!(visibility(BrandRole::Icon, 0), Visibility::Soft);
        assert_eq!(visibility(BrandRole::Wordmark, 0), Visibility::Hidden);
        assert_eq!(visibility(BrandRole::Icon, 1), Visibility::Full);
        assert_eq!(visibility(BrandRole::Wordmark, 1), Visibility::Soft);
        assert_eq!(visibility(BrandRole::Attribution, 1), Visibility::Hidden);
        assert_eq!(visibility(BrandRole::Wordmark, 2), Visibility::Full);
        assert_eq!(visibility(BrandRole::Tagline, 2), Visibility::Soft);
        assert_eq!(visibility(BrandRole::Tagline, 3), Visibility::Full);
    }

    #[test]
    fn rendered_frames_preserve_lockup_width_without_color() {
        let lockup = brand::lockup(BrandVariant::Full, BrandConstraints::default());
        let theme = Theme::new("mono", ColorSupport::None);
        for stage in 0..STAGES {
            let lines = frame_lines(&lockup, stage, &theme);
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
        let lines = frame_lines(&lockup, STAGES - 1, &theme);
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
