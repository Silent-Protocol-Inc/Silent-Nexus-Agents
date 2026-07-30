//! The intent plan: 2–5 steps stated before the work starts.
//!
//! The skeleton is **deterministic and the source of truth**. It is built from
//! the same task class and work estimate the work breakdown uses, so the two
//! can never disagree about the shape of the turn, and it needs no model — a
//! constrained local model gets the same plan as a frontier one.
//!
//! A model may then improve the *wording*, and only the wording.
//! [`accept_rewording`] is the gate: same number of steps, same order, and each
//! step still opening with a verb compatible with what that step actually is.
//! Anything else and the skeleton is kept, unchanged, with `refined: false`
//! recorded rather than glossed over. So a refinement can turn
//! "Read the relevant files" into "Read the failing test and its module" — a
//! better sentence about the same act — but it cannot turn it into
//! "Delete the module", add a sixth step, or reorder the work.
//!
//! The plan is an **intention**. Nothing here marks a step done; only an
//! observed action can do that, and that lives in [`super::translate`].

use nexus_core::orchestration::WorkEstimate;
use nexus_models::types::TaskClass;

/// What a step is for. Fixed by the skeleton and never changed by refinement —
/// it is what makes "only the wording" checkable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// Look at the workspace before deciding anything.
    Ground,
    /// Gather outside material.
    Research,
    /// Decide the approach.
    Plan,
    /// Change something.
    Implement,
    /// Prove it works.
    Verify,
    /// Tell the operator what happened.
    Report,
}

impl StepKind {
    /// Opening verbs that are compatible with this kind of step.
    ///
    /// Deliberately generous — the point is to catch a refinement that changed
    /// the *act*, not to police style. A step that opens with a verb outside
    /// its kind is no longer a rewording of that step.
    fn verbs(self) -> &'static [&'static str] {
        match self {
            Self::Ground => &[
                "read",
                "inspect",
                "review",
                "examine",
                "open",
                "locate",
                "find",
                "trace",
                "scan",
                "map",
                "survey",
                "look",
                "check",
                "identify",
                "understand",
            ],
            Self::Research => &[
                "search", "gather", "collect", "find", "research", "consult", "read", "look",
                "compare",
            ],
            Self::Plan => &[
                "plan", "draft", "outline", "decide", "choose", "design", "sketch", "shape",
                "propose",
            ],
            Self::Implement => &[
                "implement",
                "apply",
                "add",
                "write",
                "update",
                "change",
                "edit",
                "fix",
                "refactor",
                "remove",
                "delete",
                "rename",
                "create",
                "patch",
                "wire",
                "introduce",
                "make",
                "adjust",
                "extend",
                "move",
            ],
            Self::Verify => &[
                "run", "verify", "check", "test", "validate", "confirm", "rerun", "prove",
                "measure",
            ],
            Self::Report => &[
                "report",
                "summarize",
                "summarise",
                "explain",
                "describe",
                "present",
                "share",
                "hand",
            ],
        }
    }

    /// Whether `text` opens with a verb this kind of step can carry.
    fn allows(self, text: &str) -> bool {
        let first = text
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|c: char| !c.is_ascii_alphabetic())
            .to_ascii_lowercase();
        self.verbs().contains(&first.as_str())
    }
}

/// One step of the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentStep {
    pub kind: StepKind,
    pub text: String,
}

/// The plan shown before the work starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentPlan {
    pub steps: Vec<IntentStep>,
    /// True only when a model rewording was accepted. Recorded rather than
    /// implied, so a degraded turn is visible instead of looking authored.
    pub refined: bool,
}

impl IntentPlan {
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// The step texts, for rendering and for the timeline event payload.
    pub fn texts(&self) -> Vec<String> {
        self.steps.iter().map(|step| step.text.clone()).collect()
    }
}

/// Longest a step may be. Past this it stops being a glance.
const MAX_STEP_CHARS: usize = 80;

