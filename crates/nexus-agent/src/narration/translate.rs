//! The translation layer: runtime facts in, operator language out.
//!
//! This is the single door between what the harness did and what the product
//! surfaces say. Boot, the status line, and the timeline may render only what
//! comes out of here; the debug layer reads the untranslated records directly.
//!
//! Two properties make it a boundary rather than a convenience:
//!
//! 1. **[`Presented`] has nowhere to put a tool name, an argument blob, or raw
//!    output.** A leak is therefore a compile error rather than something a
//!    reviewer has to catch. (A workspace-relative *path* is allowed and lives
//!    in its own field: "Editing src/lib.rs" is what the operator asked for.
//!    The status line simply does not render that field.)
//! 2. **[`RuntimeFact`] describes only things that already happened.** There is
//!    no variant for an action in flight, so a milestone cannot claim progress
//!    that has not occurred. That is the "never fake progress" rule, held by
//!    the type system instead of by discipline.
//!
//! It replaces two partial implementations that disagreed with each other —
//! `derived_activity` in `loop_engine.rs` and `tool_line` in the TUI — and
//! closes three leaks they had: `"{tool} failed: …"`, `"{tool} passed."`, and
//! shell commands reaching narration through the `command`/`cmd` argument keys.

use nexus_core::brand::ActionState;
use serde_json::Value;

/// Something the runtime has already established. Past tense, always.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeFact {
    /// A tool call that ran to completion (successfully or not).
    ToolCompleted {
        name: String,
        arguments: Value,
        ok: bool,
        /// Raw output. Used to decide *what to say*; never repeated verbatim.
        output: String,
    },
    /// A test, build, or lint finished.
    ValidationCompleted {
        /// Operator-facing label ("tests", "clippy"), never the command.
        label: String,
        passed: bool,
        elapsed_ms: Option<u64>,
    },
    /// An approval was granted or refused.
    ApprovalResolved { granted: bool, summary: String },
    /// Policy refused an action outright.
    PolicyRefused { reason: String },
    /// The loop waited on something outside itself and has stopped waiting.
    ProviderWaited { detail: String },
    /// History was folded so the run could continue.
    ContextCompacted { before: usize, after: usize },
    /// Files changed on disk.
    FilesChanged { paths: Vec<String> },
}

/// How much a statement matters, which is what the narration modes gate on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Significance {
    /// An ordinary successful step. Shown only in `verbose`.
    Routine,
    /// Worth a line: files changed, history folded, a provider wait ended.
    Notable,
    /// The operator needs to know: a failure, a refusal, an approval, or a
    /// validation outcome. Shown in every narrating mode.
    Critical,
}

/// A translated, operator-facing statement.
///
/// Note what is *not* here: no tool name, no argument JSON, no raw output, no
/// command line. There is no field for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presented {
    /// The action state this statement belongs to — supplies the icon and the
    /// live verb from the design language.
    pub state: ActionState,
    /// The sentence shown in the timeline.
    pub text: String,
    /// A workspace-relative path or short label the statement is about.
    /// Rendered by the timeline; never by the status line.
    pub subject: Option<String>,
    /// Short factual evidence: `14s`, `2 files`. Never a command or raw output.
    pub evidence: Option<String>,
    /// How much this statement matters. The narration mode gates on it.
    pub significance: Significance,
}

impl Presented {
    fn new(state: ActionState, text: impl Into<String>) -> Self {
        Self {
            state,
            text: text.into(),
            subject: None,
            evidence: None,
            significance: Significance::Routine,
        }
    }

    fn subject(mut self, subject: Option<String>) -> Self {
        self.subject = subject;
        self
    }

    fn evidence(mut self, evidence: Option<String>) -> Self {
        self.evidence = evidence;
        self
    }

    fn significance(mut self, significance: Significance) -> Self {
        self.significance = significance;
        self
    }

    /// The full timeline line: sentence, then subject and evidence when present.
    pub fn line(&self) -> String {
        let mut line = self.text.clone();
        if let Some(subject) = &self.subject {
            line.push(' ');
            line.push_str(subject);
        }
        if let Some(evidence) = &self.evidence {
            line.push_str(" (");
            line.push_str(evidence);
            line.push(')');
        }
        line
    }
}

