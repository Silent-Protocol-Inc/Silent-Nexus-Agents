//! Secret redaction.
//!
//! Two complementary mechanisms:
//! 1. **Known-secret scrubbing** — values registered at startup (API keys from
//!    config/keychain, sensitive env vars) are replaced wherever they appear.
//! 2. **Pattern scrubbing** — well-known credential shapes (bearer tokens,
//!    private key blocks, cloud key formats) are masked even when the value
//!    was never registered.
//!
//! Redaction runs before text is logged, persisted, displayed, or sent to a
//! model provider other than the one that owns the key.

use regex::Regex;
use std::collections::HashSet;
use std::sync::RwLock;

const MASK: &str = "[REDACTED]";

/// Environment variable name fragments considered sensitive by default.
const SENSITIVE_ENV_FRAGMENTS: &[&str] = &[
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "API_KEY",
    "APIKEY",
    "PRIVATE_KEY",
    "AUTH",
    "CREDENTIAL",
    "SESSION_KEY",
    "ACCESS_KEY",
];

pub struct Redactor {
    known: RwLock<HashSet<String>>,
    patterns: Vec<Regex>,
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new()
    }
}

impl Redactor {
    pub fn new() -> Self {
        // Compile-time constant patterns; expect() is acceptable because the
        // literals are tested below.
        let raw = [
            // PEM / OpenSSH private key blocks (multiline)
            r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
            // Bearer / token headers
            r"(?i)(authorization\s*:\s*bearer\s+)[A-Za-z0-9._~+/=-]{8,}",
            // OpenAI-style keys
            r"\bsk-[A-Za-z0-9_-]{16,}\b",
            // AWS access key id + generic secret patterns
            r"\bAKIA[0-9A-Z]{16}\b",
            r"(?i)\baws_secret_access_key\s*[:=]\s*\S+",
            // GitHub tokens
            r"\bgh[pousr]_[A-Za-z0-9]{20,}\b",
            // Slack tokens
            r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b",
            // Generic KEY=VALUE where the key name looks sensitive
            r#"(?i)\b([A-Z0-9_]*(?:TOKEN|SECRET|PASSWORD|API_?KEY|CREDENTIAL)[A-Z0-9_]*)\s*[:=]\s*['"]?[^\s'"]{6,}['"]?"#,
            // Credentials embedded in URLs: scheme://user:pass@host
            r"(://[^/\s:@]+):([^@/\s]+)@",
        ];
        let patterns = raw
            .iter()
            .map(|p| Regex::new(p).expect("static redaction pattern must compile"))
            .collect();
        Self {
            known: RwLock::new(HashSet::new()),
            patterns,
        }
    }

    /// Register a literal secret value (e.g. an API key loaded from config).
    /// Short values are ignored to avoid masking common substrings.
    pub fn register(&self, value: &str) {
        if value.len() >= 6 {
            if let Ok(mut k) = self.known.write() {
                k.insert(value.to_string());
            }
        }
    }

    /// Register values of sensitive-looking variables from the current
    /// process environment.
    pub fn register_env(&self) {
        for (key, value) in std::env::vars() {
            let upper = key.to_uppercase();
            if SENSITIVE_ENV_FRAGMENTS.iter().any(|f| upper.contains(f)) {
                self.register(&value);
            }
        }
    }

