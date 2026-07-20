//! Deliberation policy: when AUTO shows execution insight, and how the mode
//! modulates optional deliberation.
//!
//! The decision is **deterministic**. It is a pure function of the objective's
//! deterministic task class (see [`crate::classify`]) and structural facts from
//! the harness's own work estimate. Nothing here samples randomness, consults
//! the model, or issues a provider request, so the same prompt always produces
//! the same decision.

use nexus_core::thinking::ThinkingMode;
use nexus_models::types::TaskClass;

use crate::loop_engine::TurnLimits;

/// Deterministic inputs to the AUTO decision, all derived from structured
/// state — never from model prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoSignals {
    pub class: TaskClass,
    pub word_count: usize,
    pub predicted_actions: u32,
    pub writes: bool,
    pub multi_file: bool,
    pub external: bool,
    pub needs_grounding: bool,
}

impl Default for AutoSignals {
    fn default() -> Self {
        Self {
            class: TaskClass::Simple,
            word_count: 0,
            predicted_actions: 0,
            writes: false,
            multi_file: false,
            external: false,
            needs_grounding: false,
        }
    }
}

impl AutoSignals {
    /// Build the signals for an objective using the same deterministic
    /// classifier and work estimate the loop itself uses, so the preview shown
    /// in `/thinking` matches what the turn will actually do.
    pub fn for_objective(objective: &str) -> Self {
        let estimate = nexus_core::orchestration::WorkEstimate::from_objective(objective);
        Self {
            class: crate::classify::classify(objective),
            word_count: objective.split_whitespace().count(),
            predicted_actions: estimate.predicted_actions,
            writes: estimate.writes,
            multi_file: estimate.multi_file,
            external: estimate.external,
            needs_grounding: estimate.needs_grounding,
        }
    }
}

/// Whether AUTO shows the live activity component for this turn.
///
/// Provider reasoning capability is deliberately *not* an input: it selects the
/// component's heading, not whether it appears. Feeding it in would make the
/// same prompt behave differently across providers, which is exactly the
/// non-determinism this function exists to avoid.
pub fn auto_shows_thinking(signals: &AutoSignals) -> bool {
    match signals.class {
        // Classes whose work is inherently multi-step.
        TaskClass::Coding | TaskClass::Planning | TaskClass::Research | TaskClass::Verification => {
            // Escape hatch: a very short, read-only, single-action ask that
            // merely tripped a keyword ("what file is this?") is not real work.
            let trivial_lookup = signals.class == TaskClass::Coding
                && signals.word_count <= 6
                && !signals.writes
                && signals.predicted_actions <= 1
                && !signals.needs_grounding;
            !trivial_lookup
        }
        // Greetings and one-shot factual answers. Promote only on hard
        // structural evidence, never on phrasing.
        TaskClass::Simple => {
            signals.writes
                || signals.multi_file
                || signals.external
                || signals.predicted_actions >= 2
                || signals.needs_grounding
        }
    }
}

/// A short, stable reason for the resolved decision. Surfaced in the activity
/// detail panel so the choice is inspectable, never in the timeline.
pub fn auto_reason(signals: &AutoSignals, shown: bool) -> &'static str {
    match (signals.class, shown) {
        (TaskClass::Coding, false) => "coding: trivial lookup",
        (TaskClass::Coding, true) => "class=coding",
        (TaskClass::Planning, _) => "class=planning",
        (TaskClass::Research, _) => "class=research",
        (TaskClass::Verification, _) => "class=verification",
        (TaskClass::Simple, true) => "simple: structural signals",
        (TaskClass::Simple, false) => "class=simple",
    }
}

/// Resolve whether the component shows, for any mode.
pub fn resolve_visibility(mode: ThinkingMode, signals: &AutoSignals) -> (bool, &'static str) {
    match mode {
        ThinkingMode::Off => (false, "mode=off"),
        ThinkingMode::On => (true, "mode=on"),
        ThinkingMode::Auto => {
            let shown = auto_shows_thinking(signals);
            (shown, auto_reason(signals, shown))
        }
    }
}

