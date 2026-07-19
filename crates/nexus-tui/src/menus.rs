//! Builders that turn real service data into interactive overlays. No fake
//! rows: every entry is constructed from measured provider/goal/session state.

use crate::views::{
    LoadRequest, Menu, MenuItem, MenuSortDirection, SecretInput, SecretTarget, UiAction,
};
use nexus_agent::AgentRole;
use nexus_app::codex::CodexStatus;
use nexus_app::providers::{AuthRequirement, EndpointState, ProviderEntry};
use nexus_app::services::ResumeCandidate;
use nexus_core::brand::{self, BrandVariant};
use nexus_goals::Goal;

/// Menu-first compatibility surface for commands that do not yet have a
/// richer domain workspace. The default action explicitly bypasses menu
/// routing, so selecting it cannot recursively reopen this menu.
pub fn command_menu(def: &nexus_app::registry::CommandDef) -> Menu {
    let mut items = Vec::new();
    if !def.usage.trim_start().starts_with('<') {
        let mut run = MenuItem::new(
            format!("Run default · {}", def.summary),
            UiAction::RunDefaultCommand(def.name.into()),
        )
        .id(format!("command:{}:default", def.name))
        .category("actions");
        if def.requires_confirmation {
            run = run
                .badge("confirmation required")
                .detail("the real executor will show the full risk confirmation before applying");
        }
        items.push(run);
    }
    if !def.usage.is_empty() {
        items.push(
            MenuItem::new(
                format!("Advanced input · /{} {}", def.name, def.usage),
                UiAction::InsertInput(format!("/{} ", def.name)),
            )
            .id(format!("command:{}:advanced", def.name))
            .category("advanced")
            .detail("typed arguments remain an optional compatibility layer"),
        );
    }
    items.push(
        MenuItem::new(
            "Show command help",
            UiAction::RunDefaultCommand(format!("help {}", def.name)),
        )
        .id(format!("command:{}:help", def.name))
        .category("help"),
    );
    Menu::new(format!("/{} · {}", def.name, def.summary), items)
        .id(format!("command-menu:{}", def.name))
        .route(format!("/{}", def.name))
        .hint("Enter invokes real behavior · Esc close")
}

// ---------------------------------------------------------------------- goal

pub fn goal_menu(goals: &[Goal], active_goal: Option<&str>) -> Menu {
    let mut items = vec![MenuItem::new(
        "Create new goal",
        UiAction::Load(LoadRequest::GoalDetail("NEW".into())),
    )
    .detail("guided form: objective, criteria, budgets, paths")];
    if let Some(id) = active_goal {
        items.push(
            MenuItem::new(
                "View active goal",
                UiAction::RunCommand(format!("goal show {id}")),
            )
            .badge(id.to_string()),
        );
    }
    items.push(
        MenuItem::new("List goals", UiAction::Load(LoadRequest::Goals))
            .badge(format!("{}", goals.len())),
    );
    items.push(MenuItem::new(
        "Resume / recover",
        UiAction::Load(LoadRequest::Resume),
    ));
    if let Some(id) = active_goal {
        items.push(MenuItem::new(
            "Pause active goal",
            UiAction::RunCommand(format!("pause {id}")),
        ));
        items.push(MenuItem::new(
            "Cancel active goal",
            UiAction::RunCommand(format!("cancel {id}")),
        ));
        items.push(MenuItem::new(
            "Inspect plan / evidence",
            UiAction::RunCommand(format!("plan {id}")),
        ));
        items.push(MenuItem::new(
            "Verify acceptance criteria",
            UiAction::RunCommand(format!("goal verify {id}")),
        ));
        items.push(MenuItem::new(
            "Export as JSON",
            UiAction::RunCommand(format!("goal export {id}")),
        ));
    }
    Menu::new("goal", items)
        .route("/goal")
        .hint("Enter select · Esc close")
}

pub fn goals_menu(goals: &[Goal]) -> Menu {
    let items: Vec<MenuItem> = goals
        .iter()
        .map(|g| {
            MenuItem::new(
                g.title.clone(),
                UiAction::RunCommand(format!("goal show {}", g.id.as_str())),
            )
            .badge(g.status.as_str().to_string())
            .detail(format!(
                "{} · {}/{} steps · updated {}",
                g.id.as_str(),
                g.steps_used,
                g.step_budget,
                g.updated_at
            ))
        })
        .collect();
    let mut menu = Menu::new("goals", items).route("/goals").searchable();
    menu.hint = "type to filter · Enter opens · Esc close".into();
    menu.on_refresh = Some(UiAction::Load(LoadRequest::Goals));
    menu
}

// -------------------------------------------------------------------- resume

pub fn resume_menu(candidates: &[ResumeCandidate]) -> Menu {
    let items: Vec<MenuItem> = candidates
        .iter()
        .map(|c| {
            let action = match c.kind {
                "session" => UiAction::AttachSession(c.id.clone()),
                _ => UiAction::ResumeGoal(c.id.clone()),
            };
            MenuItem::new(format!("[{}] {}", c.kind, c.title), action)
                .badge(c.status.clone())
                .detail(format!(
                    "{} · model {} · {} · {}",
                    c.id, c.model, c.detail, c.last_activity
                ))
        })
        .collect();
    let mut menu = Menu::new("resume", items).route("/resume").searchable();
    menu.hint = "Enter resumes (completed side effects are never re-run) · Esc close".into();
    menu.on_refresh = Some(UiAction::Load(LoadRequest::Resume));
    menu
}

pub fn sessions_menu(sessions: &[nexus_agent::SessionMeta]) -> Menu {
    let items: Vec<MenuItem> = sessions
        .iter()
        .map(|s| {
            let title = if s.title.is_empty() {
                s.id.as_str().to_string()
            } else {
                s.title.clone()
            };
            MenuItem::new(title, UiAction::AttachSession(s.id.as_str().to_string()))
                .badge(s.status.clone())
                .detail(format!(
                    "{} · agent {} · model {} · updated {}",
                    s.id.as_str(),
                    s.agent,
                    s.model,
                    s.updated_at
                ))
        })
        .collect();
    let mut menu = Menu::new("sessions", items).route("/sessions").searchable();
    menu.hint = "Enter attaches the session · Esc close".into();
    menu.on_refresh = Some(UiAction::Load(LoadRequest::Sessions));
    menu
}

// ------------------------------------------------------------------- welcome

/// First-run onboarding menu, shown when no models are configured yet.
pub fn welcome_menu() -> Menu {
    let items = vec![
        MenuItem::new(
            "Run /setup (recommended)",
            UiAction::RunDefaultCommand("setup".into()),
        )
            .detail("detects local runtimes (Ollama / llama.cpp), your Codex session, and writes a starter config"),
        MenuItem::new("Connect a provider", UiAction::Load(LoadRequest::Login))
            .detail("sign in with ChatGPT (Codex), enter an API key, or point at a local server"),
        MenuItem::new(
            "Just look around",
            UiAction::RunDefaultCommand("help".into()),
        )
            .detail("keys and commands — you can run /setup any time"),
    ];
    let mut menu = Menu::new(format!("Welcome to {}", brand::MARK), items)
        .id("welcome")
        .route("/welcome")
        .branded(BrandVariant::Compact);
    menu.hint = "Enter select · Esc dismiss".into();
    menu
}

