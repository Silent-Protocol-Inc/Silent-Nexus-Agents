//! Versioned, secret-free persisted harness state.
//!
//! Holds the operator's cross-session choices that don't belong in the
//! hand-edited config file: active model, theme override, command history,
//! codex consent, last-selected menu entries. Stored as JSON next to the
//! workspace database and migrated by version on load. Secrets never land
//! here — credentials live in the restricted credential store.

use nexus_core::{NexusError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const UI_STATE_VERSION: u32 = 8;
const HISTORY_CAP: usize = 200;
const RECENT_COMMANDS_CAP: usize = 12;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    pub version: u32,
    /// Model (config entry name) selected via `/connect` or `snx model use`.
    pub active_model: Option<String>,
    /// Theme override chosen via `/theme` (falls back to config.general.theme).
    pub theme: Option<String>,
    /// Input history (newest last). Secret inputs are never recorded.
    pub history: Vec<String>,
    /// Recently executed slash commands (newest first, deduplicated).
    pub recent_commands: Vec<String>,
    /// Operator consented to reusing the existing Codex CLI session without
    /// copying it (the `/login` "use temporarily" choice).
    pub codex_use_existing: bool,
    /// Operator consent for the Claude plan bridge to use the existing
    /// official Claude Code subscription login.
    pub claude_use_existing: bool,
    /// Session id to offer first in `/resume`.
    pub last_session: Option<String>,
    /// Goal id the TUI treats as active.
    pub active_goal: Option<String>,
    /// Agent role selected for new sessions (falls back to config default).
    pub active_agent: Option<String>,
    /// Last selected provider in the `/connect` view.
    pub last_provider: Option<String>,
    /// Deliberation mode chosen via `/thinking`: `off`, `on`, or `auto`. This
    /// never requests or exposes hidden chain-of-thought — it controls how much
    /// of the harness's own execution state is shown and how much optional
    /// deliberation the loop performs.
    pub thinking_mode: String,
    /// The v6 boolean this replaced. Read once during migration, then cleared
    /// and never written again. It exists only because `UiState` derives
    /// `serde(default)` without `deny_unknown_fields`: without a field to land
    /// in, a v6 file's explicit `false` would be silently discarded and the
    /// operator's choice lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_enabled: Option<bool>,
    /// True once the harness has completed a first interactive launch, so the
    /// onboarding banner shows exactly once.
    pub first_run_completed: bool,
    /// Timeline verbosity chosen via `/view`: `default`, `detailed`, or `debug`.
    /// Default keeps the timeline to essential activity; diagnostics stay one
    /// keystroke away rather than flooding the transcript.
    pub activity_mode: String,
    /// How much the agent narrates its own work, chosen via `/narrate`:
    /// `off`, `compact`, `auto`, or `verbose`. Empty until the operator
    /// chooses, so `[narration].mode` in config keeps applying. A different axis from
    /// `activity_mode` — that one decides which stored events render, this one
    /// decides whether the agent says what it is doing. There is deliberately
    /// no `debug` value here; raw-payload visibility belongs to `/view`.
    pub narration_mode: String,
    /// Plan mode, entered with `/plan`. While it is on, every turn runs under a
    /// scope that refuses to change anything, and the agent's job is to author
    /// a plan for approval rather than to act. Approving it clears this.
    pub plan_mode: bool,
    /// Persona selected for new sessions.
    pub selected_persona: Option<String>,
    /// Approved profile-trait collection selected for new sessions.
    pub profile_name: String,
    /// Presentation-only state for menu routes. Domain selections live in the
    /// SQLite ActiveHarnessContext and are mirrored into legacy fields only
    /// for 1.x compatibility.
    pub menus: BTreeMap<String, PersistedMenuState>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PersistedMenuState {
    pub selected_item_id: Option<String>,
    pub focused_region: String,
    pub search_query: String,
    pub filters: BTreeMap<String, String>,
    pub sort_key: Option<String>,
    pub sort_descending: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            version: UI_STATE_VERSION,
            active_model: None,
            theme: None,
            history: Vec::new(),
            recent_commands: Vec::new(),
            codex_use_existing: false,
            claude_use_existing: false,
            last_session: None,
            active_goal: None,
            active_agent: None,
            last_provider: None,
            thinking_mode: nexus_core::ThinkingMode::default().as_str().into(),
            thinking_enabled: None,
            first_run_completed: false,
            activity_mode: "default".into(),
            // Empty means "the operator has not chosen", which is what lets
            // `[narration].mode` in config still apply. A filled-in default
            // here would silently outrank the config file forever.
            narration_mode: String::new(),
            plan_mode: false,
            selected_persona: None,
            profile_name: "default".into(),
            menus: BTreeMap::new(),
        }
    }
}