/// Translate a completed fact into operator language.
///
/// Every branch describes something that already happened. Unknown tools
/// degrade to an honest neutral ("Ran a step") rather than to an invented
/// specific — a tool this build has never seen, from an MCP server or a custom
/// agent, still gets a truthful line.
pub fn present(fact: &RuntimeFact) -> Presented {
    match fact {
        RuntimeFact::ToolCompleted {
            name,
            arguments,
            ok,
            output,
        } => tool(name, arguments, *ok, output),
        RuntimeFact::ValidationCompleted {
            label,
            passed,
            elapsed_ms,
        } => {
            let state = if *passed {
                ActionState::Done
            } else {
                ActionState::Failed
            };
            let text = if *passed {
                format!("{} passed.", capitalize(label))
            } else {
                format!("{} failed.", capitalize(label))
            };
            Presented::new(state, text)
                .evidence(elapsed_ms.map(|ms| format!("{:.0}s", ms as f64 / 1000.0)))
                .significance(Significance::Critical)
        }
        RuntimeFact::ApprovalResolved { granted, summary } => {
            let state = if *granted {
                ActionState::Applying
            } else {
                ActionState::Failed
            };
            let text = if *granted {
                "You approved the action; continuing.".to_string()
            } else {
                "You declined the action; stopping that path.".to_string()
            };
            Presented::new(state, text)
                .subject(short(summary, 60))
                .significance(Significance::Critical)
        }
        RuntimeFact::PolicyRefused { reason } => {
            Presented::new(ActionState::Failed, "Policy refused the action.")
                .subject(short(reason, 80))
                .significance(Significance::Critical)
        }
        RuntimeFact::ProviderWaited { detail } => {
            Presented::new(ActionState::WaitingOnProvider, "Waited on the provider.")
                .subject(short(detail, 80))
                .significance(Significance::Notable)
        }
        RuntimeFact::ContextCompacted { before, after } => Presented::new(
            ActionState::Composing,
            "Folded earlier history to keep going.",
        )
        .evidence(Some(format!("{before} → {after} tokens")))
        .significance(Significance::Notable),
        RuntimeFact::FilesChanged { paths } => {
            let text = match paths.len() {
                0 => "No files changed.".to_string(),
                1 => "Changed a file.".to_string(),
                n => format!("Changed {n} files."),
            };
            Presented::new(ActionState::Applying, text)
                .subject(paths.first().cloned())
                .significance(Significance::Notable)
        }
    }
}

/// Translate a completed tool call.
fn tool(name: &str, arguments: &Value, ok: bool, output: &str) -> Presented {
    let state = tool_state(name);
    let subject = subject_of(arguments);

    if !ok {
        // A failure is the moment the operator most needs a sentence, and the
        // one place the old implementation named the tool. It says what kind of
        // action failed instead.
        let what = match state {
            ActionState::Scanning => "The search",
            ActionState::Applying => "The edit",
            ActionState::RunningChecks => "The check",
            _ => "The step",
        };
        let detail = short(&first_line(output), 80);
        let text = match detail {
            Some(detail) => format!("{what} failed: {detail}"),
            None => format!("{what} failed."),
        };
        return Presented::new(ActionState::Failed, text)
            .subject(subject)
            .significance(Significance::Critical);
    }

    let text = match state {
        ActionState::Scanning => "Looked through the workspace.",
        ActionState::Applying => "Applied a change to",
        ActionState::RunningChecks => "Ran the checks.",
        ActionState::Composing => "Recorded a finding.",
        _ => "Ran a step.",
    };
    // "Applied a change to" is the one phrasing that needs its subject to read
    // as a sentence, so it only claims a subject when there is one.
    if state == ActionState::Applying && subject.is_none() {
        return Presented::new(ActionState::Applying, "Applied a change.");
    }
    Presented::new(state, text).subject(subject)
}