pub fn init_menu(plan: &nexus_app::services::InitPlan) -> Menu {
    let mut items = Vec::new();
    if let Some(source) = &plan.usable_source {
        items.push(
            MenuItem::new(
                format!("Use existing {source}"),
                UiAction::RunCommand("init preview".into()),
            )
            .badge("usable")
            .detail("empty or unreadable higher-priority files are skipped"),
        );
    } else {
        items.push(
            MenuItem::new(
                "Preview canonical AGENTS.md",
                UiAction::RunCommand("__init_preview".into()),
            )
            .detail(plan.target.display().to_string()),
        );
        items.push(
            MenuItem::new(
                "Write canonical AGENTS.md",
                UiAction::RunCommand("init write".into()),
            )
            .badge("confirmation required")
            .detail("never overwrites without an explicit confirmation"),
        );
    }
    if plan.git_repo {
        items.push(
            MenuItem::new(
                "Git repository detected",
                UiAction::RunCommand("branch status".into()),
            )
            .badge("ready")
            .detail("detected with git rev-parse"),
        );
    } else if plan.malformed_git_metadata {
        items.push(
            MenuItem::new(
                "Git metadata needs manual repair",
                UiAction::RunCommand("init preview".into()),
            )
            .disabled("NEXUS never deletes malformed `.git` metadata automatically"),
        );
    } else if plan.git_init_needed {
        items.push(
            MenuItem::new(
                "Initialize Git with branch main",
                UiAction::RunCommand("init git".into()),
            )
            .badge("confirmation required")
            .detail("runs git init in the exact invocation directory; avoids nested repos"),
        );
    }
    Menu::new("initialize project", items)
        .route("/init")
        .hint("Preview and confirm mutations · Esc close")
}

// --------------------------------------------------------------------- login

pub fn login_menu(entries: &[ProviderEntry]) -> Menu {
    let items: Vec<MenuItem> = entries
        .iter()
        .map(|e| {
            let action = login_action_for(e);
            let mut item = MenuItem::new(format!("{} {}", e.marker(), e.label), action)
                .badge(if e.local { "local" } else { "remote" }.to_string())
                .detail(provider_detail(e));
            if !e.implemented {
                item = item.disabled("not implemented in this build");
            }
            item
        })
        .collect();
    let mut menu = Menu::new(format!("{} · provider login", brand::MARK), items)
        .branded(BrandVariant::Compact)
        .id("provider-login")
        .route("/login")
        .searchable()
        .empty_message("no authentication providers are available");
    menu.hint = "Enter opens authentication · Ctrl+R refresh · Esc close".into();
    menu.on_refresh = Some(UiAction::Load(LoadRequest::Login));
    menu
}

/// Endpoint/runtime connections are separate from provider authentication.
/// Both views are sourced from the same provider catalog.
pub fn connect_menu(entries: &[ProviderEntry]) -> Menu {
    let mut items: Vec<MenuItem> = entries
        .iter()
        .filter(|entry| entry.local || entry.id.starts_with("custom:") || entry.endpoint.is_some())
        .map(|entry| {
            let endpoint = entry.endpoint.as_deref().unwrap_or("not configured");
            let mut item = MenuItem::new(
                format!("{} {}", entry.marker(), entry.label),
                UiAction::OpenProvider(entry.id.clone()),
            )
            .id(format!("connection:{}", entry.id))
            .category(if entry.local { "local" } else { "remote" })
            .badge(if entry.local { "runtime" } else { "endpoint" })
            .detail(format!("{} · {endpoint}", entry.auth_state));
            if !entry.implemented {
                item = item.disabled("not implemented in this build");
            }
            item
        })
        .collect();
    items.push(
        MenuItem::new(
            "＋ Custom endpoint…",
            UiAction::RunCommand("__custom_endpoint".into()),
        )
        .id("connection:create-custom")
        .category("custom")
        .detail("OpenAI-compatible, Ollama-compatible, or llama.cpp-compatible runtime"),
    );
    let mut menu = Menu::new(format!("{} · endpoint connections", brand::MARK), items)
        .branded(BrandVariant::Compact)
        .id("provider-connect")
        .route("/connect")
        .searchable()
        .sorted("label", MenuSortDirection::Ascending);
    menu.hint = "Enter inspect/test · Ctrl+R refresh · Esc close".into();
    menu.on_refresh = Some(UiAction::Load(LoadRequest::Connect));
    menu
}

fn login_action_for(e: &ProviderEntry) -> UiAction {
    match e.id.as_str() {
        // /login goes to the codex AUTH menu; /connect probes for models.
        "codex" => UiAction::RunCommand("__codex_auth".into()),
        "claude-plan" => UiAction::RunCommand("__claude_auth".into()),
        "ollama" | "llamacpp" => UiAction::ProbeProvider(e.id.clone()),
        id if e.auth == AuthRequirement::ApiKey => UiAction::ProbeProvider(id.to_string()),
        _ => UiAction::ProbeProvider(e.id.clone()),
    }
}

pub fn claude_menu(cli_installed: bool, consented: bool) -> Menu {
    let mut items = Vec::new();
    if cli_installed {
        items.push(
            MenuItem::new("Claude subscription login", UiAction::StartClaudeLogin)
                .detail("runs the official `claude auth login --claudeai` flow"),
        );
    } else {
        items.push(
            MenuItem::new("Claude subscription login", UiAction::StartClaudeLogin)
                .disabled("the official `claude` CLI is not installed"),
        );
    }
    if consented {
        items.push(
            MenuItem::new(
                "Revoke NEXUS access",
                UiAction::RunCommand("auth revoke-existing-claude".into()),
            )
            .badge("consented")
            .detail("does not sign the Claude CLI out"),
        );
    } else {
        items.push(
            MenuItem::new(
                "Allow NEXUS to use this login",
                UiAction::RunCommand("auth use-existing-claude".into()),
            )
            .detail("workspace-scoped consent; one-turn plan bridge; Claude tools disabled"),
        );
    }
    items.push(
        MenuItem::new(
            "Refresh provider status",
            UiAction::ProbeProvider("claude-plan".into()),
        )
        .detail("auth is inspected only after consent"),
    );
    Menu::new(
        format!("{} · Claude plan authentication", brand::MARK),
        items,
    )
    .branded(BrandVariant::Compact)
    .hint("Enter select · Esc back")
}

/// Provider row detail: summary plus the auth state, unless the summary
/// already says the same thing (probe reasons often repeat it).
fn provider_detail(e: &ProviderEntry) -> String {
    let summary = e.summary();
    if e.auth_state.is_empty() || summary.contains(&e.auth_state) {
        summary
    } else {
        format!("{summary} — {}", e.auth_state)
    }
}

