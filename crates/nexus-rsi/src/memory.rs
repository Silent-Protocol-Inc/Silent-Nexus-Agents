//! Memory curation.
//!
//! RSI may propose durable lessons, but a lesson is a *candidate* until evidence
//! promotes it — and legacy memory written before RSI existed is treated as
//! unverified, never silently reinterpreted as established truth. This module is
//! pure decision logic over [`MemoryRecord`]; it stores nothing itself, so it can
//! be reused by the curator that owns the `harness_memories` table and by tests.

use nexus_core::harness::{MemoryRecord, MemorySourceType, MemoryStatus};

/// Whether a memory's provenance is strong enough to count as verified, or must
/// remain unverified until corroborating evidence exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationState {
    /// Corroborated by the user or by evidence — safe to rely on.
    Verified,
    /// Agent- or tool-derived and not yet corroborated. Usable as a hint, never
    /// as ground truth for a decision.
    Unverified,
}

/// Pure curation decisions. Callers apply them against the memory store.
pub struct MemoryCurator;

impl MemoryCurator {
    /// Provenance-based verification: only the user's own words (or an explicit
    /// confirmation) start life verified. Everything the agent or tools inferred
    /// is unverified until evidence corroborates it — the "unverified ≠ truth"
    /// governance rule.
    pub fn verification_state(record: &MemoryRecord) -> VerificationState {
        match record.source_type {
            MemorySourceType::UserExplicit | MemorySourceType::UserConfirmed => {
                VerificationState::Verified
            }
            MemorySourceType::ToolObservation
            | MemorySourceType::TaskResult
            | MemorySourceType::AgentSummary
            | MemorySourceType::Imported => VerificationState::Unverified,
        }
    }

    /// A memory RSI just extracted must enter as a *candidate*, never active —
    /// the generating model does not get to make its own lesson authoritative.
    pub fn as_candidate(mut record: MemoryRecord) -> MemoryRecord {
        record.status = MemoryStatus::Candidate;
        record
    }

    /// Legacy backfill: on upgrade, an already-active memory with weak provenance
    /// should be surfaced for revalidation rather than trusted outright. Returns
    /// `true` when the record must be treated as unverified during migration.
    pub fn needs_revalidation(record: &MemoryRecord) -> bool {
        record.status == MemoryStatus::Active
            && Self::verification_state(record) == VerificationState::Unverified
    }

    /// Two memories are duplicates when they share a scope and say the same thing.
    /// Comparison is on normalised content (and summary), so trivial whitespace or
    /// case differences do not create redundant rows.
    pub fn is_duplicate(a: &MemoryRecord, b: &MemoryRecord) -> bool {
        if a.scope != b.scope {
            return false;
        }
        normalise(&a.content) == normalise(&b.content)
            || token_overlap(&a.content, &b.content) >= 0.9
    }

    /// Best-effort contradiction detection: same scope and same subject, but one
    /// asserts and the other negates it. This is a heuristic flag for human/WARP
    /// review, not a proof — it never deletes a memory on its own.
    pub fn contradicts(a: &MemoryRecord, b: &MemoryRecord) -> bool {
        if a.scope != b.scope {
            return false;
        }
        let (na, nb) = (normalise(&a.content), normalise(&b.content));
        if na == nb {
            return false;
        }
        // Opposite polarity about the same subject: the non-negation content
        // tokens overlap heavily, but exactly one statement is negated.
        let polarity_differs = is_negated(&na) != is_negated(&nb);
        polarity_differs && content_overlap(&na, &nb) >= 0.6
    }
}

fn normalise(text: &str) -> String {
    text.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(['.', '!', '?', ';', ':'])
        .to_string()
}