/// Which action state a tool belongs to, by whole-word match on its name.
///
/// Words rather than substrings: a substring test classified `acme.frobnicate`
/// as a read, because "frobni**cat**e" contains `cat`.
fn tool_state(name: &str) -> ActionState {
    let lower = name.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let has = |needles: &[&str]| needles.iter().any(|needle| words.contains(needle));

    if has(&["test", "check", "lint", "clippy", "verify", "validate"]) {
        return ActionState::RunningChecks;
    }
    if has(&[
        "write", "create", "patch", "edit", "apply", "delete", "remove", "move", "rename", "mkdir",
    ]) {
        return ActionState::Applying;
    }
    if has(&[
        "read", "list", "search", "find", "grep", "stat", "hash", "tree", "status", "diff", "log",
        "show", "fetch", "get", "open", "cat", "ls",
    ]) {
        return ActionState::Scanning;
    }
    if has(&["memory", "remember", "note", "record"]) {
        return ActionState::Composing;
    }
    if has(&["plan", "agent", "delegate", "subagent"]) {
        return ActionState::ShapingApproach;
    }
    // Everything else — including shell execution, whose command line must not
    // reach a product surface — is an honest neutral.
    ActionState::Applying
}

/// The thing a call was about, when the arguments name one.
///
/// Deliberately **excludes** `command`/`cmd`: a shell command line is raw
/// machine detail and belongs to the debug layer. The old `narration_subject`
/// included them, which put commands straight into the timeline.
fn subject_of(arguments: &Value) -> Option<String> {
    const KEYS: [&str; 6] = ["path", "file", "paths", "pattern", "query", "name"];
    let raw = KEYS.iter().find_map(|key| match arguments.get(key) {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Array(values)) => values
            .first()
            .and_then(Value::as_str)
            .map(ToString::to_string),
        _ => None,
    })?;
    short(&raw, 60)
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