/// The Codex authentication submenu — every entry reflects the measured
/// profile state; unsupported methods are simply absent.
pub fn codex_menu(status: &CodexStatus) -> Menu {
    let mut items = Vec::new();
    if !status.cli_installed {
        items.push(
            MenuItem::new("Device login", UiAction::StartDeviceLogin)
                .disabled("the `codex` CLI is not installed (npm i -g @openai/codex)"),
        );
    } else {
        items.push(
            MenuItem::new("Device login", UiAction::StartDeviceLogin)
                .detail("codex login --device-auth into the isolated NEXUS profile"),
        );
        items.push(
            MenuItem::new(
                "API key login",
                UiAction::RunCommand("__codex_api_key".into()),
            )
            .detail("stored via codex --with-api-key in the isolated profile"),
        );
    }
    match &status.existing {
        Some(p) => {
            items.push(
                MenuItem::new("Import existing Codex CLI login", UiAction::CodexImport)
                    .badge(p.mode.to_string())
                    .detail("copies auth.json into the isolated profile — the original is never modified"),
            );
            if status.active_source == Some(nexus_models::CodexSource::ExistingCli) {
                items.push(
                    MenuItem::new(
                        "Stop using existing login",
                        UiAction::RunCommand("auth revoke-existing".into()),
                    )
                    .badge("consented")
                    .detail("revokes read-only access for this workspace"),
                );
            } else {
                items.push(
                    MenuItem::new(
                        "Use existing login without copying",
                        UiAction::RunCommand("auth use-existing".into()),
                    )
                    .detail("asks for explicit read-only consent; never modifies the source"),
                );
            }
        }
        None => {
            items.push(
                MenuItem::new("Import existing Codex CLI login", UiAction::CodexImport)
                    .disabled("no existing Codex CLI login detected"),
            );
        }
    }
    items.push(
        MenuItem::new(
            "Inspect NEXUS Codex login",
            UiAction::RunCommand("auth status".into()),
        )
        .badge(match &status.isolated {
            Some(p) => format!("logged in ({})", p.mode),
            None => "not logged in".into(),
        }),
    );
    items.push(
        MenuItem::new(
            "Logout from NEXUS profile",
            UiAction::RunCommand("logout codex".into()),
        )
        .detail("removes only the isolated session; your codex CLI login stays"),
    );
    Menu::new(format!("{} · Codex authentication", brand::MARK), items)
        .branded(BrandVariant::Compact)
        .hint("Enter select · Esc back")
}

/// Secret-input overlay for a provider API key.
pub fn provider_key_input(provider: &str) -> SecretInput {
    SecretInput::new(
        format!("{provider} API key"),
        "stored in the restricted credential store; never shown or logged",
        SecretTarget::Provider(provider.to_string()),
    )
}

// --------------------------------------------------------------------- model

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ConfiguredModelCard {
    pub name: String,
    pub provider: String,
    pub model_id: String,
    pub capabilities: Option<nexus_models::ModelCapabilities>,
    pub availability: String,
}

