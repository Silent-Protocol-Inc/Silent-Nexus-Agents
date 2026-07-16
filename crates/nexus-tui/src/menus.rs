//! Builders that turn real service data into interactive overlays. No fake
//! rows: every entry is constructed from measured provider/goal/session state.

use crate::views::{LoadRequest, Menu, MenuItem, SecretInput, SecretTarget, UiAction};
use nexus_agent::AgentRole;
use nexus_app::codex::CodexStatus;
use nexus_app::providers::{AuthRequirement, EndpointState, ProviderEntry};
use nexus_app::services::ResumeCandidate;
use nexus_core::brand::{self, BrandVariant};
use nexus_goals::Goal;

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
    Menu::new("goal", items).hint("Enter select · Esc close")
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
    let mut menu = Menu::new("goals", items).searchable();
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
    let mut menu = Menu::new("resume", items).searchable();
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
    let mut menu = Menu::new("sessions", items).searchable();
    menu.hint = "Enter attaches the session · Esc close".into();
    menu.on_refresh = Some(UiAction::Load(LoadRequest::Sessions));
    menu
}

// ------------------------------------------------------------------- welcome

/// First-run onboarding menu, shown when no models are configured yet.
pub fn welcome_menu() -> Menu {
    let items = vec![
        MenuItem::new("Run /setup (recommended)", UiAction::RunCommand("setup".into()))
            .detail("detects local runtimes (Ollama / llama.cpp), your Codex session, and writes a starter config"),
        MenuItem::new("Connect a provider", UiAction::Load(LoadRequest::Login))
            .detail("sign in with ChatGPT (Codex), enter an API key, or point at a local server"),
        MenuItem::new("Just look around", UiAction::RunCommand("help".into()))
            .detail("keys and commands — you can run /setup any time"),
    ];
    let mut menu =
        Menu::new(format!("Welcome to {}", brand::MARK), items).branded(BrandVariant::Compact);
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
    Menu::new("initialize project", items).hint("Preview and confirm mutations · Esc close")
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
    let mut menu = Menu::new(format!("{} · connect provider", brand::MARK), items)
        .branded(BrandVariant::Compact)
        .searchable();
    menu.hint = "Enter opens the provider's auth flow · r refresh · Esc close".into();
    menu.on_refresh = Some(UiAction::Load(LoadRequest::Login));
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

/// The /model picker: every model available RIGHT NOW, across configured
/// entries, the codex plan (when connected), and reachable local runtimes.
/// Providers that need connecting first point at /connect instead of faking
/// availability.
pub fn model_menu(
    configured: &[(String, String, String)], // (config name, provider, model id)
    entries: &[ProviderEntry],
    active_model: &str,
) -> Menu {
    let mut items: Vec<MenuItem> = Vec::new();

    // Configured entries first — Enter pins routing (codex ones go through
    // the effort picker).
    for (name, provider, model_id) in configured {
        let mark = if name == active_model { "●" } else { " " };
        let action = if provider == "codex" {
            UiAction::PickCodexEffort {
                model_id: model_id.clone(),
            }
        } else {
            UiAction::SelectModel(name.clone())
        };
        items.push(
            MenuItem::new(format!("{mark} {name}"), action)
                .badge(provider.clone())
                .detail(format!("model {model_id} — Enter pins routing")),
        );
    }

    // Codex plan models (already fetched/cached) not yet configured.
    let codex_connected = entries.iter().any(|e| e.id == "codex" && e.authenticated);
    if codex_connected {
        let plan = nexus_app::codex::cached_plan_models();
        for m in &plan {
            if configured
                .iter()
                .any(|(_, p, id)| p == "codex" && id == &m.id)
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
                if configured.iter().any(|(_, _, id)| id == &m.id) {
                    continue;
                }
                items.push(
                    MenuItem::new(
                        format!("  {}", m.id),
                        UiAction::UseDiscovered {
                            provider: e.id.clone(),
                            base_url: e.endpoint.clone().unwrap_or_default(),
                            model_id: m.id.clone(),
                        },
                    )
                    .badge(e.label.clone())
                    .detail(format!(
                        "discovered on {}",
                        e.endpoint.as_deref().unwrap_or("endpoint")
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
    let mut menu = Menu::new("model — pick the active model", items).searchable();
    menu.hint = "Enter select · r refresh · Esc close · /connect adds providers".into();
    menu.on_refresh = Some(UiAction::Load(LoadRequest::Model));
    menu
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
    let mut menu = Menu::new(format!("{} — reasoning effort", model.display_name), items);
    menu.hint = "Enter selects model + effort · Esc back".into();
    menu
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
    if let EndpointState::Connected { models, latency_ms } = &entry.state {
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
            } else {
                UiAction::UseDiscovered {
                    provider: entry.id.clone(),
                    base_url: entry.endpoint.clone().unwrap_or_default(),
                    model_id: m.id.clone(),
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
    Menu::new("agents", items).hint("Enter sets the active agent for new sessions · Esc close")
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
        .branded(BrandVariant::Compact)
        .searchable()
        .hint("Enter selects for new sessions · create/clone/edit/delete also work as commands")
}

pub fn profile_menu(active_profile: &str, traits: &[nexus_memory::ProfileTrait]) -> Menu {
    let mut items = vec![
        MenuItem::new(
            format!("Active profile: {active_profile}"),
            UiAction::InsertInput(format!("/profile select {active_profile}")),
        )
        .badge("selected")
        .detail("edit the inserted name to switch or create a profile namespace"),
        MenuItem::new(
            "Add explicit workflow preference…",
            UiAction::InsertInput("/profile add key value".into()),
        )
        .detail("explicit low-risk workflow traits are approved automatically"),
        MenuItem::new(
            "Review RSI proposals",
            UiAction::RunCommand("profile proposals".into()),
        )
        .detail("skills, tools, connectors, config, and source changes require review"),
    ];
    for record in traits {
        if record.status == "pending" {
            items.push(
                MenuItem::new(
                    format!("Approve {} = {}", record.trait_key, record.trait_value),
                    UiAction::RunCommand(format!("profile approve {}", record.id)),
                )
                .badge(format!("{:.0}%", record.confidence * 100.0))
                .detail(format!(
                    "{} · evidence: {}",
                    record.sensitivity, record.evidence
                )),
            );
            items.push(
                MenuItem::new(
                    format!("Reject {}", record.trait_key),
                    UiAction::RunCommand(format!("profile reject {}", record.id)),
                )
                .badge("pending"),
            );
        } else if record.status == "approved" {
            items.push(
                MenuItem::new(
                    format!("{} = {}", record.trait_key, record.trait_value),
                    UiAction::RunCommand(format!("profile delete {}", record.id)),
                )
                .badge("approved")
                .detail("Enter opens a deletion confirmation"),
            );
        }
    }
    Menu::new("profile traits", items)
        .branded(BrandVariant::Compact)
        .searchable()
        .hint("Pending/sensitive traits require review · Enter action · Esc close")
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
            "  Show effective policy…",
            UiAction::RunCommand("permissions show".into()),
        )
        .detail("every decision, allow/deny lists, and denied paths".to_string()),
    );
    Menu::new("permissions — approval mode", items)
        .hint("Enter applies and persists · destructive actions always ask · Esc close")
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
    Menu::new(title, items).hint("Enter applies and persists · Esc close")
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
    Menu::new("theme", items).hint("Enter applies immediately and persists · Esc close")
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
    Menu::new("timeline details", items).hint("Enter applies · Esc close")
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
    Menu::new("timeline filter", items).hint("Enter applies · Esc close")
}

pub fn config_menu() -> Menu {
    Menu::new(
        "configuration hub",
        vec![
            MenuItem::new("Models", UiAction::Load(LoadRequest::Model))
                .detail("provider/model selection and endpoint health"),
            MenuItem::new("Permissions", UiAction::Load(LoadRequest::Permissions))
                .detail("read-only, default, auto-edit, full-access"),
            MenuItem::new("Sandbox", UiAction::Load(LoadRequest::Sandbox))
                .detail("backend, isolation, and network mode"),
            MenuItem::new("Memory", UiAction::Load(LoadRequest::Memory))
                .detail("approved durable project/global memory"),
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
    .branded(BrandVariant::Compact)
    .hint("Managed choices are written to override layers; hand-written config is preserved")
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
        .searchable()
        .hint("Push/pull/PR operations stay in connector workflows")
}
