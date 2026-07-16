//! Deterministic task classification.
//!
//! A router model can classify a request, but small local setups may not have
//! one — so classification always has a deterministic keyword fallback. The
//! class selects both the routed model and the minimal tool categories exposed
//! that turn (lazy tool discovery).

use nexus_models::types::TaskClass;
use nexus_tools::ToolCategory;

/// Classify an objective by keyword heuristics. Conservative: ambiguous input
/// falls back to `Coding` (the broadest useful tool set) rather than `Simple`.
pub fn classify(objective: &str) -> TaskClass {
    let lower = objective.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));

    if has(&[
        "search the web",
        "look up online",
        "research",
        "find documentation",
        "latest version",
        "what is the current",
        "cite",
        "sources for",
    ]) {
        return TaskClass::Research;
    }
    if has(&[
        "plan",
        "break down",
        "decompose",
        "roadmap",
        "design an approach",
        "outline the steps",
        "strategy",
    ]) {
        return TaskClass::Planning;
    }
    if has(&[
        "verify",
        "check that",
        "confirm",
        "validate",
        "make sure the tests",
        "does it pass",
        "prove",
    ]) {
        return TaskClass::Verification;
    }
    if has(&[
        "fix",
        "implement",
        "refactor",
        "add a",
        "write a",
        "debug",
        "compile",
        "test",
        "function",
        "bug",
        "error",
        "class",
        "method",
        "code",
        "build",
        "patch",
        "rename",
        "file",
        "repository",
        "repo",
    ]) {
        return TaskClass::Coding;
    }
    // Very short, greeting-like, or Q&A without code cues → simple.
    let word_count = objective.split_whitespace().count();
    if word_count <= 8 && !has(&["how", "why", "explain"]) {
        return TaskClass::Simple;
    }
    TaskClass::Coding
}

/// Minimal tool categories to expose for a task class. Small models never see
/// the whole catalog — only what the class plausibly needs.
pub fn tool_categories(class: TaskClass) -> Vec<ToolCategory> {
    match class {
        TaskClass::Simple => vec![ToolCategory::Filesystem, ToolCategory::Diagnostics],
        TaskClass::Coding => vec![
            ToolCategory::Filesystem,
            ToolCategory::Repo,
            ToolCategory::Terminal,
            ToolCategory::Diagnostics,
        ],
        TaskClass::Planning => vec![
            ToolCategory::Filesystem,
            ToolCategory::Repo,
            ToolCategory::Diagnostics,
        ],
        TaskClass::Research => vec![ToolCategory::Web, ToolCategory::Filesystem],
        TaskClass::Verification => vec![
            ToolCategory::Repo,
            ToolCategory::Terminal,
            ToolCategory::Filesystem,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_is_deterministic() {
        assert_eq!(
            classify("fix the failing test in src/lib.rs"),
            TaskClass::Coding
        );
        assert_eq!(
            classify("research the latest tokio version online"),
            TaskClass::Research
        );
        assert_eq!(
            classify("plan how to add authentication"),
            TaskClass::Planning
        );
        assert_eq!(
            classify("verify that the build passes"),
            TaskClass::Verification
        );
        assert_eq!(classify("hello"), TaskClass::Simple);
    }

    #[test]
    fn ambiguous_defaults_to_coding_not_simple() {
        assert_eq!(
            classify("please handle the thing we discussed about the widget subsystem"),
            TaskClass::Coding
        );
    }

    #[test]
    fn coding_gets_repo_and_terminal_tools() {
        let cats = tool_categories(TaskClass::Coding);
        assert!(cats.contains(&ToolCategory::Repo));
        assert!(cats.contains(&ToolCategory::Terminal));
        // Research should NOT expose the terminal.
        assert!(!tool_categories(TaskClass::Research).contains(&ToolCategory::Terminal));
    }
}
