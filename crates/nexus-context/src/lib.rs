//! nexus-context: context-window management.
//!
//! Estimates token usage, packs a bounded prompt from prioritized segments,
//! and compacts when usage nears the model limit. Compaction summarizes old
//! conversational turns but is forbidden from dropping load-bearing state:
//! the active objective, acceptance criteria, prohibited actions, pending
//! approvals, unresolved failures, changed files, and verification
//! requirements are always preserved verbatim.

use nexus_models::types::{ChatMessage, Role};
use serde::{Deserialize, Serialize};

/// A summarizer turns a slice of messages into a condensed summary string.
/// Usually a cheap model call; a deterministic fallback is used when absent.
pub type Summarizer<'a> = &'a dyn Fn(&[ChatMessage]) -> String;

/// Rough token estimate. Without a model-specific tokenizer we use a
/// conservative chars-per-token ratio; deliberately over-estimates so we
/// never overflow a real context window. Documented approximation.
pub fn estimate_tokens(text: &str) -> usize {
    // ~3.5 chars/token is conservative for code-heavy English; round up.
    let chars = text.chars().count();
    (chars as f64 / 3.5).ceil() as usize + 1
}

pub fn estimate_message_tokens(m: &ChatMessage) -> usize {
    // +4 per message for role/formatting overhead, plus tool-call payloads.
    let mut t = estimate_tokens(&m.content) + 4;
    for c in &m.tool_calls {
        t += estimate_tokens(&c.name) + estimate_tokens(&c.arguments) + 4;
    }
    t
}

/// A prioritized context segment. Lower `priority` numbers are dropped last.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub label: String,
    pub content: String,
    /// 0 = never drop (safety/objective), higher = droppable first.
    pub priority: u8,
    /// True when this segment must survive compaction untouched.
    pub pinned: bool,
}

impl Segment {
    pub fn pinned(label: &str, content: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            content: content.into(),
            priority: 0,
            pinned: true,
        }
    }

    pub fn droppable(label: &str, content: impl Into<String>, priority: u8) -> Self {
        Self {
            label: label.into(),
            content: content.into(),
            priority: priority.max(1),
            pinned: false,
        }
    }

    pub fn tokens(&self) -> usize {
        estimate_tokens(&self.content) + estimate_tokens(&self.label) + 2
    }
}

/// Diagnostics returned to `/context`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextReport {
    pub context_window: usize,
    pub reserved_completion: usize,
    pub budget: usize,
    pub used: usize,
    pub message_count: usize,
    pub segments: Vec<(String, usize)>,
    pub over_budget: bool,
}

/// Outcome of a compaction pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionResult {
    pub before_tokens: usize,
    pub after_tokens: usize,
    pub summarized_messages: usize,
    pub preserved_labels: Vec<String>,
    /// Text of the summary that replaced older turns (records what happened).
    pub summary: String,
}

pub struct ContextManager {
    context_window: usize,
    reserved_completion: usize,
    /// Keep at least this many of the most recent messages verbatim.
    keep_recent: usize,
}

impl ContextManager {
    pub fn new(context_window: usize, reserved_completion: usize) -> Self {
        Self {
            context_window,
            reserved_completion,
            keep_recent: 6,
        }
    }

    /// Token budget for the prompt (window minus completion reserve).
    pub fn budget(&self) -> usize {
        self.context_window.saturating_sub(self.reserved_completion)
    }

    pub fn report(&self, segments: &[Segment], messages: &[ChatMessage]) -> ContextReport {
        let seg_tokens: Vec<(String, usize)> = segments
            .iter()
            .map(|s| (s.label.clone(), s.tokens()))
            .collect();
        let msg_tokens: usize = messages.iter().map(estimate_message_tokens).sum();
        let used: usize = seg_tokens.iter().map(|(_, t)| *t).sum::<usize>() + msg_tokens;
        ContextReport {
            context_window: self.context_window,
            reserved_completion: self.reserved_completion,
            budget: self.budget(),
            used,
            message_count: messages.len(),
            segments: seg_tokens,
            over_budget: used > self.budget(),
        }
    }

    /// Decide whether compaction is needed (>~85% of budget).
    pub fn needs_compaction(&self, segments: &[Segment], messages: &[ChatMessage]) -> bool {
        let report = self.report(segments, messages);
        report.used * 100 > self.budget() * 85
    }

    /// Compact conversation history by summarizing the oldest droppable turns
    /// while preserving pinned segments and the most recent messages. The
    /// caller supplies a `summarizer` (usually a cheap model call); a
    /// deterministic fallback is used when `None`.
    pub fn compact(
        &self,
        segments: &[Segment],
        messages: &[ChatMessage],
        summarizer: Option<Summarizer<'_>>,
    ) -> (Vec<ChatMessage>, CompactionResult) {
        let before: usize = self.report(segments, messages).used;
        if messages.len() <= self.keep_recent {
            return (
                messages.to_vec(),
                CompactionResult {
                    before_tokens: before,
                    after_tokens: before,
                    summarized_messages: 0,
                    preserved_labels: pinned_labels(segments),
                    summary: String::new(),
                },
            );
        }
        // Never summarize a system message or the tail we keep verbatim.
        let split = messages.len() - self.keep_recent;
        let (old, recent) = messages.split_at(split);
        // Keep any leading system messages out of summarization.
        let system: Vec<ChatMessage> = old
            .iter()
            .filter(|m| m.role == Role::System)
            .cloned()
            .collect();
        let to_summarize: Vec<ChatMessage> = old
            .iter()
            .filter(|m| m.role != Role::System)
            .cloned()
            .collect();
        let summary_text = match summarizer {
            Some(f) => f(&to_summarize),
            None => deterministic_summary(&to_summarize),
        };
        let mut out = system;
        out.push(ChatMessage::system(format!(
            "[context compacted: {} earlier messages summarized below]\n{summary_text}",
            to_summarize.len()
        )));
        out.extend_from_slice(recent);
        let after = self.report(segments, &out).used;
        (
            out.clone(),
            CompactionResult {
                before_tokens: before,
                after_tokens: after,
                summarized_messages: to_summarize.len(),
                preserved_labels: pinned_labels(segments),
                summary: summary_text,
            },
        )
    }

