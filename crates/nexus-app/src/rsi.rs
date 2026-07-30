//! Reports for `/rsi` — the governed self-improvement surface.
//!
//! Everything here is read-only. The commands that *change* a candidate's state
//! go through the WARP pipeline and the promotion gate; this module only shows
//! what those decided, so an operator can inspect the queue without becoming a
//! way around it.
//!
//! One deliberate choice: the candidate list shows WARP's **classified** risk
//! tier next to the tier the candidate declared for itself. When they differ,
//! the classified one is the one that governs, and seeing both is how an
//! operator notices a candidate that undersold its own blast radius.

use crate::app::App;
use crate::report::{Report, Sev};
use nexus_core::governance::GOVERNANCE_RULES;
use nexus_core::harness::{ImprovementProposal, ImprovementStatus};
use nexus_core::Result;
use nexus_warp::risk::RiskClassifier;
use nexus_warp::rollback::PromotionLedger;

/// Rows shown in list views before truncation.
const LIST_LIMIT: usize = 30;

/// Headline health of the self-improvement loop.
pub fn status_report(app: &App) -> Result<Report> {
    let config = &app.config.self_improvement;
    let proposals = app
        .harness()
        .workspace_repository()
        .improvement_proposals(None)?;

    let count = |status: ImprovementStatus| {
        proposals
            .iter()
            .filter(|p| p.status == status)
            .count()
            .to_string()
    };

    let mut report = Report::new("rsi — governed self-improvement")
        .field(
            "observation",
            if config.enabled {
                "on — finished turns are analysed"
            } else {
                "off — no observations are recorded"
            },
        )
        .field("governance rules", GOVERNANCE_RULES.len().to_string())
        .field(
            "governance version",
            nexus_core::governance::GOVERNANCE_VERSION.to_string(),
        )
        .header("candidate queue")
        .field("observed", count(ImprovementStatus::Observed))
        .field("proposed", count(ImprovementStatus::Proposed))
        .field("testing", count(ImprovementStatus::Testing))
        .field("validated", count(ImprovementStatus::Validated))
        .field("shadow", count(ImprovementStatus::Shadow))
        .field("canary", count(ImprovementStatus::Canary))
        .field("promoted", count(ImprovementStatus::Promoted))
        .field("rejected", count(ImprovementStatus::Rejected));

    let awaiting = proposals
        .iter()
        .filter(|p| {
            matches!(
                p.status,
                ImprovementStatus::Proposed | ImprovementStatus::Validated
            )
        })
        .count();
    if awaiting > 0 {
        report = report.line_sev(
            format!("{awaiting} candidate(s) waiting on a human decision — /rsi candidates"),
            Sev::Warn,
        );
    }

    let ledger = PromotionLedger::new(app.store.clone());
    match ledger.latest(&app.workspace_key)? {
        Some(promotion) => {
            report = report
                .header("last promotion")
                .field("candidate", promotion.candidate_id)
                .field("version", promotion.version)
                .field("authorised by", promotion.promoted_by)
                .field("rollback", promotion.rollback_command)
                .field("at", promotion.promoted_at);
        }
        None => report = report.line("no promotions recorded in this workspace"),
    }
    Ok(report)
}

/// The candidate queue, newest first.
pub fn candidates_report(app: &App) -> Result<Report> {
    let proposals = app
        .harness()
        .workspace_repository()
        .improvement_proposals(None)?;
    if proposals.is_empty() {
        return Ok(Report::new("rsi candidates").warn(
            "no candidates yet — they appear once repeated evidence supports one (/rsi status)",
        ));
    }
    let shown = proposals.len().min(LIST_LIMIT);
    let rows = proposals
        .iter()
        .take(LIST_LIMIT)
        .map(|p| {
            let assessment = RiskClassifier::classify(p);
            vec![
                p.id.clone(),
                p.status.as_str().to_string(),
                p.target.as_str().to_string(),
                tier_cell(p, &assessment.tier),
                truncate(&p.problem, 48),
                p.updated_at.clone(),
            ]
        })
        .collect();
    let mut report = Report::new("rsi candidates").table(
        &["id", "status", "target", "tier", "problem", "updated"],
        rows,
    );
    if proposals.len() > shown {
        report = report.line(format!(
            "showing {shown} of {} — /rsi show <id> for one candidate",
            proposals.len()
        ));
    }
    Ok(report)
}

