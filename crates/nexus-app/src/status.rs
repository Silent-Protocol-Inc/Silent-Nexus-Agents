//! The `/status` snapshot: every value is measured, never invented. Fields
//! that cannot be known are reported as such (`unknown`, `not running`).

use crate::app::App;
use crate::report::{Report, Sev};
use nexus_sandbox::NetworkMode;

/// Live facts only the running surface knows (the TUI's counters, or the
/// CLI's "no live session" defaults).
#[derive(Debug, Clone, Default)]
pub struct ActiveContext {
    pub session_id: Option<String>,
    pub tool_calls: u64,
    pub runtime_secs: u64,
    pub pending_approvals: u32,
    pub last_error: Option<String>,
}

/// Headline state of the governed self-improvement loop, for `/status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RsiFacts {
    pub candidates: usize,
    /// Candidates whose next step is a human decision.
    pub awaiting_human: usize,
    pub governance_version: u32,
}

/// One measured status snapshot.
#[derive(Debug, Clone)]
pub struct StatusSnapshot {
    pub session: Option<SessionFacts>,
    pub goal: Option<GoalFacts>,
    pub agent: String,
    /// The flagship agent identity (`nexus · Recursive Self-Improvement (RSI)`),
    /// a fixed product fact independent of which agent is currently active.
    pub flagship: String,
    /// Pending self-improvement proposals awaiting review, or `None` when
    /// surfacing is disabled (`[self_improvement].surface_pending = false`).
    pub self_improvement_pending: Option<usize>,
    /// Governed RSI candidates and how many of them are waiting on a human.
    /// `None` when the harness cannot be read.
    pub rsi: Option<RsiFacts>,
    /// The three presentation axes, as `thinking · narration · view`. They are
    /// easy to confuse, so status shows them together and never alone.
    pub presentation: String,
    pub model: Option<ModelFacts>,
    pub sandbox_backend: String,
    pub sandbox_level: String,
    pub network: String,
    pub mcp_total: usize,
    pub mcp_trusted: usize,
    pub workspace: String,
    /// Project instruction file in effect (e.g. `CLAUDE.md`), when present.
    pub instructions: Option<String>,
    pub git_branch: Option<String>,
    pub git_modified: Vec<String>,
    pub context: Option<ContextFacts>,
    pub tool_calls: u64,
    pub runtime_secs: u64,
    pub pending_approvals: u32,
    pub last_error: Option<String>,
    pub process: Option<ProcessFacts>,
    pub codex_source: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub struct SessionFacts {
    pub id: String,
    pub created_at: String,
    pub message_count: usize,
}

#[derive(Debug, Clone)]
pub struct GoalFacts {
    pub id: String,
    pub title: String,
    pub status: String,
    pub steps_used: i64,
    pub step_budget: i64,
}

#[derive(Debug, Clone)]
pub struct ModelFacts {
    pub name: String,
    pub provider: String,
    pub model_id: String,
    pub pinned: bool,
    pub auth_state: String,
    /// `Some((reachable, detail, latency_ms))` after a health probe.
    pub health: Option<(bool, String, Option<u64>)>,
}

#[derive(Debug, Clone)]
pub struct ContextFacts {
    pub used_tokens: usize,
    pub budget_tokens: usize,
}

#[derive(Debug, Clone)]
pub struct ProcessFacts {
    pub rss_mb: u64,
    pub load_1m: Option<f64>,
}

/// Gather the snapshot. `probe_health` performs one network round-trip to the
/// active model's endpoint; pass `false` for instant (cached-facts-only) use.
pub async fn snapshot(app: &App, active: &ActiveContext, probe_health: bool) -> StatusSnapshot {
    let sessions = app.sessions();
    let session = active
        .session_id
        .as_ref()
        .and_then(|id| sessions.get(id).ok())
        .map(|meta| SessionFacts {
            message_count: sessions
                .messages(meta.id.as_str())
                .map(|m| m.len())
                .unwrap_or(0),
            id: meta.id.as_str().to_string(),
            created_at: meta.created_at,
        });

    let goals = app.goals();
    let goal = app
        .read_ui_state(|s| s.active_goal.clone())
        .and_then(|id| goals.get(&id).ok())
        .or_else(|| {
            goals
                .list(Some(&app.workspace_key))
                .ok()
                .and_then(|mut list| {
                    list.retain(|g| !g.status.is_terminal());
                    list.into_iter().next()
                })
        })
        .map(|g| GoalFacts {
            id: g.id.as_str().to_string(),
            title: g.title,
            status: g.status.as_str().to_string(),
            steps_used: g.steps_used,
            step_budget: g.step_budget,
        });

    let model = model_facts(app, probe_health).await;

    let net = match app.config.sandbox.network.as_str() {
        "off" | "none" => NetworkMode::Off,
        "full" => NetworkMode::Full,
        _ => NetworkMode::Restricted,
    };
    let isolation = app.sandbox.backend().isolation(net);

    let (mcp_total, mcp_trusted) = app
        .mcp_registry()
        .list()
        .map(|list| {
            let trusted = list
                .iter()
                .filter(|s| s.trust.as_str() == "trusted")
                .count();
            (list.len(), trusted)
        })
        .unwrap_or((0, 0));

    let context = session.as_ref().and_then(|s| context_facts(app, &s.id));

    StatusSnapshot {
        session,
        goal,
        agent: app.active_agent(),
        flagship: format!(
            "{} · {} ({})",
            nexus_core::brand::FLAGSHIP_AGENT,
            nexus_core::brand::FLAGSHIP_MODE,
            nexus_core::brand::FLAGSHIP_MODE_SHORT,
        ),
        self_improvement_pending: if app.config.self_improvement.surface_pending {
            app.rsi().list(false).map(|p| p.len()).ok()
        } else {
            None
        },
        rsi: rsi_facts(app),
        presentation: format!(
            "{} thinking · {} narration · {} view",
            app.read_ui_state(|state| state.thinking()).as_str(),
            app.narration_mode().as_str(),
            nexus_core::timeline::ActivityMode::parse(
                &app.read_ui_state(|state| state.activity_mode.clone())
            )
            .unwrap_or_default()
            .as_str(),
        ),
        model,
        sandbox_backend: isolation.backend.clone(),
        sandbox_level: isolation.level.clone(),
        network: app.config.sandbox.network.clone(),
        mcp_total,
        mcp_trusted,
        workspace: app.workspace_key.clone(),
        instructions: nexus_core::instructions::load(&app.workspace).map(|i| {
            if i.also_present.is_empty() {
                i.source
            } else {
                format!("{} (also present: {})", i.source, i.also_present.join(", "))
            }
        }),
        git_branch: crate::gitx::branch(&app.workspace),
        git_modified: crate::gitx::modified_files(&app.workspace),
        context,
        tool_calls: active.tool_calls,
        runtime_secs: active.runtime_secs,
        pending_approvals: active.pending_approvals,
        last_error: active.last_error.clone(),
        process: process_facts(),
        codex_source: nexus_models::codex_auth::resolve_with_consent(
            app.read_ui_state(|s| s.codex_use_existing),
        )
        .ok()
        .flatten()
        .map(|(_, src)| src.label()),
    }
}

async fn model_facts(app: &App, probe_health: bool) -> Option<ModelFacts> {
    let name = app.any_model_name();
    let cfg = app.config.models.get(&name)?;
    let auth_state = if cfg.auth.as_deref() == Some("codex") || cfg.provider == "codex" {
        match nexus_models::codex_auth::resolve_with_consent(cfg.allow_existing_codex) {
            Ok(Some((_, src))) => format!("authenticated ({})", src.label()),
            _ => "login required".to_string(),
        }
    } else if let Some(env) = &cfg.api_key_env {
        if std::env::var(env).map(|v| !v.is_empty()).unwrap_or(false) {
            format!("API key from ${env}")
        } else {
            format!("API key required (${env} unset)")
        }
    } else if cfg.api_key_ref.is_some() {
        if cfg.resolved_api_key.is_some() {
            "API key from credential store".to_string()
        } else {
            "API key required (credential missing)".to_string()
        }
    } else {
        "no authentication required".to_string()
    };

    let health = if probe_health {
        match nexus_models::ModelManager::from_config(&app.config) {
            Ok(manager) => match manager.get(&name) {
                Ok(provider) => {
                    let h = provider.health().await;
                    Some((h.reachable, h.detail, h.latency_ms))
                }
                Err(e) => Some((false, e.to_string(), None)),
            },
            Err(e) => Some((false, e.to_string(), None)),
        }
    } else {
        None
    };

    Some(ModelFacts {
        pinned: app.pinned_model.as_deref() == Some(name.as_str()),
        provider: cfg.provider.clone(),
        model_id: cfg.model.clone(),
        name,
        auth_state,
        health,
    })
}

/// Whether a candidate's next legal step is a decision no automated stage may
/// take for it. `Testing`, `Shadow`, and `Canary` are WARP's to advance;
/// `Proposed` and `Validated` are the two points where a human decides.
pub fn awaits_human(status: nexus_core::harness::ImprovementStatus) -> bool {
    use nexus_core::harness::ImprovementStatus as S;
    matches!(status, S::Proposed | S::Validated)
}

/// Count governed candidates and how many of them are waiting on a human.
fn rsi_facts(app: &App) -> Option<RsiFacts> {
    let proposals = app
        .harness()
        .workspace_repository()
        .improvement_proposals(None)
        .ok()?;
    let awaiting_human = proposals.iter().filter(|p| awaits_human(p.status)).count();
    Some(RsiFacts {
        candidates: proposals.len(),
        awaiting_human,
        governance_version: nexus_core::governance::GOVERNANCE_VERSION,
    })
}

fn context_facts(app: &App, session_id: &str) -> Option<ContextFacts> {
    let messages = app.sessions().messages(session_id).ok()?;
    let used: usize = messages
        .iter()
        .map(nexus_context::estimate_message_tokens)
        .sum();
    let model = app.any_model_name();
    let window = app
        .config
        .models
        .get(&model)
        .map(|m| m.context_window)
        .unwrap_or(8192);
    Some(ContextFacts {
        used_tokens: used,
        budget_tokens: window,
    })
}

/// Best-effort process facts from /proc (Linux); `None` elsewhere.
fn process_facts() -> Option<ProcessFacts> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    let page_size = 4096u64; // universal default on Linux
    let load_1m = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(String::from))
        .and_then(|s| s.parse().ok());
    Some(ProcessFacts {
        rss_mb: pages * page_size / (1024 * 1024),
        load_1m,
    })
}