impl UiState {
    /// Load from `path`; a missing file is a default state, a malformed file
    /// is an error naming the file.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)?;
        let mut state: UiState = serde_json::from_str(&text).map_err(|e| {
            NexusError::Config(format!("{} is not valid state JSON: {e}", path.display()))
        })?;
        state.migrate();
        Ok(state)
    }

    fn migrate(&mut self) {
        if self.version < 2 {
            // `serde(default)` supplies the safe default for the new field.
            self.thinking_enabled = Some(true);
        }
        if self.version < 3 && self.profile_name.trim().is_empty() {
            self.profile_name = "default".into();
        }
        if self.version < 4 {
            self.claude_use_existing = false;
        }
        if self.version < 5 {
            self.menus.clear();
        }
        if self.version < 6 && self.activity_mode.trim().is_empty() {
            self.activity_mode = "default".into();
        }
        if self.version < 7 {
            // v6 stored a boolean. `true` was the shipped default nobody
            // chose, so it maps to the new default; an explicit `false` is a
            // real preference and becomes `off`.
            self.thinking_mode =
                nexus_core::ThinkingMode::from_legacy_bool(self.thinking_enabled.unwrap_or(true))
                    .as_str()
                    .into();
            // An existing state file is itself proof of a prior launch, so a
            // returning operator never sees the first-run banner.
            self.first_run_completed = true;
        }
        if self.version < 8 {
            // Plan mode is a deliberate act, never a state someone is restored
            // into: a stored `true` from a crashed session would silently
            // refuse the operator's next edit.
            self.plan_mode = false;
        }
        // `narration_mode` deliberately has no migration step: it is a new
        // axis, so no prior operator choice exists to preserve, and leaving it
        // empty is what keeps `[narration].mode` in config authoritative until
        // someone actually runs `/narrate`.
        // Never re-emit the legacy key, whatever version we loaded.
        self.thinking_enabled = None;
        if self.thinking_mode.trim().is_empty() {
            self.thinking_mode = nexus_core::ThinkingMode::default().as_str().into();
        }

        self.version = UI_STATE_VERSION;
    }

    /// The persisted deliberation mode, falling back to the default if the
    /// stored string is unrecognized rather than failing the whole load.
    pub fn thinking(&self) -> nexus_core::ThinkingMode {
        self.thinking_mode.parse().unwrap_or_default()
    }

    /// The operator's explicit narration choice, or `None` when they have not
    /// made one and config still decides. An unrecognized stored value reads as
    /// no choice rather than failing the load — an unreadable presentation
    /// preference must never stop the agent.
    pub fn narration(&self) -> Option<nexus_core::timeline::NarrationMode> {
        nexus_core::timeline::NarrationMode::parse(&self.narration_mode)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            nexus_core::permissions::repair_private_tree(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| NexusError::Other(format!("serializing state: {e}")))?;
        nexus_core::atomic::atomic_write_private(path, text.as_bytes())
    }

    /// Record an input line in history (skips duplicates of the last entry).
    pub fn push_history(&mut self, line: &str) {
        if line.trim().is_empty() || self.history.last().map(String::as_str) == Some(line) {
            return;
        }
        self.history.push(line.to_string());
        let overflow = self.history.len().saturating_sub(HISTORY_CAP);
        if overflow > 0 {
            self.history.drain(..overflow);
        }
    }

    /// Record a slash command as recently used (front of the list, deduped).
    pub fn push_recent_command(&mut self, name: &str) {
        self.recent_commands.retain(|c| c != name);
        self.recent_commands.insert(0, name.to_string());
        self.recent_commands.truncate(RECENT_COMMANDS_CAP);
    }

    pub fn remember_menu(&mut self, route: impl Into<String>, state: PersistedMenuState) {
        // Menu state is presentation-only and intentionally uses a bounded,
        // deterministic route map. Once full, the lexicographically earliest
        // route is evicted; no timestamps or user content are needed merely
        // to manage this cache.
        const MENU_STATE_CAP: usize = 64;
        self.menus.insert(route.into(), state);
        while self.menus.len() > MENU_STATE_CAP {
            let Some(first_key) = self.menus.keys().next().cloned() else {
                break;
            };
            self.menus.remove(&first_key);
        }
    }
}

