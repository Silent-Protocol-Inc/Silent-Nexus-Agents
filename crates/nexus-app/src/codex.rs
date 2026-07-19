//! Isolated Codex authentication service.
//!
//! Every login NEXUS performs delegates to the official `codex` CLI
//! (verified: codex-cli 0.144.x supports `login --device-auth`,
//! `login --with-api-key`, and honors `CODEX_HOME`) and runs that CLI with
//! `CODEX_HOME` pointing at the isolated NEXUS profile — set on the
//! child process only, never exported globally. The user's own `~/.codex`
//! login is read at most (with consent) and never written, moved, or removed.

use nexus_core::{NexusError, Result, SecretString};
use nexus_models::codex_auth::{self, CodexSource};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, watch};

/// Auth state of one Codex profile (no secret material).
#[derive(Debug, Clone)]
pub struct CodexProfileInfo {
    /// `"oauth"` or `"api_key"`.
    pub mode: &'static str,
    pub account_id: Option<String>,
    pub auth_file: PathBuf,
}

/// Combined Codex auth status used by `/login`, `/status`, `snx auth status`.
#[derive(Debug, Clone)]
pub struct CodexStatus {
    /// The `codex` binary is on PATH.
    pub cli_installed: bool,
    /// The isolated NEXUS profile, when logged in.
    pub isolated: Option<CodexProfileInfo>,
    /// The user's existing Codex CLI session, when present (read-only).
    pub existing: Option<CodexProfileInfo>,
    /// Which source `auth = "codex"` models would use right now.
    pub active_source: Option<CodexSource>,
}

/// Events streamed to the UI during a device login. The TUI renders these in
/// a progress modal; the CLI prints them.
#[derive(Debug, Clone)]
pub enum DeviceLoginEvent {
    /// A (sanitized) line of codex CLI output worth showing.
    Info(String),
    /// The verification URL the operator must open.
    VerificationUrl(String),
    /// The one-time code to enter.
    UserCode(String),
    /// Login verified: the isolated profile now holds a session.
    Success {
        mode: &'static str,
        account_id: Option<String>,
    },
    /// Login failed with an actionable reason.
    Failed(String),
}

/// Locate `codex` on PATH (no shell involved).
pub fn codex_binary() -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join("codex");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn profile_info(path: &std::path::Path) -> Result<Option<CodexProfileInfo>> {
    Ok(codex_auth::load_from(path)?.map(|c| CodexProfileInfo {
        mode: c.mode,
        account_id: c.account_id,
        auth_file: path.to_path_buf(),
    }))
}

/// Gather the current Codex auth picture. Malformed auth files degrade to
/// "not logged in" here (status must never hard-fail), but are reported.
pub fn status() -> CodexStatus {
    status_with_consent(false)
}

pub fn status_with_consent(allow_existing: bool) -> CodexStatus {
    let isolated = codex_auth::nexus_auth_path().and_then(|p| profile_info(&p).ok().flatten());
    let existing = codex_auth::auth_path().and_then(|p| profile_info(&p).ok().flatten());
    let active_source = match (&isolated, &existing) {
        (Some(_), _) => Some(CodexSource::NexusIsolated),
        (None, Some(_)) if allow_existing => Some(CodexSource::ExistingCli),
        (None, None) => None,
        (None, Some(_)) => None,
    };
    CodexStatus {
        cli_installed: codex_binary().is_some(),
        isolated,
        existing,
        active_source,
    }
}

fn isolated_home() -> Result<PathBuf> {
    codex_auth::nexus_isolated_home()
        .ok_or_else(|| NexusError::Config("cannot determine the isolated Codex home".into()))
}

fn ensure_isolated_home() -> Result<PathBuf> {
    let home = isolated_home()?;
    std::fs::create_dir_all(&home)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(home)
}

fn require_codex() -> Result<PathBuf> {
    codex_binary().ok_or_else(|| {
        NexusError::Config(
            "the `codex` CLI is not installed. NEXUS delegates Codex login to the \
             official CLI rather than reimplementing OpenAI's OAuth. Install it (e.g. \
             `npm install -g @openai/codex`), then retry."
                .into(),
        )
    })
}