/// Modulate *optional* deliberation tolerance.
///
/// Deliberately asymmetric: `Off` may lower retry tolerance, but `On` only
/// declines to lower it — it never raises a ceiling above what the operator
/// configured, because a UI toggle must not be able to escalate a safety
/// limit. Step, tool-call, model-call, failure, token, cost, and duration
/// budgets are never touched by any mode.
pub fn modulate_limits(base: &TurnLimits, mode: ThinkingMode) -> TurnLimits {
    let mut limits = base.clone();
    match mode {
        ThinkingMode::Off => {
            limits.max_retries = limits.max_retries.min(1);
            limits.max_repeated_calls = limits.max_repeated_calls.min(2);
        }
        ThinkingMode::Auto | ThinkingMode::On => {}
    }
    limits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals(objective: &str) -> AutoSignals {
        AutoSignals::for_objective(objective)
    }

    #[test]
    fn greetings_do_not_show_thinking() {
        for objective in ["hello", "hi there", "thanks!", "good morning"] {
            assert!(
                !auto_shows_thinking(&signals(objective)),
                "`{objective}` should stay quiet"
            );
        }
    }

    #[test]
    fn simple_factual_questions_do_not_show_thinking() {
        for objective in ["capital of japan", "what is 2+2"] {
            assert!(
                !auto_shows_thinking(&signals(objective)),
                "`{objective}` should stay quiet"
            );
        }
    }

    #[test]
    fn coding_tasks_show_thinking() {
        for objective in [
            "fix this rust compile error in the loop engine",
            "refactor the provider adapters to share a builder",
            "implement retry backoff for the ollama client",
        ] {
            assert!(
                auto_shows_thinking(&signals(objective)),
                "`{objective}` should show thinking"
            );
        }
    }

    #[test]
    fn research_and_planning_and_verification_show_thinking() {
        for objective in [
            "research the current tokio release cadence",
            "break down the migration into stages",
            "verify that the sandbox tests still pass",
        ] {
            assert!(
                auto_shows_thinking(&signals(objective)),
                "`{objective}` should show thinking"
            );
        }
    }

    #[test]
    fn trivial_read_only_lookup_stays_quiet() {
        let quiet = AutoSignals {
            class: TaskClass::Coding,
            word_count: 4,
            predicted_actions: 1,
            writes: false,
            multi_file: false,
            external: false,
            needs_grounding: false,
        };
        assert!(!auto_shows_thinking(&quiet));

        // One more predicted action and it is real work again.
        let busier = AutoSignals {
            predicted_actions: 2,
            ..quiet.clone()
        };
        assert!(auto_shows_thinking(&busier));
    }

    #[test]
    fn simple_class_promotes_on_structural_evidence_only() {
        let base = AutoSignals {
            class: TaskClass::Simple,
            word_count: 3,
            ..Default::default()
        };
        assert!(!auto_shows_thinking(&base));

        for promoted in [
            AutoSignals {
                writes: true,
                ..base.clone()
            },
            AutoSignals {
                multi_file: true,
                ..base.clone()
            },
            AutoSignals {
                external: true,
                ..base.clone()
            },
            AutoSignals {
                predicted_actions: 2,
                ..base.clone()
            },
            AutoSignals {
                needs_grounding: true,
                ..base.clone()
            },
        ] {
            assert!(
                auto_shows_thinking(&promoted),
                "structural signal must promote: {promoted:?}"
            );
        }
    }

    #[test]
    fn decision_is_deterministic() {
        let corpus = [
            "hello",
            "capital of japan",
            "fix the flaky test",
            "research tokio releases",
            "rename every occurrence of Foo across the repository",
            "what is this file",
        ];
        for objective in corpus {
            let first = auto_shows_thinking(&signals(objective));
            for _ in 0..100 {
                assert_eq!(
                    auto_shows_thinking(&signals(objective)),
                    first,
                    "`{objective}` must decide identically every time"
                );
            }
        }
    }

    #[test]
    fn off_and_on_ignore_signals_entirely() {
        let noisy = signals("refactor everything everywhere");
        let quiet = signals("hello");
        assert!(!resolve_visibility(ThinkingMode::Off, &noisy).0);
        assert!(!resolve_visibility(ThinkingMode::Off, &quiet).0);
        assert!(resolve_visibility(ThinkingMode::On, &noisy).0);
        assert!(resolve_visibility(ThinkingMode::On, &quiet).0);
    }

    #[test]
    fn off_lowers_retry_tolerance_and_nothing_else() {
        let base = TurnLimits::default();
        let off = modulate_limits(&base, ThinkingMode::Off);

        assert_eq!(off.max_retries, 1);
        assert_eq!(off.max_repeated_calls, 2);

        // Every safety ceiling must be byte-identical. This is the guarantee
        // that thinking mode cannot truncate legitimate multi-step work.
        assert_eq!(off.max_steps, base.max_steps);
        assert_eq!(off.max_model_calls, base.max_model_calls);
        assert_eq!(off.max_tool_calls, base.max_tool_calls);
        assert_eq!(off.max_failures, base.max_failures);
        assert_eq!(off.max_total_tokens, base.max_total_tokens);
        assert_eq!(off.max_cost_micros, base.max_cost_micros);
        assert_eq!(off.max_duration_ms, base.max_duration_ms);
        assert_eq!(off.max_memory_writes, base.max_memory_writes);
        assert_eq!(off.max_subagents, base.max_subagents);
        assert_eq!(off.max_recursion_depth, base.max_recursion_depth);
    }

    #[test]
    fn on_never_raises_a_ceiling_above_configuration() {
        let strict = TurnLimits {
            max_retries: 1,
            max_repeated_calls: 1,
            ..TurnLimits::default()
        };
        let on = modulate_limits(&strict, ThinkingMode::On);
        assert_eq!(on.max_retries, 1, "a UI toggle must not escalate a limit");
        assert_eq!(on.max_repeated_calls, 1);
    }

    #[test]
    fn auto_leaves_limits_untouched() {
        let base = TurnLimits::default();
        let auto = modulate_limits(&base, ThinkingMode::Auto);
        assert_eq!(auto.max_retries, base.max_retries);
        assert_eq!(auto.max_repeated_calls, base.max_repeated_calls);
    }
}
