//! Versioned, secret-free persisted harness state.
//!
//! Holds the operator's cross-session choices that don't belong in the
//! hand-edited config file: active model, theme override, command history,
//! codex consent, last-selected menu entries. Stored as JSON next to the
//! workspace database and migrated by version on load. Secrets never land
//! here — credentials live in the restricted credential store.

use nexus_core::{NexusError, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const UI_STATE_VERSION: u32 = 4;
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
    /// Show provider reasoning summaries and operational traces. This never
    /// requests or exposes hidden chain-of-thought.
    pub thinking_enabled: bool,
    /// Persona selected for new sessions.
    pub selected_persona: Option<String>,
    /// Approved profile-trait collection selected for new sessions.
    pub profile_name: String,
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
            thinking_enabled: true,
            selected_persona: None,
            profile_name: "default".into(),
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
            self.thinking_enabled = true;
        }
        if self.version < 3 && self.profile_name.trim().is_empty() {
            self.profile_name = "default".into();
        }
        if self.version < 4 {
            self.claude_use_existing = false;
        }
        self.version = UI_STATE_VERSION;
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| NexusError::Other(format!("serializing state: {e}")))?;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("ui-state.json");
        let temp_path = path.with_file_name(format!(
            ".{file_name}.tmp-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let write_result = (|| -> Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp_path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&temp_path, path)?;
            if let Some(parent) = path.parent() {
                if let Ok(dir) = std::fs::File::open(parent) {
                    let _ = dir.sync_all();
                }
            }
            Ok(())
        })();
        if write_result.is_err() {
            let _ = std::fs::remove_file(&temp_path);
        }
        write_result?;
        Ok(())
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
}
