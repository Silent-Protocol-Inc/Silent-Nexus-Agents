//! The design language, defined once.
//!
//! Every product surface — boot, status line, timeline, CLI reports — renders
//! through a [`Skin`]. Nothing else picks a glyph, a separator, a casing rule,
//! or an animation timing. That is the whole point: reskinning later means
//! writing a second [`Skin`] constructor, not editing every renderer.
//!
//! Three rules the set below is built on:
//!
//! 1. **Icons name an action state, not a tool.** The operator cares that the
//!    agent is applying a change, not that `fs.patch` was the function called.
//!    Tool-family marks still exist for the debug layer; they do not belong on
//!    a product surface.
//! 2. **No emoji.** Emoji are double-width, depend on an installed font, and
//!    render as replacement boxes on several mobile clients this project
//!    supports. Every icon here is single-width with an ASCII fallback.
//! 3. **Motion is cosmetic.** Nothing in the product measures its own progress,
//!    so no animation may imply it. Under reduced motion an animation collapses
//!    to its final frame — it never becomes a different design.

/// What the agent is doing, in the operator's terms.
///
/// This is the closed vocabulary the status line and timeline speak. The
/// translation layer maps runtime facts onto it; renderers map it onto pixels.
/// Nothing else may invent a phrase for an agent state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionState {
    /// Reading the request before committing to an approach.
    TracingIntent,
    /// Deciding how to go about it.
    ShapingApproach,
    /// Looking through the workspace.
    Scanning,
    /// Changing something.
    Applying,
    /// Tests, builds, lints.
    RunningChecks,
    /// Blocked on the operator.
    WaitingOnYou,
    /// Blocked on something outside the loop.
    WaitingOnProvider,
    /// Writing the answer.
    Composing,
    /// Finished, successfully.
    Done,
    /// Finished, unsuccessfully.
    Failed,
    /// Cannot proceed without an approval decision.
    NeedsApproval,
}

impl ActionState {
    /// Every state, for exhaustive iteration in tests and pickers.
    pub const ALL: [ActionState; 11] = [
        Self::TracingIntent,
        Self::ShapingApproach,
        Self::Scanning,
        Self::Applying,
        Self::RunningChecks,
        Self::WaitingOnYou,
        Self::WaitingOnProvider,
        Self::Composing,
        Self::Done,
        Self::Failed,
        Self::NeedsApproval,
    ];

    /// Stable machine name (telemetry, tests, config).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TracingIntent => "tracing_intent",
            Self::ShapingApproach => "shaping_approach",
            Self::Scanning => "scanning",
            Self::Applying => "applying",
            Self::RunningChecks => "running_checks",
            Self::WaitingOnYou => "waiting_on_you",
            Self::WaitingOnProvider => "waiting_on_provider",
            Self::Composing => "composing",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::NeedsApproval => "needs_approval",
        }
    }

    /// The terse present-tense verb shown live. Sentence case, no trailing
    /// punctuation — it is a label, not a sentence.
    pub fn verb(self) -> &'static str {
        match self {
            Self::TracingIntent => "Tracing intent",
            Self::ShapingApproach => "Shaping the approach",
            Self::Scanning => "Scanning the workspace",
            Self::Applying => "Applying changes",
            Self::RunningChecks => "Running checks",
            Self::WaitingOnYou => "Waiting on your approval",
            Self::WaitingOnProvider => "Waiting on the provider",
            Self::Composing => "Composing the answer",
            Self::Done => "Done",
            Self::Failed => "Failed",
            Self::NeedsApproval => "Needs your approval",
        }
    }

    /// True for the states that are waiting on a human. Those are the ones a
    /// renderer colors for attention: nothing moves until the operator acts.
    pub fn is_blocked_on_operator(self) -> bool {
        matches!(self, Self::WaitingOnYou | Self::NeedsApproval)
    }

    /// True once the state is an outcome rather than an activity.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }
}

/// Which marks to draw with. Emoji are deliberately absent — see the module
/// docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IconSet {
    /// Single-width geometric marks. The default wherever the terminal can
    /// draw beyond ASCII.
    #[default]
    Geometric,
    /// Plain ASCII, for `TERM=dumb`, a `C`/`POSIX` locale, or `SNX_ASCII`.
    Ascii,
}

