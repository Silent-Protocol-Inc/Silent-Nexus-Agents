//! Deliberation mode: how much execution insight the operator sees, and how
//! much optional deliberation the harness performs.
//!
//! This is a harness-level control, deliberately independent of the provider.
//! It is **not** a switch for chain-of-thought: raw provider reasoning is
//! destroyed at ingestion (see `nexus-agent`'s stream handling) and no value of
//! [`ThinkingMode`] can surface it. What the mode controls is (a) whether the
//! live activity component renders, and (b) how much *optional* deliberation —
//! grounding, staged planning, verification — the loop performs. Safety
//! ceilings (`TurnLimits`) are never widened by this setting.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::NexusError;

/// How much deliberation the harness performs, and how much of it is shown.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingMode {
    /// Optimize for latency. Optional deliberation is skipped for work that
    /// carries no risk flags, and the live activity component never renders.
    Off,
    /// Always render the activity component, and prefer grounded, staged
    /// execution with verification.
    On,
    /// Decide per turn from the deterministic task classification. The
    /// recommended default.
    #[default]
    Auto,
}

impl ThinkingMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ThinkingMode::Off => "off",
            ThinkingMode::On => "on",
            ThinkingMode::Auto => "auto",
        }
    }

    /// Status-bar value. Short by design: the bar shows `THINK deep`, not the
    /// whole mode name, because the label already says which control it is.
    pub fn bar_value(&self) -> &'static str {
        match self {
            ThinkingMode::Off => "fast",
            ThinkingMode::On => "deep",
            ThinkingMode::Auto => "auto",
        }
    }

    /// One-line description used by `/thinking status`, the CLI, and the menu.
    pub fn description(&self) -> &'static str {
        match self {
            ThinkingMode::Off => {
                "answers only — skips optional planning and extra verification passes"
            }
            ThinkingMode::On => {
                "always show what the agent is doing, with deeper planning and verification"
            }
            ThinkingMode::Auto => {
                "decide per request — quiet for quick answers, detailed for real work"
            }
        }
    }

    /// Next mode for the legacy `/thinking toggle` alias, which shipped in
    /// 2.3.0 as a boolean flip and is kept working as a three-way cycle.
    pub fn cycle(&self) -> Self {
        match self {
            ThinkingMode::Off => ThinkingMode::On,
            ThinkingMode::On => ThinkingMode::Auto,
            ThinkingMode::Auto => ThinkingMode::Off,
        }
    }

    /// Map the 2.3.0 `thinking_enabled` boolean onto a mode. `true` was the
    /// shipped default that nobody chose, so it maps to the new default rather
    /// than to `On`; an explicit `false` is a real preference and is honored.
    pub fn from_legacy_bool(enabled: bool) -> Self {
        if enabled {
            ThinkingMode::Auto
        } else {
            ThinkingMode::Off
        }
    }
}

impl FromStr for ThinkingMode {
    type Err = NexusError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "hide" | "false" | "0" | "fast" => Ok(ThinkingMode::Off),
            "on" | "show" | "true" | "1" | "deep" => Ok(ThinkingMode::On),
            "auto" | "adaptive" => Ok(ThinkingMode::Auto),
            other => Err(NexusError::Config(format!(
                "unknown thinking mode `{other}` — expected one of: off, on, auto"
            ))),
        }
    }
}

impl fmt::Display for ThinkingMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_round_trip_through_str() {
        for mode in [ThinkingMode::Off, ThinkingMode::On, ThinkingMode::Auto] {
            assert_eq!(
                mode.as_str().parse::<ThinkingMode>().expect("round trip"),
                mode
            );
            assert_eq!(mode.to_string(), mode.as_str());
        }
    }

    #[test]
    fn accepts_documented_aliases() {
        for word in ["off", "hide", "false", "0", "fast", "OFF", " Off "] {
            assert_eq!(
                word.parse::<ThinkingMode>().expect("alias parses"),
                ThinkingMode::Off
            );
        }
        for word in ["on", "show", "true", "1", "deep"] {
            assert_eq!(
                word.parse::<ThinkingMode>().expect("alias parses"),
                ThinkingMode::On
            );
        }
        for word in ["auto", "adaptive"] {
            assert_eq!(
                word.parse::<ThinkingMode>().expect("alias parses"),
                ThinkingMode::Auto
            );
        }
    }

    #[test]
    fn unknown_mode_error_names_every_valid_value() {
        let err = "sometimes"
            .parse::<ThinkingMode>()
            .expect_err("must reject")
            .to_string();
        for expected in ["off", "on", "auto"] {
            assert!(err.contains(expected), "{err} should mention {expected}");
        }
    }

    #[test]
    fn default_is_auto() {
        assert_eq!(ThinkingMode::default(), ThinkingMode::Auto);
    }

    #[test]
    fn legacy_true_maps_to_auto_and_false_to_off() {
        // `true` was the unchosen 2.3.0 default, so it must land on the new
        // default rather than forcing every existing user into On.
        assert_eq!(ThinkingMode::from_legacy_bool(true), ThinkingMode::Auto);
        assert_eq!(ThinkingMode::from_legacy_bool(false), ThinkingMode::Off);
    }

    #[test]
    fn toggle_cycles_through_every_mode() {
        let mut mode = ThinkingMode::Off;
        let mut seen = vec![mode];
        for _ in 0..3 {
            mode = mode.cycle();
            seen.push(mode);
        }
        assert_eq!(
            seen,
            vec![
                ThinkingMode::Off,
                ThinkingMode::On,
                ThinkingMode::Auto,
                ThinkingMode::Off
            ]
        );
    }

    #[test]
    fn bar_values_are_distinct_and_short() {
        let values = [
            ThinkingMode::Off.bar_value(),
            ThinkingMode::On.bar_value(),
            ThinkingMode::Auto.bar_value(),
        ];
        for value in values {
            assert!(value.len() <= 4, "{value} is too wide for the status bar");
        }
        assert_eq!(
            values
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            3
        );
    }

    #[test]
    fn serde_uses_lowercase_names() {
        let json = serde_json::to_string(&ThinkingMode::Auto).expect("serialize");
        assert_eq!(json, "\"auto\"");
        let parsed: ThinkingMode = serde_json::from_str("\"off\"").expect("deserialize");
        assert_eq!(parsed, ThinkingMode::Off);
    }
}
