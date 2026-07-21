//! Provider catalog for the interactive `/connect` and `/login` flows.
//!
//! Every entry reflects measured state: endpoints are probed, auth states are
//! read from the credential store / Codex profiles, and providers this build
//! does not implement are labeled exactly that — never faked.

use crate::app::App;
use crate::report::{Report, Sev};
use nexus_core::config::ModelConfig;
use nexus_core::config::{LimitMode, LimitSource};
use nexus_core::{NexusError, Result, SecretString};
use nexus_models::{DiscoveredModel, ProbeError};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
pub const OLLAMA_DEFAULT: &str = "http://127.0.0.1:11434";
pub const LLAMACPP_DEFAULT: &str = "http://127.0.0.1:8080/v1";
pub const OPENAI_DEFAULT: &str = "https://api.openai.com/v1";
pub const OPENROUTER_DEFAULT: &str = "https://openrouter.ai/api/v1";
pub const ANTHROPIC_DEFAULT: &str = nexus_models::ANTHROPIC_DEFAULT;
pub const GEMINI_DEFAULT: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
pub const GROQ_DEFAULT: &str = "https://api.groq.com/openai/v1";
pub const MISTRAL_DEFAULT: &str = "https://api.mistral.ai/v1";
pub const XAI_DEFAULT: &str = "https://api.x.ai/v1";
pub const DEEPSEEK_DEFAULT: &str = "https://api.deepseek.com";

const OPENAI_PRESETS: &[(&str, &str, &str, &str)] = &[
    (
        "openrouter",
        "OpenRouter",
        OPENROUTER_DEFAULT,
        "OPENROUTER_API_KEY",
    ),
    ("gemini", "Google Gemini", GEMINI_DEFAULT, "GEMINI_API_KEY"),
    ("groq", "Groq", GROQ_DEFAULT, "GROQ_API_KEY"),
    ("mistral", "Mistral", MISTRAL_DEFAULT, "MISTRAL_API_KEY"),
    ("xai", "xAI", XAI_DEFAULT, "XAI_API_KEY"),
    ("deepseek", "DeepSeek", DEEPSEEK_DEFAULT, "DEEPSEEK_API_KEY"),
];

/// What a provider needs before it can serve completions.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum AuthRequirement {
    None,
    ApiKey,
    /// Codex device login (delegated to the codex CLI) or an API key.
    DeviceLoginOrApiKey,
    /// Official Claude CLI subscription login, gated by explicit consent.
    SubscriptionLogin,
}

impl AuthRequirement {
    pub fn label(&self) -> &'static str {
        match self {
            AuthRequirement::None => "no authentication required",
            AuthRequirement::ApiKey => "API key required",
            AuthRequirement::DeviceLoginOrApiKey => "device login available (or API key)",
            AuthRequirement::SubscriptionLogin => "Claude subscription login required",
        }
    }
}

/// Probe outcome for one provider endpoint.
#[derive(Debug, Clone, serde::Serialize)]
pub enum EndpointState {
    /// Reachable; models enumerated.
    Connected {
        models: Vec<DiscoveredModel>,
        latency_ms: u64,
    },
    /// The last successful inventory retained after a failed refresh.
    Stale {
        models: Vec<DiscoveredModel>,
        latency_ms: u64,
        error: String,
    },
    /// Probe ran and failed (classified).
    Unreachable(String),
    /// Not probed (no endpoint to probe, or auth missing).
    NotProbed(String),
}

/// One row of the provider list.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderEntry {
    /// Stable id: `ollama`, `llamacpp`, `codex`, `openai`, `openrouter`,
    /// `custom:<name>`, `claude-plan`, `anthropic`, and API presets.
    pub id: String,
    pub label: String,
    pub local: bool,
    /// False = shown but honestly marked unavailable in this build.
    pub implemented: bool,
    pub auth: AuthRequirement,
    pub auth_state: String,
    pub authenticated: bool,
    #[serde(serialize_with = "serialize_redacted_endpoint")]
    pub endpoint: Option<String>,
    pub state: EndpointState,
    /// Config entries (`[models.*]`) backed by this provider.
    pub configured_models: Vec<String>,
}

pub fn redacted_endpoint_identity(endpoint: Option<&str>) -> Option<String> {
    endpoint.map(|raw| {
        url::Url::parse(raw)
            .map(|mut parsed| {
                let _ = parsed.set_username("");
                let _ = parsed.set_password(None);
                parsed.set_query(None);
                parsed.set_fragment(None);
                parsed.as_str().trim_end_matches('/').to_string()
            })
            .unwrap_or_else(|_| "<invalid endpoint>".into())
    })
}

fn serialize_redacted_endpoint<S>(
    endpoint: &Option<String>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serde::Serialize::serialize(&redacted_endpoint_identity(endpoint.as_deref()), serializer)
}

const CATALOG_FRESH_SECS: i64 = 15 * 60;

fn endpoint_fingerprint(endpoint: Option<&str>) -> String {
    let normalized = endpoint
        .and_then(|raw| url::Url::parse(raw).ok())
        .map(|mut parsed| {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.set_query(None);
            parsed.set_fragment(None);
            parsed.as_str().trim_end_matches('/').to_ascii_lowercase()
        })
        .unwrap_or_else(|| {
            endpoint
                .unwrap_or("adapter-managed")
                .trim_end_matches('/')
                .to_ascii_lowercase()
        });
    hex::encode(Sha256::digest(normalized.as_bytes()))
}

fn auth_profile_identity(entry: &ProviderEntry) -> String {
    // This is deliberately an identity label, never key material. Custom
    // profile names and preset provider ids are non-secret configuration.
    entry
        .id
        .strip_prefix("custom:")
        .unwrap_or(&entry.id)
        .to_string()
}

fn instance_key(entry: &ProviderEntry) -> String {
    format!(
        "{}:{}:{}",
        entry.id,
        endpoint_fingerprint(entry.endpoint.as_deref()),
        auth_profile_identity(entry)
    )
}

fn load_cached_state(app: &App, entry: &ProviderEntry) -> Result<Option<EndpointState>> {
    let key = instance_key(entry);
    app.global_store.with_retry(|connection| {
        let mut statement = connection.prepare(
            "SELECT inventory_json, health, latency_ms, last_success_at, last_error
             FROM provider_catalog_cache WHERE instance_key=?1",
        )?;
        let mut rows = statement.query([&key])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let inventory: String = row.get(0)?;
        let health: String = row.get(1)?;
        let latency: Option<u64> = row.get(2)?;
        let last_success: Option<String> = row.get(3)?;
        let error: Option<String> = row.get(4)?;
        let models: Vec<DiscoveredModel> = serde_json::from_str(&inventory)
            .map_err(|e| NexusError::other(format!("provider catalog cache: {e}")))?;
        let age = last_success
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|stamp| {
                chrono::Utc::now()
                    .signed_duration_since(stamp.with_timezone(&chrono::Utc))
                    .num_seconds()
            })
            .unwrap_or(i64::MAX);
        let latency_ms = latency.unwrap_or_default();
        let stale_reason = error.unwrap_or_else(|| {
            if age > CATALOG_FRESH_SECS {
                format!("cached inventory is {}s old", age.max(0))
            } else {
                "refresh interrupted".into()
            }
        });
        let state = match health.as_str() {
            "healthy" if age <= CATALOG_FRESH_SECS => {
                EndpointState::Connected { models, latency_ms }
            }
            "healthy" | "stale" | "refreshing" if !models.is_empty() => EndpointState::Stale {
                models,
                latency_ms,
                error: stale_reason,
            },
            "error" | "refreshing" => EndpointState::Unreachable(stale_reason),
            _ => return Ok(None),
        };
        Ok(Some(state))
    })
}