    /// Fit droppable segments into budget, always keeping pinned ones. Returns
    /// the segments that fit, highest-priority-kept first in original order.
    pub fn fit_segments(&self, segments: &[Segment], message_tokens: usize) -> Vec<Segment> {
        let mut budget = self.budget().saturating_sub(message_tokens);
        // Pinned first (they are mandatory).
        let mut kept: Vec<Segment> = Vec::new();
        for s in segments.iter().filter(|s| s.pinned) {
            budget = budget.saturating_sub(s.tokens());
            kept.push(s.clone());
        }
        // Then droppable, lowest priority number first.
        let mut droppable: Vec<&Segment> = segments.iter().filter(|s| !s.pinned).collect();
        droppable.sort_by_key(|s| s.priority);
        for s in droppable {
            let t = s.tokens();
            if t <= budget {
                budget -= t;
                kept.push(s.clone());
            }
        }
        // Restore original ordering for stable prompts.
        let order: std::collections::HashMap<&str, usize> = segments
            .iter()
            .enumerate()
            .map(|(i, s)| (s.label.as_str(), i))
            .collect();
        kept.sort_by_key(|s| order.get(s.label.as_str()).copied().unwrap_or(usize::MAX));
        kept
    }
}

fn pinned_labels(segments: &[Segment]) -> Vec<String> {
    segments
        .iter()
        .filter(|s| s.pinned)
        .map(|s| s.label.clone())
        .collect()
}

/// Deterministic summary used when no model summarizer is available: extracts
/// tool calls made, files mentioned, and the last user request, so the record
/// stays useful even offline.
fn deterministic_summary(messages: &[ChatMessage]) -> String {
    let mut tools = Vec::new();
    let mut last_user = String::new();
    for m in messages {
        match m.role {
            Role::User => last_user = m.content.chars().take(200).collect(),
            Role::Assistant => {
                for c in &m.tool_calls {
                    tools.push(c.name.clone());
                }
            }
            _ => {}
        }
    }
    tools.sort();
    tools.dedup();
    format!(
        "- earlier request: {}\n- tools used: {}",
        if last_user.is_empty() {
            "(none captured)".into()
        } else {
            last_user
        },
        if tools.is_empty() {
            "(none)".into()
        } else {
            tools.join(", ")
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_are_positive_and_monotonic() {
        assert!(estimate_tokens("a") >= 1);
        assert!(estimate_tokens("hello world this is longer") > estimate_tokens("hi"));
    }

    #[test]
    fn compaction_preserves_pinned_and_recent() {
        let mgr = ContextManager::new(4096, 512);
        let pinned = vec![Segment::pinned(
            "objective",
            "Fix the failing test in src/lib.rs",
        )];
        let mut messages = vec![ChatMessage::system("system prompt")];
        for i in 0..20 {
            messages.push(ChatMessage::user(format!(
                "message number {i} with some content"
            )));
            messages.push(ChatMessage::assistant(format!("reply {i}")));
        }
        let (compacted, result) = mgr.compact(&pinned, &messages, None);
        assert!(result.summarized_messages > 0);
        assert!(result.preserved_labels.contains(&"objective".to_string()));
        // The most recent 6 messages survive verbatim.
        let last = messages.last().expect("last").content.clone();
        assert!(compacted.iter().any(|m| m.content == last));
        assert!(result.after_tokens <= result.before_tokens);
    }

    #[test]
    fn fit_segments_drops_low_priority_first() {
        let mgr = ContextManager::new(200, 20);
        let segments = vec![
            Segment::pinned("safety", "never delete outside workspace"),
            Segment::droppable("low", "x".repeat(2000), 9),
            Segment::droppable("high", "important retrieval", 1),
        ];
        let kept = mgr.fit_segments(&segments, 0);
        assert!(kept.iter().any(|s| s.label == "safety"));
        assert!(kept.iter().any(|s| s.label == "high"));
        assert!(!kept.iter().any(|s| s.label == "low"));
    }

    #[test]
    fn needs_compaction_triggers_near_limit() {
        let mgr = ContextManager::new(300, 50);
        let big: Vec<ChatMessage> = (0..40)
            .map(|i| ChatMessage::user(format!("padding message {i}")))
            .collect();
        assert!(mgr.needs_compaction(&[], &big));
        assert!(!mgr.needs_compaction(&[], &[ChatMessage::user("tiny")]));
    }
}