/// Run `codex login --device-auth` against the ISOLATED profile, streaming
/// progress events. Cancellation (`cancel` flipping to true) kills the child.
/// On success the isolated profile is verified by re-reading its auth file.
pub async fn device_login(
    events: mpsc::UnboundedSender<DeviceLoginEvent>,
    mut cancel: watch::Receiver<bool>,
) -> Result<()> {
    let codex = require_codex()?;
    let home = ensure_isolated_home()?;

    let mut child = tokio::process::Command::new(&codex)
        .args(["login", "--device-auth"])
        .env("CODEX_HOME", &home) // child process only; never exported
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            NexusError::Other(format!("failed to launch `codex login --device-auth`: {e}"))
        })?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (line_tx, mut line_rx) = mpsc::unbounded_channel::<String>();
    if let Some(out) = stdout {
        let tx = line_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(line);
            }
        });
    }
    if let Some(err) = stderr {
        let tx = line_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(line);
            }
        });
    }
    drop(line_tx);

    let outcome = loop {
        tokio::select! {
            maybe_line = line_rx.recv() => {
                match maybe_line {
                    Some(line) => forward_login_line(&events, &line),
                    None => {
                        // Output closed; wait for exit below.
                        break child.wait().await;
                    }
                }
            }
            status = child.wait() => break status,
            changed = cancel.changed() => {
                let cancelled = changed.is_ok() && *cancel.borrow();
                if cancelled || changed.is_err() {
                    let _ = child.kill().await;
                    let _ = events.send(DeviceLoginEvent::Failed("cancelled by operator".into()));
                    return Err(NexusError::Other("device login cancelled".into()));
                }
            }
        }
    };

    let status = outcome.map_err(|e| NexusError::Other(format!("waiting for codex login: {e}")))?;
    if !status.success() {
        let msg = format!(
            "`codex login --device-auth` exited with {status}. The device authorization \
             may have timed out or been denied."
        );
        let _ = events.send(DeviceLoginEvent::Failed(msg.clone()));
        return Err(NexusError::Other(msg));
    }

    // Verify: the isolated profile must now hold a usable session.
    match codex_auth::nexus_auth_path().and_then(|p| codex_auth::load_from(&p).ok().flatten()) {
        Some(cred) => {
            let _ = events.send(DeviceLoginEvent::Success {
                mode: cred.mode,
                account_id: cred.account_id.clone(),
            });
            Ok(())
        }
        None => {
            let msg =
                "codex reported success but the isolated profile holds no session".to_string();
            let _ = events.send(DeviceLoginEvent::Failed(msg.clone()));
            Err(NexusError::Other(msg))
        }
    }
}

/// Forward one codex output line, extracting the verification URL and the
/// one-time code so the UI can present them for copying.
fn forward_login_line(events: &mpsc::UnboundedSender<DeviceLoginEvent>, raw: &str) {
    let line = nexus_core::sanitize::sanitize_terminal(raw);
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    for word in trimmed.split_whitespace() {
        if word.starts_with("https://") || word.starts_with("http://") {
            let _ = events.send(DeviceLoginEvent::VerificationUrl(word.to_string()));
        }
    }
    if let Some(code) = extract_user_code(trimmed) {
        let _ = events.send(DeviceLoginEvent::UserCode(code));
    }
    let _ = events.send(DeviceLoginEvent::Info(trimmed.to_string()));
}

/// A device code looks like `XXXX-XXXX` (alphanumeric groups joined by `-`).
fn extract_user_code(line: &str) -> Option<String> {
    for token in line.split_whitespace() {
        let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
        let groups: Vec<&str> = token.split('-').collect();
        if groups.len() >= 2
            && groups.len() <= 4
            && groups.iter().all(|g| {
                (3..=6).contains(&g.len())
                    && g.chars().all(|c| c.is_ascii_alphanumeric())
                    && g.chars()
                        .any(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            })
            && !token.starts_with("http")
        {
            return Some(token.to_string());
        }
    }
    None
}

/// Log in to the ISOLATED profile with an API key (delegates to
/// `codex login --with-api-key`, key passed over stdin, never argv).
pub async fn login_with_api_key(key: &SecretString) -> Result<CodexProfileInfo> {
    if key.is_empty() {
        return Err(NexusError::Config("API key cannot be empty".into()));
    }
    let codex = require_codex()?;
    let home = ensure_isolated_home()?;
    let mut child = tokio::process::Command::new(&codex)
        .args(["login", "--with-api-key"])
        .env("CODEX_HOME", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            NexusError::Other(format!(
                "failed to launch `codex login --with-api-key`: {e}"
            ))
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(key.expose().as_bytes())
            .await
            .map_err(|e| NexusError::Other(format!("sending key to codex: {e}")))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| NexusError::Other(format!("finishing key input: {e}")))?;
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| NexusError::Other(format!("waiting for codex login: {e}")))?;
    if !output.status.success() {
        // stderr may mention the key? codex does not echo keys; still sanitize.
        let detail =
            nexus_core::sanitize::sanitize_terminal(String::from_utf8_lossy(&output.stderr).trim());
        return Err(NexusError::Other(format!(
            "`codex login --with-api-key` exited with {}: {detail}",
            output.status
        )));
    }
    verify_isolated()
}