impl IconSet {
    /// A terminal that cannot draw Unicode always wins over any preference.
    pub fn resolve(unicode_supported: bool) -> Self {
        if unicode_supported {
            Self::Geometric
        } else {
            Self::Ascii
        }
    }

    /// The mark for an action state. Single-width in both tiers.
    pub fn icon(self, state: ActionState) -> &'static str {
        match self {
            Self::Geometric => match state {
                ActionState::TracingIntent => "◇",
                ActionState::ShapingApproach => "◈",
                ActionState::Scanning => "⌕",
                ActionState::Applying => "▸",
                ActionState::RunningChecks => "◎",
                ActionState::WaitingOnYou | ActionState::WaitingOnProvider => "◌",
                ActionState::Composing => "◆",
                ActionState::Done => "✓",
                ActionState::Failed => "✕",
                ActionState::NeedsApproval => "△",
            },
            Self::Ascii => match state {
                ActionState::TracingIntent => "?",
                ActionState::ShapingApproach => "*",
                ActionState::Scanning => "/",
                ActionState::Applying => ">",
                ActionState::RunningChecks => "=",
                ActionState::WaitingOnYou | ActionState::WaitingOnProvider => ".",
                ActionState::Composing => "+",
                ActionState::Done => "v",
                ActionState::Failed => "x",
                ActionState::NeedsApproval => "!",
            },
        }
    }
}

/// Lifecycle marks for a record's status, as opposed to an agent's activity.
///
/// Kept as its own vocabulary rather than reusing [`ActionState`]: "this event
/// is pending" and "the agent is tracing intent" are different statements, and
/// collapsing them would make a reskin change one when it meant the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifecycleMark {
    Pending,
    Running,
    Done,
    Failed,
    Blocked,
    Cancelled,
    Skipped,
    Waiting,
}

impl LifecycleMark {
    pub const ALL: [LifecycleMark; 8] = [
        Self::Pending,
        Self::Running,
        Self::Done,
        Self::Failed,
        Self::Blocked,
        Self::Cancelled,
        Self::Skipped,
        Self::Waiting,
    ];
}

impl IconSet {
    /// The mark for a record's lifecycle status.
    pub fn lifecycle(self, mark: LifecycleMark) -> &'static str {
        match self {
            Self::Geometric => match mark {
                LifecycleMark::Pending => "◇",
                LifecycleMark::Running => "◆",
                LifecycleMark::Done => "✓",
                LifecycleMark::Failed => "✕",
                LifecycleMark::Blocked => "■",
                LifecycleMark::Cancelled => "×",
                LifecycleMark::Skipped => "–",
                LifecycleMark::Waiting => "◫",
            },
            Self::Ascii => match mark {
                LifecycleMark::Pending => "?",
                LifecycleMark::Running => ">",
                LifecycleMark::Done => "v",
                LifecycleMark::Failed => "x",
                LifecycleMark::Blocked => "#",
                LifecycleMark::Cancelled => "-",
                LifecycleMark::Skipped => ".",
                LifecycleMark::Waiting => "=",
            },
        }
    }
}

/// Animation timing and the one reduced-motion rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Motion {
    /// Milliseconds per animation frame.
    pub frame_ms: u64,
    /// How long a label holds before it may change, so a fast sequence of
    /// states does not strobe through four words in a second.
    pub dwell_ms: u64,
    /// Collapse every animation to its final frame.
    pub reduced: bool,
}

impl Motion {
    /// Apply the operator's (or terminal's) reduced-motion preference.
    pub fn reduced(mut self, reduced: bool) -> Self {
        self.reduced = self.reduced || reduced;
        self
    }

    /// The frame to draw at `tick`. Under reduced motion this is always the
    /// final frame — the design does not change shape, it stops moving.
    pub fn frame(self, tick: u64, frames: u64) -> u64 {
        if frames == 0 {
            return 0;
        }
        if self.reduced {
            return frames - 1;
        }
        tick % frames
    }

    /// Whether a label that changed `since_ms` ago may be replaced yet.
    pub fn may_change(self, since_ms: u64) -> bool {
        since_ms >= self.dwell_ms
    }
}