/// One candidate, with WARP's classification and its declared success metrics.
pub fn candidate_show_report(app: &App, id: &str) -> Result<Report> {
    let proposal = app.harness().workspace_repository().improvement(id)?;
    let assessment = RiskClassifier::classify(&proposal);

    let mut report = Report::new(format!("candidate {}", proposal.id))
        .field("status", proposal.status.as_str())
        .field("target", proposal.target.as_str())
        .field(
            "plane",
            match proposal.target.plane() {
                nexus_core::harness::ImprovementPlane::Data => "data — applied after WARP",
                nexus_core::harness::ImprovementPlane::Code => {
                    "code — ships through a human-approved release"
                }
            },
        )
        .field("declared tier", assessment.declared_tier.as_str())
        .field_sev(
            "classified tier",
            assessment.tier.as_str(),
            if assessment.is_prohibited() {
                Sev::Err
            } else if assessment.requires_human_approval() {
                Sev::Warn
            } else {
                Sev::Info
            },
        )
        .field("created by", &proposal.created_by)
        .field("created", &proposal.created_at);

    if let Some(reviewed) = &proposal.reviewed_at {
        report = report.field("reviewed", reviewed);
    }

    report = report.header("problem").line(&proposal.problem);
    if !proposal.root_cause_hypothesis.trim().is_empty() {
        report = report
            .header("root cause hypothesis")
            .line(&proposal.root_cause_hypothesis);
    }
    report = report
        .header("proposed change")
        .line(&proposal.proposed_change);

    if !proposal.success_metrics.is_empty() {
        report = report.header("success metrics");
        for metric in &proposal.success_metrics {
            report = report.line(format!(
                "{} {} — {}",
                if metric.hard_constraint { "◆" } else { "·" },
                metric.id,
                metric.description
            ));
        }
        report = report.line("◆ = hard constraint: a miss is a veto, never averaged away");
    }

    if !proposal.affected_components.is_empty() {
        report = report
            .header("affected components")
            .line(proposal.affected_components.join(", "));
    }

    report = report.header("warp classification");
    for reason in &assessment.rationale {
        report = report.line(format!("· {reason}"));
    }
    if !assessment.governance.permits() {
        report = report.line_sev("governance refuses this candidate:", Sev::Err);
        for violation in assessment.governance.describe() {
            report = report.line_sev(format!("  ✖ {violation}"), Sev::Err);
        }
    }
    Ok(report)
}

/// Recent RSI observations from the harness event log.
pub fn observations_report(app: &App) -> Result<Report> {
    let events = app
        .harness()
        .workspace_repository()
        .events(None, None, None, None, None, None, LIST_LIMIT)?;
    let rsi_events: Vec<_> = events
        .into_iter()
        .filter(|e| e.event_type.starts_with("rsi."))
        .collect();
    if rsi_events.is_empty() {
        return Ok(Report::new("rsi observations")
            .warn("no RSI observations recorded yet — they accumulate as turns finish"));
    }
    let rows = rsi_events
        .iter()
        .map(|e| {
            vec![
                e.timestamp.clone(),
                e.event_type.clone(),
                e.severity.clone(),
                truncate(&e.summary, 60),
            ]
        })
        .collect();
    Ok(Report::new("rsi observations")
        .table(&["at", "event", "severity", "summary"], rows)
        .line("observations are redacted before storage"))
}

/// Recent multi-dimensional outcome records.
pub fn outcomes_report(app: &App) -> Result<Report> {
    let store = nexus_rsi::OutcomeStore::new(app.store.clone());
    let outcomes = store.recent(&app.workspace_key, LIST_LIMIT)?;
    if outcomes.is_empty() {
        return Ok(Report::new("rsi outcomes").warn("no scored outcomes yet"));
    }
    let rows = outcomes
        .iter()
        .map(|o| {
            vec![
                o.created_at.clone(),
                o.session_id.clone().unwrap_or_else(|| "-".into()),
                o.completion_status.clone(),
                o.final_score
                    .map(|s| format!("{s:.2}"))
                    .unwrap_or_else(|| "—".into()),
                format!("{:.2}", o.confidence),
            ]
        })
        .collect();
    Ok(Report::new("rsi outcomes")
        .table(&["at", "session", "status", "score", "confidence"], rows)
        .line("scores come from evidence, weakest tier being the agent's own assessment"))
}