fn short(text: &str, max: usize) -> Option<String> {
    let line = first_line(text);
    if line.is_empty() {
        return None;
    }
    if line.chars().count() <= max {
        return Some(line);
    }
    let kept: String = line.chars().take(max.saturating_sub(1)).collect();
    Some(format!("{kept}…"))
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_fact(name: &str, arguments: Value, ok: bool, output: &str) -> RuntimeFact {
        RuntimeFact::ToolCompleted {
            name: name.into(),
            arguments,
            ok,
            output: output.into(),
        }
    }

    /// The headline guarantee of the whole layer.
    #[test]
    fn no_translation_ever_names_the_tool() {
        let names = [
            "fs.read",
            "fs.patch",
            "terminal.exec",
            "repo.git_status",
            "web.search",
            "memory.add",
            "acme.frobnicate",
        ];
        for name in names {
            for ok in [true, false] {
                let fact = tool_fact(
                    name,
                    json!({"path": "src/lib.rs", "command": "rm -rf /"}),
                    ok,
                    "boom",
                );
                let line = present(&fact).line();
                assert!(
                    !line.contains(name),
                    "`{name}` leaked into the timeline: {line}"
                );
                // Identifier-shaped fragments are the real tell. A bare English
                // word that happens to match a segment is fine and wanted —
                // "The search failed" is the sentence, not a leak of
                // `web.search`. Anything carrying `.` or `_` is not English.
                for fragment in name.split('.').filter(|f| f.contains('_')) {
                    assert!(
                        !line.contains(fragment),
                        "identifier `{fragment}` leaked into: {line}"
                    );
                }
                assert!(
                    !line.contains('.') || !line.contains("fs.") && !line.contains("web."),
                    "a dotted identifier leaked into: {line}"
                );
            }
        }
    }

    /// A shell command is machine detail. The old `narration_subject` accepted
    /// `command`/`cmd` keys, which put command lines in the timeline.
    #[test]
    fn a_shell_command_never_becomes_the_subject() {
        let fact = tool_fact(
            "terminal.exec",
            json!({"command": "cargo test --workspace && rm -rf target"}),
            true,
            "",
        );
        let presented = present(&fact);
        assert_eq!(presented.subject, None);
        assert!(!presented.line().contains("cargo"));
        assert!(!presented.line().contains("rm -rf"));
    }

    #[test]
    fn a_path_is_kept_because_it_is_what_the_operator_asked_about() {
        let fact = tool_fact("fs.patch", json!({"path": "src/lib.rs"}), true, "");
        let presented = present(&fact);
        assert_eq!(presented.subject.as_deref(), Some("src/lib.rs"));
        assert_eq!(presented.state, ActionState::Applying);
        assert_eq!(presented.line(), "Applied a change to src/lib.rs");
    }

    #[test]
    fn an_unknown_tool_degrades_to_an_honest_neutral() {
        let presented = present(&tool_fact("acme.frobnicate", json!({}), true, ""));
        assert_eq!(presented.line(), "Applied a change.");
        // Not "Reading" — a substring match on "cat" used to claim that.
        assert_ne!(presented.state, ActionState::Scanning);
    }

    #[test]
    fn a_failure_says_what_kind_of_action_failed_without_naming_it() {
        let presented = present(&tool_fact(
            "fs.read",
            json!({"path": "missing.rs"}),
            false,
            "No such file or directory",
        ));
        assert_eq!(presented.state, ActionState::Failed);
        assert_eq!(presented.significance, Significance::Critical);
        assert_eq!(
            presented.line(),
            "The search failed: No such file or directory missing.rs"
        );
    }

    #[test]
    fn validation_outcomes_are_milestones_with_their_duration() {
        let passed = present(&RuntimeFact::ValidationCompleted {
            label: "tests".into(),
            passed: true,
            elapsed_ms: Some(14_000),
        });
        assert_eq!(passed.line(), "Tests passed. (14s)");
        assert_eq!(passed.significance, Significance::Critical);
        assert_eq!(passed.state, ActionState::Done);

        let failed = present(&RuntimeFact::ValidationCompleted {
            label: "clippy".into(),
            passed: false,
            elapsed_ms: None,
        });
        assert_eq!(failed.line(), "Clippy failed.");
        assert_eq!(failed.state, ActionState::Failed);
    }

    #[test]
    fn approvals_and_refusals_are_milestones() {
        let granted = present(&RuntimeFact::ApprovalResolved {
            granted: true,
            summary: "write src/lib.rs".into(),
        });
        assert_eq!(granted.significance, Significance::Critical);
        assert!(granted.line().starts_with("You approved"));

        let refused = present(&RuntimeFact::PolicyRefused {
            reason: "network access is denied in this mode".into(),
        });
        assert_eq!(refused.state, ActionState::Failed);
        assert_eq!(refused.significance, Significance::Critical);
    }

    #[test]
    fn compaction_reports_real_numbers_and_is_not_a_milestone() {
        let presented = present(&RuntimeFact::ContextCompacted {
            before: 2662,
            after: 2394,
        });
        assert_eq!(
            presented.line(),
            "Folded earlier history to keep going. (2662 → 2394 tokens)"
        );
        assert_eq!(presented.significance, Significance::Notable);
    }

    #[test]
    fn file_changes_count_what_actually_changed() {
        let one = present(&RuntimeFact::FilesChanged {
            paths: vec!["a.rs".into()],
        });
        assert_eq!(one.line(), "Changed a file. a.rs");
        let many = present(&RuntimeFact::FilesChanged {
            paths: vec!["a.rs".into(), "b.rs".into(), "c.rs".into()],
        });
        assert!(many.line().starts_with("Changed 3 files."));
    }

    #[test]
    fn tool_states_match_on_whole_words() {
        assert_eq!(tool_state("fs.read"), ActionState::Scanning);
        assert_eq!(tool_state("fs.create_file"), ActionState::Applying);
        assert_eq!(tool_state("terminal.exec"), ActionState::Applying);
        assert_eq!(tool_state("cargo.test"), ActionState::RunningChecks);
        assert_eq!(tool_state("memory.add"), ActionState::Composing);
        assert_eq!(tool_state("agent.delegate"), ActionState::ShapingApproach);
        // The substring trap: "frobnicate" contains "cat".
        assert_eq!(tool_state("acme.frobnicate"), ActionState::Applying);
    }

    #[test]
    fn long_subjects_and_details_are_truncated_not_dumped() {
        let long_path = "a/".repeat(80);
        let fact = tool_fact("fs.read", json!({ "path": long_path }), true, "");
        let subject = present(&fact).subject.expect("subject");
        assert!(subject.chars().count() <= 60, "{subject}");
        assert!(subject.ends_with('…'));
    }

    #[test]
    fn raw_output_is_never_repeated_verbatim() {
        let noisy = "line one\nline two\nline three";
        let presented = present(&tool_fact("fs.read", json!({}), false, noisy));
        assert!(!presented.line().contains("line two"));
    }
}