/// Copy the user's existing Codex CLI session INTO the isolated profile.
/// Caller must have obtained explicit operator confirmation first. The
/// original file is read once and never modified.
pub fn import_existing() -> Result<CodexProfileInfo> {
    let source = codex_auth::auth_path()
        .filter(|p| p.exists())
        .ok_or_else(|| {
            NexusError::NotFound(
                "no existing Codex CLI session to import (run `codex login` first)".into(),
            )
        })?;
    // Validate before copying so we never import garbage.
    codex_auth::load_from(&source)?.ok_or_else(|| {
        NexusError::Config("the existing Codex auth file holds no usable session".into())
    })?;
    let home = ensure_isolated_home()?;
    let dest = home.join("auth.json");
    let body = std::fs::read_to_string(&source)?;
    nexus_core::atomic::atomic_write_private(&dest, body.as_bytes())?;
    verify_isolated()
}

/// Remove the isolated NEXUS session only. Never touches `~/.codex`.
pub fn logout_isolated() -> Result<bool> {
    let Some(path) = codex_auth::nexus_auth_path() else {
        return Ok(false);
    };
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&path)?;
    Ok(true)
}

fn verify_isolated() -> Result<CodexProfileInfo> {
    let path = codex_auth::nexus_auth_path()
        .ok_or_else(|| NexusError::Config("cannot determine the isolated Codex home".into()))?;
    profile_info(&path)?.ok_or_else(|| {
        NexusError::Other("login finished but the isolated profile holds no session".into())
    })
}

// --------------------------------------------------------- plan model list

/// One reasoning effort a plan model supports.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EffortInfo {
    pub effort: String,
    #[serde(default)]
    pub description: String,
}

/// One model available on the operator's ChatGPT plan, as reported by the
/// codex CLI's `model/list` (the same source the official CLI's picker uses).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlanModel {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub default_reasoning_effort: Option<String>,
    /// Reasoning efforts this model supports (`low` … `ultra`), in the order
    /// the provider reports them.
    #[serde(default)]
    pub reasoning_efforts: Vec<EffortInfo>,
    pub is_default: bool,
    #[serde(default)]
    pub context_window: Option<usize>,
    #[serde(default)]
    pub max_output_tokens: Option<usize>,
}

/// Run `codex app-server` against the ISOLATED profile and issue the given
/// JSON-RPC requests after `initialize`; returns the result value per id.
/// The user's own `~/.codex` is never touched.
async fn app_server_call(
    requests: &[(u64, &str, serde_json::Value)],
    timeout_secs: u64,
) -> Result<std::collections::BTreeMap<u64, serde_json::Value>> {
    let codex = require_codex()?;
    let home = ensure_isolated_home()?;
    if codex_auth::nexus_auth_path()
        .map(|p| !p.exists())
        .unwrap_or(true)
    {
        return Err(NexusError::Config(
            "this needs a NEXUS Codex login (the isolated profile) — run /connect \
             → Codex and use device login or import"
                .into(),
        ));
    }

    let mut child = tokio::process::Command::new(&codex)
        .arg("app-server")
        .env("CODEX_HOME", &home) // child process only; never exported
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| NexusError::Other(format!("failed to launch `codex app-server`: {e}")))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| NexusError::Other("codex app-server: no stdin".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| NexusError::Other("codex app-server: no stdout".into()))?;

    let want: Vec<u64> = requests.iter().map(|(id, _, _)| *id).collect();
    let run = async {
        use tokio::io::AsyncWriteExt;
        let mut payload = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "silent-nexus", "title": "NEXUS", "version": env!("CARGO_PKG_VERSION")}},
        })
        .to_string();
        for (id, method, params) in requests {
            payload.push('\n');
            payload.push_str(
                &serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
                    .to_string(),
            );
        }
        payload.push('\n');
        stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| NexusError::Other(format!("writing to codex app-server: {e}")))?;

        let mut results = std::collections::BTreeMap::new();
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let Some(id) = v
                .get("id")
                .and_then(|i| i.as_u64())
                .filter(|i| want.contains(i))
            else {
                continue;
            };
            if let Some(err) = v.get("error") {
                let method = requests
                    .iter()
                    .find(|(i, _, _)| *i == id)
                    .map(|(_, m, _)| *m)
                    .unwrap_or("request");
                let msg = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("failed");
                return Err(NexusError::Other(format!("codex {method}: {msg}")));
            }
            results.insert(
                id,
                v.get("result").cloned().unwrap_or(serde_json::Value::Null),
            );
            if results.len() == want.len() {
                return Ok(results);
            }
        }
        Err(NexusError::Other(
            "codex app-server closed before answering".into(),
        ))
    };

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), run)
        .await
        .map_err(|_| {
            NexusError::Other(format!("codex app-server timed out after {timeout_secs}s"))
        })?;
    let _ = child.kill().await;
    outcome
}