/// Promotions recorded for this workspace.
pub fn promotions_report(app: &App) -> Result<Report> {
    let ledger = PromotionLedger::new(app.store.clone());
    match ledger.latest(&app.workspace_key)? {
        None => Ok(Report::new("rsi promotions").warn("no promotions recorded in this workspace")),
        Some(promotion) => Ok(Report::new("rsi promotions")
            .field("candidate", promotion.candidate_id)
            .field("version", promotion.version)
            .field("parent", promotion.parent_version)
            .field("authorised by", promotion.promoted_by)
            .field(
                "governance version",
                promotion.governance_version.to_string(),
            )
            .field("rollback", promotion.rollback_command)
            .field("at", promotion.promoted_at)),
    }
}

/// Rollbacks recorded against the most recent promotion.
pub fn rollbacks_report(app: &App) -> Result<Report> {
    let ledger = PromotionLedger::new(app.store.clone());
    let Some(promotion) = ledger.latest(&app.workspace_key)? else {
        return Ok(Report::new("rsi rollbacks").warn("no promotions, and so no rollbacks"));
    };
    let rollbacks = ledger.rollbacks(&promotion.id)?;
    if rollbacks.is_empty() {
        return Ok(Report::new("rsi rollbacks").ok(format!(
            "promotion {} has not been rolled back",
            promotion.id
        )));
    }
    let rows = rollbacks
        .iter()
        .map(|r| {
            vec![
                r.rolled_back_at.clone(),
                r.trigger.as_str().to_string(),
                r.restored_version.clone(),
                truncate(&r.detail, 60),
            ]
        })
        .collect();
    Ok(Report::new("rsi rollbacks").table(&["at", "trigger", "restored", "detail"], rows))
}

/// The governance constitution, as compiled into the binary.
pub fn governance_report() -> Report {
    let mut report = Report::new("rsi governance")
        .field(
            "version",
            nexus_core::governance::GOVERNANCE_VERSION.to_string(),
        )
        .field("rules", GOVERNANCE_RULES.len().to_string())
        .line("compile-time and not editable at runtime — changing these requires a release")
        .header("rules");
    for rule in GOVERNANCE_RULES {
        report = report.line(format!("· {} — {}", rule.id, rule.statement));
    }
    report
        .header("prohibited for autonomous change")
        .line(nexus_core::governance::PROTECTED_COMPONENTS.join(", "))
}

/// `declared → classified` when the classifier raised it, otherwise the tier.
fn tier_cell(proposal: &ImprovementProposal, classified: &nexus_core::harness::RiskTier) -> String {
    if proposal.risk_tier == *classified {
        classified.as_str().to_string()
    } else {
        format!("{} → {}", proposal.risk_tier.as_str(), classified.as_str())
    }
}

fn truncate(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    let kept: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_keeps_the_first_line_and_marks_the_cut() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("first\nsecond", 10), "first");
        assert_eq!(truncate("0123456789abc", 5), "0123…");
    }

    #[test]
    fn the_tier_cell_shows_a_raise_and_hides_agreement() {
        let mut p =
            ImprovementProposal::new(nexus_core::harness::ImprovementCategory::Tool, "p", "c")
                .expect("proposal");
        p.risk_tier = nexus_core::harness::RiskTier::Low;
        assert_eq!(
            tier_cell(&p, &nexus_core::harness::RiskTier::High),
            "low → high"
        );
        assert_eq!(tier_cell(&p, &nexus_core::harness::RiskTier::Low), "low");
    }

    #[test]
    fn the_governance_report_lists_every_rule() {
        let report = governance_report();
        let rendered = format!("{report:?}");
        for rule in GOVERNANCE_RULES {
            assert!(rendered.contains(rule.id), "missing rule {}", rule.id);
        }
    }
}