/// Handle bundling the state with its on-disk location.
#[derive(Debug, Clone)]
pub struct UiStateFile {
    pub path: PathBuf,
    pub state: UiState,
}

impl UiStateFile {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
            state: UiState::load(path)?,
        })
    }

    pub fn save(&self) -> Result<()> {
        self.state.save(&self.path)
    }

    /// Mutate the state and persist in one step.
    pub fn update(&mut self, f: impl FnOnce(&mut UiState)) -> Result<()> {
        let mut next = self.state.clone();
        f(&mut next);
        next.save(&self.path)?;
        self.state = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("state").join("ui-state.json");
        let mut f = UiStateFile::load(&path).expect("load default");
        assert_eq!(f.state.active_model, None);
        f.update(|s| {
            s.active_model = Some("qwen".into());
            s.push_history("/status");
            s.push_recent_command("status");
        })
        .expect("save");
        let g = UiStateFile::load(&path).expect("reload");
        assert_eq!(g.state.active_model.as_deref(), Some("qwen"));
        assert_eq!(g.state.history, vec!["/status".to_string()]);
        assert_eq!(g.state.recent_commands, vec!["status".to_string()]);
    }

    #[test]
    fn history_caps_and_dedups() {
        let mut s = UiState::default();
        s.push_history("a");
        s.push_history("a");
        assert_eq!(s.history.len(), 1);
        for i in 0..300 {
            s.push_history(&format!("line {i}"));
        }
        assert_eq!(s.history.len(), 200);
        assert_eq!(s.history.last().map(String::as_str), Some("line 299"));
    }

    #[test]
    fn malformed_state_is_an_error() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("ui-state.json");
        std::fs::write(&path, "not json").expect("write");
        assert!(UiState::load(&path).is_err());
    }

    #[test]
    fn v4_state_migrates_with_empty_menu_state() {
        let raw = r#"{
            "version": 4,
            "profile_name": "Sans",
            "thinking_enabled": true
        }"#;
        let mut state: UiState = serde_json::from_str(raw).expect("legacy state");
        state.migrate();
        assert_eq!(state.version, UI_STATE_VERSION);
        assert_eq!(state.profile_name, "Sans");
        assert!(state.menus.is_empty());
    }

    #[test]
    fn v5_state_migrates_to_the_concise_activity_view() {
        let raw = r#"{ "version": 5, "thinking_enabled": true }"#;
        let mut state: UiState = serde_json::from_str(raw).expect("legacy state");
        state.migrate();
        assert_eq!(state.version, UI_STATE_VERSION);
        assert_eq!(state.activity_mode, "default");
    }

    #[test]
    fn v6_default_thinking_becomes_auto() {
        // `true` was the shipped default nobody chose: it must land on the new
        // default rather than forcing every existing operator into `on`.
        let raw = r#"{ "version": 6, "thinking_enabled": true }"#;
        let mut state: UiState = serde_json::from_str(raw).expect("legacy state");
        state.migrate();
        assert_eq!(state.thinking(), nexus_core::ThinkingMode::Auto);
    }

    #[test]
    fn v6_explicit_opt_out_is_preserved_as_off() {
        // The regression the shadow field exists to prevent: without it serde
        // drops the unknown key and this operator silently gets `auto`.
        let raw = r#"{ "version": 6, "thinking_enabled": false }"#;
        let mut state: UiState = serde_json::from_str(raw).expect("legacy state");
        state.migrate();
        assert_eq!(state.thinking(), nexus_core::ThinkingMode::Off);
    }

    #[test]
    fn migrated_state_never_re_emits_the_legacy_key() {
        let raw = r#"{ "version": 6, "thinking_enabled": false }"#;
        let mut state: UiState = serde_json::from_str(raw).expect("legacy state");
        state.migrate();
        let json = serde_json::to_string(&state).expect("serialize");
        assert!(
            !json.contains("thinking_enabled"),
            "the v6 key must not survive into a v7 file: {json}"
        );
        assert!(json.contains("thinking_mode"));
    }

    #[test]
    fn upgrading_never_restores_someone_into_plan_mode() {
        // A crash while planning leaves `plan_mode: true` on disk. Honoring it
        // on the next launch would make the operator's first edit be refused
        // by a mode they did not knowingly re-enter.
        let raw = r#"{ "version": 7, "plan_mode": true }"#;
        let mut state: UiState = serde_json::from_str(raw).expect("v7 state");
        state.migrate();
        assert!(!state.plan_mode);
        assert_eq!(state.version, UI_STATE_VERSION);

        // Within the current version the flag is a real preference and stands.
        let raw = format!(r#"{{ "version": {UI_STATE_VERSION}, "plan_mode": true }}"#);
        let mut state: UiState = serde_json::from_str(&raw).expect("current state");
        state.migrate();
        assert!(state.plan_mode);
    }

    #[test]
    fn an_existing_state_file_is_never_treated_as_a_first_run() {
        let raw = r#"{ "version": 6 }"#;
        let mut state: UiState = serde_json::from_str(raw).expect("legacy state");
        state.migrate();
        assert!(
            state.first_run_completed,
            "a returning operator must not see the onboarding banner"
        );
    }

    #[test]
    fn a_fresh_state_is_a_first_run_on_auto() {
        let state = UiState::default();
        assert!(!state.first_run_completed);
        assert_eq!(state.thinking(), nexus_core::ThinkingMode::Auto);
    }

    #[test]
    fn an_unreadable_thinking_mode_falls_back_to_the_default() {
        let raw = r#"{ "version": 7, "thinking_mode": "enthusiastic" }"#;
        let state: UiState = serde_json::from_str(raw).expect("state");
        assert_eq!(state.thinking(), nexus_core::ThinkingMode::Auto);
    }

    #[test]
    fn an_explicit_v7_mode_round_trips() {
        let raw = r#"{ "version": 7, "thinking_mode": "off" }"#;
        let mut state: UiState = serde_json::from_str(raw).expect("state");
        state.migrate();
        assert_eq!(state.thinking(), nexus_core::ThinkingMode::Off);
    }

    #[test]
    fn an_explicit_activity_mode_survives_migration() {
        let raw = r#"{ "version": 5, "activity_mode": "debug" }"#;
        let mut state: UiState = serde_json::from_str(raw).expect("legacy state");
        state.migrate();
        assert_eq!(state.activity_mode, "debug");
    }

    #[test]
    fn save_replaces_the_file_without_leaving_a_temp_artifact() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("ui-state.json");
        let mut state = UiState {
            active_model: Some("first".into()),
            ..Default::default()
        };
        state.save(&path).expect("first save");
        state.active_model = Some("second".into());
        state.save(&path).expect("second save");
        assert_eq!(
            UiState::load(&path).expect("load").active_model.as_deref(),
            Some("second")
        );
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "temporary state files leaked");
    }

    #[test]
    fn menu_state_cache_is_bounded_and_deterministic() {
        let mut state = UiState::default();
        for index in 0..70 {
            state.remember_menu(
                format!("/route-{index:02}"),
                PersistedMenuState {
                    selected_item_id: Some(format!("item-{index}")),
                    ..PersistedMenuState::default()
                },
            );
        }
        assert_eq!(state.menus.len(), 64);
        assert!(!state.menus.contains_key("/route-00"));
        assert!(state.menus.contains_key("/route-69"));
    }
}