fn begin_refresh(app: &App, entry: &ProviderEntry) -> Result<i64> {
    let key = instance_key(entry);
    let provider = entry.id.clone();
    let fingerprint = endpoint_fingerprint(entry.endpoint.as_deref());
    let auth_profile = auth_profile_identity(entry);
    app.global_store.with_retry(|connection| {
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO provider_catalog_cache
             (instance_key, provider_id, endpoint_fingerprint, auth_profile_id, health, refresh_generation, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'refreshing', 1, ?5)
             ON CONFLICT(instance_key) DO UPDATE SET
               health='refreshing', refresh_generation=refresh_generation+1, updated_at=excluded.updated_at, last_error=NULL",
            rusqlite::params![key, provider, fingerprint, auth_profile, nexus_core::now_rfc3339()],
        )?;
        let generation = transaction.query_row(
            "SELECT refresh_generation FROM provider_catalog_cache WHERE instance_key=?1",
            [&key], |row| row.get(0),
        )?;
        transaction.commit()?;
        Ok(generation)
    })
}

fn commit_refresh(
    app: &App,
    entry: &ProviderEntry,
    generation: i64,
    state: &EndpointState,
) -> Result<bool> {
    let key = instance_key(entry);
    app.global_store.with_retry(|connection| {
        let now = nexus_core::now_rfc3339();
        let changed = match state {
            EndpointState::Connected { models, latency_ms } => connection.execute(
                "UPDATE provider_catalog_cache SET inventory_json=?3, health='healthy', latency_ms=?4,
                 last_success_at=?5, updated_at=?5, last_error=NULL WHERE instance_key=?1 AND refresh_generation=?2",
                rusqlite::params![key, generation, serde_json::to_string(models)?, latency_ms, now],
            )?,
            EndpointState::Unreachable(error) | EndpointState::NotProbed(error) => connection.execute(
                "UPDATE provider_catalog_cache SET health=CASE WHEN inventory_json='[]' THEN 'error' ELSE 'stale' END,
                 updated_at=?3, last_error=?4 WHERE instance_key=?1 AND refresh_generation=?2",
                rusqlite::params![key, generation, now, app.redactor.redact(error)],
            )?,
            EndpointState::Stale { error, .. } => connection.execute(
                "UPDATE provider_catalog_cache SET health='stale', updated_at=?3, last_error=?4
                 WHERE instance_key=?1 AND refresh_generation=?2",
                rusqlite::params![key, generation, now, app.redactor.redact(error)],
            )?,
        };
        Ok(changed == 1)
    })
}

impl ProviderEntry {
    /// Terminal-safe status marker: `●` connected/authenticated, `◐` needs
    /// setup, `○` unavailable. Never the only signal — text accompanies it.
    pub fn marker(&self) -> &'static str {
        if !self.implemented {
            return "○";
        }
        match &self.state {
            EndpointState::Connected { .. } => "●",
            _ if self.authenticated => "●",
            _ => "◐",
        }
    }

    pub fn summary(&self) -> String {
        let place = if self.local { "local" } else { "remote" };
        let state = match &self.state {
            EndpointState::Connected { models, .. } => {
                format!("connected, {} model(s)", models.len())
            }
            EndpointState::Stale { models, error, .. } => {
                format!("stale, {} model(s) · {error}", models.len())
            }
            EndpointState::Unreachable(reason) => reason.clone(),
            EndpointState::NotProbed(reason) => reason.clone(),
        };
        format!("{place} · {state}")
    }
}

fn config_endpoint_for(app: &App, provider: &str) -> Option<String> {
    app.config
        .models
        .values()
        .find(|m| m.provider == provider && !m.base_url.is_empty())
        .map(|m| m.base_url.clone())
}

fn configured_models_for(app: &App, pred: impl Fn(&ModelConfig) -> bool) -> Vec<String> {
    app.config
        .models
        .iter()
        .filter(|(_, m)| pred(m))
        .map(|(n, _)| n.clone())
        .collect()
}