/// Build the deterministic plan for a turn, or `None` when the turn does not
/// warrant one.
///
/// `max_steps` is clamped to 2..=5 by the caller's config; a plan of one step
/// is not a plan and is collapsed to `None`.
pub fn skeleton(class: TaskClass, estimate: &WorkEstimate, max_steps: usize) -> Option<IntentPlan> {
    let max_steps = max_steps.clamp(2, 5);
    let mut steps: Vec<IntentStep> = Vec::new();

    if estimate.needs_grounding || matches!(class, TaskClass::Coding | TaskClass::Verification) {
        steps.push(step(StepKind::Ground, "Read the relevant files"));
    }

    match class {
        TaskClass::Research => {
            steps.push(step(StepKind::Research, "Gather the source material"));
            steps.push(step(StepKind::Report, "Report the findings with sources"));
        }
        TaskClass::Planning => {
            steps.push(step(StepKind::Plan, "Draft the approach"));
            steps.push(step(StepKind::Report, "Present the plan for review"));
        }
        TaskClass::Verification => {
            steps.push(step(StepKind::Verify, "Run the checks"));
            steps.push(step(StepKind::Report, "Report what the evidence shows"));
        }
        TaskClass::Coding | TaskClass::Simple => {
            if estimate.writes {
                steps.push(step(StepKind::Implement, "Apply the change"));
                steps.push(step(StepKind::Verify, "Verify it still builds and passes"));
            } else {
                steps.push(step(StepKind::Ground, "Trace the behaviour in question"));
            }
            steps.push(step(StepKind::Report, "Report what changed"));
        }
    }

    steps.truncate(max_steps);
    if steps.len() < 2 {
        return None;
    }
    Some(IntentPlan {
        steps,
        refined: false,
    })
}

fn step(kind: StepKind, text: &str) -> IntentStep {
    IntentStep {
        kind,
        text: text.to_string(),
    }
}