/// Render the snapshot as a report (CLI output; the TUI has a richer panel).
pub fn to_report(s: &StatusSnapshot) -> Report {
    let mut r = Report::new("status");
    match &s.session {
        Some(sess) => {
            r = r
                .field("session", &sess.id)
                .field("messages", sess.message_count.to_string())
        }
        None => r = r.field_sev("session", "none active", Sev::Dim),
    }
    match &s.goal {
        Some(g) => {
            r = r.field(
                "goal",
                format!(
                    "{} [{}] {}/{} steps — {}",
                    g.id, g.status, g.steps_used, g.step_budget, g.title
                ),
            )
        }
        None => r = r.field_sev("goal", "none", Sev::Dim),
    }
    r = r.field("agent", &s.agent);
    r = r.field("flagship", &s.flagship);
    if let Some(pending) = s.self_improvement_pending {
        if pending > 0 {
            r = r.field(
                "self-improvement",
                format!("{pending} pending proposal(s) — review with 'snx profile'"),
            );
        }
    }
    r = r.field("presentation", &s.presentation);
    if let Some(rsi) = &s.rsi {
        if rsi.candidates > 0 {
            let text = format!(
                "{} candidate(s), {} awaiting a human — /rsi (governance v{})",
                rsi.candidates, rsi.awaiting_human, rsi.governance_version
            );
            r = if rsi.awaiting_human > 0 {
                r.field_sev("rsi", text, Sev::Warn)
            } else {
                r.field("rsi", text)
            };
        }
    }
    match &s.model {
        Some(m) => {
            let pin = if m.pinned { " (pinned)" } else { "" };
            r = r.field(
                "model",
                format!("{} — {} / {}{}", m.name, m.provider, m.model_id, pin),
            );
            let auth_sev = if m.auth_state.contains("required") {
                Sev::Warn
            } else {
                Sev::Ok
            };
            r = r.field_sev("auth", &m.auth_state, auth_sev);
            if let Some((ok, detail, latency)) = &m.health {
                let lat = latency.map(|l| format!(" {l}ms")).unwrap_or_default();
                r = r.field_sev(
                    "endpoint",
                    format!("{detail}{lat}"),
                    if *ok { Sev::Ok } else { Sev::Err },
                );
            }
        }
        None => r = r.field_sev("model", "not configured — run /connect", Sev::Warn),
    }
    if let Some(src) = s.codex_source {
        r = r.field("codex auth", src);
    }
    let sandbox = if s.sandbox_backend == s.sandbox_level {
        s.sandbox_level.clone()
    } else {
        format!("{} — {}", s.sandbox_backend, s.sandbox_level)
    };
    r = r
        .field("sandbox", sandbox)
        .field("network", &s.network)
        .field(
            "mcp",
            format!("{} server(s), {} trusted", s.mcp_total, s.mcp_trusted),
        )
        .field("workspace", &s.workspace);
    match &s.instructions {
        Some(i) => r = r.field("instructions", i),
        None => {
            r = r.field_sev(
                "instructions",
                "none (add SILENT.md or AGENTS.md)",
                Sev::Dim,
            )
        }
    }
    match &s.git_branch {
        Some(b) => {
            r = r.field(
                "git",
                format!("{b} — {} modified file(s)", s.git_modified.len()),
            )
        }
        None => r = r.field_sev("git", "not a repository", Sev::Dim),
    }
    if let Some(c) = &s.context {
        let pct = (c.used_tokens * 100)
            .checked_div(c.budget_tokens)
            .unwrap_or(0);
        r = r.field(
            "context",
            format!("≈{} / {} tokens ({pct}%)", c.used_tokens, c.budget_tokens),
        );
    }
    r = r
        .field("tool calls", s.tool_calls.to_string())
        .field("runtime", format!("{}s", s.runtime_secs))
        .field("pending approvals", s.pending_approvals.to_string());
    if let Some(e) = &s.last_error {
        r = r.field_sev("last error", e, Sev::Err);
    }
    if let Some(p) = &s.process {
        let load = p
            .load_1m
            .map(|l| format!(", load {l:.2}"))
            .unwrap_or_default();
        r = r.field("process", format!("{} MB RSS{load}", p.rss_mb));
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::harness::ImprovementStatus as S;

    #[test]
    fn only_the_two_decision_points_await_a_human() {
        for status in [S::Proposed, S::Validated] {
            assert!(awaits_human(status), "{status:?} needs a human");
        }
        // WARP advances these itself; counting them would nag the operator
        // about work that is not theirs.
        for status in [
            S::Observed,
            S::Draft,
            S::Approved,
            S::Testing,
            S::Shadow,
            S::Canary,
            S::Promoted,
            S::Rejected,
            S::RolledBack,
            S::Deprecated,
        ] {
            assert!(!awaits_human(status), "{status:?} must not await a human");
        }
    }
}