/// Build the provider list without network access. Explicit refresh/probe
/// actions perform discovery; ordinary menu construction uses cached config
/// and never invents model rows.
pub async fn catalog(app: &App) -> Vec<ProviderEntry> {
    let mut ollama_instances: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, model) in app
        .config
        .models
        .iter()
        .filter(|(_, model)| model.provider == "ollama")
    {
        ollama_instances
            .entry(model.base_url.trim_end_matches('/').to_string())
            .or_default()
            .push(name.clone());
    }
    let ollama_endpoint = ollama_instances
        .keys()
        .next()
        .cloned()
        .filter(|endpoint| !endpoint.is_empty())
        .unwrap_or_else(|| OLLAMA_DEFAULT.into());
    let llamacpp_endpoint =
        config_endpoint_for(app, "llamacpp").unwrap_or_else(|| LLAMACPP_DEFAULT.into());

    let mut out = Vec::new();

    out.push(ProviderEntry {
        id: "ollama".into(),
        label: "Ollama".into(),
        local: true,
        implemented: true,
        auth: AuthRequirement::None,
        auth_state: AuthRequirement::None.label().into(),
        authenticated: true,
        endpoint: Some(ollama_endpoint.clone()),
        state: EndpointState::NotProbed("refresh to discover endpoint models".into()),
        configured_models: ollama_instances
            .get(ollama_endpoint.trim_end_matches('/'))
            .cloned()
            .unwrap_or_default(),
    });
    for (index, (endpoint, configured_models)) in ollama_instances
        .iter()
        .filter(|(endpoint, _)| endpoint.as_str() != ollama_endpoint.trim_end_matches('/'))
        .enumerate()
    {
        out.push(ProviderEntry {
            id: format!("ollama:{}", index + 2),
            label: format!("Ollama · {endpoint}"),
            local: nexus_models::openai_compat::is_local_url(endpoint),
            implemented: true,
            auth: AuthRequirement::None,
            auth_state: AuthRequirement::None.label().into(),
            authenticated: true,
            endpoint: Some(endpoint.clone()),
            state: EndpointState::NotProbed("refresh to discover endpoint models".into()),
            configured_models: configured_models.clone(),
        });
    }

    out.push(ProviderEntry {
        id: "llamacpp".into(),
        label: "llama.cpp".into(),
        local: true,
        implemented: true,
        auth: AuthRequirement::None,
        auth_state: AuthRequirement::None.label().into(),
        authenticated: true,
        endpoint: Some(llamacpp_endpoint.clone()),
        state: EndpointState::NotProbed("refresh to discover endpoint models".into()),
        configured_models: configured_models_for(app, |m| m.provider == "llamacpp"),
    });

    // Codex: state from the auth service, no network probe.
    let allow_existing = app.read_ui_state(|s| s.codex_use_existing);
    let codex = crate::codex::status_with_consent(allow_existing);
    let (codex_state, codex_authed) = match (&codex.isolated, &codex.existing) {
        (Some(p), _) => (
            format!("authenticated — isolated profile ({})", p.mode),
            true,
        ),
        (None, Some(p)) if allow_existing => (
            format!(
                "authenticated — existing CLI session with consent ({})",
                p.mode
            ),
            true,
        ),
        (None, Some(p)) => (
            format!(
                "existing Codex CLI session available ({}) — consent required",
                p.mode
            ),
            false,
        ),
        (None, None) if codex.cli_installed => ("login required".to_string(), false),
        (None, None) => ("codex CLI not installed".to_string(), false),
    };
    out.push(ProviderEntry {
        id: "codex".into(),
        label: "Codex (Sign in with ChatGPT)".into(),
        local: false,
        implemented: true,
        auth: AuthRequirement::DeviceLoginOrApiKey,
        auth_state: codex_state.clone(),
        authenticated: codex_authed,
        endpoint: Some(nexus_models::CODEX_BACKEND_DEFAULT.into()),
        state: EndpointState::NotProbed(if codex_authed {
            "select to list the models on your plan".into()
        } else {
            codex_state
        }),
        configured_models: configured_models_for(app, |m| {
            m.provider == "codex" || m.auth.as_deref() == Some("codex")
        }),
    });

    // Claude plan bridge: auth is inspected only after explicit consent.
    let claude_consent = app.read_ui_state(|state| state.claude_use_existing);
    let claude = crate::claude::status_with_consent(claude_consent).await;
    let claude_authenticated = claude.authenticated == Some(true);
    out.push(ProviderEntry {
        id: "claude-plan".into(),
        label: "Claude Plan (subscription)".into(),
        local: false,
        implemented: true,
        auth: AuthRequirement::SubscriptionLogin,
        auth_state: claude.detail.clone(),
        authenticated: claude_authenticated,
        endpoint: None,
        state: EndpointState::NotProbed(if claude_authenticated {
            "select to use a Claude model alias".into()
        } else {
            claude.detail
        }),
        configured_models: configured_models_for(app, |model| model.provider == "claude-plan"),
    });

    // OpenAI (API key).
    let openai_key = api_key_state(app, "openai", "OPENAI_API_KEY");
    out.push(ProviderEntry {
        id: "openai".into(),
        label: "OpenAI API".into(),
        local: false,
        implemented: true,
        auth: AuthRequirement::ApiKey,
        authenticated: openai_key.0,
        auth_state: openai_key.1.clone(),
        endpoint: Some(OPENAI_DEFAULT.into()),
        state: EndpointState::NotProbed(openai_key.1),
        configured_models: configured_models_for(app, |m| {
            m.provider == "openai" && m.auth.as_deref() != Some("codex")
        }),
    });

    // Anthropic native Messages API.
    let anthropic_key = api_key_state(app, "anthropic", "ANTHROPIC_API_KEY");
    out.push(ProviderEntry {
        id: "anthropic".into(),
        label: "Anthropic API".into(),
        local: false,
        implemented: true,
        auth: AuthRequirement::ApiKey,
        authenticated: anthropic_key.0,
        auth_state: anthropic_key.1.clone(),
        endpoint: Some(ANTHROPIC_DEFAULT.into()),
        state: EndpointState::NotProbed(anthropic_key.1),
        configured_models: configured_models_for(app, |model| model.provider == "anthropic"),
    });

    // Official OpenAI-compatible API presets.
    for (id, label, endpoint, env_var) in OPENAI_PRESETS {
        let key = api_key_state(app, id, env_var);
        out.push(ProviderEntry {
            id: (*id).into(),
            label: (*label).into(),
            local: false,
            implemented: true,
            auth: AuthRequirement::ApiKey,
            authenticated: key.0,
            auth_state: key.1.clone(),
            endpoint: Some((*endpoint).into()),
            state: EndpointState::NotProbed(key.1),
            configured_models: configured_models_for(app, |model| {
                model.base_url.trim_end_matches('/') == endpoint.trim_end_matches('/')
            }),
        });
    }

    // Custom endpoints already configured (openai_compatible / custom_http
    // that aren't the llama.cpp default or OpenRouter).
    for (name, m) in app.config.models.iter() {
        let is_custom = matches!(m.provider.as_str(), "openai_compatible" | "custom_http")
            && m.base_url != llamacpp_endpoint
            && !is_preset_endpoint(&m.base_url);
        if !is_custom {
            continue;
        }
        let local = nexus_models::openai_compat::is_local_url(&m.base_url);
        let needs_key = m.api_key_env.is_some() || m.api_key_ref.is_some();
        let (authed, auth_state) = if !needs_key {
            (true, AuthRequirement::None.label().to_string())
        } else if m.resolved_api_key.is_some()
            || m.api_key_env
                .as_ref()
                .map(|e| std::env::var(e).map(|v| !v.is_empty()).unwrap_or(false))
                .unwrap_or(false)
        {
            (true, "API key configured".to_string())
        } else {
            (false, "API key required (missing)".to_string())
        };
        out.push(ProviderEntry {
            id: format!("custom:{name}"),
            label: format!("Custom · {name}"),
            local,
            implemented: true,
            auth: if needs_key {
                AuthRequirement::ApiKey
            } else {
                AuthRequirement::None
            },
            authenticated: authed,
            auth_state,
            endpoint: Some(m.base_url.clone()),
            state: EndpointState::NotProbed("select to test".into()),
            configured_models: vec![name.clone()],
        });
    }

    for entry in &mut out {
        if let Ok(Some(state)) = load_cached_state(app, entry) {
            entry.state = state;
        }
    }
    out
}

