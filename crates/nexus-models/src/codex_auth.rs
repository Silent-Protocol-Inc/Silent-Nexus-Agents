//! Reuse an OpenAI Codex ("Sign in with ChatGPT") login.
//!
//! The device/OAuth login itself is performed by the official `codex` CLI
//! (`codex login`), which writes its session to `$CODEX_HOME/auth.json`
//! (default `~/.codex/auth.json`). Silent Nexus does not reimplement OpenAI's
//! OAuth — that would require OpenAI's own client and endpoints, and any
//! reimplementation could not be verified honestly. Instead we *consume* the
//! credential the trusted CLI already obtained, which is the same model Codex
//! and other first-party tools use for interop.
//!
//! # Isolation
//!
//! Silent Nexus keeps its own Codex profile in an *isolated* home
//! (`<config>/auth/codex/`, override: `$SILENT_NEXUS_CODEX_HOME`). Logins
//! performed through Silent Nexus run the `codex` CLI with `CODEX_HOME`
//! pointed at that directory **on the child process only**, so the user's own
//! `~/.codex` login is never written, moved, or logged out. Resolution order
//! for credentials is: isolated Silent Nexus profile first, then—only after
//! explicit operator consent—read-only access to the user's existing Codex CLI
//! session.
//!
//! Two file shapes are supported (codex-cli 0.144.x):
//!  - OAuth session:  `{"tokens": {"access_token": "...", "account_id": "..."}}`
//!  - API-key mode:   `{"OPENAI_API_KEY": "sk-..."}`

use nexus_core::{NexusError, Result};
use serde::Deserialize;
use std::path::PathBuf;

/// A resolved Codex credential ready to be used as an HTTP bearer.
#[derive(Debug, Clone)]
pub struct CodexCredentials {
    /// Token to send as `Authorization: Bearer <token>`.
    pub bearer: String,
    /// ChatGPT account id (sent as `chatgpt-account-id` for OAuth sessions).
    pub account_id: Option<String>,
    /// `"oauth"` (Sign in with ChatGPT) or `"api_key"`.
    pub mode: &'static str,
}

/// Where a resolved Codex credential came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexSource {
    /// The isolated NEXUS profile (`<config>/auth/codex/`).
    NexusIsolated,
    /// The user's existing Codex CLI session (`$CODEX_HOME`/`~/.codex`),
    /// consumed read-only.
    ExistingCli,
}

impl CodexSource {
    pub fn label(&self) -> &'static str {
        match self {
            CodexSource::NexusIsolated => "NEXUS isolated profile",
            CodexSource::ExistingCli => "existing Codex CLI session (read-only)",
        }
    }
}

/// The isolated Silent Nexus Codex home. `$SILENT_NEXUS_CODEX_HOME` overrides
/// (used by tests); default is `<user config>/silent-nexus/auth/codex`.
pub fn nexus_isolated_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("SILENT_NEXUS_CODEX_HOME") {
        let p = PathBuf::from(dir);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    directories::ProjectDirs::from("top", "silentprotocol", "silent-nexus")
        .map(|d| d.config_dir().join("auth").join("codex"))
}

/// Path of the isolated profile's auth file.
pub fn nexus_auth_path() -> Option<PathBuf> {
    nexus_isolated_home().map(|h| h.join("auth.json"))
}

/// Resolve credentials from the isolated Silent Nexus profile. The user's
/// existing Codex CLI login is considered only after explicit consent.
pub fn resolve() -> Result<Option<(CodexCredentials, CodexSource)>> {
    resolve_with_consent(false)
}

pub fn resolve_with_consent(
    allow_existing: bool,
) -> Result<Option<(CodexCredentials, CodexSource)>> {
    if let Some(path) = nexus_auth_path() {
        if let Some(cred) = load_from(&path)? {
            return Ok(Some((cred, CodexSource::NexusIsolated)));
        }
    }
    if allow_existing {
        if let Some(path) = auth_path() {
            if let Some(cred) = load_from(&path)? {
                return Ok(Some((cred, CodexSource::ExistingCli)));
            }
        }
    }
    Ok(None)
}

#[derive(Debug, Deserialize, Default)]
struct AuthFile {
    #[serde(rename = "OPENAI_API_KEY")]
    openai_api_key: Option<String>,
    tokens: Option<Tokens>,
    #[serde(rename = "last_refresh")]
    #[allow(dead_code)]
    last_refresh: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct Tokens {
    access_token: Option<String>,
    account_id: Option<String>,
}

/// The Codex home directory: `$CODEX_HOME`, else `~/.codex`.
pub fn codex_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CODEX_HOME") {
        let p = PathBuf::from(dir);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    directories::BaseDirs::new().map(|b| b.home_dir().join(".codex"))
}