/// The characters that join things. One place, so every surface punctuates the
/// same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Separators {
    /// Between fields on one row: `verb · elapsed · effort`.
    pub field: &'static str,
    /// Between a cause and its result.
    pub arrow: &'static str,
    /// Horizontal rule fill.
    pub rule: &'static str,
}

/// Casing rules. Verbs read as language; labels read as chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Casing {
    /// Section labels are upper-case (`TIMELINE`, `INTENT`).
    pub labels_upper: bool,
}

impl Casing {
    pub fn label(self, text: &str) -> String {
        if self.labels_upper {
            text.to_uppercase()
        } else {
            text.to_string()
        }
    }
}

/// How long an elapsed duration is rendered, chosen by available width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElapsedStyle {
    /// `24 seconds` — reads as prose on a wide row.
    Long,
    /// `24s` — for narrow rows.
    Short,
}

impl ElapsedStyle {
    /// Minutes are always compact: "1 minute 4 seconds" is too long for a row
    /// that has to share space with a verb and an effort.
    pub fn format(self, seconds: u64) -> String {
        if seconds >= 60 {
            return format!("{}m{:02}s", seconds / 60, seconds % 60);
        }
        match self {
            Self::Short => format!("{seconds}s"),
            Self::Long if seconds == 1 => "1 second".to_string(),
            Self::Long => format!("{seconds} seconds"),
        }
    }
}

/// The complete design language for one look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Skin {
    pub icons: IconSet,
    pub motion: Motion,
    pub separators: Separators,
    pub casing: Casing,
}

impl Default for Skin {
    fn default() -> Self {
        Self::nexus()
    }
}

impl Skin {
    /// The shipped look: cyberpunk-clean, geometric, subtle.
    pub fn nexus() -> Self {
        Self {
            icons: IconSet::Geometric,
            motion: Motion {
                frame_ms: 120,
                dwell_ms: 500,
                reduced: false,
            },
            separators: Separators {
                field: " · ",
                arrow: " → ",
                rule: "─",
            },
            casing: Casing { labels_upper: true },
        }
    }

    /// Adapt to the terminal: ASCII marks when Unicode is unavailable, and no
    /// motion when the operator or terminal asked for none.
    pub fn for_terminal(mut self, unicode_supported: bool, reduced_motion: bool) -> Self {
        self.icons = IconSet::resolve(unicode_supported);
        self.motion = self.motion.reduced(reduced_motion);
        if !unicode_supported {
            self.separators = Separators {
                field: " | ",
                arrow: " -> ",
                rule: "-",
            };
        }
        self
    }