/// Explicit provider-wide refresh used by `/model`. Every configured provider
/// instance is probed through its adapter; no model name is synthesized here.
/// Failed inventories are non-authoritative and therefore never delete cache.
pub async fn refresh_catalog(app: &App) -> Vec<ProviderEntry> {
    use futures::{stream, StreamExt};

    let entries = catalog(app).await;
    let mut refreshed: Vec<ProviderEntry> =
        stream::iter(entries.into_iter().map(|mut entry| async move {
            let should_probe =
                !entry.configured_models.is_empty() || (entry.authenticated && !entry.local);
            if should_probe {
                let generation = begin_refresh(app, &entry).ok();
                let refreshed = probe_provider(app, &entry).await;
                entry.state = match refreshed {
                    EndpointState::Unreachable(error) => load_cached_state(app, &entry)
                        .ok()
                        .flatten()
                        .and_then(|cached| match cached {
                            EndpointState::Connected { models, latency_ms }
                            | EndpointState::Stale {
                                models, latency_ms, ..
                            } => Some(EndpointState::Stale {
                                models,
                                latency_ms,
                                error: error.clone(),
                            }),
                            _ => None,
                        })
                        .unwrap_or(EndpointState::Unreachable(error)),
                    state => state,
                };
                if let Some(generation) = generation {
                    if !commit_refresh(app, &entry, generation, &entry.state).unwrap_or(false) {
                        if let Ok(Some(newer)) = load_cached_state(app, &entry) {
                            entry.state = newer;
                        }
                    }
                }
            }
            entry
        }))
        .buffer_unordered(8)
        .collect()
        .await;
    refreshed.sort_by(|left, right| left.id.cmp(&right.id));
    if let Err(error) = reconcile_managed_inventory(app, &refreshed) {
        // Swallowing this left the operator staring at models the server no
        // longer has, with nothing anywhere saying why.
        tracing::warn!(%error, "managed model inventory could not be reconciled");
    }
    refreshed
}

pub async fn catalog_report(app: &App) -> Report {
    let entries = catalog(app).await;
    let active = app.read_ui_state(|state| state.active_model.clone());
    let mut report = Report::new("model catalog")
        .field("active session pin", active.as_deref().unwrap_or("none"))
        .field(
            "routing simple",
            app.config.routing.simple.as_deref().unwrap_or("inherited"),
        )
        .field(
            "routing coding",
            app.config.routing.coding.as_deref().unwrap_or("inherited"),
        )
        .field(
            "routing planning",
            app.config
                .routing
                .planning
                .as_deref()
                .unwrap_or("inherited"),
        )
        .field(
            "routing fallback",
            app.config.routing.fallback.as_deref().unwrap_or("none"),
        );
    for entry in entries {
        report = report.header(format!("{} {}", entry.marker(), entry.label));
        report = report.line(entry.summary());
        let inventory = match &entry.state {
            EndpointState::Connected { models, .. } | EndpointState::Stale { models, .. } => {
                Some(models)
            }
            _ => None,
        };
        for configured_name in &entry.configured_models {
            if let Some(configured) = app.config.models.get(configured_name) {
                let discovered = inventory
                    .and_then(|models| models.iter().find(|model| model.id == configured.model));
                let availability = if discovered.is_some() {
                    "available"
                } else {
                    "unavailable"
                };
                let reasoning = discovered
                    .and_then(|model| model.reasoning.as_ref())
                    .map(|profile| {
                        format!(
                            "reasoning {:?} · {:?}",
                            profile.supported_efforts, profile.provenance
                        )
                    })
                    .unwrap_or_else(|| "reasoning default only".into());
                report = report.line(format!(
                    "{configured_name} · {} · {availability} · {reasoning}",
                    configured.model
                ));
            }
        }
    }
    report
}

fn reconcile_managed_inventory(app: &App, entries: &[ProviderEntry]) -> Result<()> {
    let mut managed = nexus_core::config::Config::load_managed_models(&app.paths)?;
    let original = toml::to_string(&managed).unwrap_or_default();
    for entry in entries {
        let EndpointState::Connected { models, .. } = &entry.state else {
            continue;
        };
        for name in &entry.configured_models {
            let Some(configured) = managed.get(name).cloned() else {
                continue; // hand-written config is never rewritten here
            };
            let Some(discovered) = models.iter().find(|model| model.id == configured.model) else {
                managed.remove(name);
                continue;
            };
            if configured.limit_mode == LimitMode::Auto {
                if let Some(model) = managed.get_mut(name) {
                    apply_effective_limits(model, discovered);
                }
            }
        }
    }
    if toml::to_string(&managed).unwrap_or_default() != original {
        nexus_core::config::Config::save_managed_models(&app.paths, &managed)?;
    }
    Ok(())
}

fn bundled_limits(model_id: &str) -> Option<(usize, usize)> {
    // Exact ids only. This table enriches a model already returned by a
    // provider and must never be used to create discovery rows.
    match model_id {
        "gpt-4.1" | "gpt-4.1-mini" | "gpt-4.1-nano" => Some((1_047_576, 32_768)),
        _ => None,
    }
}

fn apply_effective_limits(config: &mut ModelConfig, discovered: &DiscoveredModel) {
    // A self-hosted server allocates a KV cache the size of the requested
    // context before it answers, so the reported maximum is a ceiling to
    // record, not a window to request. Hosted providers charge per token
    // instead and are happy to be told the full number.
    let record_context = |config: &mut ModelConfig, context: usize, source| {
        if config.is_self_hosted() {
            config.context_ceiling = Some(context);
            config.context_window = context.min(nexus_core::config::SELF_HOSTED_DEFAULT_CONTEXT);
        } else {
            config.context_window = context;
        }
        config.context_limit_source = source;
    };
    if let Some(context) = discovered.context_window {
        record_context(config, context, LimitSource::ProviderMetadata);
    } else if let Some((context, _)) = bundled_limits(&discovered.id) {
        record_context(config, context, LimitSource::BundledCatalog);
    }
    if let Some(output) = discovered.max_output_tokens {
        config.max_output_tokens = output;
        config.output_limit_source = LimitSource::ProviderMetadata;
    } else if let Some((_, output)) = bundled_limits(&discovered.id) {
        config.max_output_tokens = output;
        config.output_limit_source = LimitSource::BundledCatalog;
    } else if config.is_self_hosted()
        && config.output_limit_source == LimitSource::ConfiguredConservative
    {
        // Nothing to go on, and the general 1024-token default truncates a
        // local model mid-answer for no benefit — these tokens are not metered.
        config.max_output_tokens = nexus_core::config::SELF_HOSTED_DEFAULT_OUTPUT;
    }
}

fn is_preset_endpoint(endpoint: &str) -> bool {
    let endpoint = endpoint.trim_end_matches('/');
    OPENAI_PRESETS
        .iter()
        .any(|(_, _, preset, _)| endpoint == preset.trim_end_matches('/'))
}

fn probe_to_state(
    probe: std::result::Result<nexus_models::ProbeOutcome, ProbeError>,
) -> EndpointState {
    match probe {
        Ok(outcome) => EndpointState::Connected {
            models: outcome.models,
            latency_ms: outcome.latency_ms,
        },
        Err(e) => EndpointState::Unreachable(e.to_string()),
    }
}

fn api_key_state(app: &App, provider: &str, env_var: &str) -> (bool, String) {
    if std::env::var(env_var)
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        return (true, format!("API key from ${env_var}"));
    }
    if app.credentials.exists(provider, "default") {
        return (true, "API key in credential store".into());
    }
    (false, "API key required".into())
}