#[allow(dead_code)]
fn capability_marker(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

#[allow(dead_code)]
fn configured_model_detail(card: &ConfiguredModelCard) -> String {
    let Some(capabilities) = card.capabilities.as_ref() else {
        return format!(
            "model {} · {} · capabilities unavailable until the provider is repaired",
            card.model_id, card.availability
        );
    };
    format!(
        "model {} · {} · context {} ({}) · output {} ({}) · tools {} · structured {} · schema {} · vision {} · streaming {} · {:?} · privacy {:?} · latency {:?} · cost {:?} · fallback {:?}",
        card.model_id,
        card.availability,
        capabilities.context_window,
        capabilities.context_limit_source,
        capabilities.max_output_tokens,
        capabilities.output_limit_source,
        capability_marker(capabilities.native_tool_calls),
        capability_marker(capabilities.structured_output),
        capability_marker(capabilities.json_schema),
        capability_marker(capabilities.image_input),
        capability_marker(capabilities.streaming),
        capabilities.locality,
        capabilities.privacy,
        capabilities.latency_class,
        capabilities.cost_class,
        capabilities.fallback_eligibility,
    )
}

/// The /model picker: every model available RIGHT NOW, across configured
/// entries, the codex plan (when connected), and reachable local runtimes.
/// Providers that need connecting first point at /connect instead of faking
/// availability.
#[allow(dead_code)]
pub fn model_menu(
    configured: &[ConfiguredModelCard],
    entries: &[ProviderEntry],
    active_model: &str,
) -> Menu {
    let mut items: Vec<MenuItem> = Vec::new();

    // Configured entries first — Enter pins routing (codex ones go through
    // the effort picker).
    for card in configured {
        let mark = if card.name == active_model {
            "●"
        } else {
            " "
        };
        let action = if card.provider == "codex" {
            UiAction::PickCodexEffort {
                model_id: card.model_id.clone(),
            }
        } else {
            UiAction::SelectModel(card.name.clone())
        };
        items.push(
            MenuItem::new(format!("{mark} {}", card.name), action)
                .id(format!("model:{}", card.name))
                .category(card.provider.clone())
                .badge(card.provider.clone())
                .detail(configured_model_detail(card)),
        );
    }

    // Codex plan models (already fetched/cached) not yet configured.
    let codex_connected = entries.iter().any(|e| e.id == "codex" && e.authenticated);
    if codex_connected {
        let plan = nexus_app::codex::cached_plan_models();
        for m in &plan {
            if configured
                .iter()
                .any(|card| card.provider == "codex" && card.model_id == m.id)
            {
                continue;
            }
            items.push(
                MenuItem::new(
                    format!("  {} ({})", m.display_name, m.id),
                    UiAction::PickCodexEffort {
                        model_id: m.id.clone(),
                    },
                )
                .badge(if m.is_default { "plan default" } else { "plan" })
                .detail(m.description.clone()),
            );
        }
        items.push(
            MenuItem::new(
                "  Refresh plan models…",
                UiAction::ProbeProvider("codex".into()),
            )
            .detail(if plan.is_empty() {
                "fetches the models on your ChatGPT plan"
            } else {
                "re-fetches the list (and effort options) from your plan"
            }),
        );
    }

    // Reachable local runtimes' models.
    for e in entries.iter().filter(|e| e.local) {
        if let EndpointState::Connected { models, .. } = &e.state {
            for m in models {
                if configured.iter().any(|card| card.model_id == m.id) {
                    continue;
                }
                items.push(
                    MenuItem::new(
                        format!("  {}", m.id),
                        if m.reasoning
                            .as_ref()
                            .is_some_and(|profile| !profile.supported_efforts.is_empty())
                        {
                            UiAction::PickDiscoveredEffort {
                                provider: e.id.clone(),
                                base_url: e.endpoint.clone().unwrap_or_default(),
                                model: m.clone(),
                            }
                        } else {
                            UiAction::UseDiscovered {
                                provider: e.id.clone(),
                                base_url: e.endpoint.clone().unwrap_or_default(),
                                model: m.clone(),
                                effort: None,
                            }
                        },
                    )
                    .badge(e.label.clone())
                    .detail(format!(
                        "discovered on {}",
                        nexus_app::providers::redacted_endpoint_identity(e.endpoint.as_deref())
                            .as_deref()
                            .unwrap_or("endpoint")
                    )),
                );
            }
        }
    }

    if items.is_empty() {
        items.push(
            MenuItem::new("Connect a provider…", UiAction::Load(LoadRequest::Login))
                .detail("no models available yet — /connect signs in or points at a runtime"),
        );
    }
    items.push(
        MenuItem::new(
            "＋ Custom endpoint…",
            UiAction::RunCommand("__custom_endpoint".into()),
        )
        .detail("OpenAI-compatible / Ollama-compatible / llama.cpp-compatible"),
    );
    items.push(
        MenuItem::new(
            "Use config routing (clear pin)",
            UiAction::RunCommand("model clear".into()),
        )
        .detail(format!("currently active: {active_model}")),
    );
    let mut menu = Menu::new("model — pick the active model", items)
        .route("/model")
        .searchable();
    menu.hint = "Enter select · Ctrl+R refresh · Esc close · /connect adds providers".into();
    menu.on_refresh = Some(UiAction::Load(LoadRequest::RefreshModel));
    menu
}

/// Provider-first `/model` root. Inventories remain cached until the operator
/// explicitly refreshes, so opening the picker never invents model rows.
pub fn model_provider_menu(entries: &[ProviderEntry], active_model: &str) -> Menu {
    let mut items: Vec<MenuItem> = entries
        .iter()
        .filter(|entry| entry.authenticated || !entry.configured_models.is_empty())
        .map(|entry| {
            MenuItem::new(
                format!("{} {}", entry.marker(), entry.label),
                UiAction::ProbeProvider(entry.id.clone()),
            )
            .id(format!("model-provider:{}", entry.id))
            .category(if entry.local { "local" } else { "remote" })
            .detail(entry.summary())
        })
        .collect();
    items.push(
        MenuItem::new(
            "Refresh all providers",
            UiAction::Load(LoadRequest::RefreshModel),
        )
        .detail("concurrently refresh configured endpoints and authenticated providers"),
    );
    items.push(
        MenuItem::new(
            "Use config routing (clear pin)",
            UiAction::RunCommand("model clear".into()),
        )
        .detail(format!("currently active: {active_model}")),
    );
    Menu::new("model — choose provider", items)
        .route("/model")
        .searchable()
        .hint("Enter provider · then model, effort, and Use/Test · Ctrl+R refresh all")
}

/// Reasoning-effort picker for one codex plan model.
pub fn effort_menu(model: &nexus_app::codex::PlanModel) -> Menu {
    let default = model.default_reasoning_effort.as_deref();
    let items: Vec<MenuItem> = model
        .reasoning_efforts
        .iter()
        .map(|e| {
            let mut item = MenuItem::new(
                e.effort.clone(),
                UiAction::UseCodexModel {
                    model_id: model.id.clone(),
                    effort: Some(e.effort.clone()),
                },
            )
            .detail(e.description.clone());
            if default == Some(e.effort.as_str()) {
                item = item.badge("default");
            }
            item
        })
        .collect();
    let mut menu = Menu::new(format!("{} — reasoning effort", model.display_name), items)
        .parent("/model", UiAction::Load(LoadRequest::Model));
    menu.hint = "Enter selects model + effort · Esc back".into();
    menu
}

pub fn discovered_effort_menu(
    provider: &str,
    base_url: &str,
    model: &nexus_models::DiscoveredModel,
) -> Menu {
    let profile = model.reasoning.clone().unwrap_or_default();
    let items = profile
        .supported_efforts
        .iter()
        .map(|effort| {
            let mut item = MenuItem::new(
                effort.clone(),
                UiAction::UseDiscovered {
                    provider: provider.into(),
                    base_url: base_url.into(),
                    model: model.clone(),
                    effort: Some(effort.clone()),
                },
            );
            if profile.default_effort.as_deref() == Some(effort) {
                item = item.badge("default");
            }
            item.detail(format!("reasoning metadata: {:?}", profile.provenance))
        })
        .collect();
    Menu::new(format!("{} — reasoning effort", model.id), items)
        .parent("/model", UiAction::Load(LoadRequest::Model))
        .hint("Enter continues to Use/Test actions · Esc back")
}

/// Submenu for one probed provider: its models and setup actions.
pub fn provider_menu(
    entry: &ProviderEntry,
    configured: &[(String, String)], // (config entry name, model id)
) -> Menu {
    let mut items = Vec::new();

    // Configured entries first (selectable immediately).
    for (name, model_id) in configured {
        items.push(
            MenuItem::new(
                format!("Select {name}"),
                UiAction::SelectModel(name.clone()),
            )
            .badge("configured")
            .detail(format!("model {model_id} — pins routing to this entry")),
        );
        items.push(
            MenuItem::new(
                format!("Test {name}"),
                UiAction::RunCommand(format!("__model_test {name}")),
            )
            .detail("minimal safe prompt; reports first-token and total latency"),
        );
    }

    // Discovered (unconfigured) models: endpoint probe results, or for Codex
    // the models on the operator's plan.
    if let EndpointState::Connected { models, latency_ms }
    | EndpointState::Stale {
        models, latency_ms, ..
    } = &entry.state
    {
        let plan_default = if entry.id == "codex" {
            nexus_app::codex::cached_default_model()
        } else {
            None
        };
        for m in models {
            let already = configured.iter().any(|(_, id)| id == &m.id);
            if already {
                continue;
            }
            let mut detail = if let Some(desc) = &m.description {
                desc.clone()
            } else {
                format!(
                    "discovered on {} ({latency_ms}ms)",
                    entry.endpoint.as_deref().unwrap_or("endpoint")
                )
            };
            if let Some(size) = m.size_bytes.map(nexus_models::human_size) {
                detail = format!("{size} · {detail}");
            }
            if let Some(q) = &m.quantization {
                detail = format!("{detail} · {q}");
            }
            let label = match &m.display_name {
                Some(name) => format!("Use {name} ({})", m.id),
                None => format!("Use {}", m.id),
            };
            let action = if entry.id == "codex" {
                UiAction::PickCodexEffort {
                    model_id: m.id.clone(),
                }
            } else if m
                .reasoning
                .as_ref()
                .is_some_and(|profile| !profile.supported_efforts.is_empty())
            {
                UiAction::PickDiscoveredEffort {
                    provider: entry.id.clone(),
                    base_url: entry.endpoint.clone().unwrap_or_default(),
                    model: m.clone(),
                }
            } else {
                UiAction::UseDiscovered {
                    provider: entry.id.clone(),
                    base_url: entry.endpoint.clone().unwrap_or_default(),
                    model: m.clone(),
                    effort: None,
                }
            };
            items.push(
                MenuItem::new(label, action)
                    .badge(if plan_default.as_deref() == Some(m.id.as_str()) {
                        "plan default"
                    } else {
                        "discovered"
                    })
                    .detail(detail),
            );
        }
    }

    // Setup / auth actions based on measured state.
    match (&entry.state, &entry.auth) {
        (EndpointState::Unreachable(reason), _) => {
            items.push(
                MenuItem::new(
                    "Retry connection",
                    UiAction::ProbeProvider(entry.id.clone()),
                )
                .detail(reason.clone()),
            );
            if entry.id == "ollama" {
                items.push(
                    MenuItem::new(
                        "View startup instructions",
                        UiAction::RunCommand("__ollama_help".into()),
                    )
                    .detail("NEXUS never installs or starts Ollama itself"),
                );
            }
            if entry.id == "llamacpp" {
                items.push(
                    MenuItem::new(
                        "View startup instructions",
                        UiAction::RunCommand("__llamacpp_help".into()),
                    )
                    .detail("NEXUS never owns the llama.cpp process"),
                );
            }
            if entry.id == "codex" {
                items.push(
                    MenuItem::new(
                        "Login / authentication…",
                        UiAction::RunCommand("__codex_auth".into()),
                    )
                    .detail("listing plan models needs a NEXUS Codex login"),
                );
            } else {
                items.push(
                    MenuItem::new(
                        "Configure endpoint…",
                        UiAction::RunCommand("__custom_endpoint".into()),
                    )
                    .detail("point NEXUS at a different host/port"),
                );
            }
        }
        (_, AuthRequirement::ApiKey) if !entry.authenticated => {
            items.push(
                MenuItem::new(
                    "Enter API key",
                    UiAction::RunCommand(format!("__provider_key {}", entry.id)),
                )
                .detail(entry.auth_state.clone()),
            );
        }
        (_, AuthRequirement::DeviceLoginOrApiKey) if !entry.authenticated => {
            items.push(
                MenuItem::new(
                    "Login to provider",
                    UiAction::RunCommand("__codex_auth".into()),
                )
                .detail("device login available"),
            );
        }
        (_, AuthRequirement::SubscriptionLogin) if !entry.authenticated => {
            items.push(
                MenuItem::new(
                    "Claude login / consent…",
                    UiAction::RunCommand("__claude_auth".into()),
                )
                .detail(entry.auth_state.clone()),
            );
        }
        _ => {}
    }
    if entry.id == "codex" && entry.authenticated {
        items.push(
            MenuItem::new(
                "Authentication…",
                UiAction::RunCommand("__codex_auth".into()),
            )
            .detail("device login, API key, import, logout (isolated profile)"),
        );
    }
    if entry.id == "claude-plan" && entry.authenticated {
        items.push(
            MenuItem::new(
                "Authentication…",
                UiAction::RunCommand("__claude_auth".into()),
            )
            .detail("subscription login and workspace consent"),
        );
    }

    items.push(
        MenuItem::new("Refresh models", UiAction::ProbeProvider(entry.id.clone()))
            .detail("re-probe the endpoint"),
    );

    let mut menu = Menu::new(format!("{} — {}", entry.label, entry.summary()), items);
    menu.hint = "Enter select · Esc back · models pin routing when selected".into();
    menu
}

// -------------------------------------------------------------------- agents

pub fn agents_menu(active: &str, custom: &[nexus_agent::CustomAgentDefinition]) -> Menu {
    let mut items: Vec<MenuItem> = AgentRole::all()
        .iter()
        .map(|r| {
            let marker = if r.as_str() == active { "●" } else { " " };
            MenuItem::new(
                format!("{marker} {}", r.as_str()),
                UiAction::RunCommand(format!("agent {}", r.as_str())),
            )
            .badge(
                if r.can_write() {
                    "read-write"
                } else {
                    "read-only"
                }
                .to_string(),
            )
            .detail(r.output_contract().chars().take(90).collect::<String>())
        })
        .collect();
    items.extend(custom.iter().map(|definition| {
        let marker = if definition.name == active {
            "●"
        } else {
            " "
        };
        MenuItem::new(
            format!("{marker} {}", definition.name),
            UiAction::RunCommand(format!("agent {}", definition.name)),
        )
        .badge(format!("custom · {}", definition.scope))
        .detail(format!(
            "inherits {} · {} · {}",
            definition.base,
            if definition.can_write().unwrap_or(false) {
                "read-write"
            } else {
                "read-only"
            },
            definition.description
        ))
    }));
    Menu::new("agents", items)
        .route("/agent")
        .hint("Enter sets the active agent for new sessions · Esc close")
}

pub fn personas_menu(personas: &[nexus_memory::PersonaRecord], selected: Option<&str>) -> Menu {
    let mut items = vec![
        MenuItem::new(
            "Create persona…",
            UiAction::InsertInput("/persona create name instructions".into()),
        )
        .detail("edit the inserted command; personas cannot override safety or project rules"),
        MenuItem::new(
            "Clear persona",
            UiAction::RunCommand("persona select none".into()),
        ),
    ];
    items.extend(personas.iter().map(|persona| {
        MenuItem::new(
            format!(
                "{} {}",
                if selected == Some(persona.id.as_str()) {
                    "●"
                } else {
                    " "
                },
                persona.name
            ),
            UiAction::RunCommand(format!("persona select {}", persona.id)),
        )
        .badge(persona.scope.clone())
        .detail(format!(
            "{}{}",
            persona.description,
            persona
                .parent_id
                .as_ref()
                .map(|parent| format!(" · inherits {parent}"))
                .unwrap_or_default()
        ))
    }));
    Menu::new("persona", items)
        .route("/persona")
        .branded(BrandVariant::Compact)
        .searchable()
        .hint("Enter selects for new sessions · create/clone/edit/delete also work as commands")
}

/// Structured profile cards backed by the canonical harness repository.
pub fn profile_cards_menu(
    profiles: &[(nexus_core::harness::UserProfile, usize, usize)],
    active_profile_id: Option<&str>,
    pending_conflicts: usize,
) -> Menu {
    let mut items = vec![MenuItem::new(
        "Create profile card…",
        UiAction::InsertInput("/profile select new-name".into()),
    )
    .id("profile:create")
    .category("actions")
    .detail("creates a separate profile; existing people are never silently merged")];
    items.extend(profiles.iter().map(|(profile, fact_count, memory_count)| {
        let selected = active_profile_id == Some(profile.id.as_str());
        MenuItem::new(
            format!(
                "{} {}",
                if selected { "●" } else { " " },
                profile
                    .preferred_name
                    .as_deref()
                    .unwrap_or(&profile.display_name)
            ),
            UiAction::SelectHarnessProfile(profile.id.clone()),
        )
        .id(profile.id.clone())
        .category("profiles")
        .badge(format!("{:?}", profile.status).to_ascii_lowercase())
        .detail(format!(
            "{} fact(s) · {} linked memory record(s) · aliases {} · isolation explicit",
            fact_count,
            memory_count,
            if profile.aliases.is_empty() {
                "none".into()
            } else {
                profile.aliases.join(", ")
            }
        ))
    }));
    Menu::new(
        format!("profile cards · {pending_conflicts} conflict(s) pending"),
        items,
    )
    .id("profile-dashboard")
    .route("/profile")
    .branded(BrandVariant::Compact)
    .searchable()
    .hint("/ search · Enter action · Tab details · ? controls · Esc close")
}

pub fn memory_dashboard(records: &[nexus_core::harness::MemoryRecord]) -> Menu {
    let mut items = Vec::new();
    items.extend(records.iter().map(|record| {
        MenuItem::new(
            record.content.lines().next().unwrap_or("(empty memory)"),
            UiAction::ShowHarnessMemory(Box::new(record.clone())),
        )
        .id(record.id.clone())
        .category(format!("{:?}", record.memory_type).to_ascii_lowercase())
        .badge(format!("{:?}", record.status).to_ascii_lowercase())
        .detail(format!(
            "scope {} · source {:?} · sensitivity {} · confidence {:.0}% · importance {:.0}% · created {}{}",
            memory_scope_label(&record.scope),
            record.source_type,
            record.sensitivity,
            record.confidence * 100.0,
            record.importance * 100.0,
            record.created_at,
            record
                .expires_at
                .as_ref()
                .map(|expiry| format!(" · expires {expiry}"))
                .unwrap_or_default()
        ))
    }));
    Menu::new("memory dashboard", items)
        .id("memory-dashboard")
        .route("/memory")
        .searchable()
        .hint("/ search · Enter details/action · Space select · Ctrl+R refresh · Esc close")
}

fn memory_scope_label(scope: &nexus_core::harness::MemoryScope) -> String {
    if scope.global {
        return "global".into();
    }
    [
        ("profile", scope.profile_id.as_deref()),
        ("workspace", scope.workspace_id.as_deref()),
        ("project", scope.project_id.as_deref()),
        ("session", scope.session_id.as_deref()),
        ("goal", scope.goal_id.as_deref()),
        ("plan", scope.plan_id.as_deref()),
        ("task", scope.task_id.as_deref()),
        ("agent", scope.agent_id.as_deref()),
    ]
    .into_iter()
    .filter_map(|(kind, value)| value.map(|value| format!("{kind}:{value}")))
    .collect::<Vec<_>>()
    .join(", ")
}

pub fn plan_workspace(
    work: Option<&nexus_core::orchestration::WorkBreakdown>,
    has_session: bool,
) -> Menu {
    let create = MenuItem::new(
        "Create plan from objective…",
        UiAction::InsertInput("/plan create ".into()),
    )
    .id("plan:create")
    .category("actions");
    let create = if has_session {
        create
    } else {
        create.disabled("start or attach a session before creating a plan")
    };
    let revise = MenuItem::new(
        "Generate revised proposal…",
        UiAction::InsertInput("/plan replan ".into()),
    )
    .id("plan:revise")
    .category("actions");
    let revise = if has_session {
        revise
    } else {
        revise.disabled("start or attach a session before revising a plan")
    };
    let mut items = vec![
        create,
        revise,
        MenuItem::new("Open task graph", UiAction::RunCommand("task".into()))
            .id("plan:tasks")
            .category("work"),
    ];
    if let Some(work) = work {
        items.insert(
            0,
            MenuItem::new(
                format!("Active plan {} v{}", work.id, work.version),
                UiAction::RunCommand("plan verify".into()),
            )
            .id(work.id.as_str())
            .category("plan")
            .badge(if work.paused {
                "paused"
            } else if work.approved {
                "approved"
            } else {
                "under review"
            })
            .detail(format!(
                "{} · current phase {} · next {} · {} stage(s)",
                work.objective,
                work.current_stage.as_deref().unwrap_or("not started"),
                work.next_stage.as_deref().unwrap_or("none"),
                work.stages.len()
            )),
        );
        items.push(
            MenuItem::new(
                if work.approved {
                    "Start / continue execution"
                } else {
                    "Review and approve plan"
                },
                UiAction::RunCommand("plan approve".into()),
            )
            .id("plan:approve")
            .category("approval"),
        );
        items.push(
            MenuItem::new(
                if work.paused {
                    "Resume plan"
                } else {
                    "Pause plan"
                },
                UiAction::RunCommand(if work.paused {
                    "plan resume".into()
                } else {
                    "plan pause".into()
                }),
            )
            .id("plan:pause")
            .category("controls"),
        );
        items.push(
            MenuItem::new(
                "Revision history",
                UiAction::RunCommand("plan history".into()),
            )
            .id("plan:history")
            .category("inspection"),
        );
        items.push(
            MenuItem::new("Export plan", UiAction::RunCommand("plan export".into()))
                .id("plan:export")
                .category("inspection"),
        );
    }
    Menu::new("planning workspace", items)
        .id("plan-workspace")
        .route("/plan")
        .searchable()
        .hint("Review assumptions, phases, risks, gates, and rollback before approval")
}

pub fn tasks_menu(tasks: &[nexus_core::orchestration::BackgroundTask], has_session: bool) -> Menu {
    let create_reader = MenuItem::new(
        "Create read-only task…",
        UiAction::InsertInput("/task create reader title objective".into()),
    )
    .id("task:create-reader")
    .category("actions");
    let create_reader = if has_session {
        create_reader
    } else {
        create_reader.disabled("start or attach a session before creating a task")
    };
    let create_writer = MenuItem::new(
        "Create writer task…",
        UiAction::InsertInput("/task create writer title objective".into()),
    )
    .id("task:create-writer")
    .category("actions")
    .detail("writer tasks require confirmation and an isolated worktree");
    let create_writer = if has_session {
        create_writer
    } else {
        create_writer.disabled("start or attach a session before creating a task")
    };
    let manage = MenuItem::new(
        "Manage task by id…",
        UiAction::InsertInput("/task show ".into()),
    )
    .id("task:manage")
    .category("actions")
    .detail("pause, resume, retry, cancel, validate, or inspect artifacts");
    let manage = if has_session {
        manage
    } else {
        manage.disabled("start or attach a session before managing tasks")
    };
    let mut items = vec![create_reader, create_writer, manage];
    items.extend(tasks.iter().map(|task| {
        MenuItem::new(
            task.title.clone(),
            UiAction::RunCommand(format!("task show {}", task.id)),
        )
        .id(task.id.as_str())
        .category(task.status.as_str())
        .badge(if task.writer { "writer" } else { "reader" })
        .detail(format!(
            "{} · owner {} · attempts {} · plan {} · stage {}",
            task.status.as_str(),
            task.owner,
            task.attempts,
            task.plan_id.as_deref().unwrap_or("none"),
            task.stage_id.as_deref().unwrap_or("none")
        ))
    }));
    Menu::new("task graph", items)
        .id("task-graph")
        .route("/task")
        .searchable()
        .hint("/ search · Enter inspect · dependency and write-conflict checks are enforced")
}

pub fn subagents_menu(runs: &[nexus_core::orchestration::AgentRun], has_session: bool) -> Menu {
    let create = MenuItem::new(
        "Create bounded subagent…",
        UiAction::InsertInput("/subagents spawn role assignment".into()),
    )
    .id("subagent:create")
    .category("actions")
    .detail("configure role, context, model, tools, budgets, and output contract");
    let create = if has_session {
        create
    } else {
        create.disabled("start or attach a session before creating a subagent")
    };
    let manage = MenuItem::new(
        "Inspect or steer by id…",
        UiAction::InsertInput("/subagents show ".into()),
    )
    .id("subagent:manage")
    .category("actions");
    let manage = if has_session {
        manage
    } else {
        manage.disabled("start or attach a session before managing subagents")
    };
    let mut items = vec![create, manage];
    items.extend(runs.iter().map(|run| {
        MenuItem::new(
            format!("{} · {}", run.role, run.objective),
            UiAction::RunCommand(format!("subagents show {}", run.id)),
        )
        .id(run.id.as_str())
        .category(run.status.as_str())
        .badge(format!("depth {}", run.depth))
        .detail(format!(
            "model {} · task {} · unread events {} · budget {} action(s)",
            run.model,
            run.task_id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "none".into()),
            run.unread_events,
            run.budget
                .max_actions
                .map_or_else(|| "unbounded".into(), |limit| limit.to_string())
        ))
    }));
    Menu::new("subagent control", items)
        .id("subagent-control")
        .route("/subagents")
        .searchable()
        .hint("Results require review before promotion; circular delegation is rejected")
}