fn tokens(text: &str) -> Vec<String> {
    normalise(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Jaccard token overlap in [0.0, 1.0].
fn token_overlap(a: &str, b: &str) -> f64 {
    use std::collections::BTreeSet;
    let sa: BTreeSet<String> = tokens(a).into_iter().collect();
    let sb: BTreeSet<String> = tokens(b).into_iter().collect();
    if sa.is_empty() && sb.is_empty() {
        return 1.0;
    }
    let inter = sa.intersection(&sb).count() as f64;
    let union = sa.union(&sb).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

const NEGATIONS: [&str; 6] = ["not", "no", "never", "don't", "dont", "avoid"];
/// Low-content filler that should not sway subject comparison.
const FILLER: [&str; 6] = ["do", "does", "please", "the", "a", "an"];

fn is_negated(normalised: &str) -> bool {
    normalised
        .split_whitespace()
        .any(|w| NEGATIONS.contains(&w))
}

/// Content tokens: drop negations and filler so two statements are compared on
/// their subject, not their polarity or grammar.
fn content_tokens(normalised: &str) -> std::collections::BTreeSet<String> {
    normalised
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .filter(|t| !NEGATIONS.contains(t) && !FILLER.contains(t))
        .map(str::to_string)
        .collect()
}

/// Jaccard overlap of the two statements' content tokens.
fn content_overlap(a: &str, b: &str) -> f64 {
    let sa = content_tokens(a);
    let sb = content_tokens(b);
    if sa.is_empty() && sb.is_empty() {
        return 1.0;
    }
    let inter = sa.intersection(&sb).count() as f64;
    let union = sa.union(&sb).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::harness::{MemoryScope, MemoryType};

    fn mem(content: &str, source: MemorySourceType, scope: MemoryScope) -> MemoryRecord {
        MemoryRecord::new(MemoryType::Semantic, scope, content, source).expect("memory")
    }

    #[test]
    fn only_user_provenance_is_verified() {
        let scope = MemoryScope::global();
        assert_eq!(
            MemoryCurator::verification_state(&mem(
                "x",
                MemorySourceType::UserExplicit,
                scope.clone()
            )),
            VerificationState::Verified
        );
        assert_eq!(
            MemoryCurator::verification_state(&mem("x", MemorySourceType::AgentSummary, scope)),
            VerificationState::Unverified
        );
    }

    #[test]
    fn rsi_memories_enter_as_candidates() {
        let mut record = mem(
            "lesson",
            MemorySourceType::TaskResult,
            MemoryScope::global(),
        );
        record.status = MemoryStatus::Active;
        let curated = MemoryCurator::as_candidate(record);
        assert_eq!(curated.status, MemoryStatus::Candidate);
    }

    #[test]
    fn active_weak_provenance_memory_needs_revalidation_on_upgrade() {
        let mut agent = mem(
            "build with cargo",
            MemorySourceType::AgentSummary,
            MemoryScope::global(),
        );
        agent.status = MemoryStatus::Active;
        assert!(MemoryCurator::needs_revalidation(&agent));

        let mut user = mem(
            "build with cargo",
            MemorySourceType::UserExplicit,
            MemoryScope::global(),
        );
        user.status = MemoryStatus::Active;
        assert!(!MemoryCurator::needs_revalidation(&user));
    }

    #[test]
    fn duplicates_ignore_case_and_whitespace_but_respect_scope() {
        let a = mem(
            "Use   cargo BUILD.",
            MemorySourceType::TaskResult,
            MemoryScope::global(),
        );
        let b = mem(
            "use cargo build",
            MemorySourceType::TaskResult,
            MemoryScope::global(),
        );
        assert!(MemoryCurator::is_duplicate(&a, &b));

        let scoped = mem(
            "use cargo build",
            MemorySourceType::TaskResult,
            MemoryScope::profile("p1"),
        );
        assert!(
            !MemoryCurator::is_duplicate(&a, &scoped),
            "different scope is not a duplicate"
        );
    }

    #[test]
    fn contradiction_flags_opposite_polarity_same_subject() {
        let yes = mem(
            "run the tests",
            MemorySourceType::TaskResult,
            MemoryScope::global(),
        );
        let no = mem(
            "do not run the tests",
            MemorySourceType::TaskResult,
            MemoryScope::global(),
        );
        assert!(MemoryCurator::contradicts(&yes, &no));

        let unrelated = mem(
            "deploy the app",
            MemorySourceType::TaskResult,
            MemoryScope::global(),
        );
        assert!(!MemoryCurator::contradicts(&yes, &unrelated));
    }
}
