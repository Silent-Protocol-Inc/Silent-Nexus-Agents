//! The narration layer: what the agent says about its own work.
//!
//! Three parts, in dependency order:
//!
//! * [`translate`] — the single door from runtime facts to operator language.
//!   Nothing above it can see a tool name, an argument blob, or raw output.
//! * [`NarrationPolicy`] — whether this turn narrates at all, and which
//!   statements survive the current mode.
//! * [`intent`] — the deterministic 2–5 step plan shown before work starts,
//!   which a model may reword but not redirect.
//!
//! The policy exists because "say something useful" and "say everything" are
//! different products. A greeting gets silence; a two-file refactor gets an
//! intent and a handful of milestones; `verbose` gets every observed action.
//! What none of them get is a line for something that has not happened.

pub mod intent;
pub mod translate;

pub use intent::{accept_rewording, skeleton, IntentPlan, IntentStep, StepKind};
pub use translate::{present, Presented, RuntimeFact, Significance};

use nexus_core::orchestration::WorkEstimate;
use nexus_core::timeline::NarrationMode;
use nexus_models::types::TaskClass;

/// Decides what this turn is allowed to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NarrationPolicy {
    mode: NarrationMode,
    /// False for turns that are not tasks — a greeting, a one-line question.
    task_shaped: bool,
}

impl NarrationPolicy {
    /// Build the policy for one turn.
    ///
    /// Two independent reasons to stay quiet, and either is enough:
    /// the operator turned narration off, or the turn is not task-shaped.
    /// "Task-shaped" is deliberately conservative — it uses the same
    /// deterministic classification and work estimate the plan generator uses,
    /// so narration and the work breakdown can never disagree about whether
    /// there is work to describe.
    pub fn for_turn(mode: NarrationMode, class: TaskClass, estimate: &WorkEstimate) -> Self {
        Self {
            mode,
            task_shaped: task_shaped(class, estimate),
        }
    }

    /// A policy that says nothing. The state a loop is in before a turn has
    /// been classified: silence is the safe default, because a narration line
    /// emitted before the turn is understood would describe nothing real.
    pub fn silent(mode: NarrationMode) -> Self {
        Self {
            mode,
            task_shaped: false,
        }
    }

    pub fn mode(self) -> NarrationMode {
        self.mode
    }

    /// Whether this turn narrates at all.
    pub fn narrates(self) -> bool {
        self.mode.narrates() && self.task_shaped
    }

    /// Whether to emit the intent plan. Same condition as narrating: an intent
    /// for a greeting is noise, and an intent nobody will see is a wasted
    /// refinement pass.
    pub fn emits_intent(self) -> bool {
        self.narrates()
    }

    /// Whether the one bounded wording-refinement pass may run this turn.
    pub fn refines_wording(self) -> bool {
        self.narrates() && self.mode.refines_wording()
    }

    /// The lowest significance this mode surfaces.
    pub fn floor(self) -> Significance {
        match self.mode {
            // Nothing is shown; `narrates()` already gates this, and the floor
            // stays at the top so a caller that skips that check still cannot
            // leak a routine line.
            NarrationMode::Off => Significance::Critical,
            NarrationMode::Compact => Significance::Critical,
            NarrationMode::Auto => Significance::Notable,
            NarrationMode::Verbose => Significance::Routine,
        }
    }

    /// Whether a translated statement is surfaced in this mode.
    pub fn shows(self, presented: &Presented) -> bool {
        self.narrates() && presented.significance >= self.floor()
    }
}

