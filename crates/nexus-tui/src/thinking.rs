//! Thinking phases and the summarization engine.
//!
//! Two rules govern everything here.
//!
//! First, a phase is a **pure projection of state**, never an event. The widget
//! is recomputed from `State` every frame, so a phase transition updates one
//! widget instead of appending a timeline entry — that property is structural,
//! not a convention to be maintained.
//!
//! Second, the summarizer reports **what the harness is doing**, derived from
//! structured runtime state: the active tool, the running stage, pending
//! validations. It never paraphrases model prose and never speculates about
//! intent. Where a provider supplies its own summary the harness prefers it;
//! raw provider reasoning is destroyed at ingestion and cannot appear here.

use crate::state::State;
use nexus_core::timeline::TimelineKind;

/// Hard ceiling on rendered preview lines, independent of configuration.
/// `[tui.activity].reasoning_preview_lines` may lower this but never raise it.
pub const MAX_PREVIEW_LINES: usize = 3;

/// What the harness is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThinkingState {
    #[default]
    Understanding,
    Planning,
    Searching,
    Executing,
    Waiting,
    Verifying,
    Finalizing,
}

impl ThinkingState {
    /// Heading text.
    ///
    /// `Understanding` and `Finalizing` deliberately collapse onto the shipped
    /// `PROCESSING` label: the six strings this can return are exactly the six
    /// that 2.3.0 returned, so adding phases changed no existing output.
    pub fn title(&self) -> &'static str {
        match self {
            ThinkingState::Waiting => "WAITING",
            ThinkingState::Searching => "SEARCHING",
            ThinkingState::Executing => "EXECUTING",
            ThinkingState::Verifying => "VERIFYING",
            ThinkingState::Planning => "PLANNING",
            ThinkingState::Understanding | ThinkingState::Finalizing => "PROCESSING",
        }
    }

    /// Action-oriented fallback used when no richer structured detail exists.
    pub fn summary(&self) -> &'static str {
        match self {
            ThinkingState::Understanding => "Reading the request.",
            ThinkingState::Planning => "Preparing a plan for review.",
            ThinkingState::Searching => "Searching for relevant material.",
            ThinkingState::Executing => "Running the selected tool.",
            ThinkingState::Waiting => "Waiting on the provider.",
            ThinkingState::Verifying => "Verifying the result.",
            ThinkingState::Finalizing => "Composing the answer.",
        }
    }

    /// Waiting is the one phase that wants the operator's attention, so it is
    /// the one phase that colors its heading differently.
    pub fn is_blocked(&self) -> bool {
        matches!(self, ThinkingState::Waiting)
    }
}

/// A concise line describing what a tool call is doing, from the tool's name
/// and arguments rather than from anything the model said about it.
fn tool_line(tool: &str) -> String {
    let lower = tool.to_ascii_lowercase();
    // Longest / most specific prefixes first.
    if lower.starts_with("web.search") {
        "Searching the web.".into()
    } else if lower.starts_with("web.fetch") || lower.starts_with("web.head") {
        "Reading an external page.".into()
    } else if lower.starts_with("web.download") {
        "Downloading a file.".into()
    } else if lower.starts_with("fs.search_text") || lower.starts_with("fs.find_files") {
        "Searching the workspace.".into()
    } else if lower.starts_with("fs.read")
        || lower.starts_with("fs.list")
        || lower.starts_with("fs.stat")
        || lower.starts_with("fs.hash")
    {
        "Inspecting workspace files.".into()
    } else if lower.starts_with("fs.create")
        || lower.starts_with("fs.patch")
        || lower.starts_with("fs.move")
        || lower.starts_with("fs.copy")
        || lower.starts_with("fs.delete")
        || lower.starts_with("fs.mkdir")
    {
        "Applying changes to the workspace.".into()
    } else if lower.starts_with("repo.git_diff") || lower.starts_with("repo.git_status") {
        "Preparing repository comparison.".into()
    } else if lower.starts_with("repo.git_log") || lower.starts_with("repo.git_branches") {
        "Reviewing repository history.".into()
    } else if lower.starts_with("repo.structure") || lower.starts_with("repo.dependencies") {
        "Mapping the project layout.".into()
    } else if lower.starts_with("repo.check") {
        "Checking the project builds.".into()
    } else if lower.starts_with("repo.") {
        "Working with the repository.".into()
    } else if lower.starts_with("terminal.") || lower.starts_with("shell.") {
        "Running a shell command.".into()
    } else if lower.starts_with("diag.") {
        "Collecting diagnostics.".into()
    } else {
        format!("Running {tool}.")
    }
}

