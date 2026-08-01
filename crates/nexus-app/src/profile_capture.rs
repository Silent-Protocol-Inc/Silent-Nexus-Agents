//! Deterministic detection of durable facts about the operator.
//!
//! What belongs here is narrow on purpose. A profile is read into the prompt of
//! every later turn, so a wrong entry is not a wrong answer once — it is a wrong
//! premise indefinitely, and the operator has no reason to suspect it. That
//! asymmetry sets the bar: a pattern earns its place by being hard to trigger by
//! accident, not by catching as much as possible.
//!
//! So every candidate here comes from a literal marker the operator wrote
//! (`my name is`, `my timezone is`, `I prefer`), never from inference about what
//! they seem to be like. Phrasings the markers miss are not lost: the agent
//! carries `profile.add_fact` and can record a fact it recognised during its own
//! turn, where the decision is visible in the transcript and attributable. That
//! is a better place for judgement than a silent pass over every message.
//!
//! Three things are refused outright, before any pattern runs:
//!
//! * anything that reads as a secret — credentials do not become less dangerous
//!   for being stored under a friendly key;
//! * anything scoped to right now — "I'm at a café", "I'm tired today" — which
//!   is true when written and misleading a week later;
//! * anything about someone else, or quoted from somewhere else.
//!
//! Facts in a sensitive category are detected but never stored live: they become
//! candidates for a human to approve, because consent to mention something in
//! conversation is not consent to keep it.

use nexus_core::redact::Redactor;

/// A durable fact detected in one operator message.
#[derive(Debug, Clone, PartialEq)]
pub struct FactCandidate {
    /// Canonical fact key, e.g. `identity.name`, `preferences.communication_style`.
    pub key: &'static str,
    pub value: String,
    /// Whether the operator stated this outright. Only explicit, non-sensitive
    /// facts are stored live; everything else waits for review.
    pub explicit: bool,
    /// `normal` or `sensitive`; drives the review requirement.
    pub sensitivity: &'static str,
    /// Short, safe explanation of why this was captured. Shown in `/profile`,
    /// so it never contains the raw message.
    pub reason: &'static str,
}

impl FactCandidate {
    fn normal(key: &'static str, value: String, reason: &'static str) -> Self {
        Self {
            key,
            value,
            explicit: true,
            sensitivity: "normal",
            reason,
        }
    }
}

/// Longest value any pattern will accept. A profile fact is an attribute, not a
/// paragraph; past this length the match is almost certainly a sentence that
/// happened to start with a marker.
const MAX_VALUE_CHARS: usize = 96;

/// Words that make a statement true now and misleading later. A fact carried
/// into every future turn must not be one of these.
const TEMPORAL: &[&str] = &[
    "today",
    "tonight",
    "right now",
    "at the moment",
    "currently",
    "this week",
    "this morning",
    "this afternoon",
    "for now",
    "temporarily",
    "just for this",
    "at a ",
    "at the ",
];

/// Markers that the surrounding text is being quoted, translated, or used as an
/// example — someone else's words, or nobody's.
const REPORTED: &[&str] = &[
    "translate",
    "for example",
    "e.g.",
    "such as",
    "sample",
    "example:",
    "quote",
    "it says",
    "the docs say",
    "the error says",
];

/// Statements about a third party. The operator talking about a colleague must
/// not end up as a fact about the operator.
const THIRD_PARTY: &[&str] = &[
    "his name",
    "her name",
    "their name",
    "his timezone",
    "her timezone",
    "their timezone",
    "he prefers",
    "she prefers",
    "they prefer",
    "he works",
    "she works",
    "they work",
];

/// Vocabulary that means "this is a credential", checked independently of the
/// redactor. The redactor recognises the shapes secrets usually take; this
/// catches the ones announced in words but written in a form it would not flag.
const SECRET_WORDS: &[&str] = &[
    "password",
    "passphrase",
    "api key",
    "api-key",
    "apikey",
    "secret key",
    "access token",
    "auth token",
    "bearer",
    "private key",
    "ssh key",
    "session cookie",
    "auth cookie",
    "credit card",
    "card number",
    "cvv",
    "recovery code",
    "backup code",
    "seed phrase",
    "mnemonic",
    "2fa code",
    "otp",
];

/// Categories where storing a true fact can still harm the person it is about.
/// Detected, never stored live.
const SENSITIVE_WORDS: &[&str] = &[
    "diagnos",
    "medication",
    "disability",
    "therapy",
    "depress",
    "religio",
    "muslim",
    "christian",
    "jewish",
    "hindu",
    "buddhist",
    "atheist",
    "ethnic",
    "political",
    "vote",
    "gay",
    "lesbian",
    "bisexual",
    "transgender",
    "salary",
    "bank account",
    "criminal",
    "arrested",
];