// --------------------------------------------------------- permissions & sandbox

/// `/permissions` — approval presets, most restrictive first. Full access is
/// explicit about what it stops asking for; destructive actions always ask.
pub fn permissions_menu(active_mode: &str) -> Menu {
    let mut items: Vec<MenuItem> = nexus_app::services::PERMISSION_MODES
        .iter()
        .map(|(mode, description)| {
            let marker = if *mode == active_mode { "●" } else { " " };
            let mut item = MenuItem::new(
                format!("{marker} {mode}"),
                UiAction::RunCommand(format!("permissions {mode}")),
            )
            .detail((*description).to_string());
            if *mode == "full-access" {
                item = item.badge("no prompts".to_string());
            }
            item
        })
        .collect();
    if active_mode == "custom" {
        items.push(
            MenuItem::new("● custom", UiAction::RunCommand("permissions show".into()))
                .detail("hand-edited policy — Enter shows the effective decisions".to_string()),
        );
    }
    items.push(
        MenuItem::new(
            "  File read access…",
            UiAction::Load(LoadRequest::ReadFormats),
        )
        .detail("per-format allow, ask, deny rules; workspace scope by default"),
    );
    items.push(
        MenuItem::new(
            "  Show effective policy…",
            UiAction::RunCommand("permissions show".into()),
        )
        .detail("every decision, allow/deny lists, and denied paths".to_string()),
    );
    Menu::new("permissions — approval mode", items)
        .route("/permissions")
        .hint("Enter applies and persists · destructive actions always ask · Esc close")
}