/// Compress a possibly long passage to a single leading sentence, so a
/// multi-paragraph provider summary can never flood the widget.
fn first_sentence(text: &str) -> String {
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let end = clean
        .find(". ")
        .map(|i| i + 1)
        .or_else(|| clean.find('.').map(|i| i + 1))
        .unwrap_or(clean.len());
    clean[..end].trim().to_string()
}

/// Truncate to `width` display columns on a word boundary.
fn clamp_width(text: &str, width: usize) -> String {
    if width == 0 || nexus_core::brand::visible_width(text) <= width {
        return text.to_string();
    }
    let budget = width.saturating_sub(1);
    let mut out = String::new();
    for word in text.split_whitespace() {
        let candidate = if out.is_empty() {
            word.to_string()
        } else {
            format!("{out} {word}")
        };
        if nexus_core::brand::visible_width(&candidate) > budget {
            break;
        }
        out = candidate;
    }
    if out.is_empty() {
        out = text.chars().take(budget).collect();
    }
    format!("{out}…")
}

/// Build the preview lines for the live component.
///
/// Sources in priority order: a provider-supplied summary (when the provider
/// actually has a reasoning channel and the operator allows it), then the
/// harness's own structured state, then the objective/stage fallback that
/// predates this module.
pub fn summarize(st: &State, phase: ThinkingState, width: usize) -> Vec<String> {
    let max_lines = st.preview_lines.clamp(1, MAX_PREVIEW_LINES);
    let mut lines: Vec<String> = Vec::new();

    // 1. A real provider summary, compressed to one line.
    if st.summarize_provider_reasoning {
        if let Some(text) = provider_summary(st) {
            let compressed = first_sentence(&text);
            if !compressed.is_empty() {
                lines.push(compressed);
            }
        }
    }

    // 2. Structured harness state.
    let work = &st.active_work;
    if lines.len() < max_lines {
        match phase {
            ThinkingState::Waiting => {
                // The live request outranks the snapshot, which is only
                // refreshed between turns. A turn that asks twice — plan mode
                // approves a plan and then its first write — would otherwise
                // keep naming the first tool while the second is on screen.
                if let Some(request) = st.pending.as_ref() {
                    lines.push(format!(
                        "Waiting for your approval · {}",
                        request.action.tool
                    ));
                } else if let Some(pending) = work.waiting_approvals.first() {
                    lines.push(format!("Waiting for your approval · {pending}"));
                } else {
                    lines.push("Waiting on the provider.".into());
                }
            }
            ThinkingState::Searching | ThinkingState::Executing => {
                if let Some(tool) = work.active_foreground_tool.as_ref() {
                    lines.push(tool_line(tool));
                } else {
                    lines.push(phase.summary().into());
                }
            }
            ThinkingState::Verifying => {
                if let Some(pending) = work.validation_pending.first() {
                    lines.push(format!("Checking {pending}."));
                } else {
                    lines.push(phase.summary().into());
                }
            }
            ThinkingState::Planning => {
                let staged = work.work.as_ref().is_some_and(|plan| plan.stages.len() > 1);
                lines.push(if staged {
                    "Breaking the work into stages.".into()
                } else {
                    phase.summary().to_string()
                });
            }
            ThinkingState::Understanding => {
                if let Some(objective) = work.objective.as_ref().filter(|o| !o.trim().is_empty()) {
                    lines.push(format!("Considering: {}", objective.trim()));
                } else {
                    lines.push(phase.summary().into());
                }
            }
            ThinkingState::Finalizing => lines.push(phase.summary().into()),
        }
    }

    // 3. The structured detail the concise timeline is holding back — running
    //    stage, next action, pending checks. This is exactly what
    //    `State::activity_preview` already assembles, so reuse it rather than
    //    maintaining a second copy of the same rules.
    if lines.len() < max_lines {
        for entry in st.activity_preview() {
            if lines.len() >= max_lines {
                break;
            }
            if !lines.iter().any(|existing| existing == &entry) {
                lines.push(entry);
            }
        }
    }

    lines.truncate(max_lines);
    lines
        .into_iter()
        .map(|line| clamp_width(&nexus_core::sanitize::sanitize_terminal(&line), width))
        .collect()
}

