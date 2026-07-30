//! The wake flow: what NEXUS says when it comes up.
//!
//! Startup used to be a brand reveal followed by three unrelated `st.system(…)`
//! lines pushed from two different files, with no owner and no ordering
//! guarantee. This module gathers the facts; the TUI renders them.
//!
//! One rule governs the whole sequence: **a stage with nothing real to say is
//! omitted, never faked.** A fresh workspace has no session to restore and no
//! memory to link, so it simply does not show those lines — an empty
//! "Session restored · none" would be worse than silence. Nothing here is a
//! progress bar either: startup is not measurable in advance, so nothing
//! pretends to measure it.

use crate::app::App;
use nexus_core::brand::ActionState;

/// One line of the wake flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootLine {
    /// Supplies the icon from the design language.
    pub state: ActionState,
    /// Short label: "Session restored", "Memory linked".
    pub label: String,
    /// The facts, already joined for display. Never a path to an internal file,
    /// never a timing of an internal step.
    pub detail: String,
}

impl BootLine {
    fn new(state: ActionState, label: &str, detail: impl Into<String>) -> Self {
        Self {
            state,
            label: label.to_string(),
            detail: detail.into(),
        }
    }
}

/// The curated wake sequence, in order. Empty is a legitimate answer.
pub fn wake_flow(app: &App) -> Vec<BootLine> {
    [
        session_restore(app),
        memory_link(app),
        whats_new(app),
        welcome(app),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// What the operator was last doing here, when there was a last time.
fn session_restore(app: &App) -> Option<BootLine> {
    let session = app.read_ui_state(|state| state.last_session.clone())?;
    let meta = app.sessions().get(&session).ok()?;
    let mut facts = Vec::new();
    if let Some(title) = Some(meta.title.trim()).filter(|title| !title.is_empty()) {
        facts.push(title.chars().take(48).collect::<String>());
    }
    if let Some(branch) = crate::gitx::branch(&app.workspace) {
        facts.push(branch);
    }
    facts.push(meta.created_at.clone());
    Some(BootLine::new(
        ActionState::ShapingApproach,
        "Session restored",
        facts.join(" · "),
    ))
}

/// What the harness remembers about this workspace, and what is waiting on a
/// human. Both counts are real reads; zero of everything means no line.
fn memory_link(app: &App) -> Option<BootLine> {
    let memories = app.harness().memories(None, None, true, 10_000).ok()?;
    let active = memories
        .iter()
        .filter(|record| record.status == nexus_core::harness::MemoryStatus::Active)
        .count();
    let candidates = memories
        .iter()
        .filter(|record| record.status == nexus_core::harness::MemoryStatus::Candidate)
        .count();
    let awaiting = app
        .harness()
        .workspace_repository()
        .improvement_proposals(None)
        .map(|proposals| {
            proposals
                .iter()
                .filter(|proposal| crate::status::awaits_human(proposal.status))
                .count()
        })
        .unwrap_or(0);

    if active == 0 && candidates == 0 && awaiting == 0 {
        return None;
    }
    let mut facts = Vec::new();
    if active > 0 {
        facts.push(format!("{active} fact{}", plural(active)));
    }
    if candidates > 0 {
        facts.push(format!("{candidates} awaiting review"));
    }
    if awaiting > 0 {
        facts.push(format!(
            "{awaiting} improvement candidate{} — /rsi",
            plural(awaiting)
        ));
    }
    Some(BootLine::new(
        ActionState::Composing,
        "Memory linked",
        facts.join(" · "),
    ))
}

/// The headline for the running version, shown once per version.
///
/// Read from the changelog compiled into the binary, so it cannot drift from
/// what actually shipped and cannot claim a feature this build does not have.
fn whats_new(app: &App) -> Option<BootLine> {
    let version = nexus_core::brand::VERSION;
    let seen = app.read_ui_state(|state| state.last_seen_version.clone());
    if seen == version {
        return None;
    }
    let headline = changelog_headline(CHANGELOG, version)?;
    Some(BootLine::new(
        ActionState::TracingIntent,
        "What's new",
        headline,
    ))
}

/// Where to go next, when there is somewhere real to point.
fn welcome(app: &App) -> Option<BootLine> {
    let hint = crate::services::next_step_hint(app)
        .unwrap_or_else(|| "Type a message, / for commands, Ctrl+K for the palette.".into());
    Some(BootLine::new(ActionState::Done, "Ready", hint))
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

/// First bolded headline under `## [version]`, e.g. `- **Thing happened.**`.
///
/// Returns `None` when the version has no section — a build from an unreleased
/// working tree says nothing rather than showing the previous release's news.
fn changelog_headline(changelog: &str, version: &str) -> Option<String> {
    let heading = format!("## [{version}]");
    let section = changelog.split(&heading).nth(1)?;
    let section = section.split("\n## ").next().unwrap_or(section);
    for line in section.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("- **") else {
            continue;
        };
        let headline = rest.split("**").next()?.trim().trim_end_matches(['.', ':']);
        if headline.is_empty() {
            continue;
        }
        // A boot line is a glance. Changelog headlines are written to be read
        // in a document and routinely run past a terminal row, so this one is
        // cut rather than allowed to wrap the whole wake flow.
        const MAX: usize = 64;
        if headline.chars().count() <= MAX {
            return Some(headline.to_string());
        }
        let kept: String = headline.chars().take(MAX - 1).collect();
        let kept = kept.trim_end().to_string();
        return Some(format!("{kept}…"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# Changelog

## [2.11.0] — 2026-07-30

### Added

- **Governed self-improvement.** Long body text that should not be shown.
- **Something else.** More text.

## [2.10.2] — 2026-07-25

- **An older headline.** Body.
";

    #[test]
    fn the_headline_comes_from_the_running_version_only() {
        assert_eq!(
            changelog_headline(SAMPLE, "2.11.0").as_deref(),
            Some("Governed self-improvement")
        );
        assert_eq!(
            changelog_headline(SAMPLE, "2.10.2").as_deref(),
            Some("An older headline")
        );
    }

    /// A build whose version has no changelog section says nothing, rather than
    /// showing the previous release's news as if it were this one's.
    #[test]
    fn an_unknown_version_produces_no_headline() {
        assert_eq!(changelog_headline(SAMPLE, "9.9.9"), None);
        assert_eq!(changelog_headline("", "2.11.0"), None);
    }

    /// The shipped changelog has to actually parse — otherwise the stage would
    /// silently vanish for every real build.
    #[test]
    fn the_shipped_changelog_yields_a_headline_for_this_build() {
        let headline = changelog_headline(CHANGELOG, nexus_core::brand::VERSION)
            .expect("the running version needs a changelog section");
        assert!(!headline.is_empty());
        assert!(!headline.contains("**"), "{headline}");
    }

    /// The wake flow is a glance; a changelog headline is written for a
    /// document and routinely outruns a terminal row.
    #[test]
    fn a_long_headline_is_cut_to_a_glance() {
        let long = format!("## [9.0.0]\n\n- **{}.** body\n", "x".repeat(200));
        let headline = changelog_headline(&long, "9.0.0").expect("headline");
        assert!(headline.chars().count() <= 64, "{headline}");
        assert!(headline.ends_with('…'));
    }

    #[test]
    fn plurals_read_correctly() {
        assert_eq!(format!("1 fact{}", plural(1)), "1 fact");
        assert_eq!(format!("2 fact{}", plural(2)), "2 facts");
    }
}
