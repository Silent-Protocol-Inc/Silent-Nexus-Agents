//! Consent-gated Claude Code subscription authentication helpers.
//!
//! NEXUS never copies Claude credentials. The `claude-plan` provider invokes
//! the official CLI only after workspace-scoped consent. Logging out of NEXUS
//! revokes that consent; it does not silently sign the operator out of Claude
//! Code itself.

use nexus_core::{NexusError, Result};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ClaudeStatus {
    pub cli_installed: bool,
    pub consented: bool,
    /// `None` means NEXUS intentionally did not inspect auth because consent
    /// has not been granted.
    pub authenticated: Option<bool>,
    pub detail: String,
}

pub fn claude_binary() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|directory| directory.join("claude"))
            .find(|candidate| candidate.is_file())
    })
}

pub async fn status_with_consent(consented: bool) -> ClaudeStatus {
    let Some(binary) = claude_binary() else {
        return ClaudeStatus {
            cli_installed: false,
            consented,
            authenticated: Some(false),
            detail: "official Claude CLI is not installed".into(),
        };
    };
    if !consented {
        return ClaudeStatus {
            cli_installed: true,
            consented: false,
            authenticated: None,
            detail: "login not inspected — explicit consent required".into(),
        };
    }
    match tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(binary)
            .args(["auth", "status", "--json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    {
        Ok(Ok(output)) if output.status.success() => ClaudeStatus {
            cli_installed: true,
            consented: true,
            authenticated: Some(true),
            detail: auth_detail(&output.stdout),
        },
        Ok(Ok(output)) => ClaudeStatus {
            cli_installed: true,
            consented: true,
            authenticated: Some(false),
            detail: process_detail(&output.stderr, "Claude subscription login required"),
        },
        Ok(Err(error)) => ClaudeStatus {
            cli_installed: true,
            consented: true,
            authenticated: Some(false),
            detail: format!("could not inspect Claude login: {error}"),
        },
        Err(_) => ClaudeStatus {
            cli_installed: true,
            consented: true,
            authenticated: Some(false),
            detail: "Claude auth status timed out".into(),
        },
    }
}

/// Launch official Claude subscription login. Selecting this action is the
/// operator's explicit authorization for the external browser/login flow.
pub async fn login_subscription() -> Result<String> {
    let binary = claude_binary()
        .ok_or_else(|| NexusError::Config("the official `claude` CLI is not installed".into()))?;
    let output = tokio::process::Command::new(binary)
        .args(["auth", "login", "--claudeai"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| NexusError::Other(format!("launching Claude login: {error}")))?;
    if !output.status.success() {
        return Err(NexusError::Other(format!(
            "`claude auth login --claudeai` exited with {}: {}",
            output.status,
            process_detail(&output.stderr, "login was not completed")
        )));
    }
    Ok(process_detail(
        &output.stdout,
        "Claude subscription login completed",
    ))
}

fn auth_detail(bytes: &[u8]) -> String {
    let parsed = serde_json::from_slice::<serde_json::Value>(bytes).ok();
    let method = parsed
        .as_ref()
        .and_then(|value| {
            value
                .get("authMethod")
                .or_else(|| value.get("auth_method"))
                .or_else(|| value.get("method"))
        })
        .and_then(serde_json::Value::as_str);
    let email = parsed
        .as_ref()
        .and_then(|value| value.get("email"))
        .and_then(serde_json::Value::as_str);
    match (method, email) {
        (Some(method), Some(email)) => format!("authenticated via {method} as {email}"),
        (Some(method), None) => format!("authenticated via {method}"),
        _ => "Claude subscription login available".into(),
    }
}

fn process_detail(bytes: &[u8], fallback: &str) -> String {
    let text = nexus_core::sanitize::sanitize_terminal(String::from_utf8_lossy(bytes).trim());
    if text.is_empty() {
        fallback.into()
    } else {
        text.chars().take(400).collect()
    }
}