/// Probe one provider entry on demand (used when the operator selects it).
pub async fn probe_provider(app: &App, entry: &ProviderEntry) -> EndpointState {
    match entry.id.as_str() {
        "claude-plan" => {
            let started = std::time::Instant::now();
            let status = crate::claude::status_with_consent(
                app.read_ui_state(|state| state.claude_use_existing),
            )
            .await;
            if status.authenticated == Some(true) {
                EndpointState::Connected {
                    models: entry
                        .configured_models
                        .iter()
                        .filter_map(|name| app.config.models.get(name))
                        .map(|configured| DiscoveredModel {
                            id: configured.model.clone(),
                            size_bytes: None,
                            family: Some("Claude subscription alias".into()),
                            parameter_size: None,
                            quantization: None,
                            display_name: Some(configured.model.clone()),
                            description: Some(
                                "resolved by the official Claude CLI at request time".into(),
                            ),
                            context_window: None,
                            max_output_tokens: None,
                            context_limit_source: None,
                            output_limit_source: None,
                            reasoning: Some(nexus_models::ReasoningProfile {
                                supported_efforts: configured
                                    .reasoning_effort
                                    .clone()
                                    .into_iter()
                                    .collect(),
                                default_effort: configured.reasoning_effort.clone(),
                                control: if configured.reasoning_effort.is_some() {
                                    nexus_models::ReasoningControl::Optional
                                } else {
                                    nexus_models::ReasoningControl::ProviderManaged
                                },
                                mandatory: false,
                                provider_managed: configured.reasoning_effort.is_none(),
                                provenance: nexus_models::ReasoningProvenance::InstalledCli,
                            }),
                        })
                        .collect(),
                    latency_ms: started.elapsed().as_millis() as u64,
                }
            } else {
                EndpointState::Unreachable(status.detail)
            }
        }
        _ if entry.endpoint.is_none() => EndpointState::NotProbed("no endpoint".into()),
        id if id == "ollama" || id.starts_with("ollama:") => probe_to_state(
            nexus_models::list_ollama_models(
                entry.endpoint.as_deref().unwrap_or(OLLAMA_DEFAULT),
                PROBE_TIMEOUT,
            )
            .await,
        ),
        // Codex: the model list comes from the operator's plan (via the codex
        // CLI's app-server), not from probing the inference endpoint.
        "codex" => {
            let started = std::time::Instant::now();
            match crate::codex::list_plan_models().await {
                Ok(models) => EndpointState::Connected {
                    models: models
                        .into_iter()
                        .map(|m| nexus_models::DiscoveredModel {
                            id: m.id,
                            size_bytes: None,
                            family: None,
                            parameter_size: None,
                            quantization: None,
                            display_name: Some(m.display_name).filter(|d| !d.is_empty()),
                            description: Some(m.description).filter(|d| !d.is_empty()),
                            context_window: m.context_window,
                            max_output_tokens: m.max_output_tokens,
                            context_limit_source: m
                                .context_window
                                .map(|_| LimitSource::ProviderMetadata),
                            output_limit_source: m
                                .max_output_tokens
                                .map(|_| LimitSource::ProviderMetadata),
                            reasoning: Some(nexus_models::ReasoningProfile {
                                supported_efforts: m
                                    .reasoning_efforts
                                    .iter()
                                    .map(|effort| effort.effort.clone())
                                    .collect(),
                                default_effort: m.default_reasoning_effort.clone(),
                                control: if m.default_reasoning_effort.is_some() {
                                    nexus_models::ReasoningControl::Mandatory
                                } else {
                                    nexus_models::ReasoningControl::ProviderManaged
                                },
                                mandatory: true,
                                provider_managed: m.default_reasoning_effort.is_none(),
                                provenance: nexus_models::ReasoningProvenance::ProviderMetadata,
                            }),
                        })
                        .collect(),
                    latency_ms: started.elapsed().as_millis() as u64,
                },
                Err(e) => EndpointState::Unreachable(e.to_string()),
            }
        }
        "anthropic" => {
            let Some(key) = resolve_key(app, "anthropic", "ANTHROPIC_API_KEY") else {
                return EndpointState::NotProbed("API key required".into());
            };
            probe_to_state(
                nexus_models::anthropic::list_models(
                    entry.endpoint.as_deref().unwrap_or(ANTHROPIC_DEFAULT),
                    key.expose(),
                    PROBE_TIMEOUT,
                )
                .await,
            )
        }
        id => {
            let endpoint = entry.endpoint.as_deref().unwrap_or_default();
            let key = preset_env(id)
                .and_then(|env_var| resolve_key(app, id, env_var))
                .or_else(|| custom_key(app, entry));
            probe_to_state(
                nexus_models::list_openai_models(
                    endpoint,
                    key.as_ref().map(|k| k.expose()),
                    PROBE_TIMEOUT,
                )
                .await,
            )
        }
    }
}

fn preset_env(id: &str) -> Option<&'static str> {
    if id == "openai" {
        return Some("OPENAI_API_KEY");
    }
    OPENAI_PRESETS
        .iter()
        .find(|(preset, _, _, _)| *preset == id)
        .map(|(_, _, _, env)| *env)
}

fn resolve_key(app: &App, provider: &str, env_var: &str) -> Option<SecretString> {
    std::env::var(env_var)
        .ok()
        .filter(|v| !v.is_empty())
        .map(SecretString::new)
        .or_else(|| app.credentials.get(provider, "default").ok().flatten())
}

fn custom_key(app: &App, entry: &ProviderEntry) -> Option<SecretString> {
    let name = entry.id.strip_prefix("custom:")?;
    let m = app.config.models.get(name)?;
    if let Some(env) = &m.api_key_env {
        if let Ok(v) = std::env::var(env) {
            if !v.is_empty() {
                return Some(SecretString::new(v));
            }
        }
    }
    m.resolved_api_key.clone()
}

// -------------------------------------------------------- custom endpoints

/// The guided custom-endpoint form.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomEndpointSpec {
    /// Config entry name (TOML-key-safe).
    pub name: String,
    /// `openai_compatible`, `ollama`, or `llamacpp`.
    pub protocol: String,
    /// Host/port or full HTTP(S) URL. Host-only values are normalized using
    /// `use_tls`, and OpenAI-style presets receive `/v1` when no path is given.
    pub base_url: String,
    pub use_tls: bool,
    pub tls_verify: bool,
    /// Stored in the credential store, referenced by `api_key_ref`.
    pub api_key: Option<SecretString>,
    pub model: String,
    pub context_window: usize,
    pub max_output_tokens: usize,
    pub timeout_secs: u64,
}

impl Default for CustomEndpointSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            protocol: "openai_compatible".into(),
            base_url: String::new(),
            use_tls: true,
            tls_verify: true,
            api_key: None,
            model: String::new(),
            context_window: 8192,
            max_output_tokens: 2048,
            timeout_secs: 120,
        }
    }
}