/// Whether a turn has enough real work to be worth describing.
///
/// `Simple` covers greetings and short questions. Beyond that, a turn that
/// predicts at most one action and writes nothing is a lookup, not a task —
/// announcing a plan for it would be theatre.
fn task_shaped(class: TaskClass, estimate: &WorkEstimate) -> bool {
    if class == TaskClass::Simple {
        return false;
    }
    estimate.writes || estimate.predicted_actions > 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn estimate(predicted_actions: u32, writes: bool) -> WorkEstimate {
        WorkEstimate {
            predicted_actions,
            writes,
            ..Default::default()
        }
    }

    fn policy(mode: NarrationMode, class: TaskClass) -> NarrationPolicy {
        NarrationPolicy::for_turn(mode, class, &estimate(4, true))
    }

    fn presented(significance: Significance) -> Presented {
        let fact = RuntimeFact::ToolCompleted {
            name: "fs.read".into(),
            arguments: json!({"path": "a.rs"}),
            ok: significance != Significance::Critical,
            output: String::new(),
        };
        let mut p = present(&fact);
        p.significance = significance;
        p
    }

    #[test]
    fn a_greeting_never_narrates_in_any_mode() {
        for mode in [
            NarrationMode::Off,
            NarrationMode::Compact,
            NarrationMode::Auto,
            NarrationMode::Verbose,
        ] {
            let policy = NarrationPolicy::for_turn(mode, TaskClass::Simple, &estimate(1, false));
            assert!(!policy.narrates(), "{mode:?} narrated a greeting");
            assert!(!policy.emits_intent());
        }
    }

    #[test]
    fn a_lookup_with_one_action_and_no_writes_is_not_task_shaped() {
        let policy =
            NarrationPolicy::for_turn(NarrationMode::Auto, TaskClass::Coding, &estimate(1, false));
        assert!(!policy.narrates());
    }

    #[test]
    fn a_write_is_task_shaped_even_as_a_single_action() {
        let policy =
            NarrationPolicy::for_turn(NarrationMode::Auto, TaskClass::Coding, &estimate(1, true));
        assert!(policy.narrates());
    }

    #[test]
    fn off_narrates_nothing_even_for_a_real_task() {
        let policy = policy(NarrationMode::Off, TaskClass::Coding);
        assert!(!policy.narrates());
        assert!(!policy.shows(&presented(Significance::Critical)));
    }

    #[test]
    fn each_mode_surfaces_exactly_its_floor_and_above() {
        let cases = [
            (NarrationMode::Compact, [false, false, true]),
            (NarrationMode::Auto, [false, true, true]),
            (NarrationMode::Verbose, [true, true, true]),
        ];
        for (mode, expected) in cases {
            let policy = policy(mode, TaskClass::Coding);
            let levels = [
                Significance::Routine,
                Significance::Notable,
                Significance::Critical,
            ];
            for (level, want) in levels.into_iter().zip(expected) {
                assert_eq!(
                    policy.shows(&presented(level)),
                    want,
                    "{mode:?} / {level:?}"
                );
            }
        }
    }

    #[test]
    fn a_failure_is_never_silenced_by_a_quieter_mode() {
        for mode in [
            NarrationMode::Compact,
            NarrationMode::Auto,
            NarrationMode::Verbose,
        ] {
            assert!(policy(mode, TaskClass::Coding).shows(&presented(Significance::Critical)));
        }
    }

    #[test]
    fn only_auto_and_verbose_spend_a_refinement_pass() {
        assert!(!policy(NarrationMode::Off, TaskClass::Coding).refines_wording());
        assert!(!policy(NarrationMode::Compact, TaskClass::Coding).refines_wording());
        assert!(policy(NarrationMode::Auto, TaskClass::Coding).refines_wording());
        assert!(policy(NarrationMode::Verbose, TaskClass::Coding).refines_wording());
        // A non-task turn never spends one either, whatever the mode.
        assert!(!NarrationPolicy::for_turn(
            NarrationMode::Verbose,
            TaskClass::Simple,
            &estimate(1, false)
        )
        .refines_wording());
    }
    /// **The layer boundary, against the real registry.**
    ///
    /// Boot, the status line, and the timeline can only render a `Presented`,
    /// and a `Presented` has no field for a tool name. This checks the other
    /// half: that no translation *writes* one into the text either, for every
    /// tool this build actually ships.
    #[test]
    fn no_real_registry_tool_can_reach_a_product_surface() {
        let registry = nexus_tools::ToolRegistry::with_builtins();
        let names = registry.names();
        assert!(names.len() > 5, "registry looks empty: {names:?}");

        for name in &names {
            for ok in [true, false] {
                let fact = RuntimeFact::ToolCompleted {
                    name: name.clone(),
                    arguments: json!({"path": "src/lib.rs", "command": "cargo test"}),
                    ok,
                    output: "some output".into(),
                };
                let line = present(&fact).line();
                assert!(!line.contains(name.as_str()), "`{name}` reached: {line}");
                // Identifier-shaped fragments are the tell; a bare English word
                // that happens to match a segment is the sentence working.
                for fragment in name.split('.').filter(|f| f.contains('_')) {
                    assert!(!line.contains(fragment), "`{fragment}` reached: {line}");
                }
            }
        }
    }

    /// The intent plan is written before any tool is chosen, so it cannot name
    /// one — but it is also model-reworded, and this pins that the rewording
    /// gate does not let one in.
    #[test]
    fn an_intent_plan_never_names_a_tool() {
        let registry = nexus_tools::ToolRegistry::with_builtins();
        let plan = skeleton(
            TaskClass::Coding,
            &WorkEstimate {
                writes: true,
                needs_grounding: true,
                predicted_actions: 4,
                ..Default::default()
            },
            5,
        )
        .expect("plan");
        for step in plan.texts() {
            for name in registry.names() {
                assert!(!step.contains(&name), "`{name}` in intent step: {step}");
            }
        }

        // And a rewording that tries to smuggle one in is refused, so the
        // model cannot put a function name on a product surface by describing
        // a legitimate step in the machine's vocabulary.
        let mut reworded = plan.texts();
        reworded[0] = "Read via fs.read_file".into();
        assert_eq!(accept_rewording(&plan, &reworded), plan);
    }
}