/// Every durable fact the message states about the operator.
///
/// `redactor` is consulted per value rather than per message: a message may
/// legitimately mention a token in one clause and the operator's timezone in
/// another, and refusing the whole message would quietly drop the harmless half.
pub fn detect(text: &str, redactor: &Redactor) -> Vec<FactCandidate> {
    let trimmed = text.trim();
    if trimmed.is_empty() || contains_any(&trimmed.to_ascii_lowercase(), REPORTED) {
        return Vec::new();
    }
    let lower = trimmed.to_ascii_lowercase();

    let mut found: Vec<FactCandidate> = Vec::new();
    for candidate in [
        occupation(trimmed, &lower),
        timezone(trimmed, &lower),
        language(trimmed, &lower),
        communication_style(trimmed, &lower),
        technical_stack(trimmed, &lower),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(candidate) = admit(candidate, redactor) {
            // One key per message, and never a broader restatement of
            // something already captured: "please always reply in Indonesian"
            // matches both the language marker and the preference marker, and
            // recording it twice would put the same instruction on the card
            // under two names.
            let redundant = found.iter().any(|existing| {
                existing.key == candidate.key
                    || candidate
                        .value
                        .to_ascii_lowercase()
                        .contains(&existing.value.to_ascii_lowercase())
            });
            if !redundant {
                found.push(candidate);
            }
        }
    }
    found
}

/// The last gate every candidate passes, whatever produced it.
///
/// Kept separate from the patterns so a new pattern cannot be written that
/// forgets one of these — the checks are not optional and not per-pattern.
fn admit(mut candidate: FactCandidate, redactor: &Redactor) -> Option<FactCandidate> {
    let value = candidate.value.trim();
    if value.is_empty() || value.chars().count() > MAX_VALUE_CHARS {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    if contains_any(&lower, SECRET_WORDS) || redactor.redact(value) != value {
        return None;
    }
    if contains_any(&lower, TEMPORAL) || contains_any(&lower, THIRD_PARTY) {
        return None;
    }
    if contains_any(&lower, SENSITIVE_WORDS) {
        candidate.sensitivity = "sensitive";
    }
    candidate.value = value.to_string();
    Some(candidate)
}

/// Whether the message states a durable fact but is disqualified by a gate.
///
/// Used to tell the operator that something was recognised and deliberately not
/// kept, rather than letting a refusal look like the feature not working.
pub fn refused_secret(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    contains_any(&lower, SECRET_WORDS)
        && ["my ", "i use ", "i have ", "here is ", "here's "]
            .iter()
            .any(|marker| lower.contains(marker))
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn occupation(orig: &str, lower: &str) -> Option<FactCandidate> {
    for marker in [
        "i work as ",
        "i am a ",
        "i'm a ",
        "my job is ",
        "my role is ",
    ] {
        if let Some(rest) = crate::control_plane::after_marker(orig, lower, marker) {
            // "I work as a X" and "I am a X" name the same job; the article is
            // part of the phrasing, not of the answer.
            let value = clause(rest)
                .trim_start_matches("a ")
                .trim_start_matches("an ")
                .trim_start_matches("the ")
                .trim();
            // "I'm a bit stuck" is not a job. Requiring a noun-ish tail keeps
            // the weakest marker ("I'm a") from swallowing ordinary sentences.
            if value.split_whitespace().count() <= 6 && !value.is_empty() {
                return Some(FactCandidate::normal(
                    "identity.occupation",
                    value.to_string(),
                    "stated occupation",
                ));
            }
        }
    }
    None
}

fn timezone(orig: &str, lower: &str) -> Option<FactCandidate> {
    for marker in ["my timezone is ", "my time zone is ", "i'm in timezone "] {
        if let Some(rest) = crate::control_plane::after_marker(orig, lower, marker) {
            let value = clause(rest);
            if !value.is_empty() && value.split_whitespace().count() <= 3 {
                return Some(FactCandidate::normal(
                    "identity.timezone",
                    value.to_string(),
                    "stated timezone",
                ));
            }
        }
    }
    None
}

fn language(orig: &str, lower: &str) -> Option<FactCandidate> {
    for marker in [
        "reply in ",
        "respond in ",
        "answer in ",
        "speak to me in ",
        "write to me in ",
    ] {
        if let Some(rest) = crate::control_plane::after_marker(orig, lower, marker) {
            let value = clause(rest);
            // One or two words: a language name. More than that is a sentence
            // about how to reply, which is a style preference, not a language.
            if !value.is_empty() && value.split_whitespace().count() <= 2 {
                return Some(FactCandidate::normal(
                    "identity.language",
                    value.to_string(),
                    "stated language preference",
                ));
            }
        }
    }
    None
}

/// [`after_marker`](crate::control_plane::after_marker) restricted to the head
/// of the message, for markers too common to trust mid-sentence.
fn at_start<'a>(orig: &'a str, lower: &str, marker: &str) -> Option<&'a str> {
    debug_assert!(marker.is_ascii(), "byte offsets must agree with `orig`");
    lower.starts_with(marker).then(|| &orig[marker.len()..])
}