pub fn read_formats_menu(policy: &nexus_core::config::PolicyConfig) -> Menu {
    const FORMATS: &[&str] = &[
        "rust",
        "toml",
        "silent",
        "python",
        "javascript",
        "typescript",
        "go",
        "jvm",
        "c_cpp",
        "ruby",
        "php",
        "shell",
        "markup",
        "web",
        "json",
        "yaml",
        "xml",
        "configuration",
        "data",
        "sql",
        "text",
        "document",
        "image",
        "media",
        "archive",
        "binary",
        "special",
        "other",
    ];
    let mut items = vec![
        MenuItem::new(
            "🔒 Sensitive environment and credential files",
            UiAction::RunCommand("permissions show".into()),
        )
        .disabled("hard safety denial — Full Access cannot unlock these"),
        MenuItem::new(
            "🔒 .git and .nexus",
            UiAction::RunCommand("permissions show".into()),
        )
        .disabled("hard safety denial — internal state is never model-readable"),
    ];
    for format in FORMATS {
        let current = policy
            .read_formats
            .get(*format)
            .map(String::as_str)
            .unwrap_or(policy.reads.as_str());
        let next = match current {
            "allow" => "ask",
            "ask" => "deny",
            _ => "allow",
        };
        items.push(
            MenuItem::new(
                format!("{format} · {current}"),
                UiAction::RunCommand(format!("permissions format {format} {next}")),
            )
            .badge(if policy.read_formats.contains_key(*format) {
                "workspace/effective"
            } else {
                "inherited fallback"
            })
            .detail(format!("Enter changes workspace rule to {next}")),
        );
    }
    Menu::new("file read access", items)
        .route("/config/permissions/read-formats")
        .searchable()
        .hint("Enter cycles workspace rule · use /permissions format FORMAT VALUE global for global defaults")
}

