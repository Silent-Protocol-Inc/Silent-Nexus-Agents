//! What a turn's prompt will contain, decided in one place.
//!
//! The loop builds the prompt and the inspector describes it. When those were
//! two separate pieces of reasoning, the inspector reported a healthy
//! composition for a prompt that was not the one being sent — it rebuilt a
//! summary from the same *inputs* and hardcoded its conclusions, so it could
//! not have detected a delivery problem even in principle.
//!
//! [`PromptShape::decide`] is the single answer to "what does this turn
//! include". The loop gates its sections on it; the inspector reports it. There
//! is no second copy to drift.

use nexus_core::config::PersonaConfig;
use nexus_core::orchestration::{WorkBreakdown, WorkBreakdownKind};
use nexus_models::types::TaskClass;

/// The sections a turn will carry, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptShape {
    /// A conversation rather than a piece of work: no plan, no contract, no
    /// charter, no tool inventory.
    pub conversational: bool,
    pub includes_agent_contract: bool,
    pub includes_charter: bool,
    pub includes_plan: bool,
    pub includes_tool_inventory: bool,
    /// The persona section is prefixed with the sentence naming it as the
    /// identity to answer as.
    pub adoption_directive: bool,
}

impl PromptShape {
    /// Decide the shape of a turn.
    ///
    /// Conservative by construction. The classifier already runs every turn and
    /// falls back to [`TaskClass::Coding`] for anything ambiguous, so
    /// [`TaskClass::Simple`] means "no signal that this touches the workspace"
    /// rather than "probably fine". Every other condition is a hard veto: an
    /// active goal, a pending task, or a breakdown that is `Tracked` or
    /// `Planned` all mean work is in flight, and work gets the full prompt
    /// however conversational the sentence reads.
    ///
    /// The check is on `kind`, not on `approved`. `WorkBreakdown::generate`
    /// sets `approved: kind != Planned`, so every `Direct` breakdown is already
    /// "approved" — there is nothing to approve. Reading that field as operator
    /// consent would veto every conversational turn there is.
    pub fn decide(
        objective: &str,
        has_goal: bool,
        has_pending_tasks: bool,
        work: &WorkBreakdown,
        config: &PersonaConfig,
    ) -> Self {
        let conversational = config.conversational_turns
            && crate::classify::classify(objective) == TaskClass::Simple
            && !has_goal
            && !has_pending_tasks
            && work.kind == WorkBreakdownKind::Direct
            && work.stages.len() <= 1;
        Self {
            conversational,
            includes_agent_contract: !conversational,
            includes_charter: !conversational,
            includes_plan: !conversational,
            includes_tool_inventory: !conversational,
            adoption_directive: config.adoption_directive,
        }
    }

    /// A turn that carries everything. Used where no narrowing may apply.
    pub fn full(config: &PersonaConfig) -> Self {
        Self {
            conversational: false,
            includes_agent_contract: true,
            includes_charter: true,
            includes_plan: true,
            includes_tool_inventory: true,
            adoption_directive: config.adoption_directive,
        }
    }

    /// One line for the inspector and the timeline.
    pub fn describe(&self) -> &'static str {
        if self.conversational {
            "conversational — answered directly; no plan, contract, charter, or tool inventory attached"
        } else {
            "full — plan, operational contract, charter, and tool inventory attached"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::orchestration::{WorkBreakdown, WorkEstimate};

    fn direct(objective: &str) -> WorkBreakdown {
        WorkBreakdown::generate(objective, WorkEstimate::default())
    }

    fn config() -> PersonaConfig {
        PersonaConfig::default()
    }

    #[test]
    fn a_greeting_is_conversational() {
        let work = direct("hello");
        let shape = PromptShape::decide("hello", false, false, &work, &config());
        assert!(shape.conversational);
        assert!(!shape.includes_plan);
        assert!(!shape.includes_tool_inventory);
    }

    #[test]
    fn asking_who_you_are_is_conversational() {
        let work = direct("who are you?");
        assert!(PromptShape::decide("who are you?", false, false, &work, &config()).conversational);
    }

    /// The condition that actually matters: anything that smells like work gets
    /// the full prompt, because a narrowed prompt for real work would strand
    /// the model without the tools it needs.
    #[test]
    fn work_is_never_conversational() {
        let objective = "refactor the auth module and run the tests";
        let work = direct(objective);
        assert!(!PromptShape::decide(objective, false, false, &work, &config()).conversational);
    }

    #[test]
    fn an_active_goal_vetoes_narrowing() {
        let work = direct("hello");
        assert!(!PromptShape::decide("hello", true, false, &work, &config()).conversational);
    }

    #[test]
    fn a_pending_task_vetoes_narrowing() {
        let work = direct("hello");
        assert!(!PromptShape::decide("hello", false, true, &work, &config()).conversational);
    }

    /// A tracked or planned breakdown is work in flight, whatever the sentence
    /// looks like. (`approved` is deliberately not the signal: `generate` sets
    /// it true for every non-`Planned` breakdown, so a `Direct` turn is always
    /// "approved" and reading it as consent would disable narrowing entirely.)
    #[test]
    fn a_tracked_or_planned_breakdown_vetoes_narrowing() {
        for kind in [WorkBreakdownKind::Tracked, WorkBreakdownKind::Planned] {
            let mut work = direct("hello");
            work.kind = kind;
            assert!(
                !PromptShape::decide("hello", false, false, &work, &config()).conversational,
                "{kind:?} was treated as conversational"
            );
        }
    }

    #[test]
    fn a_direct_breakdown_is_approved_by_construction_and_still_narrows() {
        let work = direct("hello");
        assert!(
            work.approved,
            "generate() marks Direct approved; the predicate must not read this as consent"
        );
        assert!(PromptShape::decide("hello", false, false, &work, &config()).conversational);
    }

    #[test]
    fn the_switch_turns_it_off() {
        let work = direct("hello");
        let config = PersonaConfig {
            conversational_turns: false,
            ..PersonaConfig::default()
        };
        assert!(!PromptShape::decide("hello", false, false, &work, &config).conversational);
    }

    /// A conversational turn still carries the persona — narrowing removes the
    /// task machine, never the identity.
    #[test]
    fn narrowing_never_touches_the_adoption_directive() {
        let work = direct("hello");
        let shape = PromptShape::decide("hello", false, false, &work, &config());
        assert!(shape.conversational);
        assert!(shape.adoption_directive);
    }
}