fn communication_style(orig: &str, lower: &str) -> Option<FactCandidate> {
    for marker in [
        "i prefer ",
        "i'd prefer ",
        "please always ",
        "always ",
        "by default ",
    ] {
        if let Some(rest) = crate::control_plane::after_marker(orig, lower, marker) {
            let value = clause(rest);
            if !value.is_empty() && value.split_whitespace().count() <= 12 {
                return Some(FactCandidate::normal(
                    "preferences.communication_style",
                    value.to_string(),
                    "stated working preference",
                ));
            }
        }
    }
    // The bare imperative — "Prefer concise summaries", no "I" in front of it.
    // Anchored to the head of the message: matched anywhere it would also catch
    // questions put *to* the agent ("would you prefer tabs or spaces?"), which
    // state nothing about the operator.
    if let Some(rest) = at_start(orig, lower, "prefer ") {
        let value = clause(rest);
        if !value.is_empty() && value.split_whitespace().count() <= 12 {
            return Some(FactCandidate::normal(
                "preferences.communication_style",
                value.to_string(),
                "stated working preference",
            ));
        }
    }
    // Hedged forms. "I usually" is a description of a habit, not an
    // instruction, so it is recorded as a candidate for the operator to
    // confirm rather than acted on as though they had asked for it.
    // "You seem to prefer" is the agent's own reading played back, which is
    // the weakest evidence of all and never commits on its own.
    for marker in [
        "i usually ",
        "i tend to ",
        "i normally ",
        "you seem to prefer ",
    ] {
        if let Some(rest) = crate::control_plane::after_marker(orig, lower, marker) {
            let value = clause(rest);
            if !value.is_empty() && value.split_whitespace().count() <= 12 {
                return Some(FactCandidate {
                    key: "preferences.communication_style",
                    value: value.to_string(),
                    explicit: false,
                    sensitivity: "normal",
                    reason: "habit described in passing",
                });
            }
        }
    }
    None
}

fn technical_stack(orig: &str, lower: &str) -> Option<FactCandidate> {
    for marker in [
        "i use ",
        "we use ",
        "we're on ",
        "we are on ",
        "my stack is ",
    ] {
        if let Some(rest) = crate::control_plane::after_marker(orig, lower, marker) {
            let value = clause(rest);
            if !value.is_empty() && value.split_whitespace().count() <= 6 {
                return Some(FactCandidate::normal(
                    "preferences.technical_stack",
                    value.to_string(),
                    "stated tooling",
                ));
            }
        }
    }
    None
}