fn validate_custom_endpoint_connection(spec: &CustomEndpointSpec) -> Result<()> {
    if !matches!(
        spec.protocol.as_str(),
        "openai_compatible" | "ollama" | "llamacpp"
    ) {
        return Err(NexusError::Config(format!(
            "unknown protocol `{}` — one of openai_compatible|ollama|llamacpp",
            spec.protocol
        )));
    }
    normalize_custom_endpoint_url(spec)?;
    if spec.context_window == 0 || spec.max_output_tokens == 0 || spec.timeout_secs == 0 {
        return Err(NexusError::Config(
            "context/output/timeout limits must be > 0".into(),
        ));
    }
    Ok(())
}

/// Normalize a custom endpoint supplied as either `host[:port]` or a full
/// URL. A full URL keeps its explicit scheme; otherwise the operator's TLS
/// choice determines `http` versus `https`.
pub fn normalize_custom_endpoint_url(spec: &CustomEndpointSpec) -> Result<String> {
    let raw = spec.base_url.trim().trim_end_matches('/');
    if raw.is_empty() {
        return Err(NexusError::Config(
            "endpoint host or URL is required".into(),
        ));
    }
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("{}://{raw}", if spec.use_tls { "https" } else { "http" })
    };
    let mut parsed = nexus_models::validate_base_url(&with_scheme)
        .map_err(|e| NexusError::Config(format!("endpoint URL: {e}")))?;
    if matches!(spec.protocol.as_str(), "openai_compatible" | "llamacpp")
        && (parsed.path().is_empty() || parsed.path() == "/")
    {
        parsed.set_path("/v1");
    }
    Ok(parsed.as_str().trim_end_matches('/').to_string())
}

/// Validate the full form before saving. Connection tests intentionally use a
/// less strict check so model discovery can happen before a model is selected.
pub fn validate_custom_endpoint(spec: &CustomEndpointSpec) -> Result<()> {
    validate_custom_endpoint_connection(spec)?;
    if spec.name.trim().is_empty()
        || !spec
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(NexusError::Config(
            "profile name must be non-empty and use letters/digits/`-`/`_`".into(),
        ));
    }
    if spec.model.trim().is_empty() {
        return Err(NexusError::Config("a model id is required".into()));
    }
    Ok(())
}

/// Test the endpoint described by the form without saving anything.
pub async fn test_custom_endpoint(spec: &CustomEndpointSpec) -> Result<Report> {
    validate_custom_endpoint_connection(spec)?;
    let base_url = normalize_custom_endpoint_url(spec)?;
    let timeout = Duration::from_secs(spec.timeout_secs.clamp(1, 300));
    let outcome = match spec.protocol.as_str() {
        "ollama" => {
            nexus_models::list_ollama_models_with_tls(&base_url, timeout, spec.tls_verify).await
        }
        _ => {
            nexus_models::list_openai_models_with_tls(
                &base_url,
                spec.api_key.as_ref().map(|k| k.expose()),
                timeout,
                spec.tls_verify,
            )
            .await
        }
    };
    match outcome {
        Ok(o) => {
            let mut r = Report::new("connection test").ok(format!(
                "reachable in {} ms — {} model(s)",
                o.latency_ms,
                o.models.len()
            ));
            for m in o.models.iter().take(10) {
                r = r.line_sev(format!("  {}", m.id), Sev::Dim);
            }
            Ok(r)
        }
        Err(e) => Ok(Report::new("connection test").error(e.to_string())),
    }
}

/// Persist a custom endpoint into the managed models file. The API key (if
/// any) goes to the credential store; config carries only `api_key_ref`.
pub fn save_custom_endpoint(app: &App, spec: &CustomEndpointSpec) -> Result<Report> {
    validate_custom_endpoint(spec)?;
    let base_url = normalize_custom_endpoint_url(spec)?;
    let mut managed = nexus_core::config::Config::load_managed_models(&app.paths)?;
    let mut model = ModelConfig {
        provider: spec.protocol.clone(),
        base_url,
        model: spec.model.clone(),
        context_window: spec.context_window,
        max_output_tokens: spec.max_output_tokens,
        timeout_secs: spec.timeout_secs,
        tls_verify: spec.tls_verify,
        role: "executor".into(),
        ..Default::default()
    };
    if let Some(key) = &spec.api_key {
        app.credentials.set("custom", &spec.name, key)?;
        model.api_key_ref = Some(format!("custom/{}", spec.name));
    }
    managed.insert(spec.name.clone(), model);
    nexus_core::config::Config::save_managed_models(&app.paths, &managed)?;
    Ok(Report::untitled().ok(format!(
        "saved `{}` to {} — reload applies it",
        spec.name,
        app.paths.managed_models_file.display()
    )))
}

/// Persist a discovered model or provider alias as a managed config entry so
/// it can be selected. Provider presets retain their credential namespace
/// while OpenAI-compatible protocols share the audited transport.
pub fn save_discovered_model(
    app: &App,
    provider_id: &str,
    base_url: &str,
    discovered: &DiscoveredModel,
) -> Result<String> {
    save_discovered_model_with_effort(app, provider_id, base_url, discovered, None)
}

pub fn save_discovered_model_with_effort(
    app: &App,
    provider_id: &str,
    base_url: &str,
    discovered: &DiscoveredModel,
    effort: Option<&str>,
) -> Result<String> {
    let mut managed = nexus_core::config::Config::load_managed_models(&app.paths)?;
    let model_id = discovered.id.as_str();
    let mut name = sanitize_model_name(model_id);
    let config_provider = match provider_id {
        "ollama" | "llamacpp" | "openai" | "codex" | "claude-plan" | "anthropic" => provider_id,
        _ => "openai_compatible",
    };
    let config_provider = if provider_id.starts_with("ollama:") {
        "ollama"
    } else {
        config_provider
    };
    // Avoid clobbering an existing different entry.
    let mut n = 2;
    while app.config.models.contains_key(&name)
        && app
            .config
            .models
            .get(&name)
            .map(|model| (model.model.as_str(), model.provider.as_str()))
            != Some((model_id, config_provider))
    {
        name = format!(
            "{}_{}_{n}",
            sanitize_model_name(provider_id),
            sanitize_model_name(model_id)
        );
        n += 1;
    }
    let mut model = ModelConfig {
        provider: config_provider.to_string(),
        base_url: base_url.to_string(),
        model: model_id.to_string(),
        role: "executor".into(),
        limit_mode: LimitMode::Auto,
        ..Default::default()
    };
    model.reasoning_effort = effort.map(str::to_string);
    apply_effective_limits(&mut model, discovered);
    if let Some(env_var) = preset_env(provider_id) {
        model.api_key_env = Some(env_var.into());
        if app.credentials.exists(provider_id, "default") {
            model.api_key_ref = Some(format!("{provider_id}/default"));
        }
        if model.context_limit_source == LimitSource::ConfiguredConservative {
            model.context_window = 128_000;
        }
        if model.output_limit_source == LimitSource::ConfiguredConservative {
            model.max_output_tokens = 8192;
        }
    }
    if provider_id == "anthropic" {
        model.api_key_env = Some("ANTHROPIC_API_KEY".into());
        if app.credentials.exists("anthropic", "default") {
            model.api_key_ref = Some("anthropic/default".into());
        }
        if model.context_limit_source == LimitSource::ConfiguredConservative {
            model.context_window = 200_000;
        }
        if model.output_limit_source == LimitSource::ConfiguredConservative {
            model.max_output_tokens = 8192;
        }
    }
    if provider_id == "codex" {
        // The backend is implied by the provider; keep config free of it.
        model.base_url = String::new();
        // The plan listing reports no context length; a conservative window
        // keeps compaction honest rather than optimistic.
        if model.context_limit_source == LimitSource::ConfiguredConservative {
            model.context_window = 128_000;
        }
        if model.output_limit_source == LimitSource::ConfiguredConservative {
            model.max_output_tokens = 8192;
        }
        model.reasoning_effort = crate::codex::cached_plan_models()
            .iter()
            .find(|m| m.id == model_id)
            .and_then(|m| m.default_reasoning_effort.clone());
    }
    if provider_id == "claude-plan" {
        model.base_url = String::new();
        if model.context_limit_source == LimitSource::ConfiguredConservative {
            model.context_window = 200_000;
        }
        if model.output_limit_source == LimitSource::ConfiguredConservative {
            model.max_output_tokens = 8192;
        }
        model.role = "planner".into();
        model.native_tool_calls = Some(false);
    }
    managed.insert(name.clone(), model);
    nexus_core::config::Config::save_managed_models(&app.paths, &managed)?;
    Ok(name)
}