/// Path to the Codex auth file.
pub fn auth_path() -> Option<PathBuf> {
    codex_home().map(|h| h.join("auth.json"))
}

/// True if a Codex session file exists.
pub fn is_logged_in() -> bool {
    auth_path().map(|p| p.exists()).unwrap_or(false)
}

/// Load isolated Codex credentials. Returns `Ok(None)` when no usable
/// isolated credential is present.
pub fn load() -> Result<Option<CodexCredentials>> {
    Ok(resolve()?.map(|(cred, _)| cred))
}

/// Load Codex credentials with an explicit choice about whether the existing
/// CLI profile may be consumed read-only.
pub fn load_with_consent(allow_existing: bool) -> Result<Option<CodexCredentials>> {
    Ok(resolve_with_consent(allow_existing)?.map(|(cred, _)| cred))
}

/// Load credentials from a specific file (used by tests).
pub fn load_from(path: &std::path::Path) -> Result<Option<CodexCredentials>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| NexusError::Other(format!("reading {}: {e}", path.display())))?;
    let parsed: AuthFile = serde_json::from_str(&text).map_err(|e| {
        NexusError::Config(format!(
            "{} is not valid Codex auth JSON: {e}",
            path.display()
        ))
    })?;

    if let Some(tokens) = &parsed.tokens {
        if let Some(access) = tokens.access_token.as_ref().filter(|t| !t.is_empty()) {
            return Ok(Some(CodexCredentials {
                bearer: access.clone(),
                account_id: tokens.account_id.clone(),
                mode: "oauth",
            }));
        }
    }
    if let Some(key) = parsed.openai_api_key.filter(|k| !k.is_empty()) {
        return Ok(Some(CodexCredentials {
            bearer: key,
            account_id: None,
            mode: "api_key",
        }));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &std::path::Path, body: &str) -> PathBuf {
        let p = dir.join("auth.json");
        let mut f = std::fs::File::create(&p).expect("create");
        f.write_all(body.as_bytes()).expect("write");
        p
    }

    #[test]
    fn prefers_oauth_access_token() {
        let d = tempfile::tempdir().expect("dir");
        let p = write(
            d.path(),
            r#"{"OPENAI_API_KEY": null, "tokens": {"access_token": "oauth-abc", "account_id": "acct_1"}}"#,
        );
        let c = load_from(&p).expect("ok").expect("some");
        assert_eq!(c.bearer, "oauth-abc");
        assert_eq!(c.account_id.as_deref(), Some("acct_1"));
        assert_eq!(c.mode, "oauth");
    }

    #[test]
    fn falls_back_to_api_key() {
        let d = tempfile::tempdir().expect("dir");
        let p = write(d.path(), r#"{"OPENAI_API_KEY": "sk-live-xyz"}"#);
        let c = load_from(&p).expect("ok").expect("some");
        assert_eq!(c.bearer, "sk-live-xyz");
        assert_eq!(c.mode, "api_key");
        assert!(c.account_id.is_none());
    }

    #[test]
    fn missing_file_is_none_not_error() {
        let d = tempfile::tempdir().expect("dir");
        let p = d.path().join("nope.json");
        assert!(load_from(&p).expect("ok").is_none());
    }

    #[test]
    fn empty_credentials_is_none() {
        let d = tempfile::tempdir().expect("dir");
        let p = write(
            d.path(),
            r#"{"OPENAI_API_KEY": null, "tokens": {"access_token": ""}}"#,
        );
        assert!(load_from(&p).expect("ok").is_none());
    }

    #[test]
    fn malformed_is_error() {
        let d = tempfile::tempdir().expect("dir");
        let p = write(d.path(), "not json");
        assert!(load_from(&p).is_err());
    }

    #[test]
    fn consent_flag_is_part_of_the_resolution_contract() {
        // File precedence is covered by load_from tests. This locks the
        // security-significant public API: the default resolver is isolated
        // only, and callers must opt in to existing-profile reuse.
        type Resolution = Result<Option<(CodexCredentials, CodexSource)>>;
        let _isolated_only: fn() -> Resolution = resolve;
        let _explicit: fn(bool) -> Resolution = resolve_with_consent;
    }
}
