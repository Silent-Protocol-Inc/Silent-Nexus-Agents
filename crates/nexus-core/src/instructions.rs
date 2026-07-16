//! Project instruction files (agent guidance checked into the workspace).
//!
//! Silent Nexus reads the same instruction files other agent harnesses use,
//! so a repo teaches every assistant once: its own `SILENT.md` first, then
//! the cross-tool `AGENTS.md`, then provider-specific files. Only the highest
//! priority file is injected (they usually duplicate each other); the others
//! are reported so the operator knows what was skipped.

use std::path::{Path, PathBuf};

/// Instruction file names in priority order. First usable match wins.
pub const INSTRUCTION_FILES: &[&str] = &[
    "SILENT.md", // Silent Nexus native
    "AGENTS.md", // cross-tool convention (Codex, others)
    "CLAUDE.md", // Claude Code
    "GEMINI.md", // Gemini CLI
    "QWEN.md",   // Qwen Code
    ".github/copilot-instructions.md",
];

/// Injected content is capped so a huge instructions file cannot crowd out
/// the actual conversation.
pub const MAX_INSTRUCTION_CHARS: usize = 24_000;

/// A loaded project instruction file.
#[derive(Debug, Clone)]
pub struct ProjectInstructions {
    /// File name relative to the workspace root (e.g. `CLAUDE.md`).
    pub source: String,
    /// File content, truncated to [`MAX_INSTRUCTION_CHARS`] when oversized.
    pub content: String,
    /// True when the content was truncated.
    pub truncated: bool,
    /// Other instruction files that exist but were not injected.
    pub also_present: Vec<String>,
}

/// One instruction-file candidate discovered in the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionCandidate {
    pub source: String,
    pub path: PathBuf,
    /// Whether the file can contribute instructions. Empty and unreadable
    /// files remain visible to `/init`, but never mask a usable lower-priority
    /// provider file.
    pub usable: bool,
    pub reason: Option<String>,
}

/// Discover every supported instruction file in priority order.
pub fn discover(workspace: &Path) -> Vec<InstructionCandidate> {
    INSTRUCTION_FILES
        .iter()
        .filter_map(|name| {
            let path = workspace.join(name);
            if !path.is_file() {
                return None;
            }
            let (usable, reason) = match std::fs::read_to_string(&path) {
                Ok(text) if text.trim().is_empty() => (false, Some("empty; skipped".to_string())),
                Ok(_) => (true, None),
                Err(e) => (false, Some(format!("unreadable; skipped ({e})"))),
            };
            Some(InstructionCandidate {
                source: (*name).to_string(),
                path,
                usable,
                reason,
            })
        })
        .collect()
}

/// Find and load the workspace's instruction file, if any.
pub fn load(workspace: &Path) -> Option<ProjectInstructions> {
    let existing = discover(workspace);
    let chosen = existing.iter().find(|candidate| candidate.usable)?;
    let raw = std::fs::read_to_string(&chosen.path).ok()?;
    let raw = raw.trim();
    let truncated = raw.chars().count() > MAX_INSTRUCTION_CHARS;
    let content = if truncated {
        let mut s: String = raw.chars().take(MAX_INSTRUCTION_CHARS).collect();
        s.push_str("\n[…truncated]");
        s
    } else {
        raw.to_string()
    };
    Some(ProjectInstructions {
        source: chosen.source.clone(),
        content,
        truncated,
        also_present: existing
            .iter()
            .filter(|candidate| candidate.source != chosen.source)
            .map(|candidate| candidate.source.clone())
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn prefers_silent_md_over_provider_files() {
        let dir = tempfile::tempdir().expect("dir");
        fs::write(dir.path().join("CLAUDE.md"), "claude rules").expect("write");
        fs::write(dir.path().join("SILENT.md"), "silent rules").expect("write");
        let ins = load(dir.path()).expect("some");
        assert_eq!(ins.source, "SILENT.md");
        assert_eq!(ins.content, "silent rules");
        assert_eq!(ins.also_present, vec!["CLAUDE.md".to_string()]);
    }

    #[test]
    fn adapts_any_provider_file() {
        let dir = tempfile::tempdir().expect("dir");
        fs::write(dir.path().join("GEMINI.md"), "gemini rules").expect("write");
        let ins = load(dir.path()).expect("some");
        assert_eq!(ins.source, "GEMINI.md");
        assert!(!ins.truncated);
    }

    #[test]
    fn empty_or_missing_is_none() {
        let dir = tempfile::tempdir().expect("dir");
        assert!(load(dir.path()).is_none());
        fs::write(dir.path().join("AGENTS.md"), "   \n").expect("write");
        assert!(load(dir.path()).is_none());
    }

    #[test]
    fn empty_higher_priority_file_does_not_mask_usable_lower_priority_file() {
        let dir = tempfile::tempdir().expect("dir");
        fs::write(dir.path().join("SILENT.md"), " \n").expect("write");
        fs::write(dir.path().join("AGENTS.md"), "usable rules").expect("write");
        let ins = load(dir.path()).expect("usable lower-priority file");
        assert_eq!(ins.source, "AGENTS.md");
        assert_eq!(ins.content, "usable rules");
        assert_eq!(ins.also_present, vec!["SILENT.md".to_string()]);
    }

    #[test]
    fn discover_reports_empty_candidates_without_marking_them_usable() {
        let dir = tempfile::tempdir().expect("dir");
        fs::write(dir.path().join("SILENT.md"), "").expect("write");
        let candidates = discover(dir.path());
        assert_eq!(candidates.len(), 1);
        assert!(!candidates[0].usable);
        assert!(candidates[0]
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("empty")));
    }

    #[test]
    fn oversized_content_is_truncated() {
        let dir = tempfile::tempdir().expect("dir");
        let big = "x".repeat(MAX_INSTRUCTION_CHARS + 100);
        fs::write(dir.path().join("AGENTS.md"), &big).expect("write");
        let ins = load(dir.path()).expect("some");
        assert!(ins.truncated);
        assert!(ins.content.ends_with("[…truncated]"));
    }
}