/// Apply a model rewording, or keep the skeleton.
///
/// All-or-nothing on purpose: a half-accepted refinement produces a plan that
/// is neither the deterministic one nor the model's, and nobody could say which
/// steps to trust. Rejection is silent to the operator and recorded in
/// `refined`.
pub fn accept_rewording(skeleton: &IntentPlan, reworded: &[String]) -> IntentPlan {
    if reworded.len() != skeleton.steps.len() {
        return skeleton.clone();
    }
    let mut steps = Vec::with_capacity(skeleton.steps.len());
    for (original, candidate) in skeleton.steps.iter().zip(reworded) {
        let text = candidate.trim();
        let acceptable = !text.is_empty()
            && !text.contains('\n')
            && text.chars().count() <= MAX_STEP_CHARS
            && original.kind.allows(text);
        if !acceptable {
            return skeleton.clone();
        }
        steps.push(IntentStep {
            kind: original.kind,
            text: text.to_string(),
        });
    }
    IntentPlan {
        steps,
        refined: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn estimate(writes: bool, needs_grounding: bool) -> WorkEstimate {
        WorkEstimate {
            writes,
            needs_grounding,
            predicted_actions: 4,
            ..Default::default()
        }
    }

    #[test]
    fn a_plan_is_always_between_two_and_five_steps() {
        for class in [
            TaskClass::Coding,
            TaskClass::Planning,
            TaskClass::Research,
            TaskClass::Verification,
        ] {
            for writes in [true, false] {
                for grounding in [true, false] {
                    let plan = skeleton(class, &estimate(writes, grounding), 5)
                        .unwrap_or_else(|| panic!("{class:?} writes={writes} produced no plan"));
                    assert!(
                        (2..=5).contains(&plan.len()),
                        "{class:?} produced {} steps",
                        plan.len()
                    );
                    assert!(!plan.refined);
                }
            }
        }
    }

    #[test]
    fn max_steps_is_respected_and_clamped() {
        let plan = skeleton(TaskClass::Coding, &estimate(true, true), 3).expect("plan");
        assert_eq!(plan.len(), 3);
        // Below the floor, the clamp keeps a plan meaningful rather than
        // producing a one-line "plan".
        let clamped = skeleton(TaskClass::Coding, &estimate(true, true), 0).expect("plan");
        assert_eq!(clamped.len(), 2);
    }

    #[test]
    fn a_writing_turn_plans_to_verify_its_own_change() {
        let plan = skeleton(TaskClass::Coding, &estimate(true, true), 5).expect("plan");
        assert!(plan.steps.iter().any(|s| s.kind == StepKind::Verify));
        assert!(plan.steps.iter().any(|s| s.kind == StepKind::Implement));
    }

    #[test]
    fn a_read_only_turn_does_not_claim_it_will_change_anything() {
        let plan = skeleton(TaskClass::Coding, &estimate(false, true), 5).expect("plan");
        assert!(!plan.steps.iter().any(|s| s.kind == StepKind::Implement));
    }

    #[test]
    fn a_valid_rewording_is_accepted_and_marked() {
        let plan = skeleton(TaskClass::Coding, &estimate(true, true), 5).expect("plan");
        let reworded: Vec<String> = vec![
            "Read the failing test and its module".into(),
            "Apply the fix to the tier check".into(),
            "Run the nexus-warp suite".into(),
            "Report the result".into(),
        ];
        assert_eq!(reworded.len(), plan.len());
        let refined = accept_rewording(&plan, &reworded);
        assert!(refined.refined);
        assert_eq!(refined.texts(), reworded);
        // Kinds are the skeleton's, always.
        assert_eq!(
            refined.steps.iter().map(|s| s.kind).collect::<Vec<_>>(),
            plan.steps.iter().map(|s| s.kind).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_rewording_that_changes_the_step_count_is_rejected() {
        let plan = skeleton(TaskClass::Coding, &estimate(true, true), 5).expect("plan");
        let too_many: Vec<String> = (0..plan.len() + 1)
            .map(|i| format!("Read step {i}"))
            .collect();
        assert_eq!(accept_rewording(&plan, &too_many), plan);
        let too_few: Vec<String> = vec!["Read one thing".into()];
        assert_eq!(accept_rewording(&plan, &too_few), plan);
    }

    #[test]
    fn a_rewording_that_changes_what_a_step_does_is_rejected() {
        let plan = skeleton(TaskClass::Coding, &estimate(true, true), 5).expect("plan");
        let mut reworded = plan.texts();
        // Step 1 is Ground — reading, not deleting.
        reworded[0] = "Delete the module".into();
        let result = accept_rewording(&plan, &reworded);
        assert_eq!(result, plan, "a changed act must not pass as a rewording");
        assert!(!result.refined);
    }

    #[test]
    fn reordering_is_rejected_because_kinds_are_positional() {
        let plan = skeleton(TaskClass::Coding, &estimate(true, true), 5).expect("plan");
        let mut reworded = plan.texts();
        reworded.reverse();
        assert_eq!(accept_rewording(&plan, &reworded), plan);
    }

    #[test]
    fn empty_multiline_and_overlong_steps_are_rejected() {
        let plan = skeleton(TaskClass::Coding, &estimate(true, true), 5).expect("plan");
        for bad in [
            String::new(),
            "Read the file\nand also this".to_string(),
            format!("Read {}", "x".repeat(MAX_STEP_CHARS)),
        ] {
            let mut reworded = plan.texts();
            reworded[0] = bad.clone();
            assert_eq!(
                accept_rewording(&plan, &reworded),
                plan,
                "accepted a bad step: {bad:?}"
            );
        }
    }

    #[test]
    fn rejection_is_recorded_rather_than_implied() {
        let plan = skeleton(TaskClass::Coding, &estimate(true, true), 5).expect("plan");
        let mut reworded = plan.texts();
        reworded[0] = "Obliterate everything".into();
        assert!(!accept_rewording(&plan, &reworded).refined);
    }

    #[test]
    fn step_kinds_accept_their_own_verbs_and_refuse_foreign_ones() {
        assert!(StepKind::Ground.allows("Read the module"));
        assert!(StepKind::Ground.allows("  inspect the config"));
        assert!(!StepKind::Ground.allows("Delete the module"));
        assert!(StepKind::Verify.allows("Run the suite"));
        assert!(!StepKind::Verify.allows("Rewrite the suite"));
        assert!(StepKind::Report.allows("Summarise the outcome"));
    }
}