    pub fn icon(&self, state: ActionState) -> &'static str {
        self.icons.icon(state)
    }

    pub fn lifecycle(&self, mark: LifecycleMark) -> &'static str {
        self.icons.lifecycle(mark)
    }

    /// One status/milestone row: `◇ Tracing intent · 24 seconds · high effort`.
    ///
    /// Absent fields are omitted rather than rendered empty — an unknown is
    /// never printed as a placeholder.
    pub fn row(&self, state: ActionState, fields: &[&str]) -> String {
        let mut row = format!("{} {}", self.icon(state), state.verb());
        for field in fields.iter().filter(|f| !f.trim().is_empty()) {
            row.push_str(self.separators.field);
            row.push_str(field.trim());
        }
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn every_action_state_has_an_icon_in_every_tier() {
        for set in [IconSet::Geometric, IconSet::Ascii] {
            for state in ActionState::ALL {
                let icon = set.icon(state);
                assert!(!icon.is_empty(), "{state:?} has no icon in {set:?}");
            }
        }
    }

    /// Emoji are double-width and font-dependent; the whole point of the set is
    /// that it draws identically on a phone SSH client and a desktop terminal.
    #[test]
    fn no_icon_is_an_emoji_and_every_icon_is_single_width() {
        for set in [IconSet::Geometric, IconSet::Ascii] {
            for state in ActionState::ALL {
                let icon = set.icon(state);
                assert_eq!(
                    UnicodeWidthStr::width(icon),
                    1,
                    "{state:?} icon {icon:?} is not single-width in {set:?}"
                );
                assert!(
                    !icon.chars().any(|c| c as u32 >= 0x1F000),
                    "{state:?} icon {icon:?} is an emoji"
                );
            }
        }
    }

    #[test]
    fn every_lifecycle_mark_has_a_single_width_icon_in_every_tier() {
        for set in [IconSet::Geometric, IconSet::Ascii] {
            for mark in LifecycleMark::ALL {
                let icon = set.lifecycle(mark);
                assert_eq!(
                    UnicodeWidthStr::width(icon),
                    1,
                    "{mark:?} in {set:?} is not single-width"
                );
                assert!(
                    !icon.chars().any(|c| c as u32 >= 0x1F000),
                    "{mark:?} is an emoji"
                );
            }
        }
    }

    #[test]
    fn ascii_icons_are_pure_ascii() {
        for state in ActionState::ALL {
            assert!(IconSet::Ascii.icon(state).is_ascii(), "{state:?}");
        }
    }

    #[test]
    fn a_terminal_without_unicode_overrides_the_default_set() {
        let skin = Skin::nexus().for_terminal(false, false);
        assert_eq!(skin.icons, IconSet::Ascii);
        assert_eq!(skin.separators.field, " | ");
    }

    #[test]
    fn reduced_motion_collapses_to_the_final_frame() {
        let moving = Motion {
            frame_ms: 120,
            dwell_ms: 500,
            reduced: false,
        };
        assert_eq!(moving.frame(0, 4), 0);
        assert_eq!(moving.frame(5, 4), 1);

        // Same design, stopped — not a different design.
        let still = moving.reduced(true);
        for tick in 0..10 {
            assert_eq!(still.frame(tick, 4), 3, "reduced motion must not animate");
        }
    }

    #[test]
    fn a_zero_frame_animation_never_divides_by_zero() {
        assert_eq!(Skin::nexus().motion.frame(7, 0), 0);
    }

    #[test]
    fn the_dwell_window_holds_a_label_before_it_may_change() {
        let motion = Skin::nexus().motion;
        assert!(!motion.may_change(120));
        assert!(motion.may_change(500));
    }

    #[test]
    fn elapsed_reads_as_prose_when_wide_and_compact_when_narrow() {
        assert_eq!(ElapsedStyle::Long.format(24), "24 seconds");
        assert_eq!(ElapsedStyle::Long.format(1), "1 second");
        assert_eq!(ElapsedStyle::Short.format(24), "24s");
        // Minutes stay compact in both styles.
        assert_eq!(ElapsedStyle::Long.format(64), "1m04s");
        assert_eq!(ElapsedStyle::Short.format(64), "1m04s");
    }

    #[test]
    fn a_row_omits_absent_fields_rather_than_padding_them() {
        let skin = Skin::nexus();
        assert_eq!(
            skin.row(ActionState::TracingIntent, &["24 seconds", "high effort"]),
            "◇ Tracing intent · 24 seconds · high effort"
        );
        // No effort reported: the field is gone, not blank.
        assert_eq!(
            skin.row(ActionState::Scanning, &["3 seconds", ""]),
            "⌕ Scanning the workspace · 3 seconds"
        );
        assert_eq!(skin.row(ActionState::Applying, &[]), "▸ Applying changes");
    }

    #[test]
    fn waiting_states_distinguish_who_is_blocking() {
        assert!(ActionState::WaitingOnYou.is_blocked_on_operator());
        assert!(ActionState::NeedsApproval.is_blocked_on_operator());
        assert!(!ActionState::WaitingOnProvider.is_blocked_on_operator());
        assert_ne!(
            ActionState::WaitingOnYou.verb(),
            ActionState::WaitingOnProvider.verb()
        );
    }

    #[test]
    fn verbs_are_labels_not_sentences() {
        for state in ActionState::ALL {
            let verb = state.verb();
            assert!(!verb.ends_with('.'), "{state:?} verb ends with a period");
            assert!(!verb.is_empty());
            assert!(
                verb.chars().next().is_some_and(|c| c.is_uppercase()),
                "{state:?} verb is not sentence case"
            );
        }
    }

    #[test]
    fn state_names_are_unique_and_stable() {
        for state in ActionState::ALL {
            assert_eq!(
                ActionState::ALL
                    .iter()
                    .filter(|other| other.as_str() == state.as_str())
                    .count(),
                1,
                "duplicate machine name for {state:?}"
            );
        }
    }
}