/// Persist (or update) a codex plan model as a managed config entry with the
/// chosen reasoning effort (falls back to the plan's default effort). Reuses
/// the name of an existing configured entry for the same model, so picking an
/// effort for the active model updates it rather than duplicating it.
pub fn save_codex_model(app: &App, model_id: &str, effort: Option<&str>) -> Result<String> {
    let mut managed = nexus_core::config::Config::load_managed_models(&app.paths)?;
    let name = app
        .config
        .models
        .iter()
        .find(|(_, m)| m.provider == "codex" && m.model == model_id)
        .map(|(n, _)| n.clone())
        .unwrap_or_else(|| sanitize_model_name(model_id));
    let cache = crate::codex::cached_plan_models();
    let plan = cache.iter().find(|m| m.id == model_id);
    let effort = effort
        .map(String::from)
        .or_else(|| plan.and_then(|m| m.default_reasoning_effort.clone()));
    let mut configured = ModelConfig {
        provider: "codex".into(),
        base_url: String::new(),
        model: model_id.to_string(),
        context_window: 128_000,
        max_output_tokens: 8192,
        role: "executor".into(),
        reasoning_effort: effort,
        limit_mode: LimitMode::Auto,
        ..Default::default()
    };
    if let Some(plan) = plan {
        let discovered = DiscoveredModel {
            id: plan.id.clone(),
            size_bytes: None,
            family: None,
            parameter_size: None,
            quantization: None,
            display_name: Some(plan.display_name.clone()),
            description: Some(plan.description.clone()),
            context_window: plan.context_window,
            max_output_tokens: plan.max_output_tokens,
            context_limit_source: plan.context_window.map(|_| LimitSource::ProviderMetadata),
            output_limit_source: plan
                .max_output_tokens
                .map(|_| LimitSource::ProviderMetadata),
            reasoning: Some(nexus_models::ReasoningProfile {
                supported_efforts: plan
                    .reasoning_efforts
                    .iter()
                    .map(|effort| effort.effort.clone())
                    .collect(),
                default_effort: plan.default_reasoning_effort.clone(),
                control: if plan.default_reasoning_effort.is_some() {
                    nexus_models::ReasoningControl::Mandatory
                } else {
                    nexus_models::ReasoningControl::ProviderManaged
                },
                mandatory: true,
                provider_managed: plan.default_reasoning_effort.is_none(),
                provenance: nexus_models::ReasoningProvenance::ProviderMetadata,
            }),
        };
        apply_effective_limits(&mut configured, &discovered);
    }
    managed.insert(name.clone(), configured);
    nexus_core::config::Config::save_managed_models(&app.paths, &managed)?;
    Ok(name)
}

fn sanitize_model_name(model: &str) -> String {
    let mut s: String = model
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    let s = s.trim_matches('_').to_string();
    if s.is_empty() {
        "model".into()
    } else {
        s
    }
}

// --------------------------------------------------------------- model test

/// Run a minimal safe prompt against a configured model and measure latency.
pub async fn test_model(app: &App, name: &str) -> Result<Report> {
    use futures::StreamExt;
    let manager = nexus_models::ModelManager::from_config(&app.config)?;
    let provider = manager.get(name)?;
    let request = nexus_models::CompletionRequest {
        messages: vec![nexus_models::ChatMessage::user(
            "Reply with the single word: ready",
        )],
        max_tokens: Some(256),
        ..Default::default()
    };
    let started = std::time::Instant::now();
    let mut stream = provider.stream(request).await?;
    let mut first_token_ms: Option<u128> = None;
    let mut text = String::new();
    let mut tool_call_seen = false;
    let mut completion_tokens = 0usize;
    let mut done_seen = false;
    let mut finish_reason = String::new();
    while let Some(event) = stream.next().await {
        match event? {
            nexus_models::StreamEvent::TextDelta(t) => {
                if first_token_ms.is_none() && !t.trim().is_empty() {
                    first_token_ms = Some(started.elapsed().as_millis());
                }
                text.push_str(&t);
            }
            nexus_models::StreamEvent::Done {
                usage,
                finish_reason: reason,
            } => {
                done_seen = true;
                completion_tokens = usage.completion_tokens;
                finish_reason = reason;
                break;
            }
            nexus_models::StreamEvent::ToolCallDelta { .. } => tool_call_seen = true,
            nexus_models::StreamEvent::ProviderPrivateDelta(_) => {}
        }
    }
    let total = started.elapsed().as_millis();
    let mut r = Report::new(format!("model test — {name}"))
        .field_sev("connection", "ok", Sev::Ok)
        .field(
            "first token",
            first_token_ms
                .map(|ms| format!("{ms} ms"))
                .unwrap_or_else(|| "no visible tokens".into()),
        )
        .field("total", format!("{total} ms"))
        .field(
            "streaming",
            if first_token_ms.is_some() {
                "yes"
            } else {
                "no output"
            },
        );
    let trimmed = text.trim();
    if !done_seen {
        return Err(nexus_core::NexusError::Provider {
            provider: provider.kind().into(),
            message: "model stream ended without a terminal response".into(),
        });
    }
    if trimmed.is_empty() && !tool_call_seen {
        return Err(nexus_core::NexusError::Provider {
            provider: provider.kind().into(),
            message: if completion_tokens >= 256 {
                "model consumed its test budget without final text or a tool call".into()
            } else {
                "model produced no final text or tool call".into()
            },
        });
    }
    if finish_reason == "length" {
        return Err(nexus_core::NexusError::Provider {
            provider: provider.kind().into(),
            message: "model exhausted its test output budget before a terminal answer".into(),
        });
    }
    if !trimmed.is_empty() {
        let mut sample: String = trimmed.chars().take(60).collect();
        if trimmed.chars().count() > 60 {
            sample.push('…');
        }
        r = r.field("reply", sample);
    }
    Ok(r)
}