/// The text up to the next clause boundary — a fact is one clause, and running
/// past the boundary is how a marker swallows the rest of a paragraph.
fn clause(rest: &str) -> &str {
    let end = rest
        .find(['.', '!', '?', ',', ';', ':', '\n'])
        .unwrap_or(rest.len());
    rest[..end].trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect_keys(text: &str) -> Vec<(&'static str, String)> {
        let redactor = Redactor::new();
        detect(text, &redactor)
            .into_iter()
            .map(|candidate| (candidate.key, candidate.value))
            .collect()
    }

    #[test]
    fn durable_statements_about_the_operator_are_captured() {
        assert_eq!(
            detect_keys("I work as a customer success specialist."),
            vec![(
                "identity.occupation",
                "customer success specialist".to_string()
            )]
        );
        assert_eq!(
            detect_keys("My timezone is GMT+7"),
            vec![("identity.timezone", "GMT+7".to_string())]
        );
        assert_eq!(
            detect_keys("Please always reply in Indonesian"),
            vec![("identity.language", "Indonesian".to_string())]
        );
        assert_eq!(
            detect_keys("I prefer concise answers"),
            vec![(
                "preferences.communication_style",
                "concise answers".to_string()
            )]
        );
        assert_eq!(
            detect_keys("I use Discordeno v21 for this bot"),
            vec![(
                "preferences.technical_stack",
                "Discordeno v21 for this bot".to_string()
            )]
        );
    }

    /// True when written, misleading a week later. The cost of storing one of
    /// these is not a wrong answer once but a wrong premise indefinitely.
    #[test]
    fn statements_about_right_now_are_not_durable_facts() {
        for text in [
            "I'm tired today",
            "I'm at a café right now",
            "I prefer tea this morning",
            "I use the staging box for now",
        ] {
            assert!(detect_keys(text).is_empty(), "captured `{text}`");
        }
    }

    #[test]
    fn facts_about_other_people_are_not_assigned_to_the_operator() {
        for text in [
            "his name is Erpan and he prefers long answers",
            "their timezone is GMT+2",
        ] {
            assert!(detect_keys(text).is_empty(), "captured `{text}`");
        }
    }

    /// Text being quoted, translated, or given as an example is nobody's
    /// statement about themselves.
    #[test]
    fn quoted_and_illustrative_text_is_not_captured() {
        for text in [
            "translate this: my timezone is GMT+7",
            "for example, I prefer concise answers",
            "the docs say I use Discordeno",
        ] {
            assert!(detect_keys(text).is_empty(), "captured `{text}`");
        }
    }

    /// Credentials do not become less dangerous for being stored under a
    /// friendly key, so they are refused before anything else runs.
    #[test]
    fn credentials_are_never_turned_into_a_profile_fact() {
        for text in [
            "I use password hunter2 for the staging box",
            "my timezone is my api key sk-abc123",
            "I prefer the recovery code method 8471-2213",
        ] {
            assert!(detect_keys(text).is_empty(), "captured `{text}`");
        }
        assert!(refused_secret("my password is hunter2"));
        assert!(!refused_secret("I prefer concise answers"));
    }

    /// The redactor's own judgement is honoured even when no keyword appears.
    #[test]
    fn a_value_the_redactor_would_mask_is_refused() {
        let redactor = Redactor::new();
        redactor.register("hunter2");
        assert!(detect("I prefer hunter2", &redactor).is_empty());
    }

    /// Consent to mention something in conversation is not consent to keep it,
    /// so these are detected and held for a human rather than stored live.
    #[test]
    fn sensitive_categories_are_captured_but_held_for_review() {
        let redactor = Redactor::new();
        let found = detect("I prefer morning meetings due to my therapy", &redactor);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].sensitivity, "sensitive");
    }

    /// A marker at the head of a paragraph must not swallow the paragraph.
    #[test]
    fn a_marker_captures_one_clause_not_the_rest_of_the_message() {
        assert_eq!(
            detect_keys("I prefer concise answers, and please check the tests before you finish"),
            vec![(
                "preferences.communication_style",
                "concise answers".to_string()
            )]
        );
        assert!(detect_keys(&format!("I prefer {}", "x ".repeat(80))).is_empty());
    }

    /// Weak markers borrow ordinary English. "I'm a bit stuck" is not a job.
    #[test]
    fn a_weak_marker_does_not_swallow_an_ordinary_sentence() {
        assert!(detect_keys("I'm a bit stuck on why the parser keeps failing here").is_empty());
    }

    /// A described habit is not an instruction, so it waits to be confirmed
    /// rather than being acted on as though the operator had asked for it.
    #[test]
    fn a_hedged_habit_is_held_for_confirmation() {
        let redactor = Redactor::new();
        let found = detect("I usually work in short sessions", &redactor);
        assert_eq!(found.len(), 1);
        assert!(!found[0].explicit, "a habit is not an instruction");

        let stated = detect("I prefer short sessions", &redactor);
        assert!(stated[0].explicit);
    }

    /// `RsiStore::after_completed_turn` has always recognised these four
    /// wordings, but it writes to the legacy `profile_traits` table, which the
    /// prompt no longer reads. It sits below `nexus-app` and cannot call this
    /// module, so the guarantee is stated the other way round: every wording
    /// it learns from, this pass also learns from, into the canonical store.
    ///
    /// The legacy write stays where it is for backward compatibility.
    #[test]
    fn every_preference_the_rsi_pass_learns_from_reaches_the_canonical_store() {
        let redactor = Redactor::new();
        for (message, explicit) in [
            ("Always run the tests before you finish", true),
            ("Please always reply in Indonesian", true),
            ("Prefer concise validation summaries", true),
            ("By default keep answers short", true),
            ("I usually review the diff first", false),
            ("You seem to prefer smaller commits", false),
        ] {
            let found = detect(message, &redactor);
            assert_eq!(found.len(), 1, "not captured at all: {message:?}");
            assert_eq!(
                found[0].explicit, explicit,
                "wrong review disposition: {message:?}"
            );
        }
    }

    /// The bare imperative only reads as an instruction at the head of a
    /// message. Mid-sentence it is usually a question aimed at the agent.
    #[test]
    fn a_question_put_to_the_agent_is_not_an_operator_preference() {
        assert!(detect_keys("Would you prefer tabs or spaces here?").is_empty());
    }
}