    /// Redact all known secrets and credential-shaped patterns from `text`.
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        if let Ok(known) = self.known.read() {
            for secret in known.iter() {
                if out.contains(secret.as_str()) {
                    out = out.replace(secret.as_str(), MASK);
                }
            }
        }
        for (i, re) in self.patterns.iter().enumerate() {
            match i {
                // Pattern 1 keeps the "Authorization: Bearer " prefix.
                1 => out = re.replace_all(&out, format!("${{1}}{MASK}")).into_owned(),
                // Pattern 8 keeps scheme://, masks user:pass.
                8 => out = re.replace_all(&out, format!("${{1}}:{MASK}@")).into_owned(),
                _ => out = re.replace_all(&out, MASK).into_owned(),
            }
        }
        out
    }

    /// Return a byte boundary before which streamed model text can be
    /// sanitized and redacted without splitting a known secret or an
    /// unfinished private-key block. The caller keeps the remaining suffix
    /// and retries when more provider output arrives.
    ///
    /// This deliberately retains at least 64 bytes and ends on whitespace so
    /// token-shaped credentials (API keys, bearer tokens, credential URLs)
    /// are not displayed while they are still being assembled.
    pub fn safe_stream_prefix_len(&self, pending: &str, final_chunk: bool) -> usize {
        if final_chunk {
            return pending.len();
        }

        let known = self.known.read().ok();
        let longest_known = known
            .as_ref()
            .and_then(|values| values.iter().map(String::len).max())
            .unwrap_or(0);
        let holdback = longest_known.saturating_add(64).max(64);
        if pending.len() <= holdback {
            return 0;
        }

        let target = pending.len().saturating_sub(holdback);
        let mut cutoff = pending
            .char_indices()
            .take_while(|(index, _)| *index <= target)
            .filter_map(|(index, ch)| ch.is_whitespace().then_some(index + ch.len_utf8()))
            .last()
            .unwrap_or(0);
        if cutoff == 0 {
            return 0;
        }

        if let Some(values) = known.as_ref() {
            for secret in values.iter() {
                for (start, _) in pending.match_indices(secret) {
                    let end = start.saturating_add(secret.len());
                    if start < cutoff && cutoff < end {
                        cutoff = cutoff.min(start);
                    }
                }
            }
        }

        if let Some(begin) = pending.find("-----BEGIN") {
            let tail = &pending[begin..];
            if tail.contains("PRIVATE KEY-----") && !tail.contains("-----END") {
                cutoff = cutoff.min(begin);
            }
        }

        while cutoff > 0 && !pending.is_char_boundary(cutoff) {
            cutoff -= 1;
        }
        cutoff
    }

    /// True if `key` names an environment variable that must not be forwarded
    /// into sandboxes or logged.
    pub fn is_sensitive_env_key(key: &str) -> bool {
        let upper = key.to_uppercase();
        SENSITIVE_ENV_FRAGMENTS.iter().any(|f| upper.contains(f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_registered_secrets() {
        let r = Redactor::new();
        r.register("super-secret-value-123");
        let out = r.redact("the key is super-secret-value-123 ok");
        assert!(!out.contains("super-secret-value-123"));
        assert!(out.contains(MASK));
    }

    #[test]
    fn masks_openai_style_keys() {
        let r = Redactor::new();
        let out = r.redact("using sk-abcdefghijklmnop1234 for auth");
        assert!(!out.contains("sk-abcdefghijklmnop1234"));
    }

    #[test]
    fn masks_private_key_blocks() {
        let r = Redactor::new();
        let text = "-----BEGIN RSA PRIVATE KEY-----\nMIIabc\nxyz\n-----END RSA PRIVATE KEY-----";
        let out = r.redact(text);
        assert!(!out.contains("MIIabc"));
    }

    #[test]
    fn masks_bearer_headers_preserving_prefix() {
        let r = Redactor::new();
        let out = r.redact("Authorization: Bearer abc123def456ghi789");
        assert!(out.to_lowercase().contains("authorization: bearer"));
        assert!(!out.contains("abc123def456ghi789"));
    }

    #[test]
    fn streaming_prefix_never_splits_a_known_secret() {
        let r = Redactor::new();
        r.register("known secret phrase");
        let pending =
            "safe prefix words known secret phrase and enough trailing words to flush safely";
        let cutoff = r.safe_stream_prefix_len(pending, false);
        assert!(
            cutoff == 0
                || !pending[..cutoff].contains("known secret")
                || pending[..cutoff].contains("known secret phrase")
        );
    }

    #[test]
    fn streaming_prefix_holds_unfinished_private_key_blocks() {
        let r = Redactor::new();
        let pending = format!(
            "safe preface\n-----BEGIN RSA PRIVATE KEY-----\n{}\n",
            "A".repeat(256)
        );
        let cutoff = r.safe_stream_prefix_len(&pending, false);
        assert!(cutoff <= pending.find("-----BEGIN").expect("marker"));
        assert_eq!(r.safe_stream_prefix_len(&pending, true), pending.len());
    }

    #[test]
    fn masks_env_style_assignments() {
        let r = Redactor::new();
        let out = r.redact("export MY_API_KEY=abcdef123456");
        assert!(!out.contains("abcdef123456"));
    }

    #[test]
    fn masks_url_credentials() {
        let r = Redactor::new();
        let out = r.redact("fetch https://user:hunter2secret@example.com/x");
        assert!(!out.contains("hunter2secret"));
        assert!(out.contains("example.com"));
    }

    #[test]
    fn leaves_normal_text_alone() {
        let r = Redactor::new();
        let text = "fn main() { println!(\"hello\"); }";
        assert_eq!(r.redact(text), text);
    }
}