/// `/sandbox` — isolation backend and network mode, with a self-test row.
pub fn sandbox_menu(cfg: &nexus_core::config::SandboxConfig) -> Menu {
    let enabled = cfg.backend != "none";
    let mut items: Vec<MenuItem> = Vec::new();
    if enabled {
        items.push(
            MenuItem::new(
                "  Disable sandbox",
                UiAction::RunCommand("sandbox off".into()),
            )
            .badge("runs unisolated".to_string())
            .detail("agent commands run directly on this machine".to_string()),
        );
    } else {
        items.push(
            MenuItem::new(
                "  Enable sandbox",
                UiAction::RunCommand("sandbox on".into()),
            )
            .detail("picks the strongest available backend (auto)".to_string()),
        );
    }
    for (backend, description) in [
        ("auto", "strongest available backend"),
        ("container", "container isolation (needs a runtime)"),
        ("process", "process-level restrictions"),
    ] {
        let marker = if cfg.backend == backend { "●" } else { " " };
        items.push(
            MenuItem::new(
                format!("{marker} backend {backend}"),
                UiAction::RunCommand(format!("sandbox backend {backend}")),
            )
            .detail(description.to_string()),
        );
    }
    for (mode, description) in [
        ("off", "no network inside the sandbox"),
        ("restricted", "allowlisted destinations only"),
        ("full", "unrestricted network inside the sandbox"),
    ] {
        let marker = if cfg.network == mode { "●" } else { " " };
        items.push(
            MenuItem::new(
                format!("{marker} network {mode}"),
                UiAction::RunCommand(format!("sandbox network {mode}")),
            )
            .detail(description.to_string()),
        );
    }
    items.push(
        MenuItem::new(
            "  Run self-test",
            UiAction::RunCommand("sandbox test".into()),
        )
        .detail("executes a probe command inside the sandbox".to_string()),
    );
    items.push(
        MenuItem::new(
            "  Show details…",
            UiAction::RunCommand("sandbox show".into()),
        )
        .detail("backend availability, isolation level, and caveats".to_string()),
    );
    let title = if enabled {
        "sandbox — active"
    } else {
        "sandbox — DISABLED"
    };
    Menu::new(title, items)
        .route("/sandbox")
        .hint("Enter applies and persists · Esc close")
}

// --------------------------------------------------------------------- theme

pub fn theme_menu(active: &str) -> Menu {
    let items: Vec<MenuItem> = nexus_app::theme_names()
        .iter()
        .map(|name| {
            let marker = if *name == active { "●" } else { " " };
            MenuItem::new(
                format!("{marker} {name}"),
                UiAction::SetTheme(name.to_string()),
            )
        })
        .collect();
    Menu::new("theme", items)
        .route("/theme")
        .hint("Enter applies immediately and persists · Esc close")
}

pub fn thinking_menu(enabled: bool) -> Menu {
    let items = vec![
        MenuItem::new(
            format!("{} enabled", if enabled { "●" } else { " " }),
            UiAction::RunCommand("thinking on".into()),
        )
        .detail("show provider reasoning summaries and operational traces"),
        MenuItem::new(
            format!("{} disabled", if enabled { " " } else { "●" }),
            UiAction::RunCommand("thinking off".into()),
        )
        .detail("show final answers, approvals, and errors only"),
    ];
    Menu::new("thinking visibility", items)
        .route("/thinking")
        .hint("Hidden chain-of-thought is never requested or displayed · Enter applies · Esc close")
}

