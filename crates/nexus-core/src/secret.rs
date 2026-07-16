//! A string wrapper that cannot leak through Debug/Display/serialization.
//!
//! Every in-memory credential the harness holds (API keys, bearer tokens)
//! travels as a [`SecretString`], so accidental `{:?}`/`{}` formatting, JSON
//! dumps (`snx config show`), or log statements print `[redacted]` instead of
//! the value. Code that genuinely needs the secret calls [`SecretString::expose`]
//! at the single point of use (an HTTP header, a child-process stdin).

use serde::{Deserialize, Serialize};

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
        // Best-effort clear. Not a guarantee against copies made before drop,
        // but removes the common case of a secret lingering in freed memory.
        // In-place zeroing of the UTF-8 buffer with NUL bytes keeps the string
        // valid UTF-8, so this is the one justified `unsafe` in the crate.
        #[allow(unsafe_code)]
        unsafe {
            for b in self.0.as_bytes_mut() {
                *b = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