/// The newest provider-supplied reasoning summary for the active turn, if the
/// provider genuinely emitted one.
fn provider_summary(st: &State) -> Option<String> {
    let turn = st.active_turn_id.as_ref();
    st.timeline.iter().rev().take(64).find_map(|event| {
        if turn.is_some_and(|id| &event.turn_id != id) {
            return None;
        }
        match &event.kind {
            TimelineKind::ReasoningSummary { text } if !text.trim().is_empty() => {
                Some(text.clone())
            }
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_phase_title_is_one_of_the_shipped_labels() {
        // The no-regression guard: adding phases must not add new headings.
        let shipped = [
            "WAITING",
            "SEARCHING",
            "EXECUTING",
            "VERIFYING",
            "PLANNING",
            "PROCESSING",
        ];
        for phase in [
            ThinkingState::Understanding,
            ThinkingState::Planning,
            ThinkingState::Searching,
            ThinkingState::Executing,
            ThinkingState::Waiting,
            ThinkingState::Verifying,
            ThinkingState::Finalizing,
        ] {
            assert!(
                shipped.contains(&phase.title()),
                "{phase:?} introduced a new heading `{}`",
                phase.title()
            );
        }
    }

    #[test]
    fn new_phases_collapse_onto_processing() {
        assert_eq!(ThinkingState::Understanding.title(), "PROCESSING");
        assert_eq!(ThinkingState::Finalizing.title(), "PROCESSING");
    }

    #[test]
    fn only_waiting_asks_for_the_operators_attention() {
        // Phase precedence itself is asserted by the transition-table test in
        // `state.rs`; here we only pin which phase is treated as blocked.
        assert!(ThinkingState::Waiting.is_blocked());
        for phase in [
            ThinkingState::Searching,
            ThinkingState::Executing,
            ThinkingState::Verifying,
            ThinkingState::Planning,
            ThinkingState::Finalizing,
            ThinkingState::Understanding,
        ] {
            assert!(!phase.is_blocked(), "{phase:?} must not be blocked");
        }
    }

    #[test]
    fn tool_lines_are_action_oriented_and_specific() {
        let cases = [
            ("web.search", "Searching the web."),
            ("web.fetch", "Reading an external page."),
            ("fs.search_text", "Searching the workspace."),
            ("fs.read_file", "Inspecting workspace files."),
            ("fs.patch_file", "Applying changes to the workspace."),
            ("repo.git_diff", "Preparing repository comparison."),
            ("repo.check", "Checking the project builds."),
            ("terminal.run", "Running a shell command."),
            ("diag.system", "Collecting diagnostics."),
        ];
        for (tool, expected) in cases {
            assert_eq!(tool_line(tool), expected, "for tool {tool}");
        }
    }

    #[test]
    fn an_unknown_tool_still_produces_a_line() {
        assert_eq!(tool_line("mcp.custom"), "Running mcp.custom.");
    }

    #[test]
    fn tool_lines_never_speculate() {
        // Every mapped line is a statement of action, not a guess at intent.
        for tool in ["web.search", "fs.read_file", "terminal.run", "repo.check"] {
            let line = tool_line(tool);
            for banned in ["maybe", "probably", "I think", "should", "wants"] {
                assert!(
                    !line.to_lowercase().contains(&banned.to_lowercase()),
                    "`{line}` speculates"
                );
            }
        }
    }

    #[test]
    fn a_long_passage_compresses_to_one_sentence() {
        let text = "Inspecting the workspace configuration. Then I will consider \
                    whether the loop engine needs a second pass, and after that \
                    probably rerun the tests.";
        let compressed = first_sentence(text);
        assert_eq!(compressed, "Inspecting the workspace configuration.");
    }

    #[test]
    fn a_passage_without_punctuation_still_compresses() {
        assert_eq!(first_sentence("  reading   files  "), "reading files");
    }

    #[test]
    fn clamp_breaks_on_word_boundaries() {
        let clamped = clamp_width("Preparing repository comparison", 14);
        assert!(clamped.ends_with('…'));
        assert!(nexus_core::brand::visible_width(&clamped) <= 14);
        assert!(clamped.starts_with("Preparing"));
    }

    #[test]
    fn clamp_leaves_short_text_alone() {
        assert_eq!(
            clamp_width("Reading the request.", 40),
            "Reading the request."
        );
    }
}