pub fn details_menu(active: nexus_core::timeline::TranscriptDetail) -> Menu {
    let items = [
        (
            "compact",
            nexus_core::timeline::TranscriptDetail::Compact,
            "titles, status, risk, duration, path/command, and short result",
        ),
        (
            "expanded",
            nexus_core::timeline::TranscriptDetail::Expanded,
            "sanitized arguments, output previews, stages, and artifacts",
        ),
        (
            "raw",
            nexus_core::timeline::TranscriptDetail::Raw,
            "complete redacted event JSON",
        ),
    ]
    .into_iter()
    .map(|(name, value, detail)| {
        MenuItem::new(
            format!("{} {name}", if value == active { "●" } else { " " }),
            UiAction::RunCommand(format!("details {name}")),
        )
        .detail(detail)
    })
    .collect();
    Menu::new("timeline details", items)
        .route("/details")
        .hint("Enter applies · Esc close")
}

pub fn transcript_menu(active: nexus_core::timeline::TranscriptFilter) -> Menu {
    let items = [
        ("all", nexus_core::timeline::TranscriptFilter::All),
        ("messages", nexus_core::timeline::TranscriptFilter::Messages),
        ("plans", nexus_core::timeline::TranscriptFilter::Plans),
        ("tools", nexus_core::timeline::TranscriptFilter::Tools),
        ("diffs", nexus_core::timeline::TranscriptFilter::Diffs),
        ("agents", nexus_core::timeline::TranscriptFilter::Agents),
        ("warnings", nexus_core::timeline::TranscriptFilter::Warnings),
        ("errors", nexus_core::timeline::TranscriptFilter::Errors),
    ]
    .into_iter()
    .map(|(name, value)| {
        MenuItem::new(
            format!("{} {name}", if value == active { "●" } else { " " }),
            UiAction::RunCommand(format!("transcript {name}")),
        )
    })
    .collect();
    Menu::new("timeline filter", items)
        .route("/transcript")
        .hint("Enter applies · Esc close")
}

pub fn config_menu() -> Menu {
    Menu::new(
        "configuration hub",
        vec![
            MenuItem::new("General / UI…", UiAction::InsertInput("/config set workspace general.theme \"nexus-dark\"".into()))
                .detail("typed theme, color, motion, default agent, and test command; edit the inserted path/value"),
            MenuItem::new("Agents / routing…", UiAction::InsertInput("/config set workspace routing.coding \"model-name\"".into()))
                .detail("simple, coding, planning, and fallback routes; reset inherits"),
            MenuItem::new("Catalog", UiAction::RunCommand("catalog".into()))
                .detail("provider/model selection and endpoint health"),
            MenuItem::new("Permissions", UiAction::Load(LoadRequest::Permissions))
                .detail("read-only, default, auto-edit, full-access"),
            MenuItem::new("Sandbox", UiAction::Load(LoadRequest::Sandbox))
                .detail("backend, isolation, and network mode"),
            MenuItem::new("Memory", UiAction::Load(LoadRequest::Memory))
                .detail("approved durable project/global memory"),
            MenuItem::new("Web…", UiAction::InsertInput("/config set workspace web.enabled true".into()))
                .detail("typed web enablement and safe limits; weakening changes require confirmation"),
            MenuItem::new("Budgets…", UiAction::InsertInput("/config set workspace limits.max_steps_per_turn 24".into()))
                .detail("turn, token, cost, runtime, memory, and delegation budgets"),
            MenuItem::new("MCP…", UiAction::Load(LoadRequest::Mcp))
                .detail("servers, transport, trust, timeouts, and allowlisted environment names"),
            MenuItem::new("Inherit / reset override…", UiAction::InsertInput("/config reset workspace general.theme".into()))
                .detail("workspace or global scope; removes only the managed override"),
            MenuItem::new("Persona", UiAction::Load(LoadRequest::Persona))
                .detail("behavior cards; never override safety"),
            MenuItem::new("Profile", UiAction::Load(LoadRequest::Profile))
                .detail("approved workflow preferences and review queue"),
            MenuItem::new("Connectors", UiAction::Load(LoadRequest::Connector))
                .detail("Codex MCP and Agent Skill imports; disabled/untrusted by default"),
            MenuItem::new("Theme", UiAction::Load(LoadRequest::Theme))
                .detail("semantic terminal palette"),
            MenuItem::new("Thinking visibility", UiAction::Load(LoadRequest::Thinking))
                .detail("reasoning summaries and operational traces"),
            MenuItem::new(
                "Advanced / provenance…",
                UiAction::RunCommand("config advanced".into()),
            )
            .detail("effective layered configuration and managed file paths"),
        ],
    )
    .route("/config")
    .branded(BrandVariant::Compact)
    .hint("Edit typed path/value · workspace|global scope · reset inherits · hand-written config is preserved")
}

pub fn connectors_menu(candidates: &[nexus_app::connectors::ConnectorCandidate]) -> Menu {
    let items = candidates
        .iter()
        .map(|candidate| {
            let quoted = format!("'{}'", candidate.id.replace('\'', "'\\''"));
            let mut detail = format!(
                "{} · {}",
                candidate.source.display(),
                candidate.preview.lines().next().unwrap_or("")
            );
            if let Some(note) = &candidate.credential_note {
                detail.push_str(&format!(" · {note}"));
            }
            MenuItem::new(
                format!("{} · {}", candidate.kind, candidate.name),
                UiAction::RunCommand(format!("connector import {quoted}")),
            )
            .badge("disabled/untrusted")
            .detail(detail)
        })
        .collect();
    Menu::new("connector catalog", items)
        .route("/connector")
        .branded(BrandVariant::Compact)
        .searchable()
        .hint("Enter previews confirmation · credentials require a separate consented import")
}

pub fn branches_menu(branches: &[nexus_app::gitx::BranchInfo]) -> Menu {
    let mut items = vec![
        MenuItem::new("Git status", UiAction::RunCommand("branch status".into())),
        MenuItem::new(
            "Working-tree diff",
            UiAction::RunCommand("branch diff".into()),
        ),
        MenuItem::new(
            "Staged diff",
            UiAction::RunCommand("branch diff --staged".into()),
        ),
        MenuItem::new(
            "Stage selected paths…",
            UiAction::InsertInput("/branch stage path".into()),
        )
        .detail("edit the inserted command; separate paths with spaces"),
        MenuItem::new(
            "Unstage selected paths…",
            UiAction::InsertInput("/branch unstage path".into()),
        ),
        MenuItem::new(
            "Restore a file…",
            UiAction::InsertInput("/branch restore path".into()),
        )
        .badge("confirmation required"),
        MenuItem::new(
            "Create branch…",
            UiAction::InsertInput("/branch create name".into()),
        )
        .detail("edit the inserted command"),
        MenuItem::new("Recent log", UiAction::RunCommand("branch log".into())),
    ];
    items.extend(branches.iter().map(|branch| {
        MenuItem::new(
            format!("{} {}", if branch.current { "●" } else { " " }, branch.name),
            if branch.current {
                UiAction::RunCommand("branch status".into())
            } else {
                UiAction::RunCommand(format!("branch switch {}", branch.name))
            },
        )
        .badge(if branch.current { "current" } else { "local" })
        .detail(if branch.current {
            "current branch"
        } else {
            "Enter previews a confirmed switch; dirty trees are refused"
        })
    }));
    Menu::new("local git branches", items)
        .route("/branch")
        .searchable()
        .hint("Push/pull/PR operations stay in connector workflows")
}
