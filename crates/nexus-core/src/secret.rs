//! A string wrapper that cannot leak through Debug/Display/serialization.
//!
//! Every in-memory credential the harness holds (API keys, bearer tokens)
//! travels as a [`SecretString`], so accidental `{:?}`/`{}` formatting, JSON
//! dumps (`snx config show`), or log statements print `[redacted]` instead of
//! the value. Code that genuinely needs the secret calls [`SecretString::expose`]
//! at the single point of use (an HTTP header, a child-process stdin).

use crate::error::{NexusError, Result};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Whether some text carries something key-shaped.
///
/// The credential markers have to be matched as *credentials*, not as
/// substrings. `contains("sk-")` fires on `asterisk-wrapped`, `risk-averse`,
/// `task-specific`, and `desk-bound` — which is how a persona describing its
/// own prose format got refused as a secret. A prefix only counts when it
/// starts a token and is followed by enough key-shaped characters to be a key.
///
/// This lives here rather than beside one store because **every** durable write
/// has to answer the same question. When it lived privately in `harness`, a
/// second store persisted personas without ever asking it.
pub fn contains_likely_secret(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("\"authorization\":")
        || lower.contains("\"api_key\":")
        || has_key_after(&lower, "bearer ")
        || has_key_after(&lower, "sk-")
}

/// Refuse `text` if it carries a credential, either key-shaped or matching a
/// redaction pattern.
///
/// `record_kind` names the thing being stored, so the refusal says what was not
/// written. Callers must run this **before** any write they cannot undo.
pub fn refuse_if_secret(text: &str, record_kind: &str) -> Result<()> {
    if crate::redact::Redactor::new().redact(text) != text || contains_likely_secret(text) {
        return Err(NexusError::PolicyDenied(format!(
            "refusing to persist {record_kind} containing a likely secret"
        )));
    }
    Ok(())
}

/// The shortest run of key characters that distinguishes a real credential from
/// a hyphenated English word. Provider keys are far longer than this; ordinary
/// prose never reaches it.
const MIN_KEY_CHARS: usize = 16;

/// Whether `marker` appears at a token boundary followed by at least
/// [`MIN_KEY_CHARS`] key-shaped characters.
fn has_key_after(lower: &str, marker: &str) -> bool {
    let mut from = 0usize;
    while let Some(offset) = lower[from..].find(marker) {
        let at = from + offset;
        // `asterisk-` must not match `sk-`: the marker has to begin a token,
        // not land in the middle of a word.
        let starts_token = at == 0
            || !lower[..at]
                .chars()
                .next_back()
                .is_some_and(|previous| previous.is_ascii_alphanumeric());
        if starts_token {
            let key_chars = lower[at + marker.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .count();
            if key_chars >= MIN_KEY_CHARS {
                return true;
            }
        }
        from = at + marker.len();
    }
    false
}

/// A secret value with redacted formatting.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The wrapped secret. Call only at the point the value is actually used.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

impl std::fmt::Display for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// Serializes as `"[redacted]"` so a secret can never round-trip into a file
/// or terminal dump. Fields carrying secrets should also be `#[serde(skip)]`
/// where possible; this impl is defense in depth.
impl Serialize for SecretString {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("[redacted]")
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(String::deserialize(deserializer)?))
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        // `zeroize` uses compiler fences and volatile writes so this clearing
        // is not optimized away. It cannot erase copies made elsewhere, which
        // is why credential values remain wrapped and short-lived.
        self.0.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The credential scan must read prose as prose.
    ///
    /// `contains("sk-")` refused a persona that described its own format as
    /// "asterisk-wrapped action lines", reporting it as a likely secret. Every
    /// string below is ordinary English that a persona, memory, or profile note
    /// could legitimately contain.
    #[test]
    fn hyphenated_words_are_not_mistaken_for_credentials() {
        for prose in [
            "asterisk-wrapped action lines",
            "risk-averse planning",
            "task-specific instructions",
            "desk-bound work",
            "brisk-paced delivery",
            "a mask-like calm",
            "the bearer of the message",
            "bearer of bad news",
        ] {
            assert!(
                !contains_likely_secret(prose),
                "ordinary prose refused as a secret: {prose}"
            );
        }
    }

    /// And credentials must still be caught, including where they sit inside
    /// surrounding text.
    #[test]
    fn real_credentials_are_still_refused() {
        // Assembled at runtime rather than written out: a literal key here
        // would be a credential-shaped string in tracked source, which the
        // repository's own secret scan rightly refuses. Excluding this file
        // from that scan would be the wrong trade — it would stop guarding
        // everything else in it.
        let body = "abcdefghijklmnopqrstuvwxyz0123456789";
        for secret in [
            format!("{}-{body}", "sk"),
            format!("use {}-proj-{body} when calling", "sk"),
            format!("Authorization: {} {body}", "Bearer"),
            "{\"api_key\": \"anything\"}".to_string(),
            "{\"authorization\": \"x\"}".to_string(),
        ] {
            assert!(
                contains_likely_secret(&secret.to_ascii_lowercase()),
                "credential not refused: {secret}"
            );
        }
    }

    /// A key prefix at the very start of the payload still counts — the token
    /// boundary check must not require a preceding character to exist.
    #[test]
    fn a_credential_at_the_start_is_refused() {
        let payload = format!("{}-0123456789abcdefghijklmnop rest of the note", "sk");
        assert!(contains_likely_secret(&payload));
    }

    /// The refusal has to name what was not stored, so an operator can tell a
    /// credential rejection from any other policy denial.
    #[test]
    fn refusal_names_the_record_kind_and_prose_passes() {
        let body = "abcdefghijklmnopqrstuvwxyz0123456789";
        let error = refuse_if_secret(&format!("{}-{body}", "sk"), "persona")
            .expect_err("a credential must be refused");
        assert!(
            error.to_string().contains("persona"),
            "refusal does not say what was refused: {error}"
        );
        refuse_if_secret("asterisk-wrapped action lines", "persona")
            .expect("ordinary prose must be storable");
    }

    #[test]
    fn debug_and_display_are_redacted() {
        let s = SecretString::new("sk-super-secret");
        assert_eq!(format!("{s:?}"), "[redacted]");
        assert_eq!(format!("{s}"), "[redacted]");
    }

    #[test]
    fn serialization_is_redacted() {
        let s = SecretString::new("sk-super-secret");
        assert_eq!(
            serde_json::to_string(&s).expect("serialize"),
            "\"[redacted]\""
        );
    }

    #[test]
    fn expose_returns_value() {
        let s = SecretString::new("abc");
        assert_eq!(s.expose(), "abc");
    }
}