fn plan_models_cache_path() -> Option<PathBuf> {
    codex_auth::nexus_isolated_home().map(|h| h.join("plan_models.json"))
}

/// Plan models from the on-disk cache (written by [`list_plan_models`]), so
/// bootstrap can pick the account's default model without spawning anything.
pub fn cached_plan_models() -> Vec<PlanModel> {
    let Some(path) = plan_models_cache_path() else {
        return Vec::new();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// The account's default model id, from the cache.
pub fn cached_default_model() -> Option<String> {
    let models = cached_plan_models();
    models
        .iter()
        .find(|m| m.is_default)
        .or_else(|| models.first())
        .map(|m| m.id.clone())
}

/// Fetch the models available on the operator's plan by asking the codex CLI
/// (`codex app-server` → JSON-RPC `model/list`), run against the ISOLATED
/// profile only — the user's own `~/.codex` is never touched. Results are
/// cached next to the isolated profile for bootstrap.
pub async fn list_plan_models() -> Result<Vec<PlanModel>> {
    let results = app_server_call(&[(2, "model/list", serde_json::json!({}))], 20).await?;
    let entries = results
        .get(&2)
        .and_then(|r| r.get("data"))
        .and_then(|d| d.as_array())
        .cloned()
        .ok_or_else(|| NexusError::Other("codex model/list returned no model data".into()))?;
    let models: Vec<PlanModel> = entries
        .iter()
        .filter(|m| !m.get("hidden").and_then(|h| h.as_bool()).unwrap_or(false))
        .filter_map(|m| {
            Some(PlanModel {
                id: m.get("id")?.as_str()?.to_string(),
                display_name: m
                    .get("displayName")
                    .and_then(|d| d.as_str())
                    .unwrap_or_default()
                    .to_string(),
                description: m
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or_default()
                    .to_string(),
                default_reasoning_effort: m
                    .get("defaultReasoningEffort")
                    .and_then(|d| d.as_str())
                    .map(String::from),
                reasoning_efforts: m
                    .get("supportedReasoningEfforts")
                    .and_then(|e| e.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| {
                                Some(EffortInfo {
                                    effort: x.get("reasoningEffort")?.as_str()?.to_string(),
                                    description: x
                                        .get("description")
                                        .and_then(|d| d.as_str())
                                        .unwrap_or_default()
                                        .to_string(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                is_default: m
                    .get("isDefault")
                    .and_then(|d| d.as_bool())
                    .unwrap_or(false),
                context_window: model_usize(m, &["contextWindow", "context_window"]),
                max_output_tokens: model_usize(
                    m,
                    &[
                        "maxOutputTokens",
                        "max_output_tokens",
                        "maxCompletionTokens",
                    ],
                ),
            })
        })
        .collect();
    if models.is_empty() {
        return Err(NexusError::Other(
            "codex model/list returned an empty model list".into(),
        ));
    }
    if let Some(path) = plan_models_cache_path() {
        if let Ok(body) = serde_json::to_string_pretty(&models) {
            if let Some(parent) = path.parent() {
                let _ = nexus_core::permissions::repair_private_tree(parent);
            }
            let _ = nexus_core::atomic::atomic_write_private(&path, body.as_bytes());
        }
    }
    Ok(models)
}

fn model_usize(value: &serde_json::Value, keys: &[&str]) -> Option<usize> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|number| usize::try_from(number).ok())
            .filter(|number| *number > 0)
    })
}

// ----------------------------------------------------------------- usage

/// One rate-limit window on the account (e.g. the 5h or weekly window).
#[derive(Debug, Clone)]
pub struct UsageWindow {
    /// Human label derived from the window length ("5h window", "weekly window").
    pub label: String,
    pub used_percent: f64,
    /// Unix seconds when the window resets.
    pub resets_at: Option<i64>,
    pub window_minutes: u64,
}

/// The account's plan usage picture, from `account/rateLimits/read` and
/// `account/usage/read` (the same data the official codex CLI shows).
#[derive(Debug, Clone)]
pub struct CodexUsage {
    pub plan_type: String,
    pub email: Option<String>,
    pub windows: Vec<UsageWindow>,
    /// Free full-reset credits currently available on the account.
    pub reset_credits: u64,
    pub lifetime_tokens: Option<u64>,
    /// Tokens used today (latest daily bucket, when it is today).
    pub today_tokens: Option<u64>,
    pub limit_reached: Option<String>,
}

fn window_label(mins: u64) -> String {
    match mins {
        300 => "5h window".into(),
        10080 => "weekly window".into(),
        m if m % 1440 == 0 => format!("{}d window", m / 1440),
        m if m % 60 == 0 => format!("{}h window", m / 60),
        m => format!("{m}min window"),
    }
}

fn parse_window(v: &serde_json::Value, mins_hint: Option<u64>) -> Option<UsageWindow> {
    let obj = v.as_object()?;
    let mins = obj
        .get("windowDurationMins")
        .and_then(|m| m.as_u64())
        .or(mins_hint)?;
    Some(UsageWindow {
        label: window_label(mins),
        used_percent: obj
            .get("usedPercent")
            .and_then(|p| p.as_f64())
            .unwrap_or(0.0),
        resets_at: obj.get("resetsAt").and_then(|r| r.as_i64()),
        window_minutes: mins,
    })
}

/// Fetch the account's rate limits (5h/weekly windows, reset times) and token
/// usage via the codex app-server, isolated profile only.
pub async fn usage() -> Result<CodexUsage> {
    let results = app_server_call(
        &[
            (2, "account/rateLimits/read", serde_json::json!({})),
            (3, "account/usage/read", serde_json::json!({})),
            (4, "account/read", serde_json::json!({})),
        ],
        20,
    )
    .await?;

    let limits = results.get(&2).cloned().unwrap_or(serde_json::Value::Null);
    let rl = limits
        .get("rateLimits")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let mut windows = Vec::new();
    if let Some(w) = rl.get("primary").and_then(|p| parse_window(p, None)) {
        windows.push(w);
    }
    if let Some(w) = rl.get("secondary").and_then(|p| parse_window(p, None)) {
        windows.push(w);
    }
    windows.sort_by_key(|w| w.window_minutes);
    let reset_credits = limits
        .pointer("/rateLimitResetCredits/availableCount")
        .and_then(|c| c.as_u64())
        .unwrap_or(0);

    let usage_v = results.get(&3).cloned().unwrap_or(serde_json::Value::Null);
    let lifetime_tokens = usage_v
        .pointer("/summary/lifetimeTokens")
        .and_then(|t| t.as_u64());
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let today_tokens = usage_v
        .pointer("/dailyUsageBuckets")
        .and_then(|b| b.as_array())
        .and_then(|buckets| {
            buckets
                .iter()
                .find(|b| b.get("startDate").and_then(|d| d.as_str()) == Some(today.as_str()))
                .and_then(|b| b.get("tokens"))
                .and_then(|t| t.as_u64())
        });

    let account = results.get(&4).cloned().unwrap_or(serde_json::Value::Null);
    Ok(CodexUsage {
        plan_type: rl
            .get("planType")
            .and_then(|p| p.as_str())
            .or_else(|| {
                account
                    .pointer("/account/planType")
                    .and_then(|p| p.as_str())
            })
            .unwrap_or("unknown")
            .to_string(),
        email: account
            .pointer("/account/email")
            .and_then(|e| e.as_str())
            .map(String::from),
        windows,
        reset_credits,
        lifetime_tokens,
        today_tokens,
        limit_reached: rl
            .get("rateLimitReachedType")
            .and_then(|t| t.as_str())
            .map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_code_extraction() {
        assert_eq!(
            extract_user_code("Enter this code: ABCD-1234").as_deref(),
            Some("ABCD-1234")
        );
        assert_eq!(
            extract_user_code("code XQ7P-99AB-CDEF shown"),
            Some("XQ7P-99AB-CDEF".into())
        );
        assert_eq!(extract_user_code("visit https://example.com/device"), None);
        assert_eq!(extract_user_code("no code here"), None);
        // Plain lowercase words with a hyphen are not codes.
        assert_eq!(extract_user_code("read the how-to guide"), None);
    }
}