/// Render the `snx catalog list`-style table from config (shared).
pub fn models_report(app: &App) -> Report {
    if app.config.models.is_empty() {
        return Report::new("models")
            .warn("no models configured — run /connect (or `snx setup`) to add one");
    }
    let active = app.any_model_name();
    let rows: Vec<Vec<String>> = app
        .config
        .models
        .iter()
        .map(|(name, m)| {
            let mark = if *name == active { "●" } else { " " };
            vec![
                format!("{mark} {name}"),
                m.provider.clone(),
                m.model.clone(),
                m.role.clone(),
                format!(
                    "{}k ctx ({}) · {} out ({})",
                    m.context_window / 1024,
                    m.context_limit_source,
                    m.max_output_tokens,
                    m.output_limit_source
                ),
                if m.base_url.is_empty() {
                    "(default)".into()
                } else {
                    m.base_url.clone()
                },
            ]
        })
        .collect();
    Report::new("models").table(
        &["name", "provider", "model", "role", "limits", "base_url"],
        rows,
    )
}

/// Health-check every configured model (shared by CLI + TUI).
pub async fn models_health_report(app: &App) -> Result<Report> {
    let manager = nexus_models::ModelManager::from_config(&app.config)?;
    let health = manager.health_all().await;
    let mut r = Report::new("model health");
    for (name, kind, h) in health {
        let lat = h
            .latency_ms
            .map(|l| format!(" · {l}ms"))
            .unwrap_or_default();
        r = r.line_sev(
            format!("{name} ({kind}){lat} — {}", h.detail),
            if h.reachable { Sev::Ok } else { Sev::Err },
        );
    }
    Ok(r)
}

/// Load managed models map (exposed for tests and the TUI remove flow).
pub fn managed_models(app: &App) -> Result<BTreeMap<String, ModelConfig>> {
    nexus_core::config::Config::load_managed_models(&app.paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn custom_endpoint_validation() {
        let mut spec = CustomEndpointSpec {
            name: "my_api".into(),
            base_url: "http://127.0.0.1:9000/v1".into(),
            model: "m".into(),
            ..Default::default()
        };
        validate_custom_endpoint(&spec).expect("valid");
        spec.base_url = "not a url".into();
        assert!(validate_custom_endpoint(&spec).is_err());
        spec.base_url = "ftp://x/".into();
        assert!(validate_custom_endpoint(&spec).is_err());
        spec.base_url = "http://ok/v1".into();
        spec.name = "bad name!".into();
        assert!(validate_custom_endpoint(&spec).is_err());
    }

    #[test]
    fn endpoint_url_normalization_accepts_host_port_and_presets() {
        let mut spec = CustomEndpointSpec {
            protocol: "openai_compatible".into(),
            base_url: "gateway.example:8443".into(),
            use_tls: true,
            ..Default::default()
        };
        assert_eq!(
            normalize_custom_endpoint_url(&spec).expect("normalize"),
            "https://gateway.example:8443/v1"
        );
        spec.protocol = "ollama".into();
        spec.base_url = "10.0.0.8:11434".into();
        spec.use_tls = false;
        assert_eq!(
            normalize_custom_endpoint_url(&spec).expect("normalize"),
            "http://10.0.0.8:11434"
        );
        spec.base_url = "https://models.example/custom/v1/".into();
        assert_eq!(
            normalize_custom_endpoint_url(&spec).expect("normalize"),
            "https://models.example/custom/v1"
        );
    }

    #[tokio::test]
    async fn connection_test_discovers_models_before_model_selection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "remote-model"}]
            })))
            .mount(&server)
            .await;
        let spec = CustomEndpointSpec {
            protocol: "openai_compatible".into(),
            base_url: server.uri(),
            use_tls: false,
            model: String::new(),
            ..Default::default()
        };
        let report = test_custom_endpoint(&spec).await.expect("test");
        let text = report.to_plain_text();
        assert!(text.contains("remote-model"), "{text}");
    }

    #[test]
    fn sanitizes_model_names() {
        assert_eq!(
            sanitize_model_name("llama3.1:8b-instruct"),
            "llama3_1_8b_instruct"
        );
        assert_eq!(sanitize_model_name("///"), "model");
    }

    #[test]
    fn endpoint_fingerprint_drops_credentials_and_sensitive_query_values() {
        let first = endpoint_fingerprint(Some(
            "https://alice:secret@example.test/v1?api_key=first#fragment",
        ));
        let second =
            endpoint_fingerprint(Some("https://bob:different@example.test/v1?token=second"));
        assert_eq!(first, second);
        assert!(!first.contains("alice"));
        assert!(!first.contains("secret"));
        assert_eq!(first.len(), 64);
    }

    fn discovered_with(context: Option<usize>) -> DiscoveredModel {
        DiscoveredModel {
            id: "qwen3.5:9b".into(),
            size_bytes: None,
            family: None,
            parameter_size: None,
            quantization: None,
            display_name: None,
            description: None,
            context_window: context,
            max_output_tokens: None,
            context_limit_source: context.map(|_| LimitSource::ProviderMetadata),
            output_limit_source: None,
            reasoning: None,
        }
    }

    #[test]
    fn a_self_hosted_context_maximum_is_recorded_rather_than_requested() {
        let mut model = ModelConfig {
            provider: "ollama".into(),
            limit_mode: LimitMode::Auto,
            ..Default::default()
        };
        apply_effective_limits(&mut model, &discovered_with(Some(262_144)));
        assert_eq!(model.context_ceiling, Some(262_144));
        assert_eq!(
            model.context_window,
            nexus_core::config::SELF_HOSTED_DEFAULT_CONTEXT,
            "the server allocates this much before it answers",
        );
        assert_eq!(
            model.max_output_tokens,
            nexus_core::config::SELF_HOSTED_DEFAULT_OUTPUT,
            "1024 completion tokens truncates a local model for no saving",
        );
    }

    #[test]
    fn a_hosted_provider_still_gets_the_full_reported_window() {
        let mut model = ModelConfig {
            provider: "anthropic".into(),
            limit_mode: LimitMode::Auto,
            ..Default::default()
        };
        apply_effective_limits(&mut model, &discovered_with(Some(200_000)));
        assert_eq!(model.context_window, 200_000);
        assert_eq!(model.context_ceiling, None);
        assert_eq!(
            model.max_output_tokens, 1024,
            "output tokens are metered here; the conservative default stands",
        );
    }
}
